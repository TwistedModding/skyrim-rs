//! Hot reload: every plugin runs from a shadow copy under `skyrim-runtime-extender\.loaded\`, the author's file
//! is polled once a second from the frame, and a changed file that can be opened exclusively (the
//! copy has finished) is swapped in at that frame boundary on the main thread.
//!
//! The swap: copy the file as the next generation, check its manifest and read its descriptor (a
//! refusal keeps the running generation); ask the running generation for its state and call its `unload`; detach
//! every registration of the plugin, which drains the calls inside them; unload the DLL; call the
//! new generation's `load` with the plugin's own `HostApi`, so it registers again and the host
//! re-points what the engine keeps; hand it the state. A `load` that returns false is detached and
//! unloaded, and the previous generation's copy is loaded again with the same state. A panic in
//! `load` aborts the game (release builds abort), so only failures before `load` are recoverable.
//!
//! The Plugins tab and the console ask for the same at the next frame ([`request`]): a reload of an
//! unchanged file, an unload (the plugin keeps its state and is not reloaded until it is loaded
//! again), a load of an unloaded plugin from its file, and a rescan of the folder.

use std::fs;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};

use super::{detach, flush_settings, plugin_name, read_checked, read_descriptor, rescan};
use crate::abi::{HostApi, Sink, Str};
use crate::{Descriptor, Plugin};

pub(super) const SHADOW_DIR: &str = ".loaded";
const POLL: Duration = Duration::from_secs(1);

type Stamp = (u64, SystemTime);

/// A plugin's state as the Plugins tab and the crash log show it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Running,
    /// Its `load` returned false.
    Stopped,
    /// Unloaded from the Plugins tab or the console.
    Unloaded,
}

/// What the Plugins tab or the console asked for, done at the next frame.
pub(super) enum Request {
    Reload(String),
    Unload(String),
    Load(String),
    Rescan,
}

/// One plugin as the host runs it: the generation loaded now and the file it came from.
pub(super) struct Loaded {
    plugin: Plugin,
    api: &'static HostApi,
    /// Null while the plugin is unloaded.
    module: HMODULE,
    descriptor: &'static Descriptor,
    /// Whether the generation's `load` returned true.
    live: bool,
    source: PathBuf,
    copy: PathBuf,
    generation: u32,
    stamp: Option<Stamp>,
    /// The state the unloaded generation saved, for the next one.
    state: Option<Vec<u8>>,
}

unsafe impl Send for Loaded {}

impl Loaded {
    pub(super) fn new(plugin: Plugin, api: &'static HostApi, module: HMODULE, descriptor: &'static Descriptor, live: bool, source: PathBuf, copy: PathBuf) -> Self {
        let stamp = stamp(&source);
        Self { plugin, api, module, descriptor, live, source, copy, generation: 1, stamp, state: None }
    }

    pub(super) fn source(&self) -> &Path {
        &self.source
    }

    fn status(&self) -> Status {
        match (self.module.is_null(), self.live) {
            (true, _) => Status::Unloaded,
            (false, true) => Status::Running,
            (false, false) => Status::Stopped,
        }
    }
}

static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());
static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// Each watched plugin's name, running generation and status.
pub(super) fn generations() -> Vec<(String, u32, Status)> {
    LOADED.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|l| (l.plugin.name.clone(), l.generation, l.status())).collect()
}

/// [`generations`] without waiting: `None` while another thread holds the list (the crash log's caller).
pub(super) fn try_generations() -> Option<Vec<(String, u32, Status)>> {
    LOADED.try_lock().ok().map(|loaded| loaded.iter().map(|l| (l.plugin.name.clone(), l.generation, l.status())).collect())
}

/// Queues `request` for the next frame, where reloads run.
pub(super) fn request(request: Request) {
    REQUESTS.lock().unwrap_or_else(PoisonError::into_inner).push(request);
}
static LAST_POLL: Mutex<Option<Instant>> = Mutex::new(None);

