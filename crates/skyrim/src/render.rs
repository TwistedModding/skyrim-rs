//! Renderer (subsystem passes 5 and 6): the scene graph node with its name, flags, transforms and
//! tree edits, the world `ShadowSceneNode` with its light lists, the sun's
//! `BSShadowDirectionalLight` and its cascade records, the culling process a node is visited by,
//! the frame camera, shader properties with their flags, materials and texture paths, the render
//! passes, the accumulators and their modes, the render window with its swap chain, the sky, the
//! render frame counter and the hooks a visual mod needs (`NiAVObject::Cull`,
//! `PlayerCharacter::SetFirstPerson`, `RenderWorld`, `PresentFrame`). The player camera is
//! [`crate::camera`].
//!
//! Reads follow the pass's thread model: light lists, roots and flags are stable while the frame
//! culls (the main thread changes them between frames); a [`CullFn`] detour runs on the job
//! threads concurrently with the main camera cull and must stay lock-free and read-only apart from
//! the call it forwards. Offsets of the transforms, the bounding sphere, the children array and
//! the viewport are the byte layouts `render.json` gives for its `undefined` fields.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};
use std::fmt;

use core::ffi::{CStr, c_char};

use sre::raw::{fns, globals, vtables};

use crate::string::BSFixedString;

pub use sre::{AccumulatorRenderMode, BSShaderProperty_flags as ShaderFlags, NiAVObject_flags as NodeFlags, NiAlphaProperty_flags as AlphaFlags, ShaderMaterialType};
pub use fns::BSShaderAccumulator__RegisterDepthGeometry_fn as RegisterDepthGeometryFn;
pub use fns::PlayerCharacter__SetFirstPerson_fn as SetFirstPersonFn;
pub use fns::PresentFrame_fn as PresentFn;
pub use fns::ProcessCullObjectList_fn as ProcessCullObjectListFn;
pub use fns::RenderWorld_fn as RenderWorldFn;

/// `NiAVObject::UpdateNode` (id 70251, medium): slot 44 with the update data, then the parent's slot 61.
type NiAVObjectUpdateNode = unsafe extern "C" fn(this: *mut sre::NiAVObject, data: *mut c_void);

/// `NiAVObject::Cull(node, culler, orderedGroup)`: the third argument passes through `r8` into the
/// culler's slot 22 and is the index of the node inside a `BSOrderedNode` (0xffffffff elsewhere),
/// not a plane mask; a detour must pass it through unchanged.
pub type CullFn = fns::NiAVObject__Cull_fn;

/// `BSSceneGraph::SetFieldOfView` (id 106461, medium): stores the FOV and, unless it is unchanged
/// and `force` is clear, reframes the frame camera from the scene graph's near and far slots.
type SetFieldOfView = unsafe extern "C" fn(graph: *mut sre::BSSceneGraph, fov: f32, force: bool, camera: *mut sre::NiCamera, update_lod: bool);

/// `NiAVObject::flags` bit 0: no culling process visits the object.
pub const HIDDEN: u32 = 1;

/// `FrameState` +0xb8 and +0xc8: the array of `CameraMatrixEntry` (0x290 bytes each) and its
/// count (`graphics.md` section 2.2; the fields are not laid out in the ledger).
const FRAME_CAMERA_ENTRIES: usize = 0xb8;
const FRAME_CAMERA_ENTRY_COUNT: usize = 0xc8;
/// `ShaderPropertyLightData` +0x1c, the receiver mask behind `BSShaderProperty::lightData` (`graphics.md` section 4.3).
const RECEIVER_MASK: usize = 0x1c;
/// A `Setting`'s value, 8 bytes after its vtable (the ledger's `_value` rows).
const SETTING_VALUE: usize = 8;

pub(crate) struct Fns {
    scene_nodes: *const [*mut sre::ShadowSceneNode; 4],
    frame_counter: *const AtomicU32,
    scene_graph: *const *const sre::SceneGraph,
    window: *const *const sre::RenderWindow,
    sky: *const *const sre::Sky,
    accumulators: [*const *const sre::BSShaderAccumulator; 3],
    cull: usize,
    render_world: usize,
    present: usize,
    register_depth_geometry: usize,
    process_cull_object_list: usize,
    update_node: NiAVObjectUpdateNode,
    set_name: fns::NiObjectNET__SetName_fn,
    set_first_person: usize,
    set_field_of_view: SetFieldOfView,
    scene_near: fns::SceneGraph__GetNearDistance_fn,
    scene_far: fns::SceneGraph__GetFarDistance_fn,
    near_setting: *mut f32,
    first_person_near_setting: *const f32,
    depth_shift_near: *const f32,
    depth_shift_far: *const f32,
    frame_state: *const sre::FrameState,
    shadow_distance: *mut f32,
    shadow_distance_sq: *mut f32,
    shadow_map_resolution: *const u32,
    cascade_settings: [*const sre::Setting; 4],
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        Ok(Self {
            scene_nodes: reloc.address(globals::g_ShadowSceneNode)? as *const [*mut sre::ShadowSceneNode; 4],
            frame_counter: reloc.address(globals::g_RenderFrameCounter)? as *const AtomicU32,
            scene_graph: reloc.address(globals::g_SceneGraph)? as *const *const sre::SceneGraph,
            window: reloc.address(globals::g_CurrentRenderWindow)? as *const *const sre::RenderWindow,
            sky: reloc.address(globals::g_Sky_ptr)? as *const *const sre::Sky,
            accumulators: [
                reloc.address(globals::g_WorldDepthAccumulator)? as *const *const sre::BSShaderAccumulator,
                reloc.address(globals::g_WorldAccumulator)? as *const *const sre::BSShaderAccumulator,
                reloc.address(globals::g_FirstPersonAccumulator)? as *const *const sre::BSShaderAccumulator,
            ],
            cull: reloc.address(fns::NiAVObject__Cull)?,
            render_world: reloc.address(fns::RenderWorld)?,
            present: reloc.address(fns::PresentFrame)?,
            register_depth_geometry: reloc.address(fns::BSShaderAccumulator__RegisterDepthGeometry)?,
            process_cull_object_list: reloc.address(fns::ProcessCullObjectList)?,
            update_node: unsafe { crate::resolve(reloc, fns::NiAVObject__UpdateNode)? },
            set_name: unsafe { crate::resolve(reloc, fns::NiObjectNET__SetName)? },
            set_first_person: reloc.address(fns::PlayerCharacter__SetFirstPerson)?,
            set_field_of_view: unsafe { crate::resolve(reloc, fns::BSSceneGraph__SetFieldOfView)? },
            scene_near: unsafe { crate::resolve(reloc, fns::SceneGraph__GetNearDistance)? },
            scene_far: unsafe { crate::resolve(reloc, fns::SceneGraph__GetFarDistance)? },
            near_setting: reloc.address(globals::INISetting_fNearDistance_Display_value)? as *mut f32,
            first_person_near_setting: reloc.address(globals::INISetting_fNear1stPersonDistance_Display_value)? as *const f32,
            depth_shift_near: reloc.address(globals::g_DepthShiftNear)? as *const f32,
            depth_shift_far: reloc.address(globals::g_DepthShiftFar)? as *const f32,
            frame_state: reloc.address(globals::g_FrameState)? as *const sre::FrameState,
            shadow_distance: reloc.address(globals::g_ShadowDistance)? as *mut f32,
            shadow_distance_sq: reloc.address(globals::g_ShadowDistanceSq)? as *mut f32,
            shadow_map_resolution: reloc.address(globals::g_ShadowMapResolution)? as *const u32,
            cascade_settings: [
                reloc.address(globals::INIPrefSetting_fShadowDistance_Display)? as *const sre::Setting,
                reloc.address(globals::INISetting_fFirstSliceDistance_Display)? as *const sre::Setting,
                reloc.address(globals::INISetting_fSplitOverlap_Display)? as *const sre::Setting,
                reloc.address(globals::INIPrefSetting_iNumSplits_Display)? as *const sre::Setting,
            ],
        })
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().render
}

