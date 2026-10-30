//! Renderer (subsystem passes 5 and 6): the hooks a visual mod needs (`NiAVObject::Cull`,
//! `PlayerCharacter::SetFirstPerson`, `RenderWorld`, `PresentFrame`, the depth registration and the
//! cull list replay), the render globals the engine keeps as raw values, and what the generated
//! scene graph, light, culler and shader classes leave `unsafe`: transforms and the children array
//! decoded from their bytes, the hidden bit set atomically, light and shadow map arrays as slices,
//! checked casts by vtable, the shadow pass tests a cull detour makes, and an effect shader's data
//! applied to or removed from a tree; a reference's point light found by type and its colour,
//! fade and radius read and written.
//!
//! Reads follow the pass's thread model: light lists, roots and flags are stable while the frame
//! culls (the main thread changes them between frames); a [`CullFn`] detour runs on the job
//! threads concurrently with the main camera cull and must stay lock-free and read-only apart from
//! the call it forwards. Offsets of the transforms, the bounding sphere, the children array and
//! the viewport are the byte layouts `render.json` gives for its `undefined` fields.

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicU32, Ordering};

use sre::raw::{fns, vtables};
use sre::{BGSArtObject, BSCullingProcess, Calendar, ModelReferenceEffect, ReferenceEffect, ShaderReferenceEffect, Sky, TESObjectREFR, TESWeather, BSEffectShaderData, BSFadeNode, BSFlattenedBoneTree, BSGeometry, BSLight, BSLightingShaderMaterialBase, BSShaderMaterial, BSShaderProperty, BSShaderTextureSet, BSShadowLight, BSTextureSet, CameraMatrixEntry, NiAVObject, NiCamera, NiNode, NiPointLight, NiSkinInstance, TESFullName, TESObjectLIGH, NiTransform, ShaderMaterialType, ShadowMapData, ShadowSceneNode, TESEffectShader, TextureSetSlot};

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

/// The value of a setting object (an `ini_setting_*` or `game_setting_*` address of `sre`), which
/// sits at +8 of the object; `T` is the setting's type (`f32` for an `f` setting, `u32` for `i`).
#[must_use]
pub fn setting_value<T>(setting: *mut c_void) -> *mut T {
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
        let mut translation = self.translation;
        for (i, value) in translation.iter_mut().enumerate() {
            *value += self.scale * (0..3).map(|k| r[i][k] * child.translation[k]).sum::<f32>();
        }
        Self { rotation: mul3(r, &child.rotation), translation, scale: self.scale * child.scale }
    }
}

/// A row-major `NiMatrix3`.
pub type Matrix3 = [[f32; 3]; 3];

