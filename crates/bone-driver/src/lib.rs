//! Bone driver: the phase D acceptance for the animation path. After every pose write of the
//! player's third-person graph it turns the head bone ("NPC Head [Head]") by a fixed angle about
//! its local vertical axis and calls nothing. The world update after the draws picks the edit up
//! and the next frame's culls and skin palettes draw the turned head: the one-frame latency the
//! animation itself has. Counts and the last pre- and post-edit rotations are logged at every save.

use std::fs::File;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use skyrim::BSFixedString;
use skyrim::animation::{ApplyPoseFn, HkQsTransform};
use skyrim::prelude::*;
use skyrim::render::Transform;
use sre::BShkbAnimationGraph;

skse::plugin! {
    name: "skyrim-rs-bone-driver",
    author: "TwistedModding",
    version: 1,
    load: load,
}

/// The turn applied every frame, in degrees about the bone's local Y axis.
const TURN_DEGREES: f32 = 35.0;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static APPLY: OnceLock<hook::Hook<ApplyPoseFn>> = OnceLock::new();
static HEAD: AtomicPtr<sre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static PLAYER: AtomicPtr<sre::TESObjectREFR> = AtomicPtr::new(core::ptr::null_mut());
static APPLIED: AtomicU32 = AtomicU32::new(0);
static LAST: Mutex<Option<(Transform, Transform)>> = Mutex::new(None);

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg)*);
        }
    };
}

/// `a * b` for row-major 3x3 rotations.
fn multiply(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    core::array::from_fn(|r| core::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn turn(degrees: f32) -> [[f32; 3]; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

unsafe extern "C" fn apply_detour(pose: *const HkQsTransform, nodes: *mut core::ffi::c_void, count: i32) {
    unsafe { APPLY.get().expect("stored before enabling").original()(pose, nodes, count) }
    let head = HEAD.load(Ordering::Acquire);
    if head.is_null() {
        return;
    }
    let Some(graph) = (unsafe { BShkbAnimationGraph::from_bone_array(nodes) }) else { return };
    if !graph.owner().is_some_and(|o| core::ptr::eq(o, PLAYER.load(Ordering::Relaxed))) {
        return;
    }
    let Some(entry) = graph.bone_nodes().iter().find(|e| core::ptr::eq(e.node_field(), head)) else { return };
    let Some(node) = entry.node() else { return };
    let before = node.local_transform();
    let after = Transform { rotation: multiply(before.rotation, turn(TURN_DEGREES)), ..before };
    node.set_local_transform(after);
    APPLIED.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut last) = LAST.try_lock() {
        *last = Some((before, after));
    }
}

/// Caches the player and its head bone on the main thread after a load; clears them before one.
fn bind(active: bool) {
    let head = active.then(sre::PlayerCharacter::singleton).flatten().and_then(|p| {
        PLAYER.store(core::ptr::from_ref::<sre::TESObjectREFR>(p).cast_mut(), Ordering::Relaxed);
        let root = p.loaded_data()?.node3_d()?;
        root.get_object_by_name(BSFixedString::new("NPC Head [Head]").raw()).map(|h| core::ptr::from_ref(h).cast_mut())
    });
    HEAD.store(head.unwrap_or(core::ptr::null_mut()), Ordering::Release);
    if let Some(h) = head {
        log!("head bone {h:p} bound; turning it {TURN_DEGREES} degrees after every pose write of the player's third-person graph");
    }
}

fn install() -> Result<(), String> {
    let apply = unsafe { skyrim::animation::hook_apply_pose(apply_detour) }.map_err(|e| format!("ApplyPoseToBoneNodes: {e}"))?;
    log!("hooked ApplyPoseToBoneNodes at {:#x}", apply.target());
    let _ = APPLY.set(apply);
    APPLY.get().expect("just set").enable().map_err(|e| format!("enable ApplyPoseToBoneNodes: {e}"))?;
    Ok(())
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    match unsafe { (*msg).kind } {
        skse::MessagingInterface::DATA_LOADED => {
            if let Err(e) = install() {
                log!("FAIL install: {e}");
            }
        }
        skse::MessagingInterface::PRE_LOAD_GAME | skse::MessagingInterface::NEW_GAME => bind(false),
        skse::MessagingInterface::POST_LOAD_GAME => {
            APPLIED.store(0, Ordering::Relaxed);
            bind(true);
        }
        skse::MessagingInterface::SAVE_GAME => {
            let last = *LAST.lock().unwrap();
            match last {
                Some((before, after)) => log!(
                    "save: head turned in {} pose writes; last rotation rows before {:?} after {:?}, translation {:?}",
                    APPLIED.load(Ordering::Relaxed),
                    before.rotation[0],
                    after.rotation[0],
                    after.translation
                ),
                None => log!("save: head turned in {} pose writes, no sample", APPLIED.load(Ordering::Relaxed)),
            }
        }
        _ => {}
    }
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-bone-driver") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    if let Err(e) = skyrim::init(skse) {
        log!("FAIL init: {e}");
        return false;
    }
    match skse.messaging() {
        Some(m) if m.listen_skse(skse.plugin_handle(), on_message) => {
            log!("listening for SKSE messages");
            true
        }
        _ => {
            log!("FAIL init: no messaging interface");
            false
        }
    }
}
