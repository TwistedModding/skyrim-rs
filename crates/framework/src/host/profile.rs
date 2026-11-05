//! The load profile the Debug tab shows: when the game reached each startup step, how long each SKSE
//! plugin's `SKSEPlugin_Load` and each of its message listeners took, how long each framework plugin
//! took to load, and how long the engine spent reading each data file.
//!
//! The SKSE plugins are timed from [`install`], which the host runs from its `SKSEPlugin_Preload`:
//! SKSE 2.3.1 calls every Preload before any plugin's Load. Each mapped plugin's `SKSEPlugin_Load`
//! export gets a detour that is removed once it returns, and the loader's `RegisterListener` hands
//! the loader a stub in place of each listener, which passes the listener's record to [`timed_listener`]
//! (`mov rdx, record; mov rax, wrapper; jmp rax`). A loader without Preload reaches [`install`] at
//! the host's Load, after the plugins loaded before it. When the host's Load fails the loader unloads
//! it, so [`uninstall`] first removes the detours and points every stub at its listener. The data files are timed by a thread that
//! samples `TESDataHandler.activeFile` every 100 µs until `DATA_LOADED`; a file read faster than
//! that may not be seen.

use core::ffi::{CStr, c_char};
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use skyrim::forms::{TESDataHandlerExt as _, TESFileExt as _};
use windows_sys::Win32::Foundation::{FILETIME, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// The time since the process started, from the first call's reading of its creation time.
pub(super) fn since_start() -> Duration {
    static ANCHOR: OnceLock<(Instant, Duration)> = OnceLock::new();
    let (at, then) = *ANCHOR.get_or_init(|| {
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        let (mut created, mut now, mut unused) = (FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }, [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 3]);
        let [exit, kernel, user] = &mut unused;
        unsafe { GetProcessTimes(GetCurrentProcess(), &raw mut created, exit, kernel, user) };
        unsafe { GetSystemTimePreciseAsFileTime(&raw mut now) };
        (Instant::now(), Duration::from_nanos(ticks(now).saturating_sub(ticks(created)) * 100))
    });
    then + at.elapsed()
}

static STEPS: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());

/// Notes that the game reached `step`, once.
pub(super) fn step(step: &'static str) {
    let at = since_start();
    let mut steps = STEPS.lock().unwrap_or_else(PoisonError::into_inner);
    if !steps.iter().any(|(s, _)| *s == step) {
        steps.push((step, at));
    }
}

pub(super) fn steps() -> Vec<(&'static str, Duration)> {
    STEPS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A DLL's load: its file, how long its entry point took, what it returned and when it started.
#[derive(Clone)]
pub(super) struct Load {
    pub file: String,
    pub took: Duration,
    pub result: bool,
    pub at: Duration,
}

static SKSE_LOADS: Mutex<Vec<Load>> = Mutex::new(Vec::new());
static FRAMEWORK_LOADS: Mutex<Vec<Load>> = Mutex::new(Vec::new());

pub(super) fn skse_loads() -> Vec<Load> {
    SKSE_LOADS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

pub(super) fn framework_loads() -> Vec<Load> {
    FRAMEWORK_LOADS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Times `load`, a framework plugin's load of `file` (reading its DLL and running its entry point).
pub(super) fn framework_load(file: &str, load: impl FnOnce() -> bool) -> bool {
    let at = since_start();
    let start = Instant::now();
    let result = load();
    FRAMEWORK_LOADS.lock().unwrap_or_else(PoisonError::into_inner).push(Load { file: file.to_owned(), took: start.elapsed(), result, at });
    result
}

/// `mov rdx, record; mov rax, target; jmp rax`: calls `target` with `record` as its second argument.
fn stub(record: usize, target: usize) -> Result<usize, hook::Error> {
    let mut code = [0u8; 22];
    code[..2].copy_from_slice(&[0x48, 0xBA]);
    code[2..10].copy_from_slice(&record.to_le_bytes());
    code[10..12].copy_from_slice(&[0x48, 0xB8]);
    code[12..20].copy_from_slice(&target.to_le_bytes());
    code[20..].copy_from_slice(&[0xFF, 0xE0]);
    hook::Trampoline::shared().alloc(target, &code)
}

/// The file name of the module holding `address`.
fn module_of(address: usize) -> String {
    let mut module: HMODULE = core::ptr::null_mut();
    if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, address as *const u16, &raw mut module) } == 0 {
        return format!("{address:#x}");
    }
    let mut path = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&path[..len]);
    Path::new(&path).file_name().map_or(path.clone(), |f| f.to_string_lossy().into_owned())
}

type LoadFn = unsafe extern "C" fn(*const skse::Interface) -> bool;
type RegisterFn = unsafe extern "C" fn(skse::PluginHandle, *const c_char, skse::MessageCallback) -> bool;

