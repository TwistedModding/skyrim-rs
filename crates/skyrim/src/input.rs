//! Input and console (subsystem pass 9): console output and console commands of a mod's own, and
//! what the generated input classes leave `unsafe`: the typed events of the frame's list as checked
//! casts, the key and button names an engine call writes into an out-parameter, the keyboard and
//! the control map's context stack.
//!
//! The input source's sinks run in registration order: the console hotkeys, `MenuControls`,
//! `PlayerControls`, then the sinks of plugins, so a plugin sink sees every event after the game
//! has acted on it. Keys injected into the event queue (`DevBench`'s `input` tool) never reach the
//! keyboard's state arrays: `keyState` answers for physical keys only.

use core::ffi::{CStr, c_char, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};

use sre::raw::fns;
use sre::{BSWin32KeyboardDevice, ButtonEvent, CharEvent, EventSourceHolder_Input, EventSourceHolder_User, InputContextEnum, InputDevice, InputEvent, InputEventType, MouseMoveEvent, ScriptParamType, TESForm, TESObjectREFR, ThumbstickEvent};

use crate::BSFixedString;

type GetButtonNameFn = unsafe extern "C" fn(this: *mut EventSourceHolder_User, user_event: *const sre::BSFixedString, device: u32, out: *mut sre::BSFixedString) -> bool;
type PrintFn = unsafe extern "C" fn(log: *mut c_void, format: *const c_char, ...);
type UnpackFn = unsafe extern "C" fn(params: *const sre::ParamInfo, data: *const c_void, offset: *mut u32, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, ...) -> bool;

