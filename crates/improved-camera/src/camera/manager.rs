//! `camera/CameraManager`: once a frame, after the player camera's state ran (`hook::cameras`),
//! IC's order: the camera type and its state object (`UpdateCameraState`, `SetCamera`), the
//! capture of the frame's camera and its mode (`CaptureCameraState`, `SetMode`), the head anchor
//! of the head-bob and fake first-person modes (`AnchorHeadNode`), the blend between two states
//! (`TransitionCameraState`), and with the body on the presented camera written to the root
//! (`PresentCamera`), the world FOV and FOV offset (`UpdateFOV`) and the near plane by pitch
//! (`UpdateNearDistance`); then the snapshot, and the body anchored to the camera
//! (`BODY::Manager::AnchorCamera`).
//!
//! The near plane is IC's pitch-driven near, written into `fNearDistance` and the frame camera
//! together (`skyrim::render::set_near_plane`, RE `graphics.md` section 3), only when it moved a
//! whole step or settled on an end of its range.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use skyrim::prelude::*;
use sre::{Actor, CameraState, NiAVObject, PlayerCamera, PlayerCharacter, ThirdPersonState};

use super::snapshot;
use super::state::{CameraMode, CameraType, Modifier};
use super::states::{Ctx, Input, Notes, State};
use super::util::look::{LookController, LookMode, camera_heading};
use crate::body::snapshot as body_snapshot;
use crate::body::state::action::ActionType;
use crate::body::state::hands::HandsType;
use crate::body::state::movement::MovementType;
use crate::body::state::race::RaceType;
use crate::settings::{self, Advanced, Profile, Stab};
use crate::util::math_util::{damped_alpha, lerp, normalize_relative_angle, rotation_from_angles, smoother_step};
use crate::util::point::{self, IDENTITY, M3, P3};
use crate::util::quaternion_util::{self, Quat};
use crate::util::skyrim_util::{self as util, FAKE_FIRST_PERSON_ZOOM};

/// `Manager::State`: one frame's camera. IC's `localOffset` member is left out: nothing sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Present {
    pub modifier: Modifier,
    pub camera: CameraType,
    pub mode: CameraMode,
    /// Third person entered from first person (IC's `OStim` fix reads it).
    pub first_person: bool,
    pub frame: u64,
    pub position: P3,
    pub rotation: M3,
    pub orientation: Quat,
}

impl Default for Present {
    fn default() -> Self {
        Self { modifier: Modifier::default(), camera: CameraType::Unknown, mode: CameraMode::None, first_person: false, frame: 0, position: [0.0; 3], rotation: IDENTITY, orientation: Quat::IDENTITY }
    }
}

/// `CAMERA::Manager`.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Manager {
    /// `m_cameraState`.
    pub state: State,
    pub current: Present,
    pub previous: Present,
    pub transition_start: Present,
    pub present: Present,
    /// `m_previousCameraID`.
    pub previous_camera: CameraType,
    /// The type a transition blends from, `Unknown` when none runs.
    pub transition_from: CameraType,
    /// The camera held while a script locks the POV (`m_cameraLock`), `Unknown` when released.
    pub camera_lock: CameraType,
    pub transition_time: f32,
    /// The first-person arms hidden at the start of a blend into first person.
    pub transition_culled_arms: bool,
    pub look: LookController,
    pub forced_first_person: bool,
    pub forced_third_person: bool,
    /// The POV key toggled between first and third person (`m_togglePOV`): the next third-person
    /// update skips the engine's zoom out.
    pub toggle_pov: bool,
    /// Fake first person turning back to face forward (`m_orientate`).
    pub orientate: bool,
    pub frame: u64,
    head_follow: Quat,
    head_follow_frame: u64,
    head_anchor: P3,
    head_anchor_frame: u64,
    /// The world FOV was changed from outside and is followed instead of the state's (`m_deferFOV`).
    pub defer_fov: bool,
    pub external_fov: f32,
    /// The world FOV IC writes, eased toward its target (`m_resolveFOV`).
    pub resolve_fov: f32,
    /// A world FOV to follow from the next update (`NotifyFOVReset`), 0 for none.
    pub pending_fov_reset: f32,
    /// The free cameras' FOV steps of the frame (`NotifyFOVZoom`).
    pub zoom_fov_offset: f32,
    /// The zoom effect's toggle and its eased part of the FOV offset.
    pub zoom_active: bool,
    pub zoom_modifier: f32,
    /// The eased near plane (`UpdateNearDistance`'s `prevNear`), 0 until the first ease.
    pub near: f32,
    /// The dialogue's speaker focus shots are wanted (`DialogueState::IsFocusActive`).
    pub dialogue_focus: bool,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            state: State::Unknown,
            current: Present::default(),
            previous: Present::default(),
            transition_start: Present::default(),
            present: Present::default(),
            previous_camera: CameraType::Unknown,
            transition_from: CameraType::Unknown,
            camera_lock: CameraType::Unknown,
            transition_time: 0.0,
            transition_culled_arms: false,
            look: LookController::default(),
            forced_first_person: false,
            forced_third_person: false,
            toggle_pov: false,
            orientate: false,
            frame: 0,
            head_follow: Quat::IDENTITY,
            head_follow_frame: 0,
            head_anchor: [0.0; 3],
            head_anchor_frame: 0,
            defer_fov: false,
            external_fov: 80.0,
            resolve_fov: 80.0,
            pending_fov_reset: 0.0,
            zoom_fov_offset: 0.0,
            zoom_active: false,
            zoom_modifier: 0.0,
            near: 0.0,
            dialogue_focus: false,
        }
    }
}

pub static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Manager::default))
}

/// `ForceFirstPerson`: a script forced first person; noted only with the body on.
pub fn force_first_person() {
    if settings::profile().General.bEnableBody {
        with(|m| m.forced_first_person = true);
    }
}

/// `ForceThirdPerson`: a script forced third person; noted only with the body on.
pub fn force_third_person() {
    if settings::profile().General.bEnableBody {
        with(|m| m.forced_third_person = true);
    }
}

/// `NotifyCameraToggle`.
pub fn notify_camera_toggle() {
    with(|m| m.toggle_pov = true);
}

static NOTIFY_FREE_LOOK: AtomicBool = AtomicBool::new(false);

/// `NotifyFreeLook`: the look's base yaw is taken again at the next camera update. The body states
/// call it from their own update, which must not wait on the manager.
pub fn notify_free_look() {
    NOTIFY_FREE_LOOK.store(true, Ordering::Relaxed);
}

/// The base yaw a look reset takes: the camera actor's heading, else the camera root's.
fn look_base(camera: &PlayerCamera) -> Option<f32> {
    let root = camera.camera_root()?;
    Some(util::camera_actor_heading().unwrap_or_else(|| camera_heading(Quat::from_matrix(&root.local_transform().rotation).orientation().yaw)))
}

/// `CameraManager::Update`. An engine call a state makes that runs the camera update again
/// (`ForceFirstPerson` and the like) finds the manager busy and is skipped.
pub fn update(camera: &PlayerCamera) {
    let Ok(mut guard) = MANAGER.try_lock() else { return };
    let m = guard.get_or_insert_with(Manager::default);
    let delta = util::delta_frame_time();
    let profile = *settings::profile();
    let input = crate::frame_input();
    sync_idle_delay(&profile);
    m.update_camera_state(camera, &profile, &input);
    if NOTIFY_FREE_LOOK.swap(false, Ordering::Relaxed) {
        m.look.reset_yaw(look_base(camera));
    }
    m.capture_camera_state(camera, &profile);
    m.anchor_head_node(&profile, delta);
    m.transition_camera_state(&profile, delta);
    if profile.General.bEnableBody {
        m.present_camera(camera, &profile);
        m.update_fov(camera, &profile, delta);
        m.update_near_distance(camera, &profile.Advanced);
    }
    let data = m.snapshot();
    snapshot::set(data);
    drop(guard);
    crate::body::manager::anchor_camera(delta);
    let world_delta = util::delta_world_time();
    crate::body::free_look::drive(&profile, &data, world_delta);
    update_dialogue_head_tracking(camera, &profile, &data, world_delta);
}

/// The engine runs the player's head tracking only from the third-person camera states on (RE
/// `camera.md` IC5 C), so in the first-person, free and furniture states the dialogue's target is
/// followed only if the update is run here; the free look runs its own.
fn update_dialogue_head_tracking(camera: &PlayerCamera, profile: &Profile, data: &snapshot::Data, dt: f32) {
    let below_third = camera.state().is_some_and(|s| (s as u32) < CameraState::ThirdPerson as u32);
    if data.current == CameraType::Dialogue && profile.Dialogue.bEnable && profile.Dialogue.bHeadTracking && below_third && !crate::body::free_look::with(|f| f.driving)
        && let Some(player) = util::player()
    {
        player.update_head_tracking(dt);
    }
}