/// `g_RenderFrameCounter`: incremented by `PresentFrame`; skin palettes and pass lists are cached per value.
#[must_use]
pub fn frame_counter() -> u32 {
    unsafe { (*fns().frame_counter).load(Ordering::Relaxed) }
}

/// Creates, disabled, a detour on `NiAVObject::Cull` (id 70267), the per-node gate of every
/// scene walk: the main-camera culls, the sun's list walk, the local lights' map culls. It is not
/// the only caster gate (pass 12): the sun's cascades replay the walk's lists through
/// [`hook_process_cull_object_list`] without it, and the focus maps register subtrees without any
/// cull. Store the hook where the detour finds its trampoline, then [`hook::Hook::enable`] it:
/// the game culls on every frame from the main menu on, so a detour enabled before it can reach
/// the trampoline is called at once.
///
/// The detour runs on the job threads during the sun's list walk and the main camera culls and
/// on the frame thread during the cascade and local map culls, all concurrently, and during
/// loads while the scene is rebuilt; it must forward to [`hook::Hook::original`] for every node
/// it does not skip and must not dereference objects the main thread may be replacing.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_cull(detour: CullFn) -> Result<hook::Hook<CullFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().cull, detour) }
}

/// Creates, disabled, a detour on `RenderWorld` (id 36558): the world draw of a frame, on the main
/// thread; after the original returns the image space and the splatter have run and the UI is next.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_render_world(detour: RenderWorldFn) -> Result<hook::Hook<RenderWorldFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().render_world, detour) }
}

/// Creates, disabled, a detour on `PresentFrame` (id 77246): the swap chain present, called by the
/// frame, the Bink movie loop and the loading screen thread; `g_RenderFrameCounter` is incremented
/// inside it, so the detour runs on more than one thread.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_present(detour: PresentFn) -> Result<hook::Hook<PresentFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().present, detour) }
}

/// Creates, disabled, a detour on `PlayerCharacter::SetFirstPerson` (id 40476), called on the
/// main thread at every switch; store it, then [`hook::Hook::enable`].
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_set_first_person(detour: SetFirstPersonFn) -> Result<hook::Hook<SetFirstPersonFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().set_first_person, detour) }
}

/// Creates, disabled, a detour on `BSShaderAccumulator::RegisterDepthGeometry` (id 106576): every
/// geometry a depth or shadow accumulator accepted from a cull, before the property's slot 43
/// builds its passes; returning without calling the original drops the geometry from that map.
/// The shadow modes (0xd spot, 0xe cascade or focus, 0xf parabolic; [`BSShaderAccumulator::render_mode_raw`])
/// arrive on the frame thread, the depth pre-pass mode on workers, concurrently.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_register_depth_geometry(detour: RegisterDepthGeometryFn) -> Result<hook::Hook<RegisterDepthGeometryFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().register_depth_geometry, detour) }
}

/// Creates, disabled, a detour on `ProcessCullObjectList` (id 76561): the replay of a geometry
/// list into a culler, which the sun's cascades run on the frame thread over the lists its walk
/// built; with `skipHidden` an entry whose own hidden bit is set is dropped without `NiAVObject::Cull`.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_process_cull_object_list(detour: ProcessCullObjectListFn) -> Result<hook::Hook<ProcessCullObjectListFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().process_cull_object_list, detour) }
}

/// `fNearDistance:Display` (id 388779): the world near plane every frustum rebuild and every depth shift read.
#[must_use]
pub fn near_distance() -> f32 {
    unsafe { fns().near_setting.read_volatile() }
}

/// Writes the setting alone: effective from the next frame's first depth shift, the GPU one frame later.
pub fn set_near_distance(near: f32) {
    unsafe { fns().near_setting.write_volatile(near) }
}

/// `fNear1stPersonDistance:Display` (id 388782): the hands camera's near.
#[must_use]
pub fn first_person_near_distance() -> f32 {
    unsafe { fns().first_person_near_setting.read_volatile() }
}

/// The near `BeginDepthShift` last cached from the scene graph (`g_DepthShiftNear`, id 414949).
#[must_use]
pub fn depth_shift_near() -> f32 {
    unsafe { fns().depth_shift_near.read_volatile() }
}

/// The far `BeginDepthShift` last cached and `EndDepthShift` restores (`g_DepthShiftFar`, id 414948).
#[must_use]
pub fn depth_shift_far() -> f32 {
    unsafe { fns().depth_shift_far.read_volatile() }
}

/// The consistent near-plane write: the setting, then a forced `SetFieldOfView` with the live
/// world FOV plus offset, so the frame camera, the GPU snapshot, the depth shifts, the cascades,
/// the water stage and the fog all see `near` from this frame on. Main thread, before `RenderWorld`:
/// the state-step detour is the place. A write into the frame camera alone is undone by the
/// frame's first depth shift.
pub fn set_near_plane(near: f32) {
    set_near_distance(near);
    let fov = crate::camera::PlayerCamera::get().map_or(0.0, crate::camera::PlayerCamera::world_fov) + crate::camera::fov_offset();
    if let Some(graph) = SceneGraph::get() {
        graph.set_field_of_view(fov, true);
    }
}

/// `g_ShadowDistance` (id 415263): the live shadow distance, `fShadowDistance` outdoors and
/// 3000 indoors (`SetShadowDistanceForInterior` rewrites it at every cell change).
#[must_use]
pub fn shadow_distance() -> f32 {
    unsafe { fns().shadow_distance.read_volatile() }
}

/// Writes the live shadow distance and its square (the local lights' distance cull); the next
/// interior or exterior transition rewrites both from the settings.
pub fn set_shadow_distance(distance: f32) {
    unsafe {
        fns().shadow_distance.write_volatile(distance);
        fns().shadow_distance_sq.write_volatile(distance * distance);
    }
}

/// `g_ShadowMapResolution` (id 415268): the cascade and local map resolution cached at startup.
#[must_use]
pub fn shadow_map_resolution() -> u32 {
    unsafe { fns().shadow_map_resolution.read_volatile() }
}

