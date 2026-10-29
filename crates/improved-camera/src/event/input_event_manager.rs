//! `event/InputEventManager`'s `TogglePOV`: a tap of the game's Toggle POV key, as the Toggle
//! POV handler reports it (`hook::input`). In the cameras that keep a third-person zoom it flips
//! between IC's fake first person and the zoom the player had; in first and third person it tells
//! the camera manager, which then skips the engine's zoom out.

use sre::{CameraState, ThirdPersonState};

use crate::body::snapshot as body_snapshot;
use crate::body::state::action::ActionType;
use crate::camera::base::THIRD_PERSON_ZOOM;
use crate::camera::manager;
use crate::camera::snapshot;
use crate::camera::state::CameraType;
use crate::settings;
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

#[cfg(test)]
mod tests {
    use super::toggled_zoom;

    #[test]
    fn the_toggle_flips_between_fake_first_person_and_the_saved_zoom() {
        assert_eq!(toggled_zoom(0.4, 0.1, 0.35), (-0.275, 0.35), "into fake first person, the current zoom saved");
        assert_eq!(toggled_zoom(-0.275, 0.35, -0.275), (0.35, 0.35), "back to the saved zoom");
        assert_eq!(toggled_zoom(-0.275, -0.275, -0.275), (-0.2, -0.2), "a saved fake first person goes to the close third person");
        assert_eq!(toggled_zoom(-0.2, -0.3, -0.2), (-0.275, -0.2), "the close third person is not fake first person");
    }
}
