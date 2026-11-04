//! First-person shadows: the player's body casts its shadow in first person. Vanilla hides the
//! third-person root as soon as the view switches, so no pass draws it. This mod shows that root
//! again in first person and detours `NiAVObject::Cull` so that the face (`BSFaceGenNiNode`) is
//! skipped for every camera but the shadow cameras: the body is drawn, the head is not, and the
//! shadow includes both. The first-person arms stay out of the shadow maps, as the frame itself
//! arranges. Counts of what the detour saw are logged at every save as the acceptance evidence.
//! Because the root is shown, every path takes the body on its own: the sun's list walk, the
//! cascade replays (which skip only entries hidden themselves) and the focus maps; the face skip
//! only has to hold for the main-camera walk, which is the one path that goes through `Cull`
//! for every node. The detour runs on the job threads and the frame thread concurrently.

use std::fs::File;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use skyrim::prelude::*;
use skyrim::render::{CullFn, SetFirstPersonFn};
use sre::raw::vtables;
use sre::PlayerCharacter;

skse::plugin! {
    name: "skyrim-rs-first-person-shadows",
    author: "TwistedModding",
    version: 1,
    load: load,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);
static CULL: OnceLock<hook::Hook<CullFn>> = OnceLock::new();
static SWITCH: OnceLock<hook::Hook<SetFirstPersonFn>> = OnceLock::new();
/// The third-person root cached on the main thread; null outside a loaded game, when the
/// detour must not touch the player.
static THIRD_ROOT: AtomicPtr<sre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static FIRST_PERSON: AtomicBool = AtomicBool::new(false);
/// In first person: visits of the third-person root by shadow cameras, by other cameras, the
/// face node skipped for other cameras, the face node passed to shadow cameras.
static BODY_SHADOW: AtomicU32 = AtomicU32::new(0);
static BODY_CAMERA: AtomicU32 = AtomicU32::new(0);
static HEAD_SKIPPED: AtomicU32 = AtomicU32::new(0);
static HEAD_SHADOW: AtomicU32 = AtomicU32::new(0);

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg)*);
        }
    };
}

/// Caches the third-person root and shows it in first person; third person is the engine's own state.
fn apply(first_person: bool) {
    FIRST_PERSON.store(first_person, Ordering::Relaxed);
    let root = sre::PlayerCharacter::singleton().and_then(PlayerCharacter::third_person_root);
    THIRD_ROOT.store(root.map_or(core::ptr::null_mut(), |r| core::ptr::from_ref(r).cast_mut()), Ordering::Release);
    if first_person && let Some(root) = root {
        root.set_hidden(false);
        log!("first person: third-person root {root:p} shown (flags {:#x})", root.flags);
    }
}

unsafe extern "C" fn switch_detour(player: *mut sre::PlayerCharacter, first_person: bool) {
    unsafe { SWITCH.get().expect("stored before enabling").original()(player, first_person) }
    apply(first_person);
}

unsafe extern "C" fn cull_detour(node: *mut sre::NiAVObject, culler: *mut sre::NiCullingProcess, plane_mask: u32) {
    let original = CULL.get().expect("stored before enabling").original();
    if FIRST_PERSON.load(Ordering::Relaxed)
        && let (Some(n), Some(c), Some(root)) = (unsafe { node.as_ref() }, unsafe { culler.cast::<sre::BSCullingProcess>().as_ref() }, unsafe { THIRD_ROOT.load(Ordering::Acquire).as_ref() })
    {
        if core::ptr::eq(n, root) {
            if c.is_shadow_pass() { &BODY_SHADOW } else { &BODY_CAMERA }.fetch_add(1, Ordering::Relaxed);
        } else if skyrim::class_of(n).is_some_and(|v| v.id == sre::id!(vtables::BSFaceGenNiNode_vtable::ID)) && n.is_under(root, 24) {
            if c.is_shadow_pass() {
                HEAD_SHADOW.fetch_add(1, Ordering::Relaxed);
            } else {
                HEAD_SKIPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
    unsafe { original(node, culler, plane_mask) }
}

fn install() -> Result<(), String> {
    let cull = unsafe { skyrim::render::hook_cull(cull_detour) }.map_err(|e| format!("NiAVObject::Cull: {e}"))?;
    let switch = unsafe { skyrim::render::hook_set_first_person(switch_detour) }.map_err(|e| format!("SetFirstPerson: {e}"))?;
    log!("hooked NiAVObject::Cull at {:#x} and PlayerCharacter::SetFirstPerson at {:#x}", cull.target(), switch.target());
    let _ = CULL.set(cull);
    let _ = SWITCH.set(switch);
    CULL.get().expect("just set").enable().map_err(|e| format!("enable NiAVObject::Cull: {e}"))?;
    SWITCH.get().expect("just set").enable().map_err(|e| format!("enable SetFirstPerson: {e}"))?;
    Ok(())
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    match unsafe { (*msg).kind } {
        skse::MessagingInterface::DATA_LOADED => {
            if let Err(e) = install() {
                log!("FAIL install: {e}");
            }
        }
        skse::MessagingInterface::PRE_LOAD_GAME | skse::MessagingInterface::NEW_GAME => {
            FIRST_PERSON.store(false, Ordering::Relaxed);
            THIRD_ROOT.store(core::ptr::null_mut(), Ordering::Release);
        }
        skse::MessagingInterface::POST_LOAD_GAME => {
            for c in [&BODY_SHADOW, &BODY_CAMERA, &HEAD_SKIPPED, &HEAD_SHADOW] {
                c.store(0, Ordering::Relaxed);
            }
            if let Some(p) = sre::PlayerCharacter::singleton() {
                apply(!p.is_third_person());
            }
        }
        skse::MessagingInterface::SAVE_GAME => {
            let first_person = FIRST_PERSON.load(Ordering::Relaxed);
            let roots = sre::PlayerCharacter::singleton().map(|p| (p.third_person_root().map(|r| r.flags), p.first_person_root().map(|r| r.flags)));
            log!(
                "save: first_person={first_person} root flags {roots:?}; in first person the body root was visited {} times by shadow cameras and {} by other cameras, the face skipped {} times for other cameras and passed {} times to shadow cameras",
                BODY_SHADOW.load(Ordering::Relaxed),
                BODY_CAMERA.load(Ordering::Relaxed),
                HEAD_SKIPPED.load(Ordering::Relaxed),
                HEAD_SHADOW.load(Ordering::Relaxed)
            );
        }
        _ => {}
    }
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-first-person-shadows") else {
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