/// The settings the sun's `UpdateForCamera` splits the shadow distance with (`graphics.md` section 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSettings {
    /// `fShadowDistance:Display`, the exterior distance and the split fraction's base.
    pub shadow_distance: f32,
    /// `fFirstSliceDistance:Display`: the end of cascade 0, at least 500 and at most the distance over the split count.
    pub first_slice_distance: f32,
    /// `fSplitOverlap:Display`: added to every split's far, twice to the next split's near.
    pub split_overlap: f32,
    /// `iNumSplits:Display`: the cascade count (2); the sun stores at most 3.
    pub splits: u32,
}

/// Reads the four cascade settings.
#[must_use]
pub fn cascade_settings() -> CascadeSettings {
    let s = fns().cascade_settings;
    CascadeSettings { shadow_distance: Setting::read(s[0]), first_slice_distance: Setting::read(s[1]), split_overlap: Setting::read(s[2]), splits: Setting::read(s[3]) }
}

/// Writes the four cascade settings; the sun reads them at its next `UpdateForCamera`, and the
/// shadow distance also at the next cell transition (see [`set_shadow_distance`] for the live value).
pub fn set_cascade_settings(settings: CascadeSettings) {
    let s = fns().cascade_settings;
    Setting::write(s[0], settings.shadow_distance);
    Setting::write(s[1], settings.first_slice_distance);
    Setting::write(s[2], settings.split_overlap);
    Setting::write(s[3], settings.splits);
}

struct Setting;

impl Setting {
    fn read<T: Copy>(setting: *const sre::Setting) -> T {
        unsafe { setting.cast::<u8>().add(SETTING_VALUE).cast::<T>().read_volatile() }
    }

    fn write<T: Copy>(setting: *const sre::Setting, value: T) {
        unsafe { setting.cast::<u8>().add(SETTING_VALUE).cast::<T>().cast_mut().write_volatile(value) }
    }
}

/// The world scene graph (`g_SceneGraph`, id 403513): the frame camera and the near and far slots.
#[repr(transparent)]
pub struct SceneGraph(sre::SceneGraph);

impl SceneGraph {
    #[must_use]
    pub fn raw(&self) -> &sre::SceneGraph {
        &self.0
    }

    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().scene_graph).cast::<Self>().as_ref() }
    }

    /// The frame camera (+0x128), what [`crate::camera::main_camera`] returns.
    #[must_use]
    pub fn frame_camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.field_128 as *const sre::NiCamera) }
    }

    /// Slot 63 (id 36844): the map menu's near override while it is set, else `fNearDistance:Display`.
    #[must_use]
    pub fn near(&self) -> f32 {
        unsafe { (fns().scene_near)(self.as_mut_ptr()) }
    }

    /// Slot 62 (id 36845): the far distance of the current scene (353840 with LOD outdoors).
    #[must_use]
    pub fn far(&self) -> f32 {
        unsafe { (fns().scene_far)(self.as_mut_ptr()) }
    }

    /// `BSSceneGraph::SetFieldOfView` (id 106461): stores `fov` and, when it changed or `force`
    /// is set, reframes the frame camera with the near and far slots and the render target size.
    /// Main thread; `ApplyWorldFOV` calls it with the world FOV plus offset twice a frame.
    pub fn set_field_of_view(&self, fov: f32, force: bool) {
        unsafe { (fns().set_field_of_view)(self.as_mut_ptr().cast(), fov, force, core::ptr::null_mut(), true) }
    }

    fn as_mut_ptr(&self) -> *mut sre::SceneGraph {
        (&raw const self.0).cast_mut()
    }
}

/// `g_FrameState` (id 411479): the frame's sizes, jitter and the GPU copies of the cameras.
#[repr(transparent)]
pub struct FrameState(sre::FrameState);

impl FrameState {
    #[must_use]
    pub fn raw(&self) -> &sre::FrameState {
        &self.0
    }

    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { fns().frame_state.cast::<Self>().as_ref() }
    }

    /// The full render target size and the swap chain size.
    #[must_use]
    pub fn sizes(&self) -> [u32; 4] {
        [self.0.screenWidth, self.0.screenHeight, self.0.backBufferWidth, self.0.backBufferHeight]
    }

    /// The dynamic resolution ratios the viewport is scaled by (1, 1 unless the console or `bEnableAutoDynamicResolution` moved them).
    #[must_use]
    pub fn dynamic_ratios(&self) -> [f32; 2] {
        [self.0.dynamicWidthRatio, self.0.dynamicHeightRatio]
    }

    /// The cameras snapshotted for the GPU this frame: the frame camera twice (jitter 0 and 1), the hands camera in first person.
    #[must_use]
    pub fn camera_entries(&self) -> &[CameraMatrixEntry] {
        let base = (&raw const self.0).cast::<u8>();
        let (data, count) = unsafe { (base.add(FRAME_CAMERA_ENTRIES).cast::<*const CameraMatrixEntry>().read_unaligned(), base.add(FRAME_CAMERA_ENTRY_COUNT).cast::<u32>().read_unaligned()) };
        if data.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(data, count as usize) } }
    }
}

/// One GPU camera snapshot (`FindOrAddEntry290`): the matrices every draw of that camera binds.
#[repr(transparent)]
pub struct CameraMatrixEntry(sre::CameraMatrixEntry);

impl CameraMatrixEntry {
    #[must_use]
    pub fn raw(&self) -> &sre::CameraMatrixEntry {
        &self.0
    }

    #[must_use]
    pub fn camera_ptr(&self) -> *const sre::NiCamera {
        self.0.camera
    }

    #[must_use]
    pub fn is_jittered(&self) -> bool {
        self.0.jittered != 0
    }

    /// The projection without the TAA jitter, row-major; standard Z (`[10] = f/(f-n)`, `[14] = -nf/(f-n)`, `[11] = 1`).
    #[must_use]
    pub fn projection_unjittered(&self) -> [f32; 16] {
        let b = &self.0.projectionUnjittered;
        core::array::from_fn(|i| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]))
    }

    /// The near plane the snapshot was built with, recovered from the projection.
    #[must_use]
    pub fn near(&self) -> f32 {
        let p = self.projection_unjittered();
        -p[14] / p[10]
    }
}

/// Three floats at `bytes[0..12]`.
pub(crate) fn vec3(bytes: &[u8]) -> [f32; 3] {
    let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    [f(0), f(4), f(8)]
}

fn u32_at(bytes: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
}

/// Rotation, translation and scale as `NiTransform` lays them out (rotation rows first).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
    pub scale: f32,
}

impl Transform {
    fn to_bytes(self) -> [u8; 0x34] {
        let mut b = [0u8; 0x34];
        let mut i = 0;
        for value in self.rotation.iter().flatten().chain(self.translation.iter()).chain(core::iter::once(&self.scale)) {
            b[i..i + 4].copy_from_slice(&value.to_le_bytes());
            i += 4;
        }
        b
    }

    fn from_bytes(b: &[u8; 0x34]) -> Self {
        Self {
            rotation: [vec3(&b[0..]), vec3(&b[12..]), vec3(&b[24..])],
            translation: vec3(&b[36..]),
            scale: f32::from_le_bytes([b[48], b[49], b[50], b[51]]),
        }
    }
}

