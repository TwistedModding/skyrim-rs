//! The framework's own plugin ABI: plugins the host (`skyrim_rs.dll`, an SKSE plugin) loads itself.
//!
//! A framework plugin is a `cdylib` in `Data\SKSE\Plugins\skyrim-rs\` (SKSE loads only the folder
//! above, so it never sees them) that declares itself with [`plugin!`]. The host loads every DLL
//! there from its own `SKSEPlugin_Load`, reads the exported `SkyrimRs_Plugin` [`Descriptor`] as
//! plain C data and calls the plugin's `load` when the descriptor's [`ABI`] is one it knows. The
//! descriptor, the per-plugin [`abi::HostApi`] behind [`Host`] and every record they carry are C
//! (`docs/subsystems/abi.md`), and a plugin draws its UI with `egui` contexts of its own and hands
//! the host meshes ([`ui`]), so a plugin runs on any loader that has the functions it uses, whatever
//! compiler and framework commit either was built with. A refused DLL is unloaded and named in
//! `skyrim-rs.log`.
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

use core::cell::RefCell;
use core::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::sync::{Mutex, PoisonError};

pub use overlay::{Frame, GameMouse, GamePad, Hotkey, Scope, View, egui, theme};
use skyrim::events::{BSTEventSource, ErasedSource, Event};
use sre::{BSAnimationGraphEvent, BSEventNotifyControl, InputDevice};
use skyrim::input::{Command, CommandContext, CommandSlot, Param};
use skyrim::papyrus::{IntoVariable, LatentFn, NativeCall, NativeFn, NativeFunction, NativeKind, NativeMethod, ParamType, Variable, VirtualMachine};

pub mod abi;
pub mod hooks;
pub mod host;
pub mod order;
pub mod settings;
pub mod ui;

use abi::{Api, HostApi, PluginRecord, RawButton, RawCommandRequest, RawCommandSlot, RawMouse, RawNativeRequest, RawPad, RawParamType, RawPress, RawRestoreFn, RawSaveFn, Sink, Str, call};
use settings::Settings;

/// The descriptor's layout version.
pub const ABI: u32 = 5;