/// The engine values IC writes and the port puts back at unload: `fMinCurrentZoom`, the mount
/// and sitting pitch limits and `fAutoVanityModeDelay`, each as found before the first write.
#[derive(Clone, Copy, Debug, Default)]
struct Originals {
    min_zoom: Option<f32>,
    pitch_limits: Option<[f32; 5]>,
    idle_delay: Option<f32>,
}

static ORIGINALS: Mutex<Originals> = Mutex::new(Originals { min_zoom: None, pitch_limits: None, idle_delay: None });

fn originals<R>(f: impl FnOnce(&mut Originals) -> R) -> R {
    f(&mut ORIGINALS.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn set_min_zoom(zoom: f32) {
    let setting = util::min_current_zoom();
    originals(|o| { o.min_zoom.get_or_insert(*setting); });
    skyrim::store(setting, zoom);
}

/// The limits to raise: the horse's, the dragon's or sitting's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchLimits {
    Horse,
    Dragon,
    Sitting,
}

/// Raises pitch limits to 90 degrees: the mounts' as `HorseState` and `DragonState` do on entry,
/// sitting's as `FixesHook`'s delayed install does.
pub fn raise_pitch_limits(which: PitchLimits) {
    let limits = util::pitch_limits();
    originals(|o| { o.pitch_limits.get_or_insert(limits.map(|l| *l)); });
    let range = match which {
        PitchLimits::Horse => 0..2,
        PitchLimits::Dragon => 2..4,
        PitchLimits::Sitting => 4..5,
    };
    for limit in &limits[range] {
        skyrim::store(*limit, 90.0);
    }
}

/// `VanityState::SyncIdleDelay`: while IC owns the idle camera the engine's idle trigger runs on
/// the profile's delay; the game's own value goes back the moment it stops owning it.
fn sync_idle_delay(profile: &Profile) {
    let setting = util::auto_vanity_delay();
    if !super::base::vanity_enabled(profile) {
        if let Some(original) = originals(|o| o.idle_delay.take()) {
            skyrim::store(setting, original);
        }
        return;
    }
    originals(|o| { o.idle_delay.get_or_insert(*setting); });
    if (*setting - profile.Vanity.fIdleDelay).abs() > 0.01 {
        skyrim::store(setting, profile.Vanity.fIdleDelay);
    }
}

/// For an unload: the state's own engine values, the arms' fade, the POV switch and the settings
/// IC changed, back as they were found.
pub fn release() {
    with(|m| {
        if let Some(camera) = util::camera() {
            m.state.release(camera, &settings::profile());
        }
        m.cancel_transition();
    });
    let o = core::mem::take(&mut *ORIGINALS.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    if let Some(zoom) = o.min_zoom {
        skyrim::store(util::min_current_zoom(), zoom);
    }
    if let Some(limits) = o.pitch_limits {
        for (setting, value) in util::pitch_limits().into_iter().zip(limits) {
            skyrim::store(setting, value);
        }
    }
    if let Some(delay) = o.idle_delay {
        skyrim::store(util::auto_vanity_delay(), delay);
    }
    util::set_pov_switch(true);
}

/// The vanilla world near plane IC holds `fNearDistance` to at least.
const VANILLA_NEAR: f32 = 15.0;
/// How far the eased near moves before it is written again: every write moves the cascade
/// borders, the fog start and the depth precision (RE `graphics.md` section 3).
const NEAR_STEP: f32 = 1.0;
/// IC's threshold for a near that changed at all.
const NEAR_SETTLE: f32 = 0.05;

static ENGINE_NEAR: AtomicU32 = AtomicU32::new(VANILLA_NEAR.to_bits());

/// IC's load-time correction (`MainHook::Install`): an `fNearDistance` below 15 is raised to 15;
/// the value it then holds is the engine's near the resets put back.
pub fn init_near() {
    if skyrim::render::near_distance() < VANILLA_NEAR {
        skyrim::render::set_near_distance(VANILLA_NEAR);
    }
    ENGINE_NEAR.store(skyrim::render::near_distance().to_bits(), Ordering::Relaxed);
}

/// Puts the engine's world near plane back, in the setting and the frame camera together, as
/// IC's `Reset_FirstPerson` and the free camera's entry do.
pub fn reset_near() {
    let engine = f32::from_bits(ENGINE_NEAR.load(Ordering::Relaxed));
    if util::camera().is_some() && skyrim::render::near_distance().to_bits() != engine.to_bits() {
        skyrim::render::set_near_plane(engine);
    }
}

/// What `UpdateNearDistance` aims the world near plane at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NearTarget {
    /// Outside first person and the close third-person zoom: the default, written at once.
    Default(f32),
    /// Eased toward `near`; `pinned` while it rests at an end of the pitch range, on the default
    /// or on the scene near, where it is written exactly once reached.
    Ease { near: f32, pinned: bool, default: f32 },
}

/// `UpdateNearDistance`'s target from the camera type, the current third-person family state's
/// target zoom, the hands and action states and the camera root's pitch in degrees (negative
/// looking down); `None` on the map menu, which IC leaves alone.
#[must_use]
pub fn near_target(a: &Advanced, camera: CameraType, third_zoom: Option<f32>, hands_drawn: bool, sex_action: bool, pitch: f32) -> Option<NearTarget> {
    if camera == CameraType::MapMenu {
        return None;
    }
    let default = a.fNearDistanceDefault;
    if camera != CameraType::FirstPerson && camera != CameraType::SexTFC && third_zoom.is_none_or(|z| z > -0.27) {
        return Some(NearTarget::Default(default));
    }
    let (mut near_min, mut near_max, mut start, mut end) = if hands_drawn {
        (a.fNearDistanceCombatMin, a.fNearDistanceCombatMax, a.fNearDistanceCombatStartAngle, a.fNearDistanceCombatEndAngle)
    } else {
        (a.fNearDistanceMin, a.fNearDistanceMax, a.fNearDistanceStartAngle, a.fNearDistanceEndAngle)
    };
    // IC swaps both pairs, so the ramp runs from the larger near at the higher angle to the smaller.
    if near_max > near_min {
        core::mem::swap(&mut near_min, &mut near_max);
    }
    if end > start {
        core::mem::swap(&mut start, &mut end);
    }
    let norm = ((pitch - start) / (end - start)).clamp(0.0, 1.0);
    let mut near = near_min + (near_max - near_min) * norm;
    let mut pinned = norm <= 0.0 || norm >= 1.0;
    if pitch > start {
        near = default;
        pinned = true;
    }
    if camera == CameraType::SexTFC || (sex_action && third_zoom.is_some_and(|z| z <= FAKE_FIRST_PERSON_ZOOM)) {
        near = 2.0;
        pinned = true;
    }
    Some(NearTarget::Ease { near, pinned, default })
}

/// The player's sit/sleep phase as IC reads it for the mount cameras.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    None,
    Entering,
    Leaving,
}

impl Phase {
    fn of_player() -> Self {
        if util::is_entering_interaction() {
            Self::Entering
        } else if util::is_leaving_interaction() {
            Self::Leaving
        } else {
            Self::None
        }
    }
}

/// The camera type of the open menus, else the engine state's (the stats menu stops the vanity camera).
fn menu_type(engine: CameraType, menu_open: impl Fn(&str) -> bool) -> CameraType {
    if menu_open("MapMenu") {
        CameraType::MapMenu
    } else if menu_open("RaceSex Menu") {
        CameraType::RaceMenu
    } else if menu_open("StatsMenu") {
        CameraType::Unknown
    } else {
        engine
    }
}

/// The camera lock under a script's POV lock (`m_cameraLock`): taken on the camera of the moment,
/// moved to the race menu and back to first person after it, to first person for the intro's cart
/// and a forced first person, to third person for a forced third person, released with the lock.
/// `forced` is the forced first and third person (`m_forcedFirstperson`, `m_forcedThirdperson`).
#[must_use]
pub fn next_lock(lock: CameraType, script: bool, camera: CameraType, action: ActionType, forced: (bool, bool)) -> CameraType {
    if !script {
        return CameraType::Unknown;
    }
    if lock == CameraType::Unknown {
        camera
    } else if lock != CameraType::RaceMenu && camera == CameraType::RaceMenu {
        CameraType::RaceMenu
    } else if (lock == CameraType::RaceMenu && camera != CameraType::RaceMenu) || (lock == CameraType::ThirdPerson && action == ActionType::CartRide) || (lock != CameraType::FirstPerson && forced.0) {
        CameraType::FirstPerson
    } else if lock != CameraType::ThirdPerson && forced.1 {
        CameraType::ThirdPerson
    } else {
        lock
    }
}

