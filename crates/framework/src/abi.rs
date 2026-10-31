//! The C side of the plugin ABI (`docs/subsystems/abi.md`): the per-plugin [`HostApi`] table, the
//! plain data its entries take, and the helpers [`plugin!`](crate::plugin) wraps a plugin's Rust
//! functions with.

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;

use overlay::Hotkey;
use skyrim::papyrus::VirtualMachine;

use crate::settings::Setting;
use crate::{Error, FrameFn, Host, Legacy, LoadFn, MessageFn, Method, PapyrusFn};

/// The [`HostApi`] layout this crate builds and reads; entries are only ever appended.
pub const API: u32 = 1;

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

pub type MessageCallback = Callback<extern "C" fn(*mut c_void, *const skse::Message)>;
pub type FrameCallback = Callback<extern "C" fn(*mut c_void)>;
pub type PapyrusCallback = Callback<extern "C" fn(*mut c_void, *const VirtualMachine)>;

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
}

/// The detail of a failed entry's [`Error`]: the thing refused, its owner, the limit.
#[repr(C)]
pub struct RawError {
    pub what: Str,
    pub owner: Str,
    pub limit: usize,
}

/// The host's side of a fallible entry: the status for `e`, its detail through `detail`.
pub(crate) fn fail(e: &Error, detail: &Sink<'_, RawError>) -> u32 {
    let text = e.to_string();
    let (status, what, owner, limit) = match e {
        Error::NoSource => (Status::NoSource, "", "", 0),
        Error::Taken { what, owner } => (Status::Taken, what.as_str(), owner.as_str(), 0),
        Error::Full { what, limit } => (Status::Full, what.as_str(), "", *limit),
        Error::Unsupported { what } => (Status::Unsupported, what.as_str(), "", 0),
        Error::Papyrus(_) => (Status::Papyrus, text.as_str(), "", 0),
        Error::Command(_) => (Status::Command, text.as_str(), "", 0),
    };
    detail.put(RawError { what: Str::from(what), owner: Str::from(owner), limit });
    status as u32
}

/// The plugin's side of a fallible entry: `call` returns its status, or `None` when the loader
/// lacks the entry, which is `Unsupported` for `what`.
pub(crate) fn status(what: &str, call: impl FnOnce(Sink<'_, RawError>) -> Option<u32>) -> Result<(), Error> {
    let mut detail = (String::new(), String::new(), 0);
    let mut keep = |d: RawError| detail = unsafe { (d.what.as_str().to_owned(), d.owner.as_str().to_owned(), d.limit) };
    let Some(code) = call(Sink::new(&mut keep)) else {
        return Err(Error::Unsupported { what: what.to_owned() });
    };
    let (what, owner, limit) = detail;
    match code {
        c if c == Status::Ok as u32 => Ok(()),
        c if c == Status::NoSource as u32 => Err(Error::NoSource),
        c if c == Status::Taken as u32 => Err(Error::Taken { what, owner }),
        c if c == Status::Full as u32 => Err(Error::Full { what, limit }),
        _ => Err(Error::Unsupported { what }),
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
    /// The Rust-ABI registrations not converted yet; removed with `BUILD` in stage 5.
    pub(crate) legacy: *const Legacy,
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
            sink.put(RawError { what: Str::from("thing"), owner: Str::from("other"), limit: 4 });
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
        assert_eq!(fail(&taken, &Sink::new(&mut keep)), Status::Taken as u32);
        assert_eq!(back, [("thing".to_owned(), "other".to_owned(), 0)]);
    }
}