/// A hooked `SKSEPlugin_Load`, its detour removed after the call.
struct TimedLoad {
    file: String,
    hook: Mutex<Option<hook::Hook<LoadFn>>>,
}

static TIMED_LOADS: Mutex<Vec<&'static TimedLoad>> = Mutex::new(Vec::new());

unsafe extern "C" fn timed_load(skse: *const skse::Interface, record: &'static TimedLoad) -> bool {
    let Some(original) = record.hook.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(hook::Hook::original) else { return false };
    let at = since_start();
    let start = Instant::now();
    let result = unsafe { original(skse) };
    let took = start.elapsed();
    SKSE_LOADS.lock().unwrap_or_else(PoisonError::into_inner).push(Load { file: record.file.clone(), took, result, at });
    if let Some(hook) = record.hook.lock().unwrap_or_else(PoisonError::into_inner).take() {
        let _ = unsafe { hook.remove() };
    }
    result
}

/// One message kind from one sender as a listener saw it.
#[derive(Clone)]
pub(super) struct Calls {
    pub sender: String,
    pub kind: u32,
    pub count: u32,
    pub total: Duration,
    pub max: Duration,
}

/// A plugin's listener: the DLL it is in, the sender it asked for, its stub and its calls.
pub(super) struct Listener {
    pub module: String,
    pub sender: String,
    original: skse::MessageCallback,
    stub: usize,
    calls: Mutex<Vec<Calls>>,
}

impl Listener {
    pub(super) fn calls(&self) -> Vec<Calls> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

static LISTENERS: Mutex<Vec<&'static Listener>> = Mutex::new(Vec::new());
static REGISTER: Mutex<Option<hook::Hook<RegisterFn>>> = Mutex::new(None);

pub(super) fn listeners() -> Vec<&'static Listener> {
    LISTENERS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

unsafe extern "C" fn timed_listener(msg: *mut skse::Message, listener: &'static Listener) {
    let start = Instant::now();
    unsafe { (listener.original)(msg) };
    let took = start.elapsed();
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    let sender = if msg.sender.is_null() { &[][..] } else { unsafe { CStr::from_ptr(msg.sender) }.to_bytes() };
    let mut calls = listener.calls.lock().unwrap_or_else(PoisonError::into_inner);
    let index = calls.iter().position(|c| c.kind == msg.kind && c.sender.as_bytes() == sender).unwrap_or_else(|| {
        calls.push(Calls { sender: String::from_utf8_lossy(sender).into_owned(), kind: msg.kind, count: 0, total: Duration::ZERO, max: Duration::ZERO });
        calls.len() - 1
    });
    let c = &mut calls[index];
    c.count += 1;
    c.total += took;
    c.max = c.max.max(took);
}

unsafe extern "C" fn register_detour(handle: skse::PluginHandle, sender: *const c_char, handler: skse::MessageCallback) -> bool {
    let Some(original) = REGISTER.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(hook::Hook::original) else { return false };
    let asked = if sender.is_null() { "every plugin".to_owned() } else { unsafe { CStr::from_ptr(sender) }.to_string_lossy().into_owned() };
    let mut listener = Box::new(Listener { module: module_of(handler as usize), sender: asked, original: handler, stub: 0, calls: Mutex::new(Vec::new()) });
    let Ok(code) = stub(core::ptr::from_ref(&*listener) as usize, timed_listener as *const () as usize) else {
        return unsafe { original(handle, sender, handler) };
    };
    listener.stub = code;
    let listener: &'static Listener = Box::leak(listener);
    let registered = unsafe { original(handle, sender, core::mem::transmute::<usize, skse::MessageCallback>(code)) };
    if registered {
        LISTENERS.lock().unwrap_or_else(PoisonError::into_inner).push(listener);
    }
    registered
}

/// Hooks the loader's `RegisterListener` and the `SKSEPlugin_Load` of every SKSE plugin mapped
/// from `Data\SKSE\Plugins` other than the host, at the first call; every call returns what the
/// first did, for the log.
pub(super) fn install(skse: &skse::Interface) -> &'static str {
    static INSTALLED: OnceLock<String> = OnceLock::new();
    INSTALLED.get_or_init(|| hook_loads(skse))
}

