// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The host's side: [`load`] is `skyrim_runtime_extender.dll`'s `SKSEPlugin_Load`. The host owns what the
//! engine and the loader keep for every plugin (one message listener and one Papyrus callback,
//! the plugins' tabs in its window, one sink per event source, the native function objects, the
//! console entries and the hook sites) and tags each registration with its plugin, so `reload`
//! can detach a plugin and unload its DLL. It keeps every plugin's settings, its own in
//! `skyrim-runtime-extender.settings.json` (the capture rule and every hotkey of the binding table `keys`),
//! draws the overlay's one window (`gui`) and answers the console command `SkyrimRs` (`console`).
//! It times the startup for the window's Debug tab (`profile`).

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use skyrim::events::{ErasedSink, ErasedSource};
use skyrim::prelude::*;
use sre::BSEventNotifyControl;
use skyrim::input::{CommandSlot, ErasedCommand, ErasedExecute, ReplacedCommand};
use skyrim::papyrus::{ErasedNative, ErasedNativeFn, NativeFunction, VirtualMachine};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{DONT_RESOLVE_DLL_REFERENCES, GetModuleHandleW, GetProcAddress, LoadLibraryExW, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use crate::abi::{self, API, FrameCallback, MessageCallback, PapyrusCallback, SinkCallback};
use crate::hooks::{Compat, Hooks};
use crate::order::{self, Constraints};
use crate::settings;
use crate::{ABI, CommandRequest, Descriptor, Error, NativeRequest, Plugin};

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
    ($($arg:tt)*) => {
        $crate::host::log_line(format_args!($($arg)*))
    };
}

mod alias;
mod api;
mod console;
mod filters;
mod gui;
mod interface;
mod keys;
mod owned;
mod profile;
mod reload;
mod support;
mod system;

use owned::Owned;

/// The folder under `Data\SKSE\Plugins` the host loads plugins from.
pub const PLUGIN_DIR: &str = NAME;
/// The host's settings in `Data\SKSE\Plugins`: `compat = stack` (the default) or `refuse`, see [`Compat`].
pub const CONFIG: &str = "skyrim-runtime-extender.ini";
/// The host's own name, as the owner of its window and settings.
pub const NAME: &str = "skyrim-runtime-extender";
/// What the folder, the settings owner and the DLL were called before the rename.
const OLD_NAME: &str = "skyrim-rs";
const OLD_DLL: &str = "skyrim_rs.dll";
/// The framework's version, as the log and the crash log show it.
const VERSION: &str = crate::version!();

/// The accepted plugins in load order; a rescan appends the ones it loads.
static PLUGINS: Mutex<&'static [Plugin]> = Mutex::new(&[]);
/// The DLLs in the plugin folder the host did not load, each with the reason.
static REFUSED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static MESSAGES: Owned<MessageCallback> = Owned::new();
/// Handlers of other SKSE plugins' messages, each with the sender it listens to.
static PLUGIN_MESSAGES: Owned<(String, MessageCallback)> = Owned::new();
static FRAMES: Owned<FrameCallback> = Owned::new();
static PAPYRUS: Owned<PapyrusCallback> = Owned::new();
static SINKS: Mutex<Vec<SourceSinks>> = Mutex::new(Vec::new());
static PLAYER_GRAPH: Owned<SinkCallback> = Owned::new();
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

