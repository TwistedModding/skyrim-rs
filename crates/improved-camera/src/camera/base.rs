//! `camera/base/*`: the states of the engine's own player camera states. `CameraFirstPerson`,
//! `CameraThirdPerson`, `CameraVanity`, `CameraVATS`, `CameraFree`, `CameraFurniture`,
//! `CameraTransition` (with the beast and lich transitions), `CameraTweenMenu`, `CameraHorse`,
//! `CameraBleedout` (bleedout and dead) and `CameraDragon`; `CameraIronSight` and `CameraAnimated`
//! do nothing.

use framework::GamePad;
use skyrim::prelude::*;
use sre::{CameraState, InputContextEnum, PlayerCamera};

use super::manager::PitchLimits;
use super::state::{CameraType, Modifier};
use super::states::Ctx;
use super::util::interpolation::{Collision, Easing};
use super::util::orbit::{OrbitController, Pose};
use crate::body::state::action::ActionType;
use crate::body::state::hands::HandsType;
use crate::body::state::race::RaceType;
use crate::settings::Vanity as VanitySettings;
use crate::util::math_util::normalize_radian;
use crate::util::point::{self, M3, P3};
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

/// `VanityState::Enabled`: the idle camera is IC's only with the body on.
pub fn vanity_enabled(profile: &crate::settings::Profile) -> bool {
    profile.Vanity.bEnable && profile.General.bEnableBody
}

/// How far in front of an obstruction the idle camera stops.
const VANITY_PULLBACK: f32 = 10.0;

/// The idle orbit's camera position around `subject` at bearing `yaw` (radians, 0 along +Y).
#[must_use]
pub fn orbit_position(subject: P3, yaw: f32, v: &VanitySettings) -> P3 {
    point::add(subject, [yaw.sin() * v.fDistance, yaw.cos() * v.fDistance, v.fHeight])
}

/// The share of the ray from `subject` to the orbit position the camera keeps in front of a hit
/// at `hit`, `ray_length` long.
#[must_use]
pub fn pull_in(subject: P3, hit: P3, ray_length: f32) -> f32 {
    ((point::length(point::sub(hit, subject)) - VANITY_PULLBACK) / ray_length).clamp(0.0, 1.0)
}

/// `VanityState`: IC's idle camera. Once the engine's idle timer starts its vanity camera, IC
/// authors the shot: an orbit around the player's head at the profile's distance, height and
/// speed, pulled in front of anything solid and eased back, with the HUD hidden (`tm`) while it
/// runs. IC's focus on random nearby actors is left out (`docs/STATUS.md`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Vanity {
    /// The authored shot `CaptureCameraState` presents.
    pub shot: Option<(P3, M3)>,
    orbit_yaw: f32,
    collision: Collision,
    hid_menus: bool,
    owned: bool,
}

impl Vanity {
    pub fn enter(&mut self, ctx: &Ctx<'_>) {
        self.shot = None;
        self.owned = false;
        if vanity_enabled(ctx.profile) {
            self.begin_shot(ctx);
        }
    }

    /// `BeginShot`: the orbit picks up the camera's bearing around the player (behind the player
    /// when there is none) and the HUD is hidden.
    fn begin_shot(&mut self, ctx: &Ctx<'_>) {
        self.shot = None;
        self.collision.snap(1.0);
        if let (Some(player), Some(root)) = (util::player(), ctx.camera.camera_root()) {
            let to_camera = point::sub(root.world_transform().translation, util::head_position(player, 0.95));
            let (flat, length) = point::unitize([to_camera[0], to_camera[1], 0.0]);
            self.orbit_yaw = if length > 0.001 { flat[0].atan2(flat[1]) } else { normalize_radian(player.rotZ + core::f32::consts::PI) };
        }
        if ctx.profile.Vanity.bToggleMenus && util::menus_visible() {
            util::show_menus(false);
            self.hid_menus = true;
        }
        self.owned = true;
    }

    fn show_menus(&mut self) {
        if core::mem::take(&mut self.hid_menus) {
            util::show_menus(true);
        }
    }

