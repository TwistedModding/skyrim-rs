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
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use sre::raw::{fns, vtables};
use sre::{BGSArtObject, BSCullingProcess, Calendar, ModelReferenceEffect, ReferenceEffect, ShaderReferenceEffect, Sky, TESObjectREFR, TESWeather, BSEffectShaderData, BSFadeNode, BSFlattenedBoneTree, BSGeometry, BSLight, BSLightingShaderMaterialBase, BSShaderMaterial, BSShaderProperty, BSShaderTextureSet, BSShadowLight, BSTextureSet, CameraMatrixEntry, NiAVObject, NiCamera, NiNode, NiPointLight, NiSkinInstance, TESObjectLIGH, NiTransform, ShaderMaterialType, ShadowMapData, ShadowSceneNode, TESEffectShader, TextureSetSlot};

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
    create(sre::id!(fns::NiAVObject__Cull), detour)
}

/// Creates, disabled, a detour on `RenderWorld` (id 36558): the world draw of a frame, on the main
/// thread; after the original returns the image space and the splatter have run and the UI is next.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_render_world(detour: RenderWorldFn) -> Result<hook::Hook<RenderWorldFn>, hook::Error> {
    create(sre::id!(fns::RenderWorld), detour)
}

/// Creates, disabled, a detour on `PresentFrame` (id 77246): the swap chain present, called by the
/// frame, the Bink movie loop and the loading screen thread; `g_RenderFrameCounter` is incremented
/// inside it, so the detour runs on more than one thread.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_present(detour: PresentFn) -> Result<hook::Hook<PresentFn>, hook::Error> {
    create(sre::id!(fns::PresentFrame), detour)
}

/// Creates, disabled, a detour on `PlayerCharacter::SetFirstPerson` (id 40476), called on the
/// main thread at every switch; store it, then [`hook::Hook::enable`].
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_set_first_person(detour: SetFirstPersonFn) -> Result<hook::Hook<SetFirstPersonFn>, hook::Error> {
    create(sre::id!(fns::PlayerCharacter__SetFirstPerson), detour)
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
    create(sre::id!(fns::BSShaderAccumulator__RegisterDepthGeometry), detour)
}

/// Creates, disabled, a detour on `ProcessCullObjectList` (id 76561): the replay of a geometry
/// list into a culler, which the sun's cascades run on the frame thread over the lists its walk
/// built; with `skipHidden` an entry whose own hidden bit is set is dropped without `NiAVObject::Cull`.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_process_cull_object_list(detour: ProcessCullObjectListFn) -> Result<hook::Hook<ProcessCullObjectListFn>, hook::Error> {
    create(sre::id!(fns::ProcessCullObjectList), detour)
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

/// `VolumetricLightingParams` (`g_VolumetricLighting`, `graphics.md` "Request IC10"): the range
/// factor, the repartition function pair `float (float x, float range)` the tables are filled
/// through, and the two 4096-texel tables.
fn volumetric_functions() -> [usize; 2] {
    [sre::member_offset!("VolumetricLightingParams.repartitionFunction"), sre::member_offset!("VolumetricLightingParams.inverseRepartitionFunction")]
}

fn volumetric_tables() -> [usize; 2] {
    [sre::member_offset!("VolumetricLightingParams.repartitionTable"), sre::member_offset!("VolumetricLightingParams.inverseRepartitionTable")]
}

/// The tables' half texel and last texel centre.
const HALF_TEXEL: f32 = 1.0 / 8192.0;
const LAST_TEXEL: f32 = 4095.0 / 4096.0;

/// `reference / near` as bits, 0 while the engine's functions are in place.
static VOLUMETRIC_SCALE: AtomicU32 = AtomicU32::new(0);
static VOLUMETRIC_ORIGINALS: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

fn volumetric_scale() -> f32 {
    match VOLUMETRIC_SCALE.load(Ordering::Relaxed) {
        0 => 1.0,
        bits => f32::from_bits(bits),
    }
}