/// The dispatcher as the host reaches it: through a table of its own, whose empty name lets it
/// act for any owner.
pub(crate) fn hooks() -> &'static Hooks {
    static HOOKS: OnceLock<Hooks> = OnceLock::new();
    HOOKS.get_or_init(|| Hooks::new(api::build("", core::ptr::null())))
}

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
    handlers: &'static Owned<SinkCallback>,
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
/// Papyrus callback, then loads every plugin in `Data\SKSE\Plugins\skyrim-runtime-extender\` from a shadow copy
/// in the order their descriptors resolve to, and watches their files from then on.
pub fn load(skse: &skse::Interface) -> bool {
    // The loader hands every plugin the same object in its read-only data, alive for the whole game.
    let skse: &'static skse::Interface = SKSE.get_or_init(|| unsafe { &*core::ptr::from_ref(skse) });
    if let Ok(dir) = skse::log::dir(skse.runtime_version) {
        let _ = LOG_DIR.set(dir);
    }
    if let Ok(file) = skse::log::open(skse.runtime_version, NAME) {
        *LOG.lock().unwrap() = Some(file);
    }
    skse::log::log_panics(skse.runtime_version, NAME, NAME);
    log!("framework {VERSION}, abi {ABI}, api {API}");
    profile::step("host loaded");
    let old_dll: Vec<u16> = OLD_DLL.encode_utf16().chain(Some(0)).collect();
    if !unsafe { GetModuleHandleW(old_dll.as_ptr()) }.is_null() {
        log!("{OLD_DLL} is the old name of this loader and is loaded too; remove it. This one does not start.");
        return false;
    }
    let hosted_by = if skse::runtime::script_extender_loaded(skse.runtime_version) { "the script extender" } else { "the standalone loader" };
    let _ = HOSTED_BY.set(hosted_by);
    log!("hosted by {hosted_by}, plugin interface {:08x}, runtime {:08x}", skse.skse_version, skse.runtime_version);
    // The loader unloads a DLL whose load returns false, so nothing of it may outlive that: this
    // check comes before the crash handler's thread and exception handler.
    if let Err(e) = skyrim::init(skse) {
        log!("game layer: {e}");
        log!("nothing was patched and no plugin was loaded");
        return false;
    }
    if let Err(e) = profile::time_data_files() {
        log!("load profile: data files not timed: {e}");
    }
    let symbol_path = [plugins_dir(), plugins_dir().join(PLUGIN_DIR), plugins_dir().join(PLUGIN_DIR).join(reload::SHADOW_DIR)].iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(";");
    match crash::install(&crash::Options { name: NAME, runtime_version: skse.runtime_version, build: VERSION, facts: Some(crash_facts), symbol_path: Some(symbol_path) }) {
        Ok(crash::Installed::Here { watchdog, symbols, symbol_mismatches, exits }) => {
            log!("crash handler installed, watchdog thread {watchdog}, game symbols {symbols:?} ({} functions, {symbol_mismatches} offsets differ from the ledger)", crash::symbols::count());
            if let Err(e) = exits {
                log!("crash handler: the game's end is not logged: {e}");
            }
        }
        Ok(crash::Installed::Elsewhere) => log!("crash handler: another copy in the process reports crashes"),
        Err(e) => log!("crash handler not installed: {e}"),
    }
    Hooks::set_logger(|line| log!("hooks: {line}"));
    let (compat, source) = match std::fs::read_to_string(plugins_dir().join(CONFIG)).ok().as_deref().and_then(|text| setting(text, "compat")).map(str::parse::<Compat>) {
        Some(Ok(compat)) => (compat, CONFIG.to_owned()),
        Some(Err(e)) => (Compat::Refuse, format!("{CONFIG}: {e}")),
        None => (Compat::default(), "default".to_owned()),
    };
    log!("hooks: compatibility mode {compat} ({source})");
    Hooks::set_compat(compat);
    Hooks::revalidate("startup");
    let dir = plugins_dir().join(PLUGIN_DIR);
    if !settings::file(&dir, NAME).exists() {
        adopt_old_folder(&plugins_dir().join(OLD_NAME), &dir);
    }
    open_settings(NAME, &dir);
    keys::declare_host();
    match overlay::install() {
        Ok(placement) => log!("overlay installed {placement}, toggled with {}, closed with {}", keys::show(keys::key(keys::TOGGLE)), keys::show(keys::key(keys::CLOSE))),
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
    let sorted = read_plugins(plugin_files(&dir), &shadow, &[]);
    log!("load order: {}", sorted.iter().map(|(_, _, p, _, _)| if p.constraints == Constraints::default() { p.name.clone() } else { format!("{} ({})", p.name, p.constraints) }).collect::<Vec<_>>().join(", "));
    for (rank, (_, _, plugin, _, _)) in sorted.iter().enumerate() {
        Hooks::set_rank(&plugin.name, i32::try_from(rank).unwrap_or(i32::MAX));
    }
    *PLUGINS.lock().unwrap_or_else(PoisonError::into_inner) = Box::leak(sorted.iter().map(|(_, _, p, _, _)| p.clone()).collect());
    let loaded = sorted.into_iter().map(|read| start_plugin(skse, &dir, read)).collect();
    reload::watch(loaded);
    overlay::on_frame(frame);
    true
}

/// A plugin file read for loading: its module, descriptor, what it declares, its file and its shadow copy.
type Read = (HMODULE, &'static Descriptor, Plugin, PathBuf, PathBuf);

/// Copies each of `files` into `shadow` and reads it, then orders what was read by the descriptors'
/// constraints. Refused: a file that cannot be read, one named as the host or as a plugin in
/// `running`, and those in a load order cycle.
fn read_plugins(files: Vec<PathBuf>, shadow: &Path, running: &[Plugin]) -> Vec<Read> {
    let mut read = Vec::new();
    for path in files {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let loaded = reload::copy(&path, shadow, 1).map_err(|e| format!("copy: {e}")).and_then(|copy| read_checked(&copy, &file).map(|(module, descriptor)| (module, descriptor, copy)));
        match loaded {
            Ok((module, descriptor, _)) if plugin_name(descriptor) == NAME => {
                refuse(file, format!("{NAME} is the host's own name, which its settings file and keys use"));
                unsafe { FreeLibrary(module) };
            }
            Ok((module, descriptor, _)) if let Some(p) = running.iter().find(|p| p.name == plugin_name(descriptor)) => {
                refuse(file, format!("{} is loaded from {}", p.name, p.file));
                unsafe { FreeLibrary(module) };
            }
            Ok((module, descriptor, copy)) => read.push(Some((module, descriptor, describe(descriptor, file), path, copy))),
            Err(e) => refuse(file, e),
        }
    }
    let items: Vec<(&str, &Constraints)> = read.iter().flatten().map(|(_, _, p, _, _)| (p.name.as_str(), &p.constraints)).collect();
    let order = order::sort(&items);
    for (asker, name) in order.missing.iter().filter(|(_, name)| !running.iter().any(|p| p.name == *name)) {
        log!("no plugin named {name} (asked by {})", items[*asker].0);
    }
    let in_cycle: Vec<String> = order.cycle.iter().map(|&i| items[i].0.to_owned()).collect();
    for &i in &order.cycle {
        if let Some((module, _, plugin, _, _)) = read[i].take() {
            refuse(plugin.file.clone(), format!("{} is in a load order cycle with {}", plugin.name, in_cycle.iter().filter(|n| **n != plugin.name).cloned().collect::<Vec<_>>().join(", ")));
            unsafe { FreeLibrary(module) };
        }
    }
    order.sorted.iter().filter_map(|&i| read[i].take()).collect()
}

/// Starts a read plugin with its own [`abi::HostApi`] and settings, as generation 1.
fn start_plugin(skse: &'static skse::Interface, dir: &Path, (module, descriptor, plugin, source, copy): Read) -> reload::Loaded {
    let name: &'static str = Box::leak(plugin.name.clone().into_boxed_str());
    let api = api::build(name, skse);
    open_settings(name, dir);
    if let Err(e) = skse::log::open(skse.runtime_version, name) {
        log!("{name}: could not clear plugin log: {e}");
    }
    crash::note_image(&plugin.name, 1, module as usize, true);
    let live = profile::framework_load(&plugin.file, || (descriptor.load)(api));
    if live {
        log!("loaded {}: {}", plugin.file, plugin.name);
        log!("{}: generation 1 loaded, image at {:#x}", plugin.name, module as usize);
    } else {
        log!("skipped {}: {}: load returned false", plugin.file, plugin.name);
        detach(name);
    }
    reload::Loaded::new(plugin, api, module, descriptor, live, source, copy)
}

/// Loads every DLL in the plugin folder that is not loaded (files added since the game started, and
/// the ones refused, tried again), ordered among themselves, after the running plugins.
fn rescan(loaded: &mut Vec<reload::Loaded>) {
    let Some(skse) = SKSE.get() else { return };
    let dir = plugins_dir().join(PLUGIN_DIR);
    let files: Vec<PathBuf> = plugin_files(&dir).into_iter().filter(|f| !loaded.iter().any(|l| l.source() == f)).collect();
    REFUSED.lock().unwrap_or_else(PoisonError::into_inner).clear();
    let running = plugins();
    let sorted = read_plugins(files, &dir.join(reload::SHADOW_DIR), running);
    log!("rescan: {}", if sorted.is_empty() { "no plugin to load".to_owned() } else { format!("loading {} after the running plugins", sorted.iter().map(|(_, _, p, _, _)| p.name.as_str()).collect::<Vec<_>>().join(", ")) });
    for (rank, (_, _, plugin, _, _)) in sorted.iter().enumerate() {
        Hooks::set_rank(&plugin.name, i32::try_from(running.len() + rank).unwrap_or(i32::MAX));
    }
    *PLUGINS.lock().unwrap_or_else(PoisonError::into_inner) = Box::leak(running.iter().chain(sorted.iter().map(|(_, _, p, _, _)| p)).cloned().collect());
    loaded.extend(sorted.into_iter().map(|read| start_plugin(skse, &dir, read)));
}

/// The overlay's frame callback: reloads first, then every plugin's [`Host::on_frame`].
fn frame() {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| profile::step("first frame drawn"));
    reload::tick();
    follow_player_graph();
    FRAMES.each(|f| {
        (f.call)(f.ctx);
        true
    });
}

/// Whether the loader's `DATA_LOADED` message has arrived (the control map is loaded by then).
fn data_loaded() -> bool {
    STARTUP.lock().unwrap_or_else(PoisonError::into_inner).contains(&skse::MessagingInterface::DATA_LOADED)
}

/// Reads a plugin's settings file and logs what came of it.
/// Copies what `old` holds into `to`, with the host's settings under their new names; plugin DLLs
/// are only logged, since `to` is where a plugin's package installs it.
fn adopt_old_folder(old: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(old) else { return };
    let _ = std::fs::create_dir_all(to);
    let old_settings = settings::file(Path::new(""), OLD_NAME);
    for entry in entries.flatten() {
        let (path, name) = (entry.path(), entry.file_name());
        if path.is_dir() {
            if name != reload::SHADOW_DIR {
                adopt_old_folder(&path, &to.join(&name));
            }
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
            log!("{} stays unloaded in the old folder; install it again", path.display());
        } else if Path::new(&name) == old_settings {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = std::fs::write(settings::file(to, NAME), text.replace(&format!("\"key.{OLD_NAME}."), &format!("\"key.{NAME}.")));
            }
        } else if !to.join(&name).exists() {
            let _ = std::fs::copy(&path, to.join(&name));
        }
    }
}

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
    *PLUGINS.lock().unwrap_or_else(PoisonError::into_inner)
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

