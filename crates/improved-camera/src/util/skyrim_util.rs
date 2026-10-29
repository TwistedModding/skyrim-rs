//! `util/SkyrimUtil.h`: the engine queries IC repeats across its modules.

use skyrim::BSFixedString;
use skyrim::actors::{RefHandle, RefPtr};
use skyrim::prelude::*;
use skyrim::ui::Value;
use sre::{Actor, ActorState_flags1, CameraState, ControlGroupFlag, EventSourceHolder_MenuOpenClose, EventSourceHolder_User, NiAVObject, PlayerCamera, PlayerCharacter, PlayerControls, PlayerInputHandler, TESObjectREFR, ThirdPersonState};

/// The third-person state's target zoom IC parks the camera at for its fake first person.
pub const FAKE_FIRST_PERSON_ZOOM: f32 = -0.275;

/// `BGSBodyPartData` part index of the head (`camera.md` section 30, rated medium).
const HEAD_PART: u32 = 1;

pub fn player() -> Option<&'static PlayerCharacter> {
    PlayerCharacter::singleton()
}

pub fn camera() -> Option<&'static PlayerCamera> {
    PlayerCamera::singleton()
}

pub fn ui() -> Option<&'static EventSourceHolder_MenuOpenClose> {
    EventSourceHolder_MenuOpenClose::singleton()
}

pub fn is_menu_open(name: &str) -> bool {
    ui().is_some_and(|ui| ui.is_open(name))
}

pub fn game_is_paused() -> bool {
    ui().is_some_and(MenuManagerExt::is_paused)
}

/// `Get_DeltaWorldTime`: the scaled frame time.
pub fn delta_world_time() -> f32 {
    skyrim::camera::frame_delta()
}

/// `Get_DeltaFrameTime`: the unscaled frame time.
pub fn delta_frame_time() -> f32 {
    skyrim::camera::frame_delta_real()
}

/// `fDefaultWorldFOV:Display`, the world FOV IC puts back.
pub fn default_world_fov() -> f32 {
    unsafe { sre::ini_setting_f_default_world_fov_display_value().read() }
}

/// `Is_AnimCamera`: the third-person state is current with its animator camera on, which other
/// mods set while they own the shot.
pub fn is_anim_camera() -> bool {
    camera().is_some_and(|c| c.state() == Some(CameraState::ThirdPerson) && c.third_person().is_some_and(|s| s.animatorCam != 0))
}

/// `Is_BodyValid`: the player has its third-person 3D.
pub fn is_body_valid() -> bool {
    player().and_then(PlayerCharacterExt::third_person_root).is_some()
}

/// The zoom pair IC reads on the third-person, horse, bleedout and dragon states, which share
/// the third-person state's layout.
pub fn zoom_state(camera: &PlayerCamera, state: CameraState) -> Option<&ThirdPersonState> {
    const { assert!(core::mem::offset_of!(ThirdPersonState, targetZoom) == core::mem::offset_of!(sre::HorseCameraState, targetZoom)) };
    const { assert!(core::mem::offset_of!(ThirdPersonState, targetZoom) == core::mem::offset_of!(sre::DragonCameraState, targetZoom)) };
    const { assert!(core::mem::offset_of!(ThirdPersonState, targetZoom) == core::mem::offset_of!(sre::BleedoutCameraState, targetZoom)) };
    let object = camera.state_object(state)?;
    Some(unsafe { &*core::ptr::from_ref(object).cast::<ThirdPersonState>() })
}

/// `Get_ThirdPersonState()->targetZoomOffset`: the target zoom of the current state when it is
/// one of the third-person family.
pub fn current_third_person_zoom(camera: &PlayerCamera) -> Option<f32> {
    let state = camera.state().filter(|s| (CameraState::ThirdPerson as u32..=CameraState::Dragon as u32).contains(&(*s as u32)))?;
    zoom_state(camera, state).map(|s| s.targetZoom)
}

/// `Get_ThirdPersonState`: the current state when it is one of the third-person family.
pub fn current_family(camera: &PlayerCamera) -> Option<&ThirdPersonState> {
    let state = camera.state().filter(|s| (CameraState::ThirdPerson as u32..=CameraState::Dragon as u32).contains(&(*s as u32)))?;
    zoom_state(camera, state)
}

/// The target zoom of one of those states on the player camera.
pub fn zoom_of(state: CameraState) -> Option<f32> {
    camera().and_then(|c| zoom_state(c, state)).map(|s| s.targetZoom)
}