/// Depth to slice: the engine's `pow(x, r)` taken at the depth `x` has under the reference near;
/// the last texel keeps the engine's sky value.
extern "C" fn near_repartition(x: f32, range: f32) -> f32 {
    let x = if x >= LAST_TEXEL { LAST_TEXEL } else { (1.0 - volumetric_scale() * (1.0 - x - HALF_TEXEL) - HALF_TEXEL).clamp(0.0, LAST_TEXEL) };
    x.powf(range)
}

/// Slice to depth: the engine's `pow(s, 1/r)` moved so slice `s` lies at its distance under the
/// reference near.
extern "C" fn near_inverse_repartition(s: f32, range: f32) -> f32 {
    1.0 - (1.0 - s.powf(1.0 / range)) / volumetric_scale()
}

fn volumetric_field<T>(offset: usize) -> *mut T {
    core::ptr::from_ref(sre::g_volumetric_lighting()).cast_mut().wrapping_byte_add(offset).cast()
}

/// Whether both tables exist, so a refill has something to fill.
fn volumetric_tables_exist() -> bool {
    volumetric_tables().iter().all(|&t| unsafe { volumetric_field::<usize>(t).read_volatile() } != 0)
}

fn refill_volumetric_tables() {
    let range = unsafe { volumetric_field::<f32>(sre::member_offset!("VolumetricLightingParams.rangeFactor")).read_volatile() };
    unsafe { sre::set_volumetric_range_factor(core::ptr::from_ref(sre::g_volumetric_lighting()).cast_mut(), range) };
}

/// Keeps volumetric lighting at the look it has under the `reference` near while the world draws
/// with `near`: the engine lays its slices at fixed depth values, so their distances scale with
/// near and a lower near hazes the frame. Swaps in near-compensated repartition functions on the
/// first call and refills the tables when the ratio changes. Main thread, before
/// `BuildVolumetricLightingVolume`; [`release_volumetric_near`] before the functions' module unloads.
pub fn compensate_volumetric_near(reference: f32, near: f32) {
    let scale = if near > 0.0 { reference / near } else { 1.0 };
    let installed = VOLUMETRIC_SCALE.load(Ordering::Relaxed);
    if installed == scale.to_bits() || (installed == 0 && scale.to_bits() == 1f32.to_bits()) || !volumetric_tables_exist() {
        return;
    }
    VOLUMETRIC_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    if installed == 0 {
        let ours = [near_repartition as *const () as usize, near_inverse_repartition as *const () as usize];
        for ((offset, original), function) in volumetric_functions().into_iter().zip(&VOLUMETRIC_ORIGINALS).zip(ours) {
            let slot = volumetric_field::<usize>(offset);
            original.store(unsafe { slot.read_volatile() }, Ordering::Relaxed);
            unsafe { slot.write_volatile(function) };
        }
    }
    refill_volumetric_tables();
}

/// Puts the engine's repartition functions back and refills the tables with them.
pub fn release_volumetric_near() {
    if VOLUMETRIC_SCALE.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    for (offset, original) in volumetric_functions().into_iter().zip(&VOLUMETRIC_ORIGINALS) {
        unsafe { volumetric_field::<usize>(offset).write_volatile(original.load(Ordering::Relaxed)) };
    }
    if volumetric_tables_exist() {
        refill_volumetric_tables();
    }
}

/// The engine's `j` in the depth shift's constants `kDepthShiftMaxDepth` = 1 - j 2^-23 and
/// `kDepthShiftFarScale` = 1 + j 2^-23, two adjacent `.rdata` floats (`graphics.md` "Request IC11").
const DEPTH_SHIFT_STEPS: u32 = 17;

static DEPTH_SHIFT_WRITTEN: AtomicU32 = AtomicU32::new(DEPTH_SHIFT_STEPS);

fn depth_shift_constants(steps: u32) -> [u8; 8] {
    #[allow(clippy::cast_precision_loss)]
    let e = steps as f32 * f32::EPSILON;
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&(1.0 - e).to_le_bytes());
    bytes[4..].copy_from_slice(&(1.0 + e).to_le_bytes());
    bytes
}

fn write_depth_shift(steps: u32) {
    let at = sre::k_depth_shift_max_depth();
    let current = unsafe { at.cast::<[u8; 8]>().read_volatile() };
    if current != depth_shift_constants(DEPTH_SHIFT_WRITTEN.load(Ordering::Relaxed)) {
        return;
    }
    if unsafe { hook::write_code(at as usize, &depth_shift_constants(steps)) }.is_ok() {
        DEPTH_SHIFT_WRITTEN.store(steps, Ordering::Relaxed);
    }
}

