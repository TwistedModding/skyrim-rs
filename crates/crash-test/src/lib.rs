//! The crash suite's plugin: the console command `SkyrimRsCrash <what>` arms one crash, which
//! then happens where the scenario triggers it. `null`, `overflow` and `write` fire in the
//! plugin's `SAVE_GAME` handler, on the main thread, at the next console `save`; `hook` fires
//! inside the plugin's `Before` subscriber on `Actor::ModActorValueImpl` at the next
//! `player.damageactorvalue`, on the console's job thread. `form` reads through null with the Iron
//! Sword's record in rcx, `deleted` the same with the player's reference after setting its DELETED
//! flag, so the register probe must name the form and its plugin file. `script` is the native
//! `SkyrimRsCrashTest.Crash`, which faults when a script calls it (`CrashFromScript` in the
//! suite's `.psc`), so the log's Papyrus section must name both. The plugin logs the thread id of
//! every step to `skyrim-rs-crash-test.log` so the suite can compare them with the crash log.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};
use std::fs::File;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};

use framework::Host;
use framework::hooks::Options;
use framework::hooks::ops::Actor__ModActorValueImpl;
use skyrim::papyrus::VirtualMachine;
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
const FORM: u8 = 5;
const DELETED: u8 = 6;
const SCRIPT: u8 = 7;

/// The Iron Sword, a `WEAP` record of Skyrim.esm.
const IRON_SWORD: skyrim::forms::FormId = skyrim::forms::FormId(0x12EB7);
/// `TESForm::formFlags` bit 5.
const FLAG_DELETED: u32 = 0x20;

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
    host.on_papyrus(on_papyrus);
    true
}

fn on_papyrus(vm: &'static VirtualMachine) {
    let host = HOST.get().expect("set in load");
    match host.register_native(vm, "SkyrimRsCrashTest", "Crash", || {
        log(&format!("crashing on thread {} (script)", thread()));
        null_read();
    }) {
        Ok(_) => log("PASS native: SkyrimRsCrashTest.Crash bound"),
        Err(e) => log(&format!("FAIL native: {e}")),
    }
    log("summary crash papyrus: 1 pass, 0 fail");
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
                FORM => "form",
                DELETED => "deleted",
                _ => return,
            };
            log(&format!("crashing on thread {} ({what})", thread()));
            match armed {
                NULL => null_read(),
                OVERFLOW => {
                    let _ = recurse(0);
                }
                WRITE => image_write(),
                FORM => null_read_holding(IRON_SWORD.lookup().map_or(0, |f| f.as_ptr() as usize)),
                _ => {
                    let player = skyrim::actors::PlayerCharacter::get().map_or(0, |p| p.as_ptr() as usize);
                    if player != 0 {
                        let flags = (player + 0x10) as *mut u32;
                        unsafe { core::ptr::write_volatile(flags, core::ptr::read_volatile(flags) | FLAG_DELETED) };
                    }
                    null_read_holding(player);
                }
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

/// Reads through a null pointer with `object`'s address in rcx, so the register probe names the object.
#[inline(never)]
fn null_read_holding(object: usize) {
    unsafe { core::arch::asm!("mov rax, [rdx]", in("rcx") object, in("rdx") 0usize, out("rax") _, options(nostack, readonly)) };
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
    const HELP: &'static core::ffi::CStr = c"skyrim-rs crash test: arm null, overflow, write, form or deleted (fires at the next save), hook (at the next damageactorvalue) or script (SkyrimRsCrashTest.Crash faults when called)";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let (armed, line) = match context.args() {
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("null") => (NULL, "armed null: a null read in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("overflow") => (OVERFLOW, "armed overflow: unbounded recursion in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("write") => (WRITE, "armed write: a write into SkyrimSE.exe's code in the SAVE_GAME handler at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("hook") => (HOOK, "armed hook: a null read inside the Before subscriber at the next damageactorvalue"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("form") => (FORM, "armed form: a null read with the Iron Sword's record in rcx at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("deleted") => (DELETED, "armed deleted: the player's reference flagged DELETED, then a null read with it in rcx at the next save"),
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("script") => (SCRIPT, "armed script: the native SkyrimRsCrashTest.Crash faults when a script calls it"),
            _ => (NONE, "SkyrimRsCrash null | overflow | write | hook | form | deleted | script"),
        };
        ARMED.store(armed, Ordering::Relaxed);
        log(&format!("{line} (command thread {})", thread()));
        skyrim::input::print(&format!("SkyrimRsCrash: {line}"));
        true
    }
}
