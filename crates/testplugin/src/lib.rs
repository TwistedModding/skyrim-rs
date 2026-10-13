//! In-game test plugin: one test per `skyrim` API, grouped by subsystem, logging
//! `PASS name: detail` / `FAIL name: detail` and a `summary <suite> <phase>` line per run.
//! `forms` runs at `DataLoaded` and after a game is loaded; `actors` runs after a game is loaded
//! and again when the game is saved, by which time the `DevBench` scenario has put the player into
//! sneak with the weapon drawn; `events` registers its sinks at `DataLoaded` and checks what they
//! received at the save; `papyrus` registers its natives in the SKSE `PapyrusInterface` callback,
//! checks handles and script types after a load and the calls it received at the save; `render`
//! detours `NiAVObject::Cull` at `DataLoaded`, reads the scene after a load and reports the caster
//! counts at the save. `scripts/verify.ps1` compares the log against `DevBench`'s answers.

use std::fs::File;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use skyrim::forms::{FormId, TESForm};

skse::plugin! {
    name: "skyrim-rs-test",
    author: "TwistedModding",
    version: 1,
    load: load,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = $crate::LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg);
        }
    };
}

mod actors;
mod events;
mod forms;
mod papyrus;
mod render;

const PLAYER: FormId = FormId(0x14);
const IRON_SWORD: FormId = FormId(0x12EB7);

type Test = fn() -> Result<String, String>;

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_owned()) }
}

fn form(id: FormId) -> Result<&'static TESForm, String> {
    id.lookup().ok_or_else(|| format!("LookupFormByID({id}) returned null"))
}

fn run(suite: &str, phase: &str, tests: &[(&str, Test)]) {
    let mut pass = 0;
    let mut fail = 0;
    for (name, test) in tests {
        match test() {
            Ok(detail) => {
                pass += 1;
                log!("PASS {name}: {detail}");
            }
            Err(detail) => {
                fail += 1;
                log!("FAIL {name}: {detail}");
            }
        }
    }
    log!("summary {suite} {phase}: {pass} pass, {fail} fail");
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    let msg = unsafe { &*msg };
    match msg.kind {
        skse::MessagingInterface::DATA_LOADED => {
            run("forms", "dataLoaded", forms::TESTS);
            run("events", "dataLoaded", events::SETUP);
            run("render", "dataLoaded", &[("install", render::install)]);
        }
        skse::MessagingInterface::POST_LOAD_GAME => {
            events::LOADS.fetch_add(1, Ordering::Relaxed);
            run("forms", "postLoadGame", forms::TESTS);
            actors::STATE_CHANGED.store(false, Ordering::Relaxed);
            run("actors", "postLoadGame", actors::TESTS);
            run("papyrus", "postLoadGame", papyrus::TESTS);
            render::reset();
            run("render", "postLoadGame", render::TESTS);
        }
        skse::MessagingInterface::PRE_LOAD_GAME | skse::MessagingInterface::NEW_GAME => render::suspend(),
        skse::MessagingInterface::SAVE_GAME => {
            actors::STATE_CHANGED.store(true, Ordering::Relaxed);
            run("actors", "saveGame", actors::TESTS);
            run("events", "saveGame", events::TESTS);
            run("papyrus", "saveGame", papyrus::CALLS_TESTS);
            run("render", "saveGame", render::CASTER_TESTS);
        }
        _ => {}
    }
}

unsafe extern "C" fn on_register(registry: *mut core::ffi::c_void) -> bool {
    papyrus::REGISTRY.store(registry, Ordering::Relaxed);
    run("papyrus", "register", papyrus::REGISTER);
    true
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-test") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    let [major, minor, build, store] = skse::runtime::unpack(skse.runtime_version);
    log!("runtime {major}.{minor}.{build} store {store}, skse {:08x}", skse.skse_version);
    if let Err(e) = skyrim::init(skse) {
        log!("FAIL init: {e}");
        return false;
    }
    log!("PASS init: skyrim::forms, skyrim::actors, skyrim::events, skyrim::papyrus and skyrim::render ids resolved, module base {:#x}", skyrim::reloc().base());
    match (skse.messaging(), skse.papyrus()) {
        (Some(m), Some(p)) if m.listen_skse(skse.plugin_handle(), on_message) && p.register(on_register) => true,
        _ => {
            log!("FAIL init: no messaging or papyrus interface");
            false
        }
    }
}
