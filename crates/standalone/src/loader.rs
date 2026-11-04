//! The plugin loader without the script extender: the acceptance rules, the plugin interface and
//! the message moments of request S1, on the game call sites S1 found the script extender hooking.
//!
//! Each runtime gets the protocol of its script extender ([`skse::loader`]). Plugins in
//! `Data\SKSE\Plugins` are loaded in file name order and checked like that build checks them, by
//! their `SKSEPlugin_Version` or, on 1.5.97, their `SKSEPlugin_Query`; `SKSEPlugin_Load` runs for the
//! accepted ones, kinds 0 and 1 are sent, and the hooks below send the others. `QueryInterface`
//! answers the Papyrus (2), task (4), messaging (5) and trampoline (7) tables; Scaleform (1),
//! serialization (3) and the object table (6) are not implemented and answer null. The hooks are
//! 1.7.104 call sites: on another runtime nothing is loaded and the log lists the sites it lacks.
//!
//! Plugins also call two Papyrus natives the script extender adds and declares in its `Game.pex`,
//! `Game.SaveGame(name)` and `Game.LoadGame(name)` (request S3: `DevBench` saves through the first).
//! As there, a call only records the request; it is served on the main thread right after the
//! main loop drains the save/load manager's own requests.

use core::ffi::{CStr, c_char, c_void};
use std::collections::VecDeque;
use std::ffi::CString;
use std::fs::File;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use skyrim::prelude::*;

use sre::raw::{fns, globals};
use skse::loader::Loader;
use skse::{Interface, Message, MessageCallback, MessagingInterface, PapyrusInterface, PluginHandle, PluginInfo, PluginVersionData, RegisterFunctions, TaskInterface, TrampolineInterface};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const LOG_NAME: &str = "skyrim-runtime-extender-standalone";
const PLUGIN_DIR: &str = "Data\\SKSE\\Plugins";
/// The save name pointer in the save stream (`Win32FileType` +0xbb0).
const SAVE_NAME: usize = 0xbb0;

struct Plugin {
    name: CString,
    file: String,
    /// The leaked `PluginInfo` `GetPluginInfo` hands out.
    info: usize,
}

struct Listener {
    handle: PluginHandle,
    /// The sender's name in lower case, `None` for every sender but the loader and the listener.
    sender: Option<String>,
    callback: MessageCallback,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);
static LOADER: OnceLock<&'static Loader> = OnceLock::new();
static INTERFACE: OnceLock<Interface> = OnceLock::new();
static PLUGINS: Mutex<Vec<Plugin>> = Mutex::new(Vec::new());
static CURRENT: AtomicU32 = AtomicU32::new(skse::INVALID_HANDLE);
static LOADED: AtomicU32 = AtomicU32::new(0);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
static PAPYRUS: Mutex<Vec<RegisterFunctions>> = Mutex::new(Vec::new());
static TASKS: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());
static UI_TASKS: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());
static RELOC: OnceLock<hook::Reloc> = OnceLock::new();
static PENDING: Mutex<Option<(Request, Instant)>> = Mutex::new(None);
/// How long a request the manager refuses (a save right after a load) is tried again, once a frame.
const RETRY: Duration = Duration::from_secs(10);

/// A save or load a plugin asked for through the `Game` natives, served by [`process_requests`].
enum Request {
    Save(String),
    Load(String),
}

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg)*);
        }
    };
}

static MESSAGING: MessagingInterface = MessagingInterface { interface_version: MessagingInterface::VERSION, register_listener, dispatch, get_event_dispatcher };
static PAPYRUS_TABLE: PapyrusInterface = PapyrusInterface { interface_version: PapyrusInterface::VERSION, register: register_papyrus };
static TASK_TABLE: TaskInterface = TaskInterface { interface_version: TaskInterface::VERSION, add_task, add_ui_task };
static TRAMPOLINE_TABLE: TrampolineInterface = TrampolineInterface { interface_version: TrampolineInterface::VERSION, alloc_branch: alloc_near, alloc_local: alloc_near };