/// A console command's execute function as `ScriptRunner::ExecuteStatement` calls it; `false` stops the script.
pub type ExecuteFn = unsafe extern "C" fn(params: *const sre::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool;

/// Console commands, opcodes 0x100 to 0x2c0 and the sentinel.
const FIRST_CONSOLE_COMMAND: u32 = 0x100;
const FIRST_KEYWORD: u32 = 0x10;
const FIRST_SCRIPT_FUNCTION: u32 = 0x1000;
/// `UnpackCommandArguments` writes at most this many out pointers here; a parsed string is at most 0x200 bytes.
const MAX_ARGS: usize = 8;
const ARG_SIZE: usize = 0x208;

pub trait InputEventExt {
    /// The event as a button event when its type is `Button`.
    fn as_button(&self) -> Option<&ButtonEvent>;
    fn as_char(&self) -> Option<&CharEvent>;
    fn as_mouse_move(&self) -> Option<&MouseMoveEvent>;
    fn as_thumbstick(&self) -> Option<&ThumbstickEvent>;
}

fn cast<T>(event: &InputEvent, kind: InputEventType) -> Option<&T> {
    (event.eventType == kind as u32).then(|| unsafe { &*core::ptr::from_ref(event).cast::<T>() })
}

impl InputEventExt for InputEvent {
    fn as_button(&self) -> Option<&ButtonEvent> {
        cast(self, InputEventType::Button)
    }

    fn as_char(&self) -> Option<&CharEvent> {
        cast(self, InputEventType::Char)
    }

    fn as_mouse_move(&self) -> Option<&MouseMoveEvent> {
        cast(self, InputEventType::MouseMove)
    }

    fn as_thumbstick(&self) -> Option<&ThumbstickEvent> {
        cast(self, InputEventType::Thumbstick)
    }
}

pub trait ButtonEventExt {
    /// The user event the control map gave the event this frame, "" when no enabled mapping took it.
    fn user_event(&self) -> &BSFixedString;
    /// Makes the game's handlers pass the event over: its user event becomes "", what
    /// `MapInputEvents` leaves on an event no enabled mapping takes. Done from a sink at the front
    /// of the input source, before `MenuControls` and `PlayerControls` read it; sinks that read the
    /// device and key still see the event.
    fn skip(&self);
}

impl ButtonEventExt for ButtonEvent {
    fn user_event(&self) -> &BSFixedString {
        BSFixedString::view(&self.userEvent)
    }

    fn skip(&self) {
        let mut blank = BSFixedString::new("");
        unsafe { core::ptr::swap(core::ptr::from_ref(&self.userEvent).cast_mut().cast::<BSFixedString>(), &raw mut blank) };
    }
}

pub trait InputManagerExt {
    /// The `DirectInput` keyboard (its `keyState` byte has bit 0x80 while a physical key is down).
    fn keyboard(&self) -> Option<&BSWin32KeyboardDevice>;
    /// The device's name for key `id` ("E", "Tab", ...), `None` for an id it does not define.
    fn key_name(&self, device: InputDevice, id: u32) -> Option<String>;
    /// The Windows virtual-key code of key `id`.
    fn key_code(&self, device: InputDevice, id: u32) -> Option<u32>;
}

impl InputManagerExt for EventSourceHolder_Input {
    fn keyboard(&self) -> Option<&BSWin32KeyboardDevice> {
        unsafe { self.keyboard.as_ref() }
    }

    fn key_name(&self, device: InputDevice, id: u32) -> Option<String> {
        let mut out = BSFixedString::empty();
        let found = unsafe { self.get_key_name(device as i32, id, (&raw mut out).cast()) };
        found.then(|| out.to_string())
    }

    fn key_code(&self, device: InputDevice, id: u32) -> Option<u32> {
        let mut out = 0;
        unsafe { self.get_key_code(device as i32, id, &raw mut out) }.then_some(out)
    }
}

pub trait ControlMapExt {
    /// The input contexts from the bottom (0, gameplay) to the top; a key maps through the topmost
    /// context that defines it.
    fn context_stack(&self) -> &[u32];
    /// The name of the key `device` maps to `user_event` in the first context that maps it.
    fn button_name(&self, user_event: &str, device: InputDevice) -> Option<String>;
    /// Every line of the parsed control map that maps key `id` of `device` (keyboard, mouse or
    /// gamepad): the context, the user event and the modifier key the line pairs with it (0 for none).
    fn user_events(&self, device: InputDevice, id: u32) -> Vec<(u32, String, u32)>;
    /// The key of `device` the gameplay context maps to `user_event` (`"Toggle POV"`), with the
    /// player's remaps once the control map has loaded them.
    fn mapped_key(&self, user_event: &str, device: InputDevice) -> Option<u32>;
    /// Removes every occurrence of `context` from the stack (`ControlMap::PopInputContext`); true
    /// when one was there. Main thread.
    fn pop_context(&self, context: InputContextEnum) -> bool;
}

impl ControlMapExt for EventSourceHolder_User {
    fn context_stack(&self) -> &[u32] {
        let stack = self.contextStack();
        if stack.data.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(stack.data.cast::<u32>(), stack.size as usize) }
    }

    fn button_name(&self, user_event: &str, device: InputDevice) -> Option<String> {
        let name = BSFixedString::new(user_event);
        let mut out = BSFixedString::empty();
        let get: GetButtonNameFn = unsafe { crate::engine_fn(sre::id!(fns::ControlMap__GetButtonNameForUserEvent)) };
        let found = unsafe { get(core::ptr::from_ref(self).cast_mut(), name.raw(), device.to_engine()?, (&raw mut out).cast()) };
        found.then(|| out.to_string())
    }

    fn user_events(&self, device: InputDevice, id: u32) -> Vec<(u32, String, u32)> {
        mappings(self, device).filter(|(_, m)| m.inputKey & 0xFFFF == id).map(|(context, m)| (context, BSFixedString::view(&m.userEvent).to_string(), m.inputKey >> 16)).collect()
    }

    fn mapped_key(&self, user_event: &str, device: InputDevice) -> Option<u32> {
        mappings(self, device).find(|(context, m)| *context == 0 && *BSFixedString::view(&m.userEvent) == *user_event).map(|(_, m)| m.inputKey & 0xFFFF)
    }

    fn pop_context(&self, context: InputContextEnum) -> bool {
        unsafe { sre::control_map__pop_input_context(core::ptr::from_ref(self).cast_mut().cast(), context) }
    }
}