pub type LoadFn = fn(&'static Host) -> bool;
/// Receives every message the loader sends as sender "SKSE" (`skse::MessagingInterface` kinds).
pub type MessageFn = fn(&skse::Message);
/// Receives the VM at the loader's Papyrus registration pass.
pub type PapyrusFn = fn(&'static VirtualMachine);
/// Runs once a frame on the main thread.
pub type FrameFn = fn();

/// What [`plugin!`] exports as `SkyrimRs_Plugin`, all C: the host reads `abi` before anything
/// else, so it stays first in every future layout.
#[repr(C)]
pub struct Descriptor {
    pub abi: u32,
    /// The [`abi::API`] the plugin was built against.
    pub api: u32,
    /// Null. A plugin of stages 1 to 4 of ABI 5 carries its build string here and is refused.
    pub build: *const c_char,
    pub name: *const c_char,
    /// The plugin crate's version, as the Plugins tab shows it.
    pub version: *const c_char,
    pub load: abi::RawLoadFn,
    /// Plugin names this one loads before, null-terminated; [`order::ALL`] for every other plugin.
    pub load_before: *const *const c_char,
    /// Plugin names this one loads after, null-terminated; [`order::ALL`] for every other plugin.
    pub load_after: *const *const c_char,
    /// Higher loads first among plugins whose constraints are satisfied.
    pub priority: i32,
    /// Runs before the DLL is unloaded for a reload, after `save_state`: stop threads and timers here.
    pub unload: Option<extern "C" fn()>,
    /// Writes the state handed to the version that replaces this one, which receives it in `restore_state` after its `load`.
    pub save_state: Option<RawSaveFn>,
    pub restore_state: Option<RawRestoreFn>,
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
    /// The host holds no more than `limit` of these.
    Full { what: String, limit: usize },
    Papyrus(skyrim::papyrus::Error),
    Command(skyrim::input::CommandError),
    /// The loader is older than the plugin and lacks the function.
    Unsupported { what: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSource => f.write_str("the event source does not exist yet"),
            Self::Taken { what, owner } => write!(f, "{what} is registered by {owner}"),
            Self::Full { what, limit } => write!(f, "{what}: the host holds at most {limit}"),
            Self::Papyrus(e) => e.fmt(f),
            Self::Command(e) => e.fmt(f),
            Self::Unsupported { what } => write!(f, "{what}: not supported by this loader"),
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

/// A native's declaration, as [`abi::RawNativeRequest`] carries it.
pub(crate) struct NativeRequest<'a> {
    pub vm: &'a VirtualMachine,
    pub class: &'a str,
    pub name: &'a str,
    pub kind: NativeKind,
    pub params: &'a [ParamType],
    pub ret: Option<ParamType>,
}

/// A console command's declaration, as [`abi::RawCommandRequest`] carries it.
pub(crate) struct CommandRequest<'a> {
    pub name: &'a CStr,
    pub help: &'a CStr,
    pub params: &'a [Param],
    pub needs_reference: bool,
}

/// The host as one plugin's `load` receives it, for the life of the DLL: every registration made
/// through it belongs to that plugin and is detached when the plugin's DLL is unloaded. On a
/// loader older than the plugin a function the loader lacks does nothing and answers `false`,
/// `None` or nothing; the fallible ones return [`Error::Unsupported`].
pub struct Host {
    api: Api,
    name: &'static str,
    skse: &'static skse::Interface,
    hooks: &'static hooks::Hooks,
}

impl Host {
    /// The plugin's side of the table the host built for it; `None` without the SKSE interface.
    pub(crate) fn new(api: *const HostApi) -> Option<Self> {
        let api = Api(api);
        if api.0.is_null() {
            return None;
        }
        let name = unsafe { (*api.0).name.as_str() };
        let skse = unsafe { (*api.0).skse.as_ref() }?;
        Some(Self { api, name, skse, hooks: Box::leak(Box::new(hooks::Hooks::new(api.0))) })
    }

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
        call!(self.api, on_message(abi::message(f)));
    }

    /// Calls `f` with every message the SKSE plugin named `sender` dispatches from now on (the
    /// loader matches the name case-insensitively): another plugin's API handed out as a message,
    /// on the dispatching thread. Messages sent before the call are not replayed, so a reloaded
    /// plugin keeps what an earlier one received through its state handoff.
    pub fn on_plugin_message(&self, sender: &str, f: MessageFn) {
        call!(self.api, on_plugin_message(Str::from(sender), abi::message(f)));
    }

    /// Serves the SKSE plugin name `name` for other SKSE plugins, as if this plugin were the SKSE
    /// plugin of that name: `f` receives on the dispatching thread what they dispatch to `name`
    /// (and, as for any listener to every sender, their broadcasts), and [`Alias::send`] answers
    /// with `name` as the sender. The name needs its stub in `Data\SKSE\Plugins` (a DLL built with
    /// `skse::alias_stub!`) and SKSE 2.3.1: once every SKSE plugin's `Load` has run the host looks
    /// the stub's messaging index up, checks the loader really names that index `name`, and logs
    /// that it serves the name or why not (no stub, a real plugin of that name installed beside
    /// it, the loader's names shifted by a plugin that failed to load). A message sent to `name`
    /// before then is lost, so consumers that ask at `PLUGINS_LOADED_2` or later are served. A
    /// later generation of the plugin serving the same name gets the same alias; a name another
    /// plugin serves is refused.
    pub fn serve_alias(&self, name: &str, f: MessageFn) -> Result<Alias, Error> {
        let mut slot = 0;
        abi::status(&format!("serving the plugin name {name}"), |error| call!(self.api, serve_alias(Str::from(name), abi::message(f), &raw mut slot, error)))?;
        Ok(Alias { slot, api: self.api })
    }

    /// Exports a C++ interface object (single inheritance, no data members) for other SKSE plugins
    /// and returns its address: `methods` are its virtual table in declaration order, each an
    /// `extern "C"` function taking the object first (MSVC x64 passes `this` that way). The object
    /// and the table belong to the host and each slot jumps through an address it keeps: while
    /// this plugin is unloaded, every method answers with its [`Returns`] stand-in instead of
    /// reaching the freed DLL, and a later generation exporting the same name gets the same
    /// object, re-pointed at its functions (the method count stays the first export's).
    ///
    /// ```ignore
    /// extern "C" fn version(_this: *const c_void) -> u32 { 2 }
    /// let api = host.export_interface("MyModAPI", &[framework::Method::new(version as *const (), framework::Returns::Value)])?;
    /// ```
    pub fn export_interface(&self, name: &str, methods: &[Method]) -> Result<*const c_void, Error> {
        let mut object = 0;
        abi::status(&format!("interface {name}"), |error| call!(self.api, export_interface(Str::from(name), methods.as_ptr(), methods.len(), &raw mut object, error)))?;
        Ok(object as *const c_void)
    }

    /// Calls `f` once a frame on the main thread, at the frame's present whether the overlay is
    /// open or not, before the overlay's windows draw: the place to poll [`Action::fired`].
    pub fn on_frame(&self, f: FrameFn) {
        call!(self.api, on_frame(abi::frame(f)));
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
        Action { index: call!(self.api, action(Str::from(name), Str::from(default), scope as u32)).unwrap_or(usize::MAX), api: self.api }
    }

    /// What the mouse did in the events that reached the game since the previous present, from the
    /// start of the present after them to the start of the next (so once per frame for an
    /// [`on_frame`](Self::on_frame) callback): the summed movement and the button presses and
    /// releases in order, the wheel's steps as buttons 8 (up) and 9 (down). Events the overlay kept
    /// from the game are not in it. For a drag or an orbit a mod reads while the game has the mouse.
    #[must_use]
    pub fn mouse(&self) -> GameMouse {
        let mut moved = RawMouse::default();
        let mut buttons = Vec::new();
        let mut push = |p: RawPress| buttons.push((p.id, p.pressed));
        call!(self.api, mouse(&raw mut moved, Sink::new(&mut push)));
        GameMouse { dx: moved.dx, dy: moved.dy, buttons }
    }

    /// What the gamepad did in the events that reached the game, in the same window as
    /// [`mouse`](Self::mouse): the button presses and releases in order (`XInput` masks, see
    /// [`GamePad`]), and the buttons held, the triggers and the sticks as the last event left them.
    #[must_use]
    pub fn gamepad(&self) -> GamePad {
        let mut pad = RawPad::default();
        let mut buttons = Vec::new();
        let mut push = |p: RawPress| buttons.push((p.id, p.pressed));
        call!(self.api, gamepad(&raw mut pad, Sink::new(&mut push)));
        GamePad { buttons, held: pad.held, triggers: pad.triggers, sticks: pad.sticks }
    }

    /// Whether the keyboard key with this `DirectInput` scan code is down in the game (its press
    /// reached the game and its release has not yet): for a modifier a mod's own keys combine with.
    #[must_use]
    pub fn key_held(&self, scan_code: u32) -> bool {
        call!(self.api, key_held(scan_code)).unwrap_or(false)
    }

    /// The seconds the keyboard key with this scan code had been held when its release reached the
    /// game, reported for one frame like [`mouse`](Self::mouse); `None` when it was not released.
    /// For a tap-or-hold key, or a key the game's control map names (`ControlMapExt::mapped_key`).
    #[must_use]
    pub fn key_released(&self, scan_code: u32) -> Option<f32> {
        let mut secs = 0.0;
        call!(self.api, key_released(scan_code, &raw mut secs)).unwrap_or(false).then_some(secs)
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
        let events: Vec<Str> = user_events.iter().map(|e| Str::from(*e)).collect();
        let buttons: Vec<RawButton> = buttons.iter().map(|&(device, id)| RawButton { device: device as u32, id }).collect();
        let index = call!(self.api, input_filter(Str::from(name), events.as_ptr(), events.len(), buttons.as_ptr(), buttons.len())).unwrap_or(usize::MAX);
        InputFilter { index, api: self.api }
    }

    /// Calls `f` with the VM at the Papyrus registration pass, or at once when the pass has run
    /// (a reloaded plugin registers its natives again from here).
    pub fn on_papyrus(&self, f: PapyrusFn) {
        call!(self.api, on_papyrus(abi::papyrus(f)));
    }

    /// This plugin's tab in the host's window, titled `title` (a second call renames it): an entry
    /// in the sidebar of the window's Mods page, in load order, shown once it has a section; with
    /// two or more, the sections are listed under it while it is selected. While the overlay is
    /// open, the selected section's `draw` runs every frame on the main thread, in a scroll area
    /// beside the sidebar, in the framework theme, with an `egui` context of the plugin's own that
    /// receives the input over the section, its popups and drags, and over no window of the host:
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
        call!(self.api, set_tab_title(Str::from(title)));
        Tab { api: self.api }
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
    /// considered: a painter that should hide under the inventory or the map checks for them. On
    /// a loader without HUD painters this does nothing.
    pub fn add_hud(&self, draw: impl FnMut(&egui::Painter, &View) + Send + 'static) {
        ui::add_hud(self.api, draw);
    }

    /// Opens the overlay on this plugin's tab.
    pub fn show_tab(&self) {
        call!(self.api, show_tab());
    }

    /// Opens the overlay on the section `name` of this plugin's tab.
    pub fn show_section(&self, name: &str) {
        call!(self.api, show_section(Str::from(name)));
    }

    /// The frame the game drew, as it will be presented without the overlay's windows and HUD
    /// painters: call it from an [`on_frame`](Self::on_frame) callback, which runs at the present
    /// before the overlay draws. The pixels are RGBA rows; a back buffer that is not the game's
    /// `R8G8B8A8_UNORM`, or is multisampled, is refused with its format named. A mod hides the game's own HUD
    /// a couple of frames before (the UI's `menusVisible` byte, which Scaleform acts on at its next
    /// render) when the shot should not show it.
    ///
    /// ```ignore
    /// host.on_frame(|| if SHOOT.get().is_some_and(|a| a.fired()) {
    ///     let written = HOST.get().unwrap().read_frame().and_then(|f| f.write_png(&path).map_err(|e| e.to_string()));
    /// });
    /// ```
    pub fn read_frame(&self) -> Result<Frame, String> {
        let mut frame = Frame { width: 0, height: 0, rgba: Vec::new() };
        let mut error = String::new();
        let mut alloc = |width, height| {
            frame = Frame { width, height, rgba: vec![0; width * height * 4] };
            frame.rgba.as_mut_ptr()
        };
        let mut fail = |e: Str| unsafe { e.as_str() }.clone_into(&mut error);
        match call!(self.api, read_frame(abi::Alloc::new(&mut alloc), Sink::new(&mut fail))) {
            Some(true) => Ok(frame),
            Some(false) => Err(error),
            None => Err("reading the frame is not supported by this loader".to_owned()),
        }
    }

    /// This plugin's settings, read from its file before `load` and kept by the host across reloads.
    #[must_use]
    pub fn settings(&self) -> Settings {
        Settings::new(self.api)
    }

    /// Registers `f` for events of type `E` through a sink the host owns; `f` runs on the sending
    /// thread with the source lock held, as `skyrim::events::EventSink` does. Sinks of every plugin
    /// on one source run in registration order until one returns `Stop`.
    pub fn add_sink<E: Event>(&self, f: impl Fn(&E, &BSTEventSource<E>) -> BSEventNotifyControl + Send + Sync + 'static) -> Result<(), Error> {
        let source = ErasedSource::of::<E>().ok_or(Error::NoSource)?;
        let handler = move |event: *const c_void, source| match (unsafe { event.cast::<E>().as_ref() }, unsafe { BSTEventSource::<E>::from_ptr(source) }) {
            (Some(event), Some(source)) => f(event, source),
            _ => BSEventNotifyControl::Continue,
        };
        call!(self.api, add_sink(source, abi::sink(handler))).ok_or_else(|| Error::Unsupported { what: "event sinks".to_owned() })
    }

    /// Registers `f` for the player's behaviour graph events (`BSAnimationGraphEvent`: the tag, the
    /// player as holder, the payload) through a sink the host owns. The graph's source dies with
    /// the player's 3D, so the host registers its sink on the current source every frame; the
    /// events between a 3D rebuild and the next frame are missed. `f` runs on the sending thread,
    /// often a job thread, with the source lock held.
    pub fn add_player_graph_sink(&self, f: impl Fn(&BSAnimationGraphEvent) -> BSEventNotifyControl + Send + Sync + 'static) {
        let handler = move |event: *const c_void, _| match unsafe { event.cast::<BSAnimationGraphEvent>().as_ref() } {
            Some(event) => f(event),
            None => BSEventNotifyControl::Continue,
        };
        call!(self.api, add_player_graph_sink(abi::sink(handler)));
    }

    /// Registers `f` as `class.name`, a `native global` function, through a function object the
    /// host owns (`VirtualMachine::register_native` otherwise). Registered again by a later
    /// generation of the plugin, the same `class.name` is re-pointed at the new `f`.
    pub fn register_native<F: NativeFn<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(&NativeRequest { vm, class, name, kind: NativeKind::Global, params: F::PARAMS, ret: F::RETURN }, move |call| call.global(f))
    }

    /// Registers `f` as `class.name`, a `native` method of a script extending the form class of
    /// its first parameter, as `VirtualMachine::register_method` does.
    pub fn register_method<F: NativeMethod<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(&NativeRequest { vm, class, name, kind: NativeKind::Method, params: F::PARAMS, ret: F::RETURN }, move |call| call.method(f))
    }

    /// Registers `f` as `class.name`, a latent `native global` function, as `VirtualMachine::register_latent` does.
    pub fn register_latent<F: LatentFn<M>, M>(&self, vm: &VirtualMachine, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.native(&NativeRequest { vm, class, name, kind: NativeKind::Latent, params: F::PARAMS, ret: <F::Output as IntoVariable>::TYPE }, move |call| call.latent(f))
    }

    fn native(&self, request: &NativeRequest<'_>, f: impl Fn(&NativeCall<'_>) -> Option<Variable> + Send + Sync + 'static) -> Result<&'static NativeFunction, Error> {
        let params: Vec<RawParamType> = request.params.iter().map(|p| RawParamType::new(Some(*p))).collect();
        let raw = RawNativeRequest::new(request, &params);
        let mut function = core::ptr::null();
        abi::status(&format!("native {}.{}", request.class, request.name), |error| call!(self.api, register_native(&raw const raw, abi::native(f), &raw mut function, error)))?;
        Ok(unsafe { &*function })
    }

    /// Takes over an unused console entry for `C` through the host (`skyrim::input::register`
    /// otherwise); register at `DATA_LOADED`. A later generation registering the same name is
    /// re-pointed at its `execute`; the name, help and parameters stay those of the first.
    pub fn register_command<C: Command>(&self) -> Result<CommandSlot, Error> {
        let request = RawCommandRequest::new(&CommandRequest { name: C::NAME, help: C::HELP, params: C::PARAMS, needs_reference: C::NEEDS_REFERENCE });
        Self::slot(&format!("command {}", C::NAME.to_string_lossy()), |slot, error| call!(self.api, register_command(&raw const request, abi::command(C::execute), slot, error)))
    }

    /// Runs `execute` in place of the engine console command `name` (its long or short name),
    /// with the arguments its own parameters unpack, on the thread that drains the console
    /// queue. The swap is one store of the entry's execute pointer, which the runner reads at
    /// every statement, so it is safe at any time; unloading the plugin puts the engine's own
    /// function back, and a later generation replacing the same name is re-pointed. A command
    /// another plugin registered or replaced is refused. `SkyrimRs commands` lists them.
    ///
    /// ```ignore
    /// host.replace_command(c"SetCameraFOV", |ctx| { /* ctx.args(): the command's own parameters */ true })?;
    /// ```
    pub fn replace_command(&self, name: &CStr, execute: impl Fn(&mut CommandContext<'_>) -> bool + Send + Sync + 'static) -> Result<CommandSlot, Error> {
        Self::slot(&format!("replacing the command {}", name.to_string_lossy()), |slot, error| call!(self.api, replace_command(name.as_ptr(), abi::command(execute), slot, error)))
    }

    /// The console entry a command registration writes, or why there is none.
    fn slot(what: &str, call: impl FnOnce(*mut RawCommandSlot, Sink<'_, *const abi::RawError>) -> Option<u32>) -> Result<CommandSlot, Error> {
        let mut slot = RawCommandSlot { opcode: 0, replaced: Str::new(&[]) };
        abi::status(what, |error| call(&raw mut slot, error))?;
        Ok(unsafe { slot.to_slot() })
    }

    /// Every plugin whose descriptor the host accepted, in load order, known before the first `load` runs.
    #[must_use]
    pub fn plugins(&self) -> &'static [Plugin] {
        static CACHED: Mutex<&'static [Plugin]> = Mutex::new(&[]);
        let read = RefCell::new(Vec::<Plugin>::new());
        let text = |s: Str| unsafe { s.as_str() }.to_owned();
        let mut record = |r: *const PluginRecord| read.borrow_mut().extend(unsafe { abi::read_sized(r) }.map(|r| Plugin { name: text(r.name), version: text(r.version), file: text(r.file), constraints: order::Constraints { priority: r.priority, ..Default::default() } }));
        let mut before = |name: Str| {
            if let Some(p) = read.borrow_mut().last_mut() {
                p.constraints.before.push(text(name));
            }
        };
        let mut after = |name: Str| {
            if let Some(p) = read.borrow_mut().last_mut() {
                p.constraints.after.push(text(name));
            }
        };
        call!(self.api, plugins(Sink::new(&mut record), Sink::new(&mut before), Sink::new(&mut after)));
        let read = read.into_inner();
        let mut cached = CACHED.lock().unwrap_or_else(PoisonError::into_inner);
        if *cached != read.as_slice() {
            *cached = Box::leak(read.into_boxed_slice());
        }
        *cached
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
    api: Api,
}

impl Tab {
    /// Adds a section named `name`, unique within the tab, after those added before; on a loader
    /// without tab sections this does nothing.
    pub fn section(&self, name: &str, draw: impl FnMut(&mut egui::Ui) + Send + 'static) -> &Self {
        ui::add_section(self.api, name, draw);
        self
    }
}

/// What a method of an exported interface returns, which decides the stand-in that answers
/// while the plugin is unloaded.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Returns {
    /// Nothing, an integer, a `bool`, an enum or a pointer: the stand-in returns 0.
    Value = 0,
    /// A `float` or `double`: 0.0.
    Float = 1,
    /// A structure written through the caller's buffer, which the method receives after `this`
    /// and returns: the stand-in returns the buffer as it was.
    Buffer = 2,
}