/// Loads and starts the plugins, then hooks the moments; on the main thread right before `WinMain`.
pub fn run(runtime: u32) {
    if let Ok(file) = skse::log::open(runtime, LOG_NAME) {
        *LOG.lock().unwrap() = Some(file);
    }
    let Some(loader) = skse::loader::for_runtime(runtime) else {
        log!("skyrim-runtime-extender standalone host: no plugin protocol known for runtime {runtime:08x}; nothing loaded");
        return;
    };
    let _ = LOADER.set(loader);
    log!("skyrim-runtime-extender standalone host, plugin interface {:08x}, runtime {runtime:08x}", loader.version);
    let missing = missing_sites(runtime);
    if !missing.is_empty() {
        log!("{} of {} call sites are not known on this runtime; nothing loaded", missing.len(), SITES.len());
        for site in missing {
            log!("needs {}: the call at id {} + {:#x} to id {}", site.moment, site.caller, site.offset, site.callee);
        }
        return;
    }
    match hook::Reloc::open() {
        Ok(reloc) => {
            let _ = RELOC.set(reloc);
        }
        Err(e) => {
            log!("Address Library: {e}; nothing loaded");
            return;
        }
    }
    let interface = INTERFACE.get_or_init(|| Interface {
        skse_version: loader.version,
        runtime_version: runtime,
        reserved: [0; 2],
        query_interface,
        get_plugin_handle,
        get_release_index,
        get_plugin_info,
    });
    if let Err(e) = skyrim::init(interface) {
        log!("game layer: {e}; nothing loaded");
        return;
    }
    if loader.calls_query() {
        for path in plugin_files() {
            load_queried(interface, &path);
        }
    } else {
        for (handle, (file, module, data)) in (1..).zip(scan(loader)) {
            start(interface, loader, handle, &file, module, data);
        }
    }
    LOADED.store(1, Ordering::Release);
    let names: Vec<String> = PLUGINS.lock().unwrap().iter().map(|p| format!("{} ({})", p.file, p.name.to_string_lossy())).collect();
    log!("loaded: {}", names.join(", "));
    send(MessagingInterface::PLUGINS_LOADED, core::ptr::null_mut(), 0);
    send(MessagingInterface::PLUGINS_LOADED_2, core::ptr::null_mut(), 0);
    if let Err(e) = install_moments() {
        log!("moment hooks: {e}");
    }
}

