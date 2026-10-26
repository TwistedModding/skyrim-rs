//! Input and console (subsystem pass 9): the typed events of the frame's input list, the
//! keyboard's key state and the devices' key names, the control map's context stack and enabled
//! controls, the player controls' handlers, console output and console commands of a mod's own.
//!
//! The input source's sinks run in registration order: the console hotkeys, `MenuControls`,
//! `PlayerControls`, then the sinks of plugins, so a plugin sink sees every event after the game
//! has acted on it. Keys injected into the event queue (`DevBench`'s `input` tool) never reach the
//! keyboard's state arrays: [`Keyboard::is_down`] answers for physical keys only.

use core::ffi::{CStr, c_char, c_void};
use core::ops::Deref;
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};

use layout::v1_7_104::{classes, fns, globals};

pub use crate::events::EventSourceHolderInput as InputManager;
pub use crate::events::{InputEvent, InputEventList};
use crate::forms::{TESForm, TESObjectREFR};
use crate::string::BSFixedString;

pub use layout::v1_7_104::enums::{InputContext, InputDevice, InputEventType, ScriptParamType};

type GetButtonNameFn = unsafe extern "C" fn(this: *mut classes::EventSourceHolder_User, user_event: *const classes::BSFixedString, device: u32, out: *mut classes::BSFixedString) -> bool;
type PrintFn = unsafe extern "C" fn(log: *mut c_void, format: *const c_char, ...);
type UnpackFn = unsafe extern "C" fn(params: *const classes::ParamInfo, data: *const c_void, offset: *mut u32, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, ...) -> bool;

/// A console command's execute function as `ScriptRunner::ExecuteStatement` calls it; `false` stops the script.
pub type ExecuteFn = unsafe extern "C" fn(params: *const classes::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool;

/// Console commands, opcodes 0x100 to 0x2c0 and the sentinel.
const CONSOLE_COMMANDS: usize = 450;
const KEYWORDS: usize = 16;
const SCRIPT_FUNCTIONS: usize = 738;
/// `UnpackCommandArguments` writes at most this many out pointers here; a parsed string is at most 0x200 bytes.
const MAX_ARGS: usize = 8;
const ARG_SIZE: usize = 0x208;

pub(crate) struct Fns {
    control_map: *const *mut classes::EventSourceHolder_User,
    player_controls: *const *mut classes::PlayerControls,
    console_log: *const *mut c_void,
    keywords: *mut classes::CommandTableEntry,
    script_functions: *mut classes::CommandTableEntry,
    console_commands: *mut classes::CommandTableEntry,
    param_types: *const classes::ScriptParamTypeInfo,
    parse: *mut c_void,
    get_key_name: fns::EventSourceHolder_Input__GetKeyName_fn,
    get_key_code: fns::EventSourceHolder_Input__GetKeyCode_fn,
    get_button_name: GetButtonNameFn,
    print: PrintFn,
    unpack: UnpackFn,
}

unsafe impl Sync for Fns {}
unsafe impl Send for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                control_map: reloc.address(globals::g_EventSourceHolder_User_ptr)? as *const *mut classes::EventSourceHolder_User,
                player_controls: reloc.address(globals::g_PlayerControls_ptr)? as *const *mut classes::PlayerControls,
                console_log: reloc.address(globals::g_ConsoleLog)? as *const *mut c_void,
                keywords: reloc.address(globals::g_ScriptKeywordTable)? as *mut classes::CommandTableEntry,
                script_functions: reloc.address(globals::g_ScriptCommandTable)? as *mut classes::CommandTableEntry,
                console_commands: reloc.address(globals::g_ConsoleCommandTable)? as *mut classes::CommandTableEntry,
                param_types: reloc.address(globals::g_ScriptParamTypeInfo)? as *const classes::ScriptParamTypeInfo,
                parse: reloc.address(fns::ParseCommandParams)? as *mut c_void,
                get_key_name: crate::resolve(reloc, fns::EventSourceHolder_Input__GetKeyName)?,
                get_key_code: crate::resolve(reloc, fns::EventSourceHolder_Input__GetKeyCode)?,
                get_button_name: crate::resolve(reloc, fns::ControlMap__GetButtonNameForUserEvent)?,
                print: crate::resolve(reloc, fns::ConsoleLog__Print)?,
                unpack: crate::resolve(reloc, fns::UnpackCommandArguments)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().input
}