/// `Is_Fake_Firstperson`: a third-person family state parked at the fake first-person zoom.
pub fn is_fake_first_person() -> bool {
    let Some(camera) = camera() else { return false };
    let Some(state) = camera.state() else { return false };
    if matches!(state, CameraState::FirstPerson | CameraState::Free) {
        return false;
    }
    let family = if (CameraState::ThirdPerson as u32..=CameraState::Dragon as u32).contains(&(state as u32)) { state } else { CameraState::ThirdPerson };
    #[allow(clippy::float_cmp)]
    zoom_of(family).is_some_and(|z| z == FAKE_FIRST_PERSON_ZOOM)
}

/// `Is_Firstperson`: real or fake first person.
pub fn is_first_person() -> bool {
    camera().is_some_and(|c| c.state() == Some(CameraState::FirstPerson)) || is_fake_first_person()
}

pub fn is_free_camera() -> bool {
    camera().is_some_and(|c| c.state() == Some(CameraState::Free))
}

/// `Get_BodyPart_Node(actor, kHead)`.
pub fn head_node(player: &PlayerCharacter) -> Option<&'static NiAVObject> {
    player.get_body_part_node(HEAD_PART).map(|node| &**node)
}

pub fn object_by_name(root: &NiAVObject, name: &str) -> Option<&'static NiAVObject> {
    root.get_object_by_name(BSFixedString::new(name).raw())
}

pub fn graph_bool(player: &PlayerCharacter, name: &str) -> bool {
    player.as_i_animation_graph_manager_holder().variable_bool(&BSFixedString::new(name)).unwrap_or(false)
}

pub fn has_movement_flag(player: &PlayerCharacter, flag: ActorState_flags1) -> bool {
    ActorState_flags1(player.as_actor_state().flags1).contains(flag)
}

/// The weapon state, bits 5-7 of `ActorState::flags2`: 3 is drawn (`actors.md`).
pub fn weapon_state(actor: &Actor) -> u32 {
    (actor.as_actor_state().flags2 >> 5) & 7
}

/// `Is_Weapon_Drawn`: IC also counts want-to-draw and drawing, which are not in the RE yet; drawn only.
pub fn is_weapon_drawn(actor: &Actor) -> bool {
    weapon_state(actor) == 3
}

/// `Get_BodyPart_Node(actor, kHead)` for any actor.
pub fn actor_head(actor: &Actor) -> Option<&'static NiAVObject> {
    actor.get_body_part_node(HEAD_PART).map(|node| &**node)
}

/// `Is_Sitting` and `Is_Interacting`: the sit/sleep state is not in the RE yet (request IC3), so
/// this is whether the actor occupies furniture it sits or lies on (`GetFurnitureAnimType`: 1 sit,
/// 2 lie, 4 lean).
pub fn is_seated(actor: &Actor) -> bool {
    matches!(actor.get_furniture_anim_type(), 1 | 2)
}

/// `Get_CameraTarget_Actor`: the actor the camera follows when it is not the player. The handle is
/// resolved again each call, as every engine reader does (RE `camera.md`, request IC2 B).
pub fn camera_target() -> Option<RefPtr> {
    let target = RefHandle(camera()?.targetHandle).lookup()?;
    let player: &TESObjectREFR = player()?;
    (!core::ptr::eq(core::ptr::from_ref::<TESObjectREFR>(&target), player) && actor_of(&target).is_some()).then_some(target)
}

/// Writes a reference's pitch (`data.angle.x`); the reference's fields are unaligned.
pub fn set_pitch(reference: &TESObjectREFR, pitch: f32) {
    unsafe { core::ptr::addr_of!(reference.rotX).cast_mut().write_volatile(pitch) };
}

pub fn actor_of(reference: &TESObjectREFR) -> Option<&Actor> {
    reference.cast::<Actor>()
}

/// Points the camera at `actor` (`PlayerCamera::SetTarget`), or back at the player. Main thread,
/// outside `PlayerCamera::Update`.
pub fn set_camera_target(actor: Option<&Actor>) {
    let (Some(camera), Some(player)) = (camera(), player()) else { return };
    camera.set_target(Some(actor.map_or(&***player, |a| &**a)));
}

/// The third-person family state object for `state` (horse, bleedout and dragon share its layout).
pub fn family(camera: &PlayerCamera, state: CameraState) -> Option<&ThirdPersonState> {
    zoom_state(camera, state)
}

/// Sets a family state's target and current zoom.
pub fn set_zoom(state: &ThirdPersonState, zoom: f32) {
    skyrim::store(&state.targetZoom, zoom);
    skyrim::store(&state.currentZoom, zoom);
}