/// Leaves everything to the script extender, which is loaded.
pub fn defer(runtime: u32) {
    if let Ok(mut file) = skse::log::open(runtime, LOG_NAME) {
        let _ = writeln!(file, "the script extender is loaded; it hosts the plugins");
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Every DLL directly in the plugin folder, in file name order.
fn plugin_files() -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(PLUGIN_DIR).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"))).collect();
    files.sort();
    files
}

fn load_library(path: &Path) -> Option<HMODULE> {
    let module = unsafe { LoadLibraryW(wide(path).as_ptr()) };
    if module.is_null() {
        log!("{}: could not be loaded ({})", file_name(path), std::io::Error::last_os_error());
        return None;
    }
    Some(module)
}

fn export(module: HMODULE, symbol: &CStr) -> Option<unsafe extern "system" fn() -> isize> {
    unsafe { GetProcAddress(module, symbol.as_ptr().cast()) }
}

/// Every DLL with version data the loader accepts, each kept loaded.
fn scan(loader: &Loader) -> Vec<(String, HMODULE, &'static PluginVersionData)> {
    let mut accepted = Vec::new();
    for path in plugin_files() {
        let file = file_name(&path);
        let Some(module) = load_library(&path) else { continue };
        let Some(data) = export(module, c"SKSEPlugin_Version").map(|p| unsafe { &*(p as *const PluginVersionData) }) else {
            log!("{file}: disabled, no version data");
            unsafe { FreeLibrary(module) };
            continue;
        };
        let versionlib = || Path::new(PLUGIN_DIR).join(skse::runtime::versionlib_name(loader.runtime)).exists();
        match loader.accepts(data, versionlib, timestamp(module)) {
            Ok(()) => accepted.push((file, module, data)),
            Err(reason) => {
                log!("{file}: {reason}");
                unsafe { FreeLibrary(module) };
            }
        }
    }
    accepted
}

/// The PE header's `TimeDateStamp` (8 bytes into the NT headers).
fn timestamp(module: HMODULE) -> u32 {
    let base = module as usize;
    unsafe {
        let nt = ((base + 0x3c) as *const u32).read_unaligned() as usize;
        ((base + nt + 8) as *const u32).read_unaligned()
    }
}

type PreloadFn = unsafe extern "C" fn(*const Interface) -> bool;
type LoadFn = unsafe extern "C" fn(*const Interface) -> bool;
type QueryFn = unsafe extern "C" fn(*const Interface, *mut PluginInfo) -> bool;
type RawExport = unsafe extern "system" fn() -> isize;

fn start(interface: &Interface, loader: &Loader, handle: PluginHandle, file: &str, module: HMODULE, data: &PluginVersionData) {
    CURRENT.store(handle, Ordering::Relaxed);
    let preload = if loader.calls_preload() { export(module, c"SKSEPlugin_Preload") } else { None };
    let preloaded = preload.is_none_or(|f| unsafe { core::mem::transmute::<RawExport, PreloadFn>(f)(interface) });
    let ran = preloaded && export(module, c"SKSEPlugin_Load").is_some_and(|f| unsafe { core::mem::transmute::<RawExport, LoadFn>(f)(interface) });
    CURRENT.store(skse::INVALID_HANDLE, Ordering::Relaxed);
    if !ran {
        log!("{file}: disabled, reported as incompatible during load");
        return;
    }
    let name = CStr::from_bytes_until_nul(&data.name).map(CStr::to_owned).unwrap_or_default();
    keep(handle, file, module, name, data.plugin_version);
}

/// SKSE 2.0.19's pass over one DLL: `SKSEPlugin_Query`, its checks, then `SKSEPlugin_Load`, the next
/// handle counting only the plugins accepted so far.
fn load_queried(interface: &Interface, path: &Path) {
    let file = file_name(path);
    let Some(module) = load_library(path) else { return };
    let refuse = |reason: &str| {
        log!("{file}: {reason}");
        unsafe { FreeLibrary(module) };
    };
    let (Some(query), Some(load)) = (export(module, c"SKSEPlugin_Query"), export(module, c"SKSEPlugin_Load")) else {
        return refuse("does not appear to be an SKSE plugin");
    };
    let handle = u32::try_from(PLUGINS.lock().unwrap().len()).unwrap_or(u32::MAX - 1) + 1;
    let mut info = PluginInfo { info_version: 0, name: core::ptr::null(), version: 0 };
    CURRENT.store(handle, Ordering::Relaxed);
    let queried = unsafe { core::mem::transmute::<RawExport, QueryFn>(query)(interface, &raw mut info) };
    let verdict = if queried { Loader::accepts_queried(&info) } else { Err("reported as incompatible during query") };
    let loaded = verdict.and_then(|()| if unsafe { core::mem::transmute::<RawExport, LoadFn>(load)(interface) } { Ok(()) } else { Err("reported as incompatible during load") });
    CURRENT.store(skse::INVALID_HANDLE, Ordering::Relaxed);
    match loaded {
        Ok(()) => keep(handle, &file, module, unsafe { CStr::from_ptr(info.name) }.to_owned(), info.version),
        Err(reason) => refuse(reason),
    }
}

fn keep(handle: PluginHandle, file: &str, module: HMODULE, name: CString, version: u32) {
    let info = Box::leak(Box::new(PluginInfo { info_version: PluginInfo::VERSION, name: name.as_ptr(), version }));
    log!("{file}: loaded as handle {handle} ({}, version {version:08x}) at {:#x}", name.to_string_lossy(), module as usize);
    PLUGINS.lock().unwrap().push(Plugin { name, file: file.to_owned(), info: core::ptr::from_ref(info) as usize });
}

unsafe extern "C" fn query_interface(id: u32) -> *mut c_void {
    let (name, table): (&str, *const c_void) = match id {
        1 => ("Scaleform, not implemented", core::ptr::null()),
        2 => ("Papyrus", (&raw const PAPYRUS_TABLE).cast()),
        3 => ("serialization, not implemented", core::ptr::null()),
        4 => ("task", (&raw const TASK_TABLE).cast()),
        5 => ("messaging", (&raw const MESSAGING).cast()),
        6 => ("object, not implemented", core::ptr::null()),
        7 => ("trampoline", (&raw const TRAMPOLINE_TABLE).cast()),
        _ => ("unknown", core::ptr::null()),
    };
    log!("QueryInterface({id}) from handle {}: {name}", CURRENT.load(Ordering::Relaxed));
    table.cast_mut()
}

unsafe extern "C" fn get_plugin_handle() -> PluginHandle {
    CURRENT.load(Ordering::Relaxed)
}

unsafe extern "C" fn get_release_index() -> u32 {
    LOADER.get().map_or(0, |l| l.release_index)
}

unsafe extern "C" fn get_plugin_info(name: *const c_char) -> *const PluginInfo {
    if name.is_null() || LOADED.load(Ordering::Acquire) == 0 {
        return core::ptr::null();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    PLUGINS.lock().unwrap().iter().find(|p| p.name.as_bytes().eq_ignore_ascii_case(name)).map_or(core::ptr::null(), |p| p.info as *const PluginInfo)
}

/// A plugin's name by handle; 0 is the loader, "SKSE".
fn sender_name(handle: PluginHandle) -> Option<*const c_char> {
    if handle == 0 {
        return Some(c"SKSE".as_ptr());
    }
    PLUGINS.lock().unwrap().get(handle as usize - 1).map(|p| p.name.as_ptr())
}

unsafe extern "C" fn register_listener(listener: PluginHandle, sender: *const c_char, callback: MessageCallback) -> bool {
    let sender = (!sender.is_null()).then(|| unsafe { CStr::from_ptr(sender) }.to_string_lossy().to_lowercase());
    log!("handle {listener} listens to {}", sender.as_deref().unwrap_or("every plugin"));
    LISTENERS.lock().unwrap().push(Listener { handle: listener, sender, callback });
    true
}

unsafe extern "C" fn dispatch(sender: PluginHandle, kind: u32, data: *mut c_void, len: u32, receiver: *const c_char) -> bool {
    let Some(name) = sender_name(sender) else { return false };
    let sender_lower = unsafe { CStr::from_ptr(name) }.to_string_lossy().to_lowercase();
    let receiver = (!receiver.is_null()).then(|| unsafe { CStr::from_ptr(receiver) }.to_string_lossy().to_lowercase());
    let plugins: Vec<String> = PLUGINS.lock().unwrap().iter().map(|p| p.name.to_string_lossy().to_lowercase()).collect();
    let callbacks: Vec<MessageCallback> = LISTENERS
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.sender.as_ref().map_or(sender != 0 && sender != l.handle, |s| *s == sender_lower))
        .filter(|l| receiver.as_ref().is_none_or(|r| plugins.get(l.handle as usize - 1).is_some_and(|n| n == r)))
        .map(|l| l.callback)
        .collect();
    if sender == 0 {
        log!("kind {kind} ({len} bytes) to {} listeners", callbacks.len());
    }
    for callback in callbacks {
        let mut message = Message { sender: name, kind, data_len: len, data };
        unsafe { callback(&raw mut message) };
    }
    true
}

unsafe extern "C" fn get_event_dispatcher(id: u32) -> *mut c_void {
    log!("GetEventDispatcher({id}): not implemented");
    core::ptr::null_mut()
}

fn send(kind: u32, data: *mut c_void, len: u32) {
    unsafe { dispatch(0, kind, data, len, core::ptr::null()) };
}

unsafe extern "C" fn register_papyrus(callback: RegisterFunctions) -> bool {
    PAPYRUS.lock().unwrap().push(callback);
    true
}

unsafe extern "C" fn add_task(task: *mut c_void) {
    TASKS.lock().unwrap().push_back(task as usize);
}

unsafe extern "C" fn add_ui_task(task: *mut c_void) {
    let queue = RELOC.get().and_then(|r| r.address(globals::g_UIMessageQueue_ptr).ok()).is_some_and(|p| !unsafe { *(p as *const *const c_void) }.is_null());
    if queue {
        UI_TASKS.lock().unwrap().push_back(task as usize);
    }
}

unsafe extern "C" fn alloc_near(handle: PluginHandle, size: u64) -> *mut c_void {
    let base = RELOC.get().map_or(0, hook::Reloc::base);
    match hook::alloc_near(base, usize::try_from(size).unwrap_or(usize::MAX)) {
        Ok(p) => {
            log!("handle {handle} allocated {size} bytes at {p:#x}");
            p as *mut c_void
        }
        Err(e) => {
            log!("handle {handle} asked for {size} bytes: {e}");
            core::ptr::null_mut()
        }
    }
}

/// Runs each queued task: slot 0 of its vtable, then slot 1 to dispose of it.
fn drain(queue: &Mutex<VecDeque<usize>>) {
    loop {
        let Some(task) = queue.lock().unwrap().pop_front() else { return };
        let vtable = unsafe { *(task as *const *const unsafe extern "C" fn(usize)) };
        unsafe { (*vtable)(task) };
        unsafe { (*vtable.add(1))(task) };
    }
}

type LoadGameFn = unsafe extern "C" fn(*mut c_void, *mut c_void, bool, u32, *mut c_void, bool) -> bool;
type SaveGameFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u64;
type DeleteSaveFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> u64;
type ControlsFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type StartQuestFn = unsafe extern "C" fn(*mut c_void, *mut bool, bool) -> bool;
type OneArgFn = unsafe extern "C" fn(*mut c_void) -> u64;
type RegisterNativesFn = unsafe extern "C" fn(*mut *mut c_void, u64, u64, u64) -> u64;

static LOAD_GAME: OnceLock<hook::CallSite<LoadGameFn>> = OnceLock::new();
static SAVE_GAME: OnceLock<hook::CallSite<SaveGameFn>> = OnceLock::new();
static DELETE_SAVE: OnceLock<hook::CallSite<DeleteSaveFn>> = OnceLock::new();
static CONTROLS: OnceLock<hook::CallSite<ControlsFn>> = OnceLock::new();
static NEW_GAME: OnceLock<hook::CallSite<StartQuestFn>> = OnceLock::new();
static INIT_DONE: OnceLock<hook::CallSite<OneArgFn>> = OnceLock::new();
static VM_SETUP: OnceLock<hook::CallSite<RegisterNativesFn>> = OnceLock::new();
static DRAIN: OnceLock<hook::CallSite<OneArgFn>> = OnceLock::new();
static UI_DRAIN: OnceLock<hook::CallSite<OneArgFn>> = OnceLock::new();
static REQUESTS: OnceLock<hook::CallSite<OneArgFn>> = OnceLock::new();

/// A `call rel32` the script extender hooks (S1): `caller` + `offset` calls `callee`, as AE ids and a
/// 1.7.104 offset.
#[derive(Clone, Copy)]
struct Site {
    moment: &'static str,
    caller: u64,
    offset: usize,
    callee: u64,
}

const SITE_LOAD: Site = Site { moment: "kinds 2 and 3, the game load", caller: fns::BGSSaveLoadManager__LoadGame, offset: 0x34f, callee: fns::BGSSaveLoadGame__LoadGame };
const SITE_SAVE: Site = Site { moment: "kind 4, the game save", caller: fns::BGSSaveLoadManager__Save, offset: 0x2b3, callee: fns::BGSSaveLoadGame__SaveGame };
const SITE_DELETE_OVER: Site = Site { moment: "kind 5, a save overwritten", caller: fns::SaveOverEntry, offset: 0x77, callee: fns::BGSSaveLoadManager__DeleteSaveFile };
const SITE_DELETE: Site = Site { moment: "kind 5, a save deleted", caller: fns::DeleteSaveEntry, offset: 0x17, callee: fns::BGSSaveLoadManager__DeleteSaveFile };
const SITE_CONTROLS: Site = Site { moment: "kind 6, the player controls", caller: fns::Main__ctor, offset: 0xd7a, callee: fns::PlayerControls__ctor };
const SITE_NEW_GAME: Site = Site { moment: "kind 7, a new game", caller: fns::MainMenu__StartNewGame, offset: 0x59, callee: fns::TryStartQuest };
const SITE_INIT_DONE: Site = Site { moment: "kind 8, the data load", caller: fns::InitTESThread__vfunc_1_sub0, offset: 0x3f7, callee: fns::TESDataHandler__CallSlot19OnArray1D8 };
const SITE_VM: Site = Site { moment: "Papyrus registration", caller: fns::SkyrimVM__ctor, offset: 0xcfb, callee: fns::SkyrimVM__RegisterNatives };
const SITE_DRAIN_WORLD: Site = Site { moment: "the task queue, first drain", caller: fns::SetupWorldRenderContext, offset: 0x178e, callee: fns::TaskQueue__Drain };
const SITE_DRAIN_REFS: Site = Site { moment: "the task queue, second drain", caller: fns::ProcessReferenceQueuesAndDrainTaskQueue, offset: 0x1c, callee: fns::TaskQueue__Drain };
const SITE_UI_DRAIN: Site = Site { moment: "the UI tasks", caller: fns::EventSourceHolder_MenuOpenClose__ProcessMessages, offset: 0xab5, callee: fns::UIMessageQueue__ReleaseStaticMessages };
const SITE_REQUESTS: Site = Site { moment: "the save and load requests", caller: fns::SetupWorldRenderContext, offset: 0xc7a, callee: fns::ProcessSaveLoadRequests };

const SITES: [Site; 12] = [SITE_LOAD, SITE_SAVE, SITE_DELETE_OVER, SITE_DELETE, SITE_CONTROLS, SITE_NEW_GAME, SITE_INIT_DONE, SITE_VM, SITE_DRAIN_WORLD, SITE_DRAIN_REFS, SITE_UI_DRAIN, SITE_REQUESTS];

/// The sites a runtime has no offset for: all of them until its profile gives the offsets.
fn missing_sites(runtime: u32) -> &'static [Site] {
    if skse::runtime::is_canonical(runtime) { &[] } else { &SITES }
}