/// A `#[repr(transparent)]` scene graph wrapper with `raw()`, `as_ptr()` and `Deref` to its base wrapper.
macro_rules! node_wrapper {
    ($(#[$doc:meta] $name:ident($layout:ident) : $base:ident) => {
        $(#[$doc]
        #[repr(transparent)]
        pub struct $name(sre::$layout);

        impl $name {
            #[must_use]
            pub fn raw(&self) -> &sre::$layout {
                &self.0
            }

            #[must_use]
            pub fn as_ptr(&self) -> *const sre::$layout {
                &raw const self.0
            }

            /// The wrapper over a live object.
            ///
            /// # Safety
            /// `p` must be null or point to a live object of this class (or a derived one) that outlives `'a`.
            #[must_use]
            pub unsafe fn from_ptr<'a>(p: *const sre::$layout) -> Option<&'a Self> {
                unsafe { p.cast::<Self>().as_ref() }
            }
        }

        impl core::ops::Deref for $name {
            type Target = $base;

            #[allow(clippy::cast_ptr_alignment)]
            fn deref(&self) -> &$base {
                unsafe { &*(&raw const self.0).cast::<$base>() }
            }
        }
    };
}

/// Any object of the scene graph: flags, parent, transforms and bounding sphere. Every class
/// below derives from it, and a [`CullFn`] detour receives one.
#[repr(transparent)]
pub struct NiAVObject(sre::NiAVObject);

impl NiAVObject {
    #[must_use]
    pub fn raw(&self) -> &sre::NiAVObject {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::NiAVObject {
        &raw const self.0
    }

    /// # Safety
    /// `p` must be null or point to a live `NiAVObject` (or derived object) that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const sre::NiAVObject) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    /// The object's class from its vtable pointer.
    #[must_use]
    pub fn class(&self) -> Option<&'static sre::raw::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    fn flags_atomic(&self) -> &AtomicU32 {
        unsafe { AtomicU32::from_ptr((&raw const self.0.flags).cast_mut()) }
    }

    /// The flag word: bit 0 hidden, bit 11 skips the frustum test, bit 12 needs the culler's
    /// +0x11e, bit 20 skips the node, bit 26 set when a culler accepted it (the rest unnamed).
    #[must_use]
    pub fn flags(&self) -> u32 {
        self.flags_atomic().load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.flags() & HIDDEN != 0
    }

    /// Sets or clears the hidden bit. The frame itself toggles this bit on the player's first-person
    /// root between its steps, so a mod changes it on the main thread between frames or on the
    /// player's third-person root only.
    pub fn set_hidden(&self, hidden: bool) {
        if hidden {
            self.flags_atomic().fetch_or(HIDDEN, Ordering::Relaxed);
        } else {
            self.flags_atomic().fetch_and(!HIDDEN, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn parent(&self) -> Option<&NiNode> {
        unsafe { self.0.parent.cast::<NiNode>().as_ref() }
    }

    /// The object's name (`NiObjectNET` +0x10), a view that does not touch the string's count.
    #[must_use]
    pub fn name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.name) }
    }

    /// Renames the object through `NiObjectNET::SetName` (id 70507). Main thread, between frames.
    pub fn set_name(&self, name: &BSFixedString) {
        unsafe { (fns().set_name)(self.as_ptr().cast_mut().cast(), core::ptr::from_ref(name).cast()) }
    }

    /// The object's index in its parent's children array.
    #[must_use]
    pub fn parent_index(&self) -> u32 {
        self.0.parentIndex
    }

    #[must_use]
    pub fn fade_amount(&self) -> f32 {
        self.0.fadeAmount
    }

    /// Recomputes the world transforms and bounds after a local transform change
    /// (`NiAVObject::UpdateNode`: slot 44 on the object, then the parent's slot 61). Main thread, between frames.
    pub fn update(&self) {
        let mut data = [0u64; 8];
        unsafe { (fns().update_node)(self.as_ptr().cast_mut(), data.as_mut_ptr().cast()) }
    }

    /// Slot 42 `GetObjectByName`: depth-first search by name pointer (the name must be an interned string).
    #[must_use]
    pub fn object_by_name(&self, name: &BSFixedString) -> Option<&NiAVObject> {
        let vtable = unsafe { &*self.0.__vftable };
        let find = vtable.GetObjectByName()?;
        unsafe { NiAVObject::from_ptr(find(self.as_ptr().cast_mut(), core::ptr::from_ref(name).cast())) }
    }

    #[must_use]
    pub fn local(&self) -> Transform {
        Transform::from_bytes(&self.0.local)
    }

    /// Writes the local transform. On the main thread between frames follow it with [`Self::update`];
    /// from a pose-write hook on a bone of the graph's own skeleton, call nothing (`crate::animation`).
    pub fn set_local(&self, local: Transform) {
        let bytes = local.to_bytes();
        unsafe { core::ptr::write_volatile((&raw const self.0.local).cast_mut(), bytes) }
    }

    #[must_use]
    pub fn world(&self) -> Transform {
        Transform::from_bytes(&self.0.world)
    }

    /// Bounding sphere centre and radius (`worldBound`).
    #[must_use]
    pub fn world_bound(&self) -> ([f32; 3], f32) {
        let b = &self.0.worldBound;
        (vec3(b), f32::from_le_bytes([b[12], b[13], b[14], b[15]]))
    }

    /// Whether `ancestor` is this object or one of its parents, looking at most `depth` levels up.
    #[must_use]
    pub fn is_under(&self, ancestor: &NiAVObject, depth: usize) -> bool {
        let mut node: &NiAVObject = self;
        for _ in 0..=depth {
            if core::ptr::eq(node, ancestor) {
                return true;
            }
            match node.parent() {
                Some(p) => node = p,
                None => return false,
            }
        }
        false
    }

    /// The object as a node when its class is one of the node classes the ledger lays out.
    #[must_use]
    pub fn as_node(&self) -> Option<&NiNode> {
        let id = self.class()?.id;
        NODE_VTABLES.contains(&id).then(|| unsafe { &*self.as_ptr().cast::<NiNode>() })
    }

    /// The object as geometry when its class is one of the `BSGeometry` classes.
    #[must_use]
    pub fn as_geometry(&self) -> Option<&BSGeometry> {
        let id = self.class()?.id;
        GEOMETRY_VTABLES.contains(&id).then(|| unsafe { &*self.as_ptr().cast::<BSGeometry>() })
    }
}

impl fmt::Debug for NiAVObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {:p} flags {:#x}", self.class().map_or("?", |c| c.class), self.as_ptr(), self.flags())
    }
}

const NODE_VTABLES: [u64; 15] = [
    vtables::NiNode_vtable::ID,
    vtables::BSFadeNode_vtable::ID,
    vtables::BSFaceGenNiNode_vtable::ID,
    vtables::BSLeafAnimNode_vtable::ID,
    vtables::BSTreeNode_vtable::ID,
    vtables::BSMultiBoundNode_vtable::ID,
    vtables::BSMultiBoundRoom_vtable::ID,
    vtables::BSOrderedNode_vtable::ID,
    vtables::BSBlastNode_vtable::ID,
    vtables::BSDamageStage_vtable::ID,
    vtables::BSValueNode_vtable::ID,
    vtables::NiBillboardNode_vtable::ID,
    vtables::NiSwitchNode_vtable::ID,
    vtables::BSPortalSharedNode_vtable::ID,
    vtables::ShadowSceneNode_vtable::ID,
];

const GEOMETRY_VTABLES: [u64; 7] = [
    vtables::BSGeometry_vtable::ID,
    vtables::BSTriShape_vtable::ID,
    vtables::BSDynamicTriShape_vtable::ID,
    vtables::BSSubIndexTriShape_vtable::ID,
    vtables::BSMeshLODTriShape_vtable::ID,
    vtables::BSLODTriShape_vtable::ID,
    vtables::BSLines_vtable::ID,
];

node_wrapper!(
    /// A node with children (`NiTObjectArray<NiPointer<NiAVObject>>` at +0x110).
    NiNode(NiNode): NiAVObject
);
node_wrapper!(
    /// A camera; the cullers' `camera` tells the passes of a frame apart.
    NiCamera(NiCamera): NiAVObject
);
node_wrapper!(
    /// Renderable geometry: properties, skin instance and type.
    BSGeometry(BSGeometry): NiAVObject
);

impl NiNode {
    /// The children, null entries included; data at +0x118 and the u16 slots-in-use count at +0x122.
    #[must_use]
    pub fn children(&self) -> &[Option<&NiAVObject>] {
        let b = &self.0.children;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let size = u16::from_le_bytes([b[18], b[19]]);
        unsafe { crate::forms::ptr_slice(data, u32::from(size)) }
    }

    /// Capacity, slots in use, non-null children and the grow step of the children array.
    #[must_use]
    pub fn children_counts(&self) -> [u16; 4] {
        let b = &self.0.children;
        let at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        [at(16), at(18), at(20), at(22)]
    }

    fn vtable(&self) -> &vtables::NiNode_vtable {
        unsafe { &*self.0.__vftable }
    }

    /// Slot 53 `AttachChild`: detaches `child` from its old parent and stores it here (in the first
    /// free slot when `first_free`); transforms are not updated, call [`NiAVObject::update`]. Main thread, between frames.
    pub fn attach_child(&self, child: &NiAVObject, first_free: bool) {
        if let Some(attach) = self.vtable().AttachChild() {
            unsafe { attach(self.as_ptr().cast_mut(), child.as_ptr().cast_mut(), first_free) }
        }
    }

    /// Slot 56 `DetachChild`: removes `child` and releases this node's reference. Main thread, between frames.
    pub fn detach_child(&self, child: &NiAVObject) {
        if let Some(detach) = self.vtable().DetachChild2() {
            unsafe { detach(self.as_ptr().cast_mut(), child.as_ptr().cast_mut()) }
        }
    }
}

/// The world scene graph behind `g_SceneGraph` (id 403513); the frame camera is its child at +0x128.
#[must_use]
pub fn world_scene_graph() -> Option<&'static NiNode> {
    unsafe { (*fns().scene_graph).cast::<NiNode>().as_ref() }
}

