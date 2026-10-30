//! `util/SkyrimUtil.h`: the engine queries IC repeats across its modules.

use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};

use skyrim::BSFixedString;
use skyrim::actors::{RefHandle, RefPtr};
use skyrim::physics::{self, Hit};
use skyrim::prelude::*;
use skyrim::ui::Value;
use sre::{Actor, ActorAttackState, ActorSitSleepState, ActorState_flags1, ActorWeaponState, BodyPartType, CameraState, CollisionLayer, ControlGroupFlag, DisablePOVType, EventSourceHolder_MenuOpenClose, EventSourceHolder_User, MenuTopicManager, NiAVObject, PlayerCamera, PlayerCharacter, PlayerControls, PlayerInputHandler, TESForm, TESObjectREFR, ThirdPersonState};

use crate::util::point::{self, M3, P3};

/// The third-person state's target zoom IC parks the camera at for its fake first person.
pub const FAKE_FIRST_PERSON_ZOOM: f32 = -0.275;

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
    is_engine_first_person() || is_fake_first_person()
}

/// `PlayerCamera::IsInFirstPerson`: the engine's first-person state is current.
pub fn is_engine_first_person() -> bool {
    camera().is_some_and(|c| c.state() == Some(CameraState::FirstPerson))
}

/// The player's `isInThirdPersonMode`, bit 0 of `viewFlags` (`camera.md` section 10).
pub fn third_person_flag(player: &PlayerCharacter) -> bool {
    player.viewFlags & 1 != 0
}

pub fn set_third_person_flag(player: &PlayerCharacter, third: bool) {
    skyrim::store(&player.viewFlags, (player.viewFlags & !1) | u8::from(third));
}

pub fn is_free_camera() -> bool {
    camera().is_some_and(|c| c.state() == Some(CameraState::Free))
}

pub fn object_by_name(root: &NiAVObject, name: &str) -> Option<&'static NiAVObject> {
    root.get_object_by_name(BSFixedString::new(name).raw())
}

/// The player's animation graphs: the third-person project first, the first-person arms second.
pub fn player_graphs(player: &PlayerCharacter) -> &[Option<&sre::BShkbAnimationGraph>] {
    player.current_process().and_then(|p| p.middle_high()).and_then(|m| m.animation_graph_manager()).map_or(&[], |m| m.graphs())
}

pub fn graph_bool(player: &PlayerCharacter, name: &str) -> bool {
    player.as_i_animation_graph_manager_holder().variable_bool(&BSFixedString::new(name)).unwrap_or(false)
}

pub fn has_movement_flag(player: &PlayerCharacter, flag: ActorState_flags1) -> bool {
    ActorState_flags1(player.as_actor_state().flags1).contains(flag)
}

/// `GetModel`: a form's model path relative to `Data\Meshes` (`GetFormModelPath`, `TESModel`+0x8; RE `camera.md` IC6 A).
pub fn model_path(form: &TESForm) -> Option<&'static str> {
    let path = sre::get_form_model_path(form);
    if path.is_null() { None } else { unsafe { core::ffi::CStr::from_ptr(path) }.to_str().ok() }
}

/// The weapon state, bits 5-7 of `ActorState::flags2`: 3 is drawn (`actors.md`).
pub fn weapon_state(actor: &Actor) -> u32 {
    (actor.as_actor_state().flags2 >> 5) & 7
}

/// `IsStaggering`: `ActorState::flags2` bit 13, set while a stagger plays (RE `camera.md` IC3 A).
pub fn is_staggered(actor: &Actor) -> bool {
    actor.as_actor_state().flags2 & 0x2000 != 0
}

/// The attack state, bits 28-31 of `ActorState::flags1` (`ActorAttackState`, RE `camera.md` IC3 A).
pub fn attack_state(actor: &Actor) -> Option<ActorAttackState> {
    ActorAttackState::from_u32(actor.as_actor_state().flags1 >> 28)
}

/// The hands a caster sits on, `Actor`+0x1a8 indexed by casting source.
#[derive(Clone, Copy, Debug)]
pub enum Hand {
    Left = 0,
    Right = 1,
}

/// `IsCasting(selectedSpells[hand])`: the hand's caster holds a spell it is casting (the caster's
/// +0x28, RE `camera.md` IC3 A).
pub fn is_casting(actor: &Actor, hand: Hand) -> bool {
    caster(actor, hand as usize).is_some_and(|c| c.field_28 != 0)
}