fn hook_loads(skse: &skse::Interface) -> String {
    let listeners = match skse.messaging() {
        Some(messaging) => {
            let mut slot = REGISTER.lock().unwrap_or_else(PoisonError::into_inner);
            match unsafe { hook::Hook::<RegisterFn>::create(messaging.register_listener as usize, register_detour) } {
                Ok(hook) => {
                    let enabled = slot.insert(hook).enable();
                    if enabled.is_err() {
                        *slot = None;
                    }
                    enabled.map_or_else(|e| format!("listeners not timed: {e}"), |()| "listeners timed".to_owned())
                }
                Err(e) => format!("listeners not timed: {e}"),
            }
        }
        None => "listeners not timed: no messaging interface".to_owned(),
    };
    let own = module_of(install as *const () as usize);
    let (mut hooked, mut failed) = (0, Vec::new());
    for path in super::plugin_files(&super::plugins_dir()) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if module.is_null() || file.eq_ignore_ascii_case(&own) {
            continue;
        }
        let Some(load) = (unsafe { GetProcAddress(module, c"SKSEPlugin_Load".as_ptr().cast()) }) else { continue };
        let record: &'static TimedLoad = Box::leak(Box::new(TimedLoad { file: file.clone(), hook: Mutex::new(None) }));
        let hooked_load = stub(core::ptr::from_ref(record) as usize, timed_load as *const () as usize).and_then(|code| {
            let mut slot = record.hook.lock().unwrap_or_else(PoisonError::into_inner);
            let enabled = slot.insert(unsafe { hook::Hook::<LoadFn>::create(load as usize, core::mem::transmute::<usize, LoadFn>(code)) }?).enable();
            if enabled.is_err() {
                *slot = None;
            }
            enabled
        });
        match hooked_load {
            Ok(()) => {
                hooked += 1;
                TIMED_LOADS.lock().unwrap_or_else(PoisonError::into_inner).push(record);
            }
            Err(e) => failed.push(format!("{file} ({e})")),
        }
    }
    let failed = if failed.is_empty() { String::new() } else { format!(", not timed: {}", failed.join(", ")) };
    format!("load profile: {listeners}, {hooked} SKSE plugin loads timed{failed}")
}

/// Leaves nothing that reaches the host's code: removes the detours on `RegisterListener` and on
/// the Loads not called yet, and points every listener's stub at the listener itself.
pub(super) fn uninstall() {
    let register = REGISTER.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(hook) = register {
        let _ = unsafe { hook.remove() };
    }
    let loads: Vec<_> = TIMED_LOADS.lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(|l| l.hook.lock().unwrap_or_else(PoisonError::into_inner).take()).collect();
    for hook in loads {
        let _ = unsafe { hook.remove() };
    }
    for listener in LISTENERS.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        let _ = unsafe { hook::write_code(listener.stub + 12, &(listener.original as usize).to_le_bytes()) };
    }
}

/// A data file as the profile lists it.
#[derive(Clone)]
pub(super) struct DataFile {
    pub name: String,
    /// `00` to `FD`, or `FE:000` to `FE:FFF` for a light file.
    pub index: String,
    pub master: bool,
    pub light: bool,
    pub took: Duration,
}

/// The files the engine loaded, in load order, and when the first was opened and `DATA_LOADED` arrived.
#[derive(Clone, Default)]
pub(super) struct DataFiles {
    pub files: Vec<DataFile>,
    pub first: Option<Duration>,
    pub done: Option<Duration>,
    /// No file was seen being read (the sampler found `activeFile` null throughout).
    pub unseen: bool,
}

/// `TESFile` addresses with the time each was the active file, and when the first was seen.
type Sampled = (Vec<(usize, Duration)>, Option<Duration>);

static SAMPLING: AtomicBool = AtomicBool::new(false);
static SAMPLER: Mutex<Option<std::thread::JoinHandle<Sampled>>> = Mutex::new(None);
static DATA_FILES: OnceLock<DataFiles> = OnceLock::new();

fn sample() -> Sampled {
    let (mut current, mut since) = (0usize, Instant::now());
    let (mut times, mut first): Sampled = (Vec::new(), None);
    let add = |times: &mut Vec<(usize, Duration)>, file: usize, took: Duration| match times.iter_mut().find(|(f, _)| *f == file) {
        Some((_, t)) => *t += took,
        None => times.push((file, took)),
    };
    while SAMPLING.load(Ordering::Acquire) {
        let file = sre::TESDataHandler::singleton().map_or(0, |h| unsafe { h.activeFile_ptr().read_volatile() } as usize);
        if file != current {
            let now = Instant::now();
            if current != 0 {
                add(&mut times, current, now - since);
            }
            if file != 0 && first.is_none() {
                first = Some(since_start());
            }
            (current, since) = (file, now);
        }
        std::thread::sleep(Duration::from_micros(if current == 0 { 1000 } else { 100 }));
    }
    if current != 0 {
        add(&mut times, current, since.elapsed());
    }
    (times, first)
}

/// Starts the thread that times the data files until [`finish_data_files`].
pub(super) fn sample_data_files() {
    if !SAMPLING.swap(true, Ordering::AcqRel) {
        *SAMPLER.lock().unwrap_or_else(PoisonError::into_inner) = std::thread::Builder::new().name("load profile".to_owned()).spawn(sample).ok();
    }
}