impl NiCamera {
    /// Frustum `[left, right, top, bottom, near, far]`; right is tan(fov / 2), top right times the aspect.
    #[must_use]
    pub fn frustum(&self) -> [f32; 6] {
        [self.0.frustumLeft, self.0.frustumRight, self.0.frustumTop, self.0.frustumBottom, self.0.frustumNear, self.0.frustumFar]
    }

    #[must_use]
    pub fn is_orthographic(&self) -> bool {
        self.0.frustumOrtho != 0
    }

    /// `far / near` as the last projection setter left it (+0x170); `SetFrustum` clamps near to keep it.
    #[must_use]
    pub fn far_near_ratio(&self) -> f32 {
        self.0.maxFarNearRatio
    }

    /// The LOD factor (+0x184): the FOV over `fDefaultFOV` for the frame camera, 1 at 80 degrees.
    #[must_use]
    pub fn lod_adjust(&self) -> f32 {
        self.0.lodAdjust
    }

    /// Viewport `[left, right, top, bottom]` in 0..1 (the full window is `[0, 1, 1, 0]`).
    #[must_use]
    pub fn viewport(&self) -> [f32; 4] {
        [self.0.portLeft, self.0.portRight, self.0.portTop, self.0.portBottom]
    }

    /// The world-to-clip matrix, row-major 4x4.
    #[must_use]
    pub fn world_to_camera(&self) -> [[f32; 4]; 4] {
        let b = &self.0.worldToCam;
        let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        core::array::from_fn(|r| core::array::from_fn(|c| f((r * 4 + c) * 4)))
    }
}

node_wrapper!(
    /// A fade node: an actor's or object's root with its fade and LOD values.
    BSFadeNode(BSFadeNode): NiNode
);

impl BSFadeNode {
    #[must_use]
    pub fn current_fade(&self) -> f32 {
        self.0.currentFade
    }

    #[must_use]
    pub fn target_fade(&self) -> f32 {
        self.0.targetFade
    }

    #[must_use]
    pub fn lod_distance(&self) -> f32 {
        self.0.lodDistance
    }
}

impl BSGeometry {
    #[must_use]
    pub fn skin_instance(&self) -> Option<&sre::NiSkinInstance> {
        unsafe { self.0.skinInstance.as_ref() }
    }

    #[must_use]
    pub fn shader_property(&self) -> Option<&BSShaderProperty> {
        unsafe { self.0.shaderProperty.cast::<BSShaderProperty>().as_ref() }
    }

    #[must_use]
    pub fn alpha_property(&self) -> Option<&NiAlphaProperty> {
        unsafe { self.0.alphaProperty.cast::<NiAlphaProperty>().as_ref() }
    }

    /// Geometry type byte, 3 for `BSTriShape`.
    #[must_use]
    pub fn geometry_type(&self) -> u8 {
        self.0.type_
    }
}

/// A light the scene node holds: the `NiLight` it wraps and its position.
#[repr(transparent)]
pub struct BSLight(sre::BSLight);