/// Keeps what the depth shift does to reconstructed positions as at the `reference` near while
/// the world draws with `near`: the world is drawn under the shift's narrowed depth range and the
/// shadow mask, SAO, SSR and DOF rebuild positions with the unshifted projection, pulling every
/// surface toward the camera by about (1 - max depth) z^2 / near, so a lower near loses shadows.
/// Scales the shift's constants with near; left alone when another module changed them. Main
/// thread, before `RenderWorld`; [`release_depth_shift`] before the caller's module unloads.
pub fn compensate_depth_shift(reference: f32, near: f32) {
    let steps = if near > 0.0 && reference > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let steps = (DEPTH_SHIFT_STEPS as f32 * near / reference).round() as u32;
        steps.max(1)
    } else {
        DEPTH_SHIFT_STEPS
    };
    if DEPTH_SHIFT_WRITTEN.load(Ordering::Relaxed) != steps {
        write_depth_shift(steps);
    }
}

/// Puts the engine's depth shift constants back.
pub fn release_depth_shift() {
    write_depth_shift(DEPTH_SHIFT_STEPS);
}

/// The viewport depth pulls per rasterizer depth-bias mode (id 411359, `.data`, copied from
/// `.rdata` at renderer init): `ApplyDirtyRenderStates` lowers the viewport's maximum depth by the
/// bound mode's pull, so decals (modes 6-9) draw in front of the surface they lie on
/// (`graphics.md` "Request IC12"). The engine's values, as bits.
const DEPTH_BIAS_PULLS: u64 = 411_359;
const ENGINE_DEPTH_BIAS_PULLS: [u32; 12] = [0, 0x377b_a882, 0, 0, 0, 0, 0x37da_1a93, 0x377b_a882, 0x37ea_e18b, 0x378e_9b39, 0x37fb_a882, 0x379f_6230];

static DEPTH_BIAS_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000);

fn depth_bias_pulls(scale: f32) -> [f32; 12] {
    ENGINE_DEPTH_BIAS_PULLS.map(|bits| f32::from_bits(bits) * scale)
}

/// Writes the pulls scaled by `scale` unless another module changed them; the engine refilling
/// its own values (renderer init or reset) counts as unchanged.
fn write_depth_bias(scale: f32) {
    let Ok(at) = crate::reloc().address(sre::id!(DEPTH_BIAS_PULLS)) else { return };
    let table = at as *mut [f32; 12];
    let bits = |pulls: [f32; 12]| pulls.map(f32::to_bits);
    let current = bits(unsafe { table.read_volatile() });
    let written = bits(depth_bias_pulls(f32::from_bits(DEPTH_BIAS_SCALE.load(Ordering::Relaxed))));
    let wanted = depth_bias_pulls(scale);
    if current == bits(wanted) {
        DEPTH_BIAS_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    } else if current == written || current == ENGINE_DEPTH_BIAS_PULLS {
        unsafe { table.write_volatile(wanted) };
        DEPTH_BIAS_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    }
}

/// Keeps the decals' pull toward the camera as at the `reference` near while the world draws with
/// `near`: a fixed pull in depth is about pull z^2 / near in distance, so at a lower near decals on
/// far terrain jump through the cloud and mist meshes in front of it. Scales the pulls with near.
/// Main thread, before `RenderWorld`; [`release_depth_bias`] before the caller's module unloads.
pub fn compensate_depth_bias(reference: f32, near: f32) {
    let scale = if near > 0.0 && reference > 0.0 { near / reference } else { 1.0 };
    write_depth_bias(scale);
}

/// Puts the engine's depth-bias pulls back.
pub fn release_depth_bias() {
    write_depth_bias(1.0);
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
    setting.wrapping_byte_add(sre::member_offset!("Setting.value")).cast()
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
    let base = core::ptr::from_ref(sre::g_frame_state());
    sre::fixed_size!("CameraMatrixEntry", CameraMatrixEntry);
    let entries = unsafe { &*base.byte_add(sre::member_offset!("FrameState.cameraMatrices")).cast::<sre::BSTArray>() };
    if entries.data.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(entries.data.cast(), entries.size as usize) } }
}