/// The actor's caster for casting source `source` (0 to 3), when it was created.
fn caster(actor: &Actor, source: usize) -> Option<&sre::ActorMagicCaster> {
    let at = source * 8;
    let pointer = usize::from_le_bytes(actor.magicCasters[at..at + 8].try_into().unwrap_or_default());
    unsafe { (pointer as *const sre::ActorMagicCaster).as_ref() }
}

/// `Reset_Magic_Nodes`: the player's four casters find their effect node again.
pub fn reset_magic_nodes() {
    let Some(player) = player() else { return };
    for source in 0..4 {
        if let Some(caster) = caster(player, source) {
            caster.reset_caster_nodes();
        }
    }
}

/// `HasMagicEffect`: an active effect on the actor has the base effect with form id `effect`.
pub fn has_magic_effect(actor: &Actor, effect: Option<u32>) -> bool {
    effect.is_some_and(|id| skyrim::papyrus::active_effects(actor).iter().any(|e| e.get_base_object().is_some_and(|base| base.formID == id)))
}

/// `ControlMap::IsMovementControlsEnabled`: both movement group bits, 0x401, set in the enabled
/// controls (`Game.IsMovementControlsEnabled`, RE `camera.md` IC3 A).
pub fn movement_controls_enabled() -> bool {
    const MOVEMENT: u32 = ControlGroupFlag::Movement as u32 | ControlGroupFlag::Movement_2 as u32;
    controls().is_none_or(|c| c.enabledControls & MOVEMENT == MOVEMENT)
}

/// `Is_Weapon_Drawn`: want to draw, drawing or drawn (`ActorWeaponState` 1-3).
pub fn is_weapon_drawn(actor: &Actor) -> bool {
    (ActorWeaponState::WantToDraw as u32..=ActorWeaponState::Drawn as u32).contains(&weapon_state(actor))
}

/// `Get_BodyPart_Node(actor, kHead)`.
pub fn actor_head(actor: &Actor) -> Option<&'static NiAVObject> {
    actor.get_body_part_node(BodyPartType::Head as u32).map(|node| &**node)
}

/// `Get_BodyPart_Node(actor, kEye)` (RE `camera.md` IC5 F).
pub fn actor_eye(actor: &Actor) -> Option<&'static NiAVObject> {
    actor.get_body_part_node(BodyPartType::Eye as u32).map(|node| &**node)
}

pub fn position(reference: &TESObjectREFR) -> P3 {
    [reference.posX, reference.posY, reference.posZ]
}

/// `Get_Head_Position`: the head node, else the feet raised by `ratio` of the actor's height,
/// taken as its 3D's bounding sphere diameter.
pub fn head_position(actor: &Actor, ratio: f32) -> P3 {
    if let Some(head) = actor_head(actor) {
        return head.world_transform().translation;
    }
    let height = actor.get3_d().map_or(0.0, |root| root.worldBound.radius * 2.0);
    let p = position(actor);
    [p[0], p[1], p[2] + height * ratio]
}

/// `IsFlying`: the actor's flying state (bits 18-20 of `ActorState::flags1`) is not `NotFlying`.
pub fn is_flying(actor: &Actor) -> bool {
    (actor.as_actor_state().flags1 >> 18) & 7 != sre::ActorFlyingState::NotFlying as u32
}

/// The collision group the camera's rays use, taken once from the world filter.
static PICK_GROUP: AtomicU16 = AtomicU16::new(0);

/// `CastRay`: a ray from `from` to `to` on the camera's pick layer (RE `camera.md` IC5 A), main
/// thread only.
pub fn cast_ray(from: P3, to: P3) -> Option<Hit> {
    let mut group = PICK_GROUP.load(Ordering::Relaxed);
    if group == 0 {
        group = physics::fresh_group()?;
        PICK_GROUP.store(group, Ordering::Relaxed);
    }
    physics::pick(from, to, physics::filter_info(CollisionLayer::CAMERAPICK, group))
}

/// `Is_Solid_Hit`: whether a hit on `layer` blocks a shot; bodies only when `actors_block`.
#[must_use]
pub fn is_solid_hit(layer: Option<CollisionLayer>, actors_block: bool) -> bool {
    use CollisionLayer as L;
    match layer {
        Some(L::BIPED | L::DEADBIP) => actors_block,
        Some(L::CHARCONTROLLER | L::SPELL | L::TRIGGER | L::ACTORZONE | L::SPELLEXPLOSION | L::UNIDENTIFIED) | None => false,
        Some(_) => true,
    }
}

