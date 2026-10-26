//! The host's side: [`load`] is `skyrim_rs.dll`'s `SKSEPlugin_Load`. The host owns what the
//! engine and the loader keep for every plugin (one message listener and one Papyrus callback,
//! one overlay window, one sink per event source, the native function objects, the console
//! entries and the hook sites) and tags each registration with its plugin, so `reload` can
//! detach a plugin and unload its DLL.

use core::ffi::{CStr, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use skyrim::events::{BSEventNotifyControl, ErasedHandler, ErasedSink, ErasedSource};
use skyrim::input::{CommandSlot, ErasedCommand};
use skyrim::papyrus::{ErasedNative, NativeFunction, VirtualMachine};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::hooks::{Compat, Hooks};
use crate::order::{self, Constraints};
use crate::{ABI, BUILD, CommandRequest, Descriptor, Error, Host, MessageFn, NativeRequest, PapyrusFn, Plugin, Table, Window};

static LOG: Mutex<Option<File>> = Mutex::new(None);

fn log_line(line: core::fmt::Arguments<'_>) {
    if let Some(f) = LOG.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(f, "{line}");
    }
}

macro_rules! log {
    ($($arg:tt) => {
        $crate::host::log_line(format_args!($($arg))
    };
}

mod owned;
mod reload;

use owned::Owned;

/// The folder under `Data\SKSE\Plugins` the host loads plugins from.
pub const PLUGIN_DIR: &str = "skyrim-rs";
/// The host's settings in `Data\SKSE\Plugins`: `compat = refuse` (the default) or `stack`, see [`Compat`].
pub const CONFIG: &str = "skyrim-rs.ini";

static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();
static MESSAGES: Owned<MessageFn> = Owned::new();
static PAPYRUS: Owned<PapyrusFn> = Owned::new();
static WINDOWS: Owned<PluginWindow> = Owned::new();
static SINKS: Mutex<Vec<SourceSinks>> = Mutex::new(Vec::new());
static NATIVES: Mutex<Vec<Native>> = Mutex::new(Vec::new());
static COMMANDS: Mutex<Vec<ConsoleCommand>> = Mutex::new(Vec::new());
/// The startup message kinds seen so far, replayed to a handler registered after them.
static STARTUP: Mutex<Vec<u32>> = Mutex::new(Vec::new());
/// The VM once the Papyrus registration pass has run.
static VM: AtomicUsize = AtomicUsize::new(0);
static HOOKS: Hooks = Hooks::HOST;
static TABLE: Table = Table { on_message, on_papyrus, add_window, add_sink, register_native, register_command, plugins };

/// A plugin's window; only the main thread runs it, under the entry's exclusive lock.
struct PluginWindow(Window);

unsafe impl Sync for PluginWindow {}

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

/// Initializes the game layer, installs the overlay, registers the host's message listener and
/// Papyrus callback, then loads every plugin in `Data\SKSE\Plugins\skyrim-rs\` from a shadow copy
/// in the order their descriptors resolve to, and watches their files from then on.
pub fn load(skse: &skse::Interface) -> bool {
    // The loader hands every plugin the same object in its read-only data, alive for the whole game.
    let skse: &'static skse::Interface = unsafe { &*core::ptr::from_ref(skse) };
    if let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs") {
        *LOG.lock().unwrap() = Some(file);
    }
    log!("{}, abi {ABI}", BUILD.to_string_lossy());
    let hosted_by = if skse::runtime::script_extender_loaded(skse.runtime_version) { "the script extender" } else { "the standalone loader" };
    log!("hosted by {hosted_by}, plugin interface {:08x}, runtime {:08x}", skse.skse_version, skse.runtime_version);
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
    match overlay::install(overlay::DEFAULT_TOGGLE) {
        Ok(()) => log!("overlay installed, toggled with scan code {:#x}", overlay::DEFAULT_TOGGLE),
        Err(e) => log!("overlay: {e}"),
    }
    let (Some(messaging), Some(papyrus)) = (skse.messaging(), skse.papyrus()) else {
        log!("no messaging or Papyrus interface");
        return false;
    };
    if !messaging.listen_skse(skse.plugin_handle(), on_skse_message) || !papyrus.register(on_papyrus_pass) {
        log!("the loader refused the message listener or the Papyrus callback");
        return false;
    }
    overlay::add(|ctx| WINDOWS.each_mut(|window| (window.0)(ctx)));
    let dir = plugins_dir().join(PLUGIN_DIR);
    let shadow = reload::clean(&dir);
    let mut read = Vec::new();
    for path in plugin_files(&dir) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let loaded = reload::copy(&path, &shadow, 1).map_err(|e| format!("copy: {e}")).and_then(|copy| read_descriptor(&copy).map(|(module, descriptor)| (module, descriptor, copy)));
        match loaded {
            Ok((module, descriptor, copy)) => read.push((module, descriptor, describe(descriptor, file), path, copy)),
            Err(e) => log!("skipped {file}: {e}"),
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
        log!("skipped {}: {} is in a load order cycle with {}", plugin.file, plugin.name, in_cycle.iter().filter(|n| **n != plugin.name).copied().collect::<Vec<_>>().join(", "));
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
    overlay::on_frame(reload::tick);
    true
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

fn plugin_name(descriptor: &Descriptor) -> String {
    if descriptor.name.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.name) }.to_string_lossy().into_owned() }
}

