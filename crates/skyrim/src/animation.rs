//! Animation (subsystem pass 7): the animation graph holder of a reference, its graph manager and
//! graphs, the graph variables and events, the pose a graph writes into the bone nodes, and the
//! bone-driver hook.
//!
//! The pose is written on a job worker thread by `ApplyPoseToBoneNodes`, once per graph and
//! frame, while the main thread draws; the world update of the skeleton runs after the draws, so
//! every frame draws the previous frame's pose. A bone driver post-hooks the pose write, edits a
//! bone's local transform and calls nothing: the world update picks it up and the next frame's
//! culls and skin palettes use it, with the same one-frame latency the animation itself has.
//! Nothing on the animation path may call `NiAVObject::update` or touch other actors' nodes.

use core::ffi::c_void;

use skre::raw::{fns, globals, vtables};

use crate::actors::{Actor, TESObjectREFR};
use crate::events::{BSAnimationGraphEvent, BSTEventSource};
use crate::render::{NiAVObject, NiNode};
use crate::string::BSFixedString;

pub use fns::PlayerCharacter__UpdateNodesAfterRender_fn as UpdateNodesAfterRenderFn;

/// `ApplyPoseToBoneNodes(pose, nodes, count)` (id 63856): the only writer of animated bone locals.
/// `nodes` is the graph's bone node array at +0x160; [`AnimationGraph::from_bone_array`] recovers the graph.
pub type ApplyPoseFn = unsafe extern "C" fn(pose: *const HkQsTransform, nodes: *mut c_void, count: i32);

/// Holder slot 1 `NotifyAnimationGraph(const BSFixedString&)`.
type NotifyFn = unsafe extern "C" fn(this: *mut skre::IAnimationGraphManagerHolder, name: *const skre::BSFixedString) -> bool;
/// Holder slots 16, 17, 18: `bool (this, const BSFixedString& name, T& out)`.
type GetVariableFn<T> = unsafe extern "C" fn(this: *mut skre::IAnimationGraphManagerHolder, name: *const skre::BSFixedString, out: *mut T) -> bool;
/// The non-virtual setters (ids 32887, 32886, 32885): `bool (this, const BSFixedString& name, T value)`.
type SetVariableFn<T> = unsafe extern "C" fn(this: *mut skre::IAnimationGraphManagerHolder, name: *const skre::BSFixedString, value: T) -> bool;
/// `TESObjectREFR::GetAnimationGraphEventSource` (id 40850): `bool (ref, out, graphIndex)`.
type GetEventSourceFn = unsafe extern "C" fn(refr: *mut skre::TESObjectREFR, out: *mut *mut skre::BSTEventSource, graph_index: u32) -> bool;

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
    pub node: *mut skre::NiAVObject,
    pub index: u32,
    pub padding: u32,
}

const _: () = assert!(size_of::<BoneNodeEntry>() == 0x10);

