//! The plugin loader without the script extender: the acceptance rules, the plugin interface and
//! the message moments of request S1, on the game call sites S1 found the script extender hooking.
//!
//! Plugins in `Data\SKSE\Plugins` that export `SKSEPlugin_Version` are loaded in file name order and
//! checked like SKSE 2.3.1 checks them; `SKSEPlugin_Preload` then `SKSEPlugin_Load` run for the
//! accepted ones, kinds 0 and 1 are sent, and the hooks below send the others. `QueryInterface`
//! answers the Papyrus (2), task (4), messaging (5) and trampoline (7) tables; Scaleform (1),
//! serialization (3) and the object table (6) are not implemented and answer null.
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
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use layout::v1_7_104::{fns, globals};
use skse::{Interface, Message, MessageCallback, MessagingInterface, PapyrusInterface, PluginHandle, PluginInfo, PluginVersionData, RegisterFunctions, TaskInterface, TrampolineInterface, compat};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};

/// The interface level implemented: SKSE 2.3.1's, as S1 read it.
const LOADER_VERSION: u32 = 0x0203_0010;
const RELEASE_INDEX: u32 = 75;
pub(crate) const RUNTIME: u32 = skse::runtime::pack(1, 7, 104, 0);
const PLUGIN_DIR: &str = "Data\\SKSE\\Plugins";
/// SKSE 2.3.1's link-timestamp gate: a plugin linked in this range needs `FLAG_NO_TIMESTAMP_GATE` or the runtime in its list.
const TIMESTAMP_GATE: core::ops::RangeInclusive<u32> = 520_128_000..=1_748_217_599;
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
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg);
        }
    };
}

static INTERFACE: Interface = Interface {
    skse_version: LOADER_VERSION,
    runtime_version: RUNTIME,
    reserved: [0; 2],
    query_interface,
    get_plugin_handle,
    get_release_index,
    get_plugin_info,
};
static MESSAGING: MessagingInterface = MessagingInterface { interface_version: MessagingInterface::VERSION, register_listener, dispatch, get_event_dispatcher };
static PAPYRUS_TABLE: PapyrusInterface = PapyrusInterface { interface_version: PapyrusInterface::VERSION, register: register_papyrus };
static TASK_TABLE: TaskInterface = TaskInterface { interface_version: TaskInterface::VERSION, add_task, add_ui_task };
static TRAMPOLINE_TABLE: TrampolineInterface = TrampolineInterface { interface_version: TrampolineInterface::VERSION, alloc_branch: alloc_near, alloc_local: alloc_near };

