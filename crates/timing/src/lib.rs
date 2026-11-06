//! `!!!skyrim-runtime-extender-timing.dll`: times every SKSE plugin's `SKSEPlugin_Load` and message
//! listeners for the host's Debug tab ([`skse::timing`]). Its name sorts before every other plugin,
//! so every loader (SKSE 2.0.19 to 2.3.1, and the standalone `dinput8.dll`) maps it first.
//!
//! When it is mapped it points the loaders' imports of `GetProcAddress` at [`get_proc_address`], which
//! answers a lookup of another loaded module's `SKSEPlugin_Load` with a stub: `mov rdx, record;
//! mov rax, timed_load; jmp rax`. Load takes one argument, so the record rides in the second. Its own
//! Load, the first the loader calls, hooks `RegisterListener` to register a stub of the same shape in
//! place of each listener. If it is unloaded it puts the imports back, removes the hook and points
//! every stub at its original, so nothing reaches its freed code.

use core::ffi::{CStr, c_char, c_void};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use skse::timing::{CallsSink, LoadSink, RawCalls, RawLoad, Table, since_start};
use windows_sys::Win32::Foundation::{FARPROC, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
use windows_sys::Win32::System::ProcessStatus::K32EnumProcessModules;
use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::{BOOL, PCSTR};

skse::plugin! {
    name: "skyrim-runtime-extender-timing",
    author: "TwistedModding",
    version: 1,
    load: load,
}

type GetProcAddressFn = unsafe extern "system" fn(HMODULE, PCSTR) -> FARPROC;
type LoadFn = unsafe extern "C" fn(*const skse::Interface) -> bool;
type RegisterFn = unsafe extern "C" fn(skse::PluginHandle, *const c_char, skse::MessageCallback) -> bool;

/// This DLL's own module, whose exports are never wrapped.
static OWN: AtomicUsize = AtomicUsize::new(0);
/// The real `GetProcAddress`, which every patched slot held.
static REAL: AtomicUsize = AtomicUsize::new(0);
/// Each patched import slot with the value it held.
static SLOTS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static STATUS: OnceLock<CString> = OnceLock::new();
static PATCHED: Mutex<String> = Mutex::new(String::new());

/// A wrapped `SKSEPlugin_Load` and the stub handed out for it.
struct Wrapped {
    file: CString,
    original: usize,
    stub: usize,
}

static WRAPPED: Mutex<Vec<&'static Wrapped>> = Mutex::new(Vec::new());
static LOADS: Mutex<Vec<(&'static Wrapped, u64, u64, bool)>> = Mutex::new(Vec::new());

/// One message kind from one sender as a listener saw it.
struct Seen {
    sender: CString,
    kind: u32,
    count: u32,
    total: Duration,
    max: Duration,
}

/// A plugin's listener, the stub registered in its place, and what it saw.
struct Listener {
    module: CString,
    asked: CString,
    original: skse::MessageCallback,
    stub: usize,
    calls: Mutex<Vec<Seen>>,
}

static LISTENERS: Mutex<Vec<&'static Listener>> = Mutex::new(Vec::new());
static REGISTER: Mutex<Option<hook::Hook<RegisterFn>>> = Mutex::new(None);

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

/// Points a stub straight at the function it wrapped.
fn bypass(stub: usize, original: usize) {
    let _ = unsafe { hook::write_code(stub + 12, &original.to_le_bytes()) };
}

/// The module holding `address`; `None` for an address in no module the loader lists.
fn module_at(address: usize) -> Option<HMODULE> {
    let mut module: HMODULE = core::ptr::null_mut();
    let found = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, address as *const u16, &raw mut module) };
    (found != 0).then_some(module)
}

fn module_path(module: HMODULE) -> PathBuf {
    let mut path = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), 260) } as usize;
    PathBuf::from(String::from_utf16_lossy(&path[..len]))
}

