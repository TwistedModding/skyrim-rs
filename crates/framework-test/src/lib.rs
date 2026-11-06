// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The framework suite's plugin, loaded by the host from `Data\SKSE\Plugins\skyrim-runtime-extender\`: it checks
//! the forwarded SKSE interface and the resolved load order in `load` and appends its name to
//! `skyrim-rs-order.log` like the `order-*` plugins, records the loader messages the host fans out,
//! registers `SkyrimRsFrameworkTest.Twice` through the host's Papyrus callback, adds an overlay
//! window and a HUD painter that boxes the player and checks the frame camera's projection, then reports at `DataLoaded`, after a load and at the save in `skyrim-rs-framework-test.log`
//! in the test plugin's `PASS`/`FAIL`/`summary` format.

use std::fs::File;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU32, Ordering};

use framework::{Host, View, egui};
use skyrim::papyrus::VirtualMachine;
use skyrim::render::NiAVObjectExt;

framework::plugin! {
    name: "skyrim-rs-framework-test",
    load: load,
}

const CLASS: &str = "SkyrimRsFrameworkTest";

static LOG: Mutex<Option<File>> = Mutex::new(None);
static KINDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static REGISTERED: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static CALLS: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static FRAMES: AtomicU32 = AtomicU32::new(0);
static HUD_FRAMES: AtomicU32 = AtomicU32::new(0);
/// The projection checks at the last HUD frame with the player's 3D loaded.
static PROJECTION: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static HOST: OnceLock<&'static Host> = OnceLock::new();

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg)*);
        }
    };
}

type Test = fn() -> Result<String, String>;

fn run(phase: &str, tests: &[(&str, Test)]) {
    let (mut pass, mut fail) = (0, 0);
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
    log!("summary framework {phase}: {pass} pass, {fail} fail");
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_owned()) }
}

fn load(host: &'static Host) -> bool {
    let skse = host.skse();
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-framework-test") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    let _ = HOST.set(host);
    let _ = skse::log::append(skse.runtime_version, "skyrim-rs-order").and_then(|mut f| writeln!(f, "skyrim-rs-framework-test"));
    run("load", &[("interface", interface), ("order", order)]);
    host.on_message(on_message);
    host.on_papyrus(on_papyrus);
    host.add_tab("skyrim-rs framework test").section("main", tab);
    host.add_hud(hud);
    true
}

/// The forwarded interface, inside `load`: the game layer initializes from it and the plugin handle answers.
fn interface() -> Result<String, String> {
    let skse = HOST.get().ok_or("no host")?.skse();
    skyrim::init(skse).map_err(|e| e.to_string())?;
    let handle = skse.plugin_handle();
    check(handle != skse::INVALID_HANDLE, "no plugin handle inside load")?;
    let messaging = skse.messaging().ok_or("no messaging interface")?;
    check(messaging.interface_version == skse::MessagingInterface::VERSION, "messaging interface version")?;
    let [major, minor, build, _] = skse::runtime::unpack(skse.runtime_version);
    Ok(format!("abi {}, api {}; runtime {major}.{minor}.{build}, skse {:08x}, handle {handle} (the host's), game layer initialized", framework::ABI, framework::abi::API, skse.skse_version))
}

/// The resolved order, inside `load`: the suite's three plugins as their constraints place them, the cycle pair absent.
fn order() -> Result<String, String> {
    let plugins = HOST.get().ok_or("no host")?.plugins();
    let listed: Vec<String> = plugins.iter().map(|p| format!("{} ({}; {})", p.name, p.file, p.constraints)).collect();
    let names: Vec<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
    check(names == ["skyrim-rs-order-head", "skyrim-rs-framework-test", "skyrim-rs-order-tail"], &format!("plugins {listed:?}"))?;
    check(plugins[0].constraints.priority == 1 && plugins[2].constraints.after == ["*"], &format!("constraints {listed:?}"))?;
    Ok(listed.join(", "))
}

fn on_message(msg: &skse::Message) {
    KINDS.lock().unwrap().push(msg.kind);
    match msg.kind {
        skse::MessagingInterface::DATA_LOADED => run("dataLoaded", &[("startup_messages", startup_messages), ("papyrus", papyrus)]),
        skse::MessagingInterface::POST_LOAD_GAME => run("postLoadGame", &[("load_messages", load_messages)]),
        skse::MessagingInterface::SAVE_GAME => run("saveGame", &[("native", native), ("tab", drawn), ("hud", hud_drawn)]),
        _ => {}
    }
}

