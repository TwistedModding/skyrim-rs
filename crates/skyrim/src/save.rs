//! Save and load (subsystem pass 10): the save/load manager's event, saves and loads by name, the
//! current save name, the saved plugin lists, and the remapping of form ids and script handles a
//! save recorded.
//!
//! Where a mod keeps its own state across saves: write it when [`SaveLoadEvent`] reports a
//! finished save (`{save, done}`, the file renamed into place, the name in
//! [`BGSSaveLoadManagerExt::current_save_name`]); drop it when a load starts (`{load, not done}`); read it
//! back at `TESLoadGameEvent`, mapping the form ids it recorded through [`remap_form_id`]. The
//! manager updates the name only at the end of a load, after the script extender's post-load
//! message, so read it there, not earlier.

use core::ffi::{CStr, c_char};
use std::ffi::CString;

use sre::raw::fns;
use sre::{BGSSaveLoadGame, BGSSaveLoadManager, SaveLoadEventKind, TESFile};

use crate::events::{BSTEventSource, Event};
use crate::ptr_slice;

type SaveFn = unsafe extern "C" fn(manager: *mut BGSSaveLoadManager, kind: i32, value: u32, name: *const c_char) -> bool;

/// `BGSSaveLoadManager::Save`'s kind for a manual save.
const MANUAL_SAVE: i32 = 2;

/// The `char*` to the current save name sits at manager +0x240, inside the name's inline storage.
const SAVE_NAME_PTR: usize = 0x240 - 0x138;

pub trait BGSSaveLoadManagerExt {
    /// A manual save named `name` through `BGSSaveLoadManager::Save` (id 35727), written whole on
    /// this thread. Call it on the main thread right after the main loop drains the manager's
    /// requests (`ProcessSaveLoadRequests`), where the script extender's own save runs: a save made
    /// while the main loop waits on the VM deadlocks.
    fn save_as(&self, name: &str) -> bool;
    /// Reads the save `name` through `BGSSaveLoadManager::RequestLoad` (id 35728) and queues its load
    /// for the next main update; the call the script extender's load makes.
    fn load_save(&self, name: &str) -> bool;
    /// The save last written or loaded, without the `.ess` a loaded name carries. While a save is
    /// being written it is still the previous name until the save's done event, and while a save
    /// is being loaded until `TESLoadGameEvent`.
    fn current_save_name(&self) -> Option<&str>;
}

impl BGSSaveLoadManagerExt for BGSSaveLoadManager {
    fn save_as(&self, name: &str) -> bool {
        let Ok(name) = CString::new(name) else { return false };
        let save: SaveFn = unsafe { crate::engine_fn(fns::BGSSaveLoadManager__Save) };
        unsafe { save(core::ptr::from_ref(self).cast_mut(), MANUAL_SAVE, 0, name.as_ptr()) }
    }

    fn load_save(&self, name: &str) -> bool {
        let Ok(name) = CString::new(name) else { return false };
        unsafe { self.request_load(name.as_ptr(), u32::MAX, 0, true) }
    }

    fn current_save_name(&self) -> Option<&str> {
        let bytes: [u8; 8] = self.currentSaveName[SAVE_NAME_PTR..SAVE_NAME_PTR + 8].try_into().expect("8 bytes");
        let name = usize::from_le_bytes(bytes) as *const c_char;
        if name.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(name) }.to_str().ok()?;
        Some(name.strip_suffix(".ess").unwrap_or(name)).filter(|n| !n.is_empty())
    }
}

/// What the manager reports: a save (kind 1) or a load (kind 2) starting (`done` false) and
/// finishing (`done` true). A save's done comes after the file is renamed into place; a load's
/// after the file is read, before the game applies it on the main thread. A load from the main
/// menu also sends a kind 0 pair after its own.
#[repr(C)]
pub struct SaveLoadEvent {
    kind: u8,
    done: u8,
}

impl SaveLoadEvent {
    #[must_use]
    pub fn kind(&self) -> Option<SaveLoadEventKind> {
        SaveLoadEventKind::from_u32(u32::from(self.kind))
    }

    #[must_use]
    pub fn kind_raw(&self) -> u8 {
        self.kind
    }

    #[must_use]
    pub fn done(&self) -> bool {
        self.done != 0
    }
}

impl core::fmt::Debug for SaveLoadEvent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SaveLoadEvent {{ kind: {}, done: {} }}", self.kind, self.done)
    }
}

impl Event for SaveLoadEvent {
    const ADD_SINK: u64 = fns::BSTEventSource_BGSSaveLoadManagerEvent__AddEventSink;
    const REMOVE_SINK: u64 = fns::BSTEventSource_BGSSaveLoadManagerEvent__RemoveEventSink;

    fn source() -> Option<&'static BSTEventSource<Self>> {
        let manager = BGSSaveLoadManager::singleton()?;
        unsafe { BSTEventSource::from_ptr((&raw const manager.saveLoadEventSource).cast()) }
    }
}

pub trait BGSSaveLoadGameExt {
    /// The regular plugins of the save last read, by their saved index; `None` for one this
    /// session does not have.
    fn saved_files(&self) -> &[Option<&TESFile>];
    /// The light plugins of the save last read, by their saved light index.
    fn saved_light_files(&self) -> &[Option<&TESFile>];
}

impl BGSSaveLoadGameExt for BGSSaveLoadGame {
    fn saved_files(&self) -> &[Option<&TESFile>] {
        unsafe { ptr_slice(self.savedFiles.data, self.savedFiles.size) }
    }

    fn saved_light_files(&self) -> &[Option<&TESFile>] {
        unsafe { ptr_slice(self.savedLightFiles.data, self.savedLightFiles.size) }
    }
}

/// The session's id of a form id a save recorded (`BGSSaveLoadGame::RemapSavedFormID`), by the
/// plugin list of the save last read; `None` when its plugin is gone. Created forms (0xFF) pass
/// through unchanged. Valid from a load until the next save's plugin list is read.
#[must_use]
pub fn remap_form_id(saved: u32) -> Option<u32> {
    if saved >> 24 == 0xFF {
        return Some(saved);
    }
    let current = unsafe { BGSSaveLoadGame::singleton()?.remap_saved_form_id(saved) };
    (current != 0).then_some(current)
}

/// A script handle a save recorded: its high 32 bits (the handle's type) are kept and its form
/// id is remapped as [`remap_form_id`] does.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn remap_handle(saved: u64) -> Option<u64> {
    let form = remap_form_id(saved as u32)?;
    Some(saved & !0xFFFF_FFFF | u64::from(form))
}
