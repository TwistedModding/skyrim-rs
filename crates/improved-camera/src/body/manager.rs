//! `body/BodyManager`: decides the body each frame from the player's race, movement, action and
//! hands, blends the four states' offsets into the first-person body placement and runs the body
//! parts. Its entry is the third-person model update (`hook::misc`), and the HUD's post display
//! while a menu pauses the game (`hook::graphics`), where it freezes the body.

use std::sync::Mutex;

use skyrim::BSFixedString;
use skyrim::prelude::*;
use skyrim::render::Transform;
use sre::{ActorAttackState, ActorState_flags1, ActorWeaponState, FormType, NiAVObject, PlayerCharacter, PlayerControls, TESFurniture, TESObjectWEAP};

use super::free_look::{self, FreeLook};
use super::parts::Parts;
use super::snapshot;
use super::state::action::{ActionState, ActionType, FurnitureAnimation, is_gather_model};
use super::state::approach;
use super::state::hands::{EquipmentType, HandsState, HandsType, InteractionType};
use super::state::movement::{MovementState, MovementType};
use super::state::race::{RaceState, RaceType};
use crate::api::fsmp;
use crate::camera::manager::notify_free_look;
use crate::camera::snapshot as camera_snapshot;
use crate::camera::state::{CameraMode, CameraType};
use crate::plugin::{self, ModInfo};
use crate::settings::{self, BodyDirectional, Offset, Profile};
use crate::util::math_util::normalize_radian;
use crate::util::point;
use crate::util::skyrim_util::{self as util, Hand};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionType {
    None = 0,
    Forward,
    ForwardRight,
    Right,
    BackRight,
    Back,
    BackLeft,
    Left,
    ForwardLeft,
}

impl DirectionType {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Forward,
            2 => Self::ForwardRight,
            3 => Self::Right,
            4 => Self::BackRight,
            5 => Self::Back,
            6 => Self::BackLeft,
            7 => Self::Left,
            8 => Self::ForwardLeft,
            _ => Self::None,
        }
    }

    /// `DetermineDirection` from the movement input (x right, y forward).
    #[must_use]
    pub fn from_input(x: f32, y: f32) -> Self {
        match (x.partial_cmp(&0.0), y.partial_cmp(&0.0)) {
            (Some(core::cmp::Ordering::Greater), Some(core::cmp::Ordering::Greater)) => Self::ForwardRight,
            (Some(core::cmp::Ordering::Greater), Some(core::cmp::Ordering::Less)) => Self::BackRight,
            (Some(core::cmp::Ordering::Less), Some(core::cmp::Ordering::Less)) => Self::BackLeft,
            (Some(core::cmp::Ordering::Less), Some(core::cmp::Ordering::Greater)) => Self::ForwardLeft,
            (_, Some(core::cmp::Ordering::Greater)) => Self::Forward,
            (_, Some(core::cmp::Ordering::Less)) => Self::Back,
            (Some(core::cmp::Ordering::Greater), _) => Self::Right,
            (Some(core::cmp::Ordering::Less), _) => Self::Left,
            _ => Self::None,
        }
    }

    const fn is_back(self) -> bool {
        matches!(self, Self::Back | Self::BackLeft | Self::BackRight)
    }

    /// `GetDirectionalSwayOffset`.
    const fn sway(self, d: &BodyDirectional) -> Option<Offset> {
        Some(match self {
            Self::Forward => d.Forward,
            Self::ForwardRight => d.ForwardRight,
            Self::Right => d.Right,
            Self::BackRight => d.BackRight,
            Self::Back => d.Back,
            Self::BackLeft => d.BackLeft,
            Self::Left => d.Left,
            Self::ForwardLeft => d.ForwardLeft,
            Self::None => return None,
        })
    }
}

