//! `event/InputEventManager`'s `TogglePOV`: a tap of the game's Toggle POV key, as the Toggle
//! POV handler reports it (`hook::input`). In the cameras that keep a third-person zoom it flips
//! between IC's fake first person and the zoom the player had; in first and third person it tells
//! the camera manager, which then skips the engine's zoom out. And `LookEventMouse` and
//! `LookEventGamepad`: in IC's first person the look input that would turn past the
//! `RestrictAngles` limits is dropped before the look handler takes it.

use sre::{Actor, CameraState, MouseMoveEvent, ThirdPersonState, ThumbstickEvent};

use crate::body::snapshot as body_snapshot;
use crate::body::state::action::ActionType;
use crate::camera::base::THIRD_PERSON_ZOOM;
use crate::camera::manager;
use crate::camera::snapshot;
use crate::camera::state::CameraType;
use crate::settings::{self, RestrictAngles};
use crate::util::math_util::normalize_relative_angle;
use crate::util::skyrim_util::{self as util, FAKE_FIRST_PERSON_ZOOM};

/// The target and saved zoom after the toggle: out of fake first person to the saved zoom (the
/// close third person when that was fake first person too), or into it, saving the current zoom.
#[must_use]
pub fn toggled_zoom(target: f32, saved: f32, current: f32) -> (f32, f32) {
    if target < -0.21 {
        if saved <= FAKE_FIRST_PERSON_ZOOM { (THIRD_PERSON_ZOOM, THIRD_PERSON_ZOOM) } else { (saved, saved) }
    } else {
        (FAKE_FIRST_PERSON_ZOOM, current)
    }
}

fn toggle_zoom(state: &ThirdPersonState) {
    let (target, saved) = toggled_zoom(state.targetZoom, state.savedZoom, state.currentZoom);
    skyrim::store(&state.targetZoom, target);
    skyrim::store(&state.savedZoom, saved);
}

/// `TogglePOV`, with the body on.
pub fn toggle_pov() {
    let general = settings::profile().General;
    if !general.bEnableBody {
        return;
    }
    let Some(camera) = util::camera() else { return };
    let action = body_snapshot::get().action;
    let state = match snapshot::get().current {
        CameraType::Bleedout => CameraState::Bleedout,
        CameraType::Dragon => CameraState::Dragon,
        CameraType::Horse => CameraState::Horse,
        CameraType::Animation | CameraType::Lich | CameraType::Sex | CameraType::Spectator | CameraType::Vampirelord | CameraType::Werewolf => {
            if matches!(action, ActionType::BoatRide | ActionType::CartRide) {
                return;
            }
            CameraState::ThirdPerson
        }
        CameraType::ThirdPerson if action == ActionType::AI || general.bEnableFakeFirstPerson => CameraState::ThirdPerson,
        CameraType::ThirdPerson | CameraType::FirstPerson => {
            manager::notify_camera_toggle();
            return;
        }
        _ => return,
    };
    if let Some(state) = util::family(camera, state) {
        toggle_zoom(state);
    }
}

/// One set of `RestrictAngles` limits, in radians: pitch up is positive, heading left negative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub up: f32,
    pub down: f32,
    pub left: f32,
    pub right: f32,
}

impl Limits {
    #[must_use]
    pub fn standing(r: &RestrictAngles) -> Self {
        Self { up: r.fPitchUp.to_radians(), down: r.fPitchDown.to_radians(), left: r.fHeadingLeft.to_radians(), right: r.fHeadingRight.to_radians() }
    }

    /// Sitting, under a script's control or in a giantess animation.
    #[must_use]
    pub fn interaction(r: &RestrictAngles) -> Self {
        Self { up: r.fInteractionPitchUp.to_radians(), down: r.fInteractionPitchDown.to_radians(), left: r.fInteractionHeadingLeft.to_radians(), right: r.fInteractionHeadingRight.to_radians() }
    }
}

/// Where the look is and which limits hold; `yaw` (from the base yaw) is `None` where the heading
/// is not limited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restriction {
    pub pitch: f32,
    pub yaw: Option<f32>,
    pub limits: Limits,
    pub invert: bool,
}

impl Restriction {
    /// Which axes of a look input to drop, `(x, y)`: `up` and `right` as the device reports them
    /// (a mouse moving up and a stick pushed up are positive), before `bInvertYValues` applies.
    #[must_use]
    pub fn blocked(&self, up: f32, right: f32) -> (bool, bool) {
        let l = self.limits;
        let (raising, lowering) = if self.invert { (up < 0.0, up > 0.0) } else { (up > 0.0, up < 0.0) };
        let y = (self.pitch > l.up && raising) || (self.pitch < l.down && lowering);
        let x = self.yaw.is_some_and(|yaw| (yaw < l.left && right < 0.0) || (yaw > l.right && right > 0.0));
        (x, y)
    }
}