/// Where `site` is, after checking that it calls its callee.
fn locate(site: Site) -> Result<usize, String> {
    let reloc = RELOC.get().ok_or("no Address Library")?;
    let resolve = |id| reloc.address(id).map_err(|e| e.to_string());
    let (at, target) = (resolve(site.caller)? + site.offset, resolve(site.callee)?);
    (hook::find_call(at, 5, target) == Some(at)).then_some(at).ok_or_else(|| format!("no call to {target:#x} at {at:#x}"))
}

fn patch<F: Copy>(cell: &OnceLock<hook::CallSite<F>>, at: usize, detour: F) -> Result<(), String> {
    let installed = unsafe { hook::CallSite::install(at, detour) }.map_err(|e| e.to_string())?;
    if cell.set(installed).is_err() {
        return Err(format!("{at:#x} patched twice"));
    }
    Ok(())
}

fn install_moments() -> Result<(), String> {
    patch(&LOAD_GAME, locate(SITE_LOAD)?, load_game as LoadGameFn)?;
    patch(&SAVE_GAME, locate(SITE_SAVE)?, save_game as SaveGameFn)?;
    let delete = [locate(SITE_DELETE_OVER)?, locate(SITE_DELETE)?];
    for at in delete {
        let installed = unsafe { hook::CallSite::install(at, delete_save as DeleteSaveFn) }.map_err(|e| e.to_string())?;
        let _ = DELETE_SAVE.set(installed);
    }
    patch(&CONTROLS, locate(SITE_CONTROLS)?, controls_ready as ControlsFn)?;
    patch(&NEW_GAME, locate(SITE_NEW_GAME)?, new_game as StartQuestFn)?;
    patch(&INIT_DONE, locate(SITE_INIT_DONE)?, init_done as OneArgFn)?;
    patch(&VM_SETUP, locate(SITE_VM)?, register_natives as RegisterNativesFn)?;
    for site in [SITE_DRAIN_WORLD, SITE_DRAIN_REFS] {
        let installed = unsafe { hook::CallSite::install(locate(site)?, drain_tasks as OneArgFn) }.map_err(|e| e.to_string())?;
        let _ = DRAIN.set(installed);
    }
    patch(&UI_DRAIN, locate(SITE_UI_DRAIN)?, drain_ui_tasks as OneArgFn)?;
    patch(&REQUESTS, locate(SITE_REQUESTS)?, process_requests as OneArgFn)?;
    log!("moment hooks installed: load, save, delete, controls, new game, init done, Papyrus, task and UI task drains, save/load requests");
    Ok(())
}

