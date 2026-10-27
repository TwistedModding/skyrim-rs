//! The player camera (subsystem passes 6 and 12): the two per-frame hook points a camera mod
//! places the camera from, the frame time and the field of view the engine keeps as raw globals,
//! and what the generated `PlayerCamera` and `TESCameraState` methods leave `unsafe`: the 13 state
//! objects by [`CameraState`], the typed state views and the rotation and translation a state computed.
//!
//! The camera is updated on the main thread once per frame (`Main::UpdatePlayer` →
//! `PlayerCharacter::UpdateCamera` → `PlayerCamera::Update` → `TESCamera::Update` → the state's
//! update slot); with the free camera `UpdateActorWithCamera` calls `TESCamera::Update` directly.
//! A mod that places the camera takes [`hook_state_step`] (the two `call` sites, inside the
//! camera's spin lock), lets the original run and then writes the camera root's local transform
//! and updates the node (`NiAVObjectExt::set_local_transform` and `update_transforms`); the frame
//! camera is a child of the root and follows, and so do the culls, the shadow scene's eye position
//! and the cached position. [`hook_update`] is the entry of `TESCamera::Update` and also sees the
//! map cameras. Fields a state's update re-reads (zoom, FOV, override bytes) are written with
//! [`crate::store`] on the main thread, from the state-step detour at the latest.

use core::ffi::c_void;

use sre::raw::{fns, vtables};
use sre::{CameraState, FirstPersonState, FreeCameraState, NiPoint3, NiQuaternion, PlayerCamera, TESCameraState, ThirdPersonState};

pub use vtables::TESCamera_vtable_Update as CameraUpdateFn;

/// `NiCamera::SetPerspectiveFromFov` (id 70643) as the two hands projection sites call it:
/// viewport (0, 1, 1, 0), `right = tan(fov / 2)` (scaled to a 16:9 frame unless `fov_spans_width`),
/// `top = right * height / width`, near and far as given, `far / near` into +0x170, then the node
/// update; `lod_adjust` sets +0x184 to `fov / lod_reference_fov`.
pub type SetPerspectiveFromFovFn = unsafe extern "C" fn(camera: *mut sre::NiCamera, fov_degrees: f32, near: f32, far: f32, width: u32, height: u32, fov_spans_width: bool, set_lod_adjust: bool, lod_reference_fov: f32);

/// Offsets of the `call` sites a camera mod redirects, inside the functions named by their ids
/// (pass 12, `camera.md` sections 5 and 7 and the byte checks of `camera-verify.md`).
const PLAYER_UPDATE_STATE_STEP: usize = 0x1a6;
const FREE_CAMERA_STATE_STEP: usize = 0x69;
const SETUP_FIRST_PERSON_PROJECTION: usize = 0x106;
const DEPTH_PREPASS_PROJECTION: usize = 0x7fd;

fn address(id: u64) -> Result<usize, hook::Error> {
    crate::reloc().address(id)
}

fn frame_timer<T: Copy>(offset: usize) -> T {
    unsafe { sre::g_frame_timer().add(offset).cast::<T>().read_unaligned() }
}

/// Frame time of the last frame in seconds, scaled by the time multiplier (`g_FrameTimer` +0x18).
#[must_use]
pub fn frame_delta() -> f32 {
    frame_timer(0x18)
}

/// Frame time of the last frame in seconds, unscaled (`g_FrameTimer` +0x1c).
#[must_use]
pub fn frame_delta_real() -> f32 {
    frame_timer(0x1c)
}

/// Milliseconds elapsed since the timer started (`g_FrameTimer` +0x20).
#[must_use]
pub fn frame_time_ms() -> u32 {
    frame_timer(0x20)
}

/// `g_CameraFOVOffset` (id 414942): added to the world FOV by `ApplyWorldFOV` and to the hands
/// FOV by both projection sites. The first- and third-person updates write it every frame from
/// the `Camera Control` node's local z, so a mod's own value goes in after the state step.
#[must_use]
pub fn fov_offset() -> f32 {
    unsafe { sre::g_camera_fov_offset().read_volatile() }
}

pub fn set_fov_offset(offset: f32) {
    unsafe { sre::g_camera_fov_offset().write_volatile(offset) }
}

/// The hands FOV the projection sites read: the copy of `PlayerCamera::firstPersonFOV` that
/// `SetupWorldRenderContext` makes every frame (`g_FirstPersonViewParams` +0xc, id 415027).
#[must_use]
#[allow(clippy::cast_ptr_alignment)]
pub fn hands_fov() -> f32 {
    unsafe { (*sre::g_first_person_view_params().cast::<sre::FirstPersonRenderContext>()).firstPersonFOV }
}