fn describe(descriptor: &Descriptor, file: String) -> Plugin {
    let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    Plugin { name: plugin_name(descriptor), file, constraints: Constraints { before: owned(descriptor.load_before), after: owned(descriptor.load_after), priority: descriptor.priority } }
}

/// Removes every registration of `owner` and returns what went, with every call inside them
/// drained. Each registry's lock is released before the drain, so a callback still running
/// may register through the host while it is waited for.
fn detach(owner: &str) -> String {
    let hooks = HOOKS.remove_owner(owner);
    let sources: Vec<&'static Owned<ErasedHandler>> = SINKS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|s| s.handlers).collect();
    let sinks: usize = sources.iter().map(|handlers| handlers.remove_owner(owner)).sum();
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
    let (natives, commands) = (natives.len(), commands.len());
    let windows = WINDOWS.remove_owner(owner);
    if windows > 0 {
        overlay::reset_context();
    }
    let messages = MESSAGES.remove_owner(owner);
    let papyrus = PAPYRUS.remove_owner(owner);
    format!("{hooks} hook subscriptions, {sinks} sinks, {natives} natives stubbed, {commands} commands stubbed, {windows} windows, {messages} message handlers, {papyrus} Papyrus callbacks")
}

fn vm() -> Option<&'static VirtualMachine> {
    let vm = VM.load(Ordering::Acquire);
    (vm != 0).then(|| unsafe { &*(vm as *const VirtualMachine) })
}

unsafe extern "C" fn on_skse_message(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    match msg.kind {
        skse::MessagingInterface::PLUGINS_LOADED_2 => Hooks::revalidate("PLUGINS_LOADED_2"),
        skse::MessagingInterface::DATA_LOADED => Hooks::revalidate("DATA_LOADED"),
        _ => {}
    }
    if matches!(msg.kind, skse::MessagingInterface::PLUGINS_LOADED | skse::MessagingInterface::PLUGINS_LOADED_2 | skse::MessagingInterface::CONTROLS_READY | skse::MessagingInterface::DATA_LOADED) {
        STARTUP.lock().unwrap_or_else(PoisonError::into_inner).push(msg.kind);
    }
    MESSAGES.each(|f| {
        f(msg);
        true
    });
}

unsafe extern "C" fn on_papyrus_pass(registry: *mut c_void) {
    let Some(vm) = (unsafe { VirtualMachine::from_ptr(registry) }) else {
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

fn on_papyrus(owner: &'static str, f: PapyrusFn) {
    PAPYRUS.add(owner, f);
    if let Some(vm) = vm() {
        f(vm);
    }
}

fn add_window(owner: &'static str, window: Window) {
    WINDOWS.add(owner, PluginWindow(window));
}

fn add_sink(owner: &'static str, source: ErasedSource, handler: ErasedHandler) {
    let mut sinks = SINKS.lock().unwrap_or_else(PoisonError::into_inner);
    let handlers = if let Some(s) = sinks.iter().find(|s| s.source == source) {
        s.handlers
    } else {
        let handlers: &'static Owned<ErasedHandler> = Box::leak(Box::new(Owned::new()));
        source.add_sink(ErasedSink::new(Box::new(move |event, source| {
            let mut control = BSEventNotifyControl::Continue;
            handlers.each(|f| {
                control = f(event, source);
                control == BSEventNotifyControl::Continue
            });
            control
        })));
        sinks.push(SourceSinks { source, handlers });
        handlers
    };
    handlers.add(owner, handler);
    log!("{owner}: sink added on the source at {:#x}", source.address());
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
    fn reads_a_setting() {
        let ini = "; host settings\n[hooks]\nCompat = stack ; opt in\nother=1\n";
        assert_eq!(setting(ini, "compat"), Some("stack"));
        assert_eq!(setting(ini, "missing"), None);
        assert_eq!(setting("# compat = stack", "compat"), None);
    }
}