/// The frozen pose of the body while a menu pauses the game.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Forced {
    pub translate: [f32; 3],
    pub rotate: [[f32; 3]; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Manager {
    pub race: RaceState,
    pub movement: MovementState,
    pub action: ActionState,
    pub hands: HandsState,
    pub parts: Parts,
    pub forced: Option<Forced>,
    pub final_offset: [f32; 3],
    pub left_hand: EquipmentType,
    pub right_hand: EquipmentType,
    pub force_update: u32,
    pub interacting: u32,
    pub interaction: InteractionType,
    pub land_timer: f32,
    pub prev_action: ActionType,
    pub direction: DirectionType,
    pub directional_offset: [f32; 3],
    pub target_directional_offset: [f32; 3],
    pub key_release_timer: f32,
    /// A bash with a bow drawn holds the aim state until the hands lower or the bow goes (`m_bashAimFix`).
    pub bash_aim_fix: bool,
    /// The root's pose after the last anchor to the camera (`m_lastAnchoredTranslate`, `Rotate`).
    pub anchored: Option<Forced>,
    /// The spine's translation after the last anchor, when foot IK held the vertical part of it
    /// there (`m_lastAnchoredSpineTranslate`).
    pub anchored_spine: Option<[f32; 3]>,
    /// Foot IK runs on the third-person graph this frame (`m_footIKActive`).
    pub foot_ik_active: bool,
    /// The yaw smoothing's eased turn toward the movement (`m_bodyYaw`), `None` until it starts.
    pub body_yaw: Option<f32>,
    /// The body's turn off the root shown last frame, held to `POSE_TURN_SPEED` while standing.
    pub pose_turn: Option<f32>,
    /// `m_humanoidRace`.
    pub humanoid: bool,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            race: RaceState::default(),
            movement: MovementState::default(),
            action: ActionState::default(),
            hands: HandsState::default(),
            parts: Parts::default(),
            forced: None,
            final_offset: [0.0; 3],
            left_hand: EquipmentType::Fist,
            right_hand: EquipmentType::Fist,
            force_update: 0,
            interacting: 0,
            interaction: InteractionType::None,
            land_timer: 0.0,
            prev_action: ActionType::None,
            direction: DirectionType::None,
            directional_offset: [0.0; 3],
            target_directional_offset: [0.0; 3],
            key_release_timer: 0.0,
            bash_aim_fix: false,
            anchored: None,
            anchored_spine: None,
            foot_ik_active: false,
            body_yaw: None,
            pose_turn: None,
            humanoid: false,
        }
    }
}

const SPINE: &str = "NPC Spine [Spn0]";
const COM: &str = "NPC COM [COM ]";
/// How fast the body may turn off the root, in degrees a second, standing and moving: the stop
/// animations after a strafe swing the whole body up to 90 degrees in one frame and back within half
/// a second, while locomotion turns it under 140 a second.
const POSE_TURN_SPEED: f32 = 45.0;
const POSE_MOVING_TURN_SPEED: f32 = 180.0;
/// How far the spine takes the anchor's vertical correction while foot IK is live.
const SPINE_DELTA_LIMIT: f32 = 20.0;

/// `m_keyReleaseDelay`: how long the movement input may read zero speed before the sway lets go.
const KEY_RELEASE_DELAY: f32 = 0.1;

pub static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

/// Runs `f` on the body manager, created on first use.
pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Manager::default))
}

/// `Update_ThirdPersonArms`: the body shows its own arms in first person, the first-person arms
/// being turned off.
pub fn third_person_arms(general: &settings::General) -> bool {
    general.bEnableBody && !general.bEnableFirstPersonArms
}

/// `Update`'s fix for weapons drawn in first person while swimming: IC asks for a sheathe while
/// the body swims with its hands up, unless the Attack Behavior Framework allows weapons in water.
/// `drawn` is the live weapon state above sheathed, so a sheathe asked once is not asked again.
#[must_use]
pub fn swim_sheathe(body: &snapshot::Data, swimming: bool, drawn: bool, bfco: bool) -> bool {
    body.action == ActionType::Swim && body.hands > HandsType::None && swimming && drawn && !bfco
}

/// Runs the swim fix on the main thread, through the engine's own immediate sheathe
/// (`ResetActorWeaponStateAndCamera`, RE `camera.md` IC6 B): the draw is refused in water, so
/// the weapon stays away until the player leaves it.
pub fn sheathe_while_swimming() {
    let Some(player) = util::player() else { return };
    let swimming = util::has_movement_flag(player, ActorState_flags1::SWIMMING);
    if swim_sheathe(&snapshot::get(), swimming, util::weapon_state(player) != ActorWeaponState::Sheathed as u32, plugin::info().bfco.is_some()) {
        player.reset_actor_weapon_state_and_camera();
    }
}

/// Whether the player is moving:`Actor::IsMoving` is not in the RE yet, so this reads the
/// current speed the movement controller keeps (`Actor::GetCurrentSpeed`).
fn is_moving(player: &PlayerCharacter) -> bool {
    player.get_current_speed() > 0.0
}

fn move_input() -> (f32, f32) {
    PlayerControls::singleton().map_or((0.0, 0.0), |c| (c.moveInput.x, c.moveInput.y))
}

fn determine_direction(player: &PlayerCharacter) -> DirectionType {
    if !is_moving(player) {
        return DirectionType::None;
    }
    let (x, y) = move_input();
    DirectionType::from_input(x, y)
}

/// `SyncBoneTree`: skinning reads the flat bone entries, which a scale written on a node reaches only
/// through the tree's own update; re-chain them from the node worlds, which are only read, so the
/// bones a physics mod drives keep its writes.
pub fn sync_bone_tree(npc_root: &NiAVObject) {
    if let Some(tree) = npc_root.cast::<sre::BSFlattenedBoneTree>() {
        tree.rechain_bones();
    }
}