fn on_papyrus(vm: &'static VirtualMachine) {
    let registered = HOST.get().ok_or("no host".to_owned()).and_then(|host| {
        host.register_native(vm, CLASS, "Twice", |x: i32| {
            CALLS.lock().unwrap().push(x);
            x * 2
        })
        .map_err(|e| e.to_string())
    });
    *REGISTERED.lock().unwrap() = Some(registered.map(|f| format!("{CLASS}.{} bound through the host, refs {}", f.name(), f.ref_count())));
}

fn tab(ui: &mut egui::Ui) {
    let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    ui.label(format!("drawn by a framework plugin, frame {frames}"));
}

fn hud(painter: &egui::Painter, view: &View) {
    HUD_FRAMES.fetch_add(1, Ordering::Relaxed);
    let Some(root) = sre::PlayerCharacter::singleton().and_then(|p| p.loaded_data()?.node3_d()) else { return };
    let player = view.project_bound(root);
    if let Some(rect) = player {
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(2.0, egui::Color32::GOLD), egui::StrokeKind::Outside);
        painter.text(rect.center_top(), egui::Align2::CENTER_BOTTOM, "skyrim-rs HUD", egui::FontId::proportional(20.0), egui::Color32::GOLD);
    }
    *PROJECTION.lock().unwrap() = Some(projection(view, player));
}

/// A point straight ahead of the camera lands on the screen's centre, one behind it does not land.
fn projection(view: &View, player: Option<egui::Rect>) -> Result<String, String> {
    let rotation = sre::get_main_camera().ok_or("no frame camera")?.world_transform().rotation;
    let eye = view.eye();
    let along = |d: f32| core::array::from_fn(|r| eye[r] + d * rotation[r][0]);
    let centre = view.screen().center();
    let ahead = view.project(along(1000.0)).ok_or("a point ahead of the camera did not project")?;
    check(ahead.distance(centre) < 1.0, &format!("a point ahead landed at {ahead:?}, the centre is {centre:?}"))?;
    check(view.project(along(-1000.0)).is_none(), "a point behind the camera projected")?;
    Ok(format!("ahead at {ahead:?} on a {:?} screen, behind not projected, player box {player:?}", view.screen().size()))
}

fn hud_drawn() -> Result<String, String> {
    let (frames, tab) = (HUD_FRAMES.load(Ordering::Relaxed), FRAMES.load(Ordering::Relaxed));
    check(frames > tab, &format!("the HUD ran in {frames} frames, the tab in {tab}: not while the overlay was closed"))?;
    let projection = PROJECTION.lock().unwrap().clone().ok_or("the player's 3D never loaded in a HUD frame")??;
    Ok(format!("HUD ran in {frames} frames (tab {tab}); {projection}"))
}

fn startup_messages() -> Result<String, String> {
    let kinds = KINDS.lock().unwrap().clone();
    check(kinds == [0, 1, 6, 8], &format!("kinds {kinds:?}, expected [0, 1, 6, 8]"))?;
    Ok(format!("kinds {kinds:?} through the host"))
}

fn load_messages() -> Result<String, String> {
    let kinds = KINDS.lock().unwrap().clone();
    check(kinds.ends_with(&[2, 3]), &format!("kinds {kinds:?}, expected to end with [2, 3]"))?;
    Ok(format!("kinds {kinds:?}"))
}

fn papyrus() -> Result<String, String> {
    REGISTERED.lock().unwrap().clone().ok_or("the host's Papyrus callback never ran")?
}

fn native() -> Result<String, String> {
    let calls = CALLS.lock().unwrap().clone();
    check(calls == [21], &format!("Twice called with {calls:?}, expected [21]"))?;
    Ok(format!("Twice called with {calls:?}"))
}

fn drawn() -> Result<String, String> {
    let frames = FRAMES.load(Ordering::Relaxed);
    check(frames > 0, "the tab never ran")?;
    Ok(format!("tab ran in {frames} frames"))
}
