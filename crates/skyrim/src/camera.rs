//! The player camera (subsystem pass 6): `TESCamera`, `PlayerCamera` with its 13 states and the
//! state stack, the state objects a mod reads (`ThirdPersonState`, `FirstPersonState`,
//! `FreeCameraState`), the per-frame update a mod detours to place the camera, and the frame time.
//!
//! The camera is updated on the main thread once per frame (`Main::UpdatePlayer` →
//! `PlayerCharacter::UpdateCamera` → `PlayerCamera::Update` → `TESCamera::Update` → the state's
//! update slot); with the free camera only `TESCamera::Update` runs. A mod that sets the camera
//! detours `TESCamera::Update`, lets the original run and then writes the camera root's local
//! transform and updates the node ([`crate::render::NiAVObject::update`]); the frame camera is a
//! child of the root and follows.

use core::ffi::c_void;

use layout::v1_7_104::{classes, enums, fns, globals, vtables};

use crate::render::{NiAVObject, NiCamera, NiNode, Transform, vec3};

pub use enums::CameraState;
pub use vtables::TESCamera_vtable_Update as CameraUpdateFn;

pub(crate) struct Fns {
    player_camera: *const *const classes::PlayerCamera,
    frame_timer: *const u8,
    update: usize,
    set_state: fns::TESCamera__SetState_fn,
    push_state: fns::PlayerCamera__PushState_fn,
    pop_state: fns::PlayerCamera__PopState_fn,
    force_first_person: fns::PlayerCamera__ForceFirstPerson_fn,
    force_third_person: fns::PlayerCamera__ForceThirdPerson_fn,
    toggle_free_camera: fns::PlayerCamera__ToggleFreeCamera_fn,
    main_camera: fns::GetMainCamera_fn,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                player_camera: reloc.address(globals::g_PlayerCamera_ptr)? as *const *const classes::PlayerCamera,
                frame_timer: reloc.address(globals::g_FrameTimer)? as *const u8,
                update: reloc.address(fns::TESCamera__Update)?,
                set_state: crate::resolve(reloc, fns::TESCamera__SetState)?,
                push_state: crate::resolve(reloc, fns::PlayerCamera__PushState)?,
                pop_state: crate::resolve(reloc, fns::PlayerCamera__PopState)?,
                force_first_person: crate::resolve(reloc, fns::PlayerCamera__ForceFirstPerson)?,
                force_third_person: crate::resolve(reloc, fns::PlayerCamera__ForceThirdPerson)?,
                toggle_free_camera: crate::resolve(reloc, fns::PlayerCamera__ToggleFreeCamera)?,
                main_camera: crate::resolve(reloc, fns::GetMainCamera)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().camera
}

/// Frame time of the last frame in seconds, scaled by the time multiplier (`g_FrameTimer` +0x18).
#[must_use]
pub fn frame_delta() -> f32 {
    unsafe { fns().frame_timer.add(0x18).cast::<f32>().read_unaligned() }
}

/// Frame time of the last frame in seconds, unscaled (`g_FrameTimer` +0x1c).
#[must_use]
pub fn frame_delta_real() -> f32 {
    unsafe { fns().frame_timer.add(0x1c).cast::<f32>().read_unaligned() }
}

/// Milliseconds elapsed since the timer started (`g_FrameTimer` +0x20).
#[must_use]
pub fn frame_time_ms() -> u32 {
    unsafe { fns().frame_timer.add(0x20).cast::<u32>().read_unaligned() }
}

/// The frame camera (`GetMainCamera`, id 36609: the world scene graph's `NiCamera` at +0x128).
#[must_use]
pub fn main_camera() -> Option<&'static NiCamera> {
    unsafe { NiCamera::from_ptr((fns().main_camera)()) }
}

/// Creates, disabled, a detour on `TESCamera::Update` (id 33025), the per-frame camera update on
/// the main thread; store it, then [`hook::Hook::enable`]. The detour receives every `TESCamera`
/// (the player's, the map's), so compare `this` with [`PlayerCamera::get`].
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_update(detour: CameraUpdateFn) -> Result<hook::Hook<CameraUpdateFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().update, detour) }
}

/// The camera base: a root node, a current state and the enabled byte that gates the update.
#[repr(transparent)]
pub struct TESCamera(classes::TESCamera);

