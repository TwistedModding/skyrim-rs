//! `camera/base/*`: the states of the engine's own player camera states. `CameraFirstPerson`,
//! `CameraThirdPerson`, `CameraVanity`, `CameraVATS`, `CameraFree`, `CameraFurniture`,
//! `CameraTransition` (with the beast and lich transitions), `CameraTweenMenu`, `CameraHorse`,
//! `CameraBleedout` (bleedout and dead) and `CameraDragon`; `CameraIronSight` and `CameraAnimated`
//! do nothing.

use skyrim::prelude::*;
use sre::{CameraState, PlayerCamera};

use super::state::{CameraType, Modifier};
use super::states::Ctx;
use super::util::orbit::{OrbitController, Pose};
use crate::body::state::action::ActionType;
use crate::body::state::hands::HandsType;
use crate::body::state::race::RaceType;
use crate::util::skyrim_util::{self as util, FAKE_FIRST_PERSON_ZOOM};

/// The third-person zoom IC parks the camera at when it leaves fake first person.
pub const THIRD_PERSON_ZOOM: f32 = -0.200;

fn third<'a>(ctx: &Ctx<'a>) -> Option<&'a sre::ThirdPersonState> {
    ctx.camera.third_person()
}

fn fake_first(ctx: &Ctx<'_>) -> bool {
    ctx.profile.General.bEnableFakeFirstPerson
}

fn hands_drawn(ctx: &Ctx<'_>) -> bool {
    ctx.body.hands > HandsType::None
}

/// The engine's first-person switch, and IC's note to itself that it forced it.
fn force_first_person(ctx: &mut Ctx<'_>) {
    ctx.camera.force_first_person();
    ctx.notes.forced_first_person = true;
}

/// A first-person modifier: IC's first-person FOV and camera offset, the combat ones with the hands drawn.
fn first_person_modifier(m: &mut Modifier, ctx: &Ctx<'_>) {
    let drawn = hands_drawn(ctx);
    m.fov = if drawn { ctx.profile.FOV.fFirstPersonCombat } else { ctx.profile.FOV.fFirstPerson };
    m.local_offset = ctx.profile.Firstperson.offset(drawn).point();
}

/// `UpdateAnimation_ThirdPerson(player3D, true)`: the body frozen where it stands.
pub fn freeze_body() {
    if let Some(third) = util::player().and_then(PlayerCharacterExt::third_person_root) {
        crate::body::manager::update_animation_third_person(third, true);
    }
}

fn body_root(first_person: bool) -> Option<&'static sre::NiAVObject> {
    let player = util::player()?;
    if first_person {
        let root: &sre::NiNode = player.first_person_root()?;
        Some(root)
    } else {
        player.third_person_root()
    }
}

fn set_body_hidden(first_person: bool, hidden: bool) {
    if let Some(root) = body_root(first_person) {
        root.set_hidden(hidden);
    }
}

fn body_hidden(first_person: bool) -> bool {
    body_root(first_person).is_some_and(|r| r.flags & 1 != 0)
}

/// `FirstPersonState::OnEnter`.
pub fn first_person_enter(previous: CameraType, ctx: &mut Ctx<'_>) {
    let Some(third) = third(ctx) else { return };
    if matches!(previous, CameraType::Crafting | CameraType::Gathering) && third.targetZoom > -0.27 {
        util::set_zoom(third, THIRD_PERSON_ZOOM);
        ctx.camera.force_third_person();
    }
    if previous == CameraType::Furniture && matches!(ctx.body.action, ActionType::Animation | ActionType::Sit | ActionType::Sleep) {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
        ctx.camera.force_third_person();
        util::set_pov_switch(false);
    }
    if previous == CameraType::Animation {
        util::set_pov_switch(true);
    }
}

/// `FirstPersonState::OnUpdate`: the first-person FOV and offset, and the third-person root shown.
pub fn first_person_update(m: &mut Modifier, ctx: &Ctx<'_>) {
    first_person_modifier(m, ctx);
    if body_hidden(false) {
        set_body_hidden(false, false);
    }
}

