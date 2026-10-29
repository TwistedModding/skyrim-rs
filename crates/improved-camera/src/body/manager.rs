//! `body/BodyManager`: decides the body each frame from the player's race, movement, action and
//! hands, blends the four states' offsets into the first-person body placement and runs the body
//! parts. Its entry is the third-person model update (`hook::misc`), and the HUD's post display
//! while a menu pauses the game (`hook::graphics`), where it freezes the body.

use std::sync::Mutex;

use skyrim::prelude::*;
use skyrim::render::Transform;
use sre::{ActorState_flags1, FormType, NiAVObject, PlayerCharacter, PlayerControls};

use super::parts::Parts;
use super::snapshot;
use super::state::action::{ActionState, ActionType};
use super::state::approach;
use super::state::hands::{EquipmentType, HandsState, HandsType, InteractionType};
use super::state::movement::{MovementState, MovementType};
use super::state::race::{RaceState, RaceType};
use crate::camera::snapshot as camera_snapshot;
use crate::camera::state::{CameraMode, CameraType};
use crate::plugin::{self, ModInfo};
use crate::settings::{self, BodyDirectional, Offset, Profile};
use crate::util::math_util::normalize_radian;
use crate::util::point;
use crate::util::skyrim_util as util;

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

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|v| v * s)
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
    /// The root's pose after the last anchor to the camera (`m_lastAnchoredTranslate`, `Rotate`).
    pub anchored: Option<Forced>,
    /// The yaw smoothing's eased turn toward the movement (`m_bodyYaw`), `None` until it starts.
    pub body_yaw: Option<f32>,
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
            anchored: None,
            body_yaw: None,
        }
    }
}

/// `m_keyReleaseDelay`: how long the movement input may read zero speed before the sway lets go.
const KEY_RELEASE_DELAY: f32 = 0.1;

pub static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

/// Runs `f` on the body manager, created on first use.
pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Manager::default))
}