/// The camera type IC derives from the engine state, the open menus, the mounting phase, the
/// body's action and race, death and a followed actor (`spectator`: the camera targets an actor
/// that is not the player, the player is not mounted and not viewing).
#[must_use]
pub fn camera_type(engine: CameraType, menu_open: impl Fn(&str) -> bool, phase: Phase, action: ActionType, race: RaceType, dead: bool, spectator: bool) -> CameraType {
    let mut camera = menu_type(engine, &menu_open);
    camera = match (camera, phase) {
        (CameraType::Dragon, Phase::Entering) => CameraType::DragonMounting,
        (CameraType::Dragon, Phase::Leaving) => CameraType::DragonDismounting,
        (CameraType::Horse, Phase::Entering) => CameraType::HorseMounting,
        (CameraType::Horse, Phase::Leaving) => CameraType::HorseDismounting,
        _ => camera,
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
        if spectator {
            camera = CameraType::Spectator;
        }
        if menu_open("Dialogue Menu") && !menu_open("BarterMenu") {
            camera = CameraType::Dialogue;
        }
    }
    if camera == CameraType::Free && action == ActionType::Sex {
        camera = CameraType::SexTFC;
    }
    camera
}

/// `IsValidCameraTransition` on plain values: whether a change from `from` to `to` blends.
#[must_use]
pub fn valid_transition(from: CameraType, to: CameraType, dialogue_smooth: bool) -> bool {
    if to == CameraType::MapMenu {
        return false;
    }
    if to == CameraType::Dialogue || from == CameraType::Dialogue {
        return dialogue_smooth;
    }
    if to == CameraType::FirstPerson {
        return from == CameraType::ThirdPerson;
    }
    true
}

/// IC's blend length between two states, seconds.
pub const TRANSITION_DURATION: f32 = 0.8;

/// `TransitionCameraState`'s blend: `present` moved from `start` toward `current` by the eased
/// share of `time`, and the share itself.
#[must_use]
pub fn blend(start: &Present, current: &Present, present: &Present, time: f32) -> (Present, f32) {
    let alpha = (time / TRANSITION_DURATION).clamp(0.0, 1.0);
    let eased = smoother_step(alpha);
    let orientation = Quat::slerp(start.orientation, current.orientation, eased);
    let mut out = *present;
    out.orientation = orientation;
    out.rotation = orientation.matrix();
    out.position = point::lerp(start.position, current.position, eased);
    out.modifier.fov = lerp(start.modifier.fov, current.modifier.fov, eased);
    (out, alpha)
}

/// Head-chase rate either end of `fHeadFollowStrength`: the 0.35 default lands on 6.
const HEAD_CHASE_MIN_RATE: f32 = 2.0;
const HEAD_CHASE_MAX_RATE: f32 = 14.0;

/// `Stabilization`: the fake first-person settings of a camera type; the mounting, dismounting
/// and beast transition types share their camera's.
#[must_use]
pub fn stabilization(profile: &Profile, camera: CameraType) -> Stab {
    let s = &profile.Stabilization;
    match camera {
        CameraType::Horse | CameraType::HorseMounting | CameraType::HorseDismounting => s.Horse,
        CameraType::Dragon | CameraType::DragonMounting | CameraType::DragonDismounting => s.Dragon,
        CameraType::Vampirelord | CameraType::VampirelordTransition => s.VampireLord,
        CameraType::Werewolf | CameraType::WerewolfTransition => s.Werewolf,
        CameraType::Lich | CameraType::LichTransition => s.Lich,
        CameraType::Sex => s.Sex,
        CameraType::Furniture => s.Furniture,
        CameraType::Crafting => s.Crafting,
        CameraType::Gathering => s.Gathering,
        CameraType::Bleedout | CameraType::Dead => s.Bleedout,
        _ => s.Transition,
    }
}

/// `HeadFollow`: whether the camera follows the head's rotation in this camera type; the beast
/// and lich forms only while an animation drives the actor (`animation_driven`). IC's exception
/// for Enderal's intro quest is left out with the Enderal detection.
#[must_use]
pub fn head_follow(camera: CameraType, body: &body_snapshot::Data, animation_driven: bool) -> bool {
    match camera {
        CameraType::Dragon | CameraType::Horse => body.movement == MovementType::Idle && body.hands <= HandsType::Sheath,
        CameraType::Sex => true,
        CameraType::Spectator => body.action == ActionType::Sex,
        CameraType::Vampirelord | CameraType::Werewolf | CameraType::Lich => animation_driven,
        _ => !matches!(body.action, ActionType::Bathe | ActionType::CartRide) && camera == CameraType::Animation,
    }
}

/// The horse camera's head basis: the head's forward flattened onto the ground, world up.
#[must_use]
pub fn level_basis(forward: P3) -> M3 {
    let (forward, _) = point::unitize([forward[0], forward[1], 0.0]);
    let up = [0.0, 0.0, 1.0];
    let (right, _) = point::unitize(point::cross(forward, up));
    let (up, _) = point::unitize(point::cross(right, forward));
    point::from_columns(right, forward, up)
}

/// `SmoothHeadAnchor`'s filter in the body's frame: the head point `local` chased from `previous`
/// at `speed`, held within `max_offset` of it.
#[must_use]
pub fn chase_anchor(previous: P3, local: P3, speed: f32, max_offset: f32, dt: f32) -> P3 {
    let chased = point::lerp(previous, local, damped_alpha(speed.max(0.1), dt));
    let lag = point::sub(chased, local);
    let len = point::length(lag);
    let max = max_offset.max(0.0);
    if len > max { point::add(local, point::scale(lag, max / len)) } else { chased }
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
    if let Some(game) = game_fov_modifier(camera, with(|m| m.present.camera)) {
        skyrim::camera::set_fov_offset(game);
    }
}

/// The first child of the actor's `NPC Root [Root]` (the centre of mass): its world axes.
fn body_directions(actor: &Actor) -> Option<(P3, P3, P3)> {
    let root = util::object_by_name(actor.get3_d()?, "NPC Root [Root]")?.cast::<sre::NiNode>()?;
    let com = root.children().first().copied().flatten()?;
    let r = com.world_transform().rotation;
    let column = |i: usize| point::unitize([r[0][i], r[1][i], r[2][i]]).0;
    Some((column(1), column(0), column(2)))
}

fn weapon_nodes() -> [&'static str; 3] {
    ["WEAPON", "NPC L MagicNode [LMag]", "NPC R MagicNode [RMag]"]
}

impl Manager {
    fn snapshot(&self) -> snapshot::Data {
        snapshot::Data {
            current: self.present.camera,
            previous: self.previous_camera,
            mode: self.present.mode,
            transition: self.transition_from,
            position: self.present.position,
            rotation: self.present.rotation,
            dialogue_focus: self.dialogue_focus,
        }
    }

    /// `UpdateCameraState`.
    fn update_camera_state(&mut self, camera: &PlayerCamera, profile: &Profile, input: &Input) {
        let body = body_snapshot::get();
        let engine = camera.current_state().map_or(CameraType::Unknown, |s| CameraType::from_state(s.id));
        let player = util::player();
        let dead = player.is_some_and(|p| p.is_dead(true));
        // The dragon camera sets the target back every frame (IC2 B), so it is never a spectator.
        let spectator = engine != CameraType::Dragon && util::camera_target().is_some() && player.is_some_and(|p| !p.is_on_mount()) && body.action != ActionType::View;
        let lock = next_lock(self.camera_lock, util::pov_script_lock(), menu_type(engine, util::is_menu_open), body.action, (self.forced_first_person, self.forced_third_person));
        if lock != self.camera_lock {
            crate::log(&format!("<Camera Lock> {:?} -> {lock:?}", self.camera_lock));
            self.camera_lock = lock;
        }
        let next = camera_type(engine, util::is_menu_open, Phase::of_player(), body.action, body.race, dead, spectator);
        let enabled = profile.General.bEnableBody;
        self.dialogue_focus = next == CameraType::Dialogue && super::extended::using_speaker_focus(&profile.Dialogue);
        let mut notes = Notes::default();
        let mut ctx = Ctx { camera, profile, body, notes: &mut notes, input, dialogue_focus: self.dialogue_focus };
        if self.state.kind() != next {
            self.set_camera(next, &mut ctx, enabled);
        }
        if enabled {
            self.state.update(&mut self.current.modifier, &mut ctx);
        }
        self.apply(notes);
        if self.current.first_person && self.current.camera == CameraType::Sex {
            self.current.first_person = false;
            if enabled && let Some(third) = camera.third_person() {
                util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
            }
        }
        // The engine zooms out from the minimum after a POV toggle; IC lands on the target at once.
        if self.toggle_pov && self.current.camera == CameraType::ThirdPerson {
            self.toggle_pov = false;
            if enabled && let Some(third) = camera.third_person() {
                skyrim::store(&third.currentZoom, third.targetZoom);
            }
        }
    }