/// Every line of the parsed control map for `device`, with its context; linked lines left out.
fn mappings(map: &EventSourceHolder_User, device: InputDevice) -> impl Iterator<Item = (u32, &sre::UserEventMapping)> {
    const LINKED: u32 = 0x7FFF_FFFF;
    let count = InputContextEnum::count.to_engine().map_or(0, |count| count as usize);
    (0u32..).zip(map.contexts.as_chunks::<8>().0.iter().take(count)).flat_map(move |(index, pointer)| {
        let context = unsafe { (usize::from_le_bytes(*pointer) as *const sre::InputContext).as_ref() };
        let lines = context.and_then(|c| match device {
            InputDevice::Keyboard => Some(&c.keyboardMappings),
            InputDevice::Mouse => Some(&c.mouseMappings),
            InputDevice::Gamepad => Some(&c.gamepadMappings),
            _ => None,
        });
        sre::fixed_size!("UserEventMapping", sre::UserEventMapping);
    let lines = lines.filter(|l| !l.data.is_null()).map_or(&[][..], |l| unsafe { core::slice::from_raw_parts(l.data.cast::<sre::UserEventMapping>(), l.size as usize) });
        lines.iter().filter(|m| m.inputKey != LINKED).map(move |m| (index, m))
    })
}

/// The player's input handlers in the order they are offered every event: `Movement`, `Look`,
/// `ReadyWeapon`, ..., then the camera state's.
#[must_use]
pub fn player_input_handlers() -> &'static [*mut c_void] {
    let Some(controls) = sre::PlayerControls::singleton() else { return &[] };
    let handlers = &controls.handlers;
    if handlers.data.is_null() {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(handlers.data.cast::<*mut c_void>(), handlers.size as usize) }
}

/// Prints one line to the console (`ConsoleLog::Print`); call it from the game's threads, as the
/// engine's commands do.
pub fn print(text: &str) {
    let log = unsafe { *sre::g_console_log() };
    if log.is_null() {
        return;
    }
    let text = std::ffi::CString::new(text.replace('\0', "")).expect("no interior nul left");
    let print: PrintFn = unsafe { crate::engine_fn(sre::id!(fns::ConsoleLog__Print)) };
    unsafe { print(log, c"%s".as_ptr(), text.as_ptr()) }
}

/// One parameter of a console command, laid out as the engine's `ParamInfo`, because a `static` of
/// them cannot go through the profile; `check_param_layout` holds the layout to the running game.
#[repr(C)]
pub struct Param {
    name: *mut c_void,
    type_code: u32,
    optional: u8,
}

unsafe impl Sync for Param {}

impl Param {
    #[must_use]
    pub const fn new(name: &'static CStr, kind: ScriptParamType, optional: bool) -> Self {
        Self { name: name.as_ptr().cast_mut().cast(), type_code: kind as u32, optional: optional as u8 }
    }
}

/// A parsed argument; `Missing` for an optional argument the line left out.
#[derive(Clone, Copy)]
pub enum Arg<'a> {
    Missing,
    Int(i32),
    Float(f32),
    Str(&'a str),
    Form(&'a TESForm),
    /// An axis letter or another one-byte argument.
    Char(u8),
}

impl core::fmt::Debug for Arg<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing => f.write_str("Missing"),
            Self::Int(i) => write!(f, "Int({i})"),
            Self::Float(x) => write!(f, "Float({x})"),
            Self::Str(s) => write!(f, "Str({s:?})"),
            Self::Form(form) => write!(f, "Form({:08X})", form.formID),
            Self::Char(c) => write!(f, "Char({c})"),
        }
    }
}

/// What an execute function gets: the reference the line ran on and the parsed arguments.
pub struct CommandContext<'a> {
    this_ref: *mut c_void,
    args: &'a [Arg<'a>],
    result: *mut f64,
}

impl<'a> CommandContext<'a> {
    /// A context from the parts the engine passed, rebuilt on the far side of the framework's plugin ABI.
    #[doc(hidden)]
    #[must_use]
    pub fn new(this_ref: *mut c_void, args: &'a [Arg<'a>], result: *mut f64) -> Self {
        Self { this_ref, args, result }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn result_ptr(&self) -> *mut f64 {
        self.result
    }

    /// The explicit `x.` reference or the console's selection, `None` when there is neither.
    #[must_use]
    pub fn this_ref(&self) -> Option<&TESObjectREFR> {
        unsafe { self.this_ref.cast::<TESObjectREFR>().as_ref() }
    }

    #[must_use]
    pub fn args(&self) -> &[Arg<'_>] {
        self.args
    }

    /// The statement's numeric result.
    pub fn set_result(&mut self, value: f64) {
        if let Some(result) = unsafe { self.result.as_mut() } {
            *result = value;
        }
    }
}

