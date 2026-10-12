//! In-game test plugin: one test per `skyrim` API, grouped by subsystem, logging
//! `PASS name: detail` / `FAIL name: detail` and a `summary <suite> <phase>` line per run.
//! `forms` runs at `DataLoaded` and after a game is loaded; `actors` runs after a game is loaded
//! and again when the game is saved, by which time the `DevBench` scenario has put the player into
//! sneak with the weapon drawn. `scripts/verify.ps1` compares the log against `DevBench`'s answers.

use std::fs::File;
use std::io::Write;
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
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg);
        }
    };
}

mod actors;
mod forms;

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
        skse::MessagingInterface::DATA_LOADED => run("forms", "dataLoaded", forms::TESTS),
        skse::MessagingInterface::POST_LOAD_GAME => {
            run("forms", "postLoadGame", forms::TESTS);
            actors::STATE_CHANGED.store(false, Ordering::Relaxed);
            run("actors", "postLoadGame", actors::TESTS);
        }
        skse::MessagingInterface::SAVE_GAME => {
            actors::STATE_CHANGED.store(true, Ordering::Relaxed);
            run("actors", "saveGame", actors::TESTS);
        }
        _ => {}
    }
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
    log!("PASS init: skyrim::forms and skyrim::actors ids resolved, module base {:#x}", skyrim::reloc().base());
    match skse.messaging() {
        Some(m) if m.listen_skse(skse.plugin_handle(), on_message) => true,
        _ => {
            log!("FAIL init: no messaging interface");
            false
        }
    }
}