/// `LookRotationMatrix`: the camera basis (right, forward, up columns) at `position` looking at
/// `target`; `None` when they coincide or the aim is straight up or down.
#[must_use]
pub fn look_rotation(position: P3, target: P3) -> Option<M3> {
    let (forward, length) = point::unitize(point::sub(target, position));
    if length < 0.001 {
        return None;
    }
    let (right, length) = point::unitize(point::cross(forward, [0.0, 0.0, 1.0]));
    if length < 0.001 {
        return None;
    }
    Some(point::from_columns(right, forward, point::cross(right, forward)))
}

/// `UI::IsShowingMenus`: the UI's `menusVisible` byte, which `tm` flips (RE `camera.md` IC5 E).
pub fn menus_visible() -> bool {
    ui().is_none_or(|ui| ui.menusVisible != 0)
}

/// `UI::ShowMenus`: writes `menusVisible`; main thread.
pub fn show_menus(visible: bool) {
    if let Some(ui) = ui() {
        skyrim::store(&ui.menusVisible, u8::from(visible));
    }
}

/// `DialogueState::GetTarget`: the topic manager's speaker, else its previous speaker
/// (`MenuTopicManager` +0x68 and +0x6c, RE `camera.md` IC5 B), as a loaded living actor; the
/// handles are resolved on every call.
pub fn dialogue_target() -> Option<RefPtr> {
    let topics = MenuTopicManager::singleton()?;
    let target = RefHandle(topics.field_68).lookup().or_else(|| RefHandle(topics.field_6C).lookup())?;
    let actor = actor_of(&target)?;
    (actor.get3_d().is_some() && !actor.is_dead(true)).then_some(target)
}

/// The sit/sleep state, bits 14-17 of `ActorState::flags1` (`ActorSitSleepState`, RE `camera.md` IC3 E).
pub fn sit_sleep_state(actor: &Actor) -> u32 {
    (actor.as_actor_state().flags1 >> 14) & 0xF
}

/// `Is_Entering_Interaction`: the player wants to sit or sleep, or is getting in.
pub fn is_entering_interaction() -> bool {
    use ActorSitSleepState as S;
    player().is_some_and(|p| matches!(S::from_u32(sit_sleep_state(p)), Some(S::WantToSit | S::EnteringSit | S::WantToSleep | S::EnteringBed)))
}

/// `Is_Leaving_Interaction`: the player is getting up from a seat or a bed.
pub fn is_leaving_interaction() -> bool {
    use ActorSitSleepState as S;
    player().is_some_and(|p| matches!(S::from_u32(sit_sleep_state(p)), Some(S::ExitingSit | S::ExitingBed)))
}

/// `Is_Interacting`: any sit/sleep phase.
pub fn is_interacting(actor: &Actor) -> bool {
    sit_sleep_state(actor) != ActorSitSleepState::None as u32
}

/// `Is_Sitting`: seated (riding included) or asleep.
pub fn is_sitting(actor: &Actor) -> bool {
    use ActorSitSleepState as S;
    matches!(S::from_u32(sit_sleep_state(actor)), Some(S::Sitting | S::Sleeping))
}

/// The idles `DrawArms` asks `Is_Animation_Playing` about. There is no engine query, so they are
/// followed through the player's graph (RE `camera.md` IC6 C): an idle starts when its event is
/// sent into the graph and ends at the graph's `IdleStop` or `tailMTIdle` tag. IC's `DrinkPotion`
/// is the idle `IdleDrinkPotion`, whose event has that name.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    None = 0,
    ReadElderScroll,
    DrinkPotion,
}

impl Idle {
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::ReadElderScroll,
            2 => Self::DrinkPotion,
            _ => Self::None,
        }
    }

    /// The idle playing once the graph has sent or received `event` (the pool folds case).
    #[must_use]
    pub fn next(self, event: &[u8]) -> Self {
        let is = |name: &str| event.eq_ignore_ascii_case(name.as_bytes());
        if is("IdleReadElderScroll") {
            Self::ReadElderScroll
        } else if is("IdleDrinkPotion") {
            Self::DrinkPotion
        } else if is("IdleStop") || is("tailMTIdle") {
            Self::None
        } else {
            self
        }
    }
}

static IDLE: AtomicU8 = AtomicU8::new(Idle::None as u8);

/// Follows an event sent into the player's graph or a tag it sent, on whichever thread did.
pub fn on_player_graph_event(event: &[u8]) {
    let _ = IDLE.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some(Idle::from_u8(v).next(event) as u8));
}

/// Forgets the idle, for a load that rebuilds the graph without its end tag.
pub fn clear_idle() {
    IDLE.store(Idle::None as u8, Ordering::Relaxed);
}

