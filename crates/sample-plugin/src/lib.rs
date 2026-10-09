//! Smallest complete plugin: declares itself, resolves one Address Library id, installs one
//! pass-through hook and logs what happened.

use core::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

skse::plugin! {
    name: "skyrim-rs-sample",
    author: "TwistedModding",
    version: 1,
    load: load,
    independence: skse::independence::ADDRESS_LIBRARY_POST_AE,
    independence_ex: skse::independence::EX_ADDRESS_LIBRARY_V5 | skse::independence::EX_NO_STRUCT_USE,
}

type LookupFormById = extern "C" fn(form_id: u32) -> *mut c_void;

const LOOKUP_FORM_BY_ID_SE: u64 = 14461;
const LOOKUP_FORM_BY_ID_AE: u64 = 14617;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static LOOKUP: OnceLock<hook::Hook<LookupFormById>> = OnceLock::new();
static CALLS: AtomicU64 = AtomicU64::new(0);

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg);
        }
    };
}

extern "C" fn lookup_detour(form_id: u32) -> *mut c_void {
    CALLS.fetch_add(1, Ordering::Relaxed);
    LOOKUP.get().unwrap().original()(form_id)
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    let msg = unsafe { &*msg };
    if msg.kind == skse::MessagingInterface::DATA_LOADED {
        log!("data loaded, LookupFormByID hook ran {} times", CALLS.load(Ordering::Relaxed));
    }
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-sample") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    let [major, minor, build, store] = skse::runtime::unpack(skse.runtime_version);
    let known = skse::runtime::find(skse.runtime_version).map_or("unknown runtime", |r| r.name);
    log!("runtime {major}.{minor}.{build} store {store} ({known}), skse {:08x}", skse.skse_version);

    let reloc = match hook::Reloc::open(skse.runtime_version) {
        Ok(r) => r,
        Err(e) => {
            log!("{e}");
            return false;
        }
    };
    log!(
        "address library format {} with {} ids, module base {:#x}",
        reloc.db().format(),
        reloc.db().len(),
        reloc.base()
    );

    let id = if skse::runtime::is_se(skse.runtime_version) { LOOKUP_FORM_BY_ID_SE } else { LOOKUP_FORM_BY_ID_AE };
    match unsafe { reloc.hook(id, lookup_detour as LookupFormById) } {
        Ok(h) => {
            log!("hooked LookupFormByID id {id} at {:#x}", h.target());
            let _ = LOOKUP.set(h);
        }
        Err(e) => {
            log!("{e}");
            return false;
        }
    }

    if let Some(m) = skse.messaging() {
        let _ = m.listen_skse(skse.plugin_handle(), on_message);
    }
    true
}
