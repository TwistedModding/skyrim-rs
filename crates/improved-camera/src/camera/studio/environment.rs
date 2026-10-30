//! `camera/studio/StudioEnvironment`: time of day and weather, the game's image-space modifiers
//! held for the shot, and the sun's angle set directly (re-asserted after every `Sky::Update`,
//! which recomputes it from the hour, the disc and glare moved with it, yielding to another plugin
//! that writes it after this one). A forced weather, the modifier and the sun are released when the
//! studio closes.

use std::sync::Mutex;

use skyrim::forms::{TESDataHandlerExt, TESFileExt, TESFormExt, local_form_id};
use skyrim::render::{self as render, NiAVObjectExt, SkyExt, SunNodes};
use sre::{FormType, NiAVObject, TESForm, TESImageSpaceModifier, TESWeather};

use crate::util::point::{self, P3};

/// One modifier of the browser: its form id and its [`form_label`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImodInfo {
    pub form_id: u32,
    pub label: String,
}

/// `FormLabel`: the editor id when the form keeps one, else `Plugin[local id]` (`Dynamic[id]`
/// for a form created at runtime).
#[must_use]
pub fn form_label(form: &TESForm) -> String {
    if let Some(id) = form.editor_id() {
        return id.to_string_lossy().into_owned();
    }
    match form.get_file(0) {
        Some(file) => format!("{}[{:06X}]", file.name(), local_form_id(form.formID)),
        None => format!("Dynamic[{:08X}]", form.formID),
    }
}

/// `FormKey`: `Plugin|LOCALID`, the load-order independent name scenes keep; empty for a form
/// without a file.
#[must_use]
pub fn form_key(form_id: u32) -> String {
    sre::find_form_by_id(form_id).and_then(|f| f.get_file(0)).map_or_else(String::new, |file| format!("{}|{:08X}", file.name(), local_form_id(form_id)))
}

/// `ResolveFormKey`: the runtime id of a [`form_key`], 0 when its plugin is not loaded or the
/// form is gone.
#[must_use]
pub fn resolve_form_key(key: &str) -> u32 {
    let Some((plugin, local)) = super::io::parse_form_key(key) else { return 0 };
    sre::TESDataHandler::singleton()
        .and_then(|d| d.file(plugin))
        .and_then(|f| f.runtime_form_id(local))
        .filter(|&id| sre::find_form_by_id(id).is_some())
        .unwrap_or(0)
}

fn sorted_by_label<T>(mut list: Vec<T>, label: impl Fn(&T) -> &str) -> Vec<T> {
    list.sort_by(|a, b| label(a).cmp(label(b)));
    list
}

/// `BuildImodList`: every `TESImageSpaceModifier`, sorted by label.
#[must_use]
pub fn imods() -> Vec<ImodInfo> {
    let Some(data) = sre::TESDataHandler::singleton() else { return Vec::new() };
    let list = data.forms_of_type(FormType::IMAD).iter().flatten().map(|form| ImodInfo { form_id: form.formID, label: form_label(form) }).collect();
    sorted_by_label(list, |i: &ImodInfo| &i.label)
}

/// `Kind`: a weather's class from its engine flags; precipitation wins, since that is what changes
/// the shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clear,
    Cloudy,
    Rainy,
    Snowy,
    Other,
}

impl Kind {
    /// `ClassifyWeather` over `TESWeather`+0x66f (bit 0 pleasant, 1 cloudy, 2 rainy, 3 snow).
    #[must_use]
    pub const fn of(flags: u8) -> Self {
        if flags & 0x8 != 0 {
            Self::Snowy
        } else if flags & 0x4 != 0 {
            Self::Rainy
        } else if flags & 0x2 != 0 {
            Self::Cloudy
        } else if flags & 0x1 != 0 {
            Self::Clear
        } else {
            Self::Other
        }
    }

    /// `KindName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clear => "Clear",
            Self::Cloudy => "Cloudy",
            Self::Rainy => "Rain",
            Self::Snowy => "Snow",
            Self::Other => "Other",
        }
    }
}

/// One weather of the picker (`WeatherInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherInfo {
    pub form_id: u32,
    pub label: String,
    pub kind: Kind,
}

/// `BuildWeatherList`: every `TESWeather`, sorted by label (weathers keep no editor id on this
/// runtime, so the label is the plugin and local id).
#[must_use]
pub fn weathers() -> Vec<WeatherInfo> {
    let Some(data) = sre::TESDataHandler::singleton() else { return Vec::new() };
    let list = data.forms_of_type(FormType::WTHR).iter().flatten().filter_map(|f| f.cast::<TESWeather>().map(|w| WeatherInfo { form_id: f.formID, label: form_label(f), kind: Kind::of(w.dataFlags) })).collect();
    sorted_by_label(list, |w: &WeatherInfo| &w.label)
}

/// `GetHour`.
#[must_use]
pub fn hour() -> f32 {
    render::game_hour().unwrap_or(12.0)
}

/// `SetHour`'s game side: the global the sky reads every frame. Main thread.
pub fn set_hour(hour: f32) {
    render::set_game_hour(hour.clamp(0.0, 23.99));
}

