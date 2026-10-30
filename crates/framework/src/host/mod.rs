//! The host's side: [`load`] is `skyrim_rs.dll`'s `SKSEPlugin_Load`. The host owns what the
//! engine and the loader keep for every plugin (one message listener and one Papyrus callback,
//! the plugins' tabs in its window, one sink per event source, the native function objects, the
//! console entries and the hook sites) and tags each registration with its plugin, so `reload`
//! can detach a plugin and unload its DLL. It keeps every plugin's settings, its own in
//! `skyrim-rs.settings.json` (the capture rule and every hotkey of the binding table `keys`),
//! draws the overlay's one window (`gui`) and answers the console command `SkyrimRs` (`console`).

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use skyrim::events::{ErasedHandler, ErasedSink, ErasedSource};
use skyrim::prelude::*;
use sre::BSEventNotifyControl;
use skyrim::input::{CommandSlot, ErasedCommand, ErasedExecute, ReplacedCommand};
use skyrim::papyrus::{ErasedNative, NativeFunction, VirtualMachine};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{DONT_RESOLVE_DLL_REFERENCES, GetModuleHandleW, GetProcAddress, LoadLibraryExW, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use crate::hooks::{Compat, Hooks};
use crate::order::{self, Constraints};
use crate::settings;
use crate::{ABI, BUILD, CommandRequest, Descriptor, Error, FrameFn, Host, MessageFn, NativeRequest, PapyrusFn, Plugin, Table};

static LOG: Mutex<Option<File>> = Mutex::new(None);
/// The SKSE log folder, where the host writes its log and `SkyrimRs capture` its frame captures.
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The last host log lines, for the crash log.
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const RECENT_LINES: usize = 24;

fn log_line(line: core::fmt::Arguments<'_>) {
    if let Some(f) = LOG.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(f, "{line}");
    }
    let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    if recent.len() == RECENT_LINES {
        recent.pop_front();
    }
    recent.push_back(line.to_string());
}

macro_rules! log {
    ($($arg:tt) => {
        $crate::host::log_line(format_args!($($arg))
    };
}

mod alias;
mod console;
mod filters;
mod gui;
mod interface;
mod keys;
mod owned;
mod reload;

use owned::Owned;

/// The folder under `Data\SKSE\Plugins` the host loads plugins from.
pub const PLUGIN_DIR: &str = "skyrim-rs";
/// The host's settings in `Data\SKSE\Plugins`: `compat = refuse` (the default) or `stack`, see [`Compat`].
pub const CONFIG: &str = "skyrim-rs.ini";
/// The host's own name, as the owner of its window and settings.
pub const NAME: &str = "skyrim-rs";

static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();
/// The DLLs in the plugin folder the host did not load, each with the reason.
static REFUSED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static MESSAGES: Owned<MessageFn> = Owned::new();
/// Handlers of other SKSE plugins' messages, each with the sender it listens to.
static PLUGIN_MESSAGES: Owned<(String, MessageFn)> = Owned::new();
static FRAMES: Owned<FrameFn> = Owned::new();
static PAPYRUS: Owned<PapyrusFn> = Owned::new();
static SINKS: Mutex<Vec<SourceSinks>> = Mutex::new(Vec::new());
static PLAYER_GRAPH: Owned<ErasedHandler> = Owned::new();
/// The host's sink on the player's graph source, made at the first [`Host::add_player_graph_sink`].
static PLAYER_GRAPH_SINK: OnceLock<&'static ErasedSink> = OnceLock::new();
static NATIVES: Mutex<Vec<Native>> = Mutex::new(Vec::new());
static COMMANDS: Mutex<Vec<ConsoleCommand>> = Mutex::new(Vec::new());
static REPLACED: Mutex<Vec<EngineCommand>> = Mutex::new(Vec::new());
/// The startup message kinds seen so far, replayed to a handler registered after them.
static STARTUP: Mutex<Vec<u32>> = Mutex::new(Vec::new());
/// The VM once the Papyrus registration pass has run.
static VM: AtomicUsize = AtomicUsize::new(0);
/// The interface the loader gave the host, for what runs after `load`.
static SKSE: OnceLock<&'static skse::Interface> = OnceLock::new();
/// Which loader hosts the game, for the crash log.
static HOSTED_BY: OnceLock<&'static str> = OnceLock::new();
/// The last loader message's kind (`u32::MAX` before the first) and the tick it arrived at.
static LAST_MESSAGE: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_MESSAGE_AT: AtomicU64 = AtomicU64::new(0);
/// The SKSE plugins in `Data\SKSE\Plugins`, read once the loader has loaded them all.
static SKSE_PLUGINS: Mutex<Vec<LegacyPlugin>> = Mutex::new(Vec::new());
static HOOKS: Hooks = Hooks::HOST;
static TABLE: Table = Table {
    on_message,
    on_plugin_message,
    serve_alias,
    alias_send: alias::send,
    export_interface: interface::export,
    on_frame,
    on_papyrus,
    action: keys::declare,
    action_fired: keys::fired,
    action_key: keys::key,
    action_held: keys::held,
    action_released: keys::released,
    key_held: overlay::game_key_held,
    key_released: overlay::game_key_released,
    game_mouse: overlay::game_mouse,
    game_pad: overlay::game_pad,
    input_filter: filters::declare,
    filter_set_active: filters::set_active,
    filter_active: filters::is_active,
    set_tab_title: gui::set_title,
    add_section: gui::add_section,
    add_hud: gui::add_hud,
    show_tab: gui::show_tab,
    show_section: gui::show_section,
    read_frame: overlay::read_back,
    setting: settings::get,
    set_setting: settings::set,
    setting_keys: settings::keys,
    remove_settings: settings::remove,
    add_sink,
    add_player_graph_sink,
    register_native,
    register_command,
    replace_command,
    plugins,
};

/// An SKSE plugin in `Data\SKSE\Plugins`: a DLL exporting `SKSEPlugin_Version` or `SKSEPlugin_Load`.
#[derive(Clone)]
struct LegacyPlugin {
    file: String,
    /// Whether the loader kept it mapped, which it does only for the plugins it accepted.
    loaded: bool,
    /// Built by `skse::alias_stub!`: a name a framework plugin serves.
    stub: bool,
    /// What its `SKSEPlugin_Version` holds; `None` when it exports none.
    data: Option<VersionData>,
}

#[derive(Clone)]
struct VersionData {
    name: String,
    version: u32,
    author: String,
    flags: u32,
    method: u32,
}

impl core::fmt::Display for LegacyPlugin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.data {
            Some(d) => write!(f, "{}: {} version {:#x} by {}, flags {:#x}, method {:#x}", self.file, d.name, d.version, d.author, d.flags, d.method)?,
            None => write!(f, "{}: no version data", self.file)?,
        }
        if self.stub {
            f.write_str(", a stub")?;
        } else if !self.loaded {
            f.write_str(", not loaded")?;
        }
        Ok(())
    }
}

