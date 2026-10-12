//! First-person shadows: the player's body casts its shadow in first person. Vanilla hides the
//! third-person root as soon as the view switches, so no pass draws it. This mod shows that root
//! again in first person and detours `NiAVObject::Cull` so that the face (`BSFaceGenNiNode`) is
//! skipped for every camera but the shadow cameras: the body is drawn, the head is not, and the
//! shadow includes both. The first-person arms stay out of the shadow maps, as the frame itself
//! arranges. Counts of what the detour saw are logged at every save as the acceptance evidence.

use std::fs::File;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use layout::v1_7_104::{classes, vtables};
use skyrim::actors::PlayerCharacter;
use skyrim::render::{CullFn, CullingProcess, NiAVObject, SetFirstPersonFn};

skse::plugin! {
    name: "skyrim-rs-first-person-shadows",
    author: "TwistedModding",
    version: 1,
    load: load,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);
static CULL: OnceLock<hook::Hook<CullFn>> = OnceLock::new();
static SWITCH: OnceLock<hook::Hook<SetFirstPersonFn>> = OnceLock::new();
static FIRST_PERSON: AtomicBool = AtomicBool::new(false);
/// In first person: visits of the third-person root by shadow cameras, by other cameras, the
/// face node skipped for other cameras, the face node passed to shadow cameras.
static BODY_SHADOW: AtomicU32 = AtomicU32::new(0);
static BODY_CAMERA: AtomicU32 = AtomicU32::new(0);
static HEAD_SKIPPED: AtomicU32 = AtomicU32::new(0);
static HEAD_SHADOW: AtomicU32 = AtomicU32::new(0);

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg);
        }
    };
}

/// Shows the third-person root in first person; third person is the engine's own state.
fn apply(first_person: bool) {
    FIRST_PERSON.store(first_person, Ordering::Relaxed);
    if first_person && let Some(root) = PlayerCharacter::get().and_then(PlayerCharacter::third_person_root) {
        root.set_hidden(false);
        log!("first person: third-person root {:p} shown (flags {:#x})", root.raw(), root.flags());
    }
}

unsafe extern "C" fn switch_detour(player: *mut classes::PlayerCharacter, first_person: bool) {
    unsafe { SWITCH.get().expect("installed before enabling").original()(player, first_person) }
    apply(first_person);
}

unsafe extern "C" fn cull_detour(node: *mut classes::NiAVObject, culler: *mut classes::partial::NiCullingProcess) {
    let original = CULL.get().expect("installed before enabling").original();
    if FIRST_PERSON.load(Ordering::Relaxed)
        && let (Some(n), Some(c), Some(root)) = (unsafe { NiAVObject::from_ptr(node) }, unsafe { CullingProcess::from_ptr(culler) }, PlayerCharacter::get().and_then(PlayerCharacter::third_person_root))
    {
        if core::ptr::eq(n.as_ptr(), root.as_ptr().cast()) {
            if c.is_shadow_pass() { &BODY_SHADOW } else { &BODY_CAMERA }.fetch_add(1, Ordering::Relaxed);
        } else if n.class().is_some_and(|v| v.id == vtables::BSFaceGenNiNode_vtable::ID) && n.is_under(root, 24) {
            if c.is_shadow_pass() {
                HEAD_SHADOW.fetch_add(1, Ordering::Relaxed);
            } else {
                HEAD_SKIPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
    unsafe { original(node, culler) }
}

fn install() -> Result<(), String> {
    let cull = unsafe { skyrim::render::hook_cull(cull_detour) }.map_err(|e| format!("NiAVObject::Cull: {e}"))?;
    let switch = unsafe { skyrim::render::hook_set_first_person(switch_detour) }.map_err(|e| format!("SetFirstPerson: {e}"))?;
    log!("hooked NiAVObject::Cull at {:#x} and PlayerCharacter::SetFirstPerson at {:#x}", cull.target(), switch.target());
    let _ = CULL.set(cull);
    let _ = SWITCH.set(switch);
    Ok(())
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    match unsafe { (*msg).kind } {
        skse::MessagingInterface::DATA_LOADED => {
            if let Err(e) = install() {
                log!("FAIL install: {e}");
            }
        }
        skse::MessagingInterface::POST_LOAD_GAME => {
            for c in [&BODY_SHADOW, &BODY_CAMERA, &HEAD_SKIPPED, &HEAD_SHADOW] {
                c.store(0, Ordering::Relaxed);
            }
            if let Some(p) = PlayerCharacter::get() {
                apply(!p.is_third_person());
            }
        }
        skse::MessagingInterface::SAVE_GAME => {
            let first_person = FIRST_PERSON.load(Ordering::Relaxed);
            let roots = PlayerCharacter::get().map(|p| (p.third_person_root().map(|r| r.flags()), p.first_person_root().map(|r| r.flags())));
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
