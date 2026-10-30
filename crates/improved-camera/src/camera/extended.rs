//! `camera/extended/*`: IC's own camera types. `CameraAnimation`, `CameraDialogue`,
//! `CameraSpectator`, `CameraRaceMenu`; `CameraCrafting`, `CameraGathering`, `CameraVampirelord`,
//! `CameraWerewolf`, `CameraLich` and `CameraSex` are in `camera::states` and `camera::base` with
//! the base states they share their code with; `CameraMapMenu` does nothing.

use std::sync::Mutex;

use skyrim::actors::RefHandle;
use skyrim::prelude::*;
use skyrim::ui::Value;
use sre::{Actor, HeadTrackSlot, PlayerCamera};

use super::base::THIRD_PERSON_ZOOM;
use super::state::{CameraType, Modifier};
use super::states::Ctx;
use super::util::orbit::{OrbitController, Pose};
use crate::body::state::action::ActionType;
use crate::settings::{Dialogue as DialogueSettings, Profile};
use crate::util::math_util::{damped_alpha, lerp, normalize_radian, smoother_step};
use crate::util::point::{self, M3, P3};
use crate::util::quaternion_util::Quat;
use crate::util::skyrim_util::{self as util, FAKE_FIRST_PERSON_ZOOM};

/// `AnimationState::OnEnter`.
pub fn animation_enter(should_sim: &mut bool, previous: CameraType, ctx: &Ctx<'_>) {
    let Some(third) = ctx.camera.third_person() else { return };
    let action = ctx.body.action;
    if matches!(previous, CameraType::Dialogue | CameraType::Free | CameraType::Furniture | CameraType::MapMenu | CameraType::ThirdPerson | CameraType::TweenMenu) {
        if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM {
            ctx.camera.force_third_person();
        }
        util::set_pov_switch(false);
        *should_sim = true;
    }
    if action == ActionType::BoatRide {
        if let Some(player) = util::player()
            && player.rotX != 0.0
        {
            util::set_pitch(player, 0.0);
        }
        util::set_pov_switch(false);
        *should_sim = false;
    }
    if action == ActionType::CartRide {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
        util::set_pov_switch(false);
        *should_sim = true;
    }
    if matches!(action, ActionType::AI | ActionType::Bathe | ActionType::Sit | ActionType::View) {
        util::set_pov_switch(false);
        if ctx.camera.state() == Some(sre::CameraState::FirstPerson) {
            ctx.camera.force_third_person();
            util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
        }
        *should_sim = true;
    }
}

/// `AnimationState::OnUpdate`: the Ravenrock boat ride's intro goes to real first person.
pub fn animation_update(should_sim: &mut bool, m: &mut Modifier, ctx: &Ctx<'_>) {
    m.sim_first_person = *should_sim;
    if ctx.body.action == ActionType::BoatRide && *should_sim {
        *should_sim = false;
        m.sim_first_person = false;
        if util::pov_switch() {
            util::set_pov_switch(false);
            ctx.camera.force_first_person();
        }
    }
}

/// `DialogueMenuState`: the dialogue menu movie's `eMenuState`.
const SHOW_GREETING: f64 = 0.0;
const TOPIC_LIST_SHOWN: f64 = 1.0;
const TRANSITIONING: f64 = 3.0;

const TOPIC_LIST: &str = "_root.DialogueMenu_mc.TopicListHolder._visible";

/// `IsSpeakerTalking`: whether a line plays, from the dialogue menu movie's state. The topic list
/// is shown while the topics are up, hidden for the greeting, and otherwise shown only without
/// `bHideDialogueMenu`.
fn is_speaker_talking(dialogue: &DialogueSettings) -> bool {
    let Some(movie) = util::ui().and_then(|ui| ui.menu("Dialogue Menu")).and_then(|menu| unsafe { menu.movie.as_ref() }) else { return false };
    let state = movie.variable("_root.DialogueMenu_mc.eMenuState").and_then(|v| v.as_number());
    #[allow(clippy::float_cmp)]
    let (visible, talking) = match state {
        Some(s) if s == TOPIC_LIST_SHOWN || s == TRANSITIONING => (true, false),
        Some(s) if s == SHOW_GREETING => (false, true),
        _ => (!dialogue.bHideDialogueMenu, true),
    };
    movie.set_variable(TOPIC_LIST, &Value::boolean(visible));
    talking
}