/// `g_ShadowSceneNode[index]` (id 390951), `None` while the slot is empty; index 0 is the world.
#[must_use]
pub fn shadow_scene_node(index: usize) -> Option<&'static ShadowSceneNode> {
    let slots = crate::reloc().address(sre::id!(sre::raw::globals::g_ShadowSceneNode)).ok()? as *const [*const ShadowSceneNode; 4];
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

/// `a * b`.
#[must_use]
pub fn mul3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
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
/// off the poles; at a pole (`|m02| >= 1`) `y` is +-pi/2, `z` 0 and `x` is `atan2(-m21, m11)`,
/// which builds `m` again where the engine's own `x` has the opposite sign (RE `camera.md` IC9 G).
#[must_use]
pub fn to_euler_xyz(m: &Matrix3) -> [f32; 3] {
    let y = -m[0][2].clamp(-1.0, 1.0).asin();
    if m[0][2].abs() >= 1.0 { [(-m[2][1]).atan2(m[1][1]), y, 0.0] } else { [m[1][2].atan2(m[2][2]), y, m[0][1].atan2(m[0][0])] }
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
/// `vtables` must list only vtables of `Self` and the classes derived from it.
pub unsafe trait NodeClass: 'static {
    fn vtables() -> &'static [u64];
}

macro_rules! node_classes {
    ($($class:ty: $($vtable:ident),+;)*) => {
        $(
            unsafe impl NodeClass for $class {
                fn vtables() -> &'static [u64] {
                    $(sre::id!(vtables::$vtable::ID);)+
                    &[$(vtables::$vtable::ID),+]
                }
            }
        )*
    };
}

node_classes! {
    NiNode: NiNode_vtable, BSFadeNode_vtable, BSFlattenedBoneTree_vtable, BSFaceGenNiNode_vtable, BSLeafAnimNode_vtable, BSTreeNode_vtable, BSMultiBoundNode_vtable, BSMultiBoundRoom_vtable,
        BSOrderedNode_vtable, BSBlastNode_vtable, BSDamageStage_vtable, BSValueNode_vtable, NiBillboardNode_vtable, NiSwitchNode_vtable, BSPortalSharedNode_vtable, ShadowSceneNode_vtable;
    BSFadeNode: BSFadeNode_vtable;
    BSFlattenedBoneTree: BSFlattenedBoneTree_vtable;
    BSGeometry: BSGeometry_vtable, BSTriShape_vtable, BSDynamicTriShape_vtable, BSSubIndexTriShape_vtable, BSMeshLODTriShape_vtable, BSLODTriShape_vtable, BSLines_vtable;
}

