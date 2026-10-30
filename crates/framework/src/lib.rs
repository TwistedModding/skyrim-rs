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
//! `load` runs), its [`settings`], its hotkeys ([`Host::action`]), and every registration it makes
//! in the game: loader messages, a per-frame callback, the Papyrus registration pass, a tab in the
//! host's window ([`Host::add_tab`]), HUD painters drawn into the game ([`Host::add_hud`]), event sinks, Papyrus natives, console commands and hook
//! subscriptions ([`Host::hooks`]). The host owns the object the engine keeps for each of
//! them and knows which plugin it belongs to, so the plugin's DLL can be unloaded and replaced
//! while the game runs: the host loads every plugin from a shadow copy, watches the author's file
//! and swaps generations at a frame boundary (`docs/subsystems/reload.md`). A plugin calls
//! `skyrim::init` with [`Host::skse`] before using the game layer and never installs the overlay
//! itself.

use core::ffi::{CStr, c_char};
use std::fmt;

pub use overlay::{GameMouse, GamePad, Hotkey, Scope, View, egui, theme};
use skyrim::events::{BSTEventSource, ErasedHandler, ErasedSource, Event};
use sre::{BSAnimationGraphEvent, BSEventNotifyControl, InputDevice};
use skyrim::input::{Command, CommandSlot, ErasedExecute, Param};
use skyrim::papyrus::{ErasedNativeFn, IntoVariable, LatentFn, NativeCall, NativeFn, NativeFunction, NativeKind, NativeMethod, ParamType, VirtualMachine};

pub mod hooks;
pub mod host;
pub mod order;
pub mod settings;

use settings::{Setting, Settings};

/// The descriptor's layout version.
pub const ABI: u32 = 4;

/// This crate's version, a hash of the sources whose types cross the host-plugin boundary (this
/// crate, `overlay`, `skyrim`, `hook`, `skse`, `sre`, `versionlib` and `Cargo.lock`) and the
/// compiler; a plugin is loaded only when its copy equals the host's.
pub const BUILD: &CStr = match CStr::from_bytes_with_nul(concat!("framework ", env!("CARGO_PKG_VERSION"), " ", env!("SKYRIM_RS_API_HASH"), ", ", env!("SKYRIM_RS_RUSTC"), "\0").as_bytes()) {
    Ok(build) => build,
    Err(_) => panic!("BUILD holds one nul, at the end"),
};