/// A console command of this mod's own, taken over from an unused entry of the console table.
pub trait Command: 'static {
    const NAME: &'static CStr;
    const HELP: &'static CStr;
    const PARAMS: &'static [Param];
    /// The command runs on a reference; the engine skips it silently when there is none.
    const NEEDS_REFERENCE: bool = false;

    /// Runs on the thread that drains the console queue; `false` stops the rest of the line.
    fn execute(context: &mut CommandContext<'_>) -> bool;
}

#[derive(Debug)]
pub enum CommandError {
    TooManyParams,
    /// A keyword, script function or console command already answers to the name.
    NameTaken,
    /// Every unused console entry is taken.
    NoFreeEntry,
    /// No console command answers to the name.
    NotFound,
}

impl core::fmt::Display for CommandError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooManyParams => write!(f, "more than {MAX_ARGS} parameters"),
            Self::NameTaken => write!(f, "the name is already a command"),
            Self::NoFreeEntry => write!(f, "no unused console entry is left"),
            Self::NotFound => write!(f, "no console command answers to the name"),
        }
    }
}

impl std::error::Error for CommandError {}

fn text_c<'a>(p: *mut c_void) -> &'a CStr {
    if p.is_null() { c"" } else { unsafe { CStr::from_ptr(p.cast()) } }
}

fn text<'a>(p: *mut c_void) -> &'a str {
    text_c(p).to_str().unwrap_or("")
}

/// A command table, whose length differs per runtime: it ends where the opcodes stop counting up,
/// the entries' only mark, which the next table or global after it does not continue.
fn table(base: *mut c_void, first_opcode: u32) -> &'static mut [sre::CommandTableEntry] {
    let entries = base.cast::<sre::CommandTableEntry>();
    let mut len = 0;
    while unsafe { (*entries.add(len)).opcode } as usize == first_opcode as usize + len {
        len += 1;
    }
    unsafe { core::slice::from_raw_parts_mut(entries, len) }
}

fn answers_to(entry: &sre::CommandTableEntry, name: &CStr) -> bool {
    let name = name.to_str().unwrap_or("");
    let short = text(entry.shortName);
    text(entry.name).eq_ignore_ascii_case(name) || (!short.is_empty() && short.eq_ignore_ascii_case(name))
}

/// An execute of three bytes (`mov al, 1; ret` or `xor al, al; ret`): an entry the binary never uses.
fn is_stub(entry: &sre::CommandTableEntry) -> bool {
    let execute = entry.execute.cast::<[u8; 3]>();
    !execute.is_null() && matches!(unsafe { *execute }, [0xB0, 0x01, 0xC3] | [0x32, 0xC0, 0xC3])
}

/// The console commands, opcodes from 0x100.
#[must_use]
pub fn console_commands() -> &'static [sre::CommandTableEntry] {
    table(sre::g_console_command_table(), FIRST_CONSOLE_COMMAND)
}

/// The console entry a command took over.
#[derive(Clone, Copy, Debug)]
pub struct CommandSlot {
    pub opcode: u32,
    /// The name of the unused command it replaced.
    pub replaced: &'static str,
}

/// Takes over the first unused console entry for `C`. Register at `DataLoaded`, before any
/// console line is compiled; the entry keeps its opcode.
pub fn register<C: Command>() -> Result<CommandSlot, CommandError> {
    take_entry(C::NAME, C::HELP, C::PARAMS, C::NEEDS_REFERENCE, execute::<C>)
}

