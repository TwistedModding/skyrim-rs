//! The player camera (subsystem passes 6 and 12): `TESCamera`, `PlayerCamera` with its 13 states
//! and the state stack, the state objects a mod reads and writes (`ThirdPersonState`,
//! `FirstPersonState`, `FreeCameraState`), the two per-frame hook points a camera mod places the
//! camera from, the field of view (world, hands, the animation offset) and the frame time.
//!
//! The camera is updated on the main thread once per frame (`Main::UpdatePlayer` →
//! `PlayerCharacter::UpdateCamera` → `PlayerCamera::Update` → `TESCamera::Update` → the state's
//! update slot); with the free camera `UpdateActorWithCamera` calls `TESCamera::Update` directly.
//! A mod that places the camera takes [`hook_state_step`] (the two `call` sites, inside the
//! camera's spin lock), lets the original run and then writes the camera root's local transform
//! and updates the node ([`crate::render::NiAVObject::update`]); the frame camera is a child of the
//! root and follows, and so do the culls, the shadow scene's eye position and the cached position.
//! [`hook_update`] is the entry of `TESCamera::Update` and also sees the map cameras.

use core::ffi::c_void;

use layout::v1_7_104::{classes, enums, fns, globals, vtables};

use crate::render::{NiAVObject, NiCamera, NiNode, Transform, vec3};

pub use enums::CameraState;
pub use vtables::TESCamera_vtable_Update as CameraUpdateFn;

/// `NiCamera::SetPerspectiveFromFov` (id 70643) as the two hands projection sites call it:
/// viewport (0, 1, 1, 0), `right = tan(fov / 2)` (scaled to a 16:9 frame unless `fov_spans_width`),
/// `top = right * height / width`, near and far as given, `far / near` into +0x170, then the node
/// update; `lod_adjust` sets +0x184 to `fov / lod_reference_fov`.
pub type SetPerspectiveFromFovFn = unsafe extern "C" fn(camera: *mut classes::NiCamera, fov_degrees: f32, near: f32, far: f32, width: u32, height: u32, fov_spans_width: bool, set_lod_adjust: bool, lod_reference_fov: f32);

type GetRotation = unsafe extern "C" fn(this: *mut classes::TESCameraState, out: *mut [f32; 4]);
type GetTranslation = unsafe extern "C" fn(this: *mut classes::TESCameraState, out: *mut [f32; 3]);

/// Offsets of the `call` sites a camera mod redirects, inside the functions named by their ids
/// (pass 12, `camera.md` sections 5 and 7 and the byte checks of `camera-verify.md`).
const PLAYER_UPDATE_STATE_STEP: usize = 0x1a6;
const FREE_CAMERA_STATE_STEP: usize = 0x69;
const SETUP_FIRST_PERSON_PROJECTION: usize = 0x106;
const DEPTH_PREPASS_PROJECTION: usize = 0x7fd;

pub(crate) struct Fns {
    player_camera: *const *const classes::PlayerCamera,
    frame_timer: *const u8,
    update: usize,
    player_update: usize,
    update_actor_with_camera: usize,
    setup_first_person_camera: usize,
    depth_prepass: usize,
    set_perspective: usize,
    fov_offset: *mut f32,
    first_person_view: *const classes::FirstPersonRenderContext,
    first_person_camera: *const *const classes::NiCamera,
    set_state: fns::TESCamera__SetState_fn,
    push_state: fns::PlayerCamera__PushState_fn,
    pop_state: fns::PlayerCamera__PopState_fn,
    force_first_person: fns::PlayerCamera__ForceFirstPerson_fn,
    force_third_person: fns::PlayerCamera__ForceThirdPerson_fn,
    toggle_pov_to_third_person: fns::PlayerCamera__TogglePOVToThirdPerson_fn,
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
                player_update: reloc.address(fns::PlayerCamera__Update)?,
                update_actor_with_camera: reloc.address(fns::UpdateActorWithCamera)?,
                setup_first_person_camera: reloc.address(fns::SetupFirstPersonCamera)?,
                depth_prepass: reloc.address(fns::RenderWorldDepthPrepass)?,
                set_perspective: reloc.address(fns::NiCamera__SetPerspectiveFromFov)?,
                fov_offset: reloc.address(globals::g_CameraFOVOffset)? as *mut f32,
                first_person_view: reloc.address(globals::g_FirstPersonViewParams)? as *const classes::FirstPersonRenderContext,
                first_person_camera: reloc.address(globals::g_FirstPersonCamera)? as *const *const classes::NiCamera,
                set_state: crate::resolve(reloc, fns::TESCamera__SetState)?,
                push_state: crate::resolve(reloc, fns::PlayerCamera__PushState)?,
                pop_state: crate::resolve(reloc, fns::PlayerCamera__PopState)?,
                force_first_person: crate::resolve(reloc, fns::PlayerCamera__ForceFirstPerson)?,
                force_third_person: crate::resolve(reloc, fns::PlayerCamera__ForceThirdPerson)?,
                toggle_pov_to_third_person: crate::resolve(reloc, fns::PlayerCamera__TogglePOVToThirdPerson)?,
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

/// The hands camera (`g_FirstPersonCamera`, id 415007): not under the camera root; placed each
/// first-person frame at the `Camera1st [Cam1]` node with the frame camera's rotation, with its
/// own near (`fNear1stPersonDistance`, 5) and the hands FOV.
#[must_use]
pub fn hands_camera() -> Option<&'static NiCamera> {
    unsafe { NiCamera::from_ptr(*fns().first_person_camera) }
}

