//! `camera/Cameras.h`: the camera state objects IC switches between, one per camera type, each
//! with its `OnEnter`, `OnUpdate` and `OnExit` (`camera::base` and `camera::extended`).

use framework::{GameMouse, GamePad};
use sre::{CameraState, PlayerCamera};

use super::base::{self, Free, ThirdPerson, Vanity};
use super::extended::{self, Dialogue, RaceMenu, Spectator};
use super::state::{CameraType, Modifier};
use super::util::look::LookMode;
use super::util::orbit::OrbitController;
use crate::body::snapshot::Data as Body;
use crate::settings::Profile;

/// The frame's input for the states that take some: the game's mouse and gamepad, the free
/// camera's keys and the game's Toggle POV key released with its held seconds.
#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Input {
    pub mouse: GameMouse,
    pub pad: GamePad,
    pub pov_released: Option<f32>,
    pub roll_left: bool,
    pub roll_right: bool,
    pub reset: bool,
    /// Left shift, IC's fixed modifier of the free camera's wheel.
    pub modifier: bool,
}

/// What a state asks of the manager (IC's `Manager::Notify*` calls from inside the states).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Notes {
    pub forced_first_person: bool,
    /// `NotifyFOVReset`: a world FOV to follow from the next update, 0 for none.
    pub fov_reset: f32,
    /// `NotifyFOVZoom`: degrees added to the free camera's FOV this frame.
    pub fov_zoom: f32,
}

pub struct Ctx<'a> {
    pub camera: &'a PlayerCamera,
    pub profile: &'a Profile,
    pub body: Body,
    pub notes: &'a mut Notes,
    pub input: &'a Input,
    /// The dialogue's speaker focus is wanted this frame (`UsingSpeakerFocus` in the dialogue camera).
    pub dialogue_focus: bool,
}

/// IC's state objects with what each keeps between frames.
#[derive(Clone, Debug)]
pub enum State {
    Unknown,
    FirstPerson,
    Vanity(Vanity),
    Vats(OrbitController),
    Free(Free),
    IronSight,
    Furniture,
    Transition { previous: CameraType },
    TweenMenu { previous: CameraType },
    Animated,
    ThirdPerson(ThirdPerson),
    Horse,
    HorseMounting,
    HorseDismounting,
    Bleedout(OrbitController),
    Dragon,
    DragonMounting,
    DragonDismounting,
    Animation { should_sim: bool },
    Crafting,
    Dead(OrbitController),
    Dialogue(Dialogue),
    Gathering(OrbitController),
    MapMenu,
    RaceMenu(RaceMenu),
    Spectator(Spectator),
    VampirelordTransition,
    Vampirelord,
    WerewolfTransition,
    Werewolf,
    LichTransition,
    Lich,
    Sex,
    SexTFC(Free),
}

fn sim(m: &mut Modifier, fov: f32) {
    m.fov = fov;
    m.sim_first_person = true;
}

fn offset_sim(m: &mut Modifier, fov: f32, offset: [f32; 3]) {
    sim(m, fov);
    m.local_offset = offset;
}