/// `UpdateAnimation_ThirdPerson`: before the third-person model update. `forced` freezes the body
/// where it stood while a menu pauses the game.
pub fn update_animation_third_person(third: &NiAVObject, forced: bool) {
    let Some(root) = util::object_by_name(third, "NPC Root [Root]").filter(|r| !r.parent.is_null()) else { return };
    let Some(player) = util::player() else { return };
    let profile = *settings::profile();
    let info = plugin::info();
    let camera = camera_snapshot::get();
    with(|m| {
        if forced {
            m.update(player, &profile, &info, camera.mode);
            let local = root.local_transform();
            let pose = *m.forced.get_or_insert(m.anchored.unwrap_or(Forced { translate: local.translation, rotate: local.rotation }));
            root.set_local_transform(Transform { rotation: pose.rotate, translation: pose.translate, ..local });
            if let Some(translation) = m.anchored_spine
                && let Some(spine) = util::object_by_name(third, SPINE)
            {
                spine.set_local_transform(Transform { translation, ..spine.local_transform() });
            }
            root.update_transforms();
            return;
        }
        if util::camera().and_then(PlayerCameraExt::state) == Some(sre::CameraState::FirstPerson) && camera.transition == CameraType::Unknown {
            let offset = m.update(player, &profile, &info, camera.mode);
            let mut local = root.local_transform();
            local.translation = point::add(local.translation, offset);
            // Last frame's anchored root held across the graph's re-pose, until the camera
            // update anchors it again.
            if let (CameraMode::First, Some(anchored)) = (camera.mode, m.anchored) {
                local.translation = anchored.translate;
                local.rotation = anchored.rotate;
            }
            root.set_local_transform(local);
            m.forced = None;
        } else {
            m.update(player, &profile, &info, camera.mode);
        }
    });
}

/// `GetAnchorOffset`: the four states' offsets, the directional sway and the global body offset
/// (its combat one with the hands drawn).
fn anchor_offset(m: &Manager, profile: &Profile) -> [f32; 3] {
    let global = if snapshot::get().hands > HandsType::None { profile.Body.Combat.point() } else { [profile.Body.fRight, profile.Body.fForward, profile.Body.fUp] };
    [m.race.offset, m.movement.offset, m.action.offset, m.hands.offset, m.directional_offset, global].into_iter().fold([0.0; 3], point::add)
}

/// `SmoothMovementOffset`: the yaw smoothing's turn toward the movement, capped at
/// `fYawSmoothingMaxAngle` and eased at `fYawSmoothingSpeed` the short way round.
#[must_use]
pub fn smooth_movement_offset(previous: Option<f32>, target: f32, speed: f32, max_angle: f32, dt: f32) -> f32 {
    let max = max_angle.abs().to_radians();
    let target = normalize_radian(target).clamp(-max, max);
    previous.map_or(target, |yaw| normalize_radian(yaw + normalize_radian(target - yaw) * (1.0 - (-speed.max(0.0) * dt).exp())))
}

/// `BODY::Manager::AnchorCamera`, after the camera update: in first person the body's root is
/// turned by the yaw smoothing or the free look's deadzone and moved so the head sits
/// `fAnchorForward` behind and `fAnchorUp` below the presented camera, less the state offsets.
/// FSMP is told the body is camera-driven only once it is anchored. The bumper is updated here
/// because the controller shape swap must run on the main thread.
pub fn anchor_camera(dt: f32) {
    let camera = camera_snapshot::get();
    let profile = *settings::profile();
    if let Some(player) = util::player() {
        let head = matches!(camera.mode, CameraMode::Head | CameraMode::Third);
        super::bumper::update(player, head || !util::is_engine_first_person(), &profile.Advanced);
    }
    if util::game_is_paused() {
        return;
    }
    let mut events = [None; 2];
    let (release, driven) = with(|m| {
        m.foot_ik_active = update_foot_ik(&profile);
        anchor(m, &camera, &profile, dt, &mut events)
    });
    fsmp::set_player_body_camera_driven(driven);
    for event in events {
        free_look::send_turn_event(event);
    }
    if release {
        free_look::release();
    }
}