    fn apply(&mut self, notes: Notes) {
        self.forced_first_person |= notes.forced_first_person;
        if notes.fov_reset > 0.0 {
            self.pending_fov_reset = notes.fov_reset;
        }
        self.zoom_fov_offset += notes.fov_zoom;
    }

    /// `SetCamera`.
    fn set_camera(&mut self, next: CameraType, ctx: &mut Ctx<'_>, enabled: bool) {
        let (state, look) = State::new(next);
        let mut previous = core::mem::replace(&mut self.state, state);
        if enabled {
            previous.exit(&mut self.current.modifier, ctx);
        }
        let from = previous.kind();
        self.previous_camera = from;
        self.transition_from = from;
        self.transition_time = 0.0;
        self.transition_start = self.current;
        crate::log(&format!("<Camera> Exit: {from:?}"));
        if core::mem::take(&mut self.forced_first_person) && next == CameraType::FirstPerson {
            self.cancel_transition();
        }
        self.look.mode = look;
        if let Some(camera) = util::camera() {
            self.look.reset(look_base(camera));
        }
        if enabled {
            self.state.enter(from, ctx);
        }
        if next == CameraType::Dialogue {
            self.cancel_transition();
        }
        crate::log(&format!("<Camera> Enter: {next:?}"));
    }

    /// `CaptureCameraState`, with its fix while getting up from furniture in an animation in fake
    /// first person (the player's position and angles stand in for the camera's), and the shots
    /// the dialogue's speaker focus and the idle camera author.
    fn capture_camera_state(&mut self, camera: &PlayerCamera, profile: &Profile) {
        self.previous = self.current;
        self.frame += 1;
        self.current.camera = self.state.kind();
        if self.current.camera == CameraType::ThirdPerson {
            self.current.first_person = self.previous.camera == CameraType::FirstPerson;
        }
        let (mut position, mut rotation) = ([0.0; 3], IDENTITY);
        if let Some(root) = camera.camera_root() {
            let local = root.local_transform();
            (position, rotation) = (local.translation, local.rotation);
            if util::is_leaving_interaction()
                && self.current.camera == CameraType::Animation
                && camera.third_person().is_some_and(|t| t.currentZoom <= FAKE_FIRST_PERSON_ZOOM)
                && let Some(player) = util::player()
            {
                position[0] = player.posX;
                position[1] = player.posY;
                rotation = rotation_from_angles(player.rotX, player.rotY, player.rotZ);
            }
            if self.look.mode != LookMode::Disabled {
                let o = Quat::from_matrix(&rotation).orientation();
                self.look.update(o.yaw, o.pitch);
            }
        }
        self.current.mode = self.set_mode(camera, profile);
        let shot = match &mut self.state {
            State::Dialogue(state) if profile.Dialogue.bEnable => state.speaker_focus(camera, profile, self.dialogue_focus),
            State::Vanity(state) if super::base::vanity_enabled(profile) => state.shot,
            _ => None,
        };
        if let Some(shot) = shot {
            (position, rotation) = shot;
        }
        self.current.position = position;
        self.current.rotation = rotation;
        self.current.orientation = Quat::from_matrix(&rotation);
        self.current.frame = self.frame;
        self.present = self.current;
        if self.current.modifier.fov == 0.0 {
            self.current.modifier.fov = camera.worldFOV;
        }
    }

    /// `SetMode`.
    fn set_mode(&mut self, camera: &PlayerCamera, profile: &Profile) -> CameraMode {
        let current = self.current.camera;
        if !profile.General.bEnableBody || matches!(current, CameraType::MapMenu | CameraType::RaceMenu) {
            return CameraMode::None;
        }
        let id = current as u32;
        let family = if (CameraState::ThirdPerson as u32..=CameraState::Dragon as u32).contains(&id) { CameraState::from_u32(id).unwrap_or(CameraState::ThirdPerson) } else { CameraState::ThirdPerson };
        let Some(third) = util::family(camera, family) else { return CameraMode::None };
        let min_zoom = *util::min_current_zoom();
        let fake = profile.General.bEnableFakeFirstPerson;
        let mut sim = self.current.modifier.sim_first_person;
        if self.forced_third_person && self.previous.camera == CameraType::FirstPerson {
            self.forced_third_person = false;
            sim = true;
            self.current.modifier.sim_first_person = true;
            util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
        }
        if self.forced_third_person && current == CameraType::ThirdPerson {
            self.forced_third_person = false;
        }
        let lock = self.camera_lock;
        let in_first = camera.state() == Some(CameraState::FirstPerson);
        if fake {
            util::set_pov_switch(false);
            if !in_first && lock == CameraType::FirstPerson {
                camera.force_first_person();
                sim = false;
            } else if in_first && lock > CameraType::FirstPerson {
                camera.force_third_person();
                sim = true;
            } else {
                sim = lock != CameraType::FirstPerson && camera.state() != Some(CameraState::Free);
            }
            self.current.modifier.sim_first_person = sim;
        }
        if self.forced_first_person && current == CameraType::FirstPerson {
            self.forced_first_person = false;
        }
        if current == CameraType::Animation && lock == CameraType::FirstPerson {
            if !in_first {
                camera.force_first_person();
            }
            self.forced_third_person = false;
            return if body_snapshot::get().action == ActionType::CartRide { CameraMode::Head } else { CameraMode::First };
        }
        if sim {
            if min_zoom > -0.27 {
                set_min_zoom(FAKE_FIRST_PERSON_ZOOM);
            }
            let action = body_snapshot::get().action;
            if current == CameraType::FirstPerson && !fake && !matches!(action, ActionType::Animation | ActionType::View | ActionType::Sit | ActionType::Sleep) {
                util::set_zoom(third, -0.200);
                self.current.modifier.sim_first_person = false;
                return CameraMode::First;
            }
            return if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM { CameraMode::Third } else { CameraMode::None };
        }
        let clean = |third: &ThirdPersonState| {
            if min_zoom <= FAKE_FIRST_PERSON_ZOOM {
                set_min_zoom(-0.200);
                if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM {
                    util::set_zoom(third, -0.200);
                }
                true
            } else {
                false
            }
        };
        match current {
            CameraType::FirstPerson => {
                let a = &profile.Advanced;
                let drawn = body_snapshot::get().hands > HandsType::None;
                clean(third);
                if (a.bHeadbob && !drawn) || (a.bHeadbobCombat && drawn) { CameraMode::Head } else { CameraMode::First }
            }
            CameraType::ThirdPerson => {
                if clean(third) {
                    util::set_pov_switch(true);
                }
                CameraMode::None
            }
            _ => CameraMode::None,
        }
    }

    /// `CancelCameraTransition`.
    pub fn cancel_transition(&mut self) {
        self.transition_from = CameraType::Unknown;
        self.transition_time = 0.0;
        if self.transition_culled_arms
            && let Some(first) = util::player().and_then(|p| p.first_person_root())
        {
            skyrim::store(&first.fadeAmount, 1.0);
            self.transition_culled_arms = false;
        }
    }

    /// `TransitionCameraState`: a POV or state change blends over 0.8 s, eased.
    fn transition_camera_state(&mut self, profile: &Profile, dt: f32) {
        if !profile.Advanced.bCameraTransition {
            self.cancel_transition();
            return;
        }
        if self.transition_from == CameraType::Unknown {
            self.transition_time = 0.0;
            return;
        }
        let to = self.present.camera;
        if to == CameraType::MapMenu {
            // IC's fix of the map opening zoomed in further than normal.
            skyrim::camera::set_fov_offset(0.0);
        }
        if !valid_transition(self.transition_from, to, profile.Dialogue.bSmoothTransition) || self.present.mode == CameraMode::Third {
            self.cancel_transition();
            return;
        }
        // Entering first person: the player's pitch synced to where the third-person camera looked
        // and the arms hidden, once; a pitch write every frame would lock the vertical look.
        if self.transition_time == 0.0
            && self.transition_from == CameraType::ThirdPerson
            && to == CameraType::FirstPerson
            && let Some(player) = util::player()
        {
            util::set_pitch(player, self.transition_start.orientation.orientation().pitch);
            if let Some(first) = player.first_person_root() {
                skyrim::store(&first.fadeAmount, 0.0);
                self.transition_culled_arms = true;
            }
        }
        self.transition_time += dt;
        let (present, alpha) = blend(&self.transition_start, &self.current, &self.present, self.transition_time);
        self.present = present;
        if alpha > 0.1
            && self.transition_culled_arms
            && let Some(first) = util::player().and_then(|p| p.first_person_root())
        {
            skyrim::store(&first.fadeAmount, 1.0);
            self.transition_culled_arms = false;
        }
        if alpha >= 1.0 {
            self.cancel_transition();
        }
    }