impl InputEvent {
    #[must_use]
    pub fn device_kind(&self) -> Option<InputDevice> {
        InputDevice::from_u32(self.device())
    }

    #[must_use]
    pub fn kind(&self) -> Option<InputEventType> {
        InputEventType::from_u32(self.event_type())
    }

    fn cast<T>(&self, kind: InputEventType) -> Option<&T> {
        (self.event_type() == kind as u32).then(|| unsafe { &*(core::ptr::from_ref(self).cast::<T>()) })
    }

    #[must_use]
    pub fn as_button(&self) -> Option<&ButtonEvent> {
        self.cast(InputEventType::Button)
    }

    #[must_use]
    pub fn as_char(&self) -> Option<&CharEvent> {
        self.cast(InputEventType::Char)
    }

    #[must_use]
    pub fn as_mouse_move(&self) -> Option<&MouseMoveEvent> {
        self.cast(InputEventType::MouseMove)
    }

    #[must_use]
    pub fn as_thumbstick(&self) -> Option<&ThumbstickEvent> {
        self.cast(InputEventType::Thumbstick)
    }
}

macro_rules! input_event {
    ($(#[$doc:meta] $name:ident) => {
        $(#[$doc]
        #[repr(transparent)]
        pub struct $name(classes::$name);

        impl $name {
            #[must_use]
            pub fn raw(&self) -> &classes::$name {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = InputEvent;

            fn deref(&self) -> &InputEvent {
                unsafe { &*(&raw const self.0).cast::<InputEvent>() }
            }
        }
    };
}

input_event! {
    /// A key, mouse button, wheel step or pad button (0x30): one per frame while it is down, then
    /// one release. The press frame reads value 1 and held 0, every later frame value 1 and the
    /// seconds held so far, the release frame value 0 with the time held.
    ButtonEvent
}

impl ButtonEvent {
    /// Keyboard: the `DirectInput` scan code; mouse: 0-7 buttons, 8 wheel up, 9 wheel down.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.idCode
    }

    #[must_use]
    pub fn value(&self) -> f32 {
        self.0.value
    }

    #[must_use]
    pub fn held_secs(&self) -> f32 {
        self.0.heldDownSecs
    }

    /// The control map's name for this key in the topmost context that maps it ("Activate",
    /// "Forward", ...), empty when no context does.
    #[must_use]
    pub fn user_event(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.userEvent) }
    }

    #[must_use]
    pub fn is_pressed(&self) -> bool {
        self.value() > 0.0 && self.held_secs() == 0.0
    }

    #[must_use]
    pub fn is_down(&self) -> bool {
        self.value() > 0.0
    }

    #[must_use]
    pub fn is_released(&self) -> bool {
        self.value() == 0.0
    }
}

input_event! {
    /// A typed character while text entry is on.
    CharEvent
}

impl CharEvent {
    #[must_use]
    pub fn char_code(&self) -> u32 {
        self.0.charCode
    }
}

input_event! {
    /// The mouse's movement this frame.
    MouseMoveEvent
}

impl MouseMoveEvent {
    #[must_use]
    pub fn delta(&self) -> (i32, i32) {
        (self.0.dx.cast_signed(), self.0.dy.cast_signed())
    }
}

input_event! {
    /// A pad stick (id 0xb left, 0xc right).
    ThumbstickEvent
}

impl ThumbstickEvent {
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.idCode
    }

    #[must_use]
    pub fn position(&self) -> (f32, f32) {
        (self.0.x, self.0.y)
    }
}

/// The `DirectInput` keyboard (`BSWin32KeyboardDevice`, 0x370).
#[repr(transparent)]
pub struct Keyboard(classes::BSWin32KeyboardDevice);

impl Keyboard {
    #[must_use]
    pub fn raw(&self) -> &classes::BSWin32KeyboardDevice {
        &self.0
    }

