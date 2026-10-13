//! Renderer (subsystem pass 5): the scene graph node and its flags, the world `ShadowSceneNode`
//! with its light lists, the sun's `BSShadowDirectionalLight` and its cascade records, the
//! culling process a node is visited by, the player camera's state, the render frame counter and
//! the two hooks a first-person shadow mod needs (`NiAVObject::Cull` and
//! `PlayerCharacter::SetFirstPerson`).
//!
//! Reads follow the pass's thread model: light lists, roots and flags are stable while the frame
//! culls (the main thread changes them between frames); a [`CullFn`] detour runs on the job
//! threads concurrently with the main camera cull and must stay lock-free and read-only apart from
//! the call it forwards. Offsets of the transforms, the bounding sphere, the children array and
//! the viewport are the byte layouts `render.json` gives for its `undefined` fields.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};
use std::fmt;

use layout::v1_7_104::{classes, enums, fns, globals, vtables};

pub use enums::CameraState;
pub use fns::PlayerCharacter__SetFirstPerson_fn as SetFirstPersonFn;

/// `NiAVObject::Cull(node, culler, planeMask)`. The pass's signature has two parameters; the
/// function leaves `r8` untouched and tail-jumps to the culler's slot 22 `CullNode(culler, node,
/// planeMask)`, so the mask is a third argument a detour must pass through (a two-argument detour
/// clobbers it and the culls read nonsense).
pub type CullFn = unsafe extern "C" fn(node: *mut classes::NiAVObject, culler: *mut classes::partial::NiCullingProcess, plane_mask: u32);

/// `NiAVObject::flags` bit 0: no culling process visits the object.
pub const HIDDEN: u32 = 1;

pub(crate) struct Fns {
    scene_nodes: *const [*mut classes::ShadowSceneNode; 4],
    frame_counter: *const AtomicU32,
    player_camera: *const *const classes::partial::PlayerCamera,
    cull: usize,
    set_first_person: usize,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        Ok(Self {
            scene_nodes: reloc.address(globals::g_ShadowSceneNode)? as *const [*mut classes::ShadowSceneNode; 4],
            frame_counter: reloc.address(globals::g_RenderFrameCounter)? as *const AtomicU32,
            player_camera: reloc.address(globals::g_PlayerCamera_ptr)? as *const *const classes::partial::PlayerCamera,
            cull: reloc.address(fns::NiAVObject__Cull)?,
            set_first_person: reloc.address(fns::PlayerCharacter__SetFirstPerson)?,
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

/// Creates, disabled, a detour on `NiAVObject::Cull` (id 70267), the gate every culling process
/// passes a node through. Store the hook where the detour finds its trampoline, then
/// [`hook::Hook::enable`] it: the game culls on every frame from the main menu on, so a detour
/// enabled before it can reach the trampoline is called at once.
///
/// The detour runs on the job threads during the shadow and main camera culls, concurrently, and
/// during loads while the scene is rebuilt; it must forward to [`hook::Hook::original`] for every
/// node it does not skip and must not dereference objects the main thread may be replacing.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_cull(detour: CullFn) -> Result<hook::Hook<CullFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().cull, detour) }
}

/// Creates, disabled, a detour on `PlayerCharacter::SetFirstPerson` (id 40476), called on the
/// main thread at every switch; store it, then [`hook::Hook::enable`].
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_set_first_person(detour: SetFirstPersonFn) -> Result<hook::Hook<SetFirstPersonFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().set_first_person, detour) }
}