/// Stops the sampler at `DATA_LOADED` and matches what it saw to the loaded files.
pub(super) fn finish_data_files() {
    let done = since_start();
    SAMPLING.store(false, Ordering::Release);
    let Some(sampler) = SAMPLER.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
    let (times, first) = sampler.join().unwrap_or_default();
    let Some(handler) = sre::TESDataHandler::singleton() else { return };
    let took = |file: &sre::TESFile| times.iter().find(|(f, _)| *f == core::ptr::from_ref(file) as usize).map_or(Duration::ZERO, |(_, t)| *t);
    let mut files: Vec<DataFile> = handler.compiled_files().iter().flatten().enumerate().map(|(i, f)| DataFile { name: f.name().to_owned(), index: format!("{i:02X}"), master: f.is_master(), light: false, took: took(f) }).collect();
    files.extend(handler.light_files().iter().flatten().enumerate().map(|(i, f)| DataFile { name: f.name().to_owned(), index: format!("FE:{i:03X}"), master: f.is_master(), light: true, took: took(f) }));
    let _ = DATA_FILES.set(DataFiles { unseen: times.is_empty(), files, first, done: Some(done) });
}

pub(super) fn data_files() -> Option<&'static DataFiles> {
    DATA_FILES.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEEN: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

    unsafe extern "C" fn plugin_listener(msg: *mut skse::Message) {
        std::thread::sleep(Duration::from_millis(2));
        SEEN.lock().unwrap().push((msg as usize, unsafe { (*msg).kind }));
    }

    #[test]
    fn a_stub_passes_the_message_and_records_the_call() {
        let listener: &'static Listener = Box::leak(Box::new(Listener { module: module_of(plugin_listener as *const () as usize), sender: "SKSE".to_owned(), original: plugin_listener, stub: 0, calls: Mutex::new(Vec::new()) }));
        let code = stub(core::ptr::from_ref(listener) as usize, timed_listener as *const () as usize).unwrap();
        let timed = unsafe { core::mem::transmute::<usize, skse::MessageCallback>(code) };
        let mut msg = skse::Message { sender: c"SKSE".as_ptr(), kind: 8, data_len: 0, data: core::ptr::null_mut() };
        unsafe { timed(&raw mut msg) };
        unsafe { timed(&raw mut msg) };
        msg.kind = 2;
        unsafe { timed(&raw mut msg) };
        assert_eq!(*SEEN.lock().unwrap(), [(&raw mut msg as usize, 8), (&raw mut msg as usize, 8), (&raw mut msg as usize, 2)]);
        let calls = listener.calls();
        assert_eq!(calls.iter().map(|c| (c.sender.as_str(), c.kind, c.count)).collect::<Vec<_>>(), [("SKSE", 8, 2), ("SKSE", 2, 1)]);
        assert!(calls[0].total >= Duration::from_millis(4) && calls[0].max >= Duration::from_millis(2));
        assert!(listener.module.to_ascii_lowercase().ends_with(".exe"), "{}", listener.module);
    }

    static UNTIMED: Mutex<u32> = Mutex::new(0);

    unsafe extern "C" fn counted(_: *mut skse::Message) {
        *UNTIMED.lock().unwrap() += 1;
    }

    #[test]
    fn an_uninstalled_stub_calls_its_listener_directly() {
        let mut listener = Box::new(Listener { module: String::new(), sender: "SKSE".to_owned(), original: counted, stub: 0, calls: Mutex::new(Vec::new()) });
        listener.stub = stub(core::ptr::from_ref(&*listener) as usize, timed_listener as *const () as usize).unwrap();
        let listener: &'static Listener = Box::leak(listener);
        LISTENERS.lock().unwrap().push(listener);
        let call = unsafe { core::mem::transmute::<usize, skse::MessageCallback>(listener.stub) };
        let mut msg = skse::Message { sender: c"SKSE".as_ptr(), kind: 1, data_len: 0, data: core::ptr::null_mut() };
        unsafe { call(&raw mut msg) };
        uninstall();
        unsafe { call(&raw mut msg) };
        assert_eq!(*UNTIMED.lock().unwrap(), 2, "the listener ran both times");
        assert_eq!(listener.calls()[0].count, 1, "only the call before the uninstall was timed");
    }

    #[test]
    fn the_process_clock_is_past_its_start() {
        let (a, b) = (since_start(), since_start());
        assert!(a > Duration::ZERO && b >= a);
        assert!(framework_load("x.dll", || true));
        assert!(framework_loads().iter().any(|l| l.file == "x.dll" && l.result));
    }
}
