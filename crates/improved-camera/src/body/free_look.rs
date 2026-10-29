//! `body/BodyFreeLook`: first-person free look. Inside a deadzone the visible body holds its
//! ground while the engine's head tracking turns the head (and with `bFreeLookSpine` the upper
//! spine) toward where the camera looks; past it the body shuffles round to catch up. Driven once
//! a frame from the camera update; the engine runs the player's head tracking only in the
//! third-person camera states, so the drive calls it itself (RE `camera.md` IC5 C).

use std::sync::Mutex;

use skyrim::BSFixedString;
use skyrim::prelude::*;
use sre::{HeadTrackSlot, NiPoint3, PlayerCharacter};

use super::snapshot::Data as Body;
use super::state::action::ActionType;
use super::state::hands::{HandsType, InteractionType};
use super::state::movement::MovementType;
use super::state::race::RaceType;
use crate::camera::snapshot::Data as Camera;
use crate::camera::state::{CameraMode, CameraType};
use crate::settings::{Body as BodySettings, Profile};
use crate::util::math_util::normalize_radian;
use crate::util::point;
use crate::util::skyrim_util as util;

/// The engine's `LookAtOnGain`; the twist strength scales it.
const LOOK_AT_GAIN: f32 = 0.05;
/// How far down the look axis the head-track target sits.
const LOOK_AT_DISTANCE: f32 = 500.0;
/// `ActorState::flags2` bit 3, the actor's head-tracking switch.
const HEAD_TRACKING: u32 = 0x8;

/// `Decide`: what the body's state allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// The engine's head and spine twist may run.
    pub twist: bool,
    /// The body holds the camera's heading (no deadzone): aiming, attacking, blocking, casting,
    /// sprinting.
    pub align: bool,
}

/// `Decide`.
#[must_use]
pub fn decide(body: &Body) -> Decision {
    let humanoid = body.humanoid && !matches!(body.race, RaceType::Werewolf | RaceType::Vampirelord | RaceType::Werebear);
    let locomotion = matches!(body.action, ActionType::None | ActionType::Sneak | ActionType::Fall);
    let align = matches!(body.hands, HandsType::Attack | HandsType::PowerAttack | HandsType::Block | HandsType::Aim | HandsType::Cast) || body.movement == MovementType::Sprint;
    let suppressed = matches!(body.hands, HandsType::Attack | HandsType::PowerAttack | HandsType::Block | HandsType::Aim);
    Decision { twist: humanoid && locomotion && !suppressed && body.interaction == InteractionType::None, align }
}

/// `MATH::UTIL::UpdateCriticallyDampedSpring` on a scalar.
fn spring(value: &mut f32, velocity: &mut f32, target: f32, halflife: f32, dt: f32) {
    if halflife <= 0.0 {
        (*value, *velocity) = (target, 0.0);
        return;
    }
    let decay = core::f32::consts::LN_2 / halflife;
    let scale = (-decay * dt).exp();
    let delta = *value - target;
    *velocity = (*velocity - delta * decay * decay * dt) * scale;
    *value = target + (delta + *velocity * dt) * scale;
}

/// `FreeLook`: the deadzone's body yaw and the twist driver's state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FreeLook {
    body_yaw: f32,
    velocity: f32,
    /// The deadzone holds a body yaw of its own (`Decoupled`).
    pub decoupled: bool,
    catching_up: bool,
    turn_playing: bool,
    turn_direction: f32,
    /// The twist runs (`TwistActive`).
    pub driving: bool,
}

impl FreeLook {
    /// `UpdateBodyYaw`: the body's heading toward `desired`; and the turn event to send, if any.
    pub fn update_body_yaw(&mut self, desired: f32, moving: bool, b: &BodySettings, dt: f32) -> (f32, Option<&'static str>) {
        if !self.decoupled {
            (self.body_yaw, self.velocity, self.decoupled, self.catching_up) = (desired, 0.0, true, false);
            return (self.body_yaw, self.turn(false, 0.0));
        }
        let threshold = b.fFreeLookMaxAngle.abs().to_radians();
        let mut delta = normalize_radian(desired - self.body_yaw);
        if !moving && !self.catching_up && delta.abs() <= threshold {
            self.velocity = 0.0;
            return (self.body_yaw, self.turn(false, 0.0));
        }
        self.catching_up = !moving;
        if delta.abs() > threshold {
            self.body_yaw = normalize_radian(desired - threshold.copysign(delta));
            delta = normalize_radian(desired - self.body_yaw);
        }
        if moving {
            let halflife = 1.0 / b.fFreeLookCatchupSpeed.max(1.0);
            let mut value = self.body_yaw;
            spring(&mut value, &mut self.velocity, self.body_yaw + delta, halflife, dt);
            self.body_yaw = normalize_radian(value);
        } else {
            let settle = b.fFreeLookSettleAngle.abs().to_radians().clamp(0.0, (threshold - 5f32.to_radians()).max(0.0));
            let remaining = delta.abs() - settle;
            if remaining <= 1f32.to_radians() {
                self.catching_up = false;
            } else {
                let ease_out_band = 15f32.to_radians();
                let rate = b.fFreeLookCatchupSpeed.max(1.0) * 10f32.to_radians() * (remaining / ease_out_band).clamp(0.25, 1.0);
                let step = remaining.min(rate * dt);
                self.body_yaw = normalize_radian(self.body_yaw + step.copysign(delta));
            }
        }
        let event = self.turn(self.catching_up && !moving, delta);
        (self.body_yaw, event)
    }