impl Returns {
    pub(crate) fn from_u32(value: u32) -> Option<Self> {
        [Self::Value, Self::Float, Self::Buffer].into_iter().find(|r| *r as u32 == value)
    }
}

/// One method of an interface for [`Host::export_interface`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Method {
    pub(crate) f: usize,
    /// A [`Returns`].
    pub(crate) returns: u32,
}

impl Method {
    /// `f` is the method's `extern "C"` function (`my_method as *const ()`).
    #[must_use]
    pub fn new(f: *const (), returns: Returns) -> Self {
        Self { f: f.addr(), returns: returns as u32 }
    }
}

/// A plugin name served through [`Host::serve_alias`].
#[derive(Clone, Copy)]
pub struct Alias {
    slot: usize,
    api: Api,
}

impl Alias {
    /// Dispatches `data` as the served name to the plugin named `receiver` (a message's `sender`
    /// for a reply), or to every plugin listening to the name when it is null; false while the
    /// name is not served or when no listener took it.
    #[must_use]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn send(&self, receiver: *const c_char, kind: u32, data: *mut c_void, len: u32) -> bool {
        call!(self.api, alias_send(self.slot, receiver, kind, data, len)).unwrap_or(false)
    }
}

/// A hotkey from [`Host::action`].
#[derive(Clone, Copy)]
pub struct Action {
    index: usize,
    api: Api,
}