/// `UsingSpeakerFocus`: the speaker focus shots hold between lines when locked on, else only while
/// a line plays. IC's fade to black between shots is left out, so what is wanted is presented.
#[must_use]
pub fn using_speaker_focus(dialogue: &DialogueSettings) -> bool {
    dialogue.bSwitchTarget && (dialogue.bLockOn || is_speaker_talking(dialogue))
}

/// `ComputeFOV`: the world FOV that frames the speaker's head at the preferred distance; the
/// default while no speaker is framed or a focus shot is up.
#[must_use]
pub fn framing_fov(preferred: f32, distance: f32, first_person: bool, default: f32) -> f32 {
    if distance < 1.0 {
        return default;
    }
    ((preferred / distance).atan() * 2.0).to_degrees().max(if first_person { 20.0 } else { 30.0 })
}

/// How far in front of an obstruction a focus shot stops.
const FOCUS_PULLBACK: f32 = 10.0;

/// A focus shot's camera position: `distance` back from `anchor` along the level line to
/// `subject`, moved by the shoulder offset (x to the side, y along the line, z up); `None` when
/// the two stand within a unit of each other.
#[must_use]
pub fn shoulder_position(anchor: P3, subject: P3, shoulder: P3, distance: f32) -> Option<P3> {
    let to_subject = point::sub(subject, anchor);
    let (to_subject, length) = point::unitize([to_subject[0], to_subject[1], 0.0]);
    if length < 1.0 {
        return None;
    }
    let up = [0.0, 0.0, 1.0];
    let side = point::cross(to_subject, up);
    Some(point::add(point::add(point::add(anchor, point::scale(to_subject, shoulder[1] - distance)), point::scale(side, shoulder[0])), point::scale(up, shoulder[2])))
}

/// The heading and pitch (positive looking down) that face `to` from `from`; `None` within a unit
/// on the ground plane.
#[must_use]
pub fn facing(from: P3, to: P3) -> Option<(f32, f32)> {
    let v = point::sub(to, from);
    let flat = v[0].hypot(v[1]);
    (flat >= 1.0).then(|| (v[0].atan2(v[1]), (-v[2]).atan2(flat)))
}

/// `RotateToTarget`: the player turned toward the speaker's head at `fCameraSpeed`, at once on
/// the first update.
fn rotate_to_target(player: &sre::PlayerCharacter, target: &Actor, speed: f32, dt: f32, snap: bool) {
    let Some((yaw, pitch)) = facing(util::head_position(player, 0.95), util::head_position(target, 0.0)) else { return };
    let share = if snap { 1.0 } else { damped_alpha(speed * 0.01, dt) };
    util::set_heading(player, player.rotZ + normalize_radian(yaw - player.rotZ) * share);
    util::set_pitch(player, lerp(player.rotX, pitch, share));
}

/// `SetHeadTracking`: the player's graph honours a head-track target only as an "NPC"; `None`
/// clears the dialogue slot.
fn set_head_tracking(target: Option<&Actor>) {
    let Some(player) = util::player() else { return };
    player.as_i_animation_graph_manager_holder().set_graph_variable_bool(skyrim::BSFixedString::new("IsNPC").raw(), target.is_some());
    util::set_head_track_slot(player, HeadTrackSlot::DialogueWithPlayer, target.map(|a| &**a));
}

/// The shot/reverse-shot of the speaker focus: the camera over the player's shoulder at the
/// speaker while a line plays, over the speaker's shoulder at the player for the reply, eased
/// between the two.
#[derive(Clone, Copy, Debug, Default)]
struct Focus {
    shot: Option<(P3, Quat)>,
    blend_start: (P3, Quat),
    blend_time: f32,
    last_npc_speaking: bool,
}