impl State {
    /// The state object `UpdateCameraState` makes for `camera`, and the look mode it sets with it.
    #[must_use]
    pub fn new(camera: CameraType) -> (Self, LookMode) {
        use CameraType as T;
        use LookMode::{Disabled, FirstPerson, ThirdPerson as Third};
        match camera {
            T::FirstPerson => (Self::FirstPerson, FirstPerson),
            T::Vanity => (Self::Vanity(Vanity::default()), Disabled),
            T::VATS => (Self::Vats(OrbitController::new()), Third),
            T::Free => (Self::Free(Free::new(false)), Disabled),
            T::IronSight => (Self::IronSight, Disabled),
            T::Furniture => (Self::Furniture, Third),
            T::Transition => (Self::Transition { previous: T::Unknown }, Third),
            T::TweenMenu => (Self::TweenMenu { previous: T::Unknown }, Disabled),
            T::Animated => (Self::Animated, Third),
            T::ThirdPerson => (Self::ThirdPerson(ThirdPerson::default()), Third),
            T::Horse => (Self::Horse, Third),
            T::HorseMounting => (Self::HorseMounting, Third),
            T::HorseDismounting => (Self::HorseDismounting, Third),
            T::DragonMounting => (Self::DragonMounting, Third),
            T::DragonDismounting => (Self::DragonDismounting, Third),
            T::Bleedout => (Self::Bleedout(OrbitController::new()), Third),
            T::Dragon => (Self::Dragon, Third),
            T::Animation => (Self::Animation { should_sim: false }, Third),
            T::Crafting => (Self::Crafting, Third),
            T::Dead => (Self::Dead(OrbitController::new()), Third),
            T::Dialogue => (Self::Dialogue(Dialogue::default()), Disabled),
            T::Gathering => (Self::Gathering(OrbitController::new()), Third),
            T::MapMenu => (Self::MapMenu, Disabled),
            T::RaceMenu => (Self::RaceMenu(RaceMenu::new()), Disabled),
            T::Spectator => (Self::Spectator(Spectator::default()), Third),
            T::VampirelordTransition => (Self::VampirelordTransition, Third),
            T::Vampirelord => (Self::Vampirelord, Third),
            T::WerewolfTransition => (Self::WerewolfTransition, Third),
            T::Werewolf => (Self::Werewolf, Third),
            T::LichTransition => (Self::LichTransition, Third),
            T::Lich => (Self::Lich, Third),
            T::Sex => (Self::Sex, Third),
            T::SexTFC => (Self::SexTFC(Free::new(true)), Disabled),
            _ => (Self::Unknown, Disabled),
        }
    }

    /// `GetType`.
    #[must_use]
    pub const fn kind(&self) -> CameraType {
        use CameraType as T;
        match self {
            Self::Unknown => T::Unknown,
            Self::FirstPerson => T::FirstPerson,
            Self::Vanity(_) => T::Vanity,
            Self::Vats(_) => T::VATS,
            Self::Free(_) => T::Free,
            Self::IronSight => T::IronSight,
            Self::Furniture => T::Furniture,
            Self::Transition { .. } => T::Transition,
            Self::TweenMenu { .. } => T::TweenMenu,
            Self::Animated => T::Animated,
            Self::ThirdPerson(_) => T::ThirdPerson,
            Self::Horse => T::Horse,
            Self::HorseMounting => T::HorseMounting,
            Self::HorseDismounting => T::HorseDismounting,
            Self::DragonMounting => T::DragonMounting,
            Self::DragonDismounting => T::DragonDismounting,
            Self::Bleedout(_) => T::Bleedout,
            Self::Dragon => T::Dragon,
            Self::Animation { .. } => T::Animation,
            Self::Crafting => T::Crafting,
            Self::Dead(_) => T::Dead,
            Self::Dialogue(_) => T::Dialogue,
            Self::Gathering(_) => T::Gathering,
            Self::MapMenu => T::MapMenu,
            Self::RaceMenu(_) => T::RaceMenu,
            Self::Spectator(_) => T::Spectator,
            Self::VampirelordTransition => T::VampirelordTransition,
            Self::Vampirelord => T::Vampirelord,
            Self::WerewolfTransition => T::WerewolfTransition,
            Self::Werewolf => T::Werewolf,
            Self::LichTransition => T::LichTransition,
            Self::Lich => T::Lich,
            Self::Sex => T::Sex,
            Self::SexTFC(_) => T::SexTFC,
        }
    }

