//! Save and load (subsystem pass 10): the save/load manager's event and current save name, the
//! saved plugin lists, and the remapping of form ids and script handles a save recorded.
//!
//! Where a mod keeps its own state across saves: write it when [`SaveLoadEvent`] reports a
//! finished save (`{save, done}`, the file renamed into place, the name in
//! [`SaveLoadManager::current_save_name`]); drop it when a load starts (`{load, not done}`); read it
//! back at `TESLoadGameEvent`, mapping the form ids it recorded through [`remap_form_id`]. The
//! manager updates the name only at the end of a load, after the script extender's post-load
//! message, so read it there, not earlier.

use core::ffi::{CStr, c_char};

use layout::v1_7_104::{classes, fns, globals};

pub use layout::v1_7_104::enums::SaveLoadEventKind;

use crate::events::{BSTEventSource, Event};
use crate::forms::{FormId, TESFile};

type RemapFn = unsafe extern "C" fn(game: *mut classes::BGSSaveLoadGame, saved: u32) -> u32;

/// The `char*` to the current save name sits at manager +0x240, inside the name's inline storage.
const SAVE_NAME_PTR: usize = 0x240 - 0x138;

pub(crate) struct Fns {
    manager: *const *mut classes::BGSSaveLoadManager,
    game: *const *mut classes::BGSSaveLoadGame,
    remap: RemapFn,
}

unsafe impl Sync for Fns {}
unsafe impl Send for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        reloc.address(SaveLoadEvent::ADD_SINK)?;
        reloc.address(SaveLoadEvent::REMOVE_SINK)?;
        unsafe {
            Ok(Self {
                manager: reloc.address(globals::g_BGSSaveLoadManager_ptr)? as *const *mut classes::BGSSaveLoadManager,
                game: reloc.address(globals::g_BGSSaveLoadGame_ptr)? as *const *mut classes::BGSSaveLoadGame,
                remap: crate::resolve(reloc, fns::BGSSaveLoadGame__RemapSavedFormID)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().save
}

/// The save/load manager (`BGSSaveLoadManager`, 0x418): the save list, the current save name and
/// the event source a save or load reports through.
#[repr(transparent)]
pub struct SaveLoadManager(classes::BGSSaveLoadManager);

impl SaveLoadManager {
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().manager).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::BGSSaveLoadManager {
        &self.0
    }

    /// The save last written or loaded, without the `.ess` a loaded name carries. While a save is
    /// being written it is still the previous name until the save's done event, and while a save
    /// is being loaded until `TESLoadGameEvent`.
    #[must_use]
    pub fn current_save_name(&self) -> Option<&str> {
        let bytes: [u8; 8] = self.0.currentSaveName[SAVE_NAME_PTR..SAVE_NAME_PTR + 8].try_into().expect("8 bytes");
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
        let manager = SaveLoadManager::get()?;
        unsafe { BSTEventSource::from_ptr((&raw const manager.0.saveLoadEventSource).cast()) }
    }
}

/// The save format's state (`BGSSaveLoadGame`, 0x348).
#[repr(transparent)]
pub struct SaveLoadGame(classes::BGSSaveLoadGame);

impl SaveLoadGame {
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().game).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::BGSSaveLoadGame {
        &self.0
    }

    /// The regular plugins of the save last read, by their saved index; `None` for one this
    /// session does not have.
    #[must_use]
    pub fn saved_files(&self) -> &[Option<&TESFile>] {
        files(&self.0.savedFiles)
    }

    /// The light plugins of the save last read, by their saved light index.
    #[must_use]
    pub fn saved_light_files(&self) -> &[Option<&TESFile>] {
        files(&self.0.savedLightFiles)
    }
}

fn files(array: &classes::BSTArray) -> &[Option<&TESFile>] {
    if array.data.is_null() {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(array.data.cast::<Option<&TESFile>>(), array.size as usize) }
}

/// The session's id of a form id a save recorded (`BGSSaveLoadGame::RemapSavedFormID`), by the
/// plugin list of the save last read; `None` when its plugin is gone. Created forms (0xFF) pass
/// through unchanged. Valid from a load until the next save's plugin list is read.
#[must_use]
pub fn remap_form_id(saved: FormId) -> Option<FormId> {
    if saved.is_runtime() {
        return Some(saved);
    }
    let game = unsafe { *fns().game };
    if game.is_null() {
        return None;
    }
    let current = unsafe { (fns().remap)(game, saved.0) };
    (current != 0).then_some(FormId(current))
}

/// A script handle a save recorded: its high 32 bits (the handle's type) are kept and its form
/// id is remapped as [`remap_form_id`] does.
#[must_use]
pub fn remap_handle(saved: u64) -> Option<u64> {
    let form = remap_form_id(FormId((saved & 0xFFFF_FFFF) as u32))?;
    Some(saved & !0xFFFF_FFFF | u64::from(form.0))
}
