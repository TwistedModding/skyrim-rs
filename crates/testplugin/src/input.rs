//! One test per `skyrim::input` API, against the RE repo's `docs/subsystems/input-verify.md`.
//! `install` adds a sink on the input source and takes over a console entry for `SkyrimRsTest` at
//! `DataLoaded`; the sink records every keyboard `ButtonEvent` per key and the context stacks it
//! sees; `TESTS` read the devices, the sink order, the control map, the player controls and the
//! command entry after a load; `SAVE_TESTS` check the keystrokes `DevBench` injected and the console
//! command, which opens and closes the ui suite's menu.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use layout::v1_7_104::globals;
use skyrim::events::{BSEventNotifyControl, BSTEventSource, Event, EventSink, Sink};
use skyrim::input::{Arg, Command, CommandContext, ControlMap, InputDevice, InputEventList, InputManager, Param, PlayerControls, ScriptParamType};

use crate::{Test, check};

const KEY_E: u32 = 0x12;
const KEY_W: u32 = 0x11;
const KEY_TAB: u32 = 0xF;

pub const TESTS: &[(&str, Test)] = &[("devices", devices), ("sinks", sinks), ("control_map", control_map), ("player_controls", player_controls), ("command_entry", command_entry)];
pub const SAVE_TESTS: &[(&str, Test)] = &[("key_e", key_e), ("key_w", key_w), ("key_tab", key_tab), ("contexts", contexts), ("polling", polling), ("command", command)];

/// The frames of one key: press, hold and release counts, the longest hold, the user event names
/// in the order they appeared, and the hold frames on which the keyboard read the key down.
#[derive(Clone, Debug, Default)]
struct KeyStats {
    presses: u32,
    holds: u32,
    releases: u32,
    max_held: f32,
    user_events: Vec<String>,
    polled_down: u32,
}

static KEYS: Mutex<BTreeMap<u32, KeyStats>> = Mutex::new(BTreeMap::new());
static STACKS: Mutex<Vec<Vec<u32>>> = Mutex::new(Vec::new());
static OPCODE: AtomicU32 = AtomicU32::new(0);
static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Keys;

impl EventSink<InputEventList> for Keys {
    fn process(&self, list: &InputEventList, _: &BSTEventSource<InputEventList>) -> BSEventNotifyControl {
        let keyboard = InputManager::get().and_then(InputManager::keyboard);
        let mut keys = KEYS.lock().unwrap();
        for button in list.iter().filter_map(|e| e.as_button()).filter(|b| b.device_kind() == Some(InputDevice::Keyboard)) {
            let stats = keys.entry(button.id()).or_default();
            if button.is_pressed() {
                stats.presses += 1;
            } else if button.is_released() {
                stats.releases += 1;
            } else {
                stats.holds += 1;
                if keyboard.is_some_and(|k| k.is_down(u8::try_from(button.id()).unwrap_or(0))) {
                    stats.polled_down += 1;
                }
            }
            stats.max_held = stats.max_held.max(button.held_secs());
            let name = button.user_event().to_string();
            if !stats.user_events.contains(&name) {
                stats.user_events.push(name);
            }
            if let Some(map) = ControlMap::get() {
                let stack = map.context_stack().to_vec();
                let mut stacks = STACKS.lock().unwrap();
                if !stacks.contains(&stack) {
                    stacks.push(stack);
                }
            }
        }
        BSEventNotifyControl::Continue
    }
}

static KEY_SINK: Sink<InputEventList, Keys> = Sink::new(Keys);

struct TestCommand;

static PARAMS: [Param; 2] = [Param::new(c"show", ScriptParamType::Integer, false), Param::new(c"target", ScriptParamType::ObjectReference, true)];

impl Command for TestCommand {
    const NAME: &'static core::ffi::CStr = c"SkyrimRsTest";
    const HELP: &'static core::ffi::CStr = c"skyrim-rs test: 1 opens the ui suite's menu, 0 closes it";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let show = match context.args().first() {
            Some(Arg::Int(n)) => *n,
            _ => -1,
        };
        let target = match context.args().get(1) {
            Some(Arg::Form(f)) => format!("{:08X}", f.form_id().0),
            Some(other) => format!("{other:?}"),
            None => "none".to_owned(),
        };
        let line = format!("SkyrimRsTest: show {show} target {target}");
        skyrim::input::print(&line);
        CALLS.lock().unwrap().push(format!("{line} on {:?}", std::thread::current().id()));
        if show >= 0 {
            crate::ui::request(show != 0);
        }
        context.set_result(f64::from(show));
        true
    }
}

