//! Animation (subsystem pass 7): the pose write and world update hooks, a reference's graph event
//! source, and what the generated holder, manager and graph methods leave `unsafe`: the graph
//! variables a getter writes into an out-parameter, the graphs and bone nodes as slices, the pose
//! the behaviour graph produced, and a graph from the bone array the pose hook receives; an
//! actor's face data, its input channels, expression override, eyes and blink (RE `camera.md` IC8 H).
//!
//! The pose is written on a job worker thread by `ApplyPoseToBoneNodes`, once per graph and
//! frame, while the main thread draws; the world update of the skeleton runs after the draws, so
//! every frame draws the previous frame's pose. A bone driver post-hooks the pose write, edits a
//! bone's local transform and calls nothing: the world update picks it up and the next frame's
//! culls and skin palettes use it, with the same one-frame latency the animation itself has.
//! Nothing on the animation path may call `NiAVObjectExt::update_transforms` or touch other actors' nodes.

use core::ffi::c_void;

use sre::raw::fns;
use sre::{Actor, BSAnimationGraphEvent, BSAnimationGraphManager, BSFaceGenAnimationData, BSFaceGenKeyframeMultiple, BShkbAnimationGraph, FaceBlinkStage, IAnimationGraphManagerHolder, NiAVObject, TESObjectREFR};

use crate::BSFixedString;
use crate::events::BSTEventSource;
use crate::ptr_slice;

pub use fns::PlayerCharacter__UpdateNodesAfterRender_fn as UpdateNodesAfterRenderFn;

/// `ApplyPoseToBoneNodes(pose, nodes, count)` (id 63856): the only writer of animated bone locals.
/// `nodes` is the graph's bone node array at +0x160; [`BShkbAnimationGraphExt::from_bone_array`] recovers the graph.
pub type ApplyPoseFn = unsafe extern "C" fn(pose: *const HkQsTransform, nodes: *mut c_void, count: i32);

/// A Havok transform: translation, rotation quaternion `[x, y, z, w]` and scale, each padded to four floats.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HkQsTransform {
    pub translation: [f32; 4],
    pub rotation: [f32; 4],
    pub scale: [f32; 4],
}

const _: () = assert!(size_of::<HkQsTransform>() == 0x30);

pub use sre::BoneNodeEntry;


/// Creates, disabled, a post-hook target on `ApplyPoseToBoneNodes`; store it, then
/// [`hook::Hook::enable`]. The detour runs on a job worker thread while the main thread draws:
/// call the original first, then edit bone locals of the graph's own nodes and nothing else.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_apply_pose(detour: ApplyPoseFn) -> Result<hook::Hook<ApplyPoseFn>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(sre::id!(fns::ApplyPoseToBoneNodes))?, detour) }
}

/// Creates, disabled, a detour on `PlayerCharacter::UpdateNodesAfterRender` (id 40522): the world
/// update of the player's roots on a job thread after the frame's draws.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_update_nodes_after_render(detour: UpdateNodesAfterRenderFn) -> Result<hook::Hook<UpdateNodesAfterRenderFn>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(sre::id!(fns::PlayerCharacter__UpdateNodesAfterRender))?, detour) }
}

/// The event source of a reference's graph through `TESObjectREFR::GetAnimationGraphEventSource`:
/// `None` for the active graph, `Some(i)` for graph `i` (graph 0 sends in first and third person).
/// The registration dies with the manager (3D unload).
#[must_use]
pub fn graph_event_source(refr: &TESObjectREFR, graph: Option<u32>) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>> {
    let mut out: *mut sre::BSTEventSource = core::ptr::null_mut();
    let found = unsafe { refr.get_animation_graph_event_source((&raw mut out).cast(), graph.unwrap_or(u32::MAX)) };
    if found { unsafe { BSTEventSource::from_ptr(out) } } else { None }
}

pub trait IAnimationGraphManagerHolderExt {
    /// Slot 16: a float graph variable of the active graph (an actor reads its cache first).
    fn variable_float(&self, name: &BSFixedString) -> Option<f32>;
    /// Slot 17: an int graph variable.
    fn variable_int(&self, name: &BSFixedString) -> Option<i32>;
    /// Slot 18: a bool graph variable.
    fn variable_bool(&self, name: &BSFixedString) -> Option<bool>;
}

impl IAnimationGraphManagerHolderExt for IAnimationGraphManagerHolder {
    fn variable_float(&self, name: &BSFixedString) -> Option<f32> {
        let mut out = 0.0f32;
        unsafe { self.get_graph_variable_float(name.raw(), &raw mut out) }.then_some(out)
    }

    fn variable_int(&self, name: &BSFixedString) -> Option<i32> {
        let mut out = 0i32;
        unsafe { self.get_graph_variable_int(name.raw(), &raw mut out) }.then_some(out)
    }

    fn variable_bool(&self, name: &BSFixedString) -> Option<bool> {
        let mut out = false;
        unsafe { self.get_graph_variable_bool(name.raw(), &raw mut out) }.then_some(out)
    }
}