/// `ThirdPersonState`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThirdPerson {
    was_custom_action: bool,
    body_fix: bool,
}

/// `shouldForceFirstperson`: whether leaving `previous` for third person goes back to first person.
fn should_force_first_person(previous: CameraType, action: ActionType, ctx: &mut Ctx<'_>) -> bool {
    let fake = fake_first(ctx);
    match previous {
        CameraType::Animation => {
            if action == ActionType::None && !fake && third(ctx).is_some_and(|t| t.targetZoom <= FAKE_FIRST_PERSON_ZOOM) {
                force_first_person(ctx);
            }
            !matches!(action, ActionType::AI | ActionType::None)
        }
        CameraType::Bleedout | CameraType::Crafting | CameraType::Gathering | CameraType::Lich | CameraType::Transition | CameraType::Vampirelord | CameraType::Werewolf => !fake,
        CameraType::Furniture => {
            if matches!(action, ActionType::Animation | ActionType::Sit | ActionType::Sleep) {
                util::set_pov_switch(false);
                return false;
            }
            if action == ActionType::AI {
                util::set_pov_switch(true);
                return false;
            }
            !fake
        }
        CameraType::Unknown => {
            if !fake {
                util::set_pov_switch(true);
            }
            !fake
        }
        _ => false,
    }
}

impl ThirdPerson {
    pub fn enter(&mut self, previous: CameraType, ctx: &mut Ctx<'_>) {
        let Some(third) = third(ctx) else { return };
        let action = ctx.body.action;
        if previous == CameraType::FirstPerson && third.inputHandler.enabled == 0 {
            util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
            return;
        }
        if previous == CameraType::FirstPerson && action == ActionType::View {
            util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
            self.was_custom_action = true;
            return;
        }
        if previous == CameraType::Animation && action == ActionType::AI {
            return;
        }
        if previous == CameraType::FirstPerson && fake_first(ctx) {
            self.body_fix = true;
            return;
        }
        if should_force_first_person(previous, action, ctx) && third.targetZoom <= FAKE_FIRST_PERSON_ZOOM {
            util::set_zoom(third, THIRD_PERSON_ZOOM);
            force_first_person(ctx);
        }
    }

    pub fn update(&mut self, m: &mut Modifier, ctx: &mut Ctx<'_>) {
        m.fov = ctx.profile.FOV.fThirdPerson;
        let Some(third) = third(ctx) else { return };
        let (action, race) = (ctx.body.action, ctx.body.race);
        let fake = fake_first(ctx);
        if fake {
            if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM {
                first_person_modifier(m, ctx);
            }
            if self.body_fix && !body_hidden(true) {
                self.body_fix = false;
                set_body_hidden(true, true);
            }
        }
        if action == ActionType::AI && !self.was_custom_action {
            self.was_custom_action = true;
            m.sim_first_person = true;
            util::set_pov_switch(false);
        }
        let triumvirate = matches!(race, RaceType::Hornedlord | RaceType::Deer);
        if triumvirate {
            self.was_custom_action = true;
            if !body_hidden(true) {
                set_body_hidden(true, true);
            }
        }
        if self.was_custom_action && action == ActionType::None && !triumvirate {
            self.was_custom_action = false;
            if !fake {
                m.sim_first_person = false;
                if third.targetZoom <= FAKE_FIRST_PERSON_ZOOM {
                    util::set_zoom(third, THIRD_PERSON_ZOOM);
                    force_first_person(ctx);
                }
            }
        }
    }
}

/// `VanityState::OnUpdate`: with IC's idle camera on (and the body), its FOV; the authored orbit
/// shot, its collision pull-in and the focus on nearby actors wait on ray casts in the RE.
pub fn vanity_update(m: &mut Modifier, ctx: &Ctx<'_>) {
    m.fov = if vanity_enabled(ctx.profile) { ctx.profile.Vanity.fFOV } else { ctx.profile.FOV.fThirdPerson };
}

/// `VanityState::Enabled`: the idle camera is IC's only with the body on.
pub fn vanity_enabled(profile: &crate::settings::Profile) -> bool {
    profile.Vanity.bEnable && profile.General.bEnableBody
}