/// Checks the DLL at `path`, a copy of the plugin file `file`, against the running game from its
/// bytes alone, and only then loads it.
fn read_checked(path: &Path, file: &str) -> Result<(HMODULE, &'static Descriptor), String> {
    let pe = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    support::note(file, &pe);
    let running = versionlib::running().map_err(|e| e.to_string())?;
    match running.refusal(&pe) {
        Some(why) => Err(why),
        None => read_descriptor(path),
    }
}

/// Whether this host knows the descriptor's layout. A stage 1 to 4 plugin of ABI 5 has the same
/// layout with its build string in `build`, and still crosses Rust-ABI types this host no longer has.
fn accept(d: &Descriptor) -> Result<(), String> {
    if d.abi != ABI {
        return Err(format!("descriptor abi {}, the host's is {ABI}", d.abi));
    }
    if !d.build.is_null() {
        return Err(format!("built as \"{}\", before the plugin ABI was stable: rebuild it against this framework", unsafe { CStr::from_ptr(d.build) }.to_string_lossy()));
    }
    Ok(())
}

/// Loads the DLL and checks its descriptor's head; a refused DLL is unloaded again. A plugin built
/// against a later [`abi::HostApi`] is accepted and logged: what this loader lacks reports `Unsupported`.
fn read_descriptor(path: &Path) -> Result<(HMODULE, &'static Descriptor), String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("LoadLibrary: {}", std::io::Error::last_os_error()));
    }
    let descriptor = unsafe { GetProcAddress(module, c"SkyrimRs_Plugin".as_ptr().cast()) }.map(|p| unsafe { &*(p as *const Descriptor) });
    let checked = descriptor.ok_or_else(|| "no SkyrimRs_Plugin export".to_owned()).and_then(|d| accept(d).map(|()| d));
    match checked {
        Ok(d) => {
            if d.api > API {
                log!("{}: built for api {}, the loader has {API}: functions after {API} report Unsupported", path.display(), d.api);
            }
            Ok((module, d))
        }
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
            let _ = writeln!(w, "hosted by {}; framework {VERSION}", HOSTED_BY.get().unwrap_or(&"an unknown loader"));
            match reload::try_generations() {
                Some(generations) if generations.is_empty() => {
                    let _ = writeln!(w, "framework plugins: none");
                }
                Some(generations) => {
                    let _ = write!(w, "framework plugins:");
                    for (name, generation, status) in generations {
                        let state = match status {
                            reload::Status::Running => "",
                            reload::Status::Stopped => " (stopped)",
                            reload::Status::Unloaded => " (unloaded)",
                        };
                        let _ = write!(w, " {name} generation {generation}{state};");
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
    let version = if descriptor.version.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.version) }.to_string_lossy().into_owned() };
    let (before, after) = unsafe { (abi::names(descriptor.load_before), abi::names(descriptor.load_after)) };
    Plugin { name: plugin_name(descriptor), version, file, constraints: Constraints { before, after, priority: descriptor.priority } }
}