    /// `AnchorHeadNode`: in the head-bob and fake first-person modes, the presented camera on the
    /// followed actor's head (the player's when mounted).
    fn anchor_head_node(&mut self, profile: &Profile, dt: f32) {
        if !matches!(self.present.mode, CameraMode::Head | CameraMode::Third) || self.transition_from != CameraType::Unknown {
            return;
        }
        let Some(player) = util::player() else { return };
        let target = util::camera_target();
        let mounted = matches!(self.present.camera, CameraType::Horse | CameraType::HorseDismounting | CameraType::HorseMounting | CameraType::Dragon | CameraType::DragonDismounting | CameraType::DragonMounting);
        let actor: &Actor = match target.as_deref().and_then(util::actor_of) {
            Some(a) if !mounted => a,
            _ => player,
        };
        let Some(head) = util::actor_head(actor) else { return };
        let Some(directions) = body_directions(actor) else { return };
        self.present.rotation = self.rotate(actor, head, profile, dt);
        if let Some(position) = self.base_position(player, actor, head, directions, profile, dt) {
            self.present.position = position;
        }
    }

    /// `Rotate`: fake first person turns back to face forward after a zoom change, and follows
    /// the head (chased at the stabilization's strength) with the look's pitch and the free
    /// rotation around it where `head_follow` says so.
    fn rotate(&mut self, actor: &Actor, head: &NiAVObject, profile: &Profile, dt: f32) -> M3 {
        let camera_type = self.present.camera;
        let mut free_rotation = 0.0;
        if let Some(third) = util::camera().and_then(util::current_family) {
            free_rotation = third.freeRotation.x;
            #[allow(clippy::float_cmp)]
            let zooming = third.currentZoom != third.targetZoom;
            if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM && zooming && !self.orientate {
                self.orientate = camera_type != CameraType::Spectator;
            }
            if self.orientate {
                free_rotation = lerp(normalize_relative_angle(free_rotation), 0.0, damped_alpha(4.5, dt));
                skyrim::store(&third.freeRotation.x, free_rotation);
                if !zooming {
                    skyrim::store(&third.freeRotation.x, 0.0);
                    free_rotation = 0.0;
                    self.orientate = false;
                }
            }
        }
        let body = body_snapshot::get();
        if head_follow(camera_type, &body, actor.is_animation_driven()) {
            let mut head_rotation = head.world_transform().rotation;
            if camera_type != CameraType::Animation {
                let strength = stabilization(profile, camera_type).fHeadFollowStrength.clamp(0.0, 1.0);
                let rate = HEAD_CHASE_MIN_RATE + (HEAD_CHASE_MAX_RATE - HEAD_CHASE_MIN_RATE) * strength;
                let mut orientation = Quat::from_matrix(&head_rotation);
                if self.head_follow_frame + 1 == self.frame {
                    orientation = Quat::slerp(self.head_follow, orientation, damped_alpha(rate, dt));
                }
                self.head_follow = orientation;
                self.head_follow_frame = self.frame;
                head_rotation = orientation.matrix();
            }
            let forward = point::mul_vec(&head_rotation, [0.0, 1.0, 0.0]);
            let up = point::mul_vec(&head_rotation, [0.0, 0.0, 1.0]);
            let (mut right, _) = point::unitize(point::cross(up, forward));
            let (mut up, _) = point::unitize(point::cross(forward, right));
            let horse = camera_type == CameraType::Horse;
            if horse {
                head_rotation = level_basis(forward);
                right = [head_rotation[0][0], head_rotation[1][0], head_rotation[2][0]];
                up = [head_rotation[0][2], head_rotation[1][2], head_rotation[2][2]];
            }
            let pitch = if horse { -self.look.state.pitch } else { self.look.state.pitch };
            let heading = Quat::from_axis_angle(up, -free_rotation).matrix();
            let tilt = Quat::from_axis_angle(right, pitch).matrix();
            return point::mul(&point::mul(&heading, &tilt), &head_rotation);
        }
        if camera_type == CameraType::Spectator
            && actor.get3_d().is_some()
            && unsafe { actor.movementController.as_ref() }.is_some_and(sre::MovementControllerNPC::is_controls_driven)
            && !actor.is_dead(true)
        {
            return self.present.rotation;
        }
        match camera_type {
            CameraType::FirstPerson | CameraType::ThirdPerson | CameraType::Horse | CameraType::Dragon | CameraType::TweenMenu | CameraType::Animation | CameraType::Vampirelord | CameraType::Werewolf | CameraType::Lich => self.present.rotation,
            _ => head.world_transform().rotation,
        }
    }

    /// `SmoothHeadAnchor`: in fake first person with the stabilization on, the head point damped
    /// in the body's frame (travel and turning stay one to one) and leashed to the head.
    fn smooth_head_anchor(&mut self, profile: &Profile, body: &NiAVObject, anchor: P3, dt: f32) -> P3 {
        let s = stabilization(profile, self.present.camera);
        if !s.bEnable || self.present.mode != CameraMode::Third {
            self.head_anchor_frame = 0;
            return anchor;
        }
        let world = body.world_transform();
        let scale = if world.scale > 0.0001 { world.scale } else { 1.0 };
        let local = point::scale(point::mul_vec(&point::transpose(&world.rotation), point::sub(anchor, world.translation)), 1.0 / scale);
        self.head_anchor = if self.head_anchor_frame + 1 == self.frame { chase_anchor(self.head_anchor, local, s.fSmoothingSpeed, s.fSmoothingMaxOffset, dt) } else { local };
        self.head_anchor_frame = self.frame;
        point::add(world.translation, point::mul_vec(&world.rotation, point::scale(self.head_anchor, scale)))
    }

    /// `BasePosition`.
    fn base_position(&mut self, player: &PlayerCharacter, actor: &Actor, head: &NiAVObject, (forward, right, up): (P3, P3, P3), profile: &Profile, dt: f32) -> Option<P3> {
        let actor_body = actor.get3_d()?;
        let scale = actor_body.local.scale;
        let camera_type = self.present.camera;
        let eye = util::actor_eye(actor).or_else(|| match camera_type {
            CameraType::Vampirelord => util::object_by_name(actor_body, "NPC EyeBrow"),
            CameraType::Lich => util::object_by_name(actor_body, "DragPriestNPC Head [Brow]"),
            _ => None,
        });
        let a = &profile.Advanced;
        let mut headbob = a.bHeadbob || camera_type > CameraType::IronSight;
        if headbob && !a.bHeadbobCombat && camera_type == CameraType::FirstPerson && util::is_weapon_drawn(player) {
            headbob = false;
        }
        let action = body_snapshot::get().action;
        let offset = if matches!(action, ActionType::None | ActionType::Ride | ActionType::Sex) { self.current.modifier.local_offset } else { [0.0; 3] };
        let along = |f: f32, r: f32, u: f32| point::add(point::add(point::scale(forward, f), point::scale(right, r)), point::scale(up, u));
        if headbob || self.camera_lock == CameraType::FirstPerson {
            let point = eye.filter(|e| e.world_transform().scale >= 0.1).unwrap_or(head).world_transform().translation;
            let anchor = self.smooth_head_anchor(profile, actor_body, point, dt);
            return Some(point::add(anchor, point::scale(along(offset[1] + 8.0, offset[0], offset[2] + 1.0), scale)));
        }
        let shift = point::scale(along(offset[1] + scale * 0.5, offset[0], offset[2]), scale);
        let first = player.get3_d_for(true);
        let camera1st = first.and_then(|f| util::object_by_name(f, "Camera1st [Cam1]"));
        Some(match (first, camera1st) {
            (Some(first), Some(cam)) if !util::is_sitting(actor) => point::add(point::add(first.world_transform().translation, point::scale(cam.local_transform().translation, scale)), shift),
            _ => point::add(self.present.position, shift),
        })
    }