/// Empties the shadow folder under `dir` and returns it.
pub(super) fn clean(dir: &Path) -> PathBuf {
    let shadow = dir.join(SHADOW_DIR);
    let _ = fs::remove_dir_all(&shadow);
    let _ = fs::create_dir_all(&shadow);
    shadow
}

/// Copies `source` into `shadow` as `<stem>.<generation>.dll`, and a `<stem>.pdb` next to it as
/// is, so the crash log names the plugin's functions.
pub(super) fn copy(source: &Path, shadow: &Path, generation: u32) -> io::Result<PathBuf> {
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let copy = shadow.join(format!("{stem}.{generation}.dll"));
    fs::copy(source, &copy)?;
    let pdb = source.with_extension("pdb");
    if pdb.is_file() {
        let _ = fs::copy(&pdb, shadow.join(format!("{stem}.pdb")));
    }
    Ok(copy)
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Whether nothing else has the file open: a copy in progress holds it.
fn settled(path: &Path) -> bool {
    fs::OpenOptions::new().read(true).share_mode(0).open(path).is_ok()
}

pub(super) fn watch(loaded: Vec<Loaded>) {
    *LOADED.lock().unwrap_or_else(PoisonError::into_inner) = loaded;
}

/// The frame callback: carries out what was asked for, and once a second swaps in every loaded
/// plugin whose file changed and settled.
pub(super) fn tick() {
    let requests = core::mem::take(&mut *REQUESTS.lock().unwrap_or_else(PoisonError::into_inner));
    let due = {
        let mut last = LAST_POLL.lock().unwrap_or_else(PoisonError::into_inner);
        let due = last.is_none_or(|t| t.elapsed() >= POLL);
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if requests.is_empty() && !due {
        return;
    }
    if due {
        flush_settings(None);
    }
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    for request in requests {
        let named = |name: &str, loaded: &mut Vec<Loaded>| loaded.iter_mut().position(|l| l.plugin.name == name);
        match request {
            Request::Rescan => rescan(&mut loaded),
            Request::Reload(name) | Request::Load(name) if let Some(i) = named(&name, &mut loaded) => {
                let entry = &mut loaded[i];
                entry.stamp = stamp(&entry.source);
                if entry.module.is_null() {
                    load(entry);
                } else {
                    swap(entry, "reload asked for");
                }
            }
            Request::Unload(name) if let Some(i) = named(&name, &mut loaded) => unload(&mut loaded[i]),
            Request::Reload(name) | Request::Load(name) | Request::Unload(name) => log!("no plugin named {name}"),
        }
    }
    if !due {
        return;
    }
    for entry in loaded.iter_mut().filter(|l| !l.module.is_null()) {
        let now = stamp(&entry.source);
        if now.is_none() || now == entry.stamp || !settled(&entry.source) {
            continue;
        }
        entry.stamp = now;
        let why = format!("{} changed", entry.plugin.file);
        swap(entry, &why);
    }
}

/// Copies the plugin's file as `generation` and reads it; a copy whose descriptor names another
/// plugin is refused.
fn next(entry: &Loaded, generation: u32) -> Result<(HMODULE, &'static Descriptor, PathBuf), String> {
    let shadow = entry.copy.parent().map(Path::to_path_buf).unwrap_or_default();
    let copy = copy(&entry.source, &shadow, generation).map_err(|e| format!("not copied: {e}"))?;
    let read = read_checked(&copy, &entry.plugin.file).and_then(|(module, descriptor)| {
        let declared = plugin_name(descriptor);
        if declared == entry.plugin.name {
            Ok((module, descriptor, copy.clone()))
        } else {
            unsafe { FreeLibrary(module) };
            Err(format!("its descriptor names {declared}"))
        }
    });
    if read.is_err() {
        let _ = fs::remove_file(&copy);
    }
    read.map_err(|e| format!("refused: {e}"))
}

/// Unloads the running generation and returns the state it saved: its `unload`, every registration
/// detached, its settings written, its DLL freed. Its shadow copy stays.
fn stop(entry: &Loaded) -> Option<Vec<u8>> {
    let name = &entry.plugin.name;
    let state = entry.descriptor.save_state.filter(|_| entry.live).map(|save| {
        let mut bytes = Vec::new();
        let mut keep = |written: Str| bytes.extend_from_slice(unsafe { written.bytes() });
        save(Sink::new(&mut keep));
        bytes
    });
    if let Some(bytes) = &state {
        log!("{name}: generation {}: {} bytes of state saved", entry.generation, bytes.len());
    }
    if entry.live
        && let Some(unload) = entry.descriptor.unload
    {
        unload();
    }
    log!("{name}: generation {}: {} detached", entry.generation, detach(name));
    flush_settings(Some(name));
    unsafe { FreeLibrary(entry.module) };
    crash::note_image(name, entry.generation, entry.module as usize, false);
    log!("{name}: generation {} unloaded", entry.generation);
    state
}

/// Unloads the plugin until it is loaded again, keeping the state it saved for that load.
fn unload(entry: &mut Loaded) {
    if entry.module.is_null() {
        return;
    }
    entry.state = stop(entry);
    let _ = fs::remove_file(&entry.copy);
    entry.module = core::ptr::null_mut();
    entry.live = false;
    log!("{}: unloaded until it is loaded again", entry.plugin.name);
}

/// Loads an unloaded plugin from its file as the next generation, with the state it saved.
fn load(entry: &mut Loaded) {
    let generation = entry.generation + 1;
    match next(entry, generation) {
        Ok((module, descriptor, copy)) => {
            let state = entry.state.take();
            if !start(entry, module, descriptor, copy, generation, state.as_deref()) {
                log!("{}: generation {generation}: load returned false", entry.plugin.name);
            }
        }
        Err(e) => log!("{}: generation {generation} {e}; the plugin stays unloaded", entry.plugin.name),
    }
}

fn swap(entry: &mut Loaded, why: &str) {
    let name = entry.plugin.name.clone();
    let generation = entry.generation + 1;
    log!("{name}: {why}, reloading as generation {generation}");
    let (module, descriptor, copy) = match next(entry, generation) {
        Ok(read) => read,
        Err(e) => {
            log!("{name}: generation {generation} {e}; generation {} stays", entry.generation);
            return;
        }
    };
    let state = stop(entry);
    let previous = (entry.copy.clone(), entry.generation);
    if start(entry, module, descriptor, copy, generation, state.as_deref()) {
        let _ = fs::remove_file(&previous.0);
        return;
    }
    log!("{name}: generation {generation}: load returned false; loading generation {} again", previous.1);
    unsafe { FreeLibrary(entry.module) };
    crash::note_image(&name, generation, entry.module as usize, false);
    let _ = fs::remove_file(&entry.copy);
    match read_descriptor(&previous.0) {
        Ok((module, descriptor)) => {
            if !start(entry, module, descriptor, previous.0, previous.1, state.as_deref()) {
                log!("{name}: generation {}: load returned false again; the plugin is unloaded until its file changes", previous.1);
            }
        }
        Err(e) => log!("{name}: generation {} refused: {e}; the plugin is unloaded until its file changes", previous.1),
    }
}

/// Makes `module` the running generation and calls its `load`; a false return detaches what it registered.
fn start(entry: &mut Loaded, module: HMODULE, descriptor: &'static Descriptor, copy: PathBuf, generation: u32, state: Option<&[u8]>) -> bool {
    let name = entry.plugin.name.clone();
    entry.module = module;
    entry.descriptor = descriptor;
    entry.copy = copy;
    entry.generation = generation;
    crash::note_image(&name, generation, module as usize, true);
    entry.live = (descriptor.load)(entry.api);
    if !entry.live {
        log!("{name}: generation {generation}: load returned false; {} detached", detach(&name));
        return false;
    }
    log!("{name}: generation {generation} loaded, image at {:#x}", module as usize);
    if let (Some(bytes), Some(restore)) = (state, descriptor.restore_state) {
        restore(bytes.as_ptr(), bytes.len());
        log!("{name}: generation {generation}: {} bytes of state restored", bytes.len());
    }
    true
}
