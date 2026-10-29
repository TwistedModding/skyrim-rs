//! Renderer (subsystem passes 5 and 6): the hooks a visual mod needs (`NiAVObject::Cull`,
//! `PlayerCharacter::SetFirstPerson`, `RenderWorld`, `PresentFrame`, the depth registration and the
//! cull list replay), the render globals the engine keeps as raw values, and what the generated
//! scene graph, light, culler and shader classes leave `unsafe`: transforms and the children array
//! decoded from their bytes, the hidden bit set atomically, light and shadow map arrays as slices,
//! checked casts by vtable, and the shadow pass tests a cull detour makes.
//!
//! Reads follow the pass's thread model: light lists, roots and flags are stable while the frame
//! culls (the main thread changes them between frames); a [`CullFn`] detour runs on the job
//! threads concurrently with the main camera cull and must stay lock-free and read-only apart from
//! the call it forwards. Offsets of the transforms, the bounding sphere, the children array and
//! the viewport are the byte layouts `render.json` gives for its `undefined` fields.

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicU32, Ordering};

use sre::raw::{fns, vtables};
use sre::{BSCullingProcess, BSFadeNode, BSFlattenedBoneTree, BSGeometry, BSLight, BSLightingShaderMaterialBase, BSShaderMaterial, BSShaderProperty, BSShaderTextureSet, BSShadowLight, BSTextureSet, CameraMatrixEntry, NiAVObject, NiCamera, NiNode, NiSkinInstance, NiTransform, ShaderMaterialType, ShadowMapData, ShadowSceneNode, TextureSetSlot};

use crate::ptr_slice;

pub use fns::BSShaderAccumulator__RegisterDepthGeometry_fn as RegisterDepthGeometryFn;
pub use fns::PlayerCharacter__SetFirstPerson_fn as SetFirstPersonFn;
pub use fns::PresentFrame_fn as PresentFn;
pub use fns::ProcessCullObjectList_fn as ProcessCullObjectListFn;
pub use fns::RenderWorld_fn as RenderWorldFn;

/// `NiAVObject::Cull(node, culler, orderedGroup)`: the third argument passes through `r8` into the
/// culler's slot 22 and is the index of the node inside a `BSOrderedNode` (0xffffffff elsewhere),
/// not a plane mask; a detour must pass it through unchanged.
pub type CullFn = fns::NiAVObject__Cull_fn;

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

fn create<F: Copy>(id: u64, detour: F) -> Result<hook::Hook<F>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(id)?, detour) }
}

/// `g_RenderFrameCounter`: incremented by `PresentFrame`; skin palettes and pass lists are cached per value.
#[must_use]
pub fn frame_counter() -> u32 {
    unsafe { AtomicU32::from_ptr(sre::g_render_frame_counter()).load(Ordering::Relaxed) }
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
    create(fns::NiAVObject__Cull, detour)
}

/// Creates, disabled, a detour on `RenderWorld` (id 36558): the world draw of a frame, on the main
/// thread; after the original returns the image space and the splatter have run and the UI is next.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_render_world(detour: RenderWorldFn) -> Result<hook::Hook<RenderWorldFn>, hook::Error> {
    create(fns::RenderWorld, detour)
}

/// Creates, disabled, a detour on `PresentFrame` (id 77246): the swap chain present, called by the
/// frame, the Bink movie loop and the loading screen thread; `g_RenderFrameCounter` is incremented
/// inside it, so the detour runs on more than one thread.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_present(detour: PresentFn) -> Result<hook::Hook<PresentFn>, hook::Error> {
    create(fns::PresentFrame, detour)
}

/// Creates, disabled, a detour on `PlayerCharacter::SetFirstPerson` (id 40476), called on the
/// main thread at every switch; store it, then [`hook::Hook::enable`].
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_set_first_person(detour: SetFirstPersonFn) -> Result<hook::Hook<SetFirstPersonFn>, hook::Error> {
    create(fns::PlayerCharacter__SetFirstPerson, detour)
}

/// Creates, disabled, a detour on `BSShaderAccumulator::RegisterDepthGeometry` (id 106576): every
/// geometry a depth or shadow accumulator accepted from a cull, before the property's slot 43
/// builds its passes; returning without calling the original drops the geometry from that map.
/// The shadow modes (0xd spot, 0xe cascade or focus, 0xf parabolic; `BSShaderAccumulator::renderMode`)
/// arrive on the frame thread, the depth pre-pass mode on workers, concurrently.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_register_depth_geometry(detour: RegisterDepthGeometryFn) -> Result<hook::Hook<RegisterDepthGeometryFn>, hook::Error> {
    create(fns::BSShaderAccumulator__RegisterDepthGeometry, detour)
}