    /// `OnEnter`.
    pub fn enter(&mut self, previous: CameraType, ctx: &mut Ctx<'_>) {
        match self {
            Self::FirstPerson => base::first_person_enter(previous, ctx),
            Self::Vats(orbit) | Self::Gathering(orbit) => orbit.reset(),
            Self::Bleedout(orbit) | Self::Dead(orbit) => {
                orbit.reset();
                base::bleedout_enter(previous, ctx);
            }
            Self::Free(free) | Self::SexTFC(free) => free.enter(previous, ctx),
            Self::Furniture | Self::VampirelordTransition | Self::WerewolfTransition | Self::LichTransition => base::zoom_in_from_first_person(previous, ctx, false),
            Self::Crafting | Self::Werewolf => base::zoom_in_from_first_person(previous, ctx, true),
            Self::Transition { previous: p } => {
                *p = previous;
                base::transition_enter(previous, ctx);
            }
            Self::TweenMenu { previous: p } => *p = previous,
            Self::RaceMenu(state) => state.enter(previous),
            Self::Vanity(state) => state.enter(ctx),
            Self::ThirdPerson(state) => state.enter(previous, ctx),
            Self::Horse => base::mount_enter(ctx, CameraState::Horse),
            Self::Dragon => base::mount_enter(ctx, CameraState::Dragon),
            Self::HorseMounting => base::mounting_enter(previous, ctx, CameraState::Horse),
            Self::DragonMounting => base::mounting_enter(previous, ctx, CameraState::Dragon),
            Self::HorseDismounting => base::horse_dismount_enter(ctx),
            Self::Animation { should_sim } => extended::animation_enter(should_sim, previous, ctx),
            Self::Dialogue(state) => state.enter(previous, ctx),
            Self::Spectator(state) => state.enter(previous, ctx),
            Self::Unknown | Self::IronSight | Self::Animated | Self::MapMenu | Self::Vampirelord | Self::Lich | Self::Sex | Self::DragonDismounting => {}
        }
    }

    /// `OnUpdate`.
    pub fn update(&mut self, m: &mut Modifier, ctx: &mut Ctx<'_>) {
        let fov = ctx.profile.FOV;
        let camera = ctx.profile.Camera;
        match self {
            Self::FirstPerson => base::first_person_update(m, ctx),
            Self::Vanity(state) => state.update(m, ctx),
            Self::Vats(orbit) => {
                base::pov_zoom_toggle(ctx, base::pov_tap);
                base::orbit_update(orbit, m, ctx, fov.fKillmove, CameraState::ThirdPerson);
            }
            Self::Bleedout(orbit) => base::orbit_update(orbit, m, ctx, fov.fRagdoll, CameraState::Bleedout),
            Self::Dead(orbit) => {
                base::pov_zoom_toggle(ctx, base::pov_tap);
                base::orbit_update(orbit, m, ctx, fov.fDeath, CameraState::ThirdPerson);
            }
            Self::Gathering(orbit) => {
                base::pov_zoom_toggle(ctx, base::pov_tap);
                base::orbit_update(orbit, m, ctx, fov.fCrafting, CameraState::ThirdPerson);
            }
            Self::Free(free) | Self::SexTFC(free) => free.update(m, ctx),
            Self::Furniture => sim(m, fov.fFurniture),
            Self::Transition { .. } => m.sim_first_person = true,
            Self::TweenMenu { previous } => {
                if *previous == CameraType::FirstPerson {
                    base::freeze_body();
                }
            }
            Self::RaceMenu(state) => state.update(ctx),
            Self::Dialogue(state) => state.update(m, ctx, ctx.dialogue_focus),
            Self::ThirdPerson(state) => state.update(m, ctx),
            Self::Horse => offset_sim(m, fov.fHorse, ctx.profile.Horse.offset().point()),
            Self::Dragon => offset_sim(m, fov.fDragon, camera.Dragon.point()),
            Self::HorseMounting | Self::HorseDismounting => sim(m, fov.fHorseTransition),
            Self::DragonMounting | Self::DragonDismounting => sim(m, fov.fDragonTransition),
            Self::Animation { should_sim } => extended::animation_update(should_sim, m, ctx),
            Self::Crafting => {
                base::pov_zoom_toggle(ctx, base::pov_long_press);
                sim(m, fov.fCrafting);
            }
            Self::Spectator(state) => state.update(m, ctx),
            Self::VampirelordTransition => sim(m, fov.fVampireLord),
            Self::Vampirelord => offset_sim(m, fov.fVampireLord, camera.VampireLord.point()),
            Self::WerewolfTransition => sim(m, fov.fWerewolf),
            Self::Werewolf => offset_sim(m, fov.fWerewolf, camera.Werewolf.point()),
            Self::LichTransition => sim(m, fov.fLich),
            Self::Lich => offset_sim(m, fov.fLich, camera.Lich.point()),
            Self::Sex => offset_sim(m, fov.fSex, camera.Sex.point()),
            Self::Unknown | Self::IronSight | Self::Animated | Self::MapMenu => {}
        }
    }

