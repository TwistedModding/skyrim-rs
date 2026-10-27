//! The reload suite's plugin: one generation per build tag (`SKYRIM_RS_RELOAD_TAG`, read at
//! compile time), each registering through the host a `Before` on the player's health damage
//! path, the native `SkyrimRsReloadTest.Version()` returning the tag, a `TESEquipEvent` sink, the
//! console command `SkyrimRsReload`, a message handler and an overlay window, and handing a
//! reload counter to the next generation. Each generation reports what it saw at the save of its
//! part of the scenario in `skyrim-rs-reload-test.log` (`PASS`/`FAIL`/`summary` lines).

use core::ffi::c_void;
use std::fmt::Display;
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use framework::hooks::ops::Actor__ModActorValueImpl;
use framework::hooks::{self, Options};
use framework::{Host, egui};
use skyrim::actors::Modifier;
use sre::{ActorValue, BSEventNotifyControl, TESEquipEvent};
use skyrim::input::{Command, CommandContext, Param};
use skyrim::papyrus::VirtualMachine;

framework::plugin! {
    name: "skyrim-rs-reload-test",
    load: load,
    unload: unload,
    save_state: save_state,
    restore_state: restore_state,
}

const CLASS: &str = "SkyrimRsReloadTest";
const LOG: &str = "skyrim-rs-reload-test";
const TAG: &str = match option_env!("SKYRIM_RS_RELOAD_TAG") {
    Some(tag) => tag,
    None => "1",
};

static HOST: OnceLock<&'static Host> = OnceLock::new();
/// How many reloads the plugin survived, handed from generation to generation.
static RELOADS: AtomicU32 = AtomicU32::new(0);
static KINDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static DAMAGE: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static EQUIPS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static COMMAND_CALLS: AtomicU32 = AtomicU32::new(0);
static NATIVE_CALLS: AtomicU32 = AtomicU32::new(0);
static FRAMES: AtomicU32 = AtomicU32::new(0);
static HOOK: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static NATIVE: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static COMMAND: Mutex<Option<Result<String, String>>> = Mutex::new(None);

type ModArgs = (*mut sre::Actor, i32, i32, f32, *mut c_void);
type Test = fn() -> Result<String, String>;

fn tag() -> i32 {
    TAG.parse().unwrap_or(0)
}