/// Whether the player is moving: `Actor::IsMoving` is not in the RE yet, so this reads the
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
            root.update_transforms();
            return;
        }
        if util::camera().and_then(PlayerCameraExt::state) == Some(sre::CameraState::FirstPerson) && camera.transition == CameraType::Unknown {
            let offset = m.update(player, &profile, &info, camera.mode);
            let mut local = root.local_transform();
            local.translation = add(local.translation, offset);
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
    [m.race.offset, m.movement.offset, m.action.offset, m.hands.offset, m.directional_offset, global].into_iter().fold([0.0; 3], add)
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
/// turned by the yaw smoothing and moved so the head sits `fAnchorForward` behind and `fAnchorUp`
/// below the presented camera, less the state offsets. IC's first-person free look (its deadzone
/// and head-track twist) needs the head-track target and `ProcessTracking`, not in the RE, and its
/// foot IK and FSMP hand-off are later items; without them the deadzone never engages, as in IC
/// with free look off.
pub fn anchor_camera(dt: f32) {
    if util::game_is_paused() {
        return;
    }
    let camera = camera_snapshot::get();
    let profile = *settings::profile();
    with(|m| {
        if camera.mode != CameraMode::First {
            m.anchored = None;
            m.body_yaw = None;
            return;
        }
        if !profile.General.bEnableBody || camera.transition != CameraType::Unknown {
            m.body_yaw = None;
            return;
        }
        let Some(player) = util::player() else { return };
        let Some(third) = player.third_person_root() else { return };
        let (Some(head), Some(root)) = (util::head_node(player), util::object_by_name(third, "NPC Root [Root]")) else { return };
        let Some(parent) = (unsafe { root.parent.cast::<NiAVObject>().as_ref() }) else { return };
        let body = profile.Body;
        let drawn = !matches!(snapshot::get().hands, HandsType::None | HandsType::Sheath);
        if !util::is_seated(player) && !drawn && body.bYawSmoothing {
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
                && let Some(spine) = util::object_by_name(third, "NPC Spine [Spn0]")
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
            let delta = normalize_radian(look_yaw + offset - body_yaw);
            if delta.abs() > 1e-5 {
                let p = parent.world_transform().rotation;
                let mut local = root.local_transform();
                local.rotation = point::mul(&point::mul(&point::mul(&point::transpose(&p), &point::z_rotation(delta)), &p), &local.rotation);
                root.set_local_transform(local);
                root.update_transforms();
            }
        } else {
            m.body_yaw = None;
        }
        let world = third.world_transform();
        let nudge = anchor_offset(m, &profile);
        let anchor = point::scale([-nudge[0], body.fAnchorForward - nudge[1], body.fAnchorUp - nudge[2]], world.scale);
        let delta = point::sub(camera.position, point::add(head.world_transform().translation, point::mul_vec(&world.rotation, anchor)));
        let parent = parent.world_transform();
        let parent_scale = if parent.scale > 0.0001 { parent.scale } else { 1.0 };
        let mut local = root.local_transform();
        local.translation = point::add(local.translation, point::mul_vec(&point::transpose(&parent.rotation), point::scale(delta, 1.0 / parent_scale)));
        root.set_local_transform(local);
        root.update_transforms();
        m.anchored = Some(Forced { translate: local.translation, rotate: local.rotation });
    });
}

impl Manager {
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
            self.movement = MovementState::enter(kind, &self.movement, &profile.Movement, self.direction);
        }
    }

    fn update_directional_state(&mut self, player: &PlayerCharacter, profile: &Profile) {
        let direction = determine_direction(player);
        if direction != self.direction {
            self.direction = direction;
            self.target_directional_offset = direction.sway(&profile.Directional).map_or([0.0; 3], |o| scale(o.point(), profile.Body.fDirectionalIntensity));
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

    fn update_action_state(&mut self, player: &PlayerCharacter, profile: &Profile, info: &ModInfo) {
        let fishing = plugin::global_value(info.fishing).is_some_and(|v| v > 0.0);
        let kind = if util::graph_bool(player, "SkyParkourOngoing") {
            ActionType::Parkour
        } else if util::graph_bool(player, "GTS_Busy") {
            ActionType::Giantess
        } else if fishing {
            ActionType::Fish
        } else if util::is_menu_open("Dialogue Menu") {
            ActionType::Dialogue
        } else if player.is_on_mount() {
            ActionType::Ride
        } else if plugin::is_disabled(info.boat_navmesh_cutter) == Some(false) {
            ActionType::BoatRide
        } else if util::has_movement_flag(player, ActorState_flags1::SNEAKING) {
            ActionType::Sneak
        } else if util::has_movement_flag(player, ActorState_flags1::SWIMMING) {
            ActionType::Swim
        } else {
            ActionType::None
        };
        if self.action.kind != kind {
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
                _ => EquipmentType::Fist,
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
        let mut kind = if util::weapon_state(player) == 3 { HandsType::Show } else { HandsType::None };
        if sre::is_power_attacking(player) {
            kind = HandsType::PowerAttack;
        }
        if self.force_update > 0 && kind > HandsType::None {
            kind = HandsType::None;
            self.force_update -= 1;
        }
        if self.hands.kind != kind {
            self.hands = HandsState::enter(kind, &self.hands);
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

        let mut result = add(self.race.offset, self.movement.offset());

        let mut action_offset = add(result, self.action.offset());
        if action == ActionType::Sneak {
            match movement {
                MovementType::Run => action_offset = scale(action_offset, 0.8),
                MovementType::Idle => action_offset = scale(action_offset, 0.95),
                _ => {}
            }
        }
        if action == ActionType::Fall || self.land_timer > 0.0 {
            action_offset = scale(action_offset, if determine_direction(player).is_back() { 5.0 } else { 2.1 });
        }
        if matches!(action, ActionType::BoatRide | ActionType::CartRide) {
            action_offset = [0.0; 3];
        }
        result = action_offset;

        let mut hands_offset = add(result, self.hands.offset());
        if hands == HandsType::Show {
            if action == ActionType::Sneak {
                hands_offset = add(hands_offset, scale(self.hands.offset(), -1.0));
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
            result = add(result, scale(self.directional_offset, movement_scale));
        }

        let body = &profile.Body;
        let placement = if hands > HandsType::None { body.Combat.point() } else { [body.fRight, body.fForward, body.fUp] };
        add(result, placement)
    }
}

#[cfg(test)]
mod tests {
    use super::DirectionType;

    #[test]
    fn direction_from_input() {
        assert_eq!(DirectionType::from_input(0.0, 1.0), DirectionType::Forward);
        assert_eq!(DirectionType::from_input(0.5, 0.5), DirectionType::ForwardRight);
        assert_eq!(DirectionType::from_input(-0.5, -0.5), DirectionType::BackLeft);
        assert_eq!(DirectionType::from_input(1.0, 0.0), DirectionType::Right);
        assert_eq!(DirectionType::from_input(0.0, 0.0), DirectionType::None);
    }
}
