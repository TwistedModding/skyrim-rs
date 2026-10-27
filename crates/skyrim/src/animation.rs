//! Animation (subsystem pass 7): the pose write and world update hooks, a reference's graph event
//! source, and what the generated holder, manager and graph methods leave `unsafe`: the graph
//! variables a getter writes into an out-parameter, the graphs and bone nodes as slices, the pose
//! the behaviour graph produced, and a graph from the bone array the pose hook receives.
//!
//! The pose is written on a job worker thread by `ApplyPoseToBoneNodes`, once per graph and
//! frame, while the main thread draws; the world update of the skeleton runs after the draws, so
//! every frame draws the previous frame's pose. A bone driver post-hooks the pose write, edits a
//! bone's local transform and calls nothing: the world update picks it up and the next frame's
//! culls and skin palettes use it, with the same one-frame latency the animation itself has.
//! Nothing on the animation path may call `NiAVObjectExt::update_transforms` or touch other actors' nodes.

use core::ffi::c_void;

use sre::raw::fns;
use sre::{BSAnimationGraphEvent, BSAnimationGraphManager, BShkbAnimationGraph, IAnimationGraphManagerHolder, NiAVObject, TESObjectREFR};

use crate::BSFixedString;
use crate::events::BSTEventSource;
use crate::forms::ptr_slice;

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

/// One entry of a graph's bone node array: the node and its index in the flattened tree.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BoneNodeEntry {
    pub node: *mut NiAVObject,
    pub index: u32,
    pub padding: u32,
}

const _: () = assert!(size_of::<BoneNodeEntry>() == 0x10);

/// Creates, disabled, a post-hook target on `ApplyPoseToBoneNodes`; store it, then
/// [`hook::Hook::enable`]. The detour runs on a job worker thread while the main thread draws:
/// call the original first, then edit bone locals of the graph's own nodes and nothing else.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_apply_pose(detour: ApplyPoseFn) -> Result<hook::Hook<ApplyPoseFn>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(fns::ApplyPoseToBoneNodes)?, detour) }
}

/// Creates, disabled, a detour on `PlayerCharacter::UpdateNodesAfterRender` (id 40522): the world
/// update of the player's roots on a job thread after the frame's draws.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_update_nodes_after_render(detour: UpdateNodesAfterRenderFn) -> Result<hook::Hook<UpdateNodesAfterRenderFn>, hook::Error> {
    unsafe { hook::Hook::create(crate::reloc().address(fns::PlayerCharacter__UpdateNodesAfterRender)?, detour) }
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
}

impl BSAnimationGraphManagerExt for BSAnimationGraphManager {
    fn graphs(&self) -> &[Option<&BShkbAnimationGraph>] {
        let b = &self.graphs;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let count = u32::from_le_bytes(b[16..20].try_into().unwrap());
        unsafe { ptr_slice(data, count) }
    }

    fn active_graph(&self) -> Option<&BShkbAnimationGraph> {
        self.graphs().get(self.activeGraph as usize).copied().flatten()
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
        unsafe { core::slice::from_raw_parts(nodes.data.cast::<BoneNodeEntry>(), nodes.size as usize) }
    }

    fn bone_node(&self, i: usize) -> Option<&NiAVObject> {
        unsafe { self.bone_nodes().get(i)?.node.as_ref() }
    }

    fn pose_local(&self) -> &[HkQsTransform] {
        let c = &self.character;
        if c.poseLocal.is_null() || c.numPoseLocal == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(c.poseLocal.cast::<HkQsTransform>(), c.numPoseLocal as usize) }
    }

    fn skeleton_bone_count(&self) -> Option<u32> {
        let setup = self.character.setup.cast::<u8>();
        if setup.is_null() {
            return None;
        }
        let skeleton = unsafe { setup.add(0x20).cast::<*const sre::hkaSkeleton>().read_unaligned() };
        unsafe { skeleton.as_ref() }.map(|s| s.boneCount)
    }

    fn event_source(&self) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>> {
        unsafe { BSTEventSource::from_ptr((&raw const self.animationGraphEventSource).cast()) }
    }
}