/// Adds the key sink and takes over a console entry for `SkyrimRsTest`; once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    InputEventList::source().ok_or("no input source")?.add_sink(&KEY_SINK).leak();
    let slot = skyrim::input::register::<TestCommand>().map_err(|e| format!("register SkyrimRsTest: {e}"))?;
    OPCODE.store(slot.opcode, Ordering::Relaxed);
    Ok(format!("key sink added, SkyrimRsTest took over console opcode {:#x} ({})", slot.opcode, slot.replaced))
}

/// Clears the key records and the command calls; called after a load.
pub fn reset() {
    KEYS.lock().unwrap().clear();
    STACKS.lock().unwrap().clear();
    CALLS.lock().unwrap().clear();
}

fn rva(p: usize) -> usize {
    p.wrapping_sub(skyrim::reloc().base())
}

fn vtable_rva(object: *const core::ffi::c_void) -> usize {
    if object.is_null() { 0 } else { rva(unsafe { *object.cast::<usize>() }) }
}

fn devices() -> Result<String, String> {
    let input = InputManager::get().ok_or("no input manager")?;
    let raw = input.raw();
    let keyboard = vtable_rva(raw.keyboard.cast());
    let mouse = vtable_rva(raw.mouse.cast());
    check(keyboard == 0x1a2_ce50 && mouse == 0x1a2_ceb0, &format!("keyboard vtable {keyboard:#x}, mouse {mouse:#x}"))?;
    check(raw.device3 == 0 && raw.device4 == 0, "device slots 3 and 4 are not null")?;
    check(input.keyboard().is_some(), "keyboard() is None")?;
    let name = input.key_name(InputDevice::Keyboard, KEY_E);
    let code = input.key_code(InputDevice::Keyboard, KEY_E);
    check(name.as_deref() == Some("E") && code == Some(0x45), &format!("scan code 0x12: name {name:?}, virtual key {code:?}"))?;
    Ok(format!("keyboard and mouse at their vtables, slots 3 and 4 null; scan code 0x12 is \"E\", virtual key 0x45; Tab is {:?}", input.key_name(InputDevice::Keyboard, KEY_TAB)))
}

fn sinks() -> Result<String, String> {
    let source = InputEventList::source().ok_or("no input source")?;
    let reloc = skyrim::reloc();
    let expected = [globals::g_ConsoleHotkeyManager, globals::g_MenuControls, globals::g_PlayerControls].map(|g| reloc.address(g).unwrap_or(0));
    let first: Vec<usize> = source.sinks().iter().take(3).map(|s| *s as usize).collect();
    check(first == expected, &format!("first sinks {:x?}, expected {expected:x?}", first.iter().map(|p| rva(*p)).collect::<Vec<_>>()))?;
    check(source.contains(&KEY_SINK), "the key sink is not registered")?;
    Ok(format!("{} sinks: console hotkeys, MenuControls, PlayerControls first, the key sink among the rest", source.sink_count()))
}

fn control_map() -> Result<String, String> {
    let map = ControlMap::get().ok_or("no control map")?;
    let stack = map.context_stack();
    check(stack.is_empty() || stack == [0], &format!("context stack {stack:?}"))?;
    check(map.enabled_controls() == 0xFFFF_FFFF, &format!("enabled controls {:#x}", map.enabled_controls()))?;
    let activate = map.button_name("Activate", InputDevice::Keyboard);
    check(activate.as_deref() == Some("E"), &format!("Activate maps to {activate:?}"))?;
    Ok(format!("context stack {stack:?} at the load (gameplay's 0 is pushed later), all controls enabled, Activate on \"E\", Forward on {:?}", map.button_name("Forward", InputDevice::Keyboard)))
}