fn log(line: impl Display) {
    if let Some(host) = HOST.get()
        && let Ok(mut f) = skse::log::append(host.skse().runtime_version, LOG)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn check(ok: bool, what: impl Display) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

fn run(phase: &str, tests: &[(&str, Test)]) {
    let (mut pass, mut fail) = (0, 0);
    for (name, test) in tests {
        match test() {
            Ok(detail) => {
                pass += 1;
                log(format!("PASS {name}: {detail}"));
            }
            Err(detail) => {
                fail += 1;
                log(format!("FAIL {name}: {detail}"));
            }
        }
    }
    log(format!("summary reload {phase}: {pass} pass, {fail} fail"));
}

fn load(host: &'static Host) -> bool {
    let _ = HOST.set(host);
    if skyrim::init(host.skse()).is_err() {
        return false;
    }
    log(format!("tag {TAG}: loading"));
    *HOOK.lock().unwrap() = Some(subscribe(host));
    let sink = host.add_sink::<TESEquipEvent>(|event, _| {
        let event = format!("{:08X} equipped {}", event.baseObject, event.equipped);
        log(format!("tag {TAG}: equip event {event}"));
        EQUIPS.lock().unwrap().push(event);
        BSEventNotifyControl::Continue
    });
    log(format!("tag {TAG}: sink {sink:?}"));
    host.on_papyrus(on_papyrus);
    host.on_message(on_message);
    host.add_window("main", "skyrim-rs reload test", window);
    true
}

fn unload() {
    log(format!("tag {TAG}: unloading after {} reloads", RELOADS.load(Ordering::Relaxed)));
}

fn save_state() -> Vec<u8> {
    RELOADS.load(Ordering::Relaxed).to_le_bytes().to_vec()
}

fn restore_state(bytes: &[u8]) {
    let previous = bytes.try_into().map_or(0, u32::from_le_bytes);
    RELOADS.store(previous + 1, Ordering::Relaxed);
    log(format!("tag {TAG}: restored {previous} reloads from the previous generation, now {}", previous + 1));
}

fn subscribe(host: &'static Host) -> Result<String, String> {
    let hooks = host.hooks();
    let foreign = hooks.before(Actor__ModActorValueImpl, Options::new("someone-else"), |_: &mut ModArgs| {});
    check(matches!(foreign, Err(hooks::Error::Owner { .. })), format!("a subscription owned by another name was not refused: {foreign:?}"))?;
    let id = hooks
        .before(Actor__ModActorValueImpl, Options::new(host.name()), |args: &mut ModArgs| {
            let player = sre::PlayerCharacter::singleton().map_or(core::ptr::null(), |p| core::ptr::from_ref(p).cast::<c_void>());
            if args.0.cast_const().cast::<c_void>() == player && args.2 == ActorValue::Health as i32 && args.1 == Modifier::Damage as i32 && args.3 < 0.0 {
                DAMAGE.lock().unwrap().push(args.3);
                log(format!("tag {TAG}: health damage {}", args.3));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("Before on Actor::ModActorValueImpl as {id:?}; {}", foreign.unwrap_err()))
}

fn on_papyrus(vm: &'static VirtualMachine) {
    let host = HOST.get().expect("set in load");
    let registered = host.register_native(vm, CLASS, "Version", || {
        NATIVE_CALLS.fetch_add(1, Ordering::Relaxed);
        tag()
    });
    *NATIVE.lock().unwrap() = Some(registered.map(|f| format!("{CLASS}.{} bound through the host", f.name())).map_err(|e| e.to_string()));
}

struct ReloadCommand;

impl Command for ReloadCommand {
    const NAME: &'static core::ffi::CStr = c"SkyrimRsReload";
    const HELP: &'static core::ffi::CStr = c"skyrim-rs reload test: prints the generation's tag and reload count";
    const PARAMS: &'static [Param] = &[];

    fn execute(context: &mut CommandContext<'_>) -> bool {
        COMMAND_CALLS.fetch_add(1, Ordering::Relaxed);
        skyrim::input::print(&format!("SkyrimRsReload: tag {TAG}, reloads {}", RELOADS.load(Ordering::Relaxed)));
        context.set_result(f64::from(tag()));
        true
    }
}

fn on_message(msg: &skse::Message) {
    KINDS.lock().unwrap().push(msg.kind);
    log(format!("tag {TAG}: message {}", msg.kind));
    match msg.kind {
        skse::MessagingInterface::DATA_LOADED => {
            let registered = HOST.get().expect("set in load").register_command::<ReloadCommand>();
            *COMMAND.lock().unwrap() = Some(registered.map(|slot| format!("SkyrimRsReload on console opcode {:#x} ({})", slot.opcode, slot.replaced)).map_err(|e| e.to_string()));
        }
        skse::MessagingInterface::SAVE_GAME => run(&format!("tag{TAG}"), &[("generation", generation), ("hook", hook), ("native", native), ("sink", sink), ("command", command), ("window", drawn), ("messages", messages)]),
        _ => {}
    }
}

fn window(window: egui::Window<'_>, ctx: &egui::Context) {
    let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if frames == 1 {
        log(format!("tag {TAG}: first window frame"));
    }
    window.default_pos([40.0, 80.0]).show(ctx, |ui| ui.label(format!("tag {TAG}, reloads {}, frame {frames}", RELOADS.load(Ordering::Relaxed))));
}

/// The reload counter the previous generation handed over equals the number of reloads this tag is behind.
fn generation() -> Result<String, String> {
    let reloads = RELOADS.load(Ordering::Relaxed);
    let expected = u32::try_from(tag() - 1).unwrap_or(0);
    check(reloads == expected, format!("tag {TAG} with {reloads} reloads restored, expected {expected}"))?;
    Ok(format!("tag {TAG}, {reloads} reloads restored"))
}

fn hook() -> Result<String, String> {
    let subscribed = HOOK.lock().unwrap().clone().ok_or("no subscription result")??;
    let seen = DAMAGE.lock().unwrap().clone();
    check(seen == [-10.0], format!("the Before saw {seen:?}, expected [-10]"))?;
    Ok(format!("{subscribed}; the Before saw {seen:?}"))
}

fn native() -> Result<String, String> {
    let bound = NATIVE.lock().unwrap().clone().ok_or("the Papyrus callback never ran")??;
    let calls = NATIVE_CALLS.load(Ordering::Relaxed);
    check(calls == 1, format!("Version called {calls} times, expected 1"))?;
    Ok(format!("{bound}; Version called once and returned {TAG}"))
}

fn sink() -> Result<String, String> {
    let seen = EQUIPS.lock().unwrap().clone();
    check(!seen.is_empty(), "no equip event reached the sink")?;
    Ok(format!("{} equip events: {}", seen.len(), seen.join(" | ")))
}

fn command() -> Result<String, String> {
    let registered = COMMAND.lock().unwrap().clone().ok_or("DATA_LOADED never arrived")??;
    let calls = COMMAND_CALLS.load(Ordering::Relaxed);
    check(calls == 1, format!("SkyrimRsReload ran {calls} times, expected 1"))?;
    Ok(format!("{registered}; ran once"))
}

fn drawn() -> Result<String, String> {
    let frames = FRAMES.load(Ordering::Relaxed);
    check(frames > 0, "the window never ran")?;
    Ok(format!("window ran in {frames} frames"))
}

fn messages() -> Result<String, String> {
    let kinds = KINDS.lock().unwrap().clone();
    check(kinds.contains(&skse::MessagingInterface::DATA_LOADED) && kinds.last() == Some(&skse::MessagingInterface::SAVE_GAME), format!("kinds {kinds:?}, expected DATA_LOADED then SAVE_GAME last"))?;
    Ok(format!("kinds {kinds:?}"))
}