/// Creates, disabled, a detour on `TESCamera::Update` (id 33025), the per-frame camera update on
/// the main thread; store it, then [`hook::Hook::enable`]. The detour receives every `TESCamera`
/// (the player's, the map's), so compare `this` with `PlayerCamera::singleton`.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_update(detour: CameraUpdateFn) -> Result<hook::Hook<CameraUpdateFn>, hook::Error> {
    unsafe { hook::Hook::create(address(fns::TESCamera__Update)?, detour) }
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
    let sites = [address(fns::PlayerCamera__Update)? + PLAYER_UPDATE_STATE_STEP, address(fns::UpdateActorWithCamera)? + FREE_CAMERA_STATE_STEP];
    unsafe { CallSites::create(sites, address(fns::TESCamera__Update)?, detour) }
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
    let sites = [address(fns::SetupFirstPersonCamera)? + SETUP_FIRST_PERSON_PROJECTION, address(fns::RenderWorldDepthPrepass)? + DEPTH_PREPASS_PROJECTION];
    unsafe { CallSites::create(sites, address(fns::NiCamera__SetPerspectiveFromFov)?, detour) }
}

pub trait PlayerCameraExt {
    /// The current state object, `None` before the camera has one.
    fn current_state(&self) -> Option<&TESCameraState>;
    /// The state object for `state`, one of the 13 at +0xb8 in `CameraState` order.
    fn state_object(&self, state: CameraState) -> Option<&TESCameraState>;
    /// Which of the 13 states is current, `None` when the pointer matches none; the engine's own
    /// first-person test is this being `FirstPerson`.
    fn state(&self) -> Option<CameraState>;
    /// The third-person state object (id 9), current or not.
    fn third_person(&self) -> Option<&ThirdPersonState>;
    /// The first-person state object (id 0), current or not.
    fn first_person(&self) -> Option<&FirstPersonState>;
}

impl PlayerCameraExt for PlayerCamera {
    fn current_state(&self) -> Option<&TESCameraState> {
        unsafe { self.currentState.cast::<TESCameraState>().as_ref() }
    }

    fn state_object(&self, state: CameraState) -> Option<&TESCameraState> {
        let i = state as usize * 8;
        let p = usize::from_le_bytes(self.cameraStates[i..i + 8].try_into().unwrap()) as *const TESCameraState;
        unsafe { p.as_ref() }
    }

    fn state(&self) -> Option<CameraState> {
        let current = self.currentState.cast_const();
        (0..13u32).filter_map(CameraState::from_u32).find(|&s| self.state_object(s).is_some_and(|o| core::ptr::eq(core::ptr::from_ref(o).cast::<c_void>(), current)))
    }

    fn third_person(&self) -> Option<&ThirdPersonState> {
        self.state_object(CameraState::ThirdPerson)?.as_third_person()
    }

    fn first_person(&self) -> Option<&FirstPersonState> {
        self.state_object(CameraState::FirstPerson)?.as_first_person()
    }
}

pub trait TESCameraStateExt {
    /// Slot 4 `GetRotation`: the rotation the state computed.
    fn rotation(&self) -> NiQuaternion;
    /// Slot 5 `GetTranslation`: the position the state computed (the root's world translation for the player states).
    fn translation(&self) -> NiPoint3;
    /// The state as a third-person state when its id is 8 or 9.
    fn as_third_person(&self) -> Option<&ThirdPersonState>;
    /// The state as the first-person state when its id is 0.
    fn as_first_person(&self) -> Option<&FirstPersonState>;
    /// The state as the free camera when its id is 3.
    fn as_free(&self) -> Option<&FreeCameraState>;
}

impl TESCameraStateExt for TESCameraState {
    fn rotation(&self) -> NiQuaternion {
        let mut out = NiQuaternion::default();
        self.get_rotation(&mut out);
        out
    }

    fn translation(&self) -> NiPoint3 {
        let mut out = NiPoint3::default();
        self.get_translation(&mut out);
        out
    }

    fn as_third_person(&self) -> Option<&ThirdPersonState> {
        matches!(CameraState::from_u32(self.id), Some(CameraState::ThirdPerson | CameraState::ThirdPersonAlt)).then(|| unsafe { &*core::ptr::from_ref(self).cast::<ThirdPersonState>() })
    }

    fn as_first_person(&self) -> Option<&FirstPersonState> {
        (CameraState::from_u32(self.id) == Some(CameraState::FirstPerson)).then(|| unsafe { &*core::ptr::from_ref(self).cast::<FirstPersonState>() })
    }

    fn as_free(&self) -> Option<&FreeCameraState> {
        (CameraState::from_u32(self.id) == Some(CameraState::Free)).then(|| unsafe { &*core::ptr::from_ref(self).cast::<FreeCameraState>() })
    }
}