fn send_name(kind: u32, name: *const c_char) {
    let (data, len) = if name.is_null() { (c"".as_ptr(), 0) } else { (name, unsafe { CStr::from_ptr(name) }.to_bytes().len()) };
    send(kind, data.cast_mut().cast(), u32::try_from(len).unwrap_or(u32::MAX));
}

fn stream_name(stream: *mut c_void) -> *const c_char {
    if stream.is_null() { core::ptr::null() } else { unsafe { stream.cast::<u8>().add(SAVE_NAME).cast::<*const c_char>().read_unaligned() } }
}

unsafe extern "C" fn load_game(this: *mut c_void, stream: *mut c_void, ignore_missing: bool, kind: u32, missing: *mut c_void, skip_match: bool) -> bool {
    send_name(MessagingInterface::PRE_LOAD_GAME, stream_name(stream));
    let ok = unsafe { LOAD_GAME.get().expect("set before patching").original()(this, stream, ignore_missing, kind, missing, skip_match) };
    send(MessagingInterface::POST_LOAD_GAME, usize::from(ok) as *mut c_void, 1);
    ok
}

unsafe extern "C" fn save_game(this: *mut c_void, stream: *mut c_void) -> u64 {
    send_name(MessagingInterface::SAVE_GAME, stream_name(stream));
    unsafe { SAVE_GAME.get().expect("set before patching").original()(this, stream) }
}

