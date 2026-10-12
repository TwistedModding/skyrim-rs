//! Smallest complete plugin on the generated layouts: resolves ids through `layout`, installs
//! two pass-through hooks, and reads live forms through the generated `TESForm` struct.

use core::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use layout::v1_7_104::{classes, fns, globals, vtables};

skse::plugin! {
    name: "skyrim-rs-sample",
    author: "TwistedModding",
    version: 1,
    load: load,
}

type LookupFormById = extern "C" fn(form_id: u32) -> *mut c_void;
type AddEventSink = extern "C" fn(source: *mut c_void, sink: *mut c_void) -> *mut c_void;

/// `LookupFormByID` is still the unnamed `ID_14617` in the ledger, so `fns` has no constant for it.
const LOOKUP_FORM_BY_ID_SE: u64 = 14461;
const LOOKUP_FORM_BY_ID_AE: u64 = 14617;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static RELOC: OnceLock<hook::Reloc> = OnceLock::new();
static LOOKUP: OnceLock<hook::Hook<LookupFormById>> = OnceLock::new();
static ADD_SINK: OnceLock<hook::Hook<AddEventSink>> = OnceLock::new();
static LOOKUP_CALLS: AtomicU64 = AtomicU64::new(0);
static ADD_SINK_CALLS: AtomicU64 = AtomicU64::new(0);

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg);
        }
    };
}

extern "C" fn lookup_detour(form_id: u32) -> *mut c_void {
    LOOKUP_CALLS.fetch_add(1, Ordering::Relaxed);
    LOOKUP.get().unwrap().original()(form_id)
}

extern "C" fn add_sink_detour(source: *mut c_void, sink: *mut c_void) -> *mut c_void {
    ADD_SINK_CALLS.fetch_add(1, Ordering::Relaxed);
    ADD_SINK.get().unwrap().original()(source, sink)
}

/// What the generated `TESForm` layout says about a live object: its vtable's class and the raw fields.
fn describe(form: *const classes::TESForm) -> String {
    if form.is_null() {
        return "null".into();
    }
    let f = unsafe { &*form };
    let vt = f.__vftable as usize;
    let class = u32::try_from(vt.wrapping_sub(RELOC.get().unwrap().base()))
        .ok()
        .and_then(vtables::find)
        .map_or_else(
            || format!("vtable {vt:#x} unknown to the ledger"),
            |v| format!("{}::vftable{} (vtable id {})", v.class, if v.offset == 0 { String::new() } else { format!("_{:X}", v.offset) }, v.id),
        );
    format!(
        "{form:p}: {class}, TESForm {{ field_8: {:#x}, field_10: {:#x}, field_18: {:#x}, field_1A: {:#x} }}",
        f.field_8, f.field_10, f.field_18, f.field_1A
    )
}

fn read_layouts() {
    let reloc = RELOC.get().unwrap();
    match reloc.address(globals::g_PlayerCharacter_ptr) {
        Ok(addr) => {
            let player = unsafe { *(addr as *const *const classes::TESForm) };
            log!("g_PlayerCharacter_ptr (global id {}) -> {}", globals::g_PlayerCharacter_ptr, describe(player));
        }
        Err(e) => log!("{e}"),
    }
    let lookup = LOOKUP.get().unwrap().original();
    for form_id in [0x14u32, 0x7] {
        log!("LookupFormByID({form_id:#x}) -> {}", describe(lookup(form_id).cast()));
    }
    log!("generated PlayerCharacter is {:#x} bytes", size_of::<classes::PlayerCharacter>());
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    let msg = unsafe { &*msg };
    if msg.kind == skse::MessagingInterface::DATA_LOADED {
        log!(
            "data loaded, LookupFormByID ran {} times, BSTEventSource<MenuOpenCloseEvent>::AddEventSink ran {} times",
            LOOKUP_CALLS.load(Ordering::Relaxed),
            ADD_SINK_CALLS.load(Ordering::Relaxed)
        );
        if ADD_SINK.get().is_some() {
            read_layouts();
        }
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
    let reloc = RELOC.get_or_init(|| reloc);

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

    if [major, minor, build] == layout::v1_7_104::VERSION {
        let id = fns::BSTEventSource_MenuOpenCloseEvent__AddEventSink;
        match unsafe { reloc.hook(id, add_sink_detour as AddEventSink) } {
            Ok(h) => {
                log!("hooked BSTEventSource<MenuOpenCloseEvent>::AddEventSink (fns id {id}) at {:#x}", h.target());
                let _ = ADD_SINK.set(h);
            }
            Err(e) => log!("{e}"),
        }
    } else {
        log!("layouts are generated for 1.7.104 only; skipping the layout reads");
    }

    if let Some(m) = skse.messaging() {
        let _ = m.listen_skse(skse.plugin_handle(), on_message);
    }
    true
}