/// `AnchorCamera` under the body manager's lock; the turn events to send after it, and whether the
/// free look's twist lets go and whether the body is anchored.
fn anchor(m: &mut Manager, camera: &camera_snapshot::Data, profile: &Profile, dt: f32, events: &mut [Option<&'static str>; 2]) -> (bool, bool) {
    let snap = |events: &mut [Option<&'static str>; 2]| events[1] = free_look::with(FreeLook::snap_body_yaw);
    if camera.mode != CameraMode::First {
        m.anchored = None;
        m.anchored_spine = None;
        m.reset_yaw_smoothing();
        snap(events);
        return (true, false);
    }
    if !profile.General.bEnableBody || camera.transition != CameraType::Unknown {
        m.reset_yaw_smoothing();
        snap(events);
        return (true, false);
    }
    let nodes = util::player().and_then(|player| {
        let third = player.third_person_root()?;
        let head = util::actor_head(player)?;
        let root = util::object_by_name(third, "NPC Root [Root]")?;
        let parent = unsafe { root.parent.cast::<NiAVObject>().as_ref() }?;
        Some((player, third, head, root, parent))
    });
    let Some((player, third, head, root, parent)) = nodes else {
        m.reset_yaw_smoothing();
        snap(events);
        return (false, false);
    };
    let body = profile.Body;
    let state = snapshot::get();
    let look = free_look::with(|f| *f);
    let free = body.bFreeLook && look.driving && !free_look::decide(&state).align;
    let furniture = util::is_interacting(player);
    let drawn = !matches!(state.hands, HandsType::None | HandsType::Sheath);
    // Drawing while the deadzone holds the body turned eases it back to the camera first.
    let unwinding = drawn && !furniture && body.bFreeLook && look.decoupled;
    if !furniture && (!drawn || unwinding) && (body.bYawSmoothing || free || unwinding) {
        let right = point::mul_vec(&camera.rotation, [1.0, 0.0, 0.0]);
        let look_yaw = normalize_radian(right[0].atan2(right[1]) - core::f32::consts::FRAC_PI_2);
        let (x, y) = move_input();
        let magnitude = x.hypot(y);
        let move_angle = if magnitude > 0.1 { (if y >= 0.0 { x } else { -x }).atan2(y.abs()) } else { 0.0 };
        let mut offset = smooth_movement_offset(m.body_yaw, move_angle, body.fYawSmoothingSpeed, body.fYawSmoothingMaxAngle, dt);
        m.body_yaw = Some(offset);
        let forward = point::mul_vec(&root.world_transform().rotation, [0.0, 1.0, 0.0]);
        let body_yaw = forward[0].atan2(forward[1]);
        if magnitude > 0.1
            && let Some(spine) = util::object_by_name(third, SPINE)
        {
            let spine_forward = point::mul_vec(&spine.world_transform().rotation, [0.0, 1.0, 0.0]);
            let turn = normalize_radian(spine_forward[0].atan2(spine_forward[1]) - body_yaw);
            let max = body.fYawSmoothingMaxAngle.abs().to_radians();
            let total = normalize_radian(offset + turn);
            if total > max {
                offset -= total - max;
            } else if total < -max {
                offset -= total + max;
            }
        }
        let counter = m.pose_counter(third, body_yaw, magnitude <= 0.1 && !look.turning(), dt);
        let desired = normalize_radian(look_yaw + offset);
        let target = if unwinding || free {
            let (target, event) = free_look::with(|f| f.update_body_yaw(desired, unwinding || magnitude > 0.1, &body, dt));
            events[0] = event;
            if unwinding && normalize_radian(desired - target).abs() < 1f32.to_radians() {
                snap(events);
            }
            target
        } else {
            snap(events);
            desired
        };
        let delta = normalize_radian(target + counter - body_yaw);
        if delta.abs() > 1e-5 {
            let mut local = root.local_transform();
            local.rotation = turned(&parent.world_transform().rotation, &local.rotation, delta);
            root.set_local_transform(local);
            root.update_transforms();
        }
    } else {
        m.reset_yaw_smoothing();
        snap(events);
    }
    let world = third.world_transform();
    let nudge = anchor_offset(m, profile);
    let anchor = point::scale([-nudge[0], body.fAnchorForward - nudge[1], body.fAnchorUp - nudge[2]], world.scale);
    let mut delta = point::sub(camera.position, point::add(head.world_transform().translation, point::mul_vec(&world.rotation, anchor)));
    // With foot IK live the engine owns the legs' ground contact: the vertical part of the anchor
    // rides in the spine instead of pushing the planted feet through the ground, up to a limit
    // past which (transitions) it spills back into the root.
    let spine = m.foot_ik_active.then(|| util::object_by_name(third, SPINE)).flatten().filter(|s| !s.parent.is_null());
    let spine_delta = spine.map_or(0.0, |_| delta[2].clamp(-SPINE_DELTA_LIMIT, SPINE_DELTA_LIMIT));
    delta[2] -= spine_delta;
    let mut local = root.local_transform();
    local.translation = point::add(local.translation, to_parent_local(parent, delta));
    root.set_local_transform(local);
    if let Some(spine) = spine
        && let Some(spine_parent) = unsafe { spine.parent.cast::<NiAVObject>().as_ref() }
    {
        let mut spine_local = spine.local_transform();
        spine_local.translation = point::add(spine_local.translation, to_parent_local(spine_parent, [0.0, 0.0, spine_delta]));
        spine.set_local_transform(spine_local);
    }
    root.update_transforms();
    m.anchored = Some(Forced { translate: local.translation, rotate: local.rotation });
    m.anchored_spine = spine.map(|s| s.local_transform().translation);
    (false, true)
}

/// The local rotation under `parent` whose world heading (`atan2(x, y)` of the forward axis) is
/// `local`'s turned by `yaw` about the world's up axis.
fn turned(parent: &point::M3, local: &point::M3, yaw: f32) -> point::M3 {
    point::mul(&point::mul(&point::mul(&point::transpose(parent), &point::z_rotation(-yaw)), parent), local)
}

/// A world-space move as a translation in `parent`'s local space.
fn to_parent_local(parent: &NiAVObject, delta: [f32; 3]) -> [f32; 3] {
    let parent = parent.world_transform();
    let parent_scale = if parent.scale > 0.0001 { parent.scale } else { 1.0 };
    point::mul_vec(&point::transpose(&parent.rotation), point::scale(delta, 1.0 / parent_scale))
}

/// `UpdateFootIK`: in the engine's first person the POV switch leaves the third-person graph's
/// foot IK driver disabled and the engine drives only the active graph's, so the anchored body
/// would never plant its feet. Drive it from the behaviour graph's own `bHumanoidFootIKDisable`,
/// which riding and swimming still raise. Whether foot IK is live.
///
/// The engine disables the driver again on every POV switch and graph re-attach, so the driver's
/// state is compared every frame and written only when it differs: enabling is a byte store,
/// disabling frees the legs' state and goes through the manager's lock.
fn update_foot_ik(profile: &Profile) -> bool {
    if !profile.General.bEnableBody || !util::is_engine_first_person() {
        return false;
    }
    let Some(manager) = util::player().and_then(util::player_graph_manager) else { return false };
    let graphs = manager.graphs();
    if graphs.len() < 2 {
        return false;
    }
    let Some(graph) = graphs[0].filter(|g| g.doFootIK != 0) else { return false };
    let Some(driver_disabled) = graph.foot_ik_disabled() else { return false };
    let mut disabled = false;
    unsafe { graph.get_graph_variable_bool(BSFixedString::new("bHumanoidFootIKDisable").raw(), &raw mut disabled) };
    if disabled != driver_disabled {
        manager.set_foot_ik_disabled(disabled);
    }
    !disabled
}

/// Parks the third-person graph's foot IK driver disabled again, as the engine leaves it in first
/// person, when this build had turned it on.
pub fn release_foot_ik() {
    if !with(|m| core::mem::take(&mut m.foot_ik_active)) {
        return;
    }
    if let Some(manager) = util::player().and_then(util::player_graph_manager)
        && manager.graphs().first().copied().flatten().and_then(BShkbAnimationGraphExt::foot_ik_disabled) == Some(false)
    {
        manager.set_foot_ik_disabled(true);
    }
}

/// The body's turn off the root shown this frame: last frame's moved toward the animation's `turn`
/// by at most `step`.
#[must_use]
pub fn limited_turn(shown: f32, turn: f32, step: f32) -> f32 {
    normalize_radian(shown + normalize_radian(turn - shown).clamp(-step, step))
}

impl Manager {
    /// `ResetYawSmoothing`.
    fn reset_yaw_smoothing(&mut self) {
        self.body_yaw = None;
        self.pose_turn = None;
    }

    /// The root turn that keeps the animated body from turning off the root faster than
    /// `POSE_TURN_SPEED` while `standing`; the root's heading is `root_yaw`.
    fn pose_counter(&mut self, third: &NiAVObject, root_yaw: f32, standing: bool, dt: f32) -> f32 {
        let Some(com) = util::object_by_name(third, COM) else { return 0.0 };
        let forward = point::mul_vec(&com.world_transform().rotation, [0.0, 1.0, 0.0]);
        let turn = normalize_radian(forward[0].atan2(forward[1]) - root_yaw);
        let speed = if standing { POSE_TURN_SPEED } else { POSE_MOVING_TURN_SPEED };
        let shown = self.pose_turn.map_or(turn, |shown| limited_turn(shown, turn, speed.to_radians() * dt));
        self.pose_turn = Some(shown);
        normalize_radian(shown - turn)
    }

    /// `Update`: the states, and the offset the body takes; none while the camera rides the head.
    fn update(&mut self, player: &PlayerCharacter, profile: &Profile, info: &ModInfo, mode: CameraMode) -> [f32; 3] {
        self.update_states(player, profile, info);
        let head = matches!(mode, CameraMode::Head | CameraMode::Third);
        if profile.General.bEnableBody && !head { self.final_offset } else { [0.0; 3] }
    }

    fn update_states(&mut self, player: &PlayerCharacter, profile: &Profile, info: &ModInfo) {
        if !util::game_is_paused() {
            self.update_race_state(player, profile, info);
            self.update_movement_state(player, profile);
            self.update_directional_state(player, profile);
            self.update_action_state(player, profile, info);
            self.update_hands_state(player, info);
            let dt = util::delta_world_time();
            self.movement.update(&profile.Movement, self.direction, dt);
            self.action.update(dt);
            self.hands.update(&profile.Hands);
            self.final_offset = self.blend_states(player, profile, dt);
            snapshot::set(self.snapshot());
        }
        self.parts.update(player, profile);
    }

    #[must_use]
    pub const fn snapshot(&self) -> snapshot::Data {
        snapshot::Data {
            race: self.race.race,
            movement: self.movement.kind,
            direction: self.direction,
            action: self.action.kind,
            hands: self.hands.kind,
            left_hand: self.left_hand,
            right_hand: self.right_hand,
            interaction: self.interaction,
            humanoid: self.humanoid,
        }
    }

    fn update_race_state(&mut self, player: &PlayerCharacter, profile: &Profile, info: &ModInfo) {
        let Some(race) = player.race() else { return };
        let id = race.formID;
        let tagged = [
            (info.lich_race, RaceType::Lich),
            (info.half_dragon_race, RaceType::HalfDragon),
            (info.half_dragon_vampire_race, RaceType::HalfDragonVampire),
            (info.hornedlord_race, RaceType::Hornedlord),
            (info.deer_race, RaceType::Deer),
        ];
        let kind = tagged.into_iter().rev().find(|(form, _)| *form == Some(id)).map_or_else(|| RaceType::from_u32(id), |(_, tag)| tag);
        self.race = RaceState { race: kind, offset: kind.offset(&profile.Race).map_or([0.0; 3], Offset::point) };
        self.humanoid = util::is_humanoid(player);
    }

    fn update_movement_state(&mut self, player: &PlayerCharacter, profile: &Profile) {
        let kind = if !is_moving(player) {
            MovementType::Idle
        } else if util::has_movement_flag(player, ActorState_flags1::SPRINTING) {
            MovementType::Sprint
        } else if player.is_running() {
            MovementType::Run
        } else {
            MovementType::Walk
        };
        if self.movement.kind != kind {
            if kind == MovementType::Idle {
                notify_free_look();
            }
            self.movement = MovementState::enter(kind, &self.movement, &profile.Movement, self.direction);
        }
        if kind > MovementType::Idle {
            notify_free_look();
        }
    }

    fn update_directional_state(&mut self, player: &PlayerCharacter, profile: &Profile) {
        let direction = determine_direction(player);
        if direction != self.direction {
            self.direction = direction;
            self.target_directional_offset = direction.sway(&profile.Directional).map_or([0.0; 3], |o| point::scale(o.point(), profile.Body.fDirectionalIntensity));
            self.key_release_timer = 0.0;
        }
        let speed = player.as_i_animation_graph_manager_holder().variable_float(&skyrim::BSFixedString::new("Speed")).unwrap_or(0.0);
        #[allow(clippy::float_cmp)]
        if self.direction != DirectionType::None && speed == 0.0 {
            self.key_release_timer += util::delta_world_time();
            if self.key_release_timer >= KEY_RELEASE_DELAY {
                self.direction = DirectionType::None;
                self.target_directional_offset = [0.0; 3];
            }
        } else {
            self.key_release_timer = 0.0;
        }
    }

    /// `UpdateActionState` in IC's order. The `SexLab` faction test and IC's `OStim` scene test are
    /// absent, and with them the sex action; so is the furniture's head tracking switch.
    fn update_action_state(&mut self, player: &PlayerCharacter, profile: &Profile, info: &ModInfo) {
        let effect = |id| util::has_magic_effect(player, id);
        let furniture = player.occupied_furniture().map(|reference| {
            let base = unsafe { reference.baseObject.as_ref() }.and_then(|form| Some((form.cast::<TESFurniture>()?, util::model_path(form))));
            base.map_or(FurnitureAnimation::Unknown, |(f, model)| FurnitureAnimation::of(f.workbenchType, model.is_some_and(is_gather_model), f.activeMarkerFlags))
        });
        let fishing = plugin::global_value(info.fishing).is_some_and(|v| v > 0.0);
        let cart_ride = plugin::quest_stage(info.quest_intro).is_some_and(|stage| stage > 0 && stage < 145);
        let kind = if effect(info.lich_transition) {
            ActionType::LichTransition
        } else if effect(info.werewolf_transition) {
            ActionType::WerewolfTransition
        } else if effect(info.vampirelord_transition) {
            ActionType::VampirelordTransition
        } else if util::is_staggered(player) {
            ActionType::Stagger
        } else if effect(info.bathe) {
            ActionType::Bathe
        } else if util::graph_bool(player, "SkyParkourOngoing") {
            ActionType::Parkour
        } else if util::graph_bool(player, "GTS_Busy") {
            ActionType::Giantess
        } else if furniture == Some(FurnitureAnimation::Unknown) {
            ActionType::Animation
        } else if furniture == Some(FurnitureAnimation::Sleeping) {
            ActionType::Sleep
        } else if furniture == Some(FurnitureAnimation::Gathering) {
            ActionType::Gather
        } else if furniture == Some(FurnitureAnimation::Crafting) {
            ActionType::Craft
        } else if fishing {
            ActionType::Fish
        } else if util::is_menu_open("Dialogue Menu") {
            ActionType::Dialogue
        } else if furniture == Some(FurnitureAnimation::Sitting) {
            ActionType::Sit
        } else if player.is_on_mount() {
            ActionType::Ride
        } else if plugin::is_disabled(info.boat_navmesh_cutter) == Some(false) {
            ActionType::BoatRide
        } else if cart_ride {
            ActionType::CartRide
        } else if player.is_animation_driven() {
            ActionType::AI
        } else if !util::movement_controls_enabled() {
            ActionType::View
        } else if player.is_in_mid_air() {
            ActionType::Fall
        } else if util::has_movement_flag(player, ActorState_flags1::SNEAKING) {
            ActionType::Sneak
        } else if util::has_movement_flag(player, ActorState_flags1::SWIMMING) {
            ActionType::Swim
        } else {
            ActionType::None
        };
        if matches!(kind, ActionType::Animation | ActionType::Sleep | ActionType::Dialogue | ActionType::Sit) {
            notify_free_look();
        }
        if self.action.kind != kind {
            if kind == ActionType::AI {
                util::set_pitch(player, 0.0);
            }
            self.action = ActionState::enter(kind, &self.action, &profile.Action);
        }
    }

    /// `GetEquipped`: what each hand holds; a change holds the hands state at none for a few frames.
    fn get_equipped(&mut self, player: &PlayerCharacter) -> u32 {
        let kind_of = |left: bool| {
            player.get_equipped_object(left).map_or(EquipmentType::Fist, |form| match form.get_form_type() {
                Ok(FormType::ARMO) if left => EquipmentType::Shield,
                Ok(FormType::LIGH) if left => EquipmentType::Torch,
                Ok(FormType::SPEL) => EquipmentType::Spell,
                _ => form.cast::<TESObjectWEAP>().map_or(EquipmentType::Fist, |weapon| EquipmentType::from_u32(u32::from(weapon.weaponType))),
            })
        };
        let mut update = 0;
        let left = kind_of(true);
        if self.left_hand != left {
            self.left_hand = left;
            update = if matches!(left, EquipmentType::Shield | EquipmentType::Axe | EquipmentType::Greataxe) { 20 } else { 2 };
        }
        let right = kind_of(false);
        if self.right_hand != right {
            self.right_hand = right;
            if matches!(right, EquipmentType::Axe | EquipmentType::Greataxe) {
                update = 20;
            } else if update == 0 {
                update = 2;
            }
        }
        update
    }

    /// `GetInteracting`.
    fn get_interacting(&mut self, player: &PlayerCharacter, info: &ModInfo) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mut interacting = plugin::global_value(info.interaction).map_or(0, |v| v as u32);
        if interacting == 0 && util::graph_bool(player, "bOffsetGPMA") {
            interacting = 1;
        }
        if self.interacting != interacting {
            self.interacting = interacting;
            self.interaction = InteractionType::from_trigger(interacting);
        }
    }

    fn update_hands_state(&mut self, player: &PlayerCharacter, info: &ModInfo) {
        let force_update = self.get_equipped(player);
        if force_update != 0 && self.force_update == 0 {
            self.force_update = force_update;
        }
        self.get_interacting(player, info);
        let mut kind = match ActorWeaponState::from_u32(util::weapon_state(player)) {
            Some(ActorWeaponState::Drawing | ActorWeaponState::Drawn) => HandsType::Show,
            Some(ActorWeaponState::WantToSheathe | ActorWeaponState::Sheathing) => HandsType::Sheath,
            _ => HandsType::None,
        };
        if player.is_blocking() {
            kind = HandsType::Block;
        }
        if util::is_casting(player, Hand::Left) || util::is_casting(player, Hand::Right) {
            kind = HandsType::Cast;
        }
        if player.is_attacking() {
            kind = HandsType::Attack;
        }
        if sre::is_power_attacking(player) {
            kind = HandsType::PowerAttack;
        }
        if (kind == HandsType::None || self.right_hand != EquipmentType::Bow) && self.bash_aim_fix {
            self.bash_aim_fix = false;
        }
        if self.bash_aim_fix {
            kind = HandsType::Aim;
        }
        if matches!(self.right_hand, EquipmentType::Bow | EquipmentType::Crossbow) {
            use ActorAttackState as A;
            match util::attack_state(player) {
                Some(A::BowDraw | A::BowAttached | A::BowDrawn | A::BowReleasing) => kind = HandsType::Aim,
                Some(A::BowReleased | A::BowNextAttack) if self.right_hand == EquipmentType::Crossbow => kind = HandsType::Aim,
                Some(A::Bash) if self.right_hand == EquipmentType::Bow => {
                    self.bash_aim_fix = true;
                    kind = HandsType::Aim;
                }
                _ => {}
            }
        }
        if self.force_update > 0 && kind > HandsType::None {
            kind = HandsType::None;
            self.force_update -= 1;
        }
        if self.hands.kind != kind {
            self.hands = HandsState::enter(kind, &self.hands);
        }
        if kind > HandsType::Sheath {
            notify_free_look();
        }
    }

    /// `BlendStates`: race, movement, action and hands offsets with IC's per-state scaling, the
    /// directional sway and the global (or combat) placement.
    fn blend_states(&mut self, player: &PlayerCharacter, profile: &Profile, dt: f32) -> [f32; 3] {
        let action = self.action.kind;
        let movement = self.movement.kind;
        let hands = self.hands.kind;
        if self.prev_action == ActionType::Fall && action != ActionType::Fall {
            self.land_timer = if determine_direction(player).is_back() { 0.4 } else { 0.2 };
        }
        self.land_timer = (self.land_timer - dt).max(0.0);
        self.prev_action = action;

        let mut result = point::add(self.race.offset, self.movement.offset());

        let mut action_offset = point::add(result, self.action.offset());
        if action == ActionType::Sneak {
            match movement {
                MovementType::Run => action_offset = point::scale(action_offset, 0.8),
                MovementType::Idle => action_offset = point::scale(action_offset, 0.95),
                _ => {}
            }
        }
        if action == ActionType::Fall || self.land_timer > 0.0 {
            action_offset = point::scale(action_offset, if determine_direction(player).is_back() { 5.0 } else { 2.1 });
        }
        if matches!(action, ActionType::BoatRide | ActionType::CartRide) {
            action_offset = [0.0; 3];
        }
        result = action_offset;

        let mut hands_offset = point::add(result, self.hands.offset());
        if hands == HandsType::Show {
            if action == ActionType::Sneak {
                hands_offset = point::add(hands_offset, point::scale(self.hands.offset(), -1.0));
            } else if movement == MovementType::Run {
                hands_offset[1] += 13.0;
            }
        }
        result = hands_offset;

        self.directional_offset = approach(self.directional_offset, self.target_directional_offset, profile.Body.fDirectionalSpeed, dt);
        let mut movement_scale = match movement {
            MovementType::Walk | MovementType::Sprint => 1.5,
            MovementType::Run => 1.2,
            MovementType::Idle => 0.1,
            MovementType::None => 1.0,
        };
        if action == ActionType::Sneak || movement == MovementType::Walk {
            movement_scale *= if determine_direction(player).is_back() {
                if movement == MovementType::Run { -6.0 } else { -2.0 }
            } else {
                0.4
            };
        }
        if !matches!(action, ActionType::BoatRide | ActionType::CartRide | ActionType::Swim | ActionType::Fall) {
            result = point::add(result, point::scale(self.directional_offset, movement_scale));
        }

        let body = &profile.Body;
        let placement = if hands > HandsType::None { body.Combat.point() } else { [body.fRight, body.fForward, body.fUp] };
        point::add(result, placement)
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionType, DirectionType, HandsType, limited_turn, point, snapshot, swim_sheathe, turned};

    #[test]
    fn a_stop_animation_swing_turns_the_shown_body_one_step_a_frame() {
        let step = 0.75f32.to_radians();
        let shown = limited_turn(3f32.to_radians(), -88f32.to_radians(), step);
        assert!((shown - 2.25f32.to_radians()).abs() < 1e-6, "{shown}");
        let settle = limited_turn(11f32.to_radians(), 11.5f32.to_radians(), step);
        assert!((settle - 11.5f32.to_radians()).abs() < 1e-6, "{settle}");
        let across = limited_turn(179.5f32.to_radians(), -179.5f32.to_radians(), step);
        assert!((across + 179.75f32.to_radians()).abs() < 1e-5, "{across}");
    }

    #[test]
    fn the_turn_moves_the_world_heading_toward_the_target() {
        let heading = |m: &point::M3| {
            let f = point::mul_vec(m, [0.0, 1.0, 0.0]);
            f[0].atan2(f[1])
        };
        let (s, c) = 0.3f32.sin_cos();
        let parent = point::mul(&point::z_rotation(0.4), &[[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]);
        let local = point::z_rotation(-0.3);
        let before = heading(&point::mul(&parent, &local));
        let after = heading(&point::mul(&parent, &turned(&parent, &local, 0.25)));
        assert!((after - before - 0.25).abs() < 1e-5, "{before} -> {after}");
    }

    #[test]
    fn sheathes_once_while_swimming_with_the_hands_up() {
        let swim = snapshot::Data { action: ActionType::Swim, hands: HandsType::Show, ..snapshot::Data::NONE };
        assert!(swim_sheathe(&swim, true, true, false));
        assert!(!swim_sheathe(&swim, true, false, false), "already sheathed");
        assert!(!swim_sheathe(&swim, true, true, true), "the Attack Behavior Framework allows weapons in water");
        assert!(!swim_sheathe(&swim, false, true, false), "out of the water since the body's last update");
        assert!(!swim_sheathe(&snapshot::Data { hands: HandsType::None, ..swim }, true, true, false));
        assert!(!swim_sheathe(&snapshot::Data { action: ActionType::None, ..swim }, true, true, false));
    }

    #[test]
    fn direction_from_input() {
        assert_eq!(DirectionType::from_input(0.0, 1.0), DirectionType::Forward);
        assert_eq!(DirectionType::from_input(0.5, 0.5), DirectionType::ForwardRight);
        assert_eq!(DirectionType::from_input(-0.5, -0.5), DirectionType::BackLeft);
        assert_eq!(DirectionType::from_input(1.0, 0.0), DirectionType::Right);
        assert_eq!(DirectionType::from_input(0.0, 0.0), DirectionType::None);
    }
}