/// `Is_Animation_Playing`.
pub fn is_animation_playing(idle: Idle) -> bool {
    Idle::from_u8(IDLE.load(Ordering::Relaxed)) == idle
}

/// `povScriptMode`: a script disabled the POV switch (`PlayerControls::bDisablePOVChange[0]`,
/// RE `camera.md` IC3 D); only read.
pub fn pov_script_lock() -> bool {
    PlayerControls::singleton().is_some_and(|c| c.bDisablePOVChange[DisablePOVType::Script as usize] != 0)
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

/// Writes a reference's heading (`data.angle.z`).
pub fn set_heading(reference: &TESObjectREFR, heading: f32) {
    unsafe { core::ptr::addr_of!(reference.rotZ).cast_mut().write_volatile(heading) };
}

/// The process's head-tracking data, the object at `AIProcess`+0x10 (RE `camera.md` IC5 C).
fn head_track_data(actor: &Actor) -> Option<*mut core::ffi::c_void> {
    let process = actor.current_process()?;
    let pointer = usize::from_le_bytes(process.unknown_10[..8].try_into().ok()?);
    (pointer != 0).then_some(pointer as *mut core::ffi::c_void)
}

/// `SetHeadtrackTarget(slot, target)`: fills or clears (`None`) one head-track slot
/// (`HeadTrackData::SetSlot`).
pub fn set_head_track_slot(actor: &Actor, slot: sre::HeadTrackSlot, target: Option<&TESObjectREFR>) {
    if let Some(data) = head_track_data(actor) {
        unsafe { sre::head_track_data__set_slot(data, slot as i32, target.map_or(core::ptr::null_mut(), |t| core::ptr::from_ref(t).cast_mut())) };
    }
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

#[cfg(test)]
mod tests {
    use sre::CollisionLayer as L;

    use super::{Idle, is_solid_hit, look_rotation};

    fn after(events: &[&str]) -> Idle {
        events.iter().fold(Idle::None, |idle, e| idle.next(e.as_bytes()))
    }

    #[test]
    fn idles_follow_the_graph_as_run_ic6_recorded_them() {
        let potion = ["IdleDrinkPotion", "IdleOffsetStop", "AnimObjLoad", "AnimObjDraw"];
        assert_eq!(after(&potion), Idle::DrinkPotion);
        assert_eq!(after(&[&potion[..], &["IdleStop"]].concat()), Idle::None);
        let scroll = ["IdleReadElderScroll", "AnimObjectUnequip", "IdleOffsetStop"];
        assert_eq!(after(&scroll), Idle::ReadElderScroll);
        assert_eq!(after(&[&scroll[..], &["IdleStop", "AnimObjectUnequip", "MTState", "tailMTState", "HeadTrackingOn", "tailMTIdle"]].concat()), Idle::None);
        assert_eq!(after(&["IdleReadElderScroll", "tailmtidle"]), Idle::None, "the pool folds case");
        assert_eq!(after(&["idledrinkpotion"]), Idle::DrinkPotion);
        assert_eq!(after(&["IdleReadElderScroll", "IdleDrinkPotion"]), Idle::DrinkPotion, "the later idle");
        assert_eq!(after(&["FootLeft", "IdleStop", "AnimObjDraw", "DrinkPotion"]), Idle::None, "no idle without its event");
    }

    #[test]
    fn bodies_block_only_when_asked_and_triggers_never() {
        assert!(is_solid_hit(Some(L::STATIC), false));
        assert!(!is_solid_hit(Some(L::BIPED), false) && is_solid_hit(Some(L::DEADBIP), true));
        assert!(!is_solid_hit(Some(L::CHARCONTROLLER), true) && !is_solid_hit(Some(L::TRIGGER), true) && !is_solid_hit(None, true));
    }

    #[test]
    fn the_look_rotation_has_right_forward_up_columns() {
        let m = look_rotation([0.0; 3], [0.0, 10.0, 0.0]).unwrap();
        assert_eq!(m, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let m = look_rotation([0.0; 3], [10.0, 0.0, 0.0]).unwrap();
        assert_eq!([m[0][1], m[1][1], m[2][1]], [1.0, 0.0, 0.0]);
        assert_eq!([m[0][0], m[1][0], m[2][0]], [0.0, -1.0, 0.0]);
        assert!(look_rotation([0.0; 3], [0.0, 0.0, 10.0]).is_none() && look_rotation([1.0; 3], [1.0; 3]).is_none());
    }
}