/// `DialogueState`. `SmoothCam`'s API, the Scene Director check, the letterbox and the fade to
/// black between focus shots (IC's own overlay effects) are left out.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dialogue {
    previous: Option<CameraType>,
    previous_fov: f32,
    forced_pov: bool,
    target_zoom: f32,
    active: bool,
    /// The aim snapped to the speaker on the first update (IC's `m_snapToFace` cleared).
    faced: bool,
    focus: Focus,
}

impl Dialogue {
    pub fn enter(&mut self, previous: CameraType, ctx: &Ctx<'_>) {
        self.previous = Some(previous);
        self.previous_fov = ctx.camera.worldFOV;
        let dialogue = ctx.profile.Dialogue;
        if !dialogue.bEnable {
            return;
        }
        self.active = true;
        if dialogue.bForceThirdPerson && previous == CameraType::FirstPerson {
            ctx.camera.force_third_person();
            self.forced_pov = true;
        } else if dialogue.bForceFirstPerson && previous >= CameraType::ThirdPerson {
            self.target_zoom = ctx.camera.third_person().map_or(THIRD_PERSON_ZOOM, |t| t.targetZoom);
            ctx.camera.force_first_person();
            self.forced_pov = true;
        }
    }

    /// `OnUpdate`: the speaker framed by the FOV, faced and head-tracked.
    pub fn update(&mut self, m: &mut Modifier, ctx: &Ctx<'_>, focus: bool) {
        if !self.active {
            return;
        }
        let dialogue = &ctx.profile.Dialogue;
        let target = util::dialogue_target();
        let actor = target.as_deref().and_then(util::actor_of);
        m.fov = match (actor, ctx.camera.camera_root()) {
            (Some(actor), Some(root)) if !focus => {
                let first = util::is_first_person();
                let preferred = if util::is_flying(actor) {
                    dialogue.fDragonZoom
                } else if first {
                    dialogue.fFirstPersonZoom
                } else {
                    dialogue.fThirdPersonZoom
                };
                let distance = point::length(point::sub(root.world_transform().translation, util::head_position(actor, 0.0)));
                framing_fov(preferred, distance, first, dialogue.fDefaultWorldFOV)
            }
            _ => dialogue.fDefaultWorldFOV,
        };
        let (Some(player), Some(actor)) = (util::player(), actor) else { return };
        if dialogue.bFaceSpeaker {
            rotate_to_target(player, actor, dialogue.fCameraSpeed, util::delta_frame_time(), !self.faced);
        }
        self.faced = true;
        if dialogue.bHeadTracking {
            set_head_tracking(Some(actor));
        }
    }