pub trait NiAVObjectExt {
    /// The object as a `T` when its vtable is one of `T`'s, so `object.cast::<sre::NiNode>()` is a
    /// node or `None`.
    fn cast<T: NodeClass>(&self) -> Option<&T>;
    /// Sets or clears the hidden bit. The frame itself toggles this bit on the player's first-person
    /// root between its steps, so a mod changes it on the main thread between frames or on the
    /// player's third-person root only.
    fn set_hidden(&self, hidden: bool);
    /// Whether the hidden bit is set.
    fn is_hidden(&self) -> bool;
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
        T::vtables().contains(&id).then(|| unsafe { &*core::ptr::from_ref(self).cast::<T>() })
    }

    fn set_hidden(&self, hidden: bool) {
        let flags = unsafe { AtomicU32::from_ptr((&raw const self.flags).cast_mut()) };
        if hidden {
            flags.fetch_or(HIDDEN, Ordering::Relaxed);
        } else {
            flags.fetch_and(!HIDDEN, Ordering::Relaxed);
        }
    }

    fn is_hidden(&self) -> bool {
        unsafe { AtomicU32::from_ptr((&raw const self.flags).cast_mut()) }.load(Ordering::Relaxed) & HIDDEN != 0
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
/// number of geometries. Main thread, where the temporary-effects update that owns `data` runs too
/// (RE `camera.md` IC9 F).
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

pub use sre::FlatBone;

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
        sre::fixed_size!("FlatBone", FlatBone);
        unsafe { core::slice::from_raw_parts(self.bones.cast::<FlatBone>(), self.boneCount as usize) }
    }

    fn rechain_bones(&self) {
        let bones = BSFlattenedBoneTreeExt::bones(self);
        for bone in bones {
            let world = bone.node().map_or_else(
                || {
                    let parent = usize::try_from(bone.parentIndex.cast_signed()).ok().and_then(|i| bones.get(i)).map_or(&self.world, |p| &p.world);
                    Transform::from(parent).compose(&Transform::from(&bone.local))
                },
                |node| node.world_transform(),
            );
            unsafe { (&raw const bone.world).cast::<Transform>().cast_mut().write_volatile(world) }
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
        sre::fixed_size!("ShadowMapData", ShadowMapData);
        unsafe { core::slice::from_raw_parts(maps.data.cast::<ShadowMapData>(), maps.size as usize) }
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn focus_shadow_maps(&self) -> &[ShadowMapData; 4] {
        sre::fixed_size!("ShadowMapData", ShadowMapData);
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
        crate::class_of(self).is_some_and(|c| c.id == sre::id!(vtables::BSGeometryListCullingProcess_vtable::ID))
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
        (!data.is_null()).then(|| unsafe { data.cast::<u8>().add(sre::member_offset!("ShaderPropertyLightData.receiverMask")).cast::<u32>().read_unaligned() })
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
        if crate::class_of(self)?.id != sre::id!(vtables::BSShaderTextureSet_vtable::ID) {
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
    let play: PlayFn = unsafe { crate::engine_fn(sre::id!(fns::CreateModelReferenceEffect)) };
    unsafe { play(target, art, duration, core::ptr::null(), false, false, core::ptr::null(), false) };
}

/// Holds an effect shader on `target` as Papyrus `EffectShader.Play` does
/// (`PlayShaderReferenceEffect`, id 19872). Main thread.
pub fn play_effect_shader(target: &TESObjectREFR, shader: &TESEffectShader, duration: f32) {
    type PlayFn = unsafe extern "C" fn(*const TESObjectREFR, *const TESEffectShader, f32, *const TESObjectREFR, bool, bool, *const NiAVObject, bool);
    let play: PlayFn = unsafe { crate::engine_fn(sre::id!(fns::PlayShaderReferenceEffect)) };
    unsafe { play(target, shader, duration, core::ptr::null(), false, false, core::ptr::null(), false) };
}

/// Whether the object's class is `node` or derives from it: the type node of slot 2, walked up
/// through the parent at +8 (RE `camera.md` IC8 E and F).
fn derives_from<T>(object: &T, node: *const u8) -> bool {
    type TypeFn = unsafe extern "C" fn(*const c_void) -> *const u8;
    let get: TypeFn = unsafe { core::mem::transmute(sre::vtable_entry(object, sre::slot_index!("NiObject", 0x0, 2))) };
    let mut current = unsafe { get(core::ptr::from_ref(object).cast()) };
    while !current.is_null() {
        if current == node {
            return true;
        }
        current = unsafe { current.add(sre::member_offset!("TypeNode.parent")).cast::<*const u8>().read_unaligned() };
    }
    false
}

/// Ends the art object (`art`) or effect shader effects of form `form_id` on the reference with
/// handle `target`, as the engine ends one: their `finished` flag set, under the effect list's
/// lock (RE `camera.md` IC8 F). Main thread.
pub fn stop_reference_effects(target: u32, form_id: u32, art: bool) {
    let Ok(holder) = crate::reloc().address(sre::id!(sre::raw::globals::g_ProcessLists_ptr)) else { return };
    let lists = unsafe { *(holder as *const *const sre::ProcessLists) };
    if lists.is_null() {
        return;
    }
    let _guard = crate::spin::SpinGuard::lock(unsafe { &*lists.byte_add(sre::member_offset!("ProcessLists.referenceEffectsLock")).cast::<sre::BSSpinLock>() });
    let effects = unsafe { &*lists.byte_add(sre::member_offset!("ProcessLists.referenceEffects")).cast::<sre::BSTArray>() };
    let (data, count) = (effects.data.cast_const(), effects.size);
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
        LightValues { color: [self.diffuse.r, self.diffuse.g, self.diffuse.b], fade: self.fade, radius: self.radius.x() }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn set_values(&self, values: LightValues) {
        let [r, g, b] = values.color;
        for (field, value) in [(&self.diffuse.r, r), (&self.diffuse.g, g), (&self.diffuse.b, b), (&self.radius.y, values.radius), (&self.fade, values.fade)] {
            crate::store(field, value);
        }
        unsafe {
            self.radius.x_ptr().write_volatile(values.radius);
            self.radius.z_ptr().write_volatile(values.radius);
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
        let name = self.as_tes_full_name().name;
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
    use super::{Matrix3, Transform, axis_turn, depth_bias_pulls, depth_shift_constants, from_euler_xyz, from_euler_zxy, mul3, near_inverse_repartition, near_repartition, to_euler_xyz};

    #[test]
    fn the_depth_bias_pulls_scale_with_near() {
        let engine = depth_bias_pulls(1.0);
        assert_eq!(engine.map(f32::to_bits), super::ENGINE_DEPTH_BIAS_PULLS);
        assert!((engine[6] - 2.6e-5).abs() < 1e-9);
        let two = depth_bias_pulls(2.0 / 15.0);
        assert!((two[6] - engine[6] * 2.0 / 15.0).abs() < 1e-12);
        assert_eq!(two[0].to_bits(), 0);
    }

    #[test]
    fn the_engine_depth_shift_constants_come_back_at_seventeen_steps() {
        let engine = [0x3f7f_ffdeu32.to_le_bytes(), 0x3f80_0011u32.to_le_bytes()].concat();
        assert_eq!(depth_shift_constants(super::DEPTH_SHIFT_STEPS).as_slice(), engine.as_slice());
        let two = depth_shift_constants(2);
        assert_eq!(f32::from_le_bytes(two[..4].try_into().unwrap()), 1.0 - 2.0 * f32::EPSILON);
        assert_eq!(f32::from_le_bytes(two[4..].try_into().unwrap()), 1.0 + 2.0 * f32::EPSILON);
    }

    #[test]
    fn volumetric_slices_keep_their_distance_under_the_reference_near() {
        use core::sync::atomic::Ordering;
        let range = 40.0;
        let distance = |depth: f32, near: f32| near / (1.0 - depth);
        super::VOLUMETRIC_SCALE.store(1f32.to_bits(), Ordering::Relaxed);
        for x in [0.1, 0.5, 0.9, 0.999] {
            assert!((near_repartition(x, range) / x.powf(range) - 1.0).abs() < 1e-4, "{x}");
            assert!((near_inverse_repartition(x, range) - x.powf(1.0 / range)).abs() < 1e-6);
        }
        super::VOLUMETRIC_SCALE.store((15f32 / 2.0).to_bits(), Ordering::Relaxed);
        for s in [0.1f32, 0.5, 0.9] {
            let engine = distance(s.powf(1.0 / range), 15.0);
            let ours = distance(near_inverse_repartition(s, range), 2.0);
            assert!((ours / engine - 1.0).abs() < 1e-3, "{s}: {ours} vs {engine}");
            let engine_lookup = (s.powf(1.0 / range) - super::HALF_TEXEL).powf(range);
            let ours = near_repartition(near_inverse_repartition(s, range) - super::HALF_TEXEL, range);
            assert!((ours / engine_lookup - 1.0).abs() < 1e-3, "{s}: {ours} vs {engine_lookup}");
        }
        super::VOLUMETRIC_SCALE.store(0, Ordering::Relaxed);
    }

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
            for z in [0.0, 0.9] {
                let mut pole = from_euler_xyz(0.4, y, z);
                pole[0][2] = pole[0][2].signum();
                let back = to_euler_xyz(&pole);
                assert_eq!((back[1], back[2]), (y, 0.0));
                assert!((back[0] - (-pole[2][1]).atan2(pole[1][1])).abs() < 1e-6, "IC9 G's x");
                assert!(close(&from_euler_xyz(back[0], back[1], back[2]), &pole), "{y} {z}: {back:?}");
            }
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