impl Action {
    /// Whether the key was pressed while the declared side had it since the previous frame: true
    /// from the start of the next present (its [`Host::on_frame`] callbacks and windows) to the
    /// start of the one after, so once per press for a frame callback.
    #[must_use]
    pub fn fired(&self) -> bool {
        call!(self.api, fired(self.index)).unwrap_or(false)
    }

    /// Whether the bound key and its modifiers are down in the game now: for an action that acts
    /// every frame while its key is held. False while the action is unbound.
    #[must_use]
    pub fn held(&self) -> bool {
        call!(self.api, held(self.index)).unwrap_or(false)
    }

    /// The seconds the bound key had been held when its release reached the game, for the frame
    /// after the release (as [`fired`](Self::fired) for the press); the modifiers may already be up.
    /// `None` while the key is not released or the action is unbound.
    #[must_use]
    pub fn released(&self) -> Option<f32> {
        let mut secs = 0.0;
        call!(self.api, released(self.index, &raw mut secs)).unwrap_or(false).then_some(secs)
    }

    /// The key bound now; `None` while a conflict leaves the action unbound.
    #[must_use]
    pub fn key(&self) -> Option<Hotkey> {
        let mut key = Hotkey::new(0);
        call!(self.api, key(self.index, &raw mut key)).unwrap_or(false).then_some(key)
    }
}