/// Loads and starts the plugins, then hooks the moments; on the main thread right before `WinMain`.
pub fn run() {
    if let Ok(file) = skse::log::open(RUNTIME, "skyrim-rs-standalone") {
        *LOG.lock().unwrap() = Some(file);
    }
    log!("skyrim-rs standalone host, plugin interface {LOADER_VERSION:08x}, runtime {RUNTIME:08x}");
    match hook::Reloc::open(RUNTIME) {
        Ok(reloc) => {
            let _ = RELOC.set(reloc);
        }
        Err(e) => {
            log!("Address Library: {e}; nothing loaded");
            return;
        }
    }
    if let Err(e) = skyrim::init(&INTERFACE) {
        log!("game layer: {e}; nothing loaded");
        return;
    }
    let accepted = scan();
    for (handle, (path, data)) in (1..).zip(&accepted) {
        start(handle, path, data);
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
pub fn defer() {
    if let Ok(mut file) = skse::log::open(RUNTIME, "skyrim-rs-standalone") {
        let _ = writeln!(file, "the script extender is loaded; it hosts the plugins");
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// Every DLL with version data in file name order, each kept loaded when the acceptance rules pass.
fn scan() -> Vec<(String, &'static PluginVersionData)> {
    let mut files: Vec<_> = std::fs::read_dir(PLUGIN_DIR).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"))).collect();
    files.sort();
    let mut accepted = Vec::new();
    for path in files {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let module = unsafe { LoadLibraryW(wide(&path).as_ptr()) };
        if module.is_null() {
            log!("{file}: could not be loaded ({})", std::io::Error::last_os_error());
            continue;
        }
        let data = unsafe { GetProcAddress(module, c"SKSEPlugin_Version".as_ptr().cast()) }.map(|p| unsafe { &*(p as *const PluginVersionData) });
        match data.ok_or("no version data").and_then(|d| accept(module, d).map(|()| d)) {
            Ok(data) => accepted.push((file, data)),
            Err(reason) => {
                log!("{file}: disabled, {reason}");
                unsafe { FreeLibrary(module) };
            }
        }
    }
    accepted
}

/// SKSE 2.3.1's checks in its order (S1).
fn accept(module: HMODULE, data: &PluginVersionData) -> Result<(), &'static str> {
    if data.data_version == 0 {
        return Err("bad version data");
    }
    if data.name[0] == 0 {
        return Err("no name specified");
    }
    if data.method > 7 {
        return Err("unsupported version independence method");
    }
    let listed = data.compatible_versions.iter().take_while(|&&v| v != 0).any(|&v| v == RUNTIME);
    if data.method & compat::METHOD_ADDRESS_LIBRARY != 0 {
        if !Path::new(PLUGIN_DIR).join(skse::runtime::versionlib_name(RUNTIME)).exists() {
            return Err("address library needs to be updated");
        }
        if data.flags & compat::FLAG_NO_TIMESTAMP_GATE == 0 && !listed && TIMESTAMP_GATE.contains(&timestamp(module)) {
            return Err("must be recompiled for new address library");
        }
    }
    if data.method & (compat::METHOD_ADDRESS_LIBRARY | compat::METHOD_NO_FILE_CHECK) != 0 {
        if data.method & compat::METHOD_STRUCTS_1_6_629 == 0 && data.flags & compat::FLAG_STRUCTS_1_6_629 == 0 {
            return Err("only compatible with versions earlier than 1.6.629");
        }
    } else if !listed {
        return Err("incompatible with current version of the game");
    }
    if data.minimum_skse > LOADER_VERSION {
        return Err("requires newer script extender");
    }
    Ok(())
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

fn start(handle: PluginHandle, file: &str, data: &PluginVersionData) {
    let name = CStr::from_bytes_until_nul(&data.name).map(CStr::to_owned).unwrap_or_default();
    let module = unsafe { GetModuleHandleW(wide(&Path::new(PLUGIN_DIR).join(file)).as_ptr()) };
    let export = |symbol: &CStr| unsafe { GetProcAddress(module, symbol.as_ptr().cast()) };
    CURRENT.store(handle, Ordering::Relaxed);
    let preloaded = export(c"SKSEPlugin_Preload").is_none_or(|f| unsafe { core::mem::transmute::<unsafe extern "system" fn() -> isize, PreloadFn>(f)(&raw const INTERFACE) });
    let loaded = preloaded && export(c"SKSEPlugin_Load").is_some_and(|f| unsafe { core::mem::transmute::<unsafe extern "system" fn() -> isize, LoadFn>(f)(&raw const INTERFACE) });
    CURRENT.store(skse::INVALID_HANDLE, Ordering::Relaxed);
    if !loaded {
        log!("{file}: disabled, reported as incompatible during load");
        return;
    }
    let info = Box::leak(Box::new(PluginInfo { info_version: PluginInfo::VERSION, name: name.as_ptr(), version: data.plugin_version }));
    log!("{file}: loaded as handle {handle} ({}, version {:08x}) at {:#x}", name.to_string_lossy(), data.plugin_version, module as usize);
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
    RELEASE_INDEX
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

/// The `call rel32` at `caller` + `offset`, after checking that it calls `callee` (S1's sites).
fn site(caller: u64, offset: usize, target_id: u64) -> Result<usize, String> {
    let reloc = RELOC.get().ok_or("no Address Library")?;
    let resolve = |id| reloc.address(id).map_err(|e| e.to_string());
    let (at, target) = (resolve(caller)? + offset, resolve(target_id)?);
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
    patch(&LOAD_GAME, site(fns::BGSSaveLoadManager__LoadGame, 0x34f, fns::BGSSaveLoadGame__LoadGame)?, load_game as LoadGameFn)?;
    patch(&SAVE_GAME, site(fns::BGSSaveLoadManager__Save, 0x2b3, fns::BGSSaveLoadGame__SaveGame)?, save_game as SaveGameFn)?;
    let delete = DELETE_SAVE_SITES.iter().map(|&(caller, offset)| site(caller, offset, fns::BGSSaveLoadManager__DeleteSaveFile)).collect::<Result<Vec<_>, _>>()?;
    for at in delete {
        let installed = unsafe { hook::CallSite::install(at, delete_save as DeleteSaveFn) }.map_err(|e| e.to_string())?;
        let _ = DELETE_SAVE.set(installed);
    }
    patch(&CONTROLS, site(fns::Main__ctor, 0xd7a, fns::PlayerControls__ctor)?, controls_ready as ControlsFn)?;
    patch(&NEW_GAME, site(fns::MainMenu__StartNewGame, 0x59, fns::TryStartQuest)?, new_game as StartQuestFn)?;
    patch(&INIT_DONE, site(fns::InitTESThread__vfunc_1_sub0, 0x3f7, fns::TESDataHandler__CallSlot19OnArray1D8)?, init_done as OneArgFn)?;
    patch(&VM_SETUP, site(fns::SkyrimVM__ctor, 0xcfb, fns::SkyrimVM__RegisterNatives)?, register_natives as RegisterNativesFn)?;
    for (caller, offset) in DRAIN_SITES {
        let installed = unsafe { hook::CallSite::install(site(caller, offset, fns::TaskQueue__Drain)?, drain_tasks as OneArgFn) }.map_err(|e| e.to_string())?;
        let _ = DRAIN.set(installed);
    }
    patch(&UI_DRAIN, site(fns::EventSourceHolder_MenuOpenClose__ProcessMessages, 0xab5, fns::UIMessageQueue__ReleaseStaticMessages)?, drain_ui_tasks as OneArgFn)?;
    patch(&REQUESTS, site(fns::SetupWorldRenderContext, 0xc7a, fns::ProcessSaveLoadRequests)?, process_requests as OneArgFn)?;
    log!("moment hooks installed: load, save, delete, controls, new game, init done, Papyrus, task and UI task drains, save/load requests");
    Ok(())
}

/// Both of the game's calls to `DeleteSaveFile` the script extender hooks; they reach one callee.
const DELETE_SAVE_SITES: [(u64, usize); 2] = [(fns::SaveOverEntry, 0x77), (fns::DeleteSaveEntry, 0x17)];
/// The main loop's two calls to `TaskQueue::Drain`.
const DRAIN_SITES: [(u64, usize); 2] = [(fns::SetupWorldRenderContext, 0x178e), (fns::ProcessReferenceQueuesAndDrainTaskQueue, 0x1c)];

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
    if let Some(vm) = unsafe { skyrim::papyrus::VirtualMachine::from_ptr(vm) } {
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
    if let (Some((request, since)), Some(manager)) = (pending.as_ref(), skyrim::save::SaveLoadManager::get()) {
        let (what, name, ok) = match request {
            Request::Save(name) => ("save", name, manager.save(name)),
            Request::Load(name) => ("load", name, manager.request_load(name)),
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
