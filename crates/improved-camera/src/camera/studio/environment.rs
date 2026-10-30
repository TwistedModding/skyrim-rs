//! `camera/studio/StudioEnvironment`: the game's image-space modifiers held for the shot, and the
//! sun's angle set directly (re-asserted after every `Sky::Update`, which recomputes it from the
//! hour, yielding to another plugin that writes it after this one). Both are released when the
//! studio closes.

use std::sync::Mutex;

use skyrim::forms::{TESDataHandlerExt, TESFormExt as _};
use skyrim::render::NiAVObjectExt;
use sre::{FormType, NiAVObject, TESImageSpaceModifier};

use crate::util::point::{self, P3};

/// One modifier of the browser: its form id and a label (the editor id when the form keeps one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImodInfo {
    pub form_id: u32,
    pub label: String,
}

/// `BuildImodList`: every `TESImageSpaceModifier`, sorted by label.
#[must_use]
pub fn imods() -> Vec<ImodInfo> {
    let Some(data) = sre::TESDataHandler::singleton() else { return Vec::new() };
    let mut list: Vec<ImodInfo> = data
        .forms_of_type(FormType::IMAD)
        .iter()
        .flatten()
        .map(|form| ImodInfo { form_id: form.formID, label: form.editor_id().map_or_else(|| format!("{:08X}", form.formID), |id| id.to_string_lossy().into_owned()) })
        .collect();
    list.sort_by(|a, b| a.label.cmp(&b.label));
    list
}

fn imod(form_id: u32) -> Option<&'static TESImageSpaceModifier> {
    let form = sre::find_form_by_id(form_id).filter(|f| f.formType == FormType::IMAD as u8)?;
    Some(unsafe { &*core::ptr::from_ref(form).cast::<TESImageSpaceModifier>() })
}

/// `SetImod`'s game side: the previous modifier stopped, the new one started at `strength`
/// (restarted rather than tracked: a modifier with a duration ends on its own). Main thread.
pub fn restart_imod(previous: u32, next: u32, strength: f32) {
    if previous != 0 {
        sre::image_space_modifier__remove(imod(previous));
    }
    if let Some(form) = imod(next) {
        sre::image_space_modifier__apply(Some(form), strength, None);
    }
}

/// `SunOverride`: the direction toward the sun as a compass azimuth (0 north, +Y, clockwise) and an
/// elevation above the horizon, degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunOverride {
    pub enabled: bool,
    pub azimuth: f32,
    pub elevation: f32,
}

impl Default for SunOverride {
    fn default() -> Self {
        Self { enabled: false, azimuth: 135.0, elevation: 20.0 }
    }
}

/// The light's column 0: the direction the light travels, from the sun toward the ground.
#[must_use]
pub fn sun_column(sun: &SunOverride) -> P3 {
    let azimuth = sun.azimuth.to_radians();
    let elevation = sun.elevation.clamp(-20.0, 90.0).to_radians();
    [-(azimuth.sin() * elevation.cos()), -(azimuth.cos() * elevation.cos()), -elevation.sin()]
}

#[derive(Default)]
struct Sun {
    wanted: SunOverride,
    contested: bool,
    written: Option<P3>,
}

/// Written from the menu on the main thread, read by the sky hook on a job thread.
static SUN: Mutex<Sun> = Mutex::new(Sun { wanted: SunOverride { enabled: false, azimuth: 135.0, elevation: 20.0 }, contested: false, written: None });

fn sun<R>(f: impl FnOnce(&mut Sun) -> R) -> R {
    f(&mut SUN.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

#[must_use]
pub fn get_sun() -> SunOverride {
    sun(|s| s.wanted)
}

/// `SetSun`: enabling again gives a lost contest another chance.
pub fn set_sun(wanted: SunOverride) {
    sun(|s| {
        if wanted.enabled && !s.wanted.enabled {
            s.contested = false;
        }
        if !wanted.enabled {
            s.written = None;
        }
        s.wanted = wanted;
    });
}

/// `SunContested`: another plugin wrote the sun after us and the override yielded.
#[must_use]
pub fn sun_contested() -> bool {
    sun(|s| s.contested)
}

/// `ApplySun`, after `Sky::Update`: the override's direction in the sun light's local rotation
/// column 0, which the shadows read. A column that moved since our last write was written by
/// another hook after ours, and the override yields rather than fight it every frame.
pub fn apply_sun(sky: &sre::Sky) {
    let Some(light) = (unsafe { sky.sun.as_ref() }).and_then(|s| unsafe { s.light.as_ref().map(|l| -> &NiAVObject { l }) }) else {
        sun(|s| s.written = None);
        return;
    };
    sun(|s| {
        if !s.wanted.enabled || s.contested {
            return;
        }
        let mut local = light.local_transform();
        let current = [local.rotation[0][0], local.rotation[1][0], local.rotation[2][0]];
        if let Some(written) = s.written
            && point::length(point::sub(current, written)) > 0.01
        {
            s.contested = true;
            s.written = None;
            crate::log("<Photo Mode> Sun override yielded: another plugin is writing the sun light");
            return;
        }
        let column = sun_column(&s.wanted);
        for (row, value) in local.rotation.iter_mut().zip(column) {
            row[0] = value;
        }
        light.set_local_transform(local);
        light.update_transforms();
        s.written = Some(column);
    });
}

/// `OnStudioExit` and `ClearSession` for the sun: the override off, so the next `Sky::Update`
/// writes the hour's direction again.
pub fn release_sun() {
    set_sun(SunOverride { enabled: false, ..get_sun() });
}

#[cfg(test)]
mod tests {
    use super::{SunOverride, sun_column};

    #[test]
    fn the_column_points_from_the_sun_to_the_ground() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 0.0, elevation: 0.0 }), [0.0, -1.0, 0.0]), "north on the horizon lights toward the south");
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: 90.0 }), [0.0, 0.0, -1.0]));
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: -60.0 }), sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: -20.0 })), "clamped below the horizon");
    }
}
