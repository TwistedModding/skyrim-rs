//! `camera/CameraManager`: once a frame, after the player camera's state ran (`hook::cameras`),
//! decides which of IC's camera types is current and publishes it. IC's order is kept: the
//! camera type, the capture of the frame's camera, then the snapshot the body reads.

use std::sync::Mutex;

use skyrim::prelude::*;
use sre::PlayerCamera;

use super::snapshot;
use super::state::CameraType;
use crate::body::snapshot as body_snapshot;
use crate::body::state::action::ActionType;
use crate::body::state::race::RaceType;
use crate::util::skyrim_util as util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manager {
    /// The type of the camera state IC runs (`m_cameraState->GetType()`).
    pub camera: CameraType,
    pub previous: CameraType,
}

impl Default for Manager {
    fn default() -> Self {
        Self { camera: CameraType::Unknown, previous: CameraType::Unknown }
    }
}

pub static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Manager::default))
}

/// `CameraManager::Update`.
pub fn update(camera: &PlayerCamera) {
    with(|m| {
        m.update_camera_state(camera);
        m.capture_camera_state();
    });
}

/// The camera type IC derives from the engine state, the open menus and the body's action and race.
#[must_use]
pub fn camera_type(engine: CameraType, menu_open: impl Fn(&str) -> bool, action: ActionType, race: RaceType, dead: bool) -> CameraType {
    let mut camera = if menu_open("MapMenu") {
        CameraType::MapMenu
    } else if menu_open("RaceSex Menu") {
        CameraType::RaceMenu
    } else if menu_open("StatsMenu") {
        CameraType::Unknown
    } else {
        engine
    };
    if camera == CameraType::Bleedout && dead {
        camera = CameraType::Dead;
    }
    if !matches!(camera, CameraType::Free | CameraType::MapMenu | CameraType::RaceMenu | CameraType::TweenMenu) {
        camera = match action {
            ActionType::Animation | ActionType::Bathe | ActionType::BoatRide | ActionType::CartRide | ActionType::Fish | ActionType::Sit | ActionType::Sleep | ActionType::View => CameraType::Animation,
            ActionType::Craft => CameraType::Crafting,
            ActionType::Gather => CameraType::Gathering,
            ActionType::LichTransition => CameraType::LichTransition,
            ActionType::Sex => CameraType::Sex,
            ActionType::VampirelordTransition => CameraType::VampirelordTransition,
            ActionType::WerewolfTransition => CameraType::WerewolfTransition,
            _ => camera,
        };
        camera = match race {
            RaceType::Lich => CameraType::Lich,
            RaceType::Vampirelord => CameraType::Vampirelord,
            RaceType::Werewolf | RaceType::Werebear => CameraType::Werewolf,
            _ => camera,
        };
        if menu_open("Dialogue Menu") && !menu_open("BarterMenu") {
            camera = CameraType::Dialogue;
        }
    }
    if camera == CameraType::Free && action == ActionType::Sex {
        camera = CameraType::SexTFC;
    }
    camera
}

impl Manager {
    fn update_camera_state(&mut self, camera: &PlayerCamera) {
        let body = body_snapshot::get();
        let engine = camera.current_state().map_or(CameraType::Unknown, |s| CameraType::from_state(s.id));
        let dead = util::player().is_some_and(|p| p.is_dead(true));
        let next = camera_type(engine, util::is_menu_open, body.action, body.race, dead);
        if self.camera != next {
            self.previous = self.camera;
            self.camera = next;
        }
    }

    fn capture_camera_state(self) {
        snapshot::set(snapshot::Data { current: self.camera, previous: self.previous });
    }
}

#[cfg(test)]
mod tests {
    use super::camera_type;
    use crate::body::state::action::ActionType;
    use crate::body::state::race::RaceType;
    use crate::camera::state::CameraType;

    #[test]
    fn menus_actions_and_races_override_the_engine_state() {
        let none = |_: &str| false;
        assert_eq!(camera_type(CameraType::FirstPerson, none, ActionType::None, RaceType::Nord, false), CameraType::FirstPerson);
        assert_eq!(camera_type(CameraType::FirstPerson, |m| m == "MapMenu", ActionType::Sit, RaceType::Nord, false), CameraType::MapMenu);
        assert_eq!(camera_type(CameraType::ThirdPerson, none, ActionType::Sit, RaceType::Nord, false), CameraType::Animation);
        assert_eq!(camera_type(CameraType::ThirdPerson, none, ActionType::None, RaceType::Werebear, false), CameraType::Werewolf);
        assert_eq!(camera_type(CameraType::Bleedout, none, ActionType::None, RaceType::Nord, true), CameraType::Dead);
        assert_eq!(camera_type(CameraType::FirstPerson, |m| m == "Dialogue Menu", ActionType::Dialogue, RaceType::Nord, false), CameraType::Dialogue);
        assert_eq!(camera_type(CameraType::Free, none, ActionType::Sex, RaceType::Nord, false), CameraType::SexTFC);
    }
}
