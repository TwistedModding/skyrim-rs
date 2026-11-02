//! In-game test plugin: tests of the generated methods and the `skyrim` layer, grouped by subsystem, logging
//! `PASS name: detail` / `FAIL name: detail` and a `summary <suite> <phase>` line per run.
//! `forms` runs at `DataLoaded` and after a game is loaded; `actors` runs after a game is loaded
//! and again when the game is saved, by which time the `DevBench` scenario has put the player into
//! sneak with the weapon drawn; `events` registers its sinks at `DataLoaded` and checks what they
//! received at the save; `papyrus` registers its natives in the SKSE `PapyrusInterface` callback,
//! checks handles and script types after a load and the calls it received at the save; `render`
//! detours `NiAVObject::Cull` at `DataLoaded`, reads the scene after a load and reports the caster
//! counts at the save; `frame` detours `TESCamera::Update` and `PresentFrame`, samples the camera
//! per state and reads the scene graph, shaders, accumulators, window and sky at the saves.
//! `animation` post-hooks the pose write and the world update, counts the graph events of the
//! player's graph 0 and samples the head bone. `ui` registers its menu and detours the menu draw,
//! the message queue and the HUD notification at `DataLoaded`, reads the UI after a load and
//! checks what it saw at the save. `input` records the keystrokes `DevBench` injects and adds the
//! `SkyrimRsTest` console command, which opens and closes the ui suite's menu. `save` keeps a
//! record next to every save and reads it back after every load. `audio` plays, follows and stops
//! sounds on a thread of its own after every load and samples a category's volume.
//! `scripts/verify.ps1` compares the log against `DevBench`'s answers.
//!
//! `overlay` runs only in mode `overlay`: it installs the overlay at load with a window `DevBench`
//! clicks through the keyboard and checks the input capture and the drawn pixels at the save.
//!
//! A run mode in `Data\SKSE\Plugins\skyrim-rs-test.mode` next to the DLL bisects a crash:
//! `nohook` skips the render detours, `passive` installs them but never counts; `overlay` adds the
//! overlay suite, whose sink at the front of the input source the input suite does not expect.

use std::fs::File;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::Ordering;

use sre::{FormType, TESForm};

skse::plugin! {
    name: "skyrim-rs-test",
    author: "TwistedModding",
    version: 1,
    load: load,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);
static MODE: OnceLock<String> = OnceLock::new();
static LOG_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// The run mode from the mode file, empty when there is none.
fn mode() -> &'static str {
    MODE.get().map_or("", String::as_str)
}

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = $crate::LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg)*);
        }
    };
}

mod actors;
mod animation;
mod audio;
mod events;
mod forms;
mod frame;
mod input;
mod overlay;
mod papyrus;
mod render;
mod save;
mod ui;

const PLAYER: u32 = 0x14;
const IRON_SWORD: u32 = 0x12EB7;

type Test = fn() -> Result<String, String>;

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_owned()) }
}

fn form(id: u32) -> Result<&'static TESForm, String> {
    sre::find_form_by_id(id).ok_or_else(|| format!("FindFormByID({id:08X}) returned null"))
}

fn third_person_root(refr: &sre::TESObjectREFR) -> Option<&sre::NiNode> {
    refr.loaded_data()?.node3_d()
}

fn file_name(file: &sre::TESFile) -> &str {
    core::ffi::CStr::from_bytes_until_nul(&file.fileName).ok().and_then(|s| s.to_str().ok()).unwrap_or("")
}

/// A keyword's editor id from its `keyword` field, a `BSFixedString` the ledger types as a pointer.
fn keyword_field(keyword: &sre::BGSKeyword) -> &skyrim::BSFixedString {
    skyrim::BSFixedString::view(unsafe { &*(&raw const keyword.keyword).cast::<sre::BSFixedString>() })
}