unsafe extern "C" fn delete_save(manager: *mut c_void, name: *const c_char) -> u64 {
    send_name(MessagingInterface::DELETE_GAME, name);
    unsafe { DELETE_SAVE.get().expect("set before patching").original()(manager, name) }
}

unsafe extern "C" fn controls_ready(this: *mut c_void) -> *mut c_void {
    let out = unsafe { CONTROLS.get().expect("set before patching").original()(this) };
    send(MessagingInterface::CONTROLS_READY, core::ptr::null_mut(), 0);
    out
}

unsafe extern "C" fn new_game(quest: *mut c_void, started: *mut bool, now: bool) -> bool {
    let out = unsafe { NEW_GAME.get().expect("set before patching").original()(quest, started, now) };
    send(MessagingInterface::NEW_GAME, quest, 8);
    out
}

unsafe extern "C" fn init_done(this: *mut c_void) -> u64 {
    let out = unsafe { INIT_DONE.get().expect("set before patching").original()(this) };
    send(MessagingInterface::DATA_LOADED, core::ptr::null_mut(), 0);
    out
}

/// `SkyrimVM::RegisterNatives` (S2) takes the address of the VM pointer at `g_SkyrimVM` + 0x210 and
/// passes its other three registers on to the engine's registration functions.
unsafe extern "C" fn register_natives(registry: *mut *mut c_void, a2: u64, a3: u64, a4: u64) -> u64 {
    let out = unsafe { VM_SETUP.get().expect("set before patching").original()(registry, a2, a3, a4) };
    let vm = if registry.is_null() { core::ptr::null_mut() } else { unsafe { *registry } };
    if let Some(vm) = unsafe { skyrim::papyrus::VirtualMachine::from_registry(vm) } {
        let save = vm.register_native("Game", "SaveGame", |name: String| *PENDING.lock().unwrap() = Some((Request::Save(name), Instant::now())));
        let load = vm.register_native("Game", "LoadGame", |name: String| *PENDING.lock().unwrap() = Some((Request::Load(name), Instant::now())));
        log!("Game.SaveGame and Game.LoadGame: {}, {}", save.is_ok(), load.is_ok());
    }
    let callbacks = PAPYRUS.lock().unwrap().clone();
    log!("Papyrus registration: {} callbacks with {vm:p}", callbacks.len());
    for callback in callbacks {
        unsafe { callback(vm) };
    }
    out
}