    /// `SpeakerFocus`: the authored shot `CaptureCameraState` presents, while `focus` is wanted.
    pub fn speaker_focus(&mut self, camera: &PlayerCamera, profile: &Profile, focus: bool) -> Option<(P3, M3)> {
        let f = &mut self.focus;
        if !focus {
            f.shot = None;
            return None;
        }
        let dialogue = &profile.Dialogue;
        let npc_speaking = is_speaker_talking(dialogue);
        let target = util::dialogue_target()?;
        let target = util::actor_of(&target)?;
        let player = util::player()?;
        camera.camera_root()?;
        let head = util::actor_head(target);
        let npc_head = head.map_or_else(|| util::position(target), |h| h.world_transform().translation);
        let humanoid = head.is_some() && !util::is_flying(target);
        let offset = if humanoid {
            [dialogue.fHumanCamOffsetX, dialogue.fHumanCamOffsetY, dialogue.fHumanCamOffsetZ]
        } else {
            [dialogue.fCreatureCamOffsetX, dialogue.fCreatureCamOffsetY, dialogue.fCreatureCamOffsetZ]
        };
        let player_head = util::head_position(player, 0.95);
        let (subject, anchor) = if npc_speaking { (npc_head, player_head) } else { (player_head, point::add(npc_head, offset)) };
        let distance = if npc_speaking && !humanoid { dialogue.fDragonZoom } else { dialogue.fThirdPersonZoom };
        let shoulder = if npc_speaking {
            [dialogue.fPlayerShoulderPosX, dialogue.fPlayerShoulderPosY, dialogue.fPlayerShoulderPosZ]
        } else {
            [dialogue.fNpcShoulderPosX, dialogue.fNpcShoulderPosY, dialogue.fNpcShoulderPosZ]
        };
        let mut position = shoulder_position(anchor, subject, shoulder, distance)?;
        if let Some(hit) = util::cast_ray(anchor, position).filter(|h| util::is_solid_hit(h.layer, true)) {
            let (to_anchor, length) = point::unitize(point::sub(anchor, hit.position));
            if length > 0.0 {
                position = point::add(hit.position, point::scale(to_anchor, FOCUS_PULLBACK));
            }
        }
        let orientation = Quat::from_matrix(&util::look_rotation(position, subject)?);
        let duration = (400.0 / dialogue.fCameraSpeed).clamp(0.2, 2.0);
        let dt = util::delta_frame_time();
        match f.shot {
            Some((shot_position, shot_orientation)) if dialogue.bSmoothTransition => {
                if npc_speaking != f.last_npc_speaking {
                    f.blend_start = (shot_position, shot_orientation);
                    f.blend_time = 0.0;
                    f.last_npc_speaking = npc_speaking;
                }
                f.blend_time += dt;
                let (from, share) = if f.blend_time < duration { (f.blend_start, smoother_step(f.blend_time / duration)) } else { ((shot_position, shot_orientation), damped_alpha(8.0, dt)) };
                f.shot = Some((point::lerp(from.0, position, share), Quat::slerp(from.1, orientation, share)));
            }
            _ => {
                f.shot = Some((position, orientation));
                f.blend_time = duration;
                f.last_npc_speaking = npc_speaking;
            }
        }
        f.shot.map(|(p, o)| (p, o.matrix()))
    }

    /// For an unload: the head tracking cleared and the topic list shown.
    pub fn release(&mut self, profile: &Profile) {
        if self.active && profile.Dialogue.bHeadTracking {
            set_head_tracking(None);
        }
        if let Some(movie) = util::ui().and_then(|ui| ui.menu("Dialogue Menu")).and_then(|menu| unsafe { menu.movie.as_ref() }) {
            movie.set_variable(TOPIC_LIST, &Value::boolean(true));
        }
    }

    /// `OnExit`: the world FOV found on entry is handed back, the head tracking cleared and a
    /// forced POV undone.
    pub fn exit(&mut self, ctx: &mut Ctx<'_>) {
        if self.previous_fov > 0.0 {
            ctx.notes.fov_reset = self.previous_fov;
        }
        if ctx.profile.Dialogue.bHeadTracking {
            set_head_tracking(None);
        }
        self.focus.shot = None;
        if !self.forced_pov {
            return;
        }
        match self.previous {
            Some(CameraType::FirstPerson) => ctx.camera.force_first_person(),
            Some(p) if p >= CameraType::ThirdPerson => {
                ctx.camera.force_third_person();
                if let Some(third) = ctx.camera.third_person() {
                    util::set_zoom(third, self.target_zoom);
                }
            }
            _ => {}
        }
    }
}

/// The share of the screen width left of which the race menu's panels sit: the engine starts its
/// own head-look drag only right of it (RE `camera.md` IC5 D).
const RACE_MENU_PANEL: f32 = 0.45;

/// Whether the cursor at `x` pixels is over the race menu's panels rather than the character, on
/// a screen `width` pixels wide. IC hit-tests the menu movie; the engine has no hit test and
/// splits the screen at 45 % for its own drag, which this follows.
#[must_use]
pub fn cursor_over_menu(x: f32, width: f32) -> bool {
    width <= 0.0 || x / width <= RACE_MENU_PANEL
}