pub type LoadFn = fn(&'static Host) -> bool;
/// Receives every message the loader sends as sender "SKSE" (`skse::MessagingInterface` kinds).
pub type MessageFn = fn(&skse::Message);
/// Receives the VM at the loader's Papyrus registration pass.
pub type PapyrusFn = fn(&'static VirtualMachine);
/// Runs once a frame on the main thread.
pub type FrameFn = fn();
/// Draws one section of a plugin's tab in the host's window.
pub type DrawFn = Box<dyn FnMut(&mut egui::Ui) + Send>;
/// Paints a plugin's shapes into the game, placed through the frame camera's [`View`].
pub type HudDrawFn = Box<dyn FnMut(&egui::Painter, &View) + Send>;

/// What [`plugin!`] exports as `SkyrimRs_Plugin`. The host reads `abi` and `build` before anything
/// else, so they stay first and C-typed in every future ABI.
#[repr(C)]
pub struct Descriptor {
    pub abi: u32,
    pub build: *const c_char,
    pub name: *const c_char,
    /// The plugin crate's version, as the Plugins tab shows it.
    pub version: *const c_char,
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
    pub version: String,
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

pub(crate) type DeclareFilterFn = fn(&str, &str, &[&str], &[(InputDevice, u32)]) -> usize;

/// The host's functions, one table shared by every plugin's [`Host`]; the owner comes first.
pub(crate) struct Table {
    pub on_message: fn(&'static str, MessageFn),
    pub on_plugin_message: fn(&'static str, &str, MessageFn),
    pub on_frame: fn(&'static str, FrameFn),
    pub on_papyrus: fn(&'static str, PapyrusFn),
    pub action: fn(&'static str, &str, &str, Scope) -> usize,
    pub action_fired: fn(usize) -> bool,
    pub action_key: fn(usize) -> Option<Hotkey>,
    pub action_held: fn(usize) -> bool,
    pub action_released: fn(usize) -> Option<f32>,
    pub key_held: fn(u32) -> bool,
    pub key_released: fn(u32) -> Option<f32>,
    pub game_mouse: fn() -> GameMouse,
    pub game_pad: fn() -> GamePad,
    pub input_filter: DeclareFilterFn,
    pub filter_set_active: fn(usize, bool),
    pub filter_active: fn(usize) -> bool,
    pub set_tab_title: fn(&'static str, &str),
    pub add_section: fn(&'static str, &str, DrawFn),
    pub add_hud: fn(&'static str, HudDrawFn),
    pub show_tab: fn(&'static str),
    pub setting: fn(&str, &str) -> Option<Setting>,
    pub set_setting: fn(&str, &str, Setting),
    pub setting_keys: fn(&str, &str) -> Vec<String>,
    pub remove_settings: fn(&str, &str) -> usize,
    pub add_sink: fn(&'static str, ErasedSource, ErasedHandler),
    pub add_player_graph_sink: fn(&'static str, ErasedHandler),
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

    /// Calls `f` with every message the SKSE plugin named `sender` dispatches from now on (the
    /// loader matches the name case-insensitively): another plugin's API handed out as a message,
    /// on the dispatching thread. Messages sent before the call are not replayed, so a reloaded
    /// plugin keeps what an earlier one received through its state handoff.
    pub fn on_plugin_message(&self, sender: &str, f: MessageFn) {
        (self.table.on_plugin_message)(self.name, sender, f);
    }

    /// Calls `f` once a frame on the main thread, at the frame's present whether the overlay is
    /// open or not, before the overlay's windows draw: the place to poll [`Action::fired`].
    pub fn on_frame(&self, f: FrameFn) {
        (self.table.on_frame)(self.name, f);
    }

    /// Declares a hotkey named `<plugin>.<name>` in the host's binding table, bound to `default`
    /// (`"F10"`, `"Ctrl+Shift+K"`, as [`Hotkey`] reads it; `""` for none) until the player binds
    /// another key in the host file (`key.<plugin>.<name>`) or the host window's Settings tab. `scope` says
    /// who must have the key: [`Scope::Overlay`] fires while the overlay is open with no widget
    /// focused and keeps the key from the game while it is open; [`Scope::Game`] fires when the
    /// press reached the game, which acts on it too. A key another action holds is refused, and
    /// the action stays unbound until one is set. A later generation of the plugin declaring the
    /// same name gets the same action and binding.
    ///
    /// ```ignore
    /// let toggle = host.action("toggle-window", "Shift+K", framework::Scope::Overlay);
    /// host.on_frame(|| if TOGGLE.get().is_some_and(|a| a.fired()) { /* ... */ });
    /// ```
    #[must_use]
    pub fn action(&self, name: &str, default: &str, scope: Scope) -> Action {
        Action { index: (self.table.action)(self.name, name, default, scope), table: self.table }
    }

    /// What the mouse did in the events that reached the game since the previous present, from the
    /// start of the present after them to the start of the next (so once per frame for an
    /// [`on_frame`](Self::on_frame) callback): the summed movement and the button presses and
    /// releases in order, the wheel's steps as buttons 8 (up) and 9 (down). Events the overlay kept
    /// from the game are not in it. For a drag or an orbit a mod reads while the game has the mouse.
    #[must_use]
    pub fn mouse(&self) -> GameMouse {
        (self.table.game_mouse)()
    }

    /// What the gamepad did in the events that reached the game, in the same window as
    /// [`mouse`](Self::mouse): the button presses and releases in order (`XInput` masks, see
    /// [`GamePad`]), and the buttons held, the triggers and the sticks as the last event left them.
    #[must_use]
    pub fn gamepad(&self) -> GamePad {
        (self.table.game_pad)()
    }

    /// Whether the keyboard key with this `DirectInput` scan code is down in the game (its press
    /// reached the game and its release has not yet): for a modifier a mod's own keys combine with.
    #[must_use]
    pub fn key_held(&self, scan_code: u32) -> bool {
        (self.table.key_held)(scan_code)
    }

    /// The seconds the keyboard key with this scan code had been held when its release reached the
    /// game, reported for one frame like [`mouse`](Self::mouse); `None` when it was not released.
    /// For a tap-or-hold key, or a key the game's control map names (`ControlMapExt::mapped_key`).
    #[must_use]
    pub fn key_released(&self, scan_code: u32) -> Option<f32> {
        (self.table.key_released)(scan_code)
    }

    /// Declares the input filter `<plugin>.<name>`: while it is active, button events whose user
    /// event is one of `user_events` (as the control map names them: `"Favorites"`, `"Quick Map"`;
    /// any case) or whose device and key are in `buttons` (`(InputDevice::Gamepad, 0x100)`) reach
    /// the game with their user event blanked, so `MenuControls`, `PlayerControls` and every
    /// handler that reads the user event passes them over; the host's actions and the sinks that
    /// read the key still see them. It starts inactive; the plugin switches it with
    /// [`InputFilter::set_active`] whenever its condition changes (from [`on_frame`](Self::on_frame)
    /// or a hook). Filters only take events away, so several plugins' filters need no order. A
    /// later generation of the plugin declaring the same name gets the same filter, inactive.
    ///
    /// ```ignore
    /// let menus = host.input_filter("no-menus", &["Favorites", "Journal"], &[]);
    /// host.on_frame(|| FILTER.get().unwrap().set_active(in_free_camera()));
    /// ```
    #[must_use]
    pub fn input_filter(&self, name: &str, user_events: &[&str], buttons: &[(InputDevice, u32)]) -> InputFilter {
        InputFilter { index: (self.table.input_filter)(self.name, name, user_events, buttons), table: self.table }
    }

    /// Calls `f` with the VM at the Papyrus registration pass, or at once when the pass has run
    /// (a reloaded plugin registers its natives again from here).
    pub fn on_papyrus(&self, f: PapyrusFn) {
        (self.table.on_papyrus)(self.name, f);
    }

    /// This plugin's tab in the host's window, titled `title` (a second call renames it): an entry
    /// in the sidebar of the window's Mods page, in load order, shown once it has a section; with
    /// two or more, the sections are listed under it while it is selected. While the overlay is
    /// open, the selected section's `draw` runs every frame on the main thread, in a scroll area
    /// beside the sidebar, in the framework theme:
    ///
    /// ```ignore
    /// host.add_tab("My mod")
    ///     .section("General", |ui| { ui.label("hello"); })
    ///     .section("About", |ui| { ui.label("version 1"); });
    /// ```
    ///
    /// The host keeps the tab and drops it with the plugin's generation; a reloaded generation
    /// adding the same section names finds the same one selected.
    #[must_use = "a tab shows once it has a section"]
    pub fn add_tab(&self, title: &str) -> Tab {
        (self.table.set_tab_title)(self.name, title);
        Tab { owner: self.name, table: self.table }
    }

    /// Adds a HUD painter: `draw` runs every frame on the main thread at the present, whether the
    /// overlay is open or not, once the scene graph exists, and what it paints lands in the frame
    /// under the host's window. [`View::project`] places a world point on screen and
    /// [`View::project_bound`] boxes an object's 3D, an actor's root for a highlight:
    ///
    /// ```ignore
    /// host.add_hud(|painter, view| {
    ///     let Some(root) = sre::PlayerCharacter::singleton().and_then(|p| p.loaded_data()?.node3_d()) else { return };
    ///     if let Some(rect) = view.project_bound(root) {
    ///         painter.rect_stroke(rect, 0.0, (2.0, egui::Color32::GOLD), egui::StrokeKind::Outside);
    ///     }
    /// });
    /// ```
    ///
    /// The painters are dropped with the plugin's generation. The game's own menus are not
    /// considered: a painter that should hide under the inventory or the map checks for them.
    pub fn add_hud(&self, draw: impl FnMut(&egui::Painter, &View) + Send + 'static) {
        (self.table.add_hud)(self.name, Box::new(draw));
    }

    /// Opens the overlay on this plugin's tab.
    pub fn show_tab(&self) {
        (self.table.show_tab)(self.name);
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

    /// Registers `f` for the player's behaviour graph events (`BSAnimationGraphEvent`: the tag, the
    /// player as holder, the payload) through a sink the host owns. The graph's source dies with
    /// the player's 3D, so the host registers its sink on the current source every frame; the
    /// events between a 3D rebuild and the next frame are missed. `f` runs on the sending thread,
    /// often a job thread, with the source lock held.
    pub fn add_player_graph_sink(&self, f: impl Fn(&BSAnimationGraphEvent) -> BSEventNotifyControl + Send + Sync + 'static) {
        let handler: ErasedHandler = Box::new(move |event, _| match unsafe { event.cast::<BSAnimationGraphEvent>().as_ref() } {
            Some(event) => f(event),
            None => BSEventNotifyControl::Continue,
        });
        (self.table.add_player_graph_sink)(self.name, handler);
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

/// A plugin's tab from [`Host::add_tab`].
#[derive(Clone, Copy)]
pub struct Tab {
    owner: &'static str,
    table: &'static Table,
}

impl Tab {
    /// Adds a section named `name`, unique within the tab, after those added before.
    pub fn section(&self, name: &str, draw: impl FnMut(&mut egui::Ui) + Send + 'static) -> &Self {
        (self.table.add_section)(self.owner, name, Box::new(draw));
        self
    }
}

/// A hotkey from [`Host::action`].
#[derive(Clone, Copy)]
pub struct Action {
    index: usize,
    table: &'static Table,
}

impl Action {
    /// Whether the key was pressed while the declared side had it since the previous frame: true
    /// from the start of the next present (its [`Host::on_frame`] callbacks and windows) to the
    /// start of the one after, so once per press for a frame callback.
    #[must_use]
    pub fn fired(&self) -> bool {
        (self.table.action_fired)(self.index)
    }

    /// Whether the bound key and its modifiers are down in the game now: for an action that acts
    /// every frame while its key is held. False while the action is unbound.
    #[must_use]
    pub fn held(&self) -> bool {
        (self.table.action_held)(self.index)
    }

    /// The seconds the bound key had been held when its release reached the game, for the frame
    /// after the release (as [`fired`](Self::fired) for the press); the modifiers may already be up.
    /// `None` while the key is not released or the action is unbound.
    #[must_use]
    pub fn released(&self) -> Option<f32> {
        (self.table.action_released)(self.index)
    }

    /// The key bound now; `None` while a conflict leaves the action unbound.
    #[must_use]
    pub fn key(&self) -> Option<Hotkey> {
        (self.table.action_key)(self.index)
    }
}

/// An input filter from [`Host::input_filter`].
#[derive(Clone, Copy)]
pub struct InputFilter {
    index: usize,
    table: &'static Table,
}

impl InputFilter {
    /// Starts or stops taking the filter's events from the game, from the next input frame.
    pub fn set_active(&self, active: bool) {
        (self.table.filter_set_active)(self.index, active);
    }

    #[must_use]
    pub fn active(&self) -> bool {
        (self.table.filter_active)(self.index)
    }
}

/// Declares a framework plugin: exports its [`Descriptor`], with the version of the crate it is
/// invoked in. Everything after `load` is optional,
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
            version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
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