impl TESCamera {
    #[must_use]
    pub fn raw(&self) -> &classes::TESCamera {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const classes::TESCamera {
        &raw const self.0
    }

    /// # Safety
    /// `p` must be null or point to a live `TESCamera` (or derived object) that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::TESCamera) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    /// The node the states move; the frame camera is its child.
    #[must_use]
    pub fn camera_root(&self) -> Option<&NiNode> {
        unsafe { self.0.cameraRoot.cast::<NiNode>().as_ref() }
    }

    #[must_use]
    pub fn current_state(&self) -> Option<&TESCameraState> {
        unsafe { self.0.currentState.cast::<TESCameraState>().as_ref() }
    }

    #[must_use]
    pub fn current_state_ptr(&self) -> *const c_void {
        self.0.currentState
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.0.enabled != 0
    }

    /// `TESCamera::SetState` (id 33026): `End` on the current state, the swap, `Begin` on the new one. Main thread.
    pub fn set_state(&self, state: &TESCameraState) {
        unsafe { (fns().set_state)(self.as_ptr().cast_mut(), state.as_ptr().cast_mut()) }
    }
}

/// The player camera, one static object behind `g_PlayerCamera_ptr`.
#[repr(transparent)]
pub struct PlayerCamera(classes::PlayerCamera);

impl core::ops::Deref for PlayerCamera {
    type Target = TESCamera;

    fn deref(&self) -> &TESCamera {
        unsafe { &*(&raw const self.0).cast::<TESCamera>() }
    }
}

impl PlayerCamera {
    #[must_use]
    pub fn raw(&self) -> &classes::PlayerCamera {
        &self.0
    }

    /// The camera behind `g_PlayerCamera_ptr` (id 400802), `None` before it is constructed.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().player_camera).cast::<Self>().as_ref() }
    }

    /// Whether `camera` is the player camera (for a [`CameraUpdateFn`] detour).
    #[must_use]
    pub fn is(camera: *const classes::TESCamera) -> bool {
        Self::get().is_some_and(|p| core::ptr::eq(p.as_ptr(), camera))
    }

    /// The state object for `state`, one of the 13 at +0xb8 in `CameraState` order.
    #[must_use]
    pub fn state_object(&self, state: CameraState) -> Option<&TESCameraState> {
        let i = state as usize * 8;
        let p = usize::from_le_bytes(self.0.cameraStates[i..i + 8].try_into().unwrap()) as *const TESCameraState;
        unsafe { p.as_ref() }
    }

    /// Which of the 13 states is current, `None` when the pointer matches none.
    #[must_use]
    pub fn state(&self) -> Option<CameraState> {
        let current = self.current_state_ptr();
        (0..13u32).filter_map(CameraState::from_u32).find(|&s| self.state_object(s).is_some_and(|o| core::ptr::eq(o.as_ptr().cast(), current)))
    }

    /// States pushed under the current one (1 with the free camera or the tween menu, else 0).
    #[must_use]
    pub fn state_stack_count(&self) -> u32 {
        self.0.stateStackCount
    }

    #[must_use]
    pub fn world_fov(&self) -> f32 {
        self.0.worldFOV
    }

    #[must_use]
    pub fn first_person_fov(&self) -> f32 {
        self.0.firstPersonFOV
    }

    /// The camera position cached by `PlayerCamera::Update`, equal to the root's and the frame camera's world translation.
    #[must_use]
    pub fn cached_position(&self) -> [f32; 3] {
        vec3(&self.0.cachedPosition)
    }

    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.0.yaw
    }

    #[must_use]
    pub fn auto_vanity_allowed(&self) -> bool {
        self.0.autoVanityAllowed != 0
    }

    /// `PlayerCamera::PushState` (id 50880): pushes the current state and makes `state` current. Main thread.
    #[must_use]
    pub fn push_state(&self, state: CameraState) -> Option<&TESCameraState> {
        unsafe { (fns().push_state)(self.as_player_ptr(), state as u32).cast::<TESCameraState>().as_ref() }
    }

    /// `PlayerCamera::PopState` (id 50879): the state popped (first person when the stack is empty); the caller sets it.
    #[must_use]
    pub fn pop_state(&self) -> Option<&TESCameraState> {
        unsafe { (fns().pop_state)(self.as_player_ptr()).cast::<TESCameraState>().as_ref() }
    }

    /// `PlayerCamera::ForceFirstPerson` (id 50790). Main thread.
    pub fn force_first_person(&self) {
        unsafe { (fns().force_first_person)(self.as_player_ptr()) }
    }

    /// `PlayerCamera::ForceThirdPerson` (id 50796). Main thread.
    pub fn force_third_person(&self) {
        unsafe { (fns().force_third_person)(self.as_player_ptr()) }
    }

    /// `PlayerCamera::ToggleFreeCamera` (id 50809): the `tfc` console command. Main thread.
    pub fn toggle_free_camera(&self, freeze_world: bool) {
        unsafe { (fns().toggle_free_camera)(self.as_player_ptr(), freeze_world) }
    }

    fn as_player_ptr(&self) -> *mut classes::PlayerCamera {
        (&raw const self.0).cast_mut()
    }
}

/// A camera state: its id, owning camera and class; downcasts to the states with accessors.
#[repr(transparent)]
pub struct TESCameraState(classes::TESCameraState);