/// Removes every registration of `owner` and returns what went, with every call inside them
/// drained. Each registry's lock is released before the drain, so a callback still running
/// may register through the host while it is waited for.
fn detach(owner: &str) -> String {
    let hooks = hooks().remove_owner(owner);
    let sources: Vec<&'static Owned<SinkCallback>> = SINKS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|s| s.handlers).collect();
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
            profile::step("all SKSE plugins loaded");
            match skse::timing::table() {
                Some(Ok(table)) => log!("SKSE plugin timing: {}", table.status()),
                Some(Err(version)) => log!("SKSE plugin timing: {} has table version {version}, this host reads {}", skse::timing::FILE, skse::timing::VERSION),
                None => log!("SKSE plugin timing: {} is not installed", skse::timing::FILE),
            }
            snapshot_skse_plugins();
            alias::resolve_waiting();
        }
        skse::MessagingInterface::PLUGINS_LOADED_2 => Hooks::revalidate("PLUGINS_LOADED_2"),
        skse::MessagingInterface::DATA_LOADED => {
            profile::step("data files loaded");
            profile::finish_data_files();
            log!("load profile: {}", profile::summary());
            Hooks::revalidate("DATA_LOADED");
        }
        skse::MessagingInterface::CONTROLS_READY => profile::step("controls ready"),
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
        (f.call)(f.ctx, msg);
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
        (f.call)(f.ctx, vm);
        true
    });
}