/// Three floats at `bytes[0..12]`.
fn vec3(bytes: &[u8]) -> [f32; 3] {
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
        pub struct $name(classes::$layout);

        impl $name {
            #[must_use]
            pub fn raw(&self) -> &classes::$layout {
                &self.0
            }

            #[must_use]
            pub fn as_ptr(&self) -> *const classes::$layout {
                &raw const self.0
            }

            /// The wrapper over a live object.
            ///
            /// # Safety
            /// `p` must be null or point to a live object of this class (or a derived one) that outlives `'a`.
            #[must_use]
            pub unsafe fn from_ptr<'a>(p: *const classes::$layout) -> Option<&'a Self> {
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
pub struct NiAVObject(classes::NiAVObject);

impl NiAVObject {
    #[must_use]
    pub fn raw(&self) -> &classes::NiAVObject {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const classes::NiAVObject {
        &raw const self.0
    }

    /// # Safety
    /// `p` must be null or point to a live `NiAVObject` (or derived object) that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::NiAVObject) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    /// The object's class from its vtable pointer.
    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
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

    #[must_use]
    pub fn local(&self) -> Transform {
        Transform::from_bytes(&self.0.local)
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
    /// The children, null entries included; data at +0x118 and the u16 count at +0x122 of the array.
    #[must_use]
    pub fn children(&self) -> &[Option<&NiAVObject>] {
        let b = &self.0.children;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let size = u16::from_le_bytes([b[18], b[19]]);
        unsafe { crate::forms::ptr_slice(data, u32::from(size)) }
    }
}

impl BSGeometry {
    #[must_use]
    pub fn skin_instance(&self) -> Option<&classes::NiSkinInstance> {
        unsafe { self.0.skinInstance.as_ref() }
    }

    #[must_use]
    pub fn shader_property(&self) -> Option<&classes::BSShaderProperty> {
        unsafe { self.0.shaderProperty.as_ref() }
    }

    /// Geometry type byte, 3 for `BSTriShape`.
    #[must_use]
    pub fn geometry_type(&self) -> u8 {
        self.0.type_
    }
}

/// A light the scene node holds: the `NiLight` it wraps and its position.
#[repr(transparent)]
pub struct BSLight(classes::BSLight);

impl BSLight {
    #[must_use]
    pub fn raw(&self) -> &classes::BSLight {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const classes::BSLight {
        &raw const self.0
    }

    fn vtable(&self) -> &vtables::BSLight_vtable {
        unsafe { &*self.0.__vftable }
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
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
pub struct BSShadowLight(classes::BSShadowLight);

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
    pub fn raw(&self) -> &classes::BSShadowLight {
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
    pub fn owns_camera(&self, camera: *const classes::NiCamera) -> bool {
        self.shadow_maps().iter().chain(self.focus_shadow_maps()).any(|m| core::ptr::eq(m.raw().camera, camera))
    }
}

/// The sun: a shadow light with a direction, a light camera and the geometry list cullers.
#[repr(transparent)]
pub struct BSShadowDirectionalLight(classes::BSShadowDirectionalLight);

impl core::ops::Deref for BSShadowDirectionalLight {
    type Target = BSShadowLight;

    fn deref(&self) -> &BSShadowLight {
        unsafe { &*(&raw const self.0).cast::<BSShadowLight>() }
    }
}

impl BSShadowDirectionalLight {
    #[must_use]
    pub fn raw(&self) -> &classes::BSShadowDirectionalLight {
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
    pub fn is_shadow_camera(&self, camera: *const classes::NiCamera) -> bool {
        core::ptr::eq(self.0.lightCamera, camera) || self.owns_camera(camera)
    }
}

/// One shadow map of a light: camera, accumulator, depth target slice, viewport and culler.
#[repr(transparent)]
pub struct ShadowMapData(classes::ShadowMapData);

impl ShadowMapData {
    #[must_use]
    pub fn raw(&self) -> &classes::ShadowMapData {
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
    pub fn accumulator(&self) -> Option<&classes::BSShaderAccumulator> {
        unsafe { self.0.accumulator.as_ref() }
    }
}

/// The culling process a [`CullFn`] detour receives: every culler of the frame is a
/// `BSCullingProcess` (or its geometry-list subclass), whose `camera` names the pass.
#[repr(transparent)]
pub struct CullingProcess(classes::BSCullingProcess);

impl CullingProcess {
    #[must_use]
    pub fn raw(&self) -> &classes::BSCullingProcess {
        &self.0
    }

    /// # Safety
    /// `p` must be null or point to a live culling process that outlives `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::partial::NiCullingProcess) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
        unsafe { crate::class_of((&raw const self.0).cast()) }
    }

    #[must_use]
    pub fn camera_ptr(&self) -> *const classes::NiCamera {
        self.0.camera
    }

    #[must_use]
    pub fn camera(&self) -> Option<&NiCamera> {
        unsafe { NiCamera::from_ptr(self.0.camera) }
    }

    #[must_use]
    pub fn accumulator(&self) -> Option<&classes::BSShaderAccumulator> {
        unsafe { self.0.accumulator.cast::<classes::BSShaderAccumulator>().as_ref() }
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
}

/// A scene's lights, shadow lights and sun; `g_ShadowSceneNode[0]` is the world.
#[repr(transparent)]
pub struct ShadowSceneNode(classes::ShadowSceneNode);

impl ShadowSceneNode {
    #[must_use]
    pub fn raw(&self) -> &classes::ShadowSceneNode {
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

/// The player camera: its node and the active state among the 13 at +0xb8.
#[repr(transparent)]
pub struct PlayerCamera(classes::partial::PlayerCamera);

impl PlayerCamera {
    #[must_use]
    pub fn raw(&self) -> &classes::partial::PlayerCamera {
        &self.0
    }

    /// The camera behind `g_PlayerCamera_ptr` (id 400802), `None` before it is constructed.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().player_camera).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn camera_root(&self) -> Option<&NiNode> {
        unsafe { self.0.cameraRoot.cast::<NiNode>().as_ref() }
    }

    #[must_use]
    pub fn current_state_ptr(&self) -> *const c_void {
        self.0.currentState
    }

    /// The state object for `state` (the 13 pointers at +0xb8, in `CameraState` order).
    #[must_use]
    pub fn state_ptr(&self, state: CameraState) -> *const c_void {
        const _: () = assert!(core::mem::offset_of!(classes::partial::PlayerCamera, field_B8) == 0xb8);
        let base = (&raw const self.0.field_B8).cast::<*const c_void>();
        unsafe { base.add(state as usize).read() }
    }

    /// Which of the 13 states is current, `None` when the pointer matches none.
    #[must_use]
    pub fn state(&self) -> Option<CameraState> {
        let current = self.current_state_ptr();
        (0..13u32).filter_map(CameraState::from_u32).find(|&s| core::ptr::eq(self.state_ptr(s), current))
    }
}
