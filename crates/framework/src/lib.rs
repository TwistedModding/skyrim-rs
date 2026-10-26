//! The framework's own plugin ABI: plugins the host (`skyrim_rs.dll`, an SKSE plugin) loads itself.
//!
//! A framework plugin is a `cdylib` in `Data\SKSE\Plugins\skyrim-rs\` (SKSE loads only the folder
//! above, so it never sees them) that declares itself with [`plugin!`]. The host loads every DLL
//! there from its own `SKSEPlugin_Load`, reads the exported `SkyrimRs_Plugin` [`Descriptor`] as
//! plain C data and calls the plugin's `load` only when the descriptor's [`ABI`] and [`BUILD`]
//! equal its own: that call and everything passed through [`Host`] use the Rust ABI, which holds
//! only for one compiler and one build of this crate and its dependencies, so a plugin is built
//! against the same framework release (its `Cargo.lock`) with the same `rustc`. A refused DLL is
//! unloaded and named in `skyrim-rs.log`.
//!
//! The `load` calls run in the order [`order::sort`] resolves from every accepted descriptor's
//! `load_before`, `load_after` and `priority`, file name order deciding what they leave open; a
//! plugin in a constraint cycle is refused. The order is read back from [`Host::plugins`].
//!
//! What a plugin gets from its [`Host`]: the SKSE interface the host was loaded with, forwarded
//! (`QueryInterface` answers as for the host, and the plugin handle is the host's, valid only while
//! `load` runs), its [`settings`], and every registration it makes in the game: loader messages, the
//! Papyrus registration pass, windows on the host's overlay, event sinks, Papyrus natives, console
//! commands and hook subscriptions ([`Host::hooks`]). The host owns the object the engine keeps for each of
//! them and knows which plugin it belongs to, so the plugin's DLL can be unloaded and replaced
//! while the game runs: the host loads every plugin from a shadow copy, watches the author's file
//! and swaps generations at a frame boundary (`docs/subsystems/reload.md`). A plugin calls
//! `skyrim::init` with [`Host::skse`] before using the game layer and never installs the overlay
//! itself.

use core::ffi::{CStr, c_char};
use std::fmt;

pub use overlay::{egui, theme};
use skyrim::events::{BSEventNotifyControl, BSTEventSource, ErasedHandler, ErasedSource, Event};
use skyrim::input::{Command, CommandSlot, ErasedExecute, Param};
use skyrim::papyrus::{ErasedNativeFn, IntoVariable, LatentFn, NativeCall, NativeFn, NativeFunction, NativeKind, NativeMethod, ParamType, VirtualMachine};

pub mod hooks;
pub mod host;
pub mod order;
pub mod settings;

use settings::{Setting, Settings};

/// The descriptor's layout version.
pub const ABI: u32 = 3;

/// This crate's version and the compiler it was built with; a plugin is loaded only when its copy
/// equals the host's.
pub const BUILD: &CStr = match CStr::from_bytes_with_nul(concat!("framework ", env!("CARGO_PKG_VERSION"), ", ", env!("SKYRIM_RS_RUSTC"), "\0").as_bytes()) {
    Ok(build) => build,
    Err(_) => panic!("BUILD holds one nul, at the end"),
};

pub type LoadFn = fn(&'static Host) -> bool;
/// Receives every message the loader sends as sender "SKSE" (`skse::MessagingInterface` kinds).
pub type MessageFn = fn(&skse::Message);
/// Receives the VM at the loader's Papyrus registration pass.
pub type PapyrusFn = fn(&'static VirtualMachine);
/// Draws one window: gets the `egui::Window` the host set up and shows it.
pub type DrawFn = Box<dyn for<'a> FnMut(egui::Window<'a>, &egui::Context) + Send>;

/// What [`plugin!`] exports as `SkyrimRs_Plugin`. The host reads `abi` and `build` before anything
/// else, so they stay first and C-typed in every future ABI.
#[repr(C)]
pub struct Descriptor {
    pub abi: u32,
    pub build: *const c_char,
    pub name: *const c_char,
    pub load: LoadFn,
    /// Plugin names this one loads before; [`order::ALL`] for every other plugin.
    pub load_before: &'static [&'static str],
    /// Plugin names this one loads after; [`order::ALL`] for every other plugin.
    pub load_after: &'static [&'static str],
    /// Higher loads first among plugins whose constraints are satisfied.
    pub priority: i32,
    /// Runs before the DLL is unloaded for a reload, after `save_state`: stop threads and timers here.
    pub unload: Option<fn()>,
    /// The state handed to the version that replaces this one, which receives it in `restore_state` after its `load`.
    pub save_state: Option<fn() -> Vec<u8>>,
    pub restore_state: Option<fn(&[u8])>,
}

unsafe impl Sync for Descriptor {}

/// A plugin the host accepted, as [`Host::plugins`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub file: String,
    pub constraints: order::Constraints,
}