    /// The physical key with `DirectInput` scan code `scan_code` is down this frame.
    #[must_use]
    pub fn is_down(&self, scan_code: u8) -> bool {
        self.0.keyState[usize::from(scan_code)] & 0x80 != 0
    }

    /// The key was down the frame before.
    #[must_use]
    pub fn was_down(&self, scan_code: u8) -> bool {
        self.0.prevKeyState[usize::from(scan_code)] & 0x80 != 0
    }
}

impl InputManager {
    fn as_ptr(&self) -> *mut classes::EventSourceHolder_Input {
        (&raw const *self.raw()).cast_mut()
    }

    #[must_use]
    pub fn keyboard(&self) -> Option<&Keyboard> {
        unsafe { self.raw().keyboard.cast::<Keyboard>().as_ref() }
    }

    /// The device's name for key `id` ("E", "Tab", ...), `None` for an id it does not define.
    #[must_use]
    pub fn key_name(&self, device: InputDevice, id: u32) -> Option<String> {
        let mut out = BSFixedString::empty();
        let found = unsafe { (fns().get_key_name)(self.as_ptr(), device as i32, id, out.as_mut_ptr()) };
        found.then(|| out.to_string())
    }

    /// The Windows virtual-key code of key `id`.
    #[must_use]
    pub fn key_code(&self, device: InputDevice, id: u32) -> Option<u32> {
        let mut out = 0;
        unsafe { (fns().get_key_code)(self.as_ptr(), device as i32, id, &raw mut out) }.then_some(out)
    }
}

/// The control map (`EventSourceHolder_User`, 0x130): user event names per input context, the
/// context stack the open menus push, and the enabled control groups.
#[repr(transparent)]
pub struct ControlMap(classes::EventSourceHolder_User);

impl ControlMap {
    /// `None` before the control map is constructed.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().control_map).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::EventSourceHolder_User {
        &self.0
    }

    /// The input contexts from the bottom (0, gameplay) to the top; a key maps through the topmost
    /// context that defines it.
    #[must_use]
    pub fn context_stack(&self) -> &[u32] {
        let stack = &self.0.contextStack;
        if stack.data.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(stack.data.cast::<u32>(), stack.size as usize) }
    }

    /// The enabled control group bits (`ControlGroupFlag`); all set in normal play.
    #[must_use]
    pub fn enabled_controls(&self) -> u32 {
        self.0.enabledControls
    }

    /// The name of the key `device` maps to `user_event` in the first context that maps it.
    #[must_use]
    pub fn button_name(&self, user_event: &str, device: InputDevice) -> Option<String> {
        let name = BSFixedString::new(user_event);
        let mut out = BSFixedString::empty();
        let this = (&raw const self.0).cast_mut();
        let found = unsafe { (fns().get_button_name)(this, name.raw(), device as u32, out.as_mut_ptr()) };
        found.then(|| out.to_string())
    }
}

/// The player's input handlers (`PlayerControls`, 0x1e0).
#[repr(transparent)]
pub struct PlayerControls(classes::PlayerControls);

impl PlayerControls {
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().player_controls).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::PlayerControls {
        &self.0
    }

    /// The handlers in the order they are offered every event: `Movement`, `Look`, `ReadyWeapon`, ...,
    /// then the camera state's.
    #[must_use]
    pub fn handlers(&self) -> &[*mut c_void] {
        let handlers = &self.0.handlers;
        if handlers.data.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(handlers.data.cast::<*mut c_void>(), handlers.size as usize) }
    }
}

/// Prints one line to the console (`ConsoleLog::Print`); call it from the game's threads, as the
/// engine's commands do.
pub fn print(text: &str) {
    let log = unsafe { *fns().console_log };
    if log.is_null() {
        return;
    }
    let text = std::ffi::CString::new(text.replace('\0', "")).expect("no interior nul left");
    unsafe { (fns().print)(log, c"%s".as_ptr(), text.as_ptr()) }
}

/// One parameter of a console command.
#[repr(transparent)]
pub struct Param(classes::ParamInfo);

unsafe impl Sync for Param {}

