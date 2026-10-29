//! `camera/extended/*`: IC's own camera types. `CameraAnimation`, `CameraDialogue` (the forced
//! POV and the FOV hand-back), `CameraSpectator`; `CameraCrafting`, `CameraGathering`,
//! `CameraVampirelord`, `CameraWerewolf`, `CameraLich` and `CameraSex` are in `camera::states` and
//! `camera::base` with the base states they share their code with; `CameraMapMenu` does nothing,
//! and `CameraRaceMenu` keeps the body frozen (its mouse orbit is left out, see `docs/STATUS.md`).

use std::sync::Mutex;

use skyrim::actors::RefHandle;
use skyrim::prelude::*;
use sre::Actor;

use super::base::THIRD_PERSON_ZOOM;
use super::state::{CameraType, Modifier};
use super::states::Ctx;
use crate::body::state::action::ActionType;
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

/// `DialogueState`, with IC's dialogue camera (`Dialogue.bEnable`) only its forced POV. Its FOV
/// framing of the speaker, facing the speaker and head tracking need the dialogue target
/// (`MenuTopicManager`), and the speaker focus shots ray casts and the screen fade, none of which
/// are in the RE yet; `SmoothCam`'s API and the Scene Director check are other plugins' interfaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dialogue {
    previous: Option<CameraType>,
    previous_fov: f32,
    forced_pov: bool,
    target_zoom: f32,
}

impl Dialogue {
    pub fn enter(&mut self, previous: CameraType, ctx: &Ctx<'_>) {
        self.previous = Some(previous);
        self.previous_fov = ctx.camera.worldFOV;
        let dialogue = ctx.profile.Dialogue;
        if !dialogue.bEnable {
            return;
        }
        if dialogue.bForceThirdPerson && previous == CameraType::FirstPerson {
            ctx.camera.force_third_person();
            self.forced_pov = true;
        } else if dialogue.bForceFirstPerson && previous >= CameraType::ThirdPerson {
            self.target_zoom = ctx.camera.third_person().map_or(THIRD_PERSON_ZOOM, |t| t.targetZoom);
            ctx.camera.force_first_person();
            self.forced_pov = true;
        }
    }

    /// `OnExit`: the world FOV found on entry is handed back, and a forced POV undone.
    pub fn exit(&mut self, ctx: &mut Ctx<'_>) {
        if self.previous_fov > 0.0 {
            ctx.notes.fov_reset = self.previous_fov;
        }
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