fn on_message(owner: &'static str, f: MessageCallback) {
    let (call, ctx) = (f.call, f.ctx);
    MESSAGES.add(owner, f);
    let seen = STARTUP.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for kind in seen {
        call(ctx, &skse::Message { sender: c"SKSE".as_ptr(), kind, data_len: 0, data: core::ptr::null_mut() });
    }
}

fn on_plugin_message(owner: &'static str, sender: &str, f: MessageCallback) {
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
            (f.call)(f.ctx, msg);
        }
        true
    });
}

fn serve_alias(owner: &'static str, name: &str, f: MessageCallback) -> Result<usize, Error> {
    let started = STARTUP.lock().unwrap_or_else(PoisonError::into_inner).contains(&skse::MessagingInterface::PLUGINS_LOADED);
    alias::serve(owner, name, f, started)
}

fn on_papyrus(owner: &'static str, f: PapyrusCallback) {
    let (call, ctx) = (f.call, f.ctx);
    PAPYRUS.add(owner, f);
    if let Some(vm) = vm() {
        call(ctx, vm);
    }
}

/// Runs `handlers` in registration order until one returns `Stop`.
fn dispatch(handlers: &Owned<SinkCallback>, event: *const c_void, source: *const sre::BSTEventSource) -> BSEventNotifyControl {
    let mut control = BSEventNotifyControl::Continue;
    handlers.each(|f| {
        control = abi::control((f.call)(f.ctx, event, source));
        control == BSEventNotifyControl::Continue
    });
    control
}

fn add_sink(owner: &'static str, source: ErasedSource, handler: SinkCallback) {
    let mut sinks = SINKS.lock().unwrap_or_else(PoisonError::into_inner);
    let handlers = if let Some(s) = sinks.iter().find(|s| s.source == source) {
        s.handlers
    } else {
        let handlers: &'static Owned<SinkCallback> = Box::leak(Box::new(Owned::new()));
        source.add_sink(ErasedSink::new(Box::new(move |event, source| dispatch(handlers, event, source))));
        sinks.push(SourceSinks { source, handlers });
        handlers
    };
    handlers.add(owner, handler);
    log!("{owner}: sink added on the source at {:#x}", source.address());
}