impl BSLight {
    #[must_use]
    pub fn raw(&self) -> &sre::BSLight {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::BSLight {
        &raw const self.0
    }

    fn vtable(&self) -> &vtables::BSLight_vtable {
        unsafe { &*self.0.__vftable }
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static sre::raw::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    /// The `NiLight` behind the entry, as a scene object.
    #[must_use]
    pub fn light(&self) -> Option<&NiAVObject> {
        unsafe { self.0.light.cast::<NiAVObject>().as_ref() }
    }

    /// World position copied from the light's translation at each update.
    #[must_use]
    pub fn position(&self) -> [f32; 3] {
        vec3(&self.0.position)
    }

    /// Slot 3: false on a plain `BSLight`, true on every `BSShadowLight`.
    #[must_use]
    pub fn is_shadow_light(&self) -> bool {
        self.vtable().IsShadowLight().is_some_and(|f| unsafe { f(self.as_ptr().cast_mut()) })
    }

    #[must_use]
    pub fn as_shadow_light(&self) -> Option<&BSShadowLight> {
        self.is_shadow_light().then(|| unsafe { &*self.as_ptr().cast::<BSShadowLight>() })
    }
}

/// A shadow-casting light: its cascade or map records and the focus maps.
#[repr(transparent)]
pub struct BSShadowLight(sre::BSShadowLight);

impl core::ops::Deref for BSShadowLight {
    type Target = BSLight;

    // A packed layout struct has alignment 1; the live object is 8-aligned.
    #[allow(clippy::cast_ptr_alignment)]
    fn deref(&self) -> &BSLight {
        unsafe { &*(&raw const self.0).cast::<BSLight>() }
    }
}

impl BSShadowLight {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShadowLight {
        &self.0
    }

    /// `iNumSplits` on the sun.
    #[must_use]
    pub fn shadow_map_count(&self) -> u32 {
        self.0.shadowMapCount
    }

    /// The `ShadowMapData` records (inline, 0xf0 each) of the light's maps.
    #[must_use]
    pub fn shadow_maps(&self) -> &[ShadowMapData] {
        let arr = unsafe { core::ptr::read_unaligned(&raw const self.0.shadowMaps) };
        if arr.data.is_null() || arr.size == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(arr.data.cast::<ShadowMapData>(), arr.size as usize) }
    }

    /// The four actor-focused map records (`iNumFocusShadow`).
    #[must_use]
    #[allow(clippy::cast_ptr_alignment)]
    pub fn focus_shadow_maps(&self) -> &[ShadowMapData; 4] {
        unsafe { &*(&raw const self.0.focusShadowMaps).cast::<[ShadowMapData; 4]>() }
    }

    /// Whether the focus maps are rendered this frame (1 in third person, 0 in first person).
    #[must_use]
    pub fn focus_shadows_active(&self) -> bool {
        self.0.focusShadowsActive != 0
    }

    #[must_use]
    pub fn bias_scale(&self) -> f32 {
        self.0.biasScale
    }

    #[must_use]
    pub fn frame_index(&self) -> u32 {
        self.0.frameIndex
    }

    /// Whether `camera` is one of this light's map or focus map cameras.
    #[must_use]
    pub fn owns_camera(&self, camera: *const sre::NiCamera) -> bool {
        self.shadow_maps().iter().chain(self.focus_shadow_maps()).any(|m| core::ptr::eq(m.raw().camera, camera))
    }
}

/// The sun: a shadow light with a direction, a light camera and the geometry list cullers.
#[repr(transparent)]
pub struct BSShadowDirectionalLight(sre::BSShadowDirectionalLight);

impl core::ops::Deref for BSShadowDirectionalLight {
    type Target = BSShadowLight;

    fn deref(&self) -> &BSShadowLight {
        unsafe { &*(&raw const self.0).cast::<BSShadowLight>() }
    }
}

impl BSShadowDirectionalLight {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShadowDirectionalLight {
        &self.0
    }

    #[must_use]
    pub fn light_direction(&self) -> [f32; 3] {
        vec3(&self.0.lightDirection)
    }

    /// The camera the sun's geometry list cullers walk the scene with.
    #[must_use]
    pub fn light_camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.lightCamera) }
    }

    /// Whether `camera` is the light camera, a cascade camera or a focus map camera of the sun.
    #[must_use]
    pub fn is_shadow_camera(&self, camera: *const sre::NiCamera) -> bool {
        core::ptr::eq(self.0.lightCamera, camera) || self.owns_camera(camera)
    }

    /// The near of each cascade (+0x598), `UpdateForCamera`'s split of the frame camera's near and the shadow distance.
    #[must_use]
    pub fn cascade_near(&self) -> [f32; 3] {
        vec3(&self.0.cascadeNear)
    }

    /// The far of each cascade (+0x5a4).
    #[must_use]
    pub fn cascade_far(&self) -> [f32; 3] {
        vec3(&self.0.cascadeFar)
    }

    /// The six geometry-list cullers of the sun's walk (+0x580), one per root list.
    #[must_use]
    pub fn geometry_list_cullers(&self) -> &[Option<&CullingProcess>] {
        let a = &self.0.geometryListCullers;
        if a.data.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(a.data.cast::<Option<&CullingProcess>>(), a.size as usize) }
    }
}

/// One shadow map of a light: camera, accumulator, depth target slice, viewport and culler.
#[repr(transparent)]
pub struct ShadowMapData(sre::ShadowMapData);

impl ShadowMapData {
    #[must_use]
    pub fn raw(&self) -> &sre::ShadowMapData {
        &self.0
    }

    #[must_use]
    pub fn camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.camera) }
    }

    /// Depth-stencil target id (2 cascades, 3 the 1/8 pre-pass, 4 focus and local maps), `None` between frames.
    #[must_use]
    pub fn depth_target(&self) -> Option<u32> {
        (self.0.depthTarget != u32::MAX).then_some(self.0.depthTarget)
    }

    #[must_use]
    pub fn slice(&self) -> u32 {
        self.0.slice
    }

    /// `[left, right, top, bottom]` in pixels.
    #[must_use]
    pub fn viewport(&self) -> [u32; 4] {
        let v = &self.0.viewport;
        [u32_at(v, 0), u32_at(v, 4), u32_at(v, 8), u32_at(v, 12)]
    }

    #[must_use]
    pub fn culler(&self) -> Option<&CullingProcess> {
        unsafe { self.0.culler.cast::<CullingProcess>().as_ref() }
    }

    #[must_use]
    pub fn accumulator(&self) -> Option<&sre::BSShaderAccumulator> {
        unsafe { self.0.accumulator.as_ref() }
    }
}

/// The culling process a [`CullFn`] detour receives: every culler of the frame is a
/// `BSCullingProcess` (or its geometry-list subclass), whose `camera` names the pass.
#[repr(transparent)]
pub struct CullingProcess(sre::BSCullingProcess);

impl CullingProcess {
    #[must_use]
    pub fn raw(&self) -> &sre::BSCullingProcess {
        &self.0
    }