/// The host's sink on one source and the plugins' handlers behind it.
struct SourceSinks {
    source: ErasedSource,
    handlers: &'static Owned<ErasedHandler>,
}

struct Native {
    class: String,
    name: String,
    owner: &'static str,
    erased: ErasedNative,
}

struct ConsoleCommand {
    name: String,
    owner: &'static str,
    erased: ErasedCommand,
}

/// An engine console command a plugin's execute function replaces.
struct EngineCommand {
    name: String,
    owner: &'static str,
    replaced: ReplacedCommand,
}

/// Initializes the game layer, installs the overlay, registers the host's message listener and
/// Papyrus callback, then loads every plugin in `Data\SKSE\Plugins\skyrim-rs\` from a shadow copy
/// in the order their descriptors resolve to, and watches their files from then on.
pub fn load(skse: &skse::Interface) -> bool {
    // The loader hands every plugin the same object in its read-only data, alive for the whole game.
    let skse: &'static skse::Interface = SKSE.get_or_init(|| unsafe { &*core::ptr::from_ref(skse) });
    if let Ok(dir) = skse::log::dir(skse.runtime_version) {
        let _ = LOG_DIR.set(dir);
    }
    if let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs") {
        *LOG.lock().unwrap() = Some(file);
    }
    log!("{}, abi {ABI}", BUILD.to_string_lossy());
    let hosted_by = if skse::runtime::script_extender_loaded(skse.runtime_version) { "the script extender" } else { "the standalone loader" };
    let _ = HOSTED_BY.set(hosted_by);
    log!("hosted by {hosted_by}, plugin interface {:08x}, runtime {:08x}", skse.skse_version, skse.runtime_version);
    let symbol_path = [plugins_dir(), plugins_dir().join(PLUGIN_DIR), plugins_dir().join(PLUGIN_DIR).join(reload::SHADOW_DIR)].iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(";");
    match crash::install(&crash::Options { name: NAME, runtime_version: skse.runtime_version, build: BUILD.to_str().unwrap_or(""), facts: Some(crash_facts), symbol_path: Some(symbol_path) }) {
        Ok(crash::Installed::Here { watchdog, symbols, symbol_mismatches }) => log!("crash handler installed, watchdog thread {watchdog}, game symbols {symbols:?} ({} functions, {symbol_mismatches} offsets differ from the ledger)", crash::symbols::count()),
        Ok(crash::Installed::Elsewhere) => log!("crash handler: another copy in the process reports crashes"),
        Err(e) => log!("crash handler not installed: {e}"),
    }
    Hooks::set_logger(|line| log!("hooks: {line}"));
    if let Err(e) = skyrim::init(skse) {
        log!("game layer: {e}");
        return false;
    }
    let (compat, source) = match std::fs::read_to_string(plugins_dir().join(CONFIG)).ok().as_deref().and_then(|text| setting(text, "compat")).map(str::parse::<Compat>) {
        Some(Ok(compat)) => (compat, CONFIG.to_owned()),
        Some(Err(e)) => (Compat::Refuse, format!("{CONFIG}: {e}")),
        None => (Compat::Refuse, "default".to_owned()),
    };
    log!("hooks: compatibility mode {compat} ({source})");
    Hooks::set_compat(compat);
    Hooks::revalidate("startup");
    let dir = plugins_dir().join(PLUGIN_DIR);
    open_settings(NAME, &dir);
    keys::declare_host();
    match overlay::install() {
        Ok(()) => log!("overlay installed, toggled with {}, closed with {}", keys::show(keys::key(keys::TOGGLE)), keys::show(keys::key(keys::CLOSE))),
        Err(e) => log!("overlay: {e}"),
    }
    log!("overlay font: {}", overlay::font_source());
    log!("overlay cursor: {}", overlay::cursor_source());
    gui::apply_capture();
    let (Some(messaging), Some(papyrus)) = (skse.messaging(), skse.papyrus()) else {
        log!("no messaging or Papyrus interface");
        return false;
    };
    if !listen(messaging, skse.plugin_handle()) || !papyrus.register(on_papyrus_pass) {
        log!("the loader refused the message listener or the Papyrus callback");
        return false;
    }
    gui::install();
    let shadow = reload::clean(&dir);
    let mut read = Vec::new();
    for path in plugin_files(&dir) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let loaded = reload::copy(&path, &shadow, 1).map_err(|e| format!("copy: {e}")).and_then(|copy| read_descriptor(&copy).map(|(module, descriptor)| (module, descriptor, copy)));
        match loaded {
            Ok((module, descriptor, _)) if plugin_name(descriptor) == NAME => {
                refuse(file, format!("{NAME} is the host's own name, which its settings file and keys use"));
                unsafe { FreeLibrary(module) };
            }
            Ok((module, descriptor, copy)) => read.push((module, descriptor, describe(descriptor, file), path, copy)),
            Err(e) => refuse(file, e),
        }
    }
    let items: Vec<(&str, &Constraints)> = read.iter().map(|(_, _, p, _, _)| (p.name.as_str(), &p.constraints)).collect();
    let order = order::sort(&items);
    for (asker, name) in &order.missing {
        log!("no plugin named {name} (asked by {})", read[*asker].2.name);
    }
    let in_cycle: Vec<&str> = order.cycle.iter().map(|&i| read[i].2.name.as_str()).collect();
    for &i in &order.cycle {
        let (module, _, plugin, _, _) = &read[i];
        refuse(plugin.file.clone(), format!("{} is in a load order cycle with {}", plugin.name, in_cycle.iter().filter(|n| **n != plugin.name).copied().collect::<Vec<_>>().join(", ")));
        unsafe { FreeLibrary(*module) };
    }
    let sorted: Vec<Plugin> = order.sorted.iter().map(|&i| read[i].2.clone()).collect();
    log!("load order: {}", sorted.iter().map(|p| if p.constraints == Constraints::default() { p.name.clone() } else { format!("{} ({})", p.name, p.constraints) }).collect::<Vec<_>>().join(", "));
    for (rank, plugin) in sorted.iter().enumerate() {
        HOOKS.set_rank(&plugin.name, i32::try_from(rank).unwrap_or(i32::MAX));
    }
    let _ = PLUGINS.set(sorted);
    let mut loaded = Vec::new();
    for &i in &order.sorted {
        let (module, descriptor, plugin, source, copy) = &read[i];
        let name: &'static str = Box::leak(plugin.name.clone().into_boxed_str());
        let host: &'static Host = Box::leak(Box::new(Host { skse, name, hooks: Box::leak(Box::new(Hooks::for_plugin(name))), table: &TABLE }));
        open_settings(name, &dir);
        crash::note_image(&plugin.name, 1, *module as usize, true);
        let live = (descriptor.load)(host);
        if live {
            log!("loaded {}: {}", plugin.file, plugin.name);
            log!("{}: generation 1 loaded, image at {:#x}", plugin.name, *module as usize);
        } else {
            log!("skipped {}: {}: load returned false", plugin.file, plugin.name);
            detach(name);
        }
        loaded.push(reload::Loaded::new(plugin.clone(), host, *module, descriptor, live, source.clone(), copy.clone()));
    }
    reload::watch(loaded);
    overlay::on_frame(frame);
    true
}