pub trait BSAnimationGraphManagerExt {
    /// The graphs (data at +0x48, count at +0x50): two for the player, third person first.
    fn graphs(&self) -> &[Option<&BShkbAnimationGraph>];
    /// The graph at `activeGraph` (0 in third person, 1 in first person on the player).
    fn active_graph(&self) -> Option<&BShkbAnimationGraph>;
    /// Disables or enables graph 0's foot IK driver through
    /// `BSAnimationGraphManager::SetGraph0FootIkDisabled` (id 40846), which holds the manager's
    /// spin lock (+0xa0) the animation update holds: disabling frees the per-leg state the foot IK
    /// run reads. The engine disables it on every switch to first person and graph re-attach
    /// (RE `camera.md` request IC7 B), so a mod that keeps it on applies again after those.
    fn set_foot_ik_disabled(&self, disabled: bool);
}

impl BSAnimationGraphManagerExt for BSAnimationGraphManager {
    fn graphs(&self) -> &[Option<&BShkbAnimationGraph>] {
        let b = &self.graphs;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let count = u32::from_le_bytes(b[16..20].try_into().unwrap());
        unsafe { ptr_slice(data, count) }
    }

    fn active_graph(&self) -> Option<&BShkbAnimationGraph> {
        self.graphs().get(self.activeGraph() as usize).copied().flatten()
    }

    fn set_foot_ik_disabled(&self, disabled: bool) {
        let mut value = disabled;
        let mut pointer = &raw mut value;
        unsafe { BSAnimationGraphManager::set_graph0_foot_ik_disabled(self, &raw mut pointer) };
    }
}

pub trait BShkbAnimationGraphExt {
    /// The graph whose bone node array `nodes` is, as handed to [`ApplyPoseFn`].
    ///
    /// # Safety
    /// `nodes` must be the +0x160 array of a live `BShkbAnimationGraph`.
    unsafe fn from_bone_array<'a>(nodes: *mut c_void) -> Option<&'a Self>;
    /// The bone node array the pose is written into, in skeleton bone order.
    fn bone_nodes(&self) -> &[BoneNodeEntry];
    /// The bone node at skeleton index `i`.
    fn bone_node(&self, i: usize) -> Option<&NiAVObject>;
    /// The parent-local pose the behaviour graph produced this frame (`hkbCharacter.poseLocal`).
    fn pose_local(&self) -> &[HkQsTransform];
    /// Bones of the Havok skeleton (`hkbCharacterSetup` +0x20 is the `hkaSkeleton`).
    fn skeleton_bone_count(&self) -> Option<u32>;
    /// The graph's `BSAnimationGraphEvent` source (+0x68).
    fn event_source(&self) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>>;
    /// Whether the graph's foot IK driver (`character.footIkDriver`) is disabled, its byte +0x4a;
    /// `None` without a driver.
    fn foot_ik_disabled(&self) -> Option<bool>;
}

impl BShkbAnimationGraphExt for BShkbAnimationGraph {
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn from_bone_array<'a>(nodes: *mut c_void) -> Option<&'a Self> {
        let graph = nodes.cast::<u8>().wrapping_sub(core::mem::offset_of!(BShkbAnimationGraph, boneNodes)).cast::<Self>();
        unsafe { graph.as_ref() }
    }

    fn bone_nodes(&self) -> &[BoneNodeEntry] {
        let nodes = &self.boneNodes;
        if nodes.data.is_null() || nodes.size == 0 {
            return &[];
        }
        sre::fixed_size!("BoneNodeEntry", BoneNodeEntry);
        unsafe { core::slice::from_raw_parts(nodes.data.cast::<BoneNodeEntry>(), nodes.size as usize) }
    }

    fn bone_node(&self, i: usize) -> Option<&NiAVObject> {
        self.bone_nodes().get(i)?.node()
    }

    fn pose_local(&self) -> &[HkQsTransform] {
        let c = &self.character;
        if c.poseLocal.is_null() || c.numPoseLocal == 0 {
            return &[];
        }
        sre::fixed_size!("hkQsTransform", HkQsTransform);
        assert!(sre::member_offset!("hkQsTransform.rotation") == 0x10 && sre::member_offset!("hkQsTransform.scale") == 0x20, "hkQsTransform's members moved");
        unsafe { core::slice::from_raw_parts(c.poseLocal.cast::<HkQsTransform>(), c.numPoseLocal as usize) }
    }

    fn skeleton_bone_count(&self) -> Option<u32> {
        let setup = self.character.setup.cast::<u8>();
        if setup.is_null() {
            return None;
        }
        let skeleton = unsafe { setup.add(sre::member_offset!("hkbCharacterSetup.m_hkpRayShapeCollectionFilter_20")).cast::<*const sre::hkaSkeleton>().read_unaligned() };
        unsafe { skeleton.as_ref() }.map(|s| s.boneCount)
    }

    fn event_source(&self) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>> {
        unsafe { BSTEventSource::from_ptr((&raw const self.animationGraphEventSource).cast()) }
    }