/// The orbit of the VATS, bleedout, dead and gathering states: the frame's mouse, then the orbit
/// around the player while the zoom of `zoom_state` is out of fake first person.
pub fn orbit_update(orbit: &mut OrbitController, m: &mut Modifier, ctx: &Ctx<'_>, fov: f32, zoom_state: CameraState) {
    m.fov = fov;
    m.sim_first_person = true;
    orbit.input(&ctx.input.mouse);
    let (Some(root), Some(state), Some(player)) = (ctx.camera.camera_root(), util::family(ctx.camera, zoom_state), util::player()) else { return };
    if state.targetZoom > -0.27 {
        let local = root.local_transform();
        if let Some(pose) = orbit.update(Pose { position: local.translation, rotation: local.rotation }, [player.posX, player.posY, player.posZ]) {
            write_root(root, pose);
        }
    }
}

pub fn orbit_release(orbit: &mut OrbitController, camera: &PlayerCamera) {
    if let (Some(root), Some(pose)) = (camera.camera_root(), orbit.release()) {
        write_root(root, pose);
    }
}

fn write_root(root: &sre::NiAVObject, pose: Pose) {
    let mut local = root.local_transform();
    local.translation = pose.position;
    local.rotation = pose.rotation;
    root.set_local_transform(local);
    root.update_transforms();
}

/// `BleedoutState::OnEnter` and `DeadState::OnEnter`: from first person, fake first person; the
/// bleedout state takes the third-person zoom.
pub fn bleedout_enter(previous: CameraType, ctx: &Ctx<'_>) {
    let (Some(third), Some(bleedout)) = (third(ctx), util::family(ctx.camera, CameraState::Bleedout)) else { return };
    if previous == CameraType::FirstPerson {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
        skyrim::store(&bleedout.freeRotation.x, 0.0);
    }
    util::copy_zoom(third, bleedout);
}

/// The `OnEnter` of furniture, crafting, the transitions and the werewolf: from first person,
/// fake first person, through a forced switch to third person for crafting and the werewolf.
pub fn zoom_in_from_first_person(previous: CameraType, ctx: &Ctx<'_>, force_third: bool) {
    if previous != CameraType::FirstPerson {
        return;
    }
    if force_third {
        ctx.camera.force_third_person();
    }
    if let Some(third) = third(ctx) {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
    }
}

/// `FurnitureState::OnExit`: fake first person stays while an animation, sit or sleep plays.
pub fn furniture_exit(m: &mut Modifier, ctx: &Ctx<'_>) {
    m.sim_first_person = matches!(ctx.body.action, ActionType::Animation | ActionType::Sit | ActionType::Sleep);
}

/// `HorseState::OnEnter` and `DragonState::OnEnter`: full pitch range, and the mount state takes
/// the third-person zoom.
pub fn mount_enter(ctx: &Ctx<'_>, state: CameraState) {
    super::manager::raise_pitch_limits(state == CameraState::Horse);
    if let (Some(third), Some(mount)) = (third(ctx), util::family(ctx.camera, state)) {
        util::copy_zoom(third, mount);
    }
}

/// `HorseMountingState::OnEnter` and `DragonMountingState::OnEnter`: from first person, fake
/// first person facing ahead.
pub fn mounting_enter(previous: CameraType, ctx: &Ctx<'_>, state: CameraState) {
    if previous != CameraType::FirstPerson {
        return;
    }
    if let Some(third) = third(ctx) {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
    }
    if let Some(mount) = util::family(ctx.camera, state) {
        skyrim::store(&mount.freeRotation.x, 0.0);
    }
}

/// `HorseDismountingState::OnEnter`: the third-person state takes the horse's zoom.
pub fn horse_dismount_enter(ctx: &Ctx<'_>) {
    if let (Some(horse), Some(third)) = (util::family(ctx.camera, CameraState::Horse), third(ctx)) {
        util::copy_zoom(horse, third);
    }
}