/// The overlay's frame callback: reloads first, then every plugin's [`Host::on_frame`].
fn frame() {
    reload::tick();
    follow_player_graph();
    FRAMES.each(|f| {
        f();
        true
    });
}

/// Whether the loader's `DATA_LOADED` message has arrived (the control map is loaded by then).
fn data_loaded() -> bool {
    STARTUP.lock().unwrap_or_else(PoisonError::into_inner).contains(&skse::MessagingInterface::DATA_LOADED)
}

/// Reads a plugin's settings file and logs what came of it.
fn open_settings(owner: &str, dir: &Path) {
    match settings::open(owner, dir) {
        Ok(0) => {}
        Ok(n) => log!("{owner}: {n} settings read from {}", settings::file(dir, owner).display()),
        Err(e) => log!("{owner}: settings not read, {e}"),
    }
}

/// Writes the settings that changed, of every plugin or of one, and logs each file.
fn flush_settings(owner: Option<&str>) {
    for written in settings::flush(owner) {
        match written {
            Ok(path) => log!("settings written to {}", path.display()),
            Err(e) => log!("settings not written: {e}"),
        }
    }
}

/// Logs a plugin file the host does not load and keeps it for the Plugins tab.
fn refuse(file: String, reason: String) {
    log!("skipped {file}: {reason}");
    REFUSED.lock().unwrap_or_else(PoisonError::into_inner).push((file, reason));
}