    /// `OnUpdate`.
    pub fn update(&mut self, m: &mut Modifier, ctx: &Ctx<'_>) {
        let enabled = vanity_enabled(ctx.profile);
        m.fov = if enabled { ctx.profile.Vanity.fFOV } else { ctx.profile.FOV.fThirdPerson };
        if !enabled {
            self.show_menus();
            self.shot = None;
            self.owned = false;
            return;
        }
        if !self.owned {
            self.begin_shot(ctx);
        }
        if util::game_is_paused() {
            return;
        }
        let Some(player) = util::player().filter(|p| p.get3_d().is_some()) else {
            self.shot = None;
            return;
        };
        let v = &ctx.profile.Vanity;
        let dt = util::delta_frame_time();
        let subject = util::head_position(player, 0.95);
        self.orbit_yaw = normalize_radian(self.orbit_yaw + v.fOrbitSpeed.to_radians() * dt);
        let position = orbit_position(subject, self.orbit_yaw, v);
        let ray = point::sub(position, subject);
        let ray_length = point::length(ray);
        let hit = util::cast_ray(subject, position).filter(|h| util::is_solid_hit(h.layer, false) && ray_length > 1.0).map_or(1.0, |h| pull_in(subject, h.position, ray_length));
        let fraction = if !v.bCollisionSmoothing || self.shot.is_none() {
            self.collision.snap(hit);
            hit
        } else {
            let pull = Easing { method: v.iCollisionPullInMethod, duration: v.fCollisionPullInDuration };
            let recovery = Easing { method: v.iCollisionRecoveryMethod, duration: v.fCollisionRecoveryDuration };
            self.collision.update(hit, pull, recovery, dt)
        };
        let position = point::add(subject, point::scale(ray, fraction));
        self.shot = util::look_rotation(position, subject).map(|rotation| (position, rotation));
    }