/// `CurrentWeather`.
#[must_use]
pub fn current_weather() -> u32 {
    sre::Sky::singleton().and_then(SkyExt::current_weather).map_or(0, |w| w.formID)
}

/// `ForceWeather`'s game side: a transition (`SetWeather` with the override) or an immediate
/// switch (`ForceWeather`). Main thread.
pub fn force_weather(form_id: u32, transition: bool) {
    let (Some(sky), Some(weather)) = (sre::Sky::singleton(), sre::find_form_by_id(form_id).and_then(|f| f.cast::<TESWeather>())) else { return };
    if transition {
        sky.set_weather(weather, true, true);
    } else {
        sky.force_weather(weather, true);
    }
}

/// `ReleaseWeather`'s game side. Main thread.
pub fn release_weather() {
    if let Some(sky) = sre::Sky::singleton() {
        sky.release_weather_override();
    }
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
    /// The root's and the glare's translations when the override took the sun, to put back.
    bases: Option<(P3, P3)>,
    /// The override went off; the hook puts the disc and glare back.
    restore: bool,
}

/// Written from the menu on the main thread, read by the sky hook on a job thread.
static SUN: Mutex<Sun> = Mutex::new(Sun { wanted: SunOverride { enabled: false, azimuth: 135.0, elevation: 20.0 }, contested: false, written: None, bases: None, restore: false });

fn place(node: Option<&NiAVObject>, translation: P3) {
    if let Some(node) = node {
        let mut local = node.local_transform();
        local.translation = translation;
        node.set_local_transform(local);
        node.update_transforms();
    }
}

/// The disc and glare back where the engine had them.
fn put_back(s: &mut Sun, nodes: &SunNodes) {
    if let Some((root, glare)) = s.bases.take() {
        place(nodes.root, root);
        place(nodes.glare, glare);
    }
    s.written = None;
}

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
        if !wanted.enabled && s.wanted.enabled {
            s.restore = true;
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
/// column 0, which the shadows read, and the sun's root (the disc) and glare moved onto it at the
/// distances the engine had them, since the sky's scattering and the disc read those nodes. A
/// column that moved since our last write was written by another hook after ours, and the
/// override yields rather than fight it every frame.
pub fn apply_sun(sky: &sre::Sky) {
    let Some(nodes) = sky.sun_nodes() else {
        sun(|s| {
            s.written = None;
            s.bases = None;
        });
        return;
    };
    sun(|s| {
        if core::mem::take(&mut s.restore) {
            put_back(s, &nodes);
            return;
        }
        if !s.wanted.enabled || s.contested {
            return;
        }
        let mut local = nodes.light.local_transform();
        let current = [local.rotation[0][0], local.rotation[1][0], local.rotation[2][0]];
        if let Some(written) = s.written
            && point::length(point::sub(current, written)) > 0.01
        {
            s.contested = true;
            put_back(s, &nodes);
            crate::log("<Photo Mode> Sun override yielded: another plugin is writing the sun light");
            return;
        }
        let column = sun_column(&s.wanted);
        for (row, value) in local.rotation.iter_mut().zip(column) {
            row[0] = value;
        }
        nodes.light.set_local_transform(local);
        nodes.light.update_transforms();
        s.written = Some(column);
        let translation = |n: Option<&NiAVObject>| n.map_or([0.0; 3], |n| n.local_transform().translation);
        let (root, glare) = *s.bases.get_or_insert_with(|| (translation(nodes.root), translation(nodes.glare)));
        let to_sun = point::scale(column, -1.0);
        for (node, base) in [(nodes.root, root), (nodes.glare, glare)] {
            let radius = point::length(base);
            if radius > 1.0 {
                place(node, point::scale(to_sun, radius));
            }
        }
    });
}

/// `OnStudioExit` and `ClearSession` for the sun: the override off, so the next `Sky::Update`
/// writes the hour's direction again.
pub fn release_sun() {
    set_sun(SunOverride { enabled: false, ..get_sun() });
}

#[cfg(test)]
mod tests {
    use super::{Kind, SunOverride, sun_column};

    #[test]
    fn precipitation_wins_the_weather_class() {
        assert_eq!(Kind::of(0x1), Kind::Clear);
        assert_eq!(Kind::of(0x3), Kind::Cloudy);
        assert_eq!(Kind::of(0x6), Kind::Rainy);
        assert_eq!(Kind::of(0xA), Kind::Snowy);
        assert_eq!(Kind::of(0), Kind::Other);
        assert_eq!(Kind::Rainy.name(), "Rain");
    }

    #[test]
    fn the_column_points_from_the_sun_to_the_ground() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 0.0, elevation: 0.0 }), [0.0, -1.0, 0.0]), "north on the horizon lights toward the south");
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: 90.0 }), [0.0, 0.0, -1.0]));
        assert!(close(sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: -60.0 }), sun_column(&SunOverride { enabled: true, azimuth: 90.0, elevation: -20.0 })), "clamped below the horizon");
    }
}
