//! The C side of the plugin ABI (`docs/subsystems/abi.md`): the per-plugin [`HostApi`] table, the
//! plain data its entries take, and the helpers [`plugin!`](crate::plugin) wraps a plugin's Rust
//! functions with.

use core::ffi::{CStr, c_char, c_void};
use core::marker::PhantomData;

use overlay::Hotkey;
use skyrim::events::ErasedSource;
use skyrim::input::{Arg, CommandContext, CommandError, CommandSlot, Param};
use skyrim::papyrus::{self, NativeCall, NativeFunction, NativeKind, ParamType, Variable, VirtualMachine};
use sre::{BSEventNotifyControl, TESForm, VMTypeID};

use crate::hooks::raw::{AfterCallback, AroundCallback, BeforeCallback, RawPatch, RawPatchReport, RawSiteReport, RawSubscription, RawValidation, ReplaceCallback};
use crate::settings::Setting;
use crate::ui::{RawFamily, RawFont, UiCallback};
use crate::{CommandRequest, Error, FrameFn, Host, LoadFn, MessageFn, Method, NativeRequest, PapyrusFn};

/// The [`HostApi`] layout this crate builds and reads; entries are only ever appended.
pub const API: u32 = 4;

/// Bytes borrowed for the call, UTF-8 where they are text.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Str {
    pub ptr: *const u8,
    pub len: usize,
}

impl Str {
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self { ptr: bytes.as_ptr(), len: bytes.len() }
    }

    pub(crate) unsafe fn bytes<'a>(self) -> &'a [u8] {
        unsafe { slice(self.ptr, self.len) }
    }

    /// The text; bytes that are not UTF-8 read as empty.
    pub(crate) unsafe fn as_str<'a>(self) -> &'a str {
        core::str::from_utf8(unsafe { self.bytes() }).unwrap_or_default()
    }

    /// `bytes`, or a null pointer for none.
    pub(crate) fn maybe(bytes: Option<&[u8]>) -> Self {
        bytes.map_or(Self { ptr: core::ptr::null(), len: 0 }, Self::new)
    }

    /// The bytes, `None` for a null pointer.
    pub(crate) unsafe fn maybe_bytes<'a>(self) -> Option<&'a [u8]> {
        (!self.ptr.is_null()).then(|| unsafe { self.bytes() })
    }
}

impl From<&str> for Str {
    fn from(text: &str) -> Self {
        Self::new(text.as_bytes())
    }
}

pub(crate) unsafe fn slice<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() { &[] } else { unsafe { core::slice::from_raw_parts(ptr, len) } }
}

/// Receives a call's variable-length results one item at a time, during the call; the receiver
/// copies what it keeps.
#[repr(C)]
pub struct Sink<'a, T> {
    call: extern "C" fn(*mut c_void, T),
    ctx: *mut c_void,
    lifetime: PhantomData<&'a mut ()>,
}

impl<'a, T> Sink<'a, T> {
    pub fn new<F: FnMut(T)>(f: &'a mut F) -> Self {
        extern "C" fn call<F: FnMut(T), T>(ctx: *mut c_void, item: T) {
            unsafe { (*ctx.cast::<F>())(item) }
        }
        Self { call: call::<F, T>, ctx: core::ptr::from_mut(f).cast(), lifetime: PhantomData }
    }

    pub fn put(&self, item: T) {
        (self.call)(self.ctx, item);
    }
}

/// The buffer [`HostApi::read_frame`] fills: asked for once with the frame's width and height,
/// it returns room for `width * height * 4` bytes.
#[repr(C)]
pub struct Alloc<'a> {
    call: extern "C" fn(*mut c_void, usize, usize) -> *mut u8,
    ctx: *mut c_void,
    lifetime: PhantomData<&'a mut ()>,
}

impl<'a> Alloc<'a> {
    pub fn new<F: FnMut(usize, usize) -> *mut u8>(f: &'a mut F) -> Self {
        extern "C" fn call<F: FnMut(usize, usize) -> *mut u8>(ctx: *mut c_void, width: usize, height: usize) -> *mut u8 {
            unsafe { (*ctx.cast::<F>())(width, height) }
        }
        Self { call: call::<F>, ctx: core::ptr::from_mut(f).cast(), lifetime: PhantomData }
    }

    #[must_use]
    pub fn get(&self, width: usize, height: usize) -> *mut u8 {
        (self.call)(self.ctx, width, height)
    }
}

/// A plugin's callable the host keeps; `drop`, when set, runs once the host lets go of it, before
/// the plugin's DLL is unloaded.
#[repr(C)]
pub struct Callback<F> {
    pub call: F,
    pub ctx: *mut c_void,
    pub drop: Option<extern "C" fn(*mut c_void)>,
}

impl<F> Drop for Callback<F> {
    fn drop(&mut self) {
        if let Some(drop) = self.drop {
            drop(self.ctx);
        }
    }
}

unsafe impl<F: Send> Send for Callback<F> {}
unsafe impl<F: Sync> Sync for Callback<F> {}

impl<F> Callback<F> {
    /// `closure` boxed behind `call`, which receives it as `ctx`; `drop` frees the box.
    pub(crate) fn boxed<C: Send + Sync + 'static>(closure: C, call: F) -> Self {
        extern "C" fn drop_box<C>(ctx: *mut c_void) {
            drop(unsafe { Box::from_raw(ctx.cast::<C>()) });
        }
        Self { call, ctx: Box::into_raw(Box::new(closure)).cast(), drop: Some(drop_box::<C>) }
    }
}