/// `RaceMenuState`: the body frozen as the menu poses it, and the mouse orbit around the player on
/// the menu's own camera, taking button presses only with the cursor off the menu's panels and
/// none while a message box (the name entry) is up.
#[derive(Clone, Debug)]
pub struct RaceMenu {
    pub previous: CameraType,
    orbit: OrbitController,
}

impl RaceMenu {
    #[must_use]
    pub fn new() -> Self {
        Self { previous: CameraType::Unknown, orbit: OrbitController::new() }
    }

    pub fn enter(&mut self, previous: CameraType) {
        self.previous = previous;
        self.orbit.reset();
    }

    pub fn update(&mut self, ctx: &Ctx<'_>) {
        if matches!(self.previous, CameraType::FirstPerson | CameraType::ThirdPerson | CameraType::Animation) {
            super::base::freeze_body();
        }
        let Some(menu) = util::ui().and_then(|ui| ui.race_sex_menu()) else {
            self.orbit.cancel_interaction();
            return;
        };
        let mouse = &ctx.input.mouse;
        if util::is_menu_open("MessageBoxMenu") {
            self.orbit.cancel_interaction();
        } else {
            if !sre::UICursor::singleton().is_none_or(|c| cursor_over_menu(c.x, menu.screenWidth)) {
                for &(button, down) in &mouse.buttons {
                    self.orbit.button(button, down);
                }
            }
            if mouse.dx != 0 || mouse.dy != 0 {
                self.orbit.moved(mouse.dx, mouse.dy);
            }
        }
        let (Some(root), Some(player)) = (unsafe { menu.camera().cameraRoot.as_ref() }, util::player()) else { return };
        let local = root.local_transform();
        if let Some(pose) = self.orbit.update(Pose { position: local.translation, rotation: local.rotation }, util::position(player)) {
            super::base::write_root(root, pose);
        }
    }

    /// `OnExit`, and an unload: the camera root back where the orbit found it.
    pub fn exit(&mut self, camera: &PlayerCamera) {
        super::base::orbit_release(&mut self.orbit, camera);
        if self.previous == CameraType::FirstPerson {
            super::base::freeze_body();
        }
    }
}

/// The camera target writes the spectator makes, applied on the next frame from the frame
/// callback: `PlayerCamera::SetTarget` from the main thread outside `PlayerCamera::Update`, and the
/// player-only `ForceFirstPerson` after the player is the target again (RE `camera.md`, IC2 B).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Deferred {
    /// The actor to follow (`RefHandle::INVALID` for the player), when one was asked.
    target: Option<RefHandle>,
    force_first_person: bool,
}

static DEFERRED: Mutex<Deferred> = Mutex::new(Deferred { target: None, force_first_person: false });