/// `HorseDismountingState::OnExit` and `DragonDismountingState::OnExit`: the player's pitch back to level.
pub fn dismount_exit(m: &mut Modifier) {
    m.sim_first_person = false;
    if let Some(player) = util::player() {
        util::set_pitch(player, 0.0);
    }
}

/// `TransitionState::OnEnter`: after the horse dismount the transition goes back to the POV the
/// horse camera had (first person at the fake first-person zoom, else third), IC taking one
/// reference on the state as it points the transition at it; after the dragon dismount the
/// third-person state takes the dragon's zoom; from first person, fake first person.
pub fn transition_enter(previous: CameraType, ctx: &Ctx<'_>) {
    if previous == CameraType::HorseDismounting
        && let Some(horse) = util::family(ctx.camera, CameraState::Horse)
        && let Some(transition) = ctx.camera.state_object(CameraState::PlayerCameraTransition)
    {
        #[allow(clippy::float_cmp)]
        let to = if horse.targetZoom == FAKE_FIRST_PERSON_ZOOM {
            Some(CameraState::FirstPerson)
        } else if horse.targetZoom > -0.27 {
            Some(CameraState::ThirdPerson)
        } else {
            None
        };
        if let Some(to) = to.and_then(|s| ctx.camera.state_object(s)) {
            let transition = unsafe { &*core::ptr::from_ref(transition).cast::<sre::PlayerCameraTransitionState>() };
            skyrim::store(&transition.toState, core::ptr::from_ref(to).cast_mut());
            unsafe { core::sync::atomic::AtomicU32::from_ptr(core::ptr::from_ref(&to.refCount).cast_mut()) }.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        }
    }
    let Some(third) = third(ctx) else { return };
    if previous == CameraType::DragonDismounting
        && let Some(dragon) = util::family(ctx.camera, CameraState::Dragon)
    {
        util::copy_zoom(dragon, third);
    }
    if previous == CameraType::FirstPerson {
        util::set_zoom(third, FAKE_FIRST_PERSON_ZOOM);
    }
}

/// `TransitionState::OnExit`: fake first person stays after a transition from first person or
/// the dragon's mounting or dismounting while the zoom is in fake first person.
pub fn transition_exit(previous: CameraType, m: &mut Modifier, ctx: &Ctx<'_>) {
    let force = matches!(previous, CameraType::DragonDismounting | CameraType::DragonMounting | CameraType::FirstPerson);
    m.sim_first_person = force && third(ctx).is_some_and(|t| t.targetZoom <= -0.27);
}

/// `FREE_CAMERA_SPEED_MIN` and `_MAX`, and IC's sex scene camera's same bounds.
const SPEED_MIN: f32 = 1.0;
const SPEED_MAX: f32 = 60.0;
/// Menus during which the free camera's controls do nothing.
const CONTROL_MENUS: [&str; 5] = ["Journal Menu", "MapMenu", "InventoryMenu", "MagicMenu", "StatsMenu"];

/// `FreeState`, and with `sex` set `SexTFCState` (the free camera during a scene).
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Free {
    sex: bool,
    active: bool,
    fix_camera_bug: bool,
    /// The fly speed found on entry, which the reset key puts back; `None` until stored.
    original_speed: Option<f32>,
    rotate: f32,
    dirty: bool,
    modifier: bool,
    player_rotation: (f32, f32),
    player_translation: [f32; 3],
    handlers_off: bool,
}

fn set_free_handlers(enabled: bool) {
    for h in [util::Handler::Activate, util::Handler::Jump, util::Handler::ReadyWeapon, util::Handler::Sneak] {
        util::set_handler(h, enabled);
    }
}

impl Free {
    #[must_use]
    pub const fn new(sex: bool) -> Self {
        Self { sex, active: true, fix_camera_bug: false, original_speed: None, rotate: 0.0, dirty: false, modifier: false, player_rotation: (0.0, 0.0), player_translation: [0.0; 3], handlers_off: false }
    }