    /// `SnapBodyYaw`: the deadzone lets go; and the turn stop to send, if a turn plays.
    pub fn snap_body_yaw(&mut self) -> Option<&'static str> {
        self.decoupled = false;
        self.catching_up = false;
        self.turn(false, 0.0)
    }

    /// `UpdateTurnAnimation`: the pivot animation the standing shuffle plays in its direction.
    fn turn(&mut self, shuffling: bool, direction: f32) -> Option<&'static str> {
        if shuffling && (!self.turn_playing || direction * self.turn_direction < 0.0) {
            (self.turn_playing, self.turn_direction) = (true, direction);
            Some(if direction >= 0.0 { "TurnRight" } else { "TurnLeft" })
        } else if !shuffling && self.turn_playing {
            (self.turn_playing, self.turn_direction) = (false, 0.0);
            Some("turnStop")
        } else {
            None
        }
    }
}

static FREE_LOOK: Mutex<FreeLook> = Mutex::new(FreeLook { body_yaw: 0.0, velocity: 0.0, decoupled: false, catching_up: false, turn_playing: false, turn_direction: 0.0, driving: false });

pub fn with<R>(f: impl FnOnce(&mut FreeLook) -> R) -> R {
    f(&mut FREE_LOOK.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// `SendTurnEvent`: not while the player's graphs are torn down for a load.
pub fn send_turn_event(event: Option<&'static str>) {
    if let (Some(event), Some(player)) = (event, util::player())
        && player.get3_d().is_some()
    {
        player.notify_animation_graph(BSFixedString::new(event).raw());
    }
}

/// The player's third-person graph: the twist writes reach it alone, since the first-person
/// arms ride the camera and a spine twist there would swing them off screen.
fn body_graph(player: &PlayerCharacter) -> Option<&sre::BShkbAnimationGraph> {
    player.current_process()?.middle_high()?.animation_graph_manager()?.graphs().first().copied().flatten()
}

fn set_head_tracking(player: &PlayerCharacter, on: bool) {
    let state = player.as_actor_state();
    let flags = state.flags2;
    skyrim::store(&state.flags2, if on { flags | HEAD_TRACKING } else { flags & !HEAD_TRACKING });
}

/// Where the head looks: down the camera's look axis, pitch included, turned by the camera's
/// heading off the body's, since the engine resolves the target in the actor's frame, which in
/// first person follows the camera.
#[must_use]
pub fn look_target(position: point::P3, rotation: &point::M3, body_forward: Option<point::P3>) -> point::P3 {
    let mut look = point::mul_vec(rotation, [0.0, 1.0, 0.0]);
    if let Some(forward) = body_forward {
        let right = point::mul_vec(rotation, [1.0, 0.0, 0.0]);
        let camera_yaw = right[0].atan2(right[1]) - core::f32::consts::FRAC_PI_2;
        let delta = normalize_radian(camera_yaw - forward[0].atan2(forward[1]));
        let (sin, cos) = delta.sin_cos();
        look = [look[0] * cos + look[1] * sin, look[1] * cos - look[0] * sin, look[2]];
    }
    point::add(position, point::scale(look, LOOK_AT_DISTANCE))
}

/// `Drive`: in plain first person with the body and free look on and the body's state allowing
/// it, the twist's graph variables, the head-tracking switch, the stray slots cleared, the target
/// point set and the player's head tracking run; otherwise `Release`.
pub fn drive(profile: &Profile, camera: &Camera, dt: f32) {
    let Some(player) = util::player() else { return };
    let process = player.current_process().filter(|p| p.middle_high().is_some());
    let body = &profile.Body;
    let wanted = process.is_some()
        && player.get3_d().is_some()
        && profile.General.bEnableBody
        && body.bFreeLook
        && camera.mode == CameraMode::First
        && camera.current == CameraType::FirstPerson
        && camera.transition == CameraType::Unknown
        && decide(&super::snapshot::get()).twist;
    let (Some(process), true) = (process, wanted) else {
        release();
        return;
    };
    if !with(|f| core::mem::replace(&mut f.driving, true)) {
        crate::log(&format!("<FreeLook> Twist engaged (deadzone {:.0} deg, spine {})", body.fFreeLookMaxAngle, if body.bFreeLookSpine { "on" } else { "off" }));
    }
    if let Some(graph) = body_graph(player) {
        let gain = LOOK_AT_GAIN * body.fFreeLookTwistStrength.max(0.0);
        graph.set_graph_variable_bool(BSFixedString::new("IsNPC").raw(), true);
        graph.set_graph_variable_bool(BSFixedString::new("bHeadTrackSpine").raw(), body.bFreeLookSpine);
        graph.set_graph_variable_float(BSFixedString::new("LookAtOnGain").raw(), gain);
        graph.set_graph_variable_float(BSFixedString::new("LookAtOffGain").raw(), gain);
    }
    set_head_tracking(player, true);
    util::set_head_track_slot(player, HeadTrackSlot::Default, None);
    if !player.is_in_combat() {
        util::set_head_track_slot(player, HeadTrackSlot::CombatOrWeaponUse, None);
    }
    let forward = player.get3_d().and_then(|root| util::object_by_name(root, "NPC Root [Root]")).map(|root| point::mul_vec(&root.world_transform().rotation, [0.0, 1.0, 0.0]));
    let [x, y, z] = look_target(camera.position, &camera.rotation, forward);
    process.set_head_track_target_point(player, &NiPoint3 { x, y, z, ..NiPoint3::default() });
    player.update_head_tracking(dt);
}

/// `Release`: back to the engine's player graph state, once, when the twist stops.
pub fn release() {
    if !with(|f| core::mem::take(&mut f.driving)) {
        return;
    }
    if let Some(player) = util::player().filter(|p| p.get3_d().is_some()) {
        let holder = player.as_i_animation_graph_manager_holder();
        holder.set_graph_variable_bool(BSFixedString::new("IsNPC").raw(), false);
        holder.set_graph_variable_bool(BSFixedString::new("bHeadTrackSpine").raw(), false);
        holder.set_graph_variable_float(BSFixedString::new("LookAtOnGain").raw(), LOOK_AT_GAIN);
        holder.set_graph_variable_float(BSFixedString::new("LookAtOffGain").raw(), LOOK_AT_GAIN);
        set_head_tracking(player, false);
        if let Some(process) = player.current_process() {
            process.clear_head_tracking_active();
        }
        send_turn_event(Some("turnStop"));
    }
    crate::log("<FreeLook> Twist released");
}

#[cfg(test)]
mod tests {
    use super::{FreeLook, decide, look_target};
    use crate::body::snapshot::Data as Body;
    use crate::body::state::action::ActionType;
    use crate::body::state::hands::HandsType;
    use crate::body::state::movement::MovementType;
    use crate::body::state::race::RaceType;
    use crate::settings::Body as BodySettings;

    #[test]
    fn the_twist_runs_in_plain_locomotion_and_aiming_pins_the_body() {
        let mut body = Body { humanoid: true, ..Body::NONE };
        assert_eq!(decide(&body), super::Decision { twist: true, align: false });
        assert!(!decide(&Body::NONE).twist);
        body.hands = HandsType::Cast;
        assert_eq!(decide(&body), super::Decision { twist: true, align: true });
        body.hands = HandsType::Aim;
        assert_eq!(decide(&body), super::Decision { twist: false, align: true });
        body.hands = HandsType::None;
        body.movement = MovementType::Sprint;
        assert!(decide(&body).align);
        body.action = ActionType::Swim;
        assert!(!decide(&body).twist);
        body.action = ActionType::None;
        body.race = RaceType::Werewolf;
        assert!(!decide(&body).twist);
    }

    #[test]
    fn the_body_holds_inside_the_deadzone_and_is_leashed_beyond_it() {
        let b = BodySettings::default();
        let mut f = FreeLook::default();
        assert_eq!(f.update_body_yaw(0.0, false, &b, 0.016), (0.0, None));
        assert_eq!(f.update_body_yaw(1.0, false, &b, 0.016), (0.0, None));
        let (yaw, event) = f.update_body_yaw(2.0, false, &b, 0.016);
        let leash = 2.0 - 80f32.to_radians();
        assert!(yaw > leash && yaw < leash + 0.1, "{yaw}");
        assert_eq!(event, Some("TurnRight"));
        assert_eq!(f.snap_body_yaw(), Some("turnStop"));
        assert!(!f.decoupled);
    }

    #[test]
    fn walking_springs_the_body_toward_the_look() {
        let b = BodySettings::default();
        let mut f = FreeLook::default();
        f.update_body_yaw(0.0, true, &b, 0.016);
        let (yaw, _) = f.update_body_yaw(0.5, true, &b, 0.1);
        assert!(yaw > 0.0 && yaw < 0.5, "{yaw}");
    }

    #[test]
    fn the_target_lies_down_the_look_axis_turned_into_the_body_frame() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(look_target([0.0, 0.0, 100.0], &identity, None), [0.0, 500.0, 100.0]);
        let t = look_target([0.0; 3], &identity, Some([1.0, 0.0, 0.0]));
        assert!((t[0] + 500.0).abs() < 1e-3 && t[1].abs() < 1e-3, "{t:?}");
    }
}