/// Copies the current, target and saved zoom from one family state to another.
pub fn copy_zoom(from: &ThirdPersonState, to: &ThirdPersonState) {
    skyrim::store(&to.currentZoom, from.currentZoom);
    skyrim::store(&to.targetZoom, from.targetZoom);
    skyrim::store(&to.savedZoom, from.savedZoom);
}

fn controls() -> Option<&'static EventSourceHolder_User> {
    EventSourceHolder_User::singleton()
}

/// Whether the control map's `POVSwitch` group is enabled.
pub fn pov_switch() -> bool {
    controls().is_some_and(|c| c.enabledControls & ControlGroupFlag::POVSwitch as u32 != 0)
}

/// Enables or disables the control map's `POVSwitch` group, as IC writes it.
pub fn set_pov_switch(enabled: bool) {
    if let Some(c) = controls() {
        let bit = ControlGroupFlag::POVSwitch as u32;
        skyrim::store(&c.enabledControls, if enabled { c.enabledControls | bit } else { c.enabledControls & !bit });
    }
}

/// The player-controls handlers IC switches, by their enabled byte (+8; RE `camera.md`, request IC2 C).
#[derive(Clone, Copy, Debug)]
pub enum Handler {
    Activate,
    Jump,
    ReadyWeapon,
    Sneak,
    AttackBlock,
    Look,
}

pub fn set_handler(handler: Handler, enabled: bool) {
    let Some(c) = PlayerControls::singleton() else { return };
    let pointer: *const PlayerInputHandler = match handler {
        Handler::Activate => c.activateHandler.cast(),
        Handler::Jump => c.jumpHandler.cast(),
        Handler::ReadyWeapon => c.readyWeaponHandler.cast(),
        Handler::Sneak => c.sneakHandler.cast(),
        Handler::AttackBlock => c.attackBlockHandler.cast(),
        Handler::Look => c.lookHandler.cast(),
    };
    if let Some(h) = unsafe { pointer.as_ref() } {
        skyrim::store(&h.enabled, u8::from(enabled));
    }
}

fn setting_f32(setting: *mut core::ffi::c_void) -> &'static f32 {
    unsafe { &*skyrim::render::setting_value::<f32>(setting) }
}

/// `fMinCurrentZoom:Camera`.
pub fn min_current_zoom() -> &'static f32 {
    setting_f32(sre::ini_setting_f_min_current_zoom_camera())
}

/// `fFreeCameraTranslationSpeed:Camera`.
pub fn free_camera_speed() -> &'static f32 {
    setting_f32(sre::ini_setting_f_free_camera_translation_speed_camera())
}

/// `fAutoVanityModeDelay:Camera`.
pub fn auto_vanity_delay() -> &'static f32 {
    setting_f32(sre::ini_setting_f_auto_vanity_mode_delay_camera())
}

/// The mount pitch limits IC raises: `fHorseMaxUpwardPitch`, `fMountedMaxLookingDown`,
/// `fDragonMaxUpwardPitch`, `fFlyingMountedMaxLookingDown`.
pub fn mount_pitch_limits() -> [&'static f32; 4] {
    unsafe {
        [
            &*sre::game_setting_f_horse_max_upward_pitch_value(),
            &*sre::game_setting_f_mounted_max_looking_down_value(),
            &*sre::game_setting_f_dragon_max_upward_pitch_value(),
            &*sre::game_setting_f_flying_mounted_max_looking_down_value(),
        ]
    }
}

/// The HUD's crosshair and stealth meter (`Display_Crosshair`): `SetCrosshairTarget` with
/// `bShowCrosshair` (argument 5 counted from 0, RE `camera.md` request IC2 G) and the stealth
/// meter's `_visible`.
pub fn show_crosshair(show: bool) {
    let Some(movie) = ui().and_then(MenuManagerExt::hud).and_then(|hud| unsafe { hud.movie.as_ref() }) else { return };
    if let Some(meter) = movie.variable("_root.HUDMovieBaseInstance.StealthMeterInstance") {
        let _ = meter.set_member("_visible", &Value::boolean(show));
    }
    let no = || Value::boolean(false);
    let args = [no(), Value::string(c""), no(), no(), no(), Value::boolean(show), Value::undefined(), Value::undefined(), Value::undefined(), Value::undefined()];
    movie.invoke("_root.HUDMovieBaseInstance.SetCrosshairTarget", &args);
}