fn refused() -> Vec<(String, String)> {
    REFUSED.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn skse_plugins() -> Vec<LegacyPlugin> {
    SKSE_PLUGINS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The accepted plugins in load order, empty before [`load`] resolved it.
#[must_use]
pub fn plugins() -> &'static [Plugin] {
    PLUGINS.get().map_or(&[], Vec::as_slice)
}

/// `Data\SKSE\Plugins` next to the executable.
fn plugins_dir() -> PathBuf {
    std::env::current_exe().map(|exe| exe.with_file_name("Data").join("SKSE").join("Plugins")).unwrap_or_default()
}

/// The value of `key = value` in an ini text; sections are ignored, `;` and `#` start comments.
fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().map(|line| line.split([';', '#']).next().unwrap_or_default()).find_map(|line| {
        let (k, v) = line.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
    })
}

fn plugin_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"))).collect();
    files.sort();
    files
}

/// Whether a descriptor's C-typed head matches this host.
fn accept(abi: u32, build: &CStr) -> Result<(), String> {
    if abi != ABI {
        return Err(format!("descriptor abi {abi}, the host's is {ABI}"));
    }
    if build != BUILD {
        return Err(format!("built as \"{}\", the host is \"{}\"", build.to_string_lossy(), BUILD.to_string_lossy()));
    }
    Ok(())
}

/// Loads the DLL and checks its descriptor's head; a refused DLL is unloaded again.
fn read_descriptor(path: &Path) -> Result<(HMODULE, &'static Descriptor), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("LoadLibrary: {}", std::io::Error::last_os_error()));
    }
    let descriptor = unsafe { GetProcAddress(module, c"SkyrimRs_Plugin".as_ptr().cast()) }.map(|p| unsafe { &*(p as *const Descriptor) });
    let checked = descriptor.ok_or_else(|| "no SkyrimRs_Plugin export".to_owned()).and_then(|d| {
        if d.build.is_null() {
            return Err("null build string".to_owned());
        }
        accept(d.abi, unsafe { CStr::from_ptr(d.build) }).map(|()| d)
    });
    match checked {
        Ok(d) => Ok((module, d)),
        Err(e) => {
            unsafe { FreeLibrary(module) };
            Err(e)
        }
    }
}

fn message_name(kind: u32) -> &'static str {
    match kind {
        skse::MessagingInterface::PLUGINS_LOADED => "PLUGINS_LOADED",
        skse::MessagingInterface::PLUGINS_LOADED_2 => "PLUGINS_LOADED_2",
        skse::MessagingInterface::PRE_LOAD_GAME => "PRE_LOAD_GAME",
        skse::MessagingInterface::POST_LOAD_GAME => "POST_LOAD_GAME",
        skse::MessagingInterface::SAVE_GAME => "SAVE_GAME",
        skse::MessagingInterface::DELETE_GAME => "DELETE_GAME",
        skse::MessagingInterface::CONTROLS_READY => "CONTROLS_READY",
        skse::MessagingInterface::NEW_GAME => "NEW_GAME",
        skse::MessagingInterface::DATA_LOADED => "DATA_LOADED",
        _ => "unknown",
    }
}