/// An input filter from [`Host::input_filter`].
#[derive(Clone, Copy)]
pub struct InputFilter {
    index: usize,
    api: Api,
}

impl InputFilter {
    /// Starts or stops taking the filter's events from the game, from the next input frame.
    pub fn set_active(&self, active: bool) {
        call!(self.api, set_filter_active(self.index, active));
    }

    #[must_use]
    pub fn active(&self) -> bool {
        call!(self.api, filter_active(self.index)).unwrap_or(false)
    }
}

/// Declares a framework plugin: exports its [`Descriptor`], with the version of the crate it is
/// invoked in, and the C entry points that call the plugin's Rust functions. Everything after
/// `load` is optional, in this order: the ordering fields (defaults: no constraints, priority 0)
/// and the reload functions (defaults: none).
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
            api: $crate::abi::API,
            build: ::core::ptr::null(),
            name: concat!($name, "\0").as_ptr().cast(),
            version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
            load: {
                extern "C" fn skyrim_rs_load(api: *const $crate::abi::HostApi) -> bool {
                    $crate::abi::start(api, $load)
                }
                skyrim_rs_load
            },
            load_before: &[$($(concat!($before, "\0").as_ptr().cast(),)? ::core::ptr::null()] as *const _,
            load_after: &[$($(concat!($after, "\0").as_ptr().cast(),)? ::core::ptr::null()] as *const _,
            priority: ($($priority,)? 0i32,).0,
            unload: ($(Some({
                extern "C" fn skyrim_rs_unload() {
                    $unload()
                }
                skyrim_rs_unload as extern "C" fn()
            }),)? None::<extern "C" fn()>,).0,
            save_state: ($(Some({
                extern "C" fn skyrim_rs_save(writer: $crate::abi::Sink<'_, $crate::abi::Str>) {
                    $crate::abi::save(&writer, $save)
                }
                skyrim_rs_save as $crate::abi::RawSaveFn
            }),)? None::<$crate::abi::RawSaveFn>,).0,
            restore_state: ($(Some({
                extern "C" fn skyrim_rs_restore(ptr: *const u8, len: usize) {
                    unsafe { $crate::abi::restore(ptr, len, $restore) }
                }
                skyrim_rs_restore as $crate::abi::RawRestoreFn
            }),)? None::<$crate::abi::RawRestoreFn>,).0,
        };
    };
}