    /// `OnExit`.
    pub fn exit(&mut self, m: &mut Modifier, ctx: &mut Ctx<'_>) {
        match self {
            Self::Vats(orbit) | Self::Bleedout(orbit) | Self::Dead(orbit) | Self::Gathering(orbit) => {
                m.sim_first_person = false;
                base::orbit_release(orbit, ctx.camera);
            }
            Self::Free(free) | Self::SexTFC(free) => free.exit(),
            Self::Furniture => base::furniture_exit(m, ctx),
            Self::Transition { previous } => base::transition_exit(*previous, m, ctx),
            Self::TweenMenu { previous } => {
                if *previous == CameraType::FirstPerson {
                    base::freeze_body();
                }
            }
            Self::RaceMenu(state) => state.exit(ctx.camera),
            Self::Vanity(state) => state.exit(),
            Self::HorseDismounting | Self::DragonDismounting => base::dismount_exit(m),
            Self::Horse | Self::Dragon | Self::Animation { .. } | Self::Crafting | Self::VampirelordTransition | Self::Vampirelord | Self::WerewolfTransition | Self::Werewolf | Self::LichTransition | Self::Lich | Self::Sex => m.sim_first_person = false,
            Self::Dialogue(state) => state.exit(ctx),
            Self::Spectator(state) => state.exit(m),
            Self::Unknown | Self::FirstPerson | Self::IronSight | Self::Animated | Self::ThirdPerson(_) | Self::MapMenu | Self::HorseMounting | Self::DragonMounting => {}
        }
    }

    /// Puts back what the state holds in the engine, for an unload: the orbit's base pose, the
    /// free camera's handlers and fly speed, the spectator's target and neck, the HUD the idle
    /// camera hid and the dialogue's head tracking.
    pub fn release(&mut self, camera: &PlayerCamera, profile: &Profile) {
        match self {
            Self::Vats(orbit) | Self::Bleedout(orbit) | Self::Dead(orbit) | Self::Gathering(orbit) => base::orbit_release(orbit, camera),
            Self::Free(free) | Self::SexTFC(free) => free.release(),
            Self::Spectator(state) => state.release(),
            Self::Vanity(state) => state.exit(),
            Self::RaceMenu(state) => state.exit(camera),
            Self::Dialogue(state) => state.release(profile),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    use crate::camera::state::CameraType;
    use crate::camera::util::look::LookMode;

    #[test]
    fn every_type_makes_its_own_state_with_ic_look_mode() {
        for value in 0..=CameraType::Unknown as u32 {
            let camera = CameraType::from_u32(value);
            let (state, look) = State::new(camera);
            let absent = camera == CameraType::Cinematic;
            assert_eq!(state.kind(), if absent { CameraType::Unknown } else { camera }, "{camera:?}");
            let disabled = matches!(camera, CameraType::Vanity | CameraType::Free | CameraType::IronSight | CameraType::TweenMenu | CameraType::Dialogue | CameraType::MapMenu | CameraType::RaceMenu | CameraType::SexTFC) || absent || camera == CameraType::Unknown;
            let expected = if camera == CameraType::FirstPerson {
                LookMode::FirstPerson
            } else if disabled {
                LookMode::Disabled
            } else {
                LookMode::ThirdPerson
            };
            assert_eq!(look, expected, "{camera:?}");
        }
    }
}