pub type MessageCallback = Callback<extern "C" fn(*mut c_void, *const skse::Message)>;
pub type FrameCallback = Callback<extern "C" fn(*mut c_void)>;
pub type PapyrusCallback = Callback<extern "C" fn(*mut c_void, *const VirtualMachine)>;
/// An event handler: the event and the source as the engine passes them, a `BSEventNotifyControl` back.
pub type SinkCallback = Callback<extern "C" fn(*mut c_void, *const c_void, *const sre::BSTEventSource) -> u32>;
/// A Papyrus native: true once the value is written to the `Variable`, which holds None on entry.
pub type NativeCallback = Callback<extern "C" fn(*mut c_void, *const RawNativeCall, *mut sre::BSScript__Variable) -> bool>;
/// A console command's execute function.
pub type CommandCallback = Callback<extern "C" fn(*mut c_void, *mut RawCommandContext) -> bool>;

pub(crate) fn message(f: MessageFn) -> MessageCallback {
    extern "C" fn call(ctx: *mut c_void, msg: *const skse::Message) {
        let f = unsafe { core::mem::transmute::<*mut c_void, MessageFn>(ctx) };
        if let Some(msg) = unsafe { msg.as_ref() } {
            f(msg);
        }
    }
    Callback { call, ctx: f as *mut c_void, drop: None }
}

pub(crate) fn frame(f: FrameFn) -> FrameCallback {
    extern "C" fn call(ctx: *mut c_void) {
        let f = unsafe { core::mem::transmute::<*mut c_void, FrameFn>(ctx) };
        f();
    }
    Callback { call, ctx: f as *mut c_void, drop: None }
}

pub(crate) fn papyrus(f: PapyrusFn) -> PapyrusCallback {
    extern "C" fn call(ctx: *mut c_void, vm: *const VirtualMachine) {
        let f = unsafe { core::mem::transmute::<*mut c_void, PapyrusFn>(ctx) };
        if let Some(vm) = unsafe { vm.as_ref() } {
            f(vm);
        }
    }
    Callback { call, ctx: f as *mut c_void, drop: None }
}

pub(crate) fn sink<C: Fn(*const c_void, *const sre::BSTEventSource) -> BSEventNotifyControl + Send + Sync + 'static>(f: C) -> SinkCallback {
    extern "C" fn call<C: Fn(*const c_void, *const sre::BSTEventSource) -> BSEventNotifyControl>(ctx: *mut c_void, event: *const c_void, source: *const sre::BSTEventSource) -> u32 {
        unsafe { (*ctx.cast::<C>())(event, source) as u32 }
    }
    SinkCallback::boxed(f, call::<C>)
}

/// What a handler returned; a value this side does not know continues the walk.
pub(crate) fn control(value: u32) -> BSEventNotifyControl {
    BSEventNotifyControl::from_u32(value).unwrap_or(BSEventNotifyControl::Continue)
}

pub(crate) fn native<C: Fn(&NativeCall<'_>) -> Option<Variable> + Send + Sync + 'static>(f: C) -> NativeCallback {
    extern "C" fn call<C: Fn(&NativeCall<'_>) -> Option<Variable>>(ctx: *mut c_void, raw: *const RawNativeCall, out: *mut sre::BSScript__Variable) -> bool {
        let value = unsafe { raw.as_ref() }.and_then(|raw| unsafe { raw.call() }).and_then(|call| unsafe { (*ctx.cast::<C>())(&call) });
        // The None the host put there owns nothing, so it is overwritten without a reset.
        value.map(|v| unsafe { out.cast::<Variable>().write(v) }).is_some()
    }
    NativeCallback::boxed(f, call::<C>)
}

pub(crate) fn command<C: Fn(&mut CommandContext<'_>) -> bool + Send + Sync + 'static>(f: C) -> CommandCallback {
    extern "C" fn call<C: Fn(&mut CommandContext<'_>) -> bool>(ctx: *mut c_void, raw: *mut RawCommandContext) -> bool {
        let Some(raw) = (unsafe { raw.as_ref() }) else { return false };
        let args: Vec<Arg<'_>> = unsafe { slice(raw.args, raw.len) }.iter().map(|a| unsafe { a.to_arg() }).collect();
        unsafe { (*ctx.cast::<C>())(&mut CommandContext::new(raw.this_ref.cast_mut().cast(), &args, raw.result)) }
    }
    CommandCallback::boxed(f, call::<C>)
}

/// A native call as it crosses: the engine's VM, `self` and argument variables, and the stack.
#[repr(C)]
pub struct RawNativeCall {
    pub vm: *const VirtualMachine,
    pub self_: *const sre::BSScript__Variable,
    pub args: *const *const sre::BSScript__Variable,
    pub len: usize,
    pub stack_id: u32,
}

impl RawNativeCall {
    pub(crate) fn new(call: &NativeCall<'_>) -> Self {
        Self { vm: call.vm, self_: call.self_.raw(), args: call.args.as_ptr().cast(), len: call.args.len(), stack_id: call.stack_id }
    }

    /// The call as the plugin's native reads it, `None` without the VM or `self`.
    unsafe fn call(&self) -> Option<NativeCall<'_>> {
        let vm = unsafe { self.vm.as_ref() }?;
        let self_ = Variable::view(unsafe { self.self_.as_ref() }?);
        Some(NativeCall { vm, self_, args: unsafe { slice(self.args.cast::<&Variable>(), self.len) }, stack_id: self.stack_id })
    }
}

/// A [`ParamType`], or none for a native that returns nothing: `kind` 0 none, 1 Int, 2 Float,
/// 3 Bool, 4 String, 5 a form, then 6 to 10 the arrays of those in the same order; `type_id` is
/// the form's `VMTypeID`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawParamType {
    pub kind: u32,
    pub type_id: u32,
}