/// Reads every SKSE plugin DLL in `Data\SKSE\Plugins`: whether the loader kept it, and its version
/// data. A DLL the loader did not keep is mapped without running any of its code to read it.
fn snapshot_skse_plugins() {
    let mut found = Vec::new();
    for path in plugin_files(&plugins_dir()) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mapped = unsafe { GetModuleHandleW(wide.as_ptr()) };
        let module = if mapped.is_null() { unsafe { LoadLibraryExW(wide.as_ptr(), core::ptr::null_mut(), DONT_RESOLVE_DLL_REFERENCES) } } else { mapped };
        if module.is_null() {
            continue;
        }
        let export = |name: &CStr| unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        let text = |bytes: &[u8]| CStr::from_bytes_until_nul(bytes).ok().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let data = export(c"SKSEPlugin_Version").map(|version| {
            let data = unsafe { &*(version as *const skse::PluginVersionData) };
            VersionData { name: text(&data.name), version: data.plugin_version, author: text(&data.author), flags: data.flags, method: data.method }
        });
        let is_plugin = data.is_some() || export(c"SKSEPlugin_Load").is_some();
        let stub = export(skse::ALIAS_STUB_SYMBOL).is_some();
        if mapped.is_null() {
            unsafe { FreeLibrary(module) };
        }
        if is_plugin {
            found.push(LegacyPlugin { file, loaded: !mapped.is_null(), stub, data });
        }
    }
    *SKSE_PLUGINS.lock().unwrap_or_else(PoisonError::into_inner) = found;
}

/// The host's facts for the crash log, on the watchdog thread: locks are tried, never waited for.
fn crash_facts(w: &mut crash::Writer<'_>, crash: &crash::Crash, section: crash::Section) {
    use core::fmt::Write as _;
    match section {
        crash::Section::Context => {
            let _ = writeln!(w, "hosted by {}; {}", HOSTED_BY.get().unwrap_or(&"an unknown loader"), BUILD.to_str().unwrap_or(""));
            match reload::try_generations() {
                Some(generations) if generations.is_empty() => {
                    let _ = writeln!(w, "framework plugins: none");
                }
                Some(generations) => {
                    let _ = write!(w, "framework plugins:");
                    for (name, generation, live) in generations {
                        let _ = write!(w, " {name} generation {generation}{};", if live { "" } else { " (stopped)" });
                    }
                    let _ = writeln!(w);
                }
                None => {
                    let _ = writeln!(w, "framework plugins: the list is locked by another thread");
                }
            }
            match SKSE_PLUGINS.try_lock() {
                Ok(plugins) if plugins.is_empty() => {
                    let _ = writeln!(w, "SKSE plugins: not read yet (before PLUGINS_LOADED)");
                }
                Ok(plugins) => {
                    let _ = writeln!(w, "SKSE plugins:");
                    for plugin in plugins.iter() {
                        let _ = writeln!(w, "  {plugin}");
                    }
                }
                Err(_) => {
                    let _ = writeln!(w, "SKSE plugins: the list is locked by another thread");
                }
            }
            match LAST_MESSAGE.load(Ordering::Relaxed) {
                u32::MAX => {
                    let _ = writeln!(w, "no loader message yet");
                }
                kind => {
                    let ago = unsafe { GetTickCount64() }.saturating_sub(LAST_MESSAGE_AT.load(Ordering::Relaxed));
                    let _ = writeln!(w, "last loader message: {} (kind {kind}), {ago} ms ago", message_name(kind));
                }
            }
        }
        crash::Section::State => {
            Hooks::crash_facts(w, crash.thread);
            match RECENT.try_lock() {
                Ok(lines) => {
                    let _ = writeln!(w, "host log, last {} lines:", lines.len());
                    for line in lines.iter() {
                        let _ = writeln!(w, "  {line}");
                    }
                }
                Err(_) => {
                    let _ = writeln!(w, "host log: locked by another thread, not read");
                }
            }
        }
    }
}

fn plugin_name(descriptor: &Descriptor) -> String {
    if descriptor.name.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.name) }.to_string_lossy().into_owned() }
}

fn describe(descriptor: &Descriptor, file: String) -> Plugin {
    let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    let version = if descriptor.version.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.version) }.to_string_lossy().into_owned() };
    Plugin { name: plugin_name(descriptor), version, file, constraints: Constraints { before: owned(descriptor.load_before), after: owned(descriptor.load_after), priority: descriptor.priority } }
}