fn file_name(module: HMODULE) -> String {
    module_path(module).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

fn c_text(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

unsafe extern "C" fn timed_load(skse: *const skse::Interface, record: &'static Wrapped) -> bool {
    let at = since_start();
    let start = Instant::now();
    let result = unsafe { core::mem::transmute::<usize, LoadFn>(record.original)(skse) };
    let took = start.elapsed();
    let nanos = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
    LOADS.lock().unwrap_or_else(PoisonError::into_inner).push((record, nanos(at), nanos(took), result));
    result
}

/// The stub for the `SKSEPlugin_Load` at `original`, made at its first lookup.
fn wrap_load(original: usize, file: &str) -> Option<usize> {
    let mut wrapped = WRAPPED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(w) = wrapped.iter().find(|w| w.original == original) {
        return Some(w.stub);
    }
    let mut record = Box::new(Wrapped { file: c_text(file), original, stub: 0 });
    record.stub = stub(core::ptr::from_ref(&*record) as usize, timed_load as *const () as usize).ok()?;
    let record: &'static Wrapped = Box::leak(record);
    wrapped.push(record);
    Some(record.stub)
}

/// What the loaders' imports now reach: the real lookup, with another loaded module's `SKSEPlugin_Load`
/// answered by its stub. A module mapped as an image only (`LoadLibraryExA` with flag 0x20, its
/// handle's low bits set) is not one the loader lists, and is never wrapped.
unsafe extern "system" fn get_proc_address(module: HMODULE, name: PCSTR) -> FARPROC {
    let real = unsafe { core::mem::transmute::<usize, GetProcAddressFn>(REAL.load(Ordering::Acquire))(module, name) };
    let Some(found) = real else { return real };
    if (name as usize) >> 16 == 0 || module as usize & 3 != 0 || module as usize == OWN.load(Ordering::Relaxed) || unsafe { CStr::from_ptr(name.cast()) } != c"SKSEPlugin_Load" {
        return real;
    }
    if module_at(found as usize) != Some(module) {
        return real;
    }
    wrap_load(found as usize, &file_name(module)).map_or(real, |stub| Some(unsafe { core::mem::transmute::<usize, unsafe extern "system" fn() -> isize>(stub) }))
}

/// The modules that load plugins: the script extender's `skse64_*.dll` and a `dinput8.dll` beside the
/// executable (the standalone loader, or another proxy, whose other lookups pass through unchanged).
fn loaders() -> Vec<(HMODULE, String)> {
    let process = unsafe { GetCurrentProcess() };
    let mut needed = 0u32;
    unsafe { K32EnumProcessModules(process, core::ptr::null_mut(), 0, &raw mut needed) };
    let mut modules: Vec<HMODULE> = vec![core::ptr::null_mut(); needed as usize / size_of::<HMODULE>()];
    let size = u32::try_from(size_of_val(modules.as_slice())).unwrap_or(0);
    if unsafe { K32EnumProcessModules(process, modules.as_mut_ptr(), size, &raw mut needed) } == 0 {
        return Vec::new();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    modules
        .into_iter()
        .filter(|m| !m.is_null())
        .filter_map(|m| {
            let path = module_path(m);
            let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
            (name.starts_with("skse64_") || (name == "dinput8.dll" && path.parent().map(Path::to_path_buf) == exe_dir)).then_some((m, name))
        })
        .collect()
}

/// Points every loader module's `GetProcAddress` import at [`get_proc_address`].
fn patch_loaders() {
    let mut slots = SLOTS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut patched = Vec::new();
    for (module, name) in loaders() {
        let Some(slot) = (unsafe { hook::import_slot(module as usize, "kernel32.dll", c"GetProcAddress") }) else { continue };
        let previous = unsafe { (slot as *const usize).read() };
        if previous == get_proc_address as GetProcAddressFn as usize {
            continue;
        }
        REAL.store(previous, Ordering::Release);
        if unsafe { hook::write_pointer(slot, get_proc_address as GetProcAddressFn as usize) }.is_ok() {
            slots.push((slot, previous));
            patched.push(name);
        }
    }
    *PATCHED.lock().unwrap_or_else(PoisonError::into_inner) = if patched.is_empty() { "no loader's GetProcAddress import found, Loads not timed".to_owned() } else { format!("Loads timed through {}", patched.join(", ")) };
}

/// Leaves nothing that reaches this DLL's code.
fn unpatch() {
    for (slot, previous) in SLOTS.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
        let _ = unsafe { hook::write_pointer(slot, previous) };
    }
    if let Some(hook) = REGISTER.lock().unwrap_or_else(PoisonError::into_inner).take() {
        let _ = unsafe { hook.remove() };
    }
    for w in WRAPPED.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        bypass(w.stub, w.original);
    }
    for l in LISTENERS.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        bypass(l.stub, l.original as usize);
    }
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(module: HMODULE, reason: u32, reserved: *mut c_void) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH => {
            OWN.store(module as usize, Ordering::Relaxed);
            patch_loaders();
        }
        DLL_PROCESS_DETACH if reserved.is_null() => unpatch(),
        _ => {}
    }
    TRUE
}

unsafe extern "C" fn timed_listener(msg: *mut skse::Message, listener: &'static Listener) {
    let start = Instant::now();
    unsafe { (listener.original)(msg) };
    let took = start.elapsed();
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    let sender = if msg.sender.is_null() { c"" } else { unsafe { CStr::from_ptr(msg.sender) } };
    let mut calls = listener.calls.lock().unwrap_or_else(PoisonError::into_inner);
    let index = calls.iter().position(|c| c.kind == msg.kind && c.sender.as_c_str() == sender).unwrap_or_else(|| {
        calls.push(Seen { sender: sender.to_owned(), kind: msg.kind, count: 0, total: Duration::ZERO, max: Duration::ZERO });
        calls.len() - 1
    });
    let c = &mut calls[index];
    c.count += 1;
    c.total += took;
    c.max = c.max.max(took);
}

