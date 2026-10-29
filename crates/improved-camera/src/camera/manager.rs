//! `camera/CameraManager`: once a frame, after the player camera's state ran (`hook::cameras`),
//! decides which of IC's camera types is current, runs the FOV part of its state and publishes it;
//! with the body on it then eases the world FOV toward the state's and sets the FOV offset
//! (`UpdateFOV`). IC's order is kept: the camera type, the capture of the frame's camera, then
//! the FOV, which the engine applies after this point.

use std::sync::Mutex;

use skyrim::prelude::*;
use sre::{CameraState, PlayerCamera};

use super::snapshot;
use super::state::CameraType;
use super::states;
use crate::body::snapshot as body_snapshot;
use crate::body::state::action::ActionType;
use crate::body::state::hands::HandsType;
use crate::body::state::race::RaceType;
use crate::settings;
use crate::util::math_util::{damped_alpha, lerp};
use crate::util::skyrim_util as util;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Manager {
    /// The type of the camera state IC runs (`m_cameraState->GetType()`).
    pub camera: CameraType,
    pub previous: CameraType,
    /// The FOV the current state asks for (`m_currentState.modifier.fov`), 0 until one does.
    pub fov: f32,
    /// The world FOV was changed from outside and is followed instead of the state's (`m_deferFOV`).
    pub defer_fov: bool,
    pub external_fov: f32,
    /// The world FOV IC writes, eased toward its target (`m_resolveFOV`).
    pub resolve_fov: f32,
    /// A world FOV to follow from the next update (`NotifyFOVReset`), 0 for none.
    pub pending_fov_reset: f32,
    /// The zoom effect's toggle and its eased part of the FOV offset.
    pub zoom_active: bool,
    pub zoom_modifier: f32,
    /// The world FOV the dialogue camera found on entry (`DialogueState::m_previousFOV`).
    pub dialogue_fov: f32,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            camera: CameraType::Unknown,
            previous: CameraType::Unknown,
            fov: 0.0,
            defer_fov: false,
            external_fov: 80.0,
            resolve_fov: 80.0,
            pending_fov_reset: 0.0,
            zoom_active: false,
            zoom_modifier: 0.0,
            dialogue_fov: 0.0,
        }
    }
}

pub static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Manager::default))
}