    /// `OnExit`, and an unload: the HUD comes back.
    pub fn exit(&mut self) {
        self.show_menus();
        self.shot = None;
        self.owned = false;
    }
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

/// The `KeyboardEvent` of the VATS, dead, gathering and crafting states: the game's Toggle POV key
/// released after a hold `accept` takes flips the third-person target zoom between fake first
/// person and the close third person.
pub fn pov_zoom_toggle(ctx: &Ctx<'_>, accept: fn(f32) -> bool) {
    if !ctx.input.pov_released.is_some_and(accept) {
        return;
    }
    if let Some(third) = third(ctx) {
        #[allow(clippy::float_cmp)]
        let zoom = if third.targetZoom == FAKE_FIRST_PERSON_ZOOM { THIRD_PERSON_ZOOM } else { FAKE_FIRST_PERSON_ZOOM };
        skyrim::store(&third.targetZoom, zoom);
    }
}

/// A tap: released within 0.3 s.
pub fn pov_tap(held: f32) -> bool {
    held < 0.3
}

/// Crafting's long press: held between 1 and 10 s.
pub fn pov_long_press(held: f32) -> bool {
    held > 1.0 && held < 10.0
}

pub fn orbit_release(orbit: &mut OrbitController, camera: &PlayerCamera) {
    if let (Some(root), Some(pose)) = (camera.camera_root(), orbit.release()) {
        write_root(root, pose);
    }
}

pub fn write_root(root: &sre::NiAVObject, pose: Pose) {
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
    super::manager::raise_pitch_limits(if state == CameraState::Horse { PitchLimits::Horse } else { PitchLimits::Dragon });
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

/// The gamepad buttons IC's free camera reads (`XInput` masks).
const PAD_UP: u32 = 0x1;
const PAD_DOWN: u32 = 0x2;
const PAD_RIGHT_STICK: u32 = 0x80;
pub const PAD_LEFT_SHOULDER: u32 = 0x100;
pub const PAD_RIGHT_SHOULDER: u32 = 0x200;
const PAD_A: u32 = 0x1000;

/// The d-pad's fly speed steps of a frame, `true` up: IC steps on every event of the button, which
/// is once each frame it is down and once more at its release.
fn pad_steps(pad: &GamePad) -> Vec<bool> {
    [(PAD_UP, true), (PAD_DOWN, false)]
        .into_iter()
        .flat_map(|(mask, up)| {
            let held = usize::from(pad.held & mask != 0);
            let released = pad.buttons.iter().filter(|b| **b == (mask, false)).count();
            core::iter::repeat_n(up, held + released)
        })
        .collect()
}

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
    /// Left shift was down last frame: its release clears the modifier.
    shift: bool,
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
        Self { sex, active: true, fix_camera_bug: false, original_speed: None, rotate: 0.0, dirty: false, modifier: false, shift: false, player_rotation: (0.0, 0.0), player_translation: [0.0; 3], handlers_off: false }
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

    /// The free camera's keys, wheel and gamepad (IC's input callbacks, here once a frame): the roll
    /// keys or shoulders held, the reset key or A, left shift held or the right stick's click
    /// toggling the modifier, and the wheel or the d-pad's up and down for the fly speed or, with
    /// the modifier, the FOV. While they run, the game's menu keys and the shoulders are swallowed.
    fn controls(&mut self, ctx: &mut Ctx<'_>) {
        let live = ctx.profile.FreeCamera.bEnable && !CONTROL_MENUS.iter().any(|m| util::is_menu_open(m)) && ctx.camera.state() == Some(CameraState::Free);
        crate::swallow_free_camera_input(live);
        if !live {
            return;
        }
        let input = ctx.input;
        let pad = &input.pad;
        let roll_left = input.roll_left || pad.held & PAD_LEFT_SHOULDER != 0;
        let roll_right = input.roll_right || pad.held & PAD_RIGHT_SHOULDER != 0;
        let reset = input.reset || pad.buttons.contains(&(PAD_A, true));
        let speed_steps = pad_steps(pad);
        if roll_left || roll_right || reset || input.modifier || !input.mouse.buttons.is_empty() || !pad.buttons.is_empty() || !speed_steps.is_empty() {
            self.dirty = true;
        }
        let speed = util::free_camera_speed();
        let step = *speed / 6.66;
        if roll_left {
            self.rotate -= step;
        }
        if roll_right {
            self.rotate += step;
        }
        if reset {
            self.rotate = 0.0;
            if let Some(original) = self.original_speed {
                skyrim::store(speed, original);
            }
            ctx.notes.fov_reset = util::default_world_fov();
        }
        if input.modifier || self.shift {
            self.modifier = input.modifier;
        }
        self.shift = input.modifier;
        if pad.buttons.contains(&(PAD_RIGHT_STICK, true)) {
            self.modifier = !self.modifier;
        }
        let wheel = input.mouse.buttons.iter().filter(|(button, down)| *down && (8..=9).contains(button)).map(|(button, _)| *button == 8);
        for up in wheel.chain(speed_steps) {
            if self.modifier {
                ctx.notes.fov_zoom += if up { -5.0 } else { 5.0 };
            } else {
                skyrim::store(speed, (*speed + if up { 1.0 } else { -1.0 }).clamp(SPEED_MIN, SPEED_MAX));
            }
        }
    }

    /// `OnExit`: the handlers back on and the game's menu keys given back. Once the engine has
    /// left the free camera, the free camera input contexts the console's `tfc` leaves on the
    /// control map's stack go too (`PopInputContext` removes every occurrence).
    pub fn exit(&mut self) {
        crate::swallow_free_camera_input(false);
        if self.handlers_off {
            set_free_handlers(true);
            self.handlers_off = false;
        }
        if util::is_free_camera() {
            return;
        }
        let Some(map) = sre::EventSourceHolder_User::singleton() else { return };
        let leaked = map.context_stack().iter().filter(|c| **c == InputContextEnum::FreeCamera as u32).count();
        if leaked > 0 && map.pop_context(InputContextEnum::FreeCamera) {
            crate::log(&format!("<Camera> Removed {leaked} TFC input context(s) on free camera exit"));
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

#[cfg(test)]
mod tests {
    use framework::GamePad;

    use super::{orbit_position, pad_steps, pov_long_press, pov_tap, pull_in};
    use crate::settings::Vanity;

    #[test]
    fn the_idle_orbit_circles_the_head_and_stops_short_of_a_hit() {
        let v = Vanity::default();
        let p = orbit_position([10.0, 20.0, 100.0], 0.0, &v);
        assert_eq!(p, [10.0, 220.0, 135.0]);
        let p = orbit_position([0.0; 3], core::f32::consts::FRAC_PI_2, &v);
        assert!((p[0] - 200.0).abs() < 1e-3 && p[1].abs() < 1e-3);
        assert!((pull_in([0.0; 3], [0.0, 110.0, 0.0], 200.0) - 0.5).abs() < 1e-6);
        assert!(pull_in([0.0; 3], [0.0, 5.0, 0.0], 200.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_d_pad_steps_while_held_and_at_its_release() {
        let mut pad = GamePad { held: 0x1, buttons: vec![(0x1, true)], ..GamePad::default() };
        assert_eq!(pad_steps(&pad), vec![true]);
        pad = GamePad { held: 0x2, buttons: vec![(0x1, false)], ..GamePad::default() };
        assert_eq!(pad_steps(&pad), vec![true, false]);
        assert_eq!(pad_steps(&GamePad { held: 0x1000, ..GamePad::default() }), Vec::<bool>::new());
    }

    #[test]
    fn the_pov_key_toggles_on_a_tap_or_crafting_long_press() {
        assert!(pov_tap(0.1) && !pov_tap(0.3));
        assert!(!pov_long_press(0.5) && pov_long_press(2.0) && !pov_long_press(10.0));
    }
}