/// Removes every registration of `owner` and returns what went, with every call inside them
/// drained. Each registry's lock is released before the drain, so a callback still running
/// may register through the host while it is waited for.
fn detach(owner: &str) -> String {
    let hooks = HOOKS.remove_owner(owner);
    let sources: Vec<&'static Owned<ErasedHandler>> = SINKS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|s| s.handlers).collect();
    let sinks = sources.iter().map(|handlers| handlers.remove_owner(owner)).sum::<usize>() + PLAYER_GRAPH.remove_owner(owner);
    let natives: Vec<(String, &'static str, ErasedNative)> = NATIVES.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|n| n.owner == owner).map(|n| (format!("{}.{}", n.class, n.name), n.owner, n.erased)).collect();
    for (what, plugin, erased) in &natives {
        let (what, plugin) = (what.clone(), *plugin);
        erased.set(Box::new(move |_| {
            log!("{what} called while {plugin} is unloaded");
            None
        }));
    }
    let commands: Vec<(String, ErasedCommand)> = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|c| c.owner == owner).map(|c| (format!("{}: {} is unloaded", c.name, c.owner), c.erased)).collect();
    for (line, erased) in &commands {
        let line = line.clone();
        erased.set(Box::new(move |_| {
            skyrim::input::print(&line);
            true
        }));
    }
    let restored: Vec<ReplacedCommand> = REPLACED.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|c| c.owner == owner).map(|c| c.replaced).collect();
    restored.iter().for_each(ReplacedCommand::restore);
    let (natives, commands, restored) = (natives.len(), commands.len(), restored.len());
    let (sections, huds) = gui::remove(owner);
    if sections + huds > 0 {
        overlay::reset_context();
    }
    let messages = MESSAGES.remove_owner(owner) + PLUGIN_MESSAGES.remove_owner(owner) + alias::remove_owner(owner);
    let interfaces = interface::remove_owner(owner);
    let papyrus = PAPYRUS.remove_owner(owner);
    let frames = FRAMES.remove_owner(owner);
    let filters = match filters::remove_owner(owner) {
        0 => String::new(),
        n => format!(", {n} active input filters"),
    };
    format!("{hooks} hook subscriptions, {sinks} sinks, {natives} natives stubbed, {commands} commands stubbed, {restored} engine commands restored, {sections} tab sections, {huds} HUD painters, {messages} message handlers, {interfaces} interfaces parked, {papyrus} Papyrus callbacks, {frames} frame callbacks{filters}")
}

fn vm() -> Option<&'static VirtualMachine> {
    let vm = VM.load(Ordering::Acquire);
    (vm != 0).then(|| unsafe { &*(vm as *const VirtualMachine) })
}

/// The loader's messages, which the host needs, and other plugins' for `on_plugin_message`, which
/// it can do without.
fn listen(messaging: &skse::MessagingInterface, handle: skse::PluginHandle) -> bool {
    if !messaging.listen_plugins(handle, on_plugin_dispatch) {
        log!("the loader refused the listener for other plugins' messages");
    }
    messaging.listen_skse(handle, on_skse_message)
}

unsafe extern "C" fn on_skse_message(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    LAST_MESSAGE.store(msg.kind, Ordering::Relaxed);
    LAST_MESSAGE_AT.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
    match msg.kind {
        skse::MessagingInterface::PLUGINS_LOADED => {
            snapshot_skse_plugins();
            alias::resolve_waiting();
        }
        skse::MessagingInterface::PLUGINS_LOADED_2 => Hooks::revalidate("PLUGINS_LOADED_2"),
        skse::MessagingInterface::DATA_LOADED => Hooks::revalidate("DATA_LOADED"),
        _ => {}
    }
    if msg.kind == skse::MessagingInterface::DATA_LOADED {
        match skyrim::input::register::<console::Command>() {
            Ok(slot) => log!("command SkyrimRs took over console opcode {:#x} ({})", slot.opcode, slot.replaced),
            Err(e) => log!("command SkyrimRs: {e}"),
        }
    }
    if matches!(msg.kind, skse::MessagingInterface::PLUGINS_LOADED | skse::MessagingInterface::PLUGINS_LOADED_2 | skse::MessagingInterface::CONTROLS_READY | skse::MessagingInterface::DATA_LOADED) {
        STARTUP.lock().unwrap_or_else(PoisonError::into_inner).push(msg.kind);
    }
    if msg.kind == skse::MessagingInterface::DATA_LOADED {
        keys::describe_game_uses();
    }
    MESSAGES.each(|f| {
        f(msg);
        true
    });
}

unsafe extern "C" fn on_papyrus_pass(registry: *mut c_void) {
    let Some(vm) = (unsafe { VirtualMachine::from_registry(registry) }) else {
        log!("the Papyrus callback did not receive the VM");
        return;
    };
    VM.store(core::ptr::from_ref(vm) as usize, Ordering::Release);
    PAPYRUS.each(|f| {
        f(vm);
        true
    });
}

fn on_message(owner: &'static str, f: MessageFn) {
    MESSAGES.add(owner, f);
    let seen = STARTUP.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for kind in seen {
        f(&skse::Message { sender: c"SKSE".as_ptr(), kind, data_len: 0, data: core::ptr::null_mut() });
    }
}