fn player_controls() -> Result<String, String> {
    const HANDLERS: [usize; 13] = [0x0193_51c8, 0x0193_5150, 0x0193_5348, 0x0193_53d8, 0x0193_5418, 0x0193_5388, 0x0193_54a8, 0x0193_5528, 0x0193_5208, 0x0193_5258, 0x0193_5458, 0x0193_54e8, 0x0193_5568];
    let controls = PlayerControls::get().ok_or("no PlayerControls")?;
    let vtables: Vec<usize> = controls.handlers().iter().map(|h| vtable_rva(h.cast_const())).collect();
    check(vtables.len() > HANDLERS.len() && vtables[..HANDLERS.len()] == HANDLERS, &format!("handler vtables {vtables:x?}"))?;
    Ok(format!("{} handlers: the 13 of the pass in order, then the camera state's ({:#x})", vtables.len(), vtables[HANDLERS.len()]))
}

fn command_entry() -> Result<String, String> {
    let opcode = OPCODE.load(Ordering::Relaxed);
    let entry = skyrim::input::console_commands().iter().find(|e| e.opcode() == opcode).ok_or(format!("no entry with opcode {opcode:#x}"))?;
    check(entry.name() == "SkyrimRsTest" && entry.short_name().is_empty(), &format!("entry {:#x} is {:?}/{:?}", opcode, entry.name(), entry.short_name()))?;
    check(entry.raw().numParams == 2 && !entry.raw().params.is_null(), "the parameters were not written")?;
    Ok(format!("SkyrimRsTest at console opcode {opcode:#x}, 2 parameters"))
}

fn stats(id: u32) -> Result<KeyStats, String> {
    KEYS.lock().unwrap().get(&id).cloned().ok_or_else(|| format!("no events for scan code {id:#x}"))
}

fn describe(s: &KeyStats) -> String {
    format!("press {} hold {} release {}, held up to {:.2} s, user events {:?}", s.presses, s.holds, s.releases, s.max_held, s.user_events)
}

fn key_e() -> Result<String, String> {
    let s = stats(KEY_E)?;
    check(s.presses == 1 && s.releases == 1 && s.user_events == ["Activate"], &describe(&s))?;
    Ok(describe(&s))
}

fn key_w() -> Result<String, String> {
    let s = stats(KEY_W)?;
    check(s.presses == 1 && s.releases == 1 && s.holds > 30 && s.max_held > 1.5 && s.user_events == ["Forward"], &describe(&s))?;
    Ok(describe(&s))
}

fn key_tab() -> Result<String, String> {
    let s = stats(KEY_TAB)?;
    check(s.presses == 2 && s.releases == 2, &describe(&s))?;
    check(s.user_events.first().map(String::as_str) == Some("Tween Menu") && s.user_events.iter().any(|u| u == "Cancel"), &describe(&s))?;
    Ok(describe(&s))
}

fn contexts() -> Result<String, String> {
    let stacks = STACKS.lock().unwrap().clone();
    check(stacks.contains(&vec![0]) && stacks.contains(&vec![0, 1, 9]), &format!("context stacks seen {stacks:?}"))?;
    Ok(format!("context stacks seen at key events: {stacks:?}"))
}

fn polling() -> Result<String, String> {
    let keys = KEYS.lock().unwrap().clone();
    let held: u32 = keys.values().map(|s| s.holds).sum();
    let down: u32 = keys.values().map(|s| s.polled_down).sum();
    check(held > 0 && down == 0, &format!("the keyboard read {down} of {held} injected hold frames down"))?;
    Ok(format!("the keyboard read none of the {held} injected hold frames down: injected keys never reach the device state"))
}

fn command() -> Result<String, String> {
    let calls = CALLS.lock().unwrap().clone();
    check(calls.len() == 2, &format!("SkyrimRsTest ran {} times: {calls:?}", calls.len()))?;
    check(calls[0].starts_with("SkyrimRsTest: show 1 target 00000014") && calls[1].starts_with("SkyrimRsTest: show 0 target Missing"), &format!("{calls:?}"))?;
    let (opened, closed) = crate::ui::own_menu_events();
    check(opened == 2 && closed == 2, &format!("the ui suite's menu opened {opened} and closed {closed} times since the load (its own cycle plus the command's)"))?;
    Ok(format!("{calls:?}; the ui suite's menu opened and closed twice since the load"))
}