fn mul3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    core::array::from_fn(|i| core::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// The engine's elementary turn by `angle` radians about axis `axis` (0 X, 1 Y, 2 Z):
/// counter-clockwise about the negative axis, so `X(a)` is `[[1,0,0],[0,c,s],[0,-s,c]]`, the
/// transpose of the textbook rotation (RE `camera.md` IC8 O).
#[must_use]
pub fn axis_turn(axis: usize, angle: f32) -> Matrix3 {
    let (sin, cos) = angle.sin_cos();
    let (first, second) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut out = [[0.0; 3]; 3];
    out[axis][axis] = 1.0;
    out[first][first] = cos;
    out[first][second] = sin;
    out[second][first] = -sin;
    out[second][second] = cos;
    out
}

/// `NiMatrix3::FromEulerAnglesXYZ` (id 70221): `X(x) * Y(y) * Z(z)`, the order of a reference's
/// `rotX`/`rotY`/`rotZ` and of bone Euler angles.
#[must_use]
pub fn from_euler_xyz(x: f32, y: f32, z: f32) -> Matrix3 {
    mul3(&mul3(&axis_turn(0, x), &axis_turn(1, y)), &axis_turn(2, z))
}

/// The `[x, y, z]` [`from_euler_xyz`] builds `m` from: `NiMatrix3::ToEulerAnglesXYZ` (id 70215)
/// off the poles; at a pole `z` is 0 and `x` is solved so the angles build `m` again (the RE row
/// gives `x` the opposite sign there, which would not).
#[must_use]
pub fn to_euler_xyz(m: &Matrix3) -> [f32; 3] {
    let y = -m[0][2].clamp(-1.0, 1.0).asin();
    if m[0][2] <= -1.0 {
        [m[1][0].atan2(m[1][1]), y, 0.0]
    } else if m[0][2] >= 1.0 {
        [-m[1][0].atan2(m[1][1]), y, 0.0]
    } else {
        [m[1][2].atan2(m[2][2]), y, m[0][1].atan2(m[0][0])]
    }
}

/// `NiMatrix3::FromEulerAnglesZXY` (id 70233): `Z(z) * X(x) * Y(y)`, heading, pitch (positive
/// looking down) and roll as actor movement and the cameras turn.
#[must_use]
pub fn from_euler_zxy(z: f32, x: f32, y: f32) -> Matrix3 {
    mul3(&mul3(&axis_turn(2, z), &axis_turn(0, x)), &axis_turn(1, y))
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
    /// Writes the transform motion vectors take as last frame's (`previousWorld`, stashed by the
    /// object's first world update of a render frame). Main thread, after the frame's updates.
    fn set_previous_world(&self, world: Transform);
    /// Recomputes the world transforms and bounds after a local transform change
    /// (`NiAVObject::UpdateNode`: slot 44 on the object, then the parent's slot 61). Main thread, between frames.
    fn update_transforms(&self);
    /// Whether `ancestor` is this object or one of its parents, looking at most `depth` levels up.
    fn is_under(&self, ancestor: &NiAVObject, depth: usize) -> bool;
    /// Calls `f` with this object and every object below it, parents before children. Main thread,
    /// between frames, or from a render step that runs while the main thread waits.
    fn for_each(&self, f: &mut dyn FnMut(&NiAVObject));
    /// Whether a geometry under this object holds `data` as its shader property's effect data
    /// (`BSShaderProperty`+0x68).
    fn holds_effect_data(&self, data: &BSEffectShaderData) -> bool;
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

    fn set_previous_world(&self, world: Transform) {
        unsafe { core::ptr::from_ref(&self.previousWorld).cast::<Transform>().cast_mut().write_volatile(world) }
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

    fn holds_effect_data(&self, data: &BSEffectShaderData) -> bool {
        let data = core::ptr::from_ref(data).cast::<c_void>();
        let mut found = false;
        self.for_each(&mut |object| {
            found |= object.cast::<BSGeometry>().and_then(|g| unsafe { g.shaderProperty.as_ref() }).is_some_and(|p| core::ptr::eq(p.effectOverlay.cast_const(), data));
        });
        found
    }
}

/// Puts an effect shader's data on every geometry under `root` that takes it, through
/// `ApplyEffectShaderData` (id 17330), as `ShaderReferenceEffect` does for its own root; the
/// number of geometries. Main thread, or the effect update that owns `data`.
pub fn apply_effect_shader_data(shader: &TESEffectShader, root: &NiAVObject, data: &BSEffectShaderData) -> i32 {
    unsafe { sre::apply_effect_shader_data(core::ptr::from_ref(shader).cast_mut(), core::ptr::from_ref(root).cast_mut(), core::ptr::from_ref(data).cast_mut(), false) }
}

/// Clears `data` from every geometry under `root` that holds it (`RemoveEffectShaderData`, id 17331).
pub fn remove_effect_shader_data(shader: &TESEffectShader, root: &NiAVObject, data: &BSEffectShaderData) {
    unsafe { sre::remove_effect_shader_data(core::ptr::from_ref(shader).cast_mut(), core::ptr::from_ref(root).cast_mut(), core::ptr::from_ref(data).cast_mut()) }
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
    /// The bone nodes the skin deforms with (`boneNodes`, as many as the bind data's `boneCount`;
    /// RE `camera.md` IC8 J).
    fn bones(&self) -> &[Option<&NiAVObject>];
}

impl NiSkinInstanceExt for NiSkinInstance {
    fn bones(&self) -> &[Option<&NiAVObject>] {
        let count = unsafe { self.skinData.as_ref() }.map_or(0, |d| d.boneCount);
        if self.boneNodes.is_null() { &[] } else { unsafe { ptr_slice(self.boneNodes, count) } }
    }

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
            let world = unsafe { bone.node.as_ref() }.map_or_else(
                || {
                    let parent = usize::try_from(bone.parent).ok().and_then(|i| bones.get(i)).map_or(&self.world, |p| &p.world);
                    Transform::from(parent).compose(&Transform::from(&bone.local))
                },
                NiAVObjectExt::world_transform,
            );
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

/// Holds an art object on `target` for `duration` seconds (-1 until stopped) as Papyrus
/// `VisualEffect.Play` does: `CreateModelReferenceEffect` (id 22769) with no facing target and
/// no camera attachment (RE `camera.md` IC8 F). Main thread.
pub fn play_art_object(target: &TESObjectREFR, art: &BGSArtObject, duration: f32) {
    type PlayFn = unsafe extern "C" fn(*const TESObjectREFR, *const BGSArtObject, f32, *const TESObjectREFR, bool, bool, *const NiAVObject, bool);
    let play: PlayFn = unsafe { crate::engine_fn(fns::CreateModelReferenceEffect) };
    unsafe { play(target, art, duration, core::ptr::null(), false, false, core::ptr::null(), false) };
}

/// Holds an effect shader on `target` as Papyrus `EffectShader.Play` does
/// (`PlayShaderReferenceEffect`, id 19872). Main thread.
pub fn play_effect_shader(target: &TESObjectREFR, shader: &TESEffectShader, duration: f32) {
    type PlayFn = unsafe extern "C" fn(*const TESObjectREFR, *const TESEffectShader, f32, *const TESObjectREFR, bool, bool, *const NiAVObject, bool);
    let play: PlayFn = unsafe { crate::engine_fn(fns::PlayShaderReferenceEffect) };
    unsafe { play(target, shader, duration, core::ptr::null(), false, false, core::ptr::null(), false) };
}

/// The reference effect list in `ProcessLists` (data +0x108, count +0x118, spin lock +0x120).
const EFFECTS: usize = 0x108;
const EFFECT_COUNT: usize = 0x118;
const EFFECT_LOCK: usize = 0x120;

/// Whether the object's class is `node` or derives from it: the type node of slot 2, walked up
/// through the parent at +8 (RE `camera.md` IC8 E and F).
fn derives_from<T>(object: &T, node: *const u8) -> bool {
    type TypeFn = unsafe extern "C" fn(*const c_void) -> *const u8;
    let get: TypeFn = unsafe { core::mem::transmute(sre::vtable_entry(object, 2)) };
    let mut current = unsafe { get(core::ptr::from_ref(object).cast()) };
    while !current.is_null() {
        if current == node {
            return true;
        }
        current = unsafe { current.add(8).cast::<*const u8>().read_unaligned() };
    }
    false
}

/// Ends the art object (`art`) or effect shader effects of form `form_id` on the reference with
/// handle `target`, as the engine ends one: their `finished` flag set, under the effect list's
/// lock (RE `camera.md` IC8 F). Main thread.
pub fn stop_reference_effects(target: u32, form_id: u32, art: bool) {
    let Ok(holder) = crate::reloc().address(sre::raw::globals::g_ProcessLists_ptr) else { return };
    let lists = unsafe { *(holder as *const *const u8) };
    if lists.is_null() {
        return;
    }
    let _guard = crate::spin::SpinGuard::lock(unsafe { &*((lists as usize + EFFECT_LOCK) as *const sre::BSSpinLock) });
    let (data, count) = unsafe { (lists.add(EFFECTS).cast::<*const c_void>().read_unaligned(), lists.add(EFFECT_COUNT).cast::<u32>().read_unaligned()) };
    let node = if art { sre::g_type_node_model_reference_effect() } else { sre::g_type_node_shader_reference_effect() };
    for effect in unsafe { ptr_slice::<ReferenceEffect>(data, count) }.iter().flatten() {
        if effect.target != target || !derives_from(effect, node) {
            continue;
        }
        let form = if art {
            unsafe { (*core::ptr::from_ref(effect).cast::<ModelReferenceEffect>()).artObject.as_ref() }.map(|a| a.formID)
        } else {
            unsafe { (*core::ptr::from_ref(effect).cast::<ShaderReferenceEffect>()).effectShader.as_ref() }.map(|s| s.formID)
        };
        if form == Some(form_id) {
            crate::store(&effect.finished, 1);
        }
    }
}

/// The first point light at or under `root`, found by its type node (`g_TypeNode_NiPointLight`;
/// RE `camera.md` IC8 E): the light a light reference's 3D carries.
#[must_use]
pub fn point_light(root: &NiAVObject) -> Option<&NiPointLight> {
    if derives_from(root, sre::g_type_node_ni_point_light()) {
        return Some(unsafe { &*core::ptr::from_ref(root).cast::<NiPointLight>() });
    }
    root.cast::<NiNode>()?.children().iter().flatten().find_map(|child| point_light(child))
}

/// What a light shines with: `NiLight`'s diffuse colour, fade (its intensity) and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightValues {
    pub color: [f32; 3],
    pub fade: f32,
    pub radius: f32,
}

pub trait NiPointLightExt {
    /// The radius is the first of the three radius floats.
    fn values(&self) -> LightValues;
    /// Writes the colour, the fade and the radius into all three radius floats, then the
    /// attenuation that radius gives (`ApplyLightAttenuationSettings`, id 17626). Main thread.
    fn set_values(&self, values: LightValues);
}

impl NiPointLightExt for NiPointLight {
    fn values(&self) -> LightValues {
        LightValues { color: [self.diffuse.r, self.diffuse.g, self.diffuse.b], fade: self.fade, radius: self.radius.x }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn set_values(&self, values: LightValues) {
        let [r, g, b] = values.color;
        for (field, value) in [(&self.diffuse.r, r), (&self.diffuse.g, g), (&self.diffuse.b, b), (&self.radius.x, values.radius), (&self.radius.y, values.radius), (&self.radius.z, values.radius), (&self.fade, values.fade)] {
            crate::store(field, value);
        }
        sre::apply_light_attenuation_settings(self, values.radius.max(0.0) as u32);
    }
}

pub trait TESObjectLIGHExt {
    /// The light's full name (its `TESFullName` base at +0x30); `None` when empty.
    fn full_name(&self) -> Option<&CStr>;
}

impl TESObjectLIGHExt for TESObjectLIGH {
    fn full_name(&self) -> Option<&CStr> {
        let name = unsafe { &*core::ptr::from_ref(self).byte_add(0x30).cast::<TESFullName>() }.name;
        if name.is_null() { None } else { Some(unsafe { CStr::from_ptr(name.cast()) }).filter(|s| !s.is_empty()) }
    }
}

/// The sun's scene objects (RE `camera.md` IC8 G): the light whose rotation the shadows read, the
/// root whose translation places the sun (the disc hangs under it) and the glare billboard under
/// the weather node, which `Sun::Update` gives the root's translation. A mod moving the sun writes
/// the root and the glare after `Sun::Update`.
pub struct SunNodes {
    pub light: &'static NiAVObject,
    pub root: Option<&'static NiAVObject>,
    pub glare: Option<&'static NiAVObject>,
}

pub trait SkyExt {
    /// `None` without a sun or its light (menus, loading).
    fn sun_nodes(&self) -> Option<SunNodes>;
    /// The weather on screen (`currentWeather`, +0x48).
    fn current_weather(&self) -> Option<&'static TESWeather>;
}

impl SkyExt for Sky {
    fn sun_nodes(&self) -> Option<SunNodes> {
        let sun = unsafe { self.sun.as_ref() }?;
        let light: &NiAVObject = unsafe { sun.light.as_ref() }?;
        Some(SunNodes { light, root: unsafe { sun.root.cast::<NiAVObject>().as_ref() }, glare: unsafe { sun.glareBillboard.cast::<NiAVObject>().as_ref() } })
    }

    fn current_weather(&self) -> Option<&'static TESWeather> {
        unsafe { self.currentWeather.as_ref() }
    }
}

/// `GameHour` (the calendar's global +0x20, 0 to 24), which the sky reads every frame.
#[must_use]
pub fn game_hour() -> Option<f32> {
    unsafe { Calendar::singleton()?.gameHour.as_ref() }.map(|g| g.value)
}

/// Writes `GameHour`. Main thread.
pub fn set_game_hour(hour: f32) {
    if let Some(global) = Calendar::singleton().and_then(|c| unsafe { c.gameHour.as_ref() }) {
        crate::store(&global.value, hour);
    }
}

/// Whether the player is in an interior cell (`TES`+0xc0 set). The sky's mode is no answer: an
/// interior that shows the sky runs mode 2 or 3.
#[must_use]
pub fn is_interior() -> bool {
    sre::TES::singleton().is_some_and(|tes| tes.field_C0 != 0)
}

#[cfg(test)]
mod tests {
    use super::{Matrix3, Transform, axis_turn, from_euler_xyz, from_euler_zxy, mul3, to_euler_xyz};

    fn close(a: &Matrix3, b: &Matrix3) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-4))
    }

    #[test]
    fn euler_xyz_is_the_engines_matrix() {
        let (sin, cos) = 0.5_f32.sin_cos();
        assert!(close(&from_euler_xyz(0.5, 0.0, 0.0), &[[1.0, 0.0, 0.0], [0.0, cos, sin], [0.0, -sin, cos]]));
        assert!(close(&from_euler_xyz(0.0, 0.5, 0.0), &[[cos, 0.0, -sin], [0.0, 1.0, 0.0], [sin, 0.0, cos]]));
        assert!(close(&from_euler_xyz(0.0, 0.0, 0.5), &[[cos, sin, 0.0], [-sin, cos, 0.0], [0.0, 0.0, 1.0]]));
        let chair = from_euler_xyz(0.0, 0.0, 5.883);
        let seen: Matrix3 = [[0.921, -0.389, 0.0], [0.389, 0.921, 0.0], [0.0, 0.0, 1.0]];
        assert!((0..3).all(|i| (0..3).all(|j| (chair[i][j] - seen[i][j]).abs() < 1e-3)), "the run's chair: {chair:?}");
        let (x, y, z) = (0.3_f32, -0.7_f32, 1.2_f32);
        let textbook = |axis: usize, a: f32| axis_turn(axis, -a);
        let product = mul3(&mul3(&textbook(2, z), &textbook(1, y)), &textbook(0, x));
        let transposed: Matrix3 = core::array::from_fn(|i| core::array::from_fn(|j| product[j][i]));
        let m = from_euler_xyz(x, y, z);
        assert!(close(&m, &transposed), "(Rz Ry Rx)^T");
        assert!((m[0][2] + y.sin()).abs() < 1e-6);
    }

    #[test]
    fn euler_xyz_round_trips_through_the_getter() {
        for angles in [[0.3, -0.7, 1.2], [-2.0, 0.4, -0.1], [0.0, 0.0, 0.0], [1.0, 1.4, -3.0], [-0.5, -1.5, 2.5]] {
            let back = to_euler_xyz(&from_euler_xyz(angles[0], angles[1], angles[2]));
            assert!(angles.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-3), "{angles:?} -> {back:?}");
        }
        for y in [core::f32::consts::FRAC_PI_2, -core::f32::consts::FRAC_PI_2] {
            let mut pole = from_euler_xyz(0.4, y, 0.0);
            pole[0][2] = pole[0][2].signum();
            let back = to_euler_xyz(&pole);
            assert!(back[2] == 0.0 && close(&from_euler_xyz(back[0], back[1], back[2]), &pole), "{y}: {back:?}");
        }
    }

    #[test]
    fn euler_zxy_turns_heading_pitch_then_roll() {
        let forward = |m: &Matrix3| [m[0][1], m[1][1], m[2][1]];
        let (pitch, heading) = (0.4_f32, 1.1_f32);
        let f = forward(&from_euler_zxy(heading, pitch, 0.0));
        let expected = [heading.sin() * pitch.cos(), heading.cos() * pitch.cos(), -pitch.sin()];
        assert!((0..3).all(|i| (f[i] - expected[i]).abs() < 1e-5), "{f:?}");
        let rolled = forward(&from_euler_zxy(0.0, 0.0, 0.3));
        assert!((rolled[1] - 1.0).abs() < 1e-6, "roll keeps the forward axis");
    }

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