/// Creates, disabled, a detour on `ProcessCullObjectList` (id 76561): the replay of a geometry
/// list into a culler, which the sun's cascades run on the frame thread over the lists its walk
/// built; with `skipHidden` an entry whose own hidden bit is set is dropped without `NiAVObject::Cull`.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_process_cull_object_list(detour: ProcessCullObjectListFn) -> Result<hook::Hook<ProcessCullObjectListFn>, hook::Error> {
    create(fns::ProcessCullObjectList, detour)
}

/// `fNearDistance:Display` (id 388779): the world near plane every frustum rebuild and every depth shift read.
#[must_use]
pub fn near_distance() -> f32 {
    unsafe { sre::ini_setting_f_near_distance_display_value().read_volatile() }
}

/// Writes the setting alone: effective from the next frame's first depth shift, the GPU one frame later.
pub fn set_near_distance(near: f32) {
    unsafe { sre::ini_setting_f_near_distance_display_value().write_volatile(near) }
}

/// `fNear1stPersonDistance:Display` (id 388782): the hands camera's near.
#[must_use]
pub fn first_person_near_distance() -> f32 {
    unsafe { sre::ini_setting_f_near1st_person_distance_display_value().read_volatile() }
}

/// The near `BeginDepthShift` last cached from the scene graph (`g_DepthShiftNear`, id 414949).
#[must_use]
pub fn depth_shift_near() -> f32 {
    unsafe { sre::g_depth_shift_near().read_volatile() }
}

/// The far `BeginDepthShift` last cached and `EndDepthShift` restores (`g_DepthShiftFar`, id 414948).
#[must_use]
pub fn depth_shift_far() -> f32 {
    unsafe { sre::g_depth_shift_far().read_volatile() }
}

/// The consistent near-plane write: the setting, then a forced `SetFieldOfView` with the live
/// world FOV plus offset, so the frame camera, the GPU snapshot, the depth shifts, the cascades,
/// the water stage and the fog all see `near` from this frame on. Main thread, before `RenderWorld`:
/// the state-step detour is the place. A write into the frame camera alone is undone by the
/// frame's first depth shift.
pub fn set_near_plane(near: f32) {
    set_near_distance(near);
    let fov = sre::PlayerCamera::singleton().map_or(0.0, |c| c.worldFOV) + crate::camera::fov_offset();
    if let Some(graph) = sre::g_scene_graph() {
        graph.set_field_of_view(fov, true, None, true);
    }
}

/// `g_ShadowDistance` (id 415263): the live shadow distance, `fShadowDistance` outdoors and
/// 3000 indoors (`SetShadowDistanceForInterior` rewrites it at every cell change).
#[must_use]
pub fn shadow_distance() -> f32 {
    unsafe { sre::g_shadow_distance().read_volatile() }
}

/// Writes the live shadow distance and its square (the local lights' distance cull); the next
/// interior or exterior transition rewrites both from the settings.
pub fn set_shadow_distance(distance: f32) {
    unsafe {
        sre::g_shadow_distance().write_volatile(distance);
        sre::g_shadow_distance_sq().write_volatile(distance * distance);
    }
}

