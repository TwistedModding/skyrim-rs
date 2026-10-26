//! Hot reload: every plugin runs from a shadow copy under `skyrim-rs\.loaded\`, the author's file
//! is polled once a second from the frame, and a changed file that can be opened exclusively (the
//! copy has finished) is swapped in at that frame boundary on the main thread.
//!
//! The swap: copy the file as the next generation and read its descriptor (a refusal keeps the
//! running generation); ask the running generation for its state and call its `unload`; detach
//! every registration of the plugin, which drains the calls inside them; unload the DLL; call the
//! new generation's `load` through the plugin's own `Host`, so it registers again and the host
//! re-points what the engine keeps; hand it the state. A `load` that returns false is detached and
//! unloaded, and the previous generation's copy is loaded again with the same state. A panic in
//! `load` aborts the game (release builds abort), so only failures before `load` are recoverable.

use std::fs;
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};

use super::{detach, flush_settings, plugin_name, read_descriptor};
use crate::{Descriptor, Host, Plugin};

const SHADOW_DIR: &str = ".loaded";
const POLL: Duration = Duration::from_secs(1);

type Stamp = (u64, SystemTime);

/// One plugin as the host runs it: the generation loaded now and the file it came from.
pub(super) struct Loaded {
    plugin: Plugin,
    host: &'static Host,
    module: HMODULE,
    descriptor: &'static Descriptor,
    /// Whether the generation's `load` returned true.
    live: bool,
    source: PathBuf,
    copy: PathBuf,
    generation: u32,
    stamp: Option<Stamp>,
}

unsafe impl Send for Loaded {}

impl Loaded {
    pub(super) fn new(plugin: Plugin, host: &'static Host, module: HMODULE, descriptor: &'static Descriptor, live: bool, source: PathBuf, copy: PathBuf) -> Self {
        let stamp = stamp(&source);
        Self { plugin, host, module, descriptor, live, source, copy, generation: 1, stamp }
    }
}

static LOADED: Mutex<Vec<Loaded>> = Mutex::new(Vec::new());

/// Each watched plugin's name, running generation and whether its `load` returned true.
pub(super) fn generations() -> Vec<(String, u32, bool)> {
    LOADED.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|l| (l.plugin.name.clone(), l.generation, l.live)).collect()
}

/// [`generations`] without waiting: `None` while another thread holds the list (the crash log's caller).
pub(super) fn try_generations() -> Option<Vec<(String, u32, bool)>> {
    LOADED.try_lock().ok().map(|loaded| loaded.iter().map(|l| (l.plugin.name.clone(), l.generation, l.live)).collect())
}
static LAST_POLL: Mutex<Option<Instant>> = Mutex::new(None);

/// Empties the shadow folder under `dir` and returns it.
pub(super) fn clean(dir: &Path) -> PathBuf {
    let shadow = dir.join(SHADOW_DIR);
    let _ = fs::remove_dir_all(&shadow);
    let _ = fs::create_dir_all(&shadow);
    shadow
}

/// Copies `source` into `shadow` as `<stem>.<generation>.dll`.
pub(super) fn copy(source: &Path, shadow: &Path, generation: u32) -> io::Result<PathBuf> {
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let copy = shadow.join(format!("{stem}.{generation}.dll"));
    fs::copy(source, &copy)?;
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

/// The frame callback: once a second, swaps in every plugin whose file changed and settled.
pub(super) fn tick() {
    {
        let mut last = LAST_POLL.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        *last = Some(Instant::now());
    }
    flush_settings(None);
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    for entry in loaded.iter_mut() {
        let now = stamp(&entry.source);
        if now.is_none() || now == entry.stamp || !settled(&entry.source) {
            continue;
        }
        entry.stamp = now;
        swap(entry);
    }
}

fn swap(entry: &mut Loaded) {
    let name = entry.plugin.name.clone();
    let generation = entry.generation + 1;
    log!("{name}: {} changed, reloading as generation {generation}", entry.plugin.file);
    let shadow = entry.copy.parent().map(Path::to_path_buf).unwrap_or_default();
    let copy = match copy(&entry.source, &shadow, generation) {
        Ok(copy) => copy,
        Err(e) => {
            log!("{name}: generation {generation} not copied: {e}; generation {} stays", entry.generation);
            return;
        }
    };
    let (module, descriptor) = match read_descriptor(&copy).and_then(|(module, descriptor)| {
        let declared = plugin_name(descriptor);
        if declared == name {
            Ok((module, descriptor))
        } else {
            unsafe { FreeLibrary(module) };
            Err(format!("its descriptor names {declared}"))
        }
    }) {
        Ok(loaded) => loaded,
        Err(e) => {
            log!("{name}: generation {generation} refused: {e}; generation {} stays", entry.generation);
            let _ = fs::remove_file(&copy);
            return;
        }
    };
    let state = if entry.live { entry.descriptor.save_state.map(|f| f()) } else { None };
    if let Some(bytes) = &state {
        log!("{name}: generation {}: {} bytes of state saved", entry.generation, bytes.len());
    }
    if entry.live
        && let Some(unload) = entry.descriptor.unload
    {
        unload();
    }
    log!("{name}: generation {}: {} detached", entry.generation, detach(&name));
    flush_settings(Some(&name));
    unsafe { FreeLibrary(entry.module) };
    crash::note_image(&name, entry.generation, entry.module as usize, false);
    log!("{name}: generation {} unloaded", entry.generation);
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
    entry.live = (descriptor.load)(entry.host);
    if !entry.live {
        log!("{name}: generation {generation}: load returned false; {} detached", detach(&name));
        return false;
    }
    log!("{name}: generation {generation} loaded, image at {:#x}", module as usize);
    if let (Some(bytes), Some(restore)) = (state, descriptor.restore_state) {
        restore(bytes);
        log!("{name}: generation {generation}: {} bytes of state restored", bytes.len());
    }
    true
}