impl RawParamType {
    pub(crate) fn new(ty: Option<ParamType>) -> Self {
        let (kind, type_id) = match ty {
            None => (0, 0),
            Some(ParamType::Int) => (1, 0),
            Some(ParamType::Float) => (2, 0),
            Some(ParamType::Bool) => (3, 0),
            Some(ParamType::String) => (4, 0),
            Some(ParamType::Form(id)) => (5, id as u32),
            Some(ParamType::IntArray) => (6, 0),
            Some(ParamType::FloatArray) => (7, 0),
            Some(ParamType::BoolArray) => (8, 0),
            Some(ParamType::StringArray) => (9, 0),
            Some(ParamType::FormArray(id)) => (10, id as u32),
        };
        Self { kind, type_id }
    }

    /// `Unsupported` for a kind or a type id this side does not know.
    pub(crate) fn to_param(self) -> Result<Option<ParamType>, Error> {
        let unknown = || Error::Unsupported { what: format!("Papyrus type {} ({})", self.kind, self.type_id) };
        let form = || VMTypeID::from_u32(self.type_id).ok_or_else(unknown);
        Ok(Some(match self.kind {
            0 => return Ok(None),
            1 => ParamType::Int,
            2 => ParamType::Float,
            3 => ParamType::Bool,
            4 => ParamType::String,
            5 => ParamType::Form(form()?),
            6 => ParamType::IntArray,
            7 => ParamType::FloatArray,
            8 => ParamType::BoolArray,
            9 => ParamType::StringArray,
            10 => ParamType::FormArray(form()?),
            _ => return Err(unknown()),
        }))
    }
}

/// A native to register: `kind` 0 global, 1 method, 2 latent.
#[repr(C)]
pub struct RawNativeRequest {
    pub vm: *const VirtualMachine,
    pub class: Str,
    pub name: Str,
    pub kind: u32,
    pub params: *const RawParamType,
    pub len: usize,
    pub ret: RawParamType,
}

const NATIVE_KINDS: [NativeKind; 3] = [NativeKind::Global, NativeKind::Method, NativeKind::Latent];

impl RawNativeRequest {
    /// `request` as it crosses, with `params` made from its parameters by [`RawParamType::new`].
    pub(crate) fn new(request: &NativeRequest<'_>, params: &[RawParamType]) -> Self {
        let kind = NATIVE_KINDS.iter().position(|k| *k == request.kind).unwrap_or_default();
        Self { vm: request.vm, class: Str::from(request.class), name: Str::from(request.name), kind: u32::try_from(kind).unwrap_or_default(), params: params.as_ptr(), len: params.len(), ret: RawParamType::new(request.ret) }
    }

    /// The parameter types, copied out of the plugin's memory.
    pub(crate) unsafe fn params(&self) -> Result<Vec<ParamType>, Error> {
        unsafe { slice(self.params, self.len) }.iter().map(|p| p.to_param()?.ok_or_else(|| Error::Unsupported { what: "a parameter without a type".to_owned() })).collect()
    }

    /// The request with the parameter types `params` from [`params`](Self::params).
    pub(crate) unsafe fn to_request<'a>(&'a self, params: &'a [ParamType]) -> Result<NativeRequest<'a>, Error> {
        let vm = unsafe { self.vm.as_ref() }.ok_or_else(|| Error::Unsupported { what: "a native without the VM".to_owned() })?;
        let kind = *usize::try_from(self.kind).ok().and_then(|k| NATIVE_KINDS.get(k)).ok_or_else(|| Error::Unsupported { what: format!("native kind {}", self.kind) })?;
        Ok(NativeRequest { vm, class: unsafe { self.class.as_str() }, name: unsafe { self.name.as_str() }, kind, params, ret: self.ret.to_param()? })
    }
}

/// A console command to register, its name and help NUL-terminated.
#[repr(C)]
pub struct RawCommandRequest {
    pub name: *const c_char,
    pub help: *const c_char,
    pub params: *const sre::ParamInfo,
    pub len: usize,
    pub needs_reference: bool,
}

impl RawCommandRequest {
    pub(crate) fn new(request: &CommandRequest<'_>) -> Self {
        Self { name: request.name.as_ptr(), help: request.help.as_ptr(), params: request.params.as_ptr().cast(), len: request.params.len(), needs_reference: request.needs_reference }
    }

    pub(crate) unsafe fn to_request(&self) -> CommandRequest<'_> {
        CommandRequest { name: unsafe { c_str(self.name) }, help: unsafe { c_str(self.help) }, params: unsafe { slice(self.params.cast::<Param>(), self.len) }, needs_reference: self.needs_reference }
    }
}

/// The text at `p`, empty for null.
pub(crate) unsafe fn c_str<'a>(p: *const c_char) -> &'a CStr {
    if p.is_null() { c"" } else { unsafe { CStr::from_ptr(p) } }
}