/// `g_ShadowMapResolution` (id 415268): the cascade and local map resolution cached at startup.
#[must_use]
pub fn shadow_map_resolution() -> u32 {
    unsafe { sre::g_shadow_map_resolution().read_volatile() }
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

fn setting_value<T>(setting: *mut c_void) -> *mut T {
    setting.wrapping_byte_add(SETTING_VALUE).cast()
}

/// Reads the four cascade settings.
#[must_use]
pub fn cascade_settings() -> CascadeSettings {
    unsafe {
        CascadeSettings {
            shadow_distance: setting_value::<f32>(sre::ini_pref_setting_f_shadow_distance_display()).read_volatile(),
            first_slice_distance: setting_value::<f32>(sre::ini_setting_f_first_slice_distance_display()).read_volatile(),
            split_overlap: setting_value::<f32>(sre::ini_setting_f_split_overlap_display()).read_volatile(),
            splits: setting_value::<u32>(sre::ini_pref_setting_i_num_splits_display()).read_volatile(),
        }
    }
}

/// Writes the four cascade settings; the sun reads them at its next `UpdateForCamera`, and the
/// shadow distance also at the next cell transition (see [`set_shadow_distance`] for the live value).
pub fn set_cascade_settings(settings: CascadeSettings) {
    unsafe {
        setting_value::<f32>(sre::ini_pref_setting_f_shadow_distance_display()).write_volatile(settings.shadow_distance);
        setting_value::<f32>(sre::ini_setting_f_first_slice_distance_display()).write_volatile(settings.first_slice_distance);
        setting_value::<f32>(sre::ini_setting_f_split_overlap_display()).write_volatile(settings.split_overlap);
        setting_value::<u32>(sre::ini_pref_setting_i_num_splits_display()).write_volatile(settings.splits);
    }
}

/// The cameras `g_FrameState` snapshotted for the GPU this frame: the frame camera twice (jitter 0
/// and 1), the hands camera in first person.
#[must_use]
pub fn camera_entries() -> &'static [CameraMatrixEntry] {
    let base = core::ptr::from_ref(sre::g_frame_state()).cast::<u8>();
    let (data, count) = unsafe { (base.add(FRAME_CAMERA_ENTRIES).cast::<*const CameraMatrixEntry>().read_unaligned(), base.add(FRAME_CAMERA_ENTRY_COUNT).cast::<u32>().read_unaligned()) };
    if data.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(data, count as usize) } }
}

/// `g_ShadowSceneNode[index]` (id 390951), `None` while the slot is empty; index 0 is the world.
#[must_use]
pub fn shadow_scene_node(index: usize) -> Option<&'static ShadowSceneNode> {
    let slots = crate::reloc().address(sre::raw::globals::g_ShadowSceneNode).ok()? as *const [*const ShadowSceneNode; 4];
    unsafe { (*slots).get(index)?.as_ref() }
}

/// The `N` floats at the start of a field: a math value type (`NiPoint3`, `NiMatrix3`) or bytes
/// the ledger has not typed.
///
/// # Panics
/// When the field is shorter than `N` floats.
#[must_use]
pub fn floats<const N: usize>(field: &impl Sized) -> [f32; N] {
    assert!(size_of_val(field) >= N * 4, "the field holds fewer than {N} floats");
    unsafe { core::ptr::from_ref(field).cast::<[f32; N]>().read_unaligned() }
}

/// Rotation, translation and scale as `NiTransform` lays them out (rotation rows first), as a
/// plain value.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
    pub scale: f32,
}

const _: () = assert!(size_of::<Transform>() == size_of::<NiTransform>());

impl Transform {
    /// `self * child`: the world transform of an object whose parent's world is `self` and whose
    /// local transform is `child`.
    #[must_use]
    pub fn compose(&self, child: &Self) -> Self {
        let r = &self.rotation;
        let mut rotation = [[0.0; 3]; 3];
        let mut translation = self.translation;
        for (i, row) in rotation.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| r[i][k] * child.rotation[k][j]).sum();
            }
            translation[i] += self.scale * (0..3).map(|k| r[i][k] * child.translation[k]).sum::<f32>();
        }
        Self { rotation, translation, scale: self.scale * child.scale }
    }
}

impl From<&NiTransform> for Transform {
    fn from(t: &NiTransform) -> Self {
        unsafe { core::ptr::from_ref(t).cast::<Self>().read_volatile() }
    }
}

/// A scene graph class an `NiAVObject` can be checked and viewed as ([`NiAVObjectExt::cast`]).
///
/// # Safety
/// `VTABLES` must list only vtables of `Self` and the classes derived from it.
pub unsafe trait NodeClass: 'static {
    const VTABLES: &'static [u64];
}

unsafe impl NodeClass for NiNode {
    const VTABLES: &'static [u64] = &[
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
}

unsafe impl NodeClass for BSFadeNode {
    const VTABLES: &'static [u64] = &[vtables::BSFadeNode_vtable::ID];
}

unsafe impl NodeClass for BSFlattenedBoneTree {
    const VTABLES: &'static [u64] = &[vtables::BSFlattenedBoneTree_vtable::ID];
}