/// `CameraManager::Update`.
pub fn update(camera: &PlayerCamera) {
    let delta = util::delta_frame_time();
    with(|m| {
        m.update_camera_state(camera);
        let present = m.capture_camera_state(camera);
        if settings::profile().General.bEnableBody {
            m.update_fov(camera, present, delta);
        }
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

/// The FOV offset the `Camera Control` node of the engine's first- or third-person state drives
/// while one of their camera types is current, read as IC reads it: the node's local z.
fn game_fov_modifier(camera: &PlayerCamera, current: CameraType) -> Option<f32> {
    camera.current_state()?;
    let node = match current {
        CameraType::FirstPerson => camera.first_person()?.cameraControlNode,
        CameraType::ThirdPerson | CameraType::Horse | CameraType::Dragon | CameraType::Vampirelord | CameraType::Werewolf | CameraType::Lich | CameraType::Sex => camera.third_person()?.cameraControlNode,
        _ => return None,
    };
    unsafe { node.as_ref() }.map(|n| n.local.translate.z)
}

/// Puts back what IC's FOV work changed: the world FOV to `fDefaultWorldFOV:Display` (as IC's
/// `Reset_FirstPerson`) and the FOV offset to the engine's own, without the zoom effect.
pub fn reset_fov() {
    let Some(camera) = util::camera() else { return };
    skyrim::store(&camera.worldFOV, util::default_world_fov());
    if let Some(game) = game_fov_modifier(camera, with(|m| m.camera)) {
        skyrim::camera::set_fov_offset(game);
    }
}

impl Manager {
    fn update_camera_state(&mut self, camera: &PlayerCamera) {
        let body = body_snapshot::get();
        let engine = camera.current_state().map_or(CameraType::Unknown, |s| CameraType::from_state(s.id));
        let dead = util::player().is_some_and(|p| p.is_dead(true));
        let next = camera_type(engine, util::is_menu_open, body.action, body.race, dead);
        let profile = settings::profile();
        let enabled = profile.General.bEnableBody;
        if self.camera != next {
            if enabled {
                self.exit(self.camera);
            }
            self.previous = self.camera;
            self.camera = next;
            if enabled {
                self.enter(next, camera);
            }
        }
        if enabled {
            let fake_first_person = profile.General.bEnableFakeFirstPerson && util::zoom_of(CameraState::ThirdPerson).is_some_and(|z| z <= util::FAKE_FIRST_PERSON_ZOOM);
            let inputs = states::Inputs { hands_drawn: body.hands != HandsType::None, fake_first_person, world_fov: camera.worldFOV };
            if let Some(fov) = states::fov(self.camera, &profile.FOV, inputs) {
                self.fov = fov;
            }
        }
    }

    /// The FOV work of the entered state's `OnEnter`: the dialogue camera remembers the world FOV,
    /// the free cameras start from the default one.
    fn enter(&mut self, state: CameraType, camera: &PlayerCamera) {
        match state {
            CameraType::Dialogue => self.dialogue_fov = camera.worldFOV,
            CameraType::Free | CameraType::SexTFC => skyrim::store(&camera.worldFOV, util::default_world_fov()),
            _ => {}
        }
    }

    /// The FOV work of the left state's `OnExit`: the dialogue camera hands back the world FOV it found.
    fn exit(&mut self, state: CameraType) {
        if state == CameraType::Dialogue && self.dialogue_fov > 0.0 {
            self.pending_fov_reset = self.dialogue_fov;
        }
    }

    /// `CaptureCameraState`: publishes the camera type and returns the FOV the frame presents; a
    /// state that asked for none adopts the world FOV from the next frame on.
    fn capture_camera_state(&mut self, camera: &PlayerCamera) -> f32 {
        snapshot::set(snapshot::Data { current: self.camera, previous: self.previous });
        let present = self.fov;
        if self.fov == 0.0 {
            self.fov = camera.worldFOV;
        }
        present
    }

    /// `UpdateFOV`: writes the eased world FOV and, where the engine drives one, the FOV offset.
    fn update_fov(&mut self, camera: &PlayerCamera, present: f32, delta: f32) {
        let external_owner = self.camera == CameraType::TweenMenu || util::is_anim_camera();
        let zoom = if self.zoom_active { settings::profile().Zoom.fFOV } else { 0.0 };
        let (world, offset) = self.resolve_fov(present, camera.worldFOV, external_owner, game_fov_modifier(camera, self.camera), zoom, delta);
        skyrim::store(&camera.worldFOV, world);
        if let Some(offset) = offset {
            skyrim::camera::set_fov_offset(offset);
        }
    }

    /// `UpdateFOV` on plain values: from the FOV the state presents and the world FOV the engine
    /// holds, the world FOV to write; with the engine's own offset (`game`), the FOV offset to write,
    /// which adds the zoom effect's eased `zoom` and keeps the sum within 5 to 165 degrees. A world
    /// FOV changed from outside (the console, another mod) is followed until IC's settles on it;
    /// the tween menu and an animator camera keep their own.
    pub fn resolve_fov(&mut self, present: f32, world: f32, external_owner: bool, game: Option<f32>, zoom: f32, delta: f32) -> (f32, Option<f32>) {
        let mut ic = present;
        let mut current = world;
        // The map menu's first frames hold garbage.
        if current < 1.0 || current > 180.0 {
            current = ic;
            self.resolve_fov = ic;
        }
        if matches!(self.camera, CameraType::Free | CameraType::Cinematic) {
            if self.pending_fov_reset > 0.0 {
                current = self.pending_fov_reset;
                self.pending_fov_reset = 0.0;
            }
            self.defer_fov = false;
            self.resolve_fov = current.clamp(5.0, 165.0);
            return (self.resolve_fov, None);
        }
        if self.pending_fov_reset > 0.0 {
            self.defer_fov = true;
            self.external_fov = self.pending_fov_reset;
            self.pending_fov_reset = 0.0;
        }
        if self.defer_fov {
            let settled = (self.resolve_fov - self.external_fov).abs() < 0.1;
            if settled && (current - self.external_fov).abs() < 0.1 {
                self.defer_fov = false;
                ic = self.external_fov;
                self.fov = ic;
            }
        } else {
            let settled = (self.resolve_fov - ic).abs() < 0.1;
            if ic > 0.0 && settled && (current - ic).abs() > 0.1 {
                self.defer_fov = true;
                self.external_fov = current;
            }
        }
        let alpha = damped_alpha(6.0, delta);
        let mut target = if self.defer_fov { self.external_fov } else { ic };
        if target < 0.1 {
            target = self.external_fov;
        }
        self.resolve_fov = lerp(self.resolve_fov, target, alpha);
        let world = if external_owner { current } else { self.resolve_fov };
        let offset = game.map(|game| {
            self.zoom_modifier = lerp(self.zoom_modifier, zoom, alpha);
            if (self.zoom_modifier - zoom).abs() < 0.01 {
                self.zoom_modifier = zoom;
            }
            if self.zoom_modifier < -180.0 || self.zoom_modifier > 180.0 {
                self.zoom_modifier = 0.0;
            }
            let total = world + game + self.zoom_modifier;
            if total < 5.0 {
                self.zoom_modifier += 5.0 - total;
            }
            if total > 165.0 {
                self.zoom_modifier -= total - 165.0;
            }
            game + self.zoom_modifier
        });
        (world, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::{Manager, camera_type};
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

    fn first_person() -> Manager {
        Manager { camera: CameraType::FirstPerson, ..Manager::default() }
    }

    /// Runs `frames` updates of 1/60 s with the engine holding what was written last; the last world FOV.
    fn settle(m: &mut Manager, present: f32, mut world: f32, frames: usize) -> f32 {
        for _ in 0..frames {
            world = m.resolve_fov(present, world, false, None, 0.0, 1.0 / 60.0).0;
        }
        world
    }

    /// Updates while the world FOV follows an outside value; the world FOV when it settled.
    fn follow(m: &mut Manager, present: f32, mut world: f32) -> f32 {
        for _ in 0..200 {
            if !m.defer_fov {
                break;
            }
            world = settle(m, present, world, 1);
        }
        world
    }

    #[test]
    fn the_world_fov_eases_to_the_state_and_follows_an_outside_change() {
        let mut m = first_person();
        let (world, offset) = m.resolve_fov(90.0, 80.0, false, None, 0.0, 1.0 / 60.0);
        assert!(world > 80.0 && world < 90.0, "{world}: a tenth of the way per 1/60 s");
        assert_eq!(offset, None);
        let world = settle(&mut m, 90.0, world, 120);
        assert!((world - 90.0).abs() < 0.1);

        let world = settle(&mut m, 90.0, 70.0, 1);
        assert!(m.defer_fov && (m.external_fov - 70.0).abs() < f32::EPSILON, "a console fov 70 is taken over");
        let world = follow(&mut m, 90.0, world);
        assert!(!m.defer_fov && (world - 70.0).abs() < 0.1 && (m.fov - 70.0).abs() < f32::EPSILON, "settled on it, it is the current FOV");
        assert!((settle(&mut m, 90.0, world, 120) - 90.0).abs() < 0.1, "until the state asks for its own again");
    }

    #[test]
    fn the_tween_menu_keeps_its_world_fov_and_garbage_is_ignored() {
        let mut m = Manager { camera: CameraType::TweenMenu, ..Manager::default() };
        assert_eq!(m.resolve_fov(80.0, 95.0, true, None, 0.0, 1.0 / 60.0).0.to_bits(), 95f32.to_bits());
        let mut m = Manager { camera: CameraType::MapMenu, ..Manager::default() };
        assert_eq!(m.resolve_fov(85.0, 1.0e9, false, None, 0.0, 1.0 / 60.0).0.to_bits(), 85f32.to_bits());
    }

    #[test]
    fn free_cameras_own_the_world_fov_and_take_a_pending_reset() {
        let mut m = Manager { camera: CameraType::Free, pending_fov_reset: 75.0, defer_fov: true, ..Manager::default() };
        assert_eq!(m.resolve_fov(80.0, 200.0, false, Some(3.0), -30.0, 1.0 / 60.0), (75.0, None));
        assert!(!m.defer_fov && m.pending_fov_reset == 0.0);
        assert_eq!(m.resolve_fov(80.0, 2.0, false, None, 0.0, 1.0 / 60.0).0.to_bits(), 5f32.to_bits(), "clamped to 5");
    }

    #[test]
    fn a_dialogue_hands_its_entry_fov_back() {
        let mut m = first_person();
        m.dialogue_fov = 65.0;
        m.exit(CameraType::Dialogue);
        let world = settle(&mut m, 80.0, 80.0, 1);
        assert!(m.defer_fov && (m.external_fov - 65.0).abs() < f32::EPSILON);
        assert!((follow(&mut m, 80.0, world) - 65.0).abs() < 0.1);
    }

    #[test]
    fn the_zoom_eases_into_the_offset_within_5_to_165_degrees() {
        let mut m = first_person();
        let mut offset = 0.0;
        for _ in 0..200 {
            offset = m.resolve_fov(80.0, 80.0, false, Some(2.0), -30.0, 1.0 / 60.0).1.unwrap();
        }
        assert!((offset - (2.0 - 30.0)).abs() < 0.01, "{offset}");
        for _ in 0..200 {
            offset = m.resolve_fov(80.0, 80.0, false, Some(2.0), -100.0, 1.0 / 60.0).1.unwrap();
        }
        assert!(80.0 + offset >= 5.0 - 0.01 && 80.0 + offset < 6.0, "{offset}: the total stops at 5");
    }
}