impl TESCameraState {
    #[must_use]
    pub fn raw(&self) -> &classes::TESCameraState {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const classes::TESCameraState {
        &raw const self.0
    }

    #[must_use]
    pub fn id(&self) -> Option<CameraState> {
        CameraState::from_u32(self.0.id)
    }

    #[must_use]
    pub fn camera(&self) -> Option<&TESCamera> {
        unsafe { TESCamera::from_ptr(self.0.camera) }
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    #[must_use]
    pub fn as_third_person(&self) -> Option<&ThirdPersonState> {
        matches!(self.id(), Some(CameraState::ThirdPerson | CameraState::ThirdPersonAlt)).then(|| unsafe { &*self.as_ptr().cast::<ThirdPersonState>() })
    }

    #[must_use]
    pub fn as_first_person(&self) -> Option<&FirstPersonState> {
        (self.id() == Some(CameraState::FirstPerson)).then(|| unsafe { &*self.as_ptr().cast::<FirstPersonState>() })
    }

    #[must_use]
    pub fn as_free(&self) -> Option<&FreeCameraState> {
        (self.id() == Some(CameraState::Free)).then(|| unsafe { &*self.as_ptr().cast::<FreeCameraState>() })
    }
}

/// The third-person states (ids 8 and 9): chase position, shoulder offsets, zoom, yaw and free rotation.
#[repr(transparent)]
pub struct ThirdPersonState(classes::ThirdPersonState);

impl core::ops::Deref for ThirdPersonState {
    type Target = TESCameraState;

    fn deref(&self) -> &TESCameraState {
        unsafe { &*(&raw const self.0).cast::<TESCameraState>() }
    }
}

impl ThirdPersonState {
    #[must_use]
    pub fn raw(&self) -> &classes::ThirdPersonState {
        &self.0
    }

    /// The camera position the state computed (the root's world translation).
    #[must_use]
    pub fn position(&self) -> [f32; 3] {
        vec3(&self.0.position)
    }

    /// Shoulder offset target and current value (the over-shoulder settings).
    #[must_use]
    pub fn target_offset(&self) -> [f32; 3] {
        vec3(&self.0.targetOffset)
    }

    #[must_use]
    pub fn current_offset(&self) -> [f32; 3] {
        vec3(&self.0.currentOffset)
    }

    #[must_use]
    pub fn target_zoom(&self) -> f32 {
        self.0.targetZoom
    }

    #[must_use]
    pub fn current_zoom(&self) -> f32 {
        self.0.currentZoom
    }

    #[must_use]
    pub fn target_yaw(&self) -> f32 {
        self.0.targetYaw
    }

    #[must_use]
    pub fn current_yaw(&self) -> f32 {
        self.0.currentYaw
    }

    /// Free rotation around the player `[yaw, pitch]`, applied while free look is on.
    #[must_use]
    pub fn free_rotation(&self) -> [f32; 2] {
        let b = &self.0.freeRotation;
        [f32::from_le_bytes([b[0], b[1], b[2], b[3]]), f32::from_le_bytes([b[4], b[5], b[6], b[7]])]
    }

    #[must_use]
    pub fn free_rotation_enabled(&self) -> bool {
        self.0.freeRotationEnabled != 0
    }
}

/// The first-person state (id 0): the camera node and the pitch and sitting values.
#[repr(transparent)]
pub struct FirstPersonState(classes::FirstPersonState);

impl core::ops::Deref for FirstPersonState {
    type Target = TESCameraState;

    fn deref(&self) -> &TESCameraState {
        unsafe { &*(&raw const self.0).cast::<TESCameraState>() }
    }
}

impl FirstPersonState {
    #[must_use]
    pub fn raw(&self) -> &classes::FirstPersonState {
        &self.0
    }

    #[must_use]
    pub fn camera_node(&self) -> Option<&NiAVObject> {
        unsafe { NiAVObject::from_ptr(self.0.cameraNode) }
    }

    #[must_use]
    pub fn sitting_yaw(&self) -> f32 {
        self.0.sittingYaw
    }

    #[must_use]
    pub fn pitch_offset(&self) -> f32 {
        self.0.pitchOffset
    }
}

/// The free camera (`tfc`, id 3): position and angles the camera root takes directly.
#[repr(transparent)]
pub struct FreeCameraState(classes::FreeCameraState);

impl core::ops::Deref for FreeCameraState {
    type Target = TESCameraState;

    fn deref(&self) -> &TESCameraState {
        unsafe { &*(&raw const self.0).cast::<TESCameraState>() }
    }
}

impl FreeCameraState {
    #[must_use]
    pub fn raw(&self) -> &classes::FreeCameraState {
        &self.0
    }

    /// The root's local translation.
    #[must_use]
    pub fn position(&self) -> [f32; 3] {
        vec3(&self.0.position)
    }

    #[must_use]
    pub fn pitch(&self) -> f32 {
        self.0.pitch
    }

    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.0.yaw
    }
}

/// The local transform of the camera root, what a state writes each frame.
#[must_use]
pub fn root_local(camera: &TESCamera) -> Option<Transform> {
    camera.camera_root().map(|r| r.local())
}
