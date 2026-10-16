//! The host's side: [`load`] is `skyrim_rs.dll`'s `SKSEPlugin_Load`.

use core::ffi::{CStr, c_void};
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use skyrim::papyrus::VirtualMachine;
use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::{ABI, BUILD, Descriptor, Host, MessageFn, PapyrusFn};

/// The folder under `Data\SKSE\Plugins` the host loads plugins from.
pub const PLUGIN_DIR: &str = "skyrim-rs";

static LOG: Mutex<Option<File>> = Mutex::new(None);
static HOST: OnceLock<Host> = OnceLock::new();
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
/// Papyrus callback, then loads every plugin in `Data\SKSE\Plugins\skyrim-rs\`.
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
    });
    let dir = std::env::current_exe().map(|exe| exe.with_file_name("Data").join("SKSE").join("Plugins").join(PLUGIN_DIR)).unwrap_or_default();
    for path in plugin_files(&dir) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        match load_plugin(&path, host) {
            Ok(name) => log!("loaded {file}: {name}"),
            Err(e) => log!("skipped {file}: {e}"),
        }
    }
    true
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

fn load_plugin(path: &Path, host: &'static Host) -> Result<String, String> {
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
    let descriptor = match checked {
        Ok(d) => d,
        Err(e) => {
            unsafe { FreeLibrary(module) };
            return Err(e);
        }
    };
    let name = if descriptor.name.is_null() { String::new() } else { unsafe { CStr::from_ptr(descriptor.name) }.to_string_lossy().into_owned() };
    if (descriptor.load)(host) { Ok(name) } else { Err(format!("{name}: load returned false")) }
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