    fn foot_ik_disabled(&self) -> Option<bool> {
        let driver = self.character.footIkDriver.cast::<u8>();
        // The disabled byte (`SetDisabled`, id 58428) is the third of the ledger's `field_48`.
        (!driver.is_null()).then(|| unsafe { driver.add(sre::member_offset!("hkbFootIkDriver.field_48") + 2).read() } != 0)
    }
}

/// A face channel group whose names the engine keeps in a table, the position being the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceChannel {
    /// 16 phonemes (`Aah` .. `W`).
    Phoneme,
    /// 17 expressions (`DialogueAnger` .. `CombatShout`).
    Expression,
    /// 17 modifiers (`BlinkLeft` .. `HeadYaw`; 14-16 are head tracking).
    Modifier,
}

/// The engine's names of a channel group (`g_FaceGenPhonemeNames`, `g_FaceGenExpressionNames`,
/// `g_FaceGenModifierNames`; RE `camera.md` IC8 H).
#[must_use]
pub fn face_channel_names(channel: FaceChannel) -> Vec<String> {
    let (table, count) = match channel {
        FaceChannel::Phoneme => (sre::g_face_gen_phoneme_names(), 16),
        FaceChannel::Expression => (sre::g_face_gen_expression_names().cast(), 17),
        FaceChannel::Modifier => (sre::g_face_gen_modifier_names().cast(), 17),
    };
    (0..count)
        .map(|i| {
            let name = unsafe { table.add(i).read() }.cast::<core::ffi::c_char>();
            if name.is_null() { String::new() } else { unsafe { core::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned() }
        })
        .collect()
}

/// The face animation data of an actor, through vtable slot 99 (`GetFaceGenAnimationData`, the
/// face node's +0x150); `None` without a face. Main thread.
#[must_use]
pub fn face_data(actor: &Actor) -> Option<&BSFaceGenAnimationData> {
    type GetFn = unsafe extern "C" fn(*const Actor) -> *const BSFaceGenAnimationData;
    let get: GetFn = unsafe { core::mem::transmute(sre::vtable_entry(actor, sre::slot_index!("Actor", 0x0, 99))) };
    unsafe { get(actor).as_ref() }
}

/// Writes a field of the packed face data, which the engine keeps aligned.
fn put<T>(field: *const T, value: T) {
    unsafe { field.cast_mut().write_volatile(value) };
}

fn write_values(channel: *const BSFaceGenKeyframeMultiple, values: &[f32]) {
    let channel = unsafe { &*channel };
    let count = (channel.count as usize).min(values.len());
    if channel.values.is_null() || count == 0 {
        return;
    }
    let target = channel.values.cast::<f32>();
    for (i, value) in values.iter().take(count).enumerate() {
        unsafe { target.add(i).write_volatile(*value) };
    }
    crate::store(&channel.isZero, 0);
}

pub trait BSFaceGenAnimationDataExt {
    /// Writes the phoneme and modifier inputs from index 0 (as many as given and the channel holds)
    /// under the data's lock, clearing each channel's all-zero flag so the morpher sees them.
    fn set_inputs(&self, phonemes: &[f32], modifiers: &[f32]);
    /// The expression held as Papyrus `SetExpressionOverride` holds it: the override lock
    /// released, `SetExpression` (id 26594; index -1 to 16, weight 0 to 1), the lock set again so
    /// the engine's mood and dialogue writes pass it by.
    fn set_expression_override(&self, expression: i32, weight: f32);
    /// Releases the override lock (`ClearExpressionOverride`).
    fn clear_expression_override(&self);
    /// The eyes' base heading and pitch, with the engine's emotion offsets held at zero.
    fn hold_eyes(&self, heading: f32, pitch: f32);
    /// Parks the blink state machine in its wait so the eyelids hold.
    fn hold_blink(&self);
}

impl BSFaceGenAnimationDataExt for BSFaceGenAnimationData {
    fn set_inputs(&self, phonemes: &[f32], modifiers: &[f32]) {
        let lock = &raw const self.lock;
        let _guard = crate::spin::SpinGuard::lock(unsafe { &*lock });
        write_values(&raw const self.phonemeInput, phonemes);
        write_values(&raw const self.modifierInput, modifiers);
    }

    fn set_expression_override(&self, expression: i32, weight: f32) {
        put(&raw const self.expressionOverride, 0);
        self.set_expression(expression, weight.clamp(0.0, 1.0));
        put(&raw const self.expressionOverride, 1);
    }

    fn clear_expression_override(&self) {
        put(&raw const self.expressionOverride, 0);
    }

    fn hold_eyes(&self, heading: f32, pitch: f32) {
        put(&raw const self.eyeHeadingBase, heading);
        put(&raw const self.eyePitchBase, pitch);
        put(&raw const self.eyeHeadingOffset, 0.0);
        put(&raw const self.eyePitchOffset, 0.0);
    }

    fn hold_blink(&self) {
        put(&raw const self.blinkStage, FaceBlinkStage::Waiting as u32);
        put(&raw const self.blinkTimer, 10.0);
    }
}