unsafe impl NodeClass for BSGeometry {
    const VTABLES: &'static [u64] = &[
        vtables::BSGeometry_vtable::ID,
        vtables::BSTriShape_vtable::ID,
        vtables::BSDynamicTriShape_vtable::ID,
        vtables::BSSubIndexTriShape_vtable::ID,
        vtables::BSMeshLODTriShape_vtable::ID,
        vtables::BSLODTriShape_vtable::ID,
        vtables::BSLines_vtable::ID,
    ];
}

pub trait NiAVObjectExt {
    /// The object as a `T` when its vtable is one of `T`'s, so `object.cast::<sre::NiNode>()` is a
    /// node or `None`.
    fn cast<T: NodeClass>(&self) -> Option<&T>;
    /// Sets or clears the hidden bit. The frame itself toggles this bit on the player's first-person
    /// root between its steps, so a mod changes it on the main thread between frames or on the
    /// player's third-person root only.
    fn set_hidden(&self, hidden: bool);
    fn local_transform(&self) -> Transform;
    /// Writes the local transform. On the main thread between frames follow it with
    /// [`update_transforms`](Self::update_transforms); from a pose-write hook on a bone of the graph's
    /// own skeleton, call nothing (`crate::animation`).
    fn set_local_transform(&self, local: Transform);
    /// Writes the local scale alone, under the same rules as [`set_local_transform`](Self::set_local_transform).
    fn set_local_scale(&self, scale: f32);
    fn world_transform(&self) -> Transform;
    /// Recomputes the world transforms and bounds after a local transform change
    /// (`NiAVObject::UpdateNode`: slot 44 on the object, then the parent's slot 61). Main thread, between frames.
    fn update_transforms(&self);
    /// Whether `ancestor` is this object or one of its parents, looking at most `depth` levels up.
    fn is_under(&self, ancestor: &NiAVObject, depth: usize) -> bool;
    /// Calls `f` with this object and every object below it, parents before children. Main thread,
    /// between frames, or from a render step that runs while the main thread waits.
    fn for_each(&self, f: &mut dyn FnMut(&NiAVObject));
}

impl NiAVObjectExt for NiAVObject {
    fn cast<T: NodeClass>(&self) -> Option<&T> {
        let id = crate::class_of(self)?.id;
        T::VTABLES.contains(&id).then(|| unsafe { &*core::ptr::from_ref(self).cast::<T>() })
    }

    fn set_hidden(&self, hidden: bool) {
        let flags = unsafe { AtomicU32::from_ptr((&raw const self.flags).cast_mut()) };
        if hidden {
            flags.fetch_or(HIDDEN, Ordering::Relaxed);
        } else {
            flags.fetch_and(!HIDDEN, Ordering::Relaxed);
        }
    }

    fn local_transform(&self) -> Transform {
        Transform::from(&self.local)
    }

    fn set_local_transform(&self, local: Transform) {
        unsafe { core::ptr::from_ref(&self.local).cast::<Transform>().cast_mut().write_volatile(local) }
    }

    fn set_local_scale(&self, scale: f32) {
        crate::store(&self.local.scale, scale);
    }

    fn world_transform(&self) -> Transform {
        Transform::from(&self.world)
    }

    fn update_transforms(&self) {
        let mut data = [0u64; 8];
        unsafe { self.update_node(data.as_mut_ptr().cast()) }
    }

    fn is_under(&self, ancestor: &NiAVObject, depth: usize) -> bool {
        let mut node: &NiAVObject = self;
        for _ in 0..=depth {
            if core::ptr::eq(node, ancestor) {
                return true;
            }
            match unsafe { node.parent.cast::<NiAVObject>().as_ref() } {
                Some(p) => node = p,
                None => return false,
            }
        }
        false
    }

    fn for_each(&self, f: &mut dyn FnMut(&NiAVObject)) {
        f(self);
        if let Some(node) = self.cast::<NiNode>() {
            for child in node.children().iter().flatten() {
                child.for_each(f);
            }
        }
    }
}

pub trait NiNodeExt {
    /// The children, null entries included; data at +0x118 and the u16 slots-in-use count at +0x122.
    fn children(&self) -> &[Option<&NiAVObject>];
}

impl NiNodeExt for NiNode {
    fn children(&self) -> &[Option<&NiAVObject>] {
        let b = &self.children;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let size = u16::from_le_bytes([b[18], b[19]]);
        unsafe { ptr_slice(data, u32::from(size)) }
    }
}