fn defer(f: impl FnOnce(&mut Deferred)) {
    f(&mut DEFERRED.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
}

/// Writes the heading of the actor the camera now follows into the third-person state's yaw
/// (+0x7c, +0x80): the state's `Begin` does not run again on a target change.
fn seed_yaw(actor: &Actor) {
    if let Some(third) = util::camera().and_then(PlayerCameraExt::third_person) {
        skyrim::store(&third.targetYaw, actor.rotZ);
        skyrim::store(&third.currentYaw, actor.rotZ);
    }
}

/// From the frame callback: the deferred target and first-person switch, and the player back as
/// the target when the followed reference stops resolving (unloaded or deleted), which would
/// otherwise skip every `PlayerCamera::Update` and freeze the camera.
pub fn apply_deferred() {
    let (Some(camera), Some(player)) = (util::camera(), util::player()) else { return };
    let deferred = core::mem::take(&mut *DEFERRED.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let player_handle = player.handle();
    let current = RefHandle(camera.targetHandle);
    if current != player_handle && current.lookup().is_none() {
        util::set_camera_target(None);
        seed_yaw(player);
    }
    if let Some(handle) = deferred.target {
        let target = (handle != RefHandle::INVALID).then(|| handle.lookup()).flatten();
        let actor = target.as_deref().and_then(util::actor_of).filter(|a| a.get3_d().is_some());
        util::set_camera_target(actor);
        seed_yaw(actor.unwrap_or(player));
    }
    if deferred.force_first_person {
        camera.force_first_person();
    }
}

/// `Enable_PlayerControls`.
fn enable_player_controls(enable: bool, controlled: bool) {
    if let Some(player) = util::player() {
        sre::set_player_ai_driven(player, !enable);
    }
    for h in [util::Handler::Activate, util::Handler::AttackBlock, util::Handler::ReadyWeapon] {
        util::set_handler(h, enable);
    }
    util::set_handler(util::Handler::Look, controlled);
}

/// The head's parent node of `actor`, which the spectator shrinks to hide the head.
fn neck(actor: &Actor) -> Option<&'static sre::NiAVObject> {
    util::actor_head(actor).and_then(|head| unsafe { head.parent.cast::<sre::NiAVObject>().as_ref() })
}

/// `SpectatorState`: the camera on a possessed actor (the console's `ToggleControlsDriven`). The
/// actor is kept by handle and its neck found again from it, so a node of an unloaded actor is
/// never written.
#[derive(Clone, Copy, Debug)]
pub struct Spectator {
    controlled: bool,
    sex: bool,
    actor: Option<RefHandle>,
    neck_scale: f32,
    frame_counter: u32,
    previous: CameraType,
}

impl Default for Spectator {
    fn default() -> Self {
        Self { controlled: false, sex: false, actor: None, neck_scale: 1.0, frame_counter: 0, previous: CameraType::Unknown }
    }
}

impl Spectator {
    pub fn enter(&mut self, previous: CameraType, ctx: &Ctx<'_>) {
        if previous == CameraType::FirstPerson {
            ctx.camera.force_third_person();
            if let Some(third) = ctx.camera.third_person() {
                util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
            }
        }
        util::set_pov_switch(false);
        if let Some(target) = util::camera_target()
            && let Some(actor) = util::actor_of(&target)
        {
            if actor.get3_d().is_some() {
                self.controlled = unsafe { actor.movementController.as_ref() }.is_some_and(sre::MovementControllerNPC::is_controls_driven);
                if let Some(neck) = neck(actor) {
                    self.neck_scale = neck.local_transform().scale;
                    neck.set_local_scale(0.001);
                }
            }
            self.actor = Some(actor.handle());
        }
        self.previous = previous;
        self.sex = ctx.body.action == ActionType::Sex;
        if !self.sex {
            enable_player_controls(false, self.controlled);
        }
    }

    pub fn update(&mut self, m: &mut Modifier, ctx: &Ctx<'_>) {
        m.fov = ctx.profile.FOV.fThirdPerson;
        m.local_offset = ctx.profile.Camera.Spectator.point();
        m.sim_first_person = true;
        let target = util::camera_target();
        let actor = target.as_deref().and_then(util::actor_of);
        if self.sex && ctx.body.action != ActionType::Sex {
            self.sex = false;
            self.reset_target(actor);
        }
        if actor.map(|a| a.handle()) != self.actor {
            self.reset_target(actor);
        }
        if let Some(actor) = actor {
            match actor.get3_d() {
                Some(model) => {
                    self.hide_head(ctx, actor);
                    let last = model.previousWorldFrame;
                    if last == self.frame_counter && !self.sex {
                        self.reset_target(Some(actor));
                    }
                    self.frame_counter = last;
                }
                None => self.reset_target(Some(actor)),
            }
        }
        util::show_crosshair(false);
        super::manager::notify_free_look();
    }

    /// `OnExit`; IC switches to first person before the target goes back to the player, where
    /// the switch does nothing (IC2 B), so here it follows the target.
    pub fn exit(&mut self, m: &mut Modifier) {
        m.sim_first_person = false;
        enable_player_controls(true, true);
        util::show_crosshair(true);
        util::set_pov_switch(true);
        let followed = self.actor.and_then(RefHandle::lookup);
        self.reset_target(followed.as_deref().and_then(util::actor_of));
        if self.previous == CameraType::FirstPerson {
            defer(|d| d.force_first_person = true);
        }
    }

    /// For an unload: the neck, the controls and the player as the target at once.
    pub fn release(&mut self) {
        self.restore_neck();
        enable_player_controls(true, true);
        util::show_crosshair(true);
        util::set_pov_switch(true);
        util::set_camera_target(None);
        if let Some(player) = util::player() {
            seed_yaw(player);
        }
    }

    fn restore_neck(&mut self) {
        if let Some(neck) = self.actor.and_then(RefHandle::lookup).as_deref().and_then(util::actor_of).and_then(neck) {
            neck.set_local_scale(self.neck_scale);
        }
    }

    /// `HideHead`: the neck shrunk in fake first person unless the spectator's head is enabled.
    fn hide_head(&self, ctx: &Ctx<'_>, actor: &Actor) {
        let Some(neck) = neck(actor) else { return };
        let zoom = ctx.camera.third_person().map_or(0.0, |t| t.targetZoom);
        let show = ctx.profile.General.bEnableHeadSpectator;
        let scale = neck.local_transform().scale;
        if !show && zoom <= FAKE_FIRST_PERSON_ZOOM && scale > 0.001 {
            neck.set_local_scale(0.001);
        } else if (show || zoom > -0.27) && scale < 0.1 {
            neck.set_local_scale(self.neck_scale);
        }
    }

    /// `Reset_CameraTarget`: back to the player when `actor` is the one followed (or gone), or on
    /// to `actor` when the target changed.
    fn reset_target(&mut self, actor: Option<&Actor>) {
        if let Some(actor) = actor.filter(|a| a.get3_d().is_some()) {
            if self.controlled
                && let Some(control) = unsafe { actor.movementController.as_ref() }
            {
                control.set_ai_driven();
            }
            let handle = actor.handle();
            if Some(handle) == self.actor {
                self.restore_neck();
                self.neck_scale = 1.0;
                self.actor = None;
            } else {
                self.restore_neck();
                self.neck_scale = neck(actor).map_or(1.0, |n| n.local_transform().scale);
                self.actor = Some(handle);
                defer(|d| d.target = Some(handle));
                return;
            }
        }
        defer(|d| d.target = Some(RefHandle::INVALID));
    }
}

#[cfg(test)]
mod tests {
    use super::{cursor_over_menu, facing, framing_fov, shoulder_position};

    #[test]
    fn the_race_menu_panels_take_the_left_45_percent() {
        assert!(cursor_over_menu(864.0, 1920.0));
        assert!(!cursor_over_menu(865.0, 1920.0));
        assert!(cursor_over_menu(1500.0, 0.0));
    }

    #[test]
    fn the_dialogue_fov_frames_the_preferred_distance() {
        assert!((framing_fov(75.0, 75.0, false, 55.0) - 90.0).abs() < 1e-3);
        assert!((framing_fov(10.0, 1000.0, false, 55.0) - 30.0).abs() < f32::EPSILON);
        assert!((framing_fov(10.0, 1000.0, true, 55.0) - 20.0).abs() < f32::EPSILON);
        assert!((framing_fov(75.0, 0.5, true, 55.0) - 55.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_focus_shot_sits_back_from_the_anchor_beside_the_line() {
        let p = shoulder_position([0.0; 3], [0.0, 100.0, 50.0], [60.0, 0.0, 5.0], 75.0).unwrap();
        assert!((p[0] - 60.0).abs() < 1e-4 && (p[1] + 75.0).abs() < 1e-4 && (p[2] - 5.0).abs() < 1e-4);
        assert!(shoulder_position([0.0; 3], [0.5, 0.0, 100.0], [0.0; 3], 75.0).is_none());
    }

    #[test]
    fn facing_gives_heading_from_y_and_pitch_down_positive() {
        let (yaw, pitch) = facing([0.0; 3], [100.0, 0.0, -100.0]).unwrap();
        assert!((yaw - core::f32::consts::FRAC_PI_2).abs() < 1e-6 && (pitch - core::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!(facing([0.0; 3], [0.0, 0.5, 10.0]).is_none());
    }
}
