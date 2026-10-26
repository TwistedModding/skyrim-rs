//! The crash suite's plugin: the console command `SkyrimRsCrash <what>` arms one crash, which
//! then happens where the scenario triggers it. `null`, `overflow` and `write` fire in the
//! plugin's `SAVE_GAME` handler, on the main thread, at the next console `save`; `hook` fires
//! inside the plugin's `Before` subscriber on `Actor::ModActorValueImpl` at the next
//! `player.damageactorvalue`, on the console's job thread. The plugin logs the thread id of
//! every step to `skyrim-rs-crash-test.log` so the suite can compare them with the crash log.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};
use std::fs::File;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};

use framework::Host;
use framework::hooks::Options;
use framework::hooks::ops::Actor__ModActorValueImpl;
use layout::v1_7_104::classes;
use skyrim::input::{Arg, Command, CommandContext, Param, ScriptParamType};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

framework::plugin! {
    name: "skyrim-rs-crash-test",
    load: load,
}

const NONE: u8 = 0;
const NULL: u8 = 1;
const OVERFLOW: u8 = 2;
const WRITE: u8 = 3;
const HOOK: u8 = 4;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static HOST: OnceLock<&'static Host> = OnceLock::new();
static ARMED: AtomicU8 = AtomicU8::new(NONE);

fn log(line: &str) {
    if let Some(f) = LOG.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

fn thread() -> u32 {
    unsafe { GetCurrentThreadId() }
}

fn load(host: &'static Host) -> bool {
    let skse = host.skse();
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-crash-test") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    let _ = HOST.set(host);
    if let Err(e) = skyrim::init(skse) {
        log(&format!("FAIL init: {e}"));
        return false;
    }
    log(&format!("PASS load: load thread {}", thread()));
    let subscribed = host.hooks().before(Actor__ModActorValueImpl, Options::new(host.name()), |_: &mut (*mut classes::Actor, i32, i32, f32, *mut c_void)| {
        if ARMED.load(Ordering::Relaxed) == HOOK {
            log(&format!("crashing on thread {} (hook)", thread()));
            null_read();
        }
    });
    match subscribed {
        Ok(id) => log(&format!("PASS hook: Before on Actor::ModActorValueImpl subscribed as #{}", id.get())),
        Err(e) => {
            log(&format!("FAIL hook: {e}"));
            return false;
        }
    }
    log("summary crash load: 2 pass, 0 fail");
    host.on_message(on_message);
    true
}

fn on_message(msg: &skse::Message) {
    match msg.kind {
        skse::MessagingInterface::DATA_LOADED => {
            let host = HOST.get().expect("set in load");
            match host.register_command::<CrashCommand>() {
                Ok(slot) => log(&format!("PASS command: SkyrimRsCrash took over console opcode {:#x}", slot.opcode)),
                Err(e) => log(&format!("FAIL command: {e}")),
            }
            log("summary crash dataLoaded: 1 pass, 0 fail");
        }
        skse::MessagingInterface::SAVE_GAME => {
            let armed = ARMED.load(Ordering::Relaxed);
            let what = match armed {
                NULL => "null",
                OVERFLOW => "overflow",
                WRITE => "write",
                _ => return,
            };
            log(&format!("crashing on thread {} ({what})", thread()));
            match armed {
                NULL => null_read(),
                OVERFLOW => {
                    let _ = recurse(0);
                }
                _ => image_write(),
            }
        }
        _ => {}
    }
}

#[inline(never)]
fn null_read() {
    let value = unsafe { core::ptr::read_volatile(core::hint::black_box(core::ptr::null::<u32>())) };
    core::hint::black_box(value);
}

/// Writes into the game's code, the first page of `.text`.
#[inline(never)]
fn image_write() {
    let target = (hook::module_base() + 0x1000) as *mut u8;
    unsafe { core::ptr::write_volatile(core::hint::black_box(target), 0) };
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u64) -> u64 {
    let mut frame = [0u8; 8192];
    frame[usize::try_from(depth % 8192).unwrap_or(0)] = u8::try_from(depth & 0xFF).unwrap_or(0);
    core::hint::black_box(&mut frame);
    recurse(depth + 1).wrapping_add(u64::from(frame[0]))
}

static PARAMS: [Param; 1] = [Param::new(c"what", ScriptParamType::String, false)];

/// `SkyrimRsCrash null | overflow | write | hook`.
struct CrashCommand;

impl Command for CrashCommand {
    const NAME: &'static core::ffi::CStr = c"SkyrimRsCrash";
    const HELP: &'static core::ffi::CStr = c"skyrim-rs crash test: arm null, overflow or write (fires at the next save) or hook (fires at the next damageactorvalue)";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let (armed, line) = match context.args() {
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("null") => (NULL, "armed null: a null read in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("overflow") => (OVERFLOW, "armed overflow: unbounded recursion in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("write") => (WRITE, "armed write: a write into SkyrimSE.exe's code in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("hook") => (HOOK, "armed hook: a null read inside the Before subscriber at the next damageactorvalue"),
            _ => (NONE, "SkyrimRsCrash null | overflow | write | hook"),
        };
        ARMED.store(armed, Ordering::Relaxed);
        log(&format!("{line} (command thread {})", thread()));
        skyrim::input::print(&format!("SkyrimRsCrash: {line}"));
        true
    }
}