pub trait NiSkinInstanceExt {
    /// Makes the next draw rebuild a palette already built in render frame `frame`
    /// (`g_RenderFrameCounter`): `paletteFrame` steps back one and the current and previous palettes
    /// swap under the skin's lock, so the rebuild's copy of current over previous restores the
    /// previous frame's palette. False when the palette was not built in `frame`.
    fn rewind_palette(&self, frame: u32) -> bool;
}

impl NiSkinInstanceExt for NiSkinInstance {
    fn rewind_palette(&self, frame: u32) -> bool {
        use windows_sys::Win32::System::Threading::{CRITICAL_SECTION, EnterCriticalSection, LeaveCriticalSection};
        const { assert!(size_of::<CRITICAL_SECTION>() == 40) };
        let lock = (&raw const self.paletteLock).cast::<CRITICAL_SECTION>().cast_mut();
        unsafe { EnterCriticalSection(lock) };
        let built = unsafe { (&raw const self.paletteFrame).read_volatile() } == frame;
        if built {
            crate::store(&self.paletteFrame, frame.wrapping_sub(1));
            let (current, previous) = unsafe { ((&raw const self.palette).read_volatile(), (&raw const self.previousPalette).read_volatile()) };
            crate::store(&self.palette, previous);
            crate::store(&self.previousPalette, current);
        }
        unsafe { LeaveCriticalSection(lock) };
        built
    }
}

/// One flat bone of a `BSFlattenedBoneTree` (0x80 bytes, `camera.md` section 10).
#[repr(C)]
pub struct FlatBone {
    pub local: NiTransform,
    pub world: NiTransform,
    /// Index of the parent entry, negative for the tree node itself.
    pub parent: i32,
    unknown_6c: [u8; 4],
    /// The bone's own node, null for a bone dissolved into the tree.
    pub node: *mut NiAVObject,
    pub name: *const core::ffi::c_char,
}

const _: () = assert!(size_of::<FlatBone>() == 0x80);

pub trait BSFlattenedBoneTreeExt {
    fn bones(&self) -> &[FlatBone];
    /// Re-derives every entry's world transform: an entry with a node takes the node's world, one
    /// without composes its parent entry's world (the tree's own for the first level) with its
    /// local. Node worlds are read, never written. Main thread, or a render step the main thread waits on.
    fn rechain_bones(&self);
}

impl BSFlattenedBoneTreeExt for BSFlattenedBoneTree {
    fn bones(&self) -> &[FlatBone] {
        if self.bones.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.bones.cast::<FlatBone>(), self.boneCount as usize) }
    }

    fn rechain_bones(&self) {
        let bones = self.bones();
        for bone in bones {
            let world = match unsafe { bone.node.as_ref() } {
                Some(node) => node.world_transform(),
                None => {
                    let parent = usize::try_from(bone.parent).ok().and_then(|i| bones.get(i)).map_or(&self.world, |p| &p.world);
                    Transform::from(parent).compose(&Transform::from(&bone.local))
                }
            };
            unsafe { core::ptr::from_ref(&bone.world).cast::<Transform>().cast_mut().write_volatile(world) }
        }
    }
}

pub trait BSShadowLightExt {
    /// The `ShadowMapData` records (inline, 0xf0 each) of the light's maps.
    fn shadow_maps(&self) -> &[ShadowMapData];
    /// The four actor-focused map records (`iNumFocusShadow`).
    fn focus_shadow_maps(&self) -> &[ShadowMapData; 4];
    /// Whether `camera` is one of this light's map or focus map cameras.
    fn owns_camera(&self, camera: *const NiCamera) -> bool;
}

impl BSShadowLightExt for BSShadowLight {
    fn shadow_maps(&self) -> &[ShadowMapData] {
        let maps = unsafe { core::ptr::read_unaligned(&raw const self.shadowMaps) };
        if maps.data.is_null() || maps.size == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(maps.data.cast::<ShadowMapData>(), maps.size as usize) }
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn focus_shadow_maps(&self) -> &[ShadowMapData; 4] {
        unsafe { &*(&raw const self.focusShadowMaps).cast::<[ShadowMapData; 4]>() }
    }

    fn owns_camera(&self, camera: *const NiCamera) -> bool {
        self.shadow_maps().iter().chain(self.focus_shadow_maps()).any(|m| core::ptr::eq(m.camera, camera))
    }
}