fn add_player_graph_sink(owner: &'static str, handler: SinkCallback) {
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

fn register_native(owner: &'static str, request: &NativeRequest<'_>, f: ErasedNativeFn) -> Result<&'static NativeFunction, Error> {
    let mut natives = NATIVES.lock().unwrap_or_else(PoisonError::into_inner);
    let what = format!("{}.{}", request.class, request.name);
    if let Some(existing) = natives.iter().find(|n| n.class.eq_ignore_ascii_case(request.class) && n.name.eq_ignore_ascii_case(request.name)) {
        if existing.owner != owner {
            return Err(Error::Taken { what, owner: existing.owner.to_owned() });
        }
        existing.erased.set(f);
        log!("{owner}: {what} re-pointed");
        return Ok(existing.erased.function());
    }
    let native = request.vm.register_erased(request.class, request.name, request.kind, request.params, request.ret, f)?;
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

/// One line per event source the plugins' sinks are on, and one for the player's graph, with the
/// owner of each handler in the order they run, for the console inspector.
fn sink_lines() -> Vec<String> {
    let line = |what: String, owners: Vec<&str>| (!owners.is_empty()).then(|| format!("sink on {what}: {}", owners.join(", ")));
    let mut lines: Vec<String> = SINKS.lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(|s| line(format!("the source at {:#x}", s.source.address()), s.handlers.owners())).collect();
    lines.extend(line("the player's graph".to_owned(), PLAYER_GRAPH.owners()));
    lines
}

/// One line per command a plugin registered or replaced and per Papyrus native it bound, for the
/// console inspector.
fn command_lines() -> Vec<String> {
    let mut lines: Vec<String> = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|c| format!("command {} ({}): opcode {:#x}, in place of the unused {}", c.name, c.owner, c.erased.slot().opcode, c.erased.slot().replaced)).collect();
    lines.extend(REPLACED.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|c| format!("command {} ({}): opcode {:#x}, the engine's execute {}", c.name, c.owner, c.replaced.slot().opcode, if c.replaced.active() { "replaced" } else { "restored" })));
    lines.extend(NATIVES.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|n| format!("native {}.{} ({})", n.class, n.name, n.owner)));
    lines
}