    pub fn enter(&mut self, previous: CameraType, ctx: &mut Ctx<'_>) {
        freeze_body();
        if !self.sex && previous >= CameraType::ThirdPerson {
            set_body_hidden(false, false);
        }
        if !ctx.profile.FreeCamera.bEnable {
            self.active = false;
            return;
        }
        if self.original_speed.is_none() {
            let speed = util::free_camera_speed();
            self.original_speed = Some(*speed);
            if self.sex {
                skyrim::store(speed, 5.0);
            }
        }
        if !self.sex && matches!(previous, CameraType::FirstPerson | CameraType::Unknown) {
            set_body_hidden(false, true);
            if let Some(player) = util::player() {
                self.player_rotation = (player.rotX, player.rotZ);
                self.player_translation = [player.posX, player.posY, player.posZ + 128.0];
                self.fix_camera_bug = true;
            }
        }
        skyrim::store(&ctx.camera.worldFOV, util::default_world_fov());
        if !self.sex {
            super::manager::reset_near();
            set_free_handlers(false);
            self.handlers_off = true;
        }
    }

    pub fn update(&mut self, m: &mut Modifier, ctx: &mut Ctx<'_>) {
        if !self.active {
            return;
        }
        if self.fix_camera_bug {
            self.fix_camera_bug = false;
            if let Some(free) = ctx.camera.state_object(CameraState::Free).and_then(TESCameraStateExt::as_free) {
                let (pitch, yaw) = self.player_rotation;
                skyrim::store(&free.pitch, pitch);
                skyrim::store(&free.yaw, yaw);
                let p = self.player_translation;
                skyrim::store(&free.position.x, p[0]);
                skyrim::store(&free.position.y, p[1]);
                skyrim::store(&free.position.z, p[2]);
            }
        }
        m.fov = ctx.camera.worldFOV;
        self.controls(ctx);
        if self.dirty
            && let Some(root) = ctx.camera.camera_root()
        {
            use crate::util::quaternion_util::Quat;
            let mut local = root.local_transform();
            let rotation = Quat::from_matrix(&local.rotation);
            let roll = Quat::from_axis_angle(rotation.forward(), self.rotate.to_radians());
            local.rotation = (roll * rotation).normalized().matrix();
            root.set_local_transform(local);
            root.update_transforms();
        }
    }

    /// The free camera's keys and wheel (IC's input callbacks, here once a frame): the roll keys
    /// held, the reset key, left shift held, and the wheel for the fly speed or, with shift, the FOV.
    fn controls(&mut self, ctx: &mut Ctx<'_>) {
        if !ctx.profile.FreeCamera.bEnable || CONTROL_MENUS.iter().any(|m| util::is_menu_open(m)) || ctx.camera.state() != Some(CameraState::Free) {
            return;
        }
        let input = ctx.input;
        if input.roll_left || input.roll_right || input.reset || input.modifier || !input.mouse.buttons.is_empty() {
            self.dirty = true;
        }
        let speed = util::free_camera_speed();
        let step = *speed / 6.66;
        if input.roll_left {
            self.rotate -= step;
        }
        if input.roll_right {
            self.rotate += step;
        }
        if input.reset {
            self.rotate = 0.0;
            if let Some(original) = self.original_speed {
                skyrim::store(speed, original);
            }
            ctx.notes.fov_reset = util::default_world_fov();
        }
        self.modifier = input.modifier;
        for &(button, down) in &input.mouse.buttons {
            if !down || !(8..=9).contains(&button) {
                continue;
            }
            if self.modifier {
                ctx.notes.fov_zoom += if button == 8 { -5.0 } else { 5.0 };
            } else {
                skyrim::store(speed, (*speed + if button == 8 { 1.0 } else { -1.0 }).clamp(SPEED_MIN, SPEED_MAX));
            }
        }
    }

    /// `OnExit`: the handlers back on. IC also pops the free camera input contexts the console's
    /// `tfc` leaks; popping one is not in the RE yet.
    pub fn exit(&mut self) {
        if self.handlers_off {
            set_free_handlers(true);
            self.handlers_off = false;
        }
    }

    /// For an unload: the handlers, and the fly speed found on entry.
    pub fn release(&mut self) {
        self.exit();
        if let Some(original) = self.original_speed {
            skyrim::store(util::free_camera_speed(), original);
        }
    }
}