    /// # Safety
    /// `p` must be null or point to a live culling process that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const sre::NiCullingProcess) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static sre::raw::Vtable> {
        unsafe { crate::class_of((&raw const self.0).cast()) }
    }

    #[must_use]
    pub fn camera_ptr(&self) -> *const sre::NiCamera {
        self.0.camera
    }

    #[must_use]
    pub fn camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.camera) }
    }

    #[must_use]
    pub fn accumulator(&self) -> Option<&sre::BSShaderAccumulator> {
        unsafe { self.0.accumulator.cast::<sre::BSShaderAccumulator>().as_ref() }
    }

    /// 1 and 2 accept without testing, 3 and 4 test planes and portals. Only valid on a
    /// `BSCullingProcess` or `BSGeometryListCullingProcess` (checked through [`Self::class`]).
    #[must_use]
    pub fn cull_mode(&self) -> Option<u32> {
        let id = self.class()?.id;
        (id == vtables::BSCullingProcess_vtable::ID || id == vtables::BSGeometryListCullingProcess_vtable::ID).then_some(self.0.cullMode)
    }

    /// Whether this culler walks the scene for a shadow map: its camera is the sun's light or
    /// cascade camera, a focus map camera, or a map camera of one of the scene's shadow lights.
    #[must_use]
    pub fn is_shadow_pass(&self) -> bool {
        let Some(scene) = ShadowSceneNode::world() else {
            return false;
        };
        let camera = self.camera_ptr();
        scene.sun_shadow_light().is_some_and(|sun| sun.is_shadow_camera(camera))
            || scene.shadow_lights().iter().flatten().any(|l| l.owns_camera(camera))
    }

    /// Whether this is one of the sun's geometry-list cullers (step 8b, a worker thread): the walk
    /// that decides the sun's caster set, whose lists the cascades replay without `NiAVObject::Cull`.
    #[must_use]
    pub fn is_sun_list_walk(&self) -> bool {
        self.class().is_some_and(|c| c.id == vtables::BSGeometryListCullingProcess_vtable::ID)
            && ShadowSceneNode::world().and_then(ShadowSceneNode::sun_shadow_light).is_some_and(|sun| core::ptr::eq(sun.raw().lightCamera, self.camera_ptr()))
    }

    /// Slot 22 `CullNode`: what `NiAVObject::Cull` does after its hidden test (the bound against
    /// the planes, then the node's `OnVisible`). A [`CullFn`] detour calls it to admit a node whose
    /// hidden bit would have stopped the walk, passing `ordered_group` through.
    pub fn cull_node(&self, node: &NiAVObject, ordered_group: u32) {
        let vtable = unsafe { &*self.0.__vftable };
        if let Some(cull) = vtable.CullNode() {
            unsafe { cull((&raw const self.0).cast_mut(), node.as_ptr().cast_mut(), ordered_group) }
        }
    }
}

/// A scene's lights, shadow lights and sun; `g_ShadowSceneNode[0]` is the world.
#[repr(transparent)]
pub struct ShadowSceneNode(sre::ShadowSceneNode);

impl ShadowSceneNode {
    #[must_use]
    pub fn raw(&self) -> &sre::ShadowSceneNode {
        &self.0
    }

    /// `g_ShadowSceneNode[index]` (id 390951), `None` while the slot is empty.
    #[must_use]
    pub fn get(index: usize) -> Option<&'static Self> {
        let slots = unsafe { &*fns().scene_nodes };
        unsafe { slots.get(index)?.cast::<Self>().as_ref() }
    }

    /// The world scene, `g_ShadowSceneNode[0]`.
    #[must_use]
    pub fn world() -> Option<&'static Self> {
        Self::get(0)
    }

    /// As a node (`ShadowSceneNode` derives from `NiNode`).
    #[must_use]
    #[allow(clippy::cast_ptr_alignment)]
    pub fn node(&self) -> &NiNode {
        unsafe { &*(&raw const self.0).cast::<NiNode>() }
    }

    /// Active lights that cast no shadow (`NiPointer<BSLight>` entries).
    #[must_use]
    pub fn lights(&self) -> &[Option<&BSLight>] {
        unsafe { crate::forms::ptr_slice(self.0.lights.data, self.0.lights.size) }
    }

    /// Shadow-casting lights other than the sun.
    #[must_use]
    pub fn shadow_lights(&self) -> &[Option<&BSShadowLight>] {
        unsafe { crate::forms::ptr_slice(self.0.shadowLights.data, self.0.shadowLights.size) }
    }

    #[must_use]
    pub fn sun_light(&self) -> Option<&BSLight> {
        unsafe { self.0.sunLight.cast::<BSLight>().as_ref() }
    }

    #[must_use]
    pub fn sun_shadow_light(&self) -> Option<&BSShadowDirectionalLight> {
        unsafe { self.0.sunShadowLight.cast::<BSShadowDirectionalLight>().as_ref() }
    }

    /// This node's index in `g_ShadowSceneNode`.
    #[must_use]
    pub fn scene_index(&self) -> u8 {
        self.0.sceneIndex
    }

    /// The camera position the frame copied in (`RenderWorld`).
    #[must_use]
    pub fn eye_position(&self) -> [f32; 3] {
        vec3(&self.0.eyePosition)
    }
}

/// A geometry's shader property: flags, alpha, the last pass state, the material and the pass list.
#[repr(transparent)]
pub struct BSShaderProperty(sre::BSShaderProperty);

impl BSShaderProperty {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShaderProperty {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::BSShaderProperty {
        &raw const self.0
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static sre::raw::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    /// The 64-bit shader flags (`ShaderFlags`: skinned, cast and receive shadows, decal, z test and write, ...).
    #[must_use]
    pub fn flags(&self) -> ShaderFlags {
        ShaderFlags(self.0.flags)
    }

    /// Material alpha times fade, recomputed each frame.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.0.alpha
    }

    /// `renderMode << 8 | shadow light mask` of the last pass that drew the geometry, 0x7fffffff when invalid.
    /// `mode << 8 | receiver mask` of the last main-view registration; `0x7fffffff` for a geometry never drawn there.
    #[must_use]
    pub fn last_render_pass_state(&self) -> u32 {
        self.0.lastRenderPassState
    }

    /// The shadow-map receiver mask (`lightData` +0x1c): bit `n` set while the geometry is
    /// registered into map `n`'s accumulator, cleared by the main view's registration after the
    /// lighting passes were built from it.
    #[must_use]
    pub fn receiver_mask(&self) -> Option<u32> {
        let data = self.0.lightData;
        (!data.is_null()).then(|| unsafe { data.cast::<u8>().add(RECEIVER_MASK).cast::<u32>().read_unaligned() })
    }

    #[must_use]
    pub fn material(&self) -> Option<&BSShaderMaterial> {
        unsafe { self.0.material.cast::<BSShaderMaterial>().as_ref() }
    }

    #[must_use]
    pub fn fade_node(&self) -> Option<&BSFadeNode> {
        unsafe { BSFadeNode::from_ptr(self.0.fadeNode) }
    }

    /// The property's colour pass list, built during the culls; empty between frames.
    #[must_use]
    pub fn render_passes(&self) -> Option<&RenderPass> {
        unsafe { self.0.renderPasses.cast::<RenderPass>().as_ref() }
    }
}

/// A material: type, feature and texture coordinate transform; the lighting material adds the texture set.
#[repr(transparent)]
pub struct BSShaderMaterial(sre::BSShaderMaterial);

impl BSShaderMaterial {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShaderMaterial {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::BSShaderMaterial {
        &raw const self.0
    }

    fn vtable(&self) -> &vtables::BSShaderMaterial_vtable {
        unsafe { &*self.0.__vftable }
    }

    /// Slot 7 `GetType`: 0 base, 1 effect, 2 lighting, 3 water.
    #[must_use]
    pub fn material_type(&self) -> Option<ShaderMaterialType> {
        let get = self.vtable().GetType()?;
        ShaderMaterialType::from_u32(unsafe { get(self.as_ptr().cast_mut()) })
    }