unsafe extern "C" fn register_detour(handle: skse::PluginHandle, sender: *const c_char, handler: skse::MessageCallback) -> bool {
    let Some(original) = REGISTER.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(hook::Hook::original) else { return false };
    let asked = if sender.is_null() { c"every plugin".to_owned() } else { unsafe { CStr::from_ptr(sender) }.to_owned() };
    let module = module_at(handler as usize).map_or_else(|| format!("{:#x}", handler as usize), file_name);
    let mut listener = Box::new(Listener { module: c_text(&module), asked, original: handler, stub: 0, calls: Mutex::new(Vec::new()) });
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

/// The first Load the loader calls: hooks `RegisterListener` before any other plugin can register.
fn load(skse: &skse::Interface) -> bool {
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
    let patched = PATCHED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let _ = STATUS.set(c_text(&format!("{patched}; {listeners}")));
    true
}

unsafe extern "C" fn table_loads(sink: LoadSink, ctx: *mut c_void) {
    let loads = LOADS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for (record, at, took, result) in loads {
        let raw = RawLoad { file: record.file.as_ptr(), at, took, result };
        unsafe { sink(ctx, &raw const raw) };
    }
}

unsafe extern "C" fn table_calls(sink: CallsSink, ctx: *mut c_void) {
    let listeners = LISTENERS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let nanos = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
    for listener in listeners {
        let calls = listener.calls.lock().unwrap_or_else(PoisonError::into_inner);
        for c in calls.iter() {
            let raw = RawCalls { module: listener.module.as_ptr(), asked: listener.asked.as_ptr(), sender: c.sender.as_ptr(), kind: c.kind, count: c.count, total: nanos(c.total), max: nanos(c.max) };
            unsafe { sink(ctx, &raw const raw) };
        }
    }
}

unsafe extern "C" fn table_status() -> *const c_char {
    STATUS.get().map_or(c"its Load has not run".as_ptr(), |s| s.as_ptr())
}

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static SkyrimRs_Timing: Table = Table { version: skse::timing::VERSION, loads: table_loads, calls: table_calls, status: table_status };

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    use super::*;

    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    unsafe extern "C" fn plugin_load(skse: *const skse::Interface) -> bool {
        std::thread::sleep(Duration::from_millis(2));
        skse.is_null()
    }

    unsafe extern "C" fn plugin_listener(msg: *mut skse::Message) {
        SEEN.lock().unwrap().push(unsafe { (*msg).kind });
    }

    #[test]
    fn a_wrapped_load_is_timed_and_a_bypassed_one_is_not() {
        let original = plugin_load as LoadFn as usize;
        let code = wrap_load(original, "plugin.dll").unwrap();
        assert_eq!(wrap_load(original, "plugin.dll"), Some(code), "one stub per Load");
        let call = unsafe { core::mem::transmute::<usize, LoadFn>(code) };
        assert!(unsafe { call(core::ptr::null()) });
        let loads = LOADS.lock().unwrap().clone();
        assert!(loads.iter().any(|(w, _, took, result)| w.file.as_c_str() == c"plugin.dll" && *took >= 2_000_000 && *result));
        bypass(code, original);
        assert!(unsafe { call(core::ptr::null()) });
        assert_eq!(LOADS.lock().unwrap().len(), loads.len(), "a bypassed stub calls the Load untimed");
    }

    #[test]
    fn a_listener_stub_counts_each_kind_and_sender() {
        let mut listener = Box::new(Listener { module: c_text("plugin.dll"), asked: c"SKSE".to_owned(), original: plugin_listener, stub: 0, calls: Mutex::new(Vec::new()) });
        listener.stub = stub(core::ptr::from_ref(&*listener) as usize, timed_listener as *const () as usize).unwrap();
        let listener: &'static Listener = Box::leak(listener);
        let call = unsafe { core::mem::transmute::<usize, skse::MessageCallback>(listener.stub) };
        let mut msg = skse::Message { sender: c"SKSE".as_ptr(), kind: 8, data_len: 0, data: core::ptr::null_mut() };
        unsafe { call(&raw mut msg) };
        unsafe { call(&raw mut msg) };
        msg.kind = 2;
        unsafe { call(&raw mut msg) };
        assert_eq!(*SEEN.lock().unwrap(), [8, 8, 2]);
        let calls = listener.calls.lock().unwrap();
        assert_eq!(calls.iter().map(|c| (c.sender.to_str().unwrap(), c.kind, c.count)).collect::<Vec<_>>(), [("SKSE", 8, 2), ("SKSE", 2, 1)]);
    }

    #[test]
    fn only_a_loaded_modules_load_lookup_is_answered_with_a_stub() {
        REAL.store(windows_sys::Win32::System::LibraryLoader::GetProcAddress as GetProcAddressFn as usize, Ordering::Release);
        let kernel32 = unsafe { GetModuleHandleW(windows_sys::core::w!("kernel32.dll")) };
        let exe = unsafe { GetModuleHandleW(core::ptr::null()) };
        assert!(unsafe { get_proc_address(exe, c"SKSEPlugin_Load".as_ptr().cast()) }.is_none(), "the test binary has no Load");
        let found = unsafe { get_proc_address(kernel32, c"GetTickCount64".as_ptr().cast()) };
        assert!(found.is_some(), "other names pass through");
        let image_only = (kernel32 as usize | 2) as HMODULE;
        assert!(unsafe { get_proc_address(image_only, c"SKSEPlugin_Load".as_ptr().cast()) }.is_none());
    }
}