fn on_plugin_message(owner: &'static str, sender: &str, f: MessageFn) {
    PLUGIN_MESSAGES.add(owner, (sender.to_owned(), f));
}

/// Whether a message's sender is `name`, compared as the loader compares senders.
fn sent_by(msg: &skse::Message, name: &str) -> bool {
    !msg.sender.is_null() && unsafe { CStr::from_ptr(msg.sender) }.to_bytes().eq_ignore_ascii_case(name.as_bytes())
}

unsafe extern "C" fn on_plugin_dispatch(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    if alias::take_probe(msg) {
        return;
    }
    PLUGIN_MESSAGES.each(|(sender, f)| {
        if sent_by(msg, sender) {
            f(msg);
        }
        true
    });
}

fn serve_alias(owner: &'static str, name: &str, f: MessageFn) -> Result<usize, Error> {
    let started = STARTUP.lock().unwrap_or_else(PoisonError::into_inner).contains(&skse::MessagingInterface::PLUGINS_LOADED);
    alias::serve(owner, name, f, started)
}

fn on_frame(owner: &'static str, f: FrameFn) {
    FRAMES.add(owner, f);
}

fn on_papyrus(owner: &'static str, f: PapyrusFn) {
    PAPYRUS.add(owner, f);
    if let Some(vm) = vm() {
        f(vm);
    }
}

/// Runs `handlers` in registration order until one returns `Stop`.
fn dispatch(handlers: &Owned<ErasedHandler>, event: *const c_void, source: *const sre::BSTEventSource) -> BSEventNotifyControl {
    let mut control = BSEventNotifyControl::Continue;
    handlers.each(|f| {
        control = f(event, source);
        control == BSEventNotifyControl::Continue
    });
    control
}

fn add_sink(owner: &'static str, source: ErasedSource, handler: ErasedHandler) {
    let mut sinks = SINKS.lock().unwrap_or_else(PoisonError::into_inner);
    let handlers = if let Some(s) = sinks.iter().find(|s| s.source == source) {
        s.handlers
    } else {
        let handlers: &'static Owned<ErasedHandler> = Box::leak(Box::new(Owned::new()));
        source.add_sink(ErasedSink::new(Box::new(move |event, source| dispatch(handlers, event, source))));
        sinks.push(SourceSinks { source, handlers });
        handlers
    };
    handlers.add(owner, handler);
    log!("{owner}: sink added on the source at {:#x}", source.address());
}

fn add_player_graph_sink(owner: &'static str, handler: ErasedHandler) {
    PLAYER_GRAPH.add(owner, handler);
    PLAYER_GRAPH_SINK.get_or_init(|| ErasedSink::new(Box::new(|event, source| dispatch(&PLAYER_GRAPH, event, source))));
    log!("{owner}: sink added on the player's graph");
}

/// Puts the player graph sink on graph 0's current source (graph 0 sends in both views).
/// `AddEventSink` skips a sink already registered, and a source rebuilt with the player's 3D, at
/// the old address or another, gets it again.
fn follow_player_graph() {
    if let (Some(sink), Some(player)) = (PLAYER_GRAPH_SINK.get(), sre::PlayerCharacter::singleton())
        && let Some(source) = skyrim::animation::graph_event_source(player, Some(0))
    {
        ErasedSource::new(source).add_sink(sink);
    }
}

fn register_native(owner: &'static str, request: NativeRequest<'_>) -> Result<&'static NativeFunction, Error> {
    let mut natives = NATIVES.lock().unwrap_or_else(PoisonError::into_inner);
    let what = format!("{}.{}", request.class, request.name);
    if let Some(existing) = natives.iter().find(|n| n.class.eq_ignore_ascii_case(request.class) && n.name.eq_ignore_ascii_case(request.name)) {
        if existing.owner != owner {
            return Err(Error::Taken { what, owner: existing.owner.to_owned() });
        }
        existing.erased.set(request.f);
        log!("{owner}: {what} re-pointed");
        return Ok(existing.erased.function());
    }
    let native = request.vm.register_erased(request.class, request.name, request.kind, request.params, request.ret, request.f)?;
    log!("{owner}: {what} bound");
    natives.push(Native { class: request.class.to_owned(), name: request.name.to_owned(), owner, erased: native });
    Ok(native.function())
}

