//! `util/SkyrimUtil.h`: the engine queries IC repeats across its modules.

use skyrim::BSFixedString;
use skyrim::prelude::*;
use sre::{ActorState_flags1, CameraState, EventSourceHolder_MenuOpenClose, NiAVObject, PlayerCamera, PlayerCharacter, ThirdPersonState};

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
pub fn weapon_state(player: &PlayerCharacter) -> u32 {
    (player.as_actor_state().flags2 >> 5) & 7
}