/// A command's console entry; `replaced` is the host's text, alive for the life of the game.
#[repr(C)]
pub struct RawCommandSlot {
    pub opcode: u32,
    pub replaced: Str,
}

impl RawCommandSlot {
    pub(crate) fn new(slot: CommandSlot) -> Self {
        Self { opcode: slot.opcode, replaced: Str::from(slot.replaced) }
    }

    pub(crate) unsafe fn to_slot(&self) -> CommandSlot {
        CommandSlot { opcode: self.opcode, replaced: unsafe { self.replaced.as_str() } }
    }
}

/// A command's run: the reference it runs on, the arguments, the statement's result.
#[repr(C)]
pub struct RawCommandContext {
    pub this_ref: *const sre::TESObjectREFR,
    pub args: *const RawArg,
    pub len: usize,
    pub result: *mut f64,
}

/// An [`Arg`]: `tag` 0 missing, 1 `int`, 2 `float`, 3 `text`, 4 `form`, 5 `char`.
#[repr(C)]
pub struct RawArg {
    pub tag: u32,
    pub int: i32,
    pub float: f32,
    pub text: Str,
    pub form: *const TESForm,
    pub char: u8,
}

impl RawArg {
    const MISSING: u32 = 0;
    const INT: u32 = 1;
    const FLOAT: u32 = 2;
    const TEXT: u32 = 3;
    const FORM: u32 = 4;
    const CHAR: u32 = 5;

    /// `arg` as it crosses; its text and form stay borrowed.
    pub(crate) fn new(arg: &Arg<'_>) -> Self {
        let missing = Self { tag: Self::MISSING, int: 0, float: 0.0, text: Str::new(&[]), form: core::ptr::null(), char: 0 };
        match *arg {
            Arg::Missing => missing,
            Arg::Int(int) => Self { tag: Self::INT, int, ..missing },
            Arg::Float(float) => Self { tag: Self::FLOAT, float, ..missing },
            Arg::Str(text) => Self { tag: Self::TEXT, text: Str::from(text), ..missing },
            Arg::Form(form) => Self { tag: Self::FORM, form, ..missing },
            Arg::Char(char) => Self { tag: Self::CHAR, char, ..missing },
        }
    }

    /// A tag this side does not know reads as missing.
    pub(crate) unsafe fn to_arg<'a>(&self) -> Arg<'a> {
        match self.tag {
            Self::INT => Arg::Int(self.int),
            Self::FLOAT => Arg::Float(self.float),
            Self::TEXT => Arg::Str(unsafe { self.text.as_str() }),
            Self::FORM => unsafe { self.form.as_ref() }.map_or(Arg::Missing, Arg::Form),
            Self::CHAR => Arg::Char(self.char),
            _ => Arg::Missing,
        }
    }
}

/// Why an entry failed, as a `u32` beside its detail; numbers are never reused.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok = 0,
    Unsupported = 1,
    NoSource = 2,
    Taken = 3,
    Full = 4,
    Papyrus = 5,
    Command = 6,
    Hook = 7,
}

/// The detail of a failed entry's error: the thing refused, its owner, the limit. A `Papyrus` or
/// `Command` error names its variant in `code` (`papyrus::Error`: 0 `UnknownType` with the type id
/// in `limit`, 1 `Bind` with the class in `what` and the name in `owner`; `CommandError`: 0
/// `TooManyParams`, 1 `NameTaken`, 2 `NoFreeEntry`, 3 `NotFound`), and `text` is its message,
/// which a side that does not know the code reports as `Unsupported`. The fields after `text`
/// carry a hook error (`hooks::Error`'s [`Crossing`]); empty or null where unused.
#[repr(C)]
pub struct RawError {
    pub what: Str,
    pub owner: Str,
    pub limit: usize,
    pub code: u32,
    pub text: Str,
    pub other: Str,
    pub names: *const Str,
    pub names_len: usize,
    pub expected: Str,
    pub found: Str,
    pub validation: RawValidation,
}

impl RawError {
    /// The error with only its message.
    pub(crate) fn text(text: &str) -> Self {
        let empty = Str::new(&[]);
        Self { what: empty, owner: empty, limit: 0, code: 0, text: Str::from(text), other: empty, names: core::ptr::null(), names_len: 0, expected: empty, found: empty, validation: RawValidation::default() }
    }
}

/// An error that crosses as a [`Status`] and a [`RawError`].
pub(crate) trait Crossing: Sized {
    /// The host's side: puts the detail through `detail` and returns the status.
    fn fail(&self, detail: &Sink<'_, RawError>) -> u32;
    /// The plugin's side: the error for a status other than `Ok`.
    fn read(status: u32, detail: Detail) -> Self;
    fn unsupported(what: String) -> Self;
}

const COMMAND_ERRORS: [CommandError; 4] = [CommandError::TooManyParams, CommandError::NameTaken, CommandError::NoFreeEntry, CommandError::NotFound];

impl Crossing for Error {
    fn fail(&self, detail: &Sink<'_, RawError>) -> u32 {
        let text = self.to_string();
        let (status, what, owner, limit, code) = match self {
            Self::NoSource => (Status::NoSource, "", "", 0, 0),
            Self::Taken { what, owner } => (Status::Taken, what.as_str(), owner.as_str(), 0, 0),
            Self::Full { what, limit } => (Status::Full, what.as_str(), "", *limit, 0),
            Self::Unsupported { what } => (Status::Unsupported, what.as_str(), "", 0, 0),
            Self::Papyrus(papyrus::Error::UnknownType(id)) => (Status::Papyrus, "", "", *id as usize, 0),
            Self::Papyrus(papyrus::Error::Bind { class, name }) => (Status::Papyrus, class.as_str(), name.as_str(), 0, 1),
            Self::Command(e) => (Status::Command, "", "", 0, COMMAND_ERRORS.iter().position(|c| core::mem::discriminant(c) == core::mem::discriminant(e)).unwrap_or_default()),
        };
        detail.put(RawError { what: Str::from(what), owner: Str::from(owner), limit, code: u32::try_from(code).unwrap_or_default(), ..RawError::text(&text) });
        status as u32
    }