fn replace_command(owner: &'static str, name: &CStr, execute: ErasedExecute) -> Result<CommandSlot, Error> {
    let text = name.to_string_lossy().into_owned();
    if let Some(taken) = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|c| c.name.eq_ignore_ascii_case(&text)) {
        return Err(Error::Taken { what: text, owner: taken.owner.to_owned() });
    }
    let mut replaced = REPLACED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = replaced.iter().find(|c| c.name.eq_ignore_ascii_case(&text)) {
        if existing.owner != owner {
            return Err(Error::Taken { what: text, owner: existing.owner.to_owned() });
        }
        existing.replaced.set(execute);
        log!("{owner}: command {text} replaced again");
        return Ok(existing.replaced.slot());
    }
    let command = skyrim::input::replace_erased(name, execute)?;
    log!("{owner}: command {text} (opcode {:#x}) replaced", command.slot().opcode);
    replaced.push(EngineCommand { name: text, owner, replaced: command });
    Ok(command.slot())
}

/// One line per command a plugin registered or replaced, for the console inspector.
fn command_lines() -> Vec<String> {
    let mut lines: Vec<String> = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|c| format!("command {} ({}): opcode {:#x}, in place of the unused {}", c.name, c.owner, c.erased.slot().opcode, c.erased.slot().replaced)).collect();
    lines.extend(REPLACED.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|c| format!("command {} ({}): opcode {:#x}, the engine's execute {}", c.name, c.owner, c.replaced.slot().opcode, if c.replaced.active() { "replaced" } else { "restored" })));
    lines
}

fn register_command(owner: &'static str, request: CommandRequest<'_>) -> Result<CommandSlot, Error> {
    let mut commands = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner);
    let name = request.name.to_string_lossy().into_owned();
    if let Some(existing) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(&name)) {
        if existing.owner != owner {
            return Err(Error::Taken { what: name, owner: existing.owner.to_owned() });
        }
        existing.erased.set(request.execute);
        log!("{owner}: command {name} re-pointed");
        return Ok(existing.erased.slot());
    }
    let command = skyrim::input::register_erased(request.name, request.help, request.params, request.needs_reference, request.execute)?;
    let slot = command.slot();
    log!("{owner}: command {name} took over console opcode {:#x} ({})", slot.opcode, slot.replaced);
    commands.push(ConsoleCommand { name, owner, erased: command });
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_its_own_build() {
        assert!(accept(ABI, BUILD).is_ok());
        assert!(accept(ABI + 1, BUILD).is_err());
        assert!(accept(ABI, c"framework 0.0.0, rustc 1.0.0").is_err());
    }

    #[test]
    fn plugin_messages_reach_the_handlers_of_their_sender_until_the_owner_goes() {
        static SEEN: AtomicU32 = AtomicU32::new(0);
        fn count(msg: &skse::Message) {
            SEEN.fetch_add(msg.kind, Ordering::Relaxed);
        }
        let send = |sender: &CStr, kind| {
            let mut msg = skse::Message { sender: sender.as_ptr(), kind, data_len: 0, data: core::ptr::null_mut() };
            unsafe { on_plugin_dispatch(&raw mut msg) };
        };
        on_plugin_message("listener", "hdtsmp64", count);
        send(c"HDTSMP64", 1);
        send(c"other", 10);
        assert_eq!(SEEN.load(Ordering::Relaxed), 1);
        assert_eq!(PLUGIN_MESSAGES.remove_owner("listener"), 1);
        send(c"hdtsmp64", 100);
        assert_eq!(SEEN.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn player_graph_handlers_run_in_order_and_go_with_their_plugin() {
        static SEEN: AtomicU32 = AtomicU32::new(0);
        let record = |bit: u32, control: BSEventNotifyControl| -> ErasedHandler {
            Box::new(move |_, _| {
                assert_eq!(SEEN.fetch_or(bit, Ordering::Relaxed) & !(bit - 1), 0, "a later handler ran first");
                control
            })
        };
        add_player_graph_sink("graph-a", record(1, BSEventNotifyControl::Continue));
        add_player_graph_sink("graph-b", record(2, BSEventNotifyControl::Stop));
        add_player_graph_sink("graph-a", record(4, BSEventNotifyControl::Continue));
        assert!(PLAYER_GRAPH_SINK.get().is_some());
        assert_eq!(dispatch(&PLAYER_GRAPH, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Stop);
        assert_eq!(SEEN.swap(0, Ordering::Relaxed), 3, "b stops the walk before a's second handler");
        assert!(detach("graph-b").contains(" 1 sinks"));
        assert_eq!(dispatch(&PLAYER_GRAPH, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Continue);
        assert_eq!(SEEN.swap(0, Ordering::Relaxed), 5);
        assert!(detach("graph-a").contains(" 2 sinks"));
        assert_eq!(dispatch(&PLAYER_GRAPH, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Continue);
        assert_eq!(SEEN.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn reads_a_setting() {
        let ini = "; host settings\n[hooks]\nCompat = stack ; opt in\nother=1\n";
        assert_eq!(setting(ini, "compat"), Some("stack"));
        assert_eq!(setting(ini, "missing"), None);
        assert_eq!(setting("# compat = stack", "compat"), None);
    }
}