/// Why the host refused a registration.
#[derive(Debug)]
pub enum Error {
    /// The event's source does not exist yet; its holder is created later in startup.
    NoSource,
    /// Another plugin registered the native or command of that name.
    Taken { what: String, owner: String },
    Papyrus(skyrim::papyrus::Error),
    Command(skyrim::input::CommandError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSource => f.write_str("the event source does not exist yet"),
            Self::Taken { what, owner } => write!(f, "{what} is registered by {owner}"),
            Self::Papyrus(e) => e.fmt(f),
            Self::Command(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<skyrim::papyrus::Error> for Error {
    fn from(e: skyrim::papyrus::Error) -> Self {
        Self::Papyrus(e)
    }
}

impl From<skyrim::input::CommandError> for Error {
    fn from(e: skyrim::input::CommandError) -> Self {
        Self::Command(e)
    }
}

/// A native as it crosses from a plugin to the host.
pub(crate) struct NativeRequest<'a> {
    pub vm: &'a VirtualMachine,
    pub class: &'a str,
    pub name: &'a str,
    pub kind: NativeKind,
    pub params: &'static [ParamType],
    pub ret: Option<ParamType>,
    pub f: ErasedNativeFn,
}

/// A console command as it crosses from a plugin to the host.
pub(crate) struct CommandRequest<'a> {
    pub name: &'a CStr,
    pub help: &'a CStr,
    pub params: &'a [Param],
    pub needs_reference: bool,
    pub execute: ErasedExecute,
}

/// The host's functions, one table shared by every plugin's [`Host`]; the owner comes first.
pub(crate) struct Table {
    pub on_message: fn(&'static str, MessageFn),
    pub on_papyrus: fn(&'static str, PapyrusFn),
    pub add_window: fn(&'static str, &str, &str, DrawFn),
    pub window_open: fn(&'static str, &str) -> bool,
    pub set_window_open: fn(&'static str, &str, bool),
    pub setting: fn(&str, &str) -> Option<Setting>,
    pub set_setting: fn(&str, &str, Setting),
    pub add_sink: fn(&'static str, ErasedSource, ErasedHandler),
    pub register_native: fn(&'static str, NativeRequest<'_>) -> Result<&'static NativeFunction, Error>,
    pub register_command: fn(&'static str, CommandRequest<'_>) -> Result<CommandSlot, Error>,
    pub plugins: fn() -> &'static [Plugin],
}

/// The host as one plugin's `load` receives it, for the life of the game: every registration made
/// through it belongs to that plugin and is detached when the plugin's DLL is unloaded.
pub struct Host {
    skse: &'static skse::Interface,
    name: &'static str,
    hooks: &'static hooks::Hooks,
    table: &'static Table,
}

impl Host {
    /// The SKSE interface the host was loaded with.
    #[must_use]
    pub fn skse(&self) -> &'static skse::Interface {
        self.skse
    }

    /// The plugin's name as its descriptor declares it, the owner its hook subscriptions name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Calls `f` with every loader message from now on, after the host's own handling. Registered
    /// after startup (by a reloaded plugin), `f` first receives the startup messages the host has
    /// seen (`PLUGINS_LOADED`, `PLUGINS_LOADED_2`, `CONTROLS_READY`, `DATA_LOADED`, without data)
    /// so setup done at `DATA_LOADED` is done again.
    pub fn on_message(&self, f: MessageFn) {
        (self.table.on_message)(self.name, f);
    }

    /// Calls `f` with the VM at the Papyrus registration pass, or at once when the pass has run
    /// (a reloaded plugin registers its natives again from here).
    pub fn on_papyrus(&self, f: PapyrusFn) {
        (self.table.on_papyrus)(self.name, f);
    }

    /// Adds a window to the host's overlay. `id` names it within the plugin: a later generation
    /// adding the same id gets the same place and open state. Every frame the overlay is open,
    /// `draw` runs on the main thread with an `egui::Window` the host has set up (the `title`, a
    /// close button bound to the window's open state, the framework theme) and shows it, after
    /// changing it through `egui`'s builder if it wants to (`default_pos`, `resizable`, `frame`
    /// for a look of its own):
    ///
    /// ```ignore
    /// host.add_window("main", "My mod", |window, ctx| {
    ///     window.show(ctx, |ui| ui.label("hello"));
    /// });
    /// ```
    pub fn add_window(&self, id: &str, title: &str, draw: impl for<'a> FnMut(egui::Window<'a>, &egui::Context) + Send + 'static) {
        (self.table.add_window)(self.name, id, title, Box::new(draw));
    }

    /// Whether the window `id` is open: shown while the overlay is. A window starts open, and
    /// the player closes it with its close button or reopens it from the host's window; the
    /// state is kept in the host's settings across reloads and game sessions.
    #[must_use]
    pub fn is_window_open(&self, id: &str) -> bool {
        (self.table.window_open)(self.name, id)
    }

    pub fn set_window_open(&self, id: &str, open: bool) {
        (self.table.set_window_open)(self.name, id, open);
    }

    /// This plugin's settings, read from its file before `load` and kept by the host across reloads.
    #[must_use]
    pub fn settings(&self) -> Settings {
        Settings::new(self.name, self.table)
    }

    /// Registers `f` for events of type `E` through a sink the host owns; `f` runs on the sending
    /// thread with the source lock held, as `skyrim::events::EventSink` does. Sinks of every plugin
    /// on one source run in registration order until one returns `Stop`.
    pub fn add_sink<E: Event>(&self, f: impl Fn(&E, &BSTEventSource<E>) -> BSEventNotifyControl + Send + Sync + 'static) -> Result<(), Error> {
        let source = ErasedSource::of::<E>().ok_or(Error::NoSource)?;
        let handler: ErasedHandler = Box::new(move |event, source| match (unsafe { event.cast::<E>().as_ref() }, unsafe { BSTEventSource::<E>::from_ptr(source) }) {
            (Some(event), Some(source)) => f(event, source),
            _ => BSEventNotifyControl::Continue,
        });
        (self.table.add_sink)(self.name, source, handler);
        Ok(())
    }

    /// Registers `f` as `class.name`, a `native global` function, through a function object the
    /// host owns (`VirtualMachine::register_native` otherwise). Registered again by a later
    /// generation of the plugin, the same `class.name` is re-pointed at the new `f`.
    pub fn register_native<F: NativeFn<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(NativeRequest { vm, class, name, kind: NativeKind::Global, params: F::PARAMS, ret: F::RETURN, f: Box::new(move |call: &NativeCall<'_>| call.global(f)) })
    }

    /// Registers `f` as `class.name`, a `native` method of a script extending the form class of
    /// its first parameter, as `VirtualMachine::register_method` does.
    pub fn register_method<F: NativeMethod<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(NativeRequest { vm, class, name, kind: NativeKind::Method, params: F::PARAMS, ret: F::RETURN, f: Box::new(move |call: &NativeCall<'_>| call.method(f)) })
    }

    /// Registers `f` as `class.name`, a latent `native global` function, as `VirtualMachine::register_latent` does.
    pub fn register_latent<F: LatentFn<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(NativeRequest { vm, class, name, kind: NativeKind::Latent, params: F::PARAMS, ret: <F::Output as IntoVariable>::TYPE, f: Box::new(move |call: &NativeCall<'_>| call.latent(f)) })
    }

    fn native(&self, request: NativeRequest<'_>) -> Result<&'static NativeFunction, Error> {
        (self.table.register_native)(self.name, request)
    }

    /// Takes over an unused console entry for `C` through the host (`skyrim::input::register`
    /// otherwise); register at `DATA_LOADED`. A later generation registering the same name is
    /// re-pointed at its `execute`; the name, help and parameters stay those of the first.
    pub fn register_command<C: Command>(&self) -> Result<CommandSlot, Error> {
        (self.table.register_command)(self.name, CommandRequest { name: C::NAME, help: C::HELP, params: C::PARAMS, needs_reference: C::NEEDS_REFERENCE, execute: Box::new(C::execute) })
    }

    /// Every plugin whose descriptor the host accepted, in load order, known before the first `load` runs.
    #[must_use]
    pub fn plugins(&self) -> &'static [Plugin] {
        (self.table.plugins)()
    }

    /// The host's hook dispatcher: one detour per engine function, shared by every plugin; this
    /// handle accepts subscriptions whose `Options` owner is this plugin's name.
    #[must_use]
    pub fn hooks(&self) -> &'static hooks::Hooks {
        self.hooks
    }
}

/// Declares a framework plugin: exports its [`Descriptor`]. Everything after `load` is optional,
/// in this order: the ordering fields (defaults: no constraints, priority 0) and the reload
/// functions (defaults: none).
///
/// ```ignore
/// framework::plugin! {
///     name: "my-mod",
///     load: load,
///     load_before: ["other-mod"],
///     load_after: ["*"],
///     priority: 1,
///     unload: unload,
///     save_state: save_state,
///     restore_state: restore_state,
/// }
/// fn load(host: &'static framework::Host) -> bool { skyrim::init(host.skse()).is_ok() }
/// fn unload() {}
/// fn save_state() -> Vec<u8> { Vec::new() }
/// fn restore_state(_: &[u8]) {}
/// ```
#[macro_export]
macro_rules! plugin {
    (name: $name:literal, load: $load:path $(, load_before: [$($before:literal),* $(,)?])? $(, load_after: [$($after:literal),* $(,)?])? $(, priority: $priority:expr)? $(, unload: $unload:path)? $(, save_state: $save:path)? $(, restore_state: $restore:path)? $(,)?) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static SkyrimRs_Plugin: $crate::Descriptor = $crate::Descriptor {
            abi: $crate::ABI,
            build: $crate::BUILD.as_ptr(),
            name: concat!($name, "\0").as_ptr().cast(),
            load: $load,
            load_before: &[$($($before),*)?],
            load_after: &[$($($after),*)?],
            priority: ($($priority,)? 0i32,).0,
            unload: ($(Some($unload as fn()),)? None::<fn()>,).0,
            save_state: ($(Some($save as fn() -> Vec<u8>),)? None::<fn() -> Vec<u8>>,).0,
            restore_state: ($(Some($restore as fn(&[u8])),)? None::<fn(&[u8])>,).0,
        };
    };
}