    fn read(status: u32, d: Detail) -> Self {
        let Detail { what, owner, limit, code, text, .. } = d;
        match status {
            s if s == Status::NoSource as u32 => Self::NoSource,
            s if s == Status::Taken as u32 => Self::Taken { what, owner },
            s if s == Status::Full as u32 => Self::Full { what, limit },
            s if s == Status::Papyrus as u32 => match code {
                0 => u32::try_from(limit).ok().and_then(VMTypeID::from_u32).map_or(Self::Unsupported { what: text }, |id| Self::Papyrus(papyrus::Error::UnknownType(id))),
                1 => Self::Papyrus(papyrus::Error::Bind { class: what, name: owner }),
                _ => Self::Unsupported { what: text },
            },
            s if s == Status::Command as u32 => usize::try_from(code).ok().and_then(|c| COMMAND_ERRORS.into_iter().nth(c)).map_or(Self::Unsupported { what: text }, Self::Command),
            _ => Self::Unsupported { what },
        }
    }

    fn unsupported(what: String) -> Self {
        Self::Unsupported { what }
    }
}

/// The plugin's side of a fallible entry: `call` returns its status, or `None` when the loader
/// lacks the entry, which is `Unsupported` for `what`.
pub(crate) fn outcome<E: Crossing>(what: &str, call: impl FnOnce(Sink<'_, RawError>) -> Option<u32>) -> Result<(), E> {
    let mut detail = Detail::default();
    let mut keep = |d: RawError| detail = unsafe { Detail::copy(&d) };
    let Some(status) = call(Sink::new(&mut keep)) else {
        return Err(E::unsupported(what.to_owned()));
    };
    if status == Status::Ok as u32 { Ok(()) } else { Err(E::read(status, detail)) }
}

/// [`outcome`] for the framework's own [`Error`].
pub(crate) fn status(what: &str, call: impl FnOnce(Sink<'_, RawError>) -> Option<u32>) -> Result<(), Error> {
    outcome(what, call)
}

/// A [`RawError`] copied out of the call.
#[derive(Default)]
pub(crate) struct Detail {
    pub what: String,
    pub owner: String,
    pub limit: usize,
    pub code: u32,
    pub text: String,
    pub other: String,
    pub names: Vec<String>,
    pub expected: Vec<u8>,
    pub found: Vec<u8>,
    pub validation: RawValidation,
}

impl Detail {
    unsafe fn copy(d: &RawError) -> Self {
        let text = |s: Str| unsafe { s.as_str() }.to_owned();
        Self {
            what: text(d.what),
            owner: text(d.owner),
            limit: d.limit,
            code: d.code,
            text: text(d.text),
            other: text(d.other),
            names: unsafe { slice(d.names, d.names_len) }.iter().map(|&n| text(n)).collect(),
            expected: unsafe { d.expected.bytes() }.to_vec(),
            found: unsafe { d.found.bytes() }.to_vec(),
            validation: d.validation,
        }
    }
}

/// The mouse's movement in [`HostApi::mouse`].
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMouse {
    pub dx: i32,
    pub dy: i32,
}

/// The gamepad's state in [`HostApi::gamepad`].
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPad {
    pub held: u32,
    pub triggers: [f32; 2],
    pub sticks: [[f32; 2]; 2],
}

/// A button press (`pressed`) or release of the mouse or the gamepad.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawPress {
    pub id: u32,
    pub pressed: bool,
}

/// A button of an input filter: an `InputDevice` and the button's id on it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawButton {
    pub device: u32,
    pub id: u32,
}

/// A [`Setting`]: `kind` 0 is `flag`, 1 `number`, 2 `text`.
#[repr(C)]
pub struct RawSetting {
    pub kind: u32,
    pub flag: bool,
    pub number: f64,
    pub text: Str,
}

impl RawSetting {
    const BOOL: u32 = 0;
    const NUMBER: u32 = 1;
    const TEXT: u32 = 2;

    /// `setting` as it crosses; the text stays borrowed from it.
    pub(crate) fn new(setting: &Setting) -> Self {
        let empty = Self { kind: Self::BOOL, flag: false, number: 0.0, text: Str::new(&[]) };
        match setting {
            Setting::Bool(v) => Self { flag: *v, ..empty },
            Setting::Number(v) => Self { kind: Self::NUMBER, number: *v, ..empty },
            Setting::Text(v) => Self { kind: Self::TEXT, text: Str::from(v.as_str()), ..empty },
        }
    }