/// The restriction of the moment: only in IC's first person (real or fake) with the body on, and
/// not in the tween or dialogue menus. The actor is the camera target, or the player on a horse
/// or dragon.
fn restriction() -> Option<Restriction> {
    let profile = *settings::profile();
    if !util::is_first_person() || !profile.General.bEnableBody || util::is_menu_open("TweenMenu") || util::is_menu_open("Dialogue Menu") {
        return None;
    }
    let player = util::player()?;
    let target = util::camera_target();
    let actor: &Actor = match snapshot::get().current {
        CameraType::Horse | CameraType::Dragon => player,
        _ => target.as_deref().and_then(util::actor_of).unwrap_or(player),
    };
    let invert = unsafe { *sre::ini_pref_setting_b_invert_y_values_controls_value() } != 0;
    let (look, camera_lock) = manager::with(|m| (m.look, m.camera_lock));
    let yaw = normalize_relative_angle(look.state.yaw - look.base_yaw);
    let pitch = -look.state.pitch;
    let r = &profile.RestrictAngles;
    Some(if util::is_sitting(actor) || util::is_script_mode() || util::graph_bool(actor, "GTS_Busy") {
        Restriction { pitch, yaw: Some(if camera_lock == CameraType::FirstPerson { 0.0 } else { yaw }), limits: Limits::interaction(r), invert }
    } else {
        Restriction { pitch, yaw: util::is_fake_first_person().then_some(yaw), limits: Limits::standing(r), invert }
    })
}

/// `LookEventMouse`: the mouse reports y growing downwards.
pub fn look_event_mouse(event: &mut MouseMoveEvent) {
    let Some(r) = restriction() else { return };
    #[allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)]
    let (x, y) = r.blocked(-(event.dy as i32 as f32), event.dx as i32 as f32);
    if x {
        event.dx = 0;
    }
    if y {
        event.dy = 0;
    }
}

/// `LookEventGamepad`.
pub fn look_event_gamepad(event: &mut ThumbstickEvent) {
    let Some(r) = restriction() else { return };
    let (x, y) = r.blocked(event.y, event.x);
    if x {
        event.x = 0.0;
    }
    if y {
        event.y = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, Restriction, toggled_zoom};
    use crate::settings::RestrictAngles;

    fn at(pitch_deg: f32, yaw_deg: Option<f32>, invert: bool) -> Restriction {
        Restriction { pitch: pitch_deg.to_radians(), yaw: yaw_deg.map(f32::to_radians), limits: Limits::standing(&RestrictAngles::default()), invert }
    }

    #[test]
    fn the_pitch_stops_only_past_a_limit_and_only_towards_it() {
        assert_eq!(at(89.5, None, false).blocked(1.0, 0.0), (false, true), "past the up limit, looking further up");
        assert_eq!(at(89.5, None, false).blocked(-1.0, 0.0), (false, false), "looking back down is free");
        assert_eq!(at(-89.5, None, false).blocked(-1.0, 0.0), (false, true), "past the down limit, looking further down");
        assert_eq!(at(88.0, None, false).blocked(1.0, 0.0), (false, false), "inside the limits");
        assert_eq!(at(89.5, None, true).blocked(-1.0, 0.0), (false, true), "inverted, down on the device raises the look");
        assert_eq!(at(89.5, None, true).blocked(1.0, 0.0), (false, false));
    }

    #[test]
    fn the_heading_is_limited_only_where_a_yaw_is_given() {
        assert_eq!(at(0.0, None, false).blocked(0.0, 1.0), (false, false), "standing first person turns freely");
        assert_eq!(at(0.0, Some(90.0), false).blocked(0.0, 1.0), (true, false), "past the right limit");
        assert_eq!(at(0.0, Some(90.0), false).blocked(0.0, -1.0), (false, false), "turning back");
        assert_eq!(at(0.0, Some(-90.0), false).blocked(0.0, -1.0), (true, false), "past the left limit");
    }

    #[test]
    fn interaction_limits_are_ic_defaults_in_radians() {
        let l = Limits::interaction(&RestrictAngles::default());
        assert!((l.up - 75f32.to_radians()).abs() < 1e-6 && (l.down + 75f32.to_radians()).abs() < 1e-6);
        assert!((l.left + 70f32.to_radians()).abs() < 1e-6 && (l.right - 70f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn the_toggle_flips_between_fake_first_person_and_the_saved_zoom() {
        assert_eq!(toggled_zoom(0.4, 0.1, 0.35), (-0.275, 0.35), "into fake first person, the current zoom saved");
        assert_eq!(toggled_zoom(-0.275, 0.35, -0.275), (0.35, 0.35), "back to the saved zoom");
        assert_eq!(toggled_zoom(-0.275, -0.275, -0.275), (-0.2, -0.2), "a saved fake first person goes to the close third person");
        assert_eq!(toggled_zoom(-0.2, -0.3, -0.2), (-0.275, -0.2), "the close third person is not fake first person");
    }
}