pub(crate) struct Fns {
    apply_pose: usize,
    update_nodes_after_render: usize,
    get_event_source: GetEventSourceFn,
    set_float: SetVariableFn<f32>,
    set_int: SetVariableFn<i32>,
    set_bool: SetVariableFn<bool>,
    current_speed: fns::Actor__GetCurrentSpeed_fn,
    mediator: *const *const c_void,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        for id in [fns::BSTEventSource_BSAnimationGraphEvent__AddEventSink, fns::BSTEventSource_BSAnimationGraphEvent__RemoveEventSink] {
            reloc.address(id)?;
        }
        unsafe {
            Ok(Self {
                apply_pose: reloc.address(fns::ApplyPoseToBoneNodes)?,
                update_nodes_after_render: reloc.address(fns::PlayerCharacter__UpdateNodesAfterRender)?,
                get_event_source: crate::resolve(reloc, fns::TESObjectREFR__GetAnimationGraphEventSource)?,
                set_float: crate::resolve(reloc, fns::IAnimationGraphManagerHolder__SetGraphVariableFloat)?,
                set_int: crate::resolve(reloc, fns::IAnimationGraphManagerHolder__SetGraphVariableInt)?,
                set_bool: crate::resolve(reloc, fns::IAnimationGraphManagerHolder__SetGraphVariableBool)?,
                current_speed: crate::resolve(reloc, fns::Actor__GetCurrentSpeed)?,
                mediator: reloc.address(globals::g_ActorMediator_ptr)? as *const *const c_void,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().animation
}

/// Creates, disabled, a post-hook target on `ApplyPoseToBoneNodes`; store it, then
/// [`hook::Hook::enable`]. The detour runs on a job worker thread while the main thread draws:
/// call the original first, then edit bone locals of the graph's own nodes and nothing else.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_apply_pose(detour: ApplyPoseFn) -> Result<hook::Hook<ApplyPoseFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().apply_pose, detour) }
}

/// Creates, disabled, a detour on `PlayerCharacter::UpdateNodesAfterRender` (id 40522): the world
/// update of the player's roots on a job thread after the frame's draws.
///
/// # Safety
/// Same as [`hook::Hook::create`].
pub unsafe fn hook_update_nodes_after_render(detour: UpdateNodesAfterRenderFn) -> Result<hook::Hook<UpdateNodesAfterRenderFn>, hook::Error> {
    unsafe { hook::Hook::create(fns().update_nodes_after_render, detour) }
}

/// The engine's shared sink registered on every actor graph (`g_ActorMediator_ptr`).
#[must_use]
pub fn actor_mediator() -> *const c_void {
    unsafe { *fns().mediator }
}

/// The event source of a reference's graph through `TESObjectREFR::GetAnimationGraphEventSource`:
/// `None` for the active graph, `Some(i)` for graph `i` (graph 0 sends in first and third person).
/// The registration dies with the manager (3D unload).
#[must_use]
pub fn graph_event_source(refr: &TESObjectREFR, graph: Option<u32>) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>> {
    let mut out: *mut skre::BSTEventSource = core::ptr::null_mut();
    let ok = unsafe { (fns().get_event_source)(refr.as_ptr().cast_mut(), &raw mut out, graph.unwrap_or(u32::MAX)) };
    if ok { unsafe { BSTEventSource::from_ptr(out) } } else { None }
}

/// The animation graph holder interface of a reference (`TESObjectREFR` +0x38).
#[repr(transparent)]
pub struct AnimationGraphHolder(skre::IAnimationGraphManagerHolder);

impl AnimationGraphHolder {
    /// The holder sub-object of `refr`.
    #[must_use]
    pub fn of(refr: &TESObjectREFR) -> &Self {
        unsafe { &*(&raw const (*refr.as_ptr()).__vftable_38).cast::<Self>() }
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut skre::IAnimationGraphManagerHolder {
        (&raw const self.0).cast_mut()
    }

    fn vtable(&self) -> &vtables::IAnimationGraphManagerHolder_vtable {
        unsafe { &*self.0.__vftable }
    }

    fn slot<F: Copy>(slot: *const c_void) -> Option<F> {
        const { assert!(size_of::<F>() == size_of::<usize>()) };
        (!slot.is_null()).then(|| unsafe { core::mem::transmute_copy::<*const c_void, F>(&slot) })
    }

    /// Slot 1: sends an animation event by name to every graph of the manager (the Papyrus
    /// `SendAnimationEvent` path). Main thread.
    #[must_use]
    pub fn notify(&self, event: &BSFixedString) -> bool {
        Self::slot::<NotifyFn>(self.vtable().slot_1).is_some_and(|f| unsafe { f(self.as_ptr(), core::ptr::from_ref(event).cast()) })
    }

    /// Slot 16: a float graph variable of the active graph (an actor reads its cache first).
    #[must_use]
    pub fn float(&self, name: &BSFixedString) -> Option<f32> {
        let mut out = 0.0f32;
        let f = Self::slot::<GetVariableFn<f32>>(self.vtable().slot_16)?;
        unsafe { f(self.as_ptr(), core::ptr::from_ref(name).cast(), &raw mut out) }.then_some(out)
    }

    /// Slot 17: an int graph variable.
    #[must_use]
    pub fn int(&self, name: &BSFixedString) -> Option<i32> {
        let mut out = 0i32;
        let f = Self::slot::<GetVariableFn<i32>>(self.vtable().slot_17)?;
        unsafe { f(self.as_ptr(), core::ptr::from_ref(name).cast(), &raw mut out) }.then_some(out)
    }

    /// Slot 18: a bool graph variable.
    #[must_use]
    pub fn bool(&self, name: &BSFixedString) -> Option<bool> {
        let mut out = false;
        let f = Self::slot::<GetVariableFn<bool>>(self.vtable().slot_18)?;
        unsafe { f(self.as_ptr(), core::ptr::from_ref(name).cast(), &raw mut out) }.then_some(out)
    }

    /// Sets a float variable on every graph of the manager; true whenever a manager exists.
    #[must_use]
    pub fn set_float(&self, name: &BSFixedString, value: f32) -> bool {
        unsafe { (fns().set_float)(self.as_ptr(), core::ptr::from_ref(name).cast(), value) }
    }

    #[must_use]
    pub fn set_int(&self, name: &BSFixedString, value: i32) -> bool {
        unsafe { (fns().set_int)(self.as_ptr(), core::ptr::from_ref(name).cast(), value) }
    }

    #[must_use]
    pub fn set_bool(&self, name: &BSFixedString, value: bool) -> bool {
        unsafe { (fns().set_bool)(self.as_ptr(), core::ptr::from_ref(name).cast(), value) }
    }
}

/// The graph manager of an actor, `MiddleHighProcessData` +0x1a8.
#[must_use]
pub fn graph_manager(actor: &Actor) -> Option<&AnimationGraphManager> {
    let middle_high = actor.process()?.middle_high()?;
    unsafe { middle_high.raw().animationGraphManager.cast::<AnimationGraphManager>().as_ref() }
}

/// `Actor::GetCurrentSpeed` (id 37957): `MiddleHighProcessData.speed`, the `Speed` channel's value.
#[must_use]
pub fn current_speed(actor: &Actor) -> f32 {
    unsafe { (fns().current_speed)(actor.as_ptr().cast_mut()) }
}

/// An actor's graph manager: its graphs and the active one.
#[repr(transparent)]
pub struct AnimationGraphManager(skre::BSAnimationGraphManager);

impl AnimationGraphManager {
    #[must_use]
    pub fn raw(&self) -> &skre::BSAnimationGraphManager {
        &self.0
    }

    /// The graphs (data at +0x48, count at +0x50): two for the player, third person first.
    #[must_use]
    pub fn graphs(&self) -> &[Option<&AnimationGraph>] {
        let b = &self.0.graphs;
        let data = usize::from_le_bytes(b[8..16].try_into().unwrap()) as *const c_void;
        let count = u32::from_le_bytes(b[16..20].try_into().unwrap());
        unsafe { crate::forms::ptr_slice(data, count) }
    }

    /// Index of the active graph (0 in third person, 1 in first person on the player).
    #[must_use]
    pub fn active_graph_index(&self) -> u32 {
        self.0.activeGraph
    }

    #[must_use]
    pub fn active_graph(&self) -> Option<&AnimationGraph> {
        self.graphs().get(self.active_graph_index() as usize).copied().flatten()
    }
}

/// One behaviour graph: its owner, root, bone nodes, pose and event source.
#[repr(transparent)]
pub struct AnimationGraph(skre::BShkbAnimationGraph);

impl AnimationGraph {
    #[must_use]
    pub fn raw(&self) -> &skre::BShkbAnimationGraph {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const skre::BShkbAnimationGraph {
        &raw const self.0
    }

    /// The graph whose bone node array `nodes` is, as handed to [`ApplyPoseFn`].
    ///
    /// # Safety
    /// `nodes` must be the +0x160 array of a live `BShkbAnimationGraph`.
    #[must_use]
    #[allow(clippy::cast_ptr_alignment)]
    pub unsafe fn from_bone_array<'a>(nodes: *mut c_void) -> Option<&'a Self> {
        let graph = nodes.cast::<u8>().wrapping_sub(core::mem::offset_of!(skre::BShkbAnimationGraph, boneNodes)).cast::<Self>();
        unsafe { graph.as_ref() }
    }

    /// The owning reference (the `PlayerCharacter` for the player's graphs).
    #[must_use]
    pub fn owner(&self) -> Option<&TESObjectREFR> {
        unsafe { self.0.owner.cast::<TESObjectREFR>().as_ref() }
    }

    /// The 3D root the graph drives.
    #[must_use]
    pub fn root_node(&self) -> Option<&NiNode> {
        unsafe { self.0.rootNode.cast::<NiNode>().as_ref() }
    }

    /// Bones the skeleton has (99 on the player).
    #[must_use]
    pub fn bone_count(&self) -> u16 {
        self.0.boneCount
    }

    /// The bone node array the pose is written into, in skeleton bone order.
    #[must_use]
    pub fn bone_nodes(&self) -> &[BoneNodeEntry] {
        let arr = &self.0.boneNodes;
        if arr.data.is_null() || arr.size == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(arr.data.cast::<BoneNodeEntry>(), arr.size as usize) }
    }

    /// The bone node at skeleton index `i`.
    #[must_use]
    pub fn bone_node(&self, i: usize) -> Option<&NiAVObject> {
        unsafe { NiAVObject::from_ptr(self.bone_nodes().get(i)?.node) }
    }

    /// The parent-local pose the behaviour graph produced this frame (`hkbCharacter.poseLocal`).
    #[must_use]
    pub fn pose_local(&self) -> &[HkQsTransform] {
        let c = &self.0.character;
        if c.poseLocal.is_null() || c.numPoseLocal == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(c.poseLocal.cast::<HkQsTransform>(), c.numPoseLocal as usize) }
    }

    /// Bones of the Havok skeleton (`hkbCharacterSetup` +0x20 is the `hkaSkeleton`).
    #[must_use]
    pub fn skeleton_bone_count(&self) -> Option<u32> {
        let setup = self.0.character.setup.cast::<u8>();
        if setup.is_null() {
            return None;
        }
        let skeleton = unsafe { setup.add(0x20).cast::<*const skre::hkaSkeleton>().read_unaligned() };
        unsafe { skeleton.as_ref() }.map(|s| s.boneCount)
    }

    /// The graph's `BSAnimationGraphEvent` source (+0x68).
    #[must_use]
    pub fn event_source(&self) -> Option<&'static BSTEventSource<BSAnimationGraphEvent>> {
        unsafe { BSTEventSource::from_ptr((&raw const self.0.animationGraphEventSource).cast()) }
    }

    /// The behaviour project's name.
    #[must_use]
    pub fn project_name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.projectName) }
    }
}