    /// Slot 6 `GetFeature`: the `LightingMaterialFeature` value as a number.
    #[must_use]
    pub fn feature(&self) -> Option<u32> {
        let get = self.vtable().GetFeature()?;
        Some(unsafe { get(self.as_ptr().cast_mut()) })
    }

    /// The lighting material's view when `material_type` is lighting.
    #[must_use]
    pub fn as_lighting(&self) -> Option<&BSLightingShaderMaterial> {
        (self.material_type() == Some(ShaderMaterialType::Lighting)).then(|| unsafe { &*self.as_ptr().cast::<BSLightingShaderMaterial>() })
    }
}

/// A lighting material: the texture set and the lighting parameters.
#[repr(transparent)]
pub struct BSLightingShaderMaterial(sre::BSLightingShaderMaterialBase);

impl core::ops::Deref for BSLightingShaderMaterial {
    type Target = BSShaderMaterial;

    fn deref(&self) -> &BSShaderMaterial {
        unsafe { &*(&raw const self.0).cast::<BSShaderMaterial>() }
    }
}

impl BSLightingShaderMaterial {
    #[must_use]
    pub fn raw(&self) -> &sre::BSLightingShaderMaterialBase {
        &self.0
    }

    #[must_use]
    pub fn texture_set(&self) -> Option<&BSShaderTextureSet> {
        unsafe { self.0.textureSet.cast::<BSShaderTextureSet>().as_ref() }
    }

    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.0.alpha
    }

    #[must_use]
    pub fn specular_power(&self) -> f32 {
        self.0.specularPower
    }
}

/// The nine texture path slots of a material (slot 0 the diffuse map).
#[repr(transparent)]
pub struct BSShaderTextureSet(sre::BSShaderTextureSet);

impl BSShaderTextureSet {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShaderTextureSet {
        &self.0
    }

    /// The path in `slot` (0..9), `None` when unset.
    #[must_use]
    pub fn path(&self, slot: usize) -> Option<&CStr> {
        if slot >= 9 {
            return None;
        }
        let p = usize::from_le_bytes(self.0.texturePaths[slot * 8..slot * 8 + 8].try_into().unwrap()) as *const c_char;
        if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }) }
    }
}

/// A geometry's alpha property: blend and test flags and the test threshold.
#[repr(transparent)]
pub struct NiAlphaProperty(sre::NiAlphaProperty);

impl NiAlphaProperty {
    #[must_use]
    pub fn raw(&self) -> &sre::NiAlphaProperty {
        &self.0
    }

    #[must_use]
    pub fn flags(&self) -> AlphaFlags {
        AlphaFlags(self.0.flags)
    }

    #[must_use]
    pub fn alpha_test_ref(&self) -> u8 {
        self.0.alphaTestRef
    }
}

/// One render pass: shader, property, geometry and technique, linked through the property's list.
#[repr(transparent)]
pub struct RenderPass(sre::RenderPass);

impl RenderPass {
    #[must_use]
    pub fn raw(&self) -> &sre::RenderPass {
        &self.0
    }

    #[must_use]
    pub fn geometry(&self) -> Option<&BSGeometry> {
        unsafe { BSGeometry::from_ptr(self.0.geometry) }
    }

    #[must_use]
    pub fn property(&self) -> Option<&BSShaderProperty> {
        unsafe { self.0.property.cast::<BSShaderProperty>().as_ref() }
    }

    #[must_use]
    pub fn technique_id(&self) -> u32 {
        self.0.techniqueId
    }

    #[must_use]
    pub fn light_count(&self) -> u8 {
        self.0.numLights
    }

    /// The next pass of the same property.
    #[must_use]
    pub fn next(&self) -> Option<&RenderPass> {
        unsafe { self.0.next.cast::<RenderPass>().as_ref() }
    }
}

/// A shader accumulator: the camera it collects for and its render mode.
#[repr(transparent)]
pub struct BSShaderAccumulator(sre::BSShaderAccumulator);

impl BSShaderAccumulator {
    #[must_use]
    pub fn raw(&self) -> &sre::BSShaderAccumulator {
        &self.0
    }

    /// # Safety
    /// `p` must be null or point to a live accumulator that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const sre::BSShaderAccumulator) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    /// `g_WorldDepthAccumulator` (mode 0xc), `g_WorldAccumulator` (mode 0) and `g_FirstPersonAccumulator` (mode 0x16).
    #[must_use]
    pub fn world() -> [Option<&'static Self>; 3] {
        let a = fns().accumulators;
        core::array::from_fn(|i| unsafe { (*a[i]).cast::<Self>().as_ref() })
    }

    #[must_use]
    pub fn render_mode(&self) -> Option<AccumulatorRenderMode> {
        AccumulatorRenderMode::from_u32(self.0.renderMode)
    }

    #[must_use]
    pub fn render_mode_raw(&self) -> u32 {
        self.0.renderMode
    }

    /// Set on the sun's cascade accumulators (mode 0xe), clear on the focus-map ones of the same mode.
    #[must_use]
    pub fn is_cascade_accumulator(&self) -> bool {
        self.0.isCascadeAccumulator != 0
    }

    #[must_use]
    pub fn cascade_index(&self) -> u32 {
        self.0.cascadeIndex
    }

    /// The shadow map this accumulator fills (`0xffff` for the main view).
    #[must_use]
    pub fn shadow_map_index(&self) -> u32 {
        self.0.shadowMapIndex
    }

    #[must_use]
    pub fn camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.camera) }
    }
}

/// The window being drawn and presented: handle, size, swap chain and back buffer views.
#[repr(transparent)]
pub struct RenderWindow(sre::RenderWindow);

impl RenderWindow {
    #[must_use]
    pub fn raw(&self) -> &sre::RenderWindow {
        &self.0
    }

    /// `g_CurrentRenderWindow` (id 411349).
    #[must_use]
    pub fn current() -> Option<&'static Self> {
        unsafe { (*fns().window).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn hwnd(&self) -> u64 {
        self.0.hwnd
    }

    #[must_use]
    pub fn size(&self) -> [u32; 2] {
        [self.0.width, self.0.height]
    }

    /// The `IDXGISwapChain` the frame presents.
    #[must_use]
    pub fn swap_chain(&self) -> *mut c_void {
        self.0.swapChain
    }

    /// The back buffer's `ID3D11RenderTargetView`; recreated on resize, never keep it across one.
    #[must_use]
    pub fn back_buffer_rtv(&self) -> *mut c_void {
        self.0.backBufferRTV
    }
}

/// The sky: the game hour it follows and the sun whose light the shadow scene node wraps.
#[repr(transparent)]
pub struct Sky(sre::Sky);

impl Sky {
    #[must_use]
    pub fn raw(&self) -> &sre::Sky {
        &self.0
    }

    /// `g_Sky_ptr` (id 401652), `None` before the sky exists.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().sky).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn current_game_hour(&self) -> f32 {
        self.0.currentGameHour
    }

    /// The sun's directional light, as a scene object.
    #[must_use]
    pub fn sun_light(&self) -> Option<&NiAVObject> {
        let sun = unsafe { self.0.sun.as_ref() }?;
        unsafe { NiAVObject::from_ptr(sun.light.cast()) }
    }
}