unsafe extern "C" fn drain_tasks(holder: *mut c_void) -> u64 {
    let out = unsafe { DRAIN.get().expect("set before patching").original()(holder) };
    drain(&TASKS);
    out
}

/// The main loop's call to `ProcessSaveLoadRequests` (S3): the manager's own requests, then the one a
/// plugin left through the `Game` natives.
unsafe extern "C" fn process_requests(manager: *mut c_void) -> u64 {
    let out = unsafe { REQUESTS.get().expect("set before patching").original()(manager) };
    let mut pending = PENDING.lock().unwrap();
    if let (Some((request, since)), Some(manager)) = (pending.as_ref(), sre::BGSSaveLoadManager::singleton()) {
        let (what, name, ok) = match request {
            Request::Save(name) => ("save", name, manager.save_as(name)),
            Request::Load(name) => ("load", name, manager.load_save(name)),
        };
        let waited = since.elapsed();
        if ok || waited >= RETRY {
            log!("{what} {name:?}: {ok} after {} ms", waited.as_millis());
            *pending = None;
        }
    }
    out
}

unsafe extern "C" fn drain_ui_tasks(queue: *mut c_void) -> u64 {
    let out = unsafe { UI_DRAIN.get().expect("set before patching").original()(queue) };
    drain(&UI_TASKS);
    out
}