pub trait BSCullingProcessExt {
    /// Whether this culler walks the scene for a shadow map: its camera is the sun's light or
    /// cascade camera, a focus map camera, or a map camera of one of the scene's shadow lights.
    fn is_shadow_pass(&self) -> bool;
    /// Whether this is one of the sun's geometry-list cullers (step 8b, a worker thread): the walk
    /// that decides the sun's caster set, whose lists the cascades replay without `NiAVObject::Cull`.
    fn is_sun_list_walk(&self) -> bool;
}

impl BSCullingProcessExt for BSCullingProcess {
    fn is_shadow_pass(&self) -> bool {
        let Some(scene) = shadow_scene_node(0) else { return false };
        let camera = self.camera.cast_const();
        scene.sun_shadow_light().is_some_and(|sun| core::ptr::eq(sun.lightCamera, camera) || sun.owns_camera(camera))
            || scene.shadow_lights().iter().flatten().any(|l| l.owns_camera(camera))
    }

    fn is_sun_list_walk(&self) -> bool {
        crate::class_of(self).is_some_and(|c| c.id == vtables::BSGeometryListCullingProcess_vtable::ID)
            && shadow_scene_node(0).and_then(ShadowSceneNode::sun_shadow_light).is_some_and(|sun| core::ptr::eq(sun.lightCamera, self.camera))
    }
}

pub trait ShadowSceneNodeExt {
    /// Active lights that cast no shadow (`NiPointer<BSLight>` entries).
    fn lights(&self) -> &[Option<&BSLight>];
    /// Shadow-casting lights other than the sun.
    fn shadow_lights(&self) -> &[Option<&BSShadowLight>];
}

impl ShadowSceneNodeExt for ShadowSceneNode {
    fn lights(&self) -> &[Option<&BSLight>] {
        unsafe { ptr_slice(self.lights.data, self.lights.size) }
    }

    fn shadow_lights(&self) -> &[Option<&BSShadowLight>] {
        unsafe { ptr_slice(self.shadowLights.data, self.shadowLights.size) }
    }
}

pub trait BSShaderPropertyExt {
    /// The shadow-map receiver mask (`lightData` +0x1c): bit `n` set while the geometry is
    /// registered into map `n`'s accumulator, cleared by the main view's registration after the
    /// lighting passes were built from it.
    fn receiver_mask(&self) -> Option<u32>;
}

impl BSShaderPropertyExt for BSShaderProperty {
    fn receiver_mask(&self) -> Option<u32> {
        let data = self.lightData;
        (!data.is_null()).then(|| unsafe { data.cast::<u8>().add(RECEIVER_MASK).cast::<u32>().read_unaligned() })
    }
}

pub trait BSShaderMaterialExt {
    /// The lighting material when slot 7 `GetType` says lighting.
    fn as_lighting(&self) -> Option<&BSLightingShaderMaterialBase>;
}

impl BSShaderMaterialExt for BSShaderMaterial {
    fn as_lighting(&self) -> Option<&BSLightingShaderMaterialBase> {
        (self.get_type() == Ok(ShaderMaterialType::Lighting)).then(|| unsafe { &*core::ptr::from_ref(self).cast::<BSLightingShaderMaterialBase>() })
    }
}

pub trait BSTextureSetExt {
    /// The path in `slot` of a `BSShaderTextureSet` through its `GetTexturePath`; `None` when
    /// unset or when the set is of another class.
    fn texture_path(&self, slot: TextureSetSlot) -> Option<&CStr>;
}

impl BSTextureSetExt for BSTextureSet {
    fn texture_path(&self, slot: TextureSetSlot) -> Option<&CStr> {
        if crate::class_of(self)?.id != vtables::BSShaderTextureSet_vtable::ID {
            return None;
        }
        let p = unsafe { &*core::ptr::from_ref(self).cast::<BSShaderTextureSet>() }.get_texture_path(slot);
        if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }) }
    }
}

#[cfg(test)]
mod tests {
    use super::Transform;

    #[test]
    fn compose_scales_rotates_then_translates_the_child() {
        let quarter_turn = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let parent = Transform { rotation: quarter_turn, translation: [10.0, 0.0, 5.0], scale: 2.0 };
        let child = Transform { rotation: quarter_turn, translation: [1.0, 0.0, 0.0], scale: 0.5 };
        let world = parent.compose(&child);
        assert_eq!(world.translation, [10.0, 2.0, 5.0]);
        assert_eq!(world.rotation, [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!((world.scale - 1.0).abs() < f32::EPSILON);
    }
}