    /// `ResolveWeaponPositions`: with the first-person arms drawn, the weapon and magic nodes
    /// follow the presented camera's move off the engine's.
    fn resolve_weapon_positions(&self, profile: &Profile) {
        if !profile.General.bEnableFirstPersonArms || body_snapshot::get().hands == HandsType::None || self.present.camera != CameraType::FirstPerson {
            return;
        }
        let delta = point::sub(self.present.position, self.current.position);
        let Some(first) = util::player().and_then(|p| p.first_person_root()) else { return };
        for name in weapon_nodes() {
            let Some(node) = util::object_by_name(first, name) else { continue };
            let Some(parent) = (unsafe { node.parent.cast::<NiAVObject>().as_ref() }) else { continue };
            let local_offset = Quat::from_matrix(&parent.world_transform().rotation).inverse().rotate(delta);
            let mut local = node.local_transform();
            local.translation = point::add(local.translation, local_offset);
            node.set_local_transform(local);
            node.update_transforms();
        }
    }

    /// `PresentCamera`.
    fn present_camera(&self, camera: &PlayerCamera, profile: &Profile) {
        let Some(root) = camera.camera_root() else { return };
        let mut local = root.local_transform();
        local.translation = self.present.position;
        local.rotation = self.present.rotation;
        root.set_local_transform(local);
        self.resolve_weapon_positions(profile);
        root.update_transforms();
    }

    /// `UpdateNearDistance`.
    fn update_near_distance(&mut self, camera: &PlayerCamera, advanced: &Advanced) {
        let body = body_snapshot::get();
        let pitch = camera.camera_root().map_or(0.0, |root| -quaternion_util::pitch(&root.local_transform().rotation).to_degrees());
        let Some(target) = near_target(advanced, self.present.camera, util::current_third_person_zoom(camera), body.hands != HandsType::None, body.action == ActionType::Sex, pitch) else { return };
        if let Some(near) = self.resolve_near(target, skyrim::render::near_distance()) {
            skyrim::render::set_near_plane(near);
        }
    }

    /// The near plane to write this frame, if any, given the one the engine holds: the default at
    /// once; otherwise IC's ease of 0.15 a frame, written when it is a whole step away from the
    /// engine's, so a camera hovering on the ramp does not rewrite it every frame, or exactly
    /// once it reached a pinned target.
    pub fn resolve_near(&mut self, target: NearTarget, current: f32) -> Option<f32> {
        match target {
            NearTarget::Default(near) => (current.to_bits() != near.to_bits()).then_some(near),
            NearTarget::Ease { near: target, pinned, default } => {
                if self.near.is_nan() || self.near <= 0.0 {
                    self.near = default;
                }
                self.near = lerp(self.near, target, 0.15);
                if (self.near - current).abs() >= NEAR_STEP {
                    Some(self.near)
                } else if pinned && (self.near - target).abs() < NEAR_SETTLE && (current - target).abs() > NEAR_SETTLE {
                    Some(target)
                } else {
                    None
                }
            }
        }
    }

    /// `UpdateFOV`: writes the eased world FOV and, where the engine drives one, the FOV offset.
    fn update_fov(&mut self, camera: &PlayerCamera, profile: &Profile, delta: f32) {
        let external_owner = self.present.camera == CameraType::TweenMenu || util::is_anim_camera();
        let zoom = if self.zoom_active { profile.Zoom.fFOV } else { 0.0 };
        let (world, offset) = self.resolve_fov(camera.worldFOV, external_owner, game_fov_modifier(camera, self.present.camera), zoom, delta);
        skyrim::store(&camera.worldFOV, world);
        if let Some(offset) = offset {
            skyrim::camera::set_fov_offset(offset);
        }
    }