impl Param {
    #[must_use]
    pub const fn new(name: &'static CStr, kind: ScriptParamType, optional: bool) -> Self {
        Self(classes::ParamInfo { name: name.as_ptr().cast_mut().cast(), typeCode: kind as u32, optional: optional as u8, unknown_D: [0; 3] })
    }
}

/// A parsed argument; `Missing` for an optional argument the line left out.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Missing,
    Int(i32),
    Float(f32),
    Str(&'a str),
    Form(&'a TESForm),
    /// An axis letter or another one-byte argument.
    Char(u8),
}

/// What an execute function gets: the reference the line ran on and the parsed arguments.
pub struct CommandContext<'a> {
    this_ref: *mut c_void,
    args: &'a [Arg<'a>],
    result: *mut f64,
}

impl CommandContext<'_> {
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
}

impl core::fmt::Display for CommandError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooManyParams => write!(f, "more than {MAX_ARGS} parameters"),
            Self::NameTaken => write!(f, "the name is already a command"),
            Self::NoFreeEntry => write!(f, "no unused console entry is left"),
        }
    }
}

impl std::error::Error for CommandError {}

/// One record of the command tables (0x50).
#[repr(transparent)]
pub struct CommandEntry(classes::CommandTableEntry);

impl CommandEntry {
    #[must_use]
    pub fn raw(&self) -> &classes::CommandTableEntry {
        &self.0
    }

    #[must_use]
    pub fn name(&self) -> &str {
        text(self.0.name)
    }

    #[must_use]
    pub fn short_name(&self) -> &str {
        text(self.0.shortName)
    }

    #[must_use]
    pub fn opcode(&self) -> u32 {
        self.0.opcode
    }

    fn answers_to(&self, name: &CStr) -> bool {
        let name = name.to_str().unwrap_or("");
        self.name().eq_ignore_ascii_case(name) || (!self.short_name().is_empty() && self.short_name().eq_ignore_ascii_case(name))
    }

    /// An execute of three bytes (`mov al, 1; ret` or `xor al, al; ret`): an entry the binary never uses.
    fn is_stub(&self) -> bool {
        let execute = self.0.execute.cast::<[u8; 3]>();
        !execute.is_null() && matches!(unsafe { *execute }, [0xB0, 0x01, 0xC3] | [0x32, 0xC0, 0xC3])
    }
}

fn text_c<'a>(p: *mut c_void) -> &'a CStr {
    if p.is_null() { c"" } else { unsafe { CStr::from_ptr(p.cast()) } }
}

fn text<'a>(p: *mut c_void) -> &'a str {
    text_c(p).to_str().unwrap_or("")
}

fn table(base: *mut classes::CommandTableEntry, len: usize) -> &'static mut [CommandEntry] {
    unsafe { core::slice::from_raw_parts_mut(base.cast::<CommandEntry>(), len) }
}

/// The console commands (opcodes 0x100 to 0x2c0).
#[must_use]
pub fn console_commands() -> &'static [CommandEntry] {
    table(fns().console_commands, CONSOLE_COMMANDS)
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
    let params: &'static [Param] = Box::leak(params.iter().map(|p| Param(classes::ParamInfo { name: leak(text_c(p.0.name)).as_ptr().cast_mut().cast(), ..p.0 })).collect());
    let slot = take_entry(leak(name), leak(help), params, needs_reference, EXECUTES[index])?;
    *ERASED[index].execute.write().unwrap_or_else(PoisonError::into_inner) = Some(execute);
    let _ = ERASED[index].params.set(params);
    Ok(ErasedCommand { index, slot })
}