/// A form as the logs name it: its record type and its form id (`ACHR 00000014`).
fn describe(form: &TESForm) -> String {
    format!("{} {:08X}", FormType::from_u8(form.formType).map_or("????", FormType::signature), form.formID)
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

/// The save name a `SAVE_GAME` message carries.
fn save_name(msg: &skse::Message) -> String {
    if msg.data.is_null() {
        return String::new();
    }
    let bytes = unsafe { core::slice::from_raw_parts(msg.data.cast::<u8>(), msg.data_len as usize) };
    String::from_utf8_lossy(bytes).trim_end_matches('\0').to_owned()
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    let msg = unsafe { &*msg };
    match msg.kind {
        skse::MessagingInterface::DATA_LOADED => {
            run("forms", "dataLoaded", forms::TESTS);
            run("events", "dataLoaded", events::SETUP);
            run("save", "dataLoaded", &[("install", save::install)]);
            if mode() == "nohook" {
                log!("render hooks skipped (mode nohook)");
            } else {
                run("render", "dataLoaded", &[("install", render::install)]);
                if mode() != "overlay" {
                    run("frame", "dataLoaded", &[("install", frame::install)]);
                }
                run("animation", "dataLoaded", &[("install", animation::install)]);
                run("ui", "dataLoaded", &[("install", ui::install)]);
                run("input", "dataLoaded", &[("install", input::install)]);
            }
            if mode() == "overlay" {
                run("overlay", "dataLoaded", &[("register", overlay::register)]);
            }
        }
        skse::MessagingInterface::POST_LOAD_GAME => {
            events::LOADS.fetch_add(1, Ordering::Relaxed);
            run("forms", "postLoadGame", forms::TESTS);
            actors::STATE_CHANGED.store(false, Ordering::Relaxed);
            run("actors", "postLoadGame", actors::TESTS);
            run("papyrus", "postLoadGame", papyrus::TESTS);
            render::reset(mode() != "passive");
            run("render", "postLoadGame", render::TESTS);
            frame::reset();
            run("frame", "postLoadGame", frame::TESTS);
            if mode() != "passive" {
                run("animation", "postLoadGame", &[&[("register", animation::register as Test)][..], animation::TESTS].concat());
            }
            if mode() != "nohook" {
                ui::reset();
                run("ui", "postLoadGame", ui::TESTS);
                input::reset();
                run("input", "postLoadGame", input::TESTS);
            }
            run("audio", "postLoadGame", audio::TESTS);
            audio::start();
            if mode() == "overlay" {
                run("overlay", "postLoadGame", overlay::TESTS);
            }
            render::SAVES.store(0, Ordering::Relaxed);
        }
        skse::MessagingInterface::PRE_LOAD_GAME | skse::MessagingInterface::NEW_GAME => {
            render::suspend();
            animation::suspend();
        }
        skse::MessagingInterface::SAVE_GAME => {
            actors::STATE_CHANGED.store(true, Ordering::Relaxed);
            run("actors", "saveGame", actors::TESTS);
            run("events", "saveGame", events::TESTS);
            run("papyrus", "saveGame", papyrus::CALLS_TESTS);
            let save = render::SAVES.fetch_add(1, Ordering::Relaxed) + 1;
            run("render", &format!("save{save}"), &[render::TESTS, render::CASTER_TESTS].concat());
            run("frame", &format!("save{save}"), &[frame::TESTS, frame::SAMPLE_TESTS].concat());
            run("animation", &format!("save{save}"), &[animation::TESTS, animation::RUN_TESTS].concat());
            run("ui", "saveGame", ui::SAVE_TESTS);
            run("input", "saveGame", input::SAVE_TESTS);
            run("audio", "saveGame", audio::SAVE_TESTS);
            if mode() == "overlay" {
                run("overlay", "saveGame", overlay::SAVE_TESTS);
            }
            if save_name(msg) == save::FINAL {
                run("save", "saveGame", save::SAVE_TESTS);
            }
        }
        _ => {}
    }
}

/// Logs what `QueryInterface` answers for ids 0 to 9 and the first dword of each object.
fn probe_interfaces(skse: &skse::Interface) {
    let answers: Vec<String> = (0..10u32)
        .map(|id| {
            let p = unsafe { (skse.query_interface)(id) };
            if p.is_null() { format!("{id}:null") } else { format!("{id}:{:#x}:v{}", p as usize, unsafe { p.cast::<u32>().read() }) }
        })
        .collect();
    log!("interfaces: {}; plugin handle {}, release index {}", answers.join(" "), skse.plugin_handle(), unsafe { (skse.get_release_index)() });
}

unsafe extern "C" fn on_register(registry: *mut core::ffi::c_void) {
    papyrus::REGISTRY.store(registry, Ordering::Relaxed);
    run("papyrus", "register", papyrus::REGISTER);
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-test") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    if let Ok(dir) = skse::log::dir(skse.runtime_version) {
        let _ = LOG_DIR.set(dir);
    }
    match crash::install(&crash::Options { name: "skyrim-rs-test", runtime_version: skse.runtime_version, build: env!("CARGO_PKG_VERSION"), facts: None, symbol_path: None }) {
        Ok(installed) => log!("crash handler: {installed:?}"),
        Err(e) => log!("crash handler not installed: {e}"),
    }
    let mode_file = std::env::current_exe().map(|exe| exe.with_file_name("Data").join("SKSE").join("Plugins").join("skyrim-rs-test.mode"));
    let _ = MODE.set(mode_file.and_then(std::fs::read_to_string).map(|m| m.trim().to_owned()).unwrap_or_default());
    if !mode().is_empty() {
        log!("run mode: {}", mode());
    }
    let [major, minor, build, store] = skse::runtime::unpack(skse.runtime_version);
    log!("runtime {major}.{minor}.{build} store {store}, skse {:08x}", skse.skse_version);
    probe_interfaces(skse);
    if let Err(e) = skyrim::init(skse) {
        log!("FAIL init: {e}");
        return false;
    }
    if mode() == "overlay" {
        run("overlay", "load", &[("install", overlay::install)]);
    }
    log!("PASS init: skyrim::forms, skyrim::actors, skyrim::events, skyrim::papyrus, skyrim::render, skyrim::ui, skyrim::input, skyrim::save and skyrim::audio ids resolved, module base {:#x}", skyrim::reloc().base());
    match (skse.messaging(), skse.papyrus()) {
        (Some(m), Some(p)) if m.listen_skse(skse.plugin_handle(), on_message) && p.register(on_register) => true,
        _ => {
            log!("FAIL init: no messaging or papyrus interface");
            false
        }
    }
}