    /// `UpdateFOV` on plain values: from the FOV the presented state asks for and the world FOV
    /// the engine holds, the world FOV to write; with the engine's own offset (`game`), the FOV
    /// offset to write, which adds the zoom effect's eased `zoom` and keeps the sum within 5 to
    /// 165 degrees. A world FOV changed from outside (the console, another mod) is followed until
    /// IC's settles on it; the tween menu and an animator camera keep their own; the free cameras
    /// own the world FOV and take the frame's zoom steps.
    pub fn resolve_fov(&mut self, world: f32, external_owner: bool, game: Option<f32>, zoom: f32, delta: f32) -> (f32, Option<f32>) {
        let mut ic = self.present.modifier.fov;
        let mut current = world;
        // The map menu's first frames hold garbage.
        if !(1.0..=180.0).contains(&current) {
            current = ic;
            self.resolve_fov = ic;
        }
        if matches!(self.present.camera, CameraType::Free | CameraType::Cinematic) {
            if self.pending_fov_reset > 0.0 {
                current = self.pending_fov_reset;
                self.pending_fov_reset = 0.0;
            }
            self.defer_fov = false;
            self.resolve_fov = (current + self.zoom_fov_offset).clamp(5.0, 165.0);
            self.zoom_fov_offset = 0.0;
            return (self.resolve_fov, None);
        }
        if self.pending_fov_reset > 0.0 {
            self.defer_fov = true;
            self.external_fov = self.pending_fov_reset;
            self.pending_fov_reset = 0.0;
            self.zoom_fov_offset = 0.0;
        }
        if self.defer_fov {
            let settled = (self.resolve_fov - self.external_fov).abs() < 0.1;
            if settled && (current - self.external_fov).abs() < 0.1 {
                self.defer_fov = false;
                ic = self.external_fov;
                self.current.modifier.fov = ic;
            }
        } else {
            let settled = (self.resolve_fov - ic).abs() < 0.1;
            if ic > 0.0 && settled && (current - ic).abs() > 0.1 {
                self.defer_fov = true;
                self.external_fov = current;
            }
        }
        let alpha = damped_alpha(6.0, delta);
        let mut target = if self.defer_fov { self.external_fov } else { ic + self.zoom_fov_offset };
        if target < 0.1 {
            target = self.external_fov;
        }
        self.resolve_fov = lerp(self.resolve_fov, target, alpha);
        self.zoom_fov_offset = 0.0;
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
    use super::{Manager, NearTarget, Phase, Present, blend, camera_type, chase_anchor, head_follow, level_basis, near_target, stabilization, valid_transition};
    use crate::body::snapshot::Data as Body;
    use crate::body::state::action::ActionType;
    use crate::body::state::hands::HandsType;
    use crate::body::state::movement::MovementType;
    use crate::body::state::race::RaceType;
    use crate::camera::state::CameraType;
    use crate::settings::{Advanced, Profile};
    use crate::util::math_util::lerp;
    use crate::util::point;
    use crate::util::quaternion_util::Quat;

    #[test]
    fn menus_actions_races_and_a_followed_actor_override_the_engine_state() {
        let none = |_: &str| false;
        let t = |engine, menus: fn(&str) -> bool, action, race, dead, spectator| camera_type(engine, menus, Phase::None, action, race, dead, spectator);
        assert_eq!(t(CameraType::FirstPerson, none, ActionType::None, RaceType::Nord, false, false), CameraType::FirstPerson);
        assert_eq!(t(CameraType::FirstPerson, |m| m == "MapMenu", ActionType::Sit, RaceType::Nord, false, false), CameraType::MapMenu);
        assert_eq!(t(CameraType::ThirdPerson, none, ActionType::Sit, RaceType::Nord, false, false), CameraType::Animation);
        assert_eq!(t(CameraType::ThirdPerson, none, ActionType::None, RaceType::Werebear, false, false), CameraType::Werewolf);
        assert_eq!(t(CameraType::Bleedout, none, ActionType::None, RaceType::Nord, true, false), CameraType::Dead);
        assert_eq!(t(CameraType::FirstPerson, |m| m == "Dialogue Menu", ActionType::Dialogue, RaceType::Nord, false, true), CameraType::Dialogue, "dialogue wins over the spectator");
        assert_eq!(t(CameraType::ThirdPerson, none, ActionType::None, RaceType::Werewolf, false, true), CameraType::Spectator, "and the spectator over the race");
        assert_eq!(t(CameraType::Free, none, ActionType::None, RaceType::Nord, false, true), CameraType::Free, "never in the free camera");
        assert_eq!(t(CameraType::Free, none, ActionType::Sex, RaceType::Nord, false, false), CameraType::SexTFC);
        assert_eq!(t(CameraType::ThirdPerson, |m| m == "StatsMenu", ActionType::None, RaceType::Nord, false, false), CameraType::Unknown);
    }

    #[test]
    fn transitions_blend_into_first_person_only_from_third_and_never_to_the_map() {
        assert!(valid_transition(CameraType::ThirdPerson, CameraType::FirstPerson, true));
        assert!(!valid_transition(CameraType::Furniture, CameraType::FirstPerson, true));
        assert!(!valid_transition(CameraType::ThirdPerson, CameraType::MapMenu, true));
        assert!(!valid_transition(CameraType::FirstPerson, CameraType::Dialogue, false));
        assert!(valid_transition(CameraType::Dialogue, CameraType::FirstPerson, true), "the dialogue setting decides first");
        assert!(valid_transition(CameraType::FirstPerson, CameraType::ThirdPerson, false));
    }

    #[test]
    fn the_blend_eases_position_rotation_and_fov_over_0_8_seconds() {
        let start = Present { position: [0.0, 0.0, 0.0], orientation: Quat::IDENTITY, modifier: crate::camera::state::Modifier { fov: 70.0, ..Default::default() }, ..Present::default() };
        let turned = Quat::from_axis_angle([0.0, 0.0, 1.0], 1.0);
        let current = Present { position: [100.0, 0.0, 0.0], orientation: turned, modifier: crate::camera::state::Modifier { fov: 90.0, ..Default::default() }, ..Present::default() };
        let (half, alpha) = blend(&start, &current, &current, 0.4);
        assert!((alpha - 0.5).abs() < 1e-6);
        assert!((half.position[0] - 50.0).abs() < 1e-3 && (half.modifier.fov - 80.0).abs() < 1e-3, "{half:?}: the quintic ease is 0.5 at half time");
        let (early, _) = blend(&start, &current, &current, 0.08);
        assert!(early.position[0] < 1.0, "{early:?}: flat start");
        let (end, alpha) = blend(&start, &current, &current, 2.0);
        assert!((alpha - 1.0).abs() < f32::EPSILON && (end.position[0] - 100.0).abs() < 1e-4);
        let forward = point::mul_vec(&end.rotation, [0.0, 1.0, 0.0]);
        let expected = turned.forward();
        assert!((0..3).all(|i| (forward[i] - expected[i]).abs() < 1e-5), "the rotation matrix follows the slerped orientation");
    }

    #[test]
    fn the_head_anchor_lags_within_its_leash() {
        let mut p = chase_anchor([0.0; 3], [0.0, 0.0, 10.0], 14.0, 6.0, 1.0 / 60.0);
        assert!((p[2] - 4.0).abs() < 1e-4, "{p:?}: a 10-unit jump is held 6 behind the head");
        p = chase_anchor(p, [0.0, 0.0, 10.0], 14.0, 6.0, 1.0 / 60.0);
        assert!(p[2] > 4.0 && p[2] < 6.0, "{p:?}: then chased");
        for _ in 0..120 {
            p = chase_anchor(p, [0.0, 0.0, 10.0], 14.0, 6.0, 1.0 / 60.0);
        }
        assert!((p[2] - 10.0).abs() < 0.01, "{p:?}: settles on the head");
        assert_eq!(chase_anchor([1.0, 0.0, 0.0], [0.0; 3], 14.0, -1.0, 0.016), [0.0; 3], "a negative leash pins it");
    }

    #[test]
    fn the_head_is_followed_where_ic_follows_it() {
        let body = |movement, hands, action| Body { movement, action, hands, ..Body::NONE };
        assert!(head_follow(CameraType::Horse, &body(MovementType::Idle, HandsType::Sheath, ActionType::Ride), false));
        assert!(!head_follow(CameraType::Horse, &body(MovementType::Run, HandsType::None, ActionType::Ride), false));
        assert!(!head_follow(CameraType::Dragon, &body(MovementType::Idle, HandsType::Show, ActionType::None), false));
        assert!(head_follow(CameraType::Sex, &Body::NONE, false));
        assert!(head_follow(CameraType::Spectator, &body(MovementType::None, HandsType::None, ActionType::Sex), false));
        assert!(head_follow(CameraType::Animation, &body(MovementType::None, HandsType::None, ActionType::Sit), false));
        assert!(!head_follow(CameraType::Animation, &body(MovementType::None, HandsType::None, ActionType::CartRide), false));
        assert!(!head_follow(CameraType::Furniture, &Body::NONE, false));
        assert!(!head_follow(CameraType::Werewolf, &Body::NONE, false) && head_follow(CameraType::Werewolf, &Body::NONE, true), "beast forms while an animation drives them");
    }

    #[test]
    fn the_sit_phase_turns_the_mount_cameras_into_mounting_and_dismounting() {
        let none = |_: &str| false;
        let t = |engine, phase| camera_type(engine, none, phase, ActionType::None, RaceType::Nord, false, false);
        assert_eq!(t(CameraType::Horse, Phase::Entering), CameraType::HorseMounting);
        assert_eq!(t(CameraType::Horse, Phase::Leaving), CameraType::HorseDismounting);
        assert_eq!(t(CameraType::Dragon, Phase::Entering), CameraType::DragonMounting);
        assert_eq!(t(CameraType::Dragon, Phase::Leaving), CameraType::DragonDismounting);
        assert_eq!(t(CameraType::ThirdPerson, Phase::Leaving), CameraType::ThirdPerson);
    }

    #[test]
    fn the_script_lock_follows_ic_rules() {
        use super::next_lock;
        const NONE: (bool, bool) = (false, false);
        let l = |lock, script, camera, action, forced| next_lock(lock, script, camera, action, forced);
        assert_eq!(l(CameraType::Unknown, true, CameraType::ThirdPerson, ActionType::None, NONE), CameraType::ThirdPerson, "set to the camera");
        assert_eq!(l(CameraType::ThirdPerson, true, CameraType::RaceMenu, ActionType::None, NONE), CameraType::RaceMenu);
        assert_eq!(l(CameraType::RaceMenu, true, CameraType::ThirdPerson, ActionType::None, NONE), CameraType::FirstPerson, "leaving the race menu");
        assert_eq!(l(CameraType::ThirdPerson, true, CameraType::Animation, ActionType::CartRide, NONE), CameraType::FirstPerson, "the intro's cart");
        assert_eq!(l(CameraType::Furniture, true, CameraType::Furniture, ActionType::None, (true, false)), CameraType::FirstPerson, "forced first person");
        assert_eq!(l(CameraType::FirstPerson, true, CameraType::FirstPerson, ActionType::None, (false, true)), CameraType::ThirdPerson, "forced third person");
        assert_eq!(l(CameraType::Furniture, true, CameraType::Furniture, ActionType::None, (true, true)), CameraType::FirstPerson, "first person wins");
        assert_eq!(l(CameraType::Furniture, true, CameraType::Animation, ActionType::None, NONE), CameraType::Furniture, "kept");
        assert_eq!(l(CameraType::FirstPerson, false, CameraType::ThirdPerson, ActionType::None, NONE), CameraType::Unknown, "released");
    }

    #[test]
    fn the_horse_basis_is_level_and_keeps_the_heading() {
        let b = level_basis([0.6, 0.6, -0.5]);
        let forward = [b[0][1], b[1][1], b[2][1]];
        let up = [b[0][2], b[1][2], b[2][2]];
        assert!(forward[2].abs() < 1e-6 && (forward[0] - forward[1]).abs() < 1e-6 && (point::length(forward) - 1.0).abs() < 1e-5);
        assert!((up[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stabilization_is_shared_by_a_camera_and_its_transitions() {
        let mut profile = Profile::default();
        profile.Stabilization.Werewolf.fSmoothingSpeed = 3.0;
        profile.Stabilization.Transition.fSmoothingSpeed = 7.0;
        assert!((stabilization(&profile, CameraType::WerewolfTransition).fSmoothingSpeed - 3.0).abs() < f32::EPSILON);
        assert!((stabilization(&profile, CameraType::ThirdPerson).fSmoothingSpeed - 7.0).abs() < f32::EPSILON);
    }

    fn presenting(camera: CameraType, fov: f32) -> Manager {
        let mut m = Manager::default();
        m.present.camera = camera;
        m.present.modifier.fov = fov;
        m.current = m.present;
        m
    }

    /// Runs `frames` updates of 1/60 s with the engine holding what was written last; the last world FOV.
    fn settle(m: &mut Manager, mut world: f32, frames: usize) -> f32 {
        for _ in 0..frames {
            world = m.resolve_fov(world, false, None, 0.0, 1.0 / 60.0).0;
        }
        world
    }

    /// Updates while the world FOV follows an outside value; the world FOV when it settled.
    fn follow(m: &mut Manager, mut world: f32) -> f32 {
        for _ in 0..200 {
            if !m.defer_fov {
                break;
            }
            world = settle(m, world, 1);
        }
        world
    }

    #[test]
    fn the_world_fov_eases_to_the_state_and_follows_an_outside_change() {
        let mut m = presenting(CameraType::FirstPerson, 90.0);
        let (world, offset) = m.resolve_fov(80.0, false, None, 0.0, 1.0 / 60.0);
        assert!(world > 80.0 && world < 90.0, "{world}: a tenth of the way per 1/60 s");
        assert_eq!(offset, None);
        let world = settle(&mut m, world, 120);
        assert!((world - 90.0).abs() < 0.1);

        let world = settle(&mut m, 70.0, 1);
        assert!(m.defer_fov && (m.external_fov - 70.0).abs() < f32::EPSILON, "a console fov 70 is taken over");
        let world = follow(&mut m, world);
        assert!(!m.defer_fov && (world - 70.0).abs() < 0.1 && (m.current.modifier.fov - 70.0).abs() < f32::EPSILON, "settled on it, it is the current FOV");
        assert!((settle(&mut m, world, 120) - 90.0).abs() < 0.1, "until the state asks for its own again");
    }

    #[test]
    fn the_tween_menu_keeps_its_world_fov_and_garbage_is_ignored() {
        let mut m = presenting(CameraType::TweenMenu, 80.0);
        assert_eq!(m.resolve_fov(95.0, true, None, 0.0, 1.0 / 60.0).0.to_bits(), 95f32.to_bits());
        let mut m = presenting(CameraType::MapMenu, 85.0);
        assert_eq!(m.resolve_fov(1.0e9, false, None, 0.0, 1.0 / 60.0).0.to_bits(), 85f32.to_bits());
    }

    #[test]
    fn free_cameras_own_the_world_fov_and_take_a_pending_reset_and_zoom_steps() {
        let mut m = Manager { pending_fov_reset: 75.0, defer_fov: true, ..presenting(CameraType::Free, 80.0) };
        assert_eq!(m.resolve_fov(200.0, false, Some(3.0), -30.0, 1.0 / 60.0), (75.0, None));
        assert!(!m.defer_fov && m.pending_fov_reset == 0.0);
        assert_eq!(m.resolve_fov(2.0, false, None, 0.0, 1.0 / 60.0).0.to_bits(), 5f32.to_bits(), "clamped to 5");
        m.zoom_fov_offset = -10.0;
        assert_eq!(m.resolve_fov(70.0, false, None, 0.0, 1.0 / 60.0).0.to_bits(), 60f32.to_bits(), "a wheel step lands at once");
        assert_eq!(m.zoom_fov_offset.to_bits(), 0f32.to_bits());
    }

    #[test]
    fn a_reset_is_followed_until_settled() {
        let mut m = presenting(CameraType::FirstPerson, 80.0);
        m.pending_fov_reset = 65.0;
        let world = settle(&mut m, 80.0, 1);
        assert!(m.defer_fov && (m.external_fov - 65.0).abs() < f32::EPSILON);
        assert!((follow(&mut m, world) - 65.0).abs() < 0.1);
    }

    fn target(camera: CameraType, third_zoom: Option<f32>, hands: bool, pitch: f32) -> Option<NearTarget> {
        near_target(&Advanced::default(), camera, third_zoom, hands, false, pitch)
    }

    fn eased(t: Option<NearTarget>) -> (f32, bool) {
        match t {
            Some(NearTarget::Ease { near, pinned, default }) => {
                assert!((default - 15.0).abs() < f32::EPSILON);
                (near, pinned)
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_near_target_follows_ic_ramp_by_pitch() {
        assert_eq!(target(CameraType::MapMenu, None, false, -40.0), None);
        assert_eq!(target(CameraType::ThirdPerson, Some(-0.2), false, -40.0), Some(NearTarget::Default(15.0)));
        assert_eq!(target(CameraType::Free, None, false, -40.0), Some(NearTarget::Default(15.0)));
        assert_eq!(eased(target(CameraType::FirstPerson, None, false, 0.0)), (15.0, true), "above the start angle: the default");
        assert_eq!(eased(target(CameraType::FirstPerson, None, false, -15.0)), (15.0, true));
        let (mid, pinned) = eased(target(CameraType::FirstPerson, None, false, -32.5));
        assert!((mid - 9.5).abs() < 1e-4 && !pinned, "{mid}: halfway from 15 to 4");
        assert_eq!(eased(target(CameraType::FirstPerson, None, false, -80.0)), (4.0, true));
        assert_eq!(eased(target(CameraType::ThirdPerson, Some(-0.275), false, -80.0)), (4.0, true), "the close zoom takes the ramp");
    }

    #[test]
    fn the_near_target_takes_the_combat_set_swapped_pairs_and_the_scene_near() {
        let a = Advanced { fNearDistanceCombatMin: 10.0, fNearDistanceCombatMax: 2.0, fNearDistanceCombatStartAngle: -60.0, fNearDistanceCombatEndAngle: -20.0, ..Advanced::default() };
        let combat = |pitch| eased(near_target(&a, CameraType::FirstPerson, None, true, false, pitch));
        assert_eq!(combat(-20.0), (10.0, true), "start and end swapped: -20 is the start");
        assert_eq!(combat(-60.0), (2.0, true));
        assert!((combat(-40.0).0 - 6.0).abs() < 1e-4);
        assert_eq!(eased(near_target(&a, CameraType::SexTFC, None, false, false, 0.0)), (2.0, true));
        assert_eq!(eased(near_target(&a, CameraType::ThirdPerson, Some(-0.3), false, true, 0.0)), (2.0, true));
    }

    /// Frames of `resolve_near` with the engine holding what was written; the writes made.
    fn run(m: &mut Manager, target: NearTarget, current: &mut f32, frames: usize) -> Vec<f32> {
        let mut writes = Vec::new();
        for _ in 0..frames {
            if let Some(near) = m.resolve_near(target, *current) {
                *current = near;
                writes.push(near);
            }
        }
        writes
    }

    #[test]
    fn the_near_eases_in_whole_steps_and_lands_exactly_on_a_pinned_end() {
        let mut m = Manager::default();
        let mut current = 15.0;
        let writes = run(&mut m, NearTarget::Ease { near: 4.0, pinned: true, default: 15.0 }, &mut current, 120);
        assert!(writes.windows(2).all(|w| w[0] - w[1] >= 1.0 || w[1].to_bits() == 4f32.to_bits()), "{writes:?}");
        assert!(writes.len() < 12, "{writes:?}: far fewer than one a frame");
        assert_eq!(current.to_bits(), 4f32.to_bits());
        assert!(run(&mut m, NearTarget::Ease { near: 4.0, pinned: true, default: 15.0 }, &mut current, 60).is_empty(), "settled: no writes");
        assert_eq!(run(&mut m, NearTarget::Default(15.0), &mut current, 3), vec![15.0], "the default at once, once");
    }

    #[test]
    fn a_near_hovering_on_the_ramp_is_not_rewritten() {
        let mut m = Manager::default();
        let mut current = 15.0;
        run(&mut m, NearTarget::Ease { near: 9.5, pinned: false, default: 15.0 }, &mut current, 120);
        assert!((current - 9.5).abs() < 1.0);
        let mut writes = 0;
        for i in 0..240 {
            let wobble = if i % 2 == 0 { 9.2 } else { 9.8 };
            writes += run(&mut m, NearTarget::Ease { near: wobble, pinned: false, default: 15.0 }, &mut current, 1).len();
        }
        assert_eq!(writes, 0, "a pitch jitter of 0.6 never moves the plane");
    }

    #[test]
    fn the_first_ease_starts_from_the_default() {
        let mut m = Manager::default();
        assert_eq!(m.resolve_near(NearTarget::Ease { near: 4.0, pinned: true, default: 15.0 }, 15.0), Some(lerp(15.0, 4.0, 0.15)));
        let mut nan = Manager { near: f32::NAN, ..Manager::default() };
        assert!(nan.resolve_near(NearTarget::Ease { near: 15.0, pinned: true, default: 15.0 }, 15.0).is_none());
        assert!((nan.near - 15.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_zoom_eases_into_the_offset_within_5_to_165_degrees() {
        let mut m = presenting(CameraType::FirstPerson, 80.0);
        let mut offset = 0.0;
        for _ in 0..200 {
            offset = m.resolve_fov(80.0, false, Some(2.0), -30.0, 1.0 / 60.0).1.unwrap();
        }
        assert!((offset - (2.0 - 30.0)).abs() < 0.01, "{offset}");
        for _ in 0..200 {
            offset = m.resolve_fov(80.0, false, Some(2.0), -100.0, 1.0 / 60.0).1.unwrap();
        }
        assert!(80.0 + offset >= 5.0 - 0.01 && 80.0 + offset < 6.0, "{offset}");
    }
}