/// What an [`ErasedCommand`] runs.
pub type ErasedExecute = Box<dyn Fn(&mut CommandContext<'_>) -> bool + Send + Sync>;

/// A console command whose Rust side is chosen at run time, for a host that registers on behalf
/// of code it may unload: the table entry points at this crate's copies of the name, the help and
/// the parameters, and [`set`](Self::set) decides what runs behind it.
#[derive(Clone, Copy, Debug)]
pub struct ErasedCommand {
    index: usize,
    slot: CommandSlot,
}

impl ErasedCommand {
    /// Replaces the execute function; returns once no call is inside the previous one.
    pub fn set(&self, execute: ErasedExecute) {
        *ERASED[self.index].execute.write().unwrap_or_else(PoisonError::into_inner) = Some(execute);
    }

    #[must_use]
    pub fn slot(&self) -> CommandSlot {
        self.slot
    }
}

/// How many erased commands one copy of this crate can hold.
pub const ERASED_COMMANDS: usize = 16;

struct ErasedSlot {
    params: OnceLock<&'static [Param]>,
    execute: RwLock<Option<ErasedExecute>>,
}

static ERASED: [ErasedSlot; ERASED_COMMANDS] = [const { ErasedSlot { params: OnceLock::new(), execute: RwLock::new(None) } }; ERASED_COMMANDS];
static REGISTER: Mutex<()> = Mutex::new(());

/// Takes over the first unused console entry with copies of `name`, `help` and `params` (which
/// may point anywhere, even into a DLL that is later unloaded) and `execute` behind an
/// [`ErasedCommand`]; at most [`ERASED_COMMANDS`] per process.
pub fn register_erased(name: &CStr, help: &CStr, params: &[Param], needs_reference: bool, execute: ErasedExecute) -> Result<ErasedCommand, CommandError> {
    let _serialized = REGISTER.lock().unwrap_or_else(PoisonError::into_inner);
    let index = ERASED.iter().position(|s| s.params.get().is_none()).ok_or(CommandError::NoFreeEntry)?;
    let leak = |s: &CStr| -> &'static CStr { Box::leak(s.to_owned().into_boxed_c_str()) };
    let params: &'static [Param] = Box::leak(params.iter().map(|p| Param { name: leak(text_c(p.name)).as_ptr().cast_mut().cast(), type_code: p.type_code, optional: p.optional }).collect());
    let slot = take_entry(leak(name), leak(help), params, needs_reference, EXECUTES[index])?;
    *ERASED[index].execute.write().unwrap_or_else(PoisonError::into_inner) = Some(execute);
    let _ = ERASED[index].params.set(params);
    Ok(ErasedCommand { index, slot })
}

/// An engine console command whose execute function [`replace_erased`] replaced.
#[derive(Clone, Copy, Debug)]
pub struct ReplacedCommand {
    index: usize,
    execute: &'static AtomicUsize,
    original: usize,
    slot: CommandSlot,
}

impl ReplacedCommand {
    /// Runs `execute` in place of the command's own from now on; returns once no call is inside
    /// the previous one.
    pub fn set(&self, execute: ErasedExecute) {
        *ERASED[self.index].execute.write().unwrap_or_else(PoisonError::into_inner) = Some(execute);
        self.execute.store(EXECUTES[self.index] as usize, Ordering::Release);
    }

    /// Puts the command's own execute function back and drops the replacement once no call is
    /// inside it.
    pub fn restore(&self) {
        self.execute.store(self.original, Ordering::Release);
        *ERASED[self.index].execute.write().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Whether the entry runs the replacement now.
    #[must_use]
    pub fn active(&self) -> bool {
        self.execute.load(Ordering::Acquire) == EXECUTES[self.index] as usize
    }

    /// The command's opcode and its own name.
    #[must_use]
    pub fn slot(&self) -> CommandSlot {
        self.slot
    }
}

/// Replaces the execute function of the console command `name` (long or short name) with
/// `execute` behind an [`ErasedCommand`]-style slot, its arguments unpacked by the command's own
/// parameters. The runner reads the pointer at every statement and nothing caches it, so the one
/// aligned store is safe at any time (RE `camera.md` IC9 B); a call already running finishes with
/// the old function. Shares the [`ERASED_COMMANDS`] slots.
pub fn replace_erased(name: &CStr, execute: ErasedExecute) -> Result<ReplacedCommand, CommandError> {
    let _serialized = REGISTER.lock().unwrap_or_else(PoisonError::into_inner);
    let entry = table(sre::g_console_command_table(), FIRST_CONSOLE_COMMAND).iter_mut().find(|e| answers_to(e, name) && !is_stub(e)).ok_or(CommandError::NotFound)?;
    sre::fixed_size!("ParamInfo", sre::ParamInfo);
    replace_entry(entry, execute)
}

fn replace_entry(entry: &'static mut sre::CommandTableEntry, execute: ErasedExecute) -> Result<ReplacedCommand, CommandError> {
    if usize::from(entry.numParams) > MAX_ARGS {
        return Err(CommandError::TooManyParams);
    }
    let index = ERASED.iter().position(|s| s.params.get().is_none()).ok_or(CommandError::NoFreeEntry)?;
    let params: &'static [Param] = if entry.params.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(entry.params.cast::<Param>(), usize::from(entry.numParams)) } };
    let _ = ERASED[index].params.set(params);
    let execute_field = unsafe { AtomicUsize::from_ptr((&raw mut entry.execute).cast()) };
    let replaced = ReplacedCommand { index, execute: execute_field, original: execute_field.load(Ordering::Acquire), slot: CommandSlot { opcode: entry.opcode, replaced: text(entry.name) } };
    replaced.set(execute);
    Ok(replaced)
}

fn check_param_layout() {
    sre::fixed_size!("ParamInfo", Param);
    let same = sre::member_offset!("ParamInfo.name") == core::mem::offset_of!(Param, name) && sre::member_offset!("ParamInfo.typeCode") == core::mem::offset_of!(Param, type_code) && sre::member_offset!("ParamInfo.optional") == core::mem::offset_of!(Param, optional);
    assert!(same, "ParamInfo moved");
}

fn take_entry(name: &'static CStr, help: &'static CStr, params: &'static [Param], needs_reference: bool, execute: ExecuteFn) -> Result<CommandSlot, CommandError> {
    check_param_layout();
    if params.len() > MAX_ARGS {
        return Err(CommandError::TooManyParams);
    }
    let tables = [table(sre::g_script_keyword_table(), FIRST_KEYWORD), table(sre::g_script_command_table(), FIRST_SCRIPT_FUNCTION), table(sre::g_console_command_table(), FIRST_CONSOLE_COMMAND)];
    if tables.iter().any(|t| t.iter().any(|e| answers_to(e, name))) {
        return Err(CommandError::NameTaken);
    }
    let entry = table(sre::g_console_command_table(), FIRST_CONSOLE_COMMAND).iter_mut().find(|e| is_stub(e)).ok_or(CommandError::NoFreeEntry)?;
    let replaced = text(entry.name);
    entry.name = name.as_ptr().cast_mut().cast();
    entry.shortName = c"".as_ptr().cast_mut().cast();
    entry.helpText = help.as_ptr().cast_mut().cast();
    entry.referenceFunction = u8::from(needs_reference);
    entry.numParams = u16::try_from(params.len()).expect("at most MAX_ARGS");
    entry.params = if params.is_empty() { core::ptr::null_mut() } else { params.as_ptr().cast_mut().cast() };
    entry.parse = crate::reloc().address(sre::id!(fns::ParseCommandParams)).expect("ParseCommandParams id") as *mut c_void;
    entry.eval = core::ptr::null_mut();
    entry.flags = 0;
    entry.execute = execute as *mut c_void;
    Ok(CommandSlot { opcode: entry.opcode, replaced })
}

#[repr(C, align(8))]
struct ArgSlot([u8; ARG_SIZE]);

unsafe extern "C" fn execute<C: Command>(params: *const sre::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool {
    unsafe { run(C::PARAMS, params, data, this_ref, containing, script, locals, result, offset, C::execute) }
}

unsafe extern "C" fn execute_erased<const I: usize>(params: *const sre::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool {
    let (Some(declared), guard) = (ERASED[I].params.get(), ERASED[I].execute.read().unwrap_or_else(PoisonError::into_inner)) else {
        return true;
    };
    match guard.as_ref() {
        Some(f) => unsafe { run(declared, params, data, this_ref, containing, script, locals, result, offset, |ctx| f(ctx)) },
        None => true,
    }
}

macro_rules! executes {
    ($($i:literal)*) => {
        const EXECUTES: [ExecuteFn; ERASED_COMMANDS] = [$(execute_erased::<$i>),*];
    };
}

executes!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15);

/// Unpacks the line's arguments as `declared` describes them and runs `execute` with them.
#[allow(clippy::too_many_arguments)]
unsafe fn run(declared: &[Param], params: *const sre::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32, execute: impl FnOnce(&mut CommandContext<'_>) -> bool) -> bool {
    let given = if data.is_null() || offset.is_null() { 0 } else { usize::from(unsafe { data.cast::<u8>().add(*offset as usize).cast::<u16>().read_unaligned() }) };
    let mut slots: [ArgSlot; MAX_ARGS] = core::array::from_fn(|_| ArgSlot([0; ARG_SIZE]));
    let out: [*mut ArgSlot; MAX_ARGS] = core::array::from_fn(|i| &raw mut slots[i]);
    // A command without parameters has no parameter table for the engine to unpack from.
    if !declared.is_empty() {
        let unpack: UnpackFn = unsafe { crate::engine_fn(sre::id!(fns::UnpackCommandArguments)) };
        if !unsafe { unpack(params, data, offset, this_ref, containing, script, locals, out[0], out[1], out[2], out[3], out[4], out[5], out[6], out[7]) } {
            return false;
        }
    }
    let args: Vec<Arg<'_>> = declared.iter().zip(&slots).enumerate().map(|(i, (param, slot))| if i < given { decode(param.type_code, slot) } else { Arg::Missing }).collect();
    execute(&mut CommandContext { this_ref, args: &args, result })
}

fn decode(kind: u32, slot: &ArgSlot) -> Arg<'_> {
    let bytes = &slot.0;
    sre::fixed_size!("ScriptParamTypeInfo", sre::ScriptParamTypeInfo);
    let info = unsafe { &*sre::g_script_param_type_info().cast::<sre::ScriptParamTypeInfo>().add(kind as usize) };
    if info.isForm != 0 {
        let form = usize::from_le_bytes(bytes[..8].try_into().expect("8 bytes")) as *const TESForm;
        return unsafe { form.as_ref() }.map_or(Arg::Missing, Arg::Form);
    }
    match kind {
        k if k == ScriptParamType::String as u32 => Arg::Str(CStr::from_bytes_until_nul(bytes).ok().and_then(|s| s.to_str().ok()).unwrap_or("")),
        k if k == ScriptParamType::Float as u32 => Arg::Float(f32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))),
        k if k == ScriptParamType::Axis as u32 => Arg::Char(bytes[0]),
        _ => Arg::Int(i32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn own(_: *const sre::ParamInfo, _: *const c_void, _: *mut c_void, _: *mut c_void, _: *mut c_void, _: *mut c_void, _: *mut f64, _: *mut u32) -> bool {
        true
    }

    #[test]
    fn a_replaced_command_runs_the_replacement_until_restored() {
        versionlib::profile::assume_canonical();
        let entry: &'static mut sre::CommandTableEntry = Box::leak(Box::new(unsafe { core::mem::zeroed() }));
        entry.name = c"SetCameraFOV".as_ptr().cast_mut().cast();
        entry.opcode = 0x13e;
        entry.execute = own as *mut c_void;
        let replaced = replace_entry(entry, Box::new(|_| true)).unwrap();
        let entry = unsafe { &*core::ptr::from_ref(replaced.execute).cast::<*mut c_void>() };
        assert!(replaced.active());
        assert_eq!(*entry as usize, EXECUTES[replaced.index] as usize);
        assert_eq!((replaced.slot().opcode, replaced.slot().replaced), (0x13e, "SetCameraFOV"));
        replaced.restore();
        assert!(!replaced.active());
        assert_eq!(*entry as usize, own as *const () as usize);
        assert!(ERASED[replaced.index].execute.read().unwrap().is_none(), "the replacement is dropped");
        replaced.set(Box::new(|_| false));
        assert!(replaced.active(), "a later generation swaps it in again");
    }
}
