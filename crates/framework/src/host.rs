//! The host's side: [`load`] is `skyrim_rs.dll`'s `SKSEPlugin_Load`.

use core::ffi::{CStr, c_void};
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use skyrim::papyrus::VirtualMachine;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::order::{self, Constraints};
use crate::{ABI, BUILD, Descriptor, Host, MessageFn, PapyrusFn, Plugin};

/// The folder under `Data\SKSE\Plugins` the host loads plugins from.
pub const PLUGIN_DIR: &str = "skyrim-rs";

static LOG: Mutex<Option<File>> = Mutex::new(None);
static HOST: OnceLock<Host> = OnceLock::new();
static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();
static MESSAGES: Mutex<Vec<MessageFn>> = Mutex::new(Vec::new());
static PAPYRUS: Mutex<Vec<PapyrusFn>> = Mutex::new(Vec::new());

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg);
        }
    };
}

/// Initializes the game layer, installs the overlay, registers the host's message listener and
/// Papyrus callback, then loads every plugin in `Data\SKSE\Plugins\skyrim-rs\` in the order
/// their descriptors resolve to.
pub fn load(skse: &skse::Interface) -> bool {
    // The loader hands every plugin the same object in its read-only data, alive for the whole game.
    let skse: &'static skse::Interface = unsafe { &*core::ptr::from_ref(skse) };
    if let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs") {
        *LOG.lock().unwrap() = Some(file);
    }
    log!("{}, abi {ABI}", BUILD.to_string_lossy());
    if let Err(e) = skyrim::init(skse) {
        log!("game layer: {e}");
        return false;
    }
    match overlay::install(overlay::DEFAULT_TOGGLE) {
        Ok(()) => log!("overlay installed, toggled with scan code {:#x}", overlay::DEFAULT_TOGGLE),
        Err(e) => log!("overlay: {e}"),
    }
    let (Some(messaging), Some(papyrus)) = (skse.messaging(), skse.papyrus()) else {
        log!("no messaging or Papyrus interface");
        return false;
    };
    if !messaging.listen_skse(skse.plugin_handle(), on_message) || !papyrus.register(on_papyrus) {
        log!("the loader refused the message listener or the Papyrus callback");
        return false;
    }
    let host = HOST.get_or_init(|| Host {
        skse,
        on_message: |f| MESSAGES.lock().unwrap().push(f),
        on_papyrus: |f| PAPYRUS.lock().unwrap().push(f),
        add_window: |w| overlay::add(w),
        plugins,
    });
    let dir = std::env::current_exe().map(|exe| exe.with_file_name("Data").join("SKSE").join("Plugins").join(PLUGIN_DIR)).unwrap_or_default();
    let mut read = Vec::new();
    for path in plugin_files(&dir) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        match read_descriptor(&path) {
            Ok((module, descriptor)) => read.push((module, descriptor, describe(descriptor, file))),
            Err(e) => log!("skipped {file}: {e}"),
        }
    }
    let items: Vec<(&str, &Constraints)> = read.iter().map(|(_, _, p)| (p.name.as_str(), &p.constraints)).collect();
    let order = order::sort(&items);
    for (asker, name) in &order.missing {
        log!("no plugin named {name} (asked by {})", read[*asker].2.name);
    }
    let in_cycle: Vec<&str> = order.cycle.iter().map(|&i| read[i].2.name.as_str()).collect();
    for &i in &order.cycle {
        let (module, _, plugin) = &read[i];
        log!("skipped {}: {} is in a load order cycle with {}", plugin.file, plugin.name, in_cycle.iter().filter(|n| **n != plugin.name).copied().collect::<Vec<_>>().join(", "));
        unsafe { FreeLibrary(*module) };
    }
    let sorted: Vec<Plugin> = order.sorted.iter().map(|&i| read[i].2.clone()).collect();
    log!("load order: {}", sorted.iter().map(|p| if p.constraints == Constraints::default() { p.name.clone() } else { format!("{} ({})", p.name, p.constraints) }).collect::<Vec<_>>().join(", "));
    let _ = PLUGINS.set(sorted);
    for &i in &order.sorted {
        let (_, descriptor, plugin) = &read[i];
        if (descriptor.load)(host) {
            log!("loaded {}: {}", plugin.file, plugin.name);
        } else {
            log!("skipped {}: {}: load returned false", plugin.file, plugin.name);
        }
    }
    true
}

/// The accepted plugins in load order, empty before [`load`] resolved it.
#[must_use]
pub fn plugins() -> &'static [Plugin] {
    PLUGINS.get().map_or(&[], Vec::as_slice)
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

fn describe(descriptor: &Descriptor, file: String) -> Plugin {
    let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    Plugin {
        name: if descriptor.name.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.name) }.to_string_lossy().into_owned() },
        file,
        constraints: Constraints { before: owned(descriptor.load_before), after: owned(descriptor.load_after), priority: descriptor.priority },
    }
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    let handlers = MESSAGES.lock().unwrap().clone();
    for f in handlers {
        f(msg);
    }
}

unsafe extern "C" fn on_papyrus(registry: *mut c_void) {
    let Some(vm) = (unsafe { VirtualMachine::from_ptr(registry) }) else {
        log!("the Papyrus callback did not receive the VM");
        return;
    };
    let handlers = PAPYRUS.lock().unwrap().clone();
    for f in handlers {
        f(vm);
    }
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
}