/// `g_CameraFOVOffset` (id 414942): added to the world FOV by `ApplyWorldFOV` and to the hands
/// FOV by both projection sites. The first- and third-person updates write it every frame from
/// the `Camera Control` node's local z, so a mod's own value goes in after the state step.
#[must_use]
pub fn fov_offset() -> f32 {
    unsafe { fns().fov_offset.read_volatile() }
}

pub fn set_fov_offset(offset: f32) {
    unsafe { fns().fov_offset.write_volatile(offset) }
}

/// The hands FOV the projection sites read: the copy of [`PlayerCamera::first_person_fov`] that
/// `SetupWorldRenderContext` makes every frame (`g_FirstPersonViewParams` +0xc, id 415027).
#[must_use]
pub fn hands_fov() -> f32 {
    unsafe { (*fns().first_person_view).firstPersonFOV }
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

/// Two `call rel32` sites redirected to one detour; created disabled, enabled once stored.
pub struct CallSites<F: Copy> {
    sites: [hook::CallSite<F>; 2],
}

impl<F: Copy> CallSites<F> {
    unsafe fn create(sites: [usize; 2], target: usize, detour: F) -> Result<Self, hook::Error> {
        for site in sites {
            if hook::find_call(site, 5, target) != Some(site) {
                return Err(hook::Error::NotRel32(site));
            }
        }
        Ok(Self { sites: [unsafe { hook::CallSite::create(sites[0], detour) }?, unsafe { hook::CallSite::create(sites[1], detour) }?] })
    }

    /// Writes both sites. Call it after the hook is stored where the detour reads [`Self::original`].
    pub fn enable(&self) -> Result<(), hook::Error> {
        self.sites.iter().try_for_each(|s| unsafe { s.enable() })
    }

    /// The callee both sites reached before (the same function in the shipped game).
    pub fn original(&self) -> F {
        self.sites[0].original()
    }

    #[must_use]
    pub fn sites(&self) -> [usize; 2] {
        [self.sites[0].site(), self.sites[1].site()]
    }
}

/// The per-frame write point after the state update, as [`hook_state_step`] installs it.
pub type StateStepHook = CallSites<CameraUpdateFn>;

/// Redirects the two `call`s to `TESCamera::Update` that run the player camera's state step:
/// `PlayerCamera::Update` +0x1a6 (id 50784) and the free-camera path `UpdateActorWithCamera`
/// +0x69 (id 40450). The map menu reaches `TESCamera::Update` through a jump and is not seen.
/// The detour runs on the main thread inside the camera's spin lock, before `ApplyWorldFOV`, the
/// culls and `RenderWorld`; after the original it writes the root's local transform and updates
/// the node, the world FOV, the FOV offset or the near plane, and every consumer of the frame
/// follows. Both sites are checked for a `call` to `TESCamera::Update` before anything is written.
///
/// # Safety
/// Same as [`hook::CallSite::create`].
pub unsafe fn hook_state_step(detour: CameraUpdateFn) -> Result<StateStepHook, hook::Error> {
    let f = fns();
    unsafe { CallSites::create([f.player_update + PLAYER_UPDATE_STATE_STEP, f.update_actor_with_camera + FREE_CAMERA_STATE_STEP], f.update, detour) }
}

/// The two hands projection sites, as [`hook_hands_projection`] installs them.
pub type HandsProjectionHook = CallSites<SetPerspectiveFromFovFn>;

/// Redirects the two `call`s to `NiCamera::SetPerspectiveFromFov` that set the hands camera's
/// projection in every first-person frame: `SetupFirstPersonCamera` +0x106 (id 107123, before the
/// GPU snapshot) and `RenderWorldDepthPrepass` +0x7fd (id 107139, before the first-person draws).
/// Both are called with `fov = ` [`fov_offset`] ` + ` [`hands_fov`], the first-person near, the
/// world far and the screen size; a detour that changes the FOV must change it at both, or the
/// second resets the camera. Main thread.
///
/// # Safety
/// Same as [`hook::CallSite::create`].
pub unsafe fn hook_hands_projection(detour: SetPerspectiveFromFovFn) -> Result<HandsProjectionHook, hook::Error> {
    let f = fns();
    unsafe { CallSites::create([f.setup_first_person_camera + SETUP_FIRST_PERSON_PROJECTION, f.depth_prepass + DEPTH_PREPASS_PROJECTION], f.set_perspective, detour) }
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

    /// The engine's own first-person test: the current state is `cameraStates[0]`.
    #[must_use]
    pub fn is_first_person(&self) -> bool {
        self.state() == Some(CameraState::FirstPerson)
    }

    /// The engine's own free-camera test: the current state is `cameraStates[3]`.
    #[must_use]
    pub fn is_free_camera(&self) -> bool {
        self.state() == Some(CameraState::Free)
    }

    /// The third-person family the engine tests as `id > 8`: third person, horse, bleedout, dragon.
    #[must_use]
    pub fn in_third_person_family(&self) -> bool {
        self.current_state().and_then(TESCameraState::id).is_some_and(|id| id as u32 > 8)
    }

    /// The third-person state object (id 9), current or not.
    #[must_use]
    pub fn third_person(&self) -> Option<&ThirdPersonState> {
        self.state_object(CameraState::ThirdPerson)?.as_third_person()
    }

    /// The first-person state object (id 0), current or not.
    #[must_use]
    pub fn first_person(&self) -> Option<&FirstPersonState> {
        self.state_object(CameraState::FirstPerson)?.as_first_person()
    }

    /// States pushed under the current one (1 with the free camera or the tween menu, else 0).
    #[must_use]
    pub fn state_stack_count(&self) -> u32 {
        self.0.stateStackCount
    }

    /// The `ObjectRefHandle` the camera follows (`SetTarget`; the player, or a mount).
    #[must_use]
    pub fn target_handle(&self) -> u32 {
        self.0.targetHandle
    }

    /// Whether the collision holder exists: null at the main menu, set once the player's 3D has a character controller.
    #[must_use]
    pub fn has_camera_caster(&self) -> bool {
        !self.0.cameraCaster.is_null()
    }

    /// The handle of the actor the collision code is fading, or the invalid handle.
    #[must_use]
    pub fn faded_actor_handle(&self) -> u32 {
        self.0.fadedActorHandle
    }

    /// World FOV in degrees, applied every frame by `ApplyWorldFOV` (plus [`fov_offset`]).
    #[must_use]
    pub fn world_fov(&self) -> f32 {
        self.0.worldFOV
    }

    /// Writes the world FOV; the frame camera's frustum follows at `ApplyWorldFOV` (main loop, after
    /// the state step), so a write from the state-step detour reaches the culls and the draw of the same frame.
    pub fn set_world_fov(&self, degrees: f32) {
        unsafe { (&raw const self.0.worldFOV).cast_mut().write_volatile(degrees) }
    }

    /// First-person (hands) FOV in degrees, copied each frame into [`hands_fov`] by `SetupWorldRenderContext`.
    #[must_use]
    pub fn first_person_fov(&self) -> f32 {
        self.0.firstPersonFOV
    }

    /// Writes the hands FOV; both projection sites read the copy from the next `SetupWorldRenderContext`.
    pub fn set_first_person_fov(&self, degrees: f32) {
        unsafe { (&raw const self.0.firstPersonFOV).cast_mut().write_volatile(degrees) }
    }

    /// The camera position cached by `PlayerCamera::Update`: the root's world translation, in first person the `Cam1` node's.
    #[must_use]
    pub fn cached_position(&self) -> [f32; 3] {
        vec3(&self.0.cachedPosition)
    }

    /// Whether the cached position was computed this frame (cleared at the start of `PlayerCamera::Update`).
    #[must_use]
    pub fn cached_position_valid(&self) -> bool {
        self.0.cachedPositionValid != 0
    }

    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.0.yaw
    }

    #[must_use]
    pub fn auto_vanity_allowed(&self) -> bool {
        self.0.autoVanityAllowed != 0
    }

    /// Set by the bow zoom start handler while the zoom perk applies.
    #[must_use]
    pub fn bow_zoom_active(&self) -> bool {
        self.0.bowZoomActive != 0
    }

    /// `!weaponDrawn`: the third-person states enable free rotation only while this is set.
    #[must_use]
    pub fn allow_free_rotation(&self) -> bool {
        self.0.allowFreeRotation != 0
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

    /// `PlayerCamera::ForceFirstPerson` (id 50790): any state to first person when the target is the player. Main thread.
    pub fn force_first_person(&self) {
        unsafe { (fns().force_first_person)(self.as_player_ptr()) }
    }

    /// `PlayerCamera::ForceThirdPerson` (id 50796): to third person with the zoom at `fMinCurrentZoom`. Main thread.
    pub fn force_third_person(&self) {
        unsafe { (fns().force_third_person)(self.as_player_ptr()) }
    }

    /// `PlayerCamera::TogglePOVToThirdPerson` (id 50792): first to third person as the Toggle POV key
    /// does, zooming back out to the saved zoom; needs `CanSwitchPOV`. Main thread.
    pub fn toggle_pov_to_third_person(&self) {
        unsafe { (fns().toggle_pov_to_third_person)(self.as_player_ptr()) }
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

    /// Slot 4 `GetRotation`: the rotation the state computed, as a quaternion `[x, y, z, w]`.
    #[must_use]
    pub fn rotation(&self) -> [f32; 4] {
        let mut out = [0.0; 4];
        unsafe { Self::slot::<GetRotation>((*self.0.__vftable).slot_4)(self.as_ptr().cast_mut(), &raw mut out) };
        out
    }

    /// Slot 5 `GetTranslation`: the position the state computed (the root's world translation for the player states).
    #[must_use]
    pub fn translation(&self) -> [f32; 3] {
        let mut out = [0.0; 3];
        unsafe { Self::slot::<GetTranslation>((*self.0.__vftable).slot_5)(self.as_ptr().cast_mut(), &raw mut out) };
        out
    }

    unsafe fn slot<F: Copy>(entry: *const c_void) -> F {
        unsafe { core::mem::transmute_copy::<*const c_void, F>(&entry) }
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

fn write<T>(field: &T, value: T) {
    unsafe { core::ptr::from_ref(field).cast_mut().write_volatile(value) }
}

/// The third-person states (ids 8 and 9): chase position, shoulder offsets, zoom, yaw, free
/// rotation and the `animcam` byte. Written on the main thread, from the state-step detour at the latest.
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

    /// `Camera3rd [Cam3]` of the target's third-person 3D; its local z is the eye height the states add.
    #[must_use]
    pub fn cam3_node(&self) -> Option<&NiAVObject> {
        unsafe { NiAVObject::from_ptr(self.0.cam3Node) }
    }

    /// `Camera Control` of the third-person 3D: the animation graph moves it and the update copies its local z into [`fov_offset`].
    #[must_use]
    pub fn camera_control_node(&self) -> Option<&NiNode> {
        unsafe { NiNode::from_ptr(self.0.cameraControlNode) }
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

    /// Zoom in `[fMinCurrentZoom, 1]`: the distance is `fVanityModeMinDist + (fVanityModeMaxDist - fVanityModeMinDist) * zoom`.
    #[must_use]
    pub fn target_zoom(&self) -> f32 {
        self.0.targetZoom
    }

    /// The update chases the current zoom toward the target at `fMouseWheelZoomSpeed`.
    pub fn set_target_zoom(&self, zoom: f32) {
        write(&self.0.targetZoom, zoom);
    }

    #[must_use]
    pub fn current_zoom(&self) -> f32 {
        self.0.currentZoom
    }

    pub fn set_current_zoom(&self, zoom: f32) {
        write(&self.0.currentZoom, zoom);
    }

    /// The zoom before a Toggle POV zoom-in, restored by `TogglePOVToThirdPerson`.
    #[must_use]
    pub fn saved_zoom(&self) -> f32 {
        self.0.savedZoom
    }

    pub fn set_saved_zoom(&self, zoom: f32) {
        write(&self.0.savedZoom, zoom);
    }

    /// Extra distance while looking down (`fPitchZoomOutMaxDist`).
    #[must_use]
    pub fn pitch_zoom_offset(&self) -> f32 {
        self.0.pitchZoomOffset
    }

    /// Zoom change since Toggle POV went down; 0 at the release means "switch to first person".
    #[must_use]
    pub fn zoom_since_toggle_press(&self) -> f32 {
        self.0.zoomSinceTogglePress
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

    pub fn set_free_rotation(&self, rotation: [f32; 2]) {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&rotation[0].to_le_bytes());
        b[4..].copy_from_slice(&rotation[1].to_le_bytes());
        write(&self.0.freeRotation, b);
    }

    /// Set each frame by slot 13 while [`PlayerCamera::allow_free_rotation`] holds and the target stands still.
    #[must_use]
    pub fn free_rotation_enabled(&self) -> bool {
        self.0.freeRotationEnabled != 0
    }

    pub fn set_free_rotation_enabled(&self, enabled: bool) {
        write(&self.0.freeRotationEnabled, u8::from(enabled));
    }

    /// The Toggle POV zoom-in to first person is running.
    #[must_use]
    pub fn toggle_to_first_person(&self) -> bool {
        self.0.toggleToFirstPerson != 0
    }

    /// Console `animcam`: look input orbits and zooms the camera, the player is not turned or pitched.
    #[must_use]
    pub fn animator_cam(&self) -> bool {
        self.0.animatorCam != 0
    }

    pub fn set_animator_cam(&self, on: bool) {
        write(&self.0.animatorCam, u8::from(on));
    }

    /// Eye height, zoom distance and shoulder offsets apply (cleared on the animated camera, id 8).
    #[must_use]
    pub fn use_shoulder_offsets(&self) -> bool {
        self.0.useShoulderOffsets != 0
    }
}

/// The first-person state (id 0): the two camera nodes, the pitch and sitting values and the override bytes.
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

    /// `Camera1st [Cam1]` of the first-person skeleton: its world pose is the first-person camera pose.
    #[must_use]
    pub fn camera_node(&self) -> Option<&NiAVObject> {
        unsafe { NiAVObject::from_ptr(self.0.cameraNode) }
    }

    /// `Camera Control` of the first-person skeleton (its local z feeds [`fov_offset`] in first person).
    #[must_use]
    pub fn camera_control_node(&self) -> Option<&NiNode> {
        unsafe { NiNode::from_ptr(self.0.cameraControlNode) }
    }

    #[must_use]
    pub fn sitting_yaw(&self) -> f32 {
        self.0.sittingYaw
    }

    #[must_use]
    pub fn pitch_offset(&self) -> f32 {
        self.0.pitchOffset
    }

    /// Frames the update skips after a load (`SetFirstPersonSkipFrames`).
    #[must_use]
    pub fn skip_frames(&self) -> u32 {
        self.0.skipFrames
    }

    /// The spring offset is off (animation events through `SetFirstPersonCameraOverride`).
    #[must_use]
    pub fn camera_override(&self) -> bool {
        self.0.cameraOverride != 0
    }

    pub fn set_camera_override(&self, on: bool) {
        write(&self.0.cameraOverride, u8::from(on));
    }

    #[must_use]
    pub fn pitch_override(&self) -> bool {
        self.0.pitchOverride != 0
    }

    pub fn set_pitch_override(&self, on: bool) {
        write(&self.0.pitchOverride, u8::from(on));
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