    /// `None` for a kind this side does not know.
    pub(crate) unsafe fn to_setting(&self) -> Option<Setting> {
        match self.kind {
            Self::BOOL => Some(Setting::Bool(self.flag)),
            Self::NUMBER => Some(Setting::Number(self.number)),
            Self::TEXT => Some(Setting::Text(unsafe { self.text.as_str() }.to_owned())),
            _ => None,
        }
    }
}

/// One accepted plugin in [`HostApi::plugins`].
#[repr(C)]
pub struct PluginRecord {
    pub name: Str,
    pub version: Str,
    pub file: Str,
    pub priority: i32,
}

/// The host's functions for one plugin, leaked by the host for the life of the game and kept
/// across the plugin's reloads. Every entry takes the table first, which names the plugin.
/// `size` is the table's size as the loader built it: a field past it does not exist on that
/// loader, which is how a plugin built against a later table runs on an earlier loader.
#[repr(C)]
pub struct HostApi {
    pub version: u32,
    pub size: u32,
    /// The plugin's name.
    pub name: Str,
    pub skse: *const skse::Interface,
    pub on_message: Option<unsafe extern "C" fn(*const Self, MessageCallback)>,
    pub on_plugin_message: Option<unsafe extern "C" fn(*const Self, Str, MessageCallback)>,
    /// Writes the alias's slot on success.
    pub serve_alias: Option<unsafe extern "C" fn(*const Self, Str, MessageCallback, *mut usize, Sink<'_, RawError>) -> u32>,
    pub alias_send: Option<unsafe extern "C" fn(*const Self, usize, *const c_char, u32, *mut c_void, u32) -> bool>,
    /// Writes the object's address on success.
    pub export_interface: Option<unsafe extern "C" fn(*const Self, Str, *const Method, usize, *mut usize, Sink<'_, RawError>) -> u32>,
    pub on_frame: Option<unsafe extern "C" fn(*const Self, FrameCallback)>,
    pub on_papyrus: Option<unsafe extern "C" fn(*const Self, PapyrusCallback)>,
    /// Name, default key and `Scope` (0 overlay, 1 game); returns the action, `usize::MAX` for an unknown scope.
    pub action: Option<unsafe extern "C" fn(*const Self, Str, Str, u32) -> usize>,
    pub fired: Option<unsafe extern "C" fn(*const Self, usize) -> bool>,
    /// Writes the bound key and returns true, false while unbound.
    pub key: Option<unsafe extern "C" fn(*const Self, usize, *mut Hotkey) -> bool>,
    pub held: Option<unsafe extern "C" fn(*const Self, usize) -> bool>,
    /// Writes the seconds the key was held and returns true when it was released.
    pub released: Option<unsafe extern "C" fn(*const Self, usize, *mut f32) -> bool>,
    pub key_held: Option<unsafe extern "C" fn(*const Self, u32) -> bool>,
    pub key_released: Option<unsafe extern "C" fn(*const Self, u32, *mut f32) -> bool>,
    pub mouse: Option<unsafe extern "C" fn(*const Self, *mut RawMouse, Sink<'_, RawPress>)>,
    pub gamepad: Option<unsafe extern "C" fn(*const Self, *mut RawPad, Sink<'_, RawPress>)>,
    /// Name, user events, buttons; returns the filter.
    pub input_filter: Option<unsafe extern "C" fn(*const Self, Str, *const Str, usize, *const RawButton, usize) -> usize>,
    pub set_filter_active: Option<unsafe extern "C" fn(*const Self, usize, bool)>,
    pub filter_active: Option<unsafe extern "C" fn(*const Self, usize) -> bool>,
    /// Puts the value of the key, when it has one.
    pub setting: Option<unsafe extern "C" fn(*const Self, Str, Sink<'_, RawSetting>)>,
    pub set_setting: Option<unsafe extern "C" fn(*const Self, Str, RawSetting)>,
    pub setting_keys: Option<unsafe extern "C" fn(*const Self, Str, Sink<'_, Str>)>,
    pub remove_settings: Option<unsafe extern "C" fn(*const Self, Str) -> usize>,
    pub set_tab_title: Option<unsafe extern "C" fn(*const Self, Str)>,
    pub show_tab: Option<unsafe extern "C" fn(*const Self)>,
    pub show_section: Option<unsafe extern "C" fn(*const Self, Str)>,
    /// Fills the buffer and returns true, or puts why not and returns false.
    pub read_frame: Option<unsafe extern "C" fn(*const Self, Alloc<'_>, Sink<'_, Str>) -> bool>,
    /// Puts each plugin's record, each followed by its `load_before` and `load_after` names.
    pub plugins: Option<unsafe extern "C" fn(*const Self, Sink<'_, PluginRecord>, Sink<'_, Str>, Sink<'_, Str>)>,
    /// The Rust-ABI table of stages 1 to 3, null since; the slot stays so the entries after it keep their offsets.
    pub legacy: *const c_void,
    pub add_sink: Option<unsafe extern "C" fn(*const Self, ErasedSource, SinkCallback)>,
    pub add_player_graph_sink: Option<unsafe extern "C" fn(*const Self, SinkCallback)>,
    /// Writes the host's function object on success.
    pub register_native: Option<unsafe extern "C" fn(*const Self, *const RawNativeRequest, NativeCallback, *mut *const NativeFunction, Sink<'_, RawError>) -> u32>,
    /// Writes the console entry on success.
    pub register_command: Option<unsafe extern "C" fn(*const Self, *const RawCommandRequest, CommandCallback, *mut RawCommandSlot, Sink<'_, RawError>) -> u32>,
    /// The engine command's name; writes its entry on success.
    pub replace_command: Option<unsafe extern "C" fn(*const Self, *const c_char, CommandCallback, *mut RawCommandSlot, Sink<'_, RawError>) -> u32>,
    /// A hook subscription of each phase; writes its id on success.
    pub hook_before: Option<unsafe extern "C" fn(*const Self, *const RawSubscription, BeforeCallback, *mut u64, Sink<'_, RawError>) -> u32>,
    pub hook_around: Option<unsafe extern "C" fn(*const Self, *const RawSubscription, AroundCallback, *mut u64, Sink<'_, RawError>) -> u32>,
    pub hook_replace: Option<unsafe extern "C" fn(*const Self, *const RawSubscription, ReplaceCallback, *mut u64, Sink<'_, RawError>) -> u32>,
    pub hook_after: Option<unsafe extern "C" fn(*const Self, *const RawSubscription, AfterCallback, *mut u64, Sink<'_, RawError>) -> u32>,
    pub hook_unsubscribe: Option<unsafe extern "C" fn(*const Self, u64) -> bool>,
    pub hook_remove_owner: Option<unsafe extern "C" fn(*const Self, Str) -> usize>,
    pub hook_set_enabled: Option<unsafe extern "C" fn(*const Self, u64, bool) -> bool>,
    pub hook_diagnostics: Option<unsafe extern "C" fn(*const Self, Sink<'_, RawSiteReport>)>,
    pub hook_messages: Option<unsafe extern "C" fn(*const Self, Sink<'_, Str>)>,
    /// Writes the patch's id on success.
    pub hook_patch: Option<unsafe extern "C" fn(*const Self, *const RawPatch, *mut u64, Sink<'_, RawError>) -> u32>,
    pub hook_set_patch_enabled: Option<unsafe extern "C" fn(*const Self, u64, bool, Sink<'_, RawError>) -> u32>,
    pub hook_unpatch: Option<unsafe extern "C" fn(*const Self, u64) -> bool>,
    pub hook_patches: Option<unsafe extern "C" fn(*const Self, Sink<'_, RawPatchReport>)>,
    /// A section of the plugin's tab, named, after those added before.
    pub add_section: Option<unsafe extern "C" fn(*const Self, Str, UiCallback)>,
    pub add_hud: Option<unsafe extern "C" fn(*const Self, UiCallback)>,
    /// Puts every font file the overlay draws with, then every family with its fonts in fallback order.
    pub fonts: Option<unsafe extern "C" fn(*const Self, Sink<'_, RawFont>, Sink<'_, RawFamily>)>,
}

/// A plugin's [`HostApi`], read only through [`entry!`] past the header.
#[derive(Clone, Copy)]
pub(crate) struct Api(pub(crate) *const HostApi);

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

impl Api {
    /// Whether the loader's table holds the pointer-sized field at `offset`.
    pub(crate) fn reaches(self, offset: usize) -> bool {
        offset + size_of::<usize>() <= unsafe { (*self.0).size } as usize
    }
}

/// The entry `$field` of an [`Api`], `None` when the loader's table ends before it or leaves it empty.
macro_rules! entry {
    ($api:expr, $field:ident) => {{
        let api: $crate::abi::Api = $api;
        if api.reaches(core::mem::offset_of!($crate::abi::HostApi, $field)) { unsafe { (*api.0).$field } } else { None }
    }};
}

/// Calls the entry `$field` with the table and the arguments; `None` when the loader lacks it.
macro_rules! call {
    ($api:expr, $field:ident($($arg:expr),* $(,)?)) => {{
        let api: $crate::abi::Api = $api;
        match $crate::abi::entry!(api, $field) {
            Some(f) => Some(unsafe { f(api.0, $($arg),*) }),
            None => None,
        }
    }};
}

pub(crate) use {call, entry};

pub type RawLoadFn = extern "C" fn(*const HostApi) -> bool;
pub type RawSaveFn = extern "C" fn(Sink<'_, Str>);
pub type RawRestoreFn = extern "C" fn(*const u8, usize);

/// [`plugin!`](crate::plugin)'s `load`: the plugin's [`Host`], one per DLL generation, then its `load`.
#[doc(hidden)]
pub fn start(api: *const HostApi, load: LoadFn) -> bool {
    static HOST: std::sync::OnceLock<Host> = std::sync::OnceLock::new();
    Host::new(api).is_some_and(|host| load(HOST.get_or_init(|| host)))
}

#[doc(hidden)]
pub fn save(writer: &Sink<'_, Str>, save_state: fn() -> Vec<u8>) {
    writer.put(Str::new(&save_state()));
}

/// # Safety
///
/// `ptr` points to `len` readable bytes.
#[doc(hidden)]
pub unsafe fn restore(ptr: *const u8, len: usize, restore_state: fn(&[u8])) {
    restore_state(unsafe { slice(ptr, len) });
}

/// The names of a descriptor's null-terminated `load_before` or `load_after` array.
pub(crate) unsafe fn names(list: *const *const c_char) -> Vec<String> {
    let mut names = Vec::new();
    if list.is_null() {
        return names;
    }
    for i in 0.. {
        let name = unsafe { *list.add(i) };
        if name.is_null() {
            break;
        }
        names.push(unsafe { core::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned());
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_crosses_and_comes_back() {
        for setting in [Setting::Bool(true), Setting::Number(-2.5), Setting::Text("a \"b\" é".to_owned()), Setting::Text(String::new())] {
            assert_eq!(unsafe { RawSetting::new(&setting).to_setting() }, Some(setting));
        }
        let unknown = RawSetting { kind: 9, ..RawSetting::new(&Setting::Bool(true)) };
        assert_eq!(unsafe { unknown.to_setting() }, None, "a kind from a later side");
    }

    #[test]
    fn a_status_comes_back_as_its_error() {
        let detail = |code: u32| move |sink: Sink<'_, RawError>| {
            sink.put(RawError { what: Str::from("thing"), owner: Str::from("other"), limit: 4, ..RawError::text("message") });
            Some(code)
        };
        assert!(status("x", detail(Status::Ok as u32)).is_ok());
        assert!(matches!(status("x", detail(Status::Taken as u32)), Err(Error::Taken { what, owner }) if what == "thing" && owner == "other"));
        assert!(matches!(status("x", detail(Status::Full as u32)), Err(Error::Full { limit: 4, .. })));
        assert!(matches!(status("x", detail(99)), Err(Error::Unsupported { what }) if what == "thing"), "a status from a later side");
        assert!(matches!(status("serving a name", |_| None), Err(Error::Unsupported { what }) if what == "serving a name"), "no entry");
        let mut back = Vec::new();
        let mut keep = |d: RawError| back.push(unsafe { (d.what.as_str().to_owned(), d.owner.as_str().to_owned(), d.limit) });
        let taken = Error::Taken { what: "thing".to_owned(), owner: "other".to_owned() };
        assert_eq!(taken.fail(&Sink::new(&mut keep)), Status::Taken as u32);
        assert_eq!(back, [("thing".to_owned(), "other".to_owned(), 0)]);
    }

    /// `e` through the host's side of an entry and back through the plugin's.
    fn cross(e: &Error) -> Error {
        status("x", |sink| Some(e.fail(&sink))).unwrap_err()
    }

    #[test]
    fn papyrus_and_command_errors_come_back_typed() {
        assert!(matches!(cross(&Error::Papyrus(papyrus::Error::UnknownType(VMTypeID::Keyword))), Error::Papyrus(papyrus::Error::UnknownType(VMTypeID::Keyword))));
        assert!(matches!(cross(&Error::Papyrus(papyrus::Error::Bind { class: "Mod".to_owned(), name: "Twice".to_owned() })), Error::Papyrus(papyrus::Error::Bind { class, name }) if class == "Mod" && name == "Twice"));
        for e in COMMAND_ERRORS {
            let text = e.to_string();
            assert!(matches!(cross(&Error::Command(e)), Error::Command(back) if back.to_string() == text));
        }
        assert!(matches!(cross(&Error::NoSource), Error::NoSource));
        assert!(matches!(cross(&Error::Full { what: "commands".to_owned(), limit: 16 }), Error::Full { what, limit: 16 } if what == "commands"));
        assert!(matches!(cross(&Error::Unsupported { what: "sinks".to_owned() }), Error::Unsupported { what } if what == "sinks"));

        let later = |status: Status, code: u32| move |sink: Sink<'_, RawError>| {
            sink.put(RawError { code, ..RawError::text("a later error") });
            Some(status as u32)
        };
        assert!(matches!(status("x", later(Status::Papyrus, 7)), Err(Error::Unsupported { what }) if what == "a later error"));
        assert!(matches!(status("x", later(Status::Command, 4)), Err(Error::Unsupported { what }) if what == "a later error"));
    }

    #[test]
    fn a_param_type_crosses_and_comes_back() {
        let all = [ParamType::Int, ParamType::Float, ParamType::Bool, ParamType::String, ParamType::Form(VMTypeID::Actor), ParamType::IntArray, ParamType::FloatArray, ParamType::BoolArray, ParamType::StringArray, ParamType::FormArray(VMTypeID::Keyword)];
        for ty in all.map(Some).into_iter().chain([None]) {
            assert_eq!(RawParamType::new(ty).to_param().ok(), Some(ty));
        }
        assert!(RawParamType { kind: 11, type_id: 0 }.to_param().is_err(), "a kind from a later side");
        assert!(RawParamType { kind: 5, type_id: 1 }.to_param().is_err(), "a type id this side does not know");
    }

    #[test]
    fn an_arg_crosses_and_comes_back() {
        let form: &'static TESForm = Box::leak(Box::new(TESForm { formID: 0x14, ..unsafe { core::mem::zeroed() } }));
        let text = String::from("pos");
        for arg in [Arg::Missing, Arg::Int(-3), Arg::Float(1.5), Arg::Str(&text), Arg::Str(""), Arg::Form(form), Arg::Char(b'X')] {
            assert_eq!(format!("{:?}", unsafe { RawArg::new(&arg).to_arg() }), format!("{arg:?}"));
        }
        let later = RawArg { tag: 9, ..RawArg::new(&Arg::Int(1)) };
        assert!(matches!(unsafe { later.to_arg() }, Arg::Missing), "a tag from a later side");
        let null_form = RawArg { form: core::ptr::null(), ..RawArg::new(&Arg::Form(form)) };
        assert!(matches!(unsafe { null_form.to_arg() }, Arg::Missing));
    }
}