fn take_entry(name: &'static CStr, help: &'static CStr, params: &'static [Param], needs_reference: bool, execute: ExecuteFn) -> Result<CommandSlot, CommandError> {
    if params.len() > MAX_ARGS {
        return Err(CommandError::TooManyParams);
    }
    let f = fns();
    let taken = [table(f.keywords, KEYWORDS), table(f.script_functions, SCRIPT_FUNCTIONS), table(f.console_commands, CONSOLE_COMMANDS)].iter().any(|t| t.iter().any(|e| e.answers_to(name)));
    if taken {
        return Err(CommandError::NameTaken);
    }
    let entry = table(f.console_commands, CONSOLE_COMMANDS).iter_mut().find(|e| e.is_stub()).ok_or(CommandError::NoFreeEntry)?;
    let replaced = text(entry.0.name);
    let raw = &mut entry.0;
    raw.name = name.as_ptr().cast_mut().cast();
    raw.shortName = c"".as_ptr().cast_mut().cast();
    raw.helpText = help.as_ptr().cast_mut().cast();
    raw.referenceFunction = u8::from(needs_reference);
    raw.numParams = u16::try_from(params.len()).expect("at most MAX_ARGS");
    raw.params = if params.is_empty() { core::ptr::null_mut() } else { params.as_ptr().cast_mut().cast() };
    raw.parse = f.parse;
    raw.eval = core::ptr::null_mut();
    raw.flags = 0;
    raw.execute = execute as *mut c_void;
    Ok(CommandSlot { opcode: raw.opcode, replaced })
}

#[repr(C, align(8))]
struct ArgSlot([u8; ARG_SIZE]);

unsafe extern "C" fn execute<C: Command>(params: *const classes::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool {
    unsafe { run(C::PARAMS, params, data, this_ref, containing, script, locals, result, offset, C::execute) }
}

unsafe extern "C" fn execute_erased<const I: usize>(params: *const classes::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32) -> bool {
    let (Some(declared), guard) = (ERASED[I].params.get(), ERASED[I].execute.read().unwrap_or_else(PoisonError::into_inner)) else {
        return true;
    };
    match guard.as_ref() {
        Some(f) => unsafe { run(declared, params, data, this_ref, containing, script, locals, result, offset, |ctx| f(ctx)) },
        None => true,
    }
}

macro_rules! executes {
    ($($i:literal) => {
        const EXECUTES: [ExecuteFn; ERASED_COMMANDS] = [$(execute_erased::<$i>),*];
    };
}

executes!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15);

/// Unpacks the line's arguments as `declared` describes them and runs `execute` with them.
#[allow(clippy::too_many_arguments)]
unsafe fn run(declared: &[Param], params: *const classes::ParamInfo, data: *const c_void, this_ref: *mut c_void, containing: *mut c_void, script: *mut c_void, locals: *mut c_void, result: *mut f64, offset: *mut u32, execute: impl FnOnce(&mut CommandContext<'_>) -> bool) -> bool {
    let given = if data.is_null() || offset.is_null() { 0 } else { usize::from(unsafe { data.cast::<u8>().add(*offset as usize).cast::<u16>().read_unaligned() }) };
    let mut slots: [ArgSlot; MAX_ARGS] = core::array::from_fn(|_| ArgSlot([0; ARG_SIZE]));
    let out: [*mut ArgSlot; MAX_ARGS] = core::array::from_fn(|i| &raw mut slots[i]);
    let unpacked = unsafe { (fns().unpack)(params, data, offset, this_ref, containing, script, locals, out[0], out[1], out[2], out[3], out[4], out[5], out[6], out[7]) };
    if !unpacked {
        return false;
    }
    let args: Vec<Arg<'_>> = declared.iter().zip(&slots).enumerate().map(|(i, (param, slot))| if i < given { decode(param.0.typeCode, slot) } else { Arg::Missing }).collect();
    execute(&mut CommandContext { this_ref, args: &args, result })
}

fn decode(kind: u32, slot: &ArgSlot) -> Arg<'_> {
    let bytes = &slot.0;
    let info = unsafe { &*fns().param_types.add(kind as usize) };
    if info.isForm != 0 {
        let form = usize::from_le_bytes(bytes[..8].try_into().expect("8 bytes")) as *const classes::TESForm;
        return unsafe { TESForm::from_ptr(form) }.map_or(Arg::Missing, Arg::Form);
    }
    match kind {
        k if k == ScriptParamType::String as u32 => Arg::Str(CStr::from_bytes_until_nul(bytes).ok().and_then(|s| s.to_str().ok()).unwrap_or("")),
        k if k == ScriptParamType::Float as u32 => Arg::Float(f32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))),
        k if k == ScriptParamType::Axis as u32 => Arg::Char(bytes[0]),
        _ => Arg::Int(i32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"))),
    }
}