fn register_command(owner: &'static str, request: &CommandRequest<'_>, execute: ErasedExecute) -> Result<CommandSlot, Error> {
    let mut commands = COMMANDS.lock().unwrap_or_else(PoisonError::into_inner);
    let name = request.name.to_string_lossy().into_owned();
    if let Some(existing) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(&name)) {
        if existing.owner != owner {
            return Err(Error::Taken { what: name, owner: existing.owner.to_owned() });
        }
        existing.erased.set(execute);
        log!("{owner}: command {name} re-pointed");
        return Ok(existing.erased.slot());
    }
    let command = skyrim::input::register_erased(request.name, request.help, request.params, request.needs_reference, execute)?;
    let slot = command.slot();
    log!("{owner}: command {name} took over console opcode {:#x} ({})", slot.opcode, slot.replaced);
    commands.push(ConsoleCommand { name, owner, erased: command });
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_folder_is_adopted_with_the_new_key_names() {
        let root = std::env::temp_dir().join(format!("skyrim-rs-adopt-{}", std::process::id()));
        let (old, new) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(old.join("improved-camera")).unwrap();
        std::fs::create_dir_all(old.join(reload::SHADOW_DIR)).unwrap();
        std::fs::write(settings::file(&old, OLD_NAME), "{\"key.skyrim-rs.toggle\": \"\", \"key.skyrim-rs-example.a\": \"K\"}").unwrap();
        std::fs::write(old.join("improved-camera").join("profile.ini"), "kept").unwrap();
        std::fs::write(old.join("a.dll"), "code").unwrap();
        std::fs::write(old.join(reload::SHADOW_DIR).join("b.txt"), "shadow").unwrap();
        adopt_old_folder(&old, &new);
        let adopted = std::fs::read_to_string(settings::file(&new, NAME)).unwrap();
        assert_eq!(adopted, "{\"key.skyrim-runtime-extender.toggle\": \"\", \"key.skyrim-rs-example.a\": \"K\"}");
        assert_eq!(std::fs::read_to_string(new.join("improved-camera").join("profile.ini")).unwrap(), "kept");
        assert!(!new.join("a.dll").exists() && !new.join(reload::SHADOW_DIR).exists() && !settings::file(&new, OLD_NAME).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    extern "C" fn never(_: *const abi::HostApi) -> bool {
        false
    }

    fn descriptor(abi: u32, api: u32, build: *const core::ffi::c_char) -> Descriptor {
        Descriptor { abi, api, build, name: c"d".as_ptr(), version: c"1".as_ptr(), load: never, load_before: core::ptr::null(), load_after: core::ptr::null(), priority: 0, unload: None, save_state: None, restore_state: None }
    }

    #[test]
    fn accepts_a_known_layout_with_any_api_and_refuses_a_build_string() {
        assert!(accept(&descriptor(ABI, API, core::ptr::null())).is_ok());
        assert!(accept(&descriptor(ABI, API + 3, core::ptr::null())).is_ok(), "a later api is only logged");
        assert!(accept(&descriptor(ABI, 1, core::ptr::null())).is_ok());
        assert!(accept(&descriptor(ABI + 1, API, core::ptr::null())).is_err());
        let stage_4 = accept(&descriptor(ABI, 4, c"framework 0.1.0 0123456789abcdef, rustc 1.99.0".as_ptr())).unwrap_err();
        assert!(stage_4.contains("\"framework 0.1.0 0123456789abcdef, rustc 1.99.0\"") && stage_4.contains("rebuild it"), "{stage_4}");
    }

    /// Plugin DLLs built elsewhere, read by the loader's own code (`docs/subsystems/abi.md`,
    /// "Proof"): `SKYRIM_RS_ABI_PLUGIN` must be accepted and `SKYRIM_RS_ABI_REFUSED` refused.
    #[test]
    fn reads_built_plugins() {
        match std::env::var_os("SKYRIM_RS_ABI_PLUGIN") {
            Some(path) => {
                let (module, d) = read_descriptor(Path::new(&path)).unwrap_or_else(|e| panic!("{} refused: {e}", path.display()));
                let plugin = describe(d, String::new());
                println!("accepted {}: {} {}, abi {}, api {}, {}", path.display(), plugin.name, plugin.version, d.abi, d.api, plugin.constraints);
                assert!(!(d.load)(api::build("abi-built-plugin", core::ptr::null())), "the plugin's entry point reads this loader's table and stops without the SKSE interface");
                unsafe { FreeLibrary(module) };
            }
            None => eprintln!("SKYRIM_RS_ABI_PLUGIN not set: no built plugin read"),
        }
        if let Some(path) = std::env::var_os("SKYRIM_RS_ABI_REFUSED") {
            let refused = read_descriptor(Path::new(&path)).map(|_| ()).expect_err("refused");
            println!("refused {}: {refused}", path.display());
        }
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
        on_plugin_message("listener", "hdtsmp64", abi::message(count));
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
        let record = |bit: u32, control: BSEventNotifyControl| {
            abi::sink(move |_, _| {
                assert_eq!(SEEN.fetch_or(bit, Ordering::Relaxed) & !(bit - 1), 0, "a later handler ran first");
                control
            })
        };
        add_player_graph_sink("graph-a", record(1, BSEventNotifyControl::Continue));
        add_player_graph_sink("graph-b", record(2, BSEventNotifyControl::Stop));
        add_player_graph_sink("graph-a", record(4, BSEventNotifyControl::Continue));
        assert!(PLAYER_GRAPH_SINK.get().is_some());
        assert!(sink_lines().contains(&"sink on the player's graph: graph-a, graph-b, graph-a".to_owned()));
        assert_eq!(dispatch(&PLAYER_GRAPH, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Stop);
        assert_eq!(SEEN.swap(0, Ordering::Relaxed), 3, "b stops the walk before a's second handler");
        assert!(detach("graph-b").contains(" 1 sinks"));
        assert_eq!(dispatch(&PLAYER_GRAPH, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Continue);
        assert_eq!(SEEN.swap(0, Ordering::Relaxed), 5);
        assert!(detach("graph-a").contains(" 2 sinks"));
        assert!(!sink_lines().iter().any(|l| l.contains("player's graph")));
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
