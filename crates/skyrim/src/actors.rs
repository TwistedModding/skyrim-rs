//! References and actors (subsystem pass 2): the reference handle and the counted pointer its
//! lookup returns, and what the generated `TESObjectREFR`, `InventoryChanges`, `TESContainer`,
//! `TESFullName` and `ActorValueList` methods leave `unsafe`: the handle out-parameter, the
//! inventory list, the base container's entries, the full name as `&CStr`, the actor value table
//! as a slice and a lookup by name; the mount a rider sits on and the process lists' high actor
//! handles.
//!
//! Read and write on the game's main thread, as with forms.

use core::ffi::CStr;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::fmt;

use sre::raw::{fns, globals};
use sre::{AIProcess, Actor, ActorValue, ActorValueInfo, ActorValueList, ContainerEntry, InventoryChanges, InventoryEntry, NiAVObject, PlayerCharacter, TESContainer, TESFullName, TESObjectREFR};

use crate::ptr_slice;

/// The modifier slots of an actor value, the `modifier` of `ActorValueOwner::ModActorValue` and
/// `Actor::GetActorValueModifier`.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modifier {
    Permanent = 0,
    Temporary = 1,
    Damage = 2,
}

/// A reference handle: bits 0-19 the handle table index, 20-25 the entry's age; `0` is the null handle.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RefHandle(pub u32);

impl RefHandle {
    /// `g_InvalidRefHandle` (id 400312) holds 0.
    pub const INVALID: Self = Self(0);

    /// The live reference through `ResolveRefHandle` (id 12331): `None` when the handle is null,
    /// stale or its table entry inactive. The lookup adds one reference, which the [`RefPtr`]
    /// releases on drop.
    #[must_use]
    pub fn lookup(self) -> Option<RefPtr> {
        let mut handle = self.0;
        let mut out: *mut TESObjectREFR = core::ptr::null_mut();
        if !unsafe { sre::resolve_ref_handle(&raw mut handle, (&raw mut out).cast()) } {
            return None;
        }
        NonNull::new(out).map(RefPtr)
    }
}

impl fmt::Display for RefHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08X}", self.0)
    }
}

/// A reference held alive by one count on its `refCount` word, taken by [`RefHandle::lookup`].
///
/// Dropping it subtracts that count atomically, which is what releasing an `NiPointer` does
/// while other references remain; the count cannot reach zero here because the handle table's own
/// `NiPointer` held the object before the lookup, so the engine's free path is never needed.
pub struct RefPtr(NonNull<TESObjectREFR>);

impl core::ops::Deref for RefPtr {
    type Target = TESObjectREFR;

    fn deref(&self) -> &TESObjectREFR {
        unsafe { self.0.as_ref() }
    }
}

impl Drop for RefPtr {
    fn drop(&mut self) {
        let word = unsafe { AtomicU32::from_ptr(&raw mut (*self.0.as_ptr()).refCount) };
        word.fetch_sub(1, Ordering::AcqRel);
    }
}

pub trait TESObjectREFRExt {
    /// The reference's handle through `TESObjectREFR::GetHandle` (id 19846), created on first use;
    /// [`RefHandle::INVALID`] for an object nothing references.
    fn handle(&self) -> RefHandle;
}

impl TESObjectREFRExt for TESObjectREFR {
    fn handle(&self) -> RefHandle {
        let mut handle = 0;
        unsafe { self.get_handle(&raw mut handle) };
        RefHandle(handle)
    }
}

pub trait ActorExt {
    /// The furniture reference the actor's process is using, the one whose base object
    /// `Actor::GetFurnitureAnimType` classifies: the handle `GetField8RefHandle` (id 39916) reads
    /// from `currentProcess` (RE `camera.md` request IC2 E). `None` without a process or furniture.
    fn occupied_furniture(&self) -> Option<RefPtr>;
    /// The mount the actor rides through `Actor::GetMountRef` (id 38702), which fills a counted
    /// pointer; `None` when the actor rides nothing.
    fn mount(&self) -> Option<RefPtr>;
}

impl ActorExt for Actor {
    fn occupied_furniture(&self) -> Option<RefPtr> {
        type GetFn = unsafe extern "C" fn(process: *const AIProcess, out: *mut u32) -> *mut u32;
        let process = self.current_process()?;
        let get: GetFn = unsafe { crate::engine_fn(fns::GetField8RefHandle) };
        let mut handle = 0;
        unsafe { get(process, &raw mut handle) };
        RefHandle(handle).lookup()
    }

    fn mount(&self) -> Option<RefPtr> {
        let mut out: *mut TESObjectREFR = core::ptr::null_mut();
        unsafe { self.get_mount_ref((&raw mut out).cast()) };
        NonNull::new(out).map(RefPtr)
    }
}

/// Offset of the high actor handle array in `ProcessLists` (data +0, capacity +8, size +0x10).
const HIGH_ACTOR_HANDLES: usize = 0x30;

/// The handles of an engine array of `u32` handles laid out as `{ data, capacity, size }` at `array`.
///
/// # Safety
/// `array` must point to such an array whose data stays readable while this runs.
unsafe fn handles_at(array: *const u8) -> Vec<RefHandle> {
    let data = unsafe { array.cast::<*const u32>().read_unaligned() };
    let size = unsafe { array.add(0x10).cast::<u32>().read_unaligned() };
    if data.is_null() {
        return Vec::new();
    }
    unsafe { core::slice::from_raw_parts(data, size as usize) }.iter().map(|&h| RefHandle(h)).collect()
}

/// The actors in high process, as the handles of the process lists' high array (`ProcessLists`
/// +0x30), copied under the lists' lock as the engine copies them (RE `camera.md` request IC7 D).
/// The player is never among them. Main thread.
#[must_use]
pub fn high_actor_handles() -> Vec<RefHandle> {
    let reloc = crate::reloc();
    let (Ok(holder), Ok(lock)) = (reloc.address(globals::g_ProcessLists_ptr), reloc.address(globals::g_ProcessListsHandleLock)) else { return Vec::new() };
    let lists = unsafe { *(holder as *const *const u8) };
    if lists.is_null() {
        return Vec::new();
    }
    let _guard = crate::RwGuard::read(unsafe { &*(lock as *const sre::BSReadWriteLock) });
    unsafe { handles_at(lists.add(HIGH_ACTOR_HANDLES)) }
}

pub trait PlayerCharacterExt {
    /// The third-person root, `loadedData->node3D`, whichever view is current; the first-person
    /// root is the generated `first_person_root()`. `None` while the player has no 3D. A load
    /// replaces both, so a pointer kept across frames is cleared at `PreLoadGame`.
    fn third_person_root(&self) -> Option<&NiAVObject>;
}

impl PlayerCharacterExt for PlayerCharacter {
    fn third_person_root(&self) -> Option<&NiAVObject> {
        self.loaded_data()?.node3_d().map(|node| &**node)
    }
}

pub trait InventoryChangesExt {
    /// The entries of the inventory list; null list items are skipped.
    fn items(&self) -> impl Iterator<Item = &InventoryEntry>;
}

impl InventoryChangesExt for InventoryChanges {
    fn items(&self) -> impl Iterator<Item = &InventoryEntry> {
        let mut node = unsafe { self.entries.as_ref() };
        core::iter::from_fn(move || {
            let n = node?;
            node = unsafe { n.next.cast::<sre::BSSimpleListNode>().as_ref() };
            Some(unsafe { n.item.cast::<InventoryEntry>().as_ref() })
        })
        .flatten()
    }
}

pub trait TESContainerExt {
    /// The base container's entries, null ones included.
    fn entries(&self) -> &[Option<&ContainerEntry>];
}

impl TESContainerExt for TESContainer {
    fn entries(&self) -> &[Option<&ContainerEntry>] {
        unsafe { ptr_slice(self.entries, self.numEntries) }
    }
}

pub trait TESFullNameExt {
    /// The name through vtable slot 5 (`GetFullName`), `None` when it is empty.
    fn full_name(&self) -> Option<&CStr>;
}

impl TESFullNameExt for TESFullName {
    fn full_name(&self) -> Option<&CStr> {
        let text = self.get_full_name();
        if text.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(text) }).filter(|s| !s.is_empty())
    }
}

pub trait ActorValueListExt {
    /// The 164 `ActorValueInfo` pointers indexed by [`ActorValue`]; they lie inline from +8, where the
    /// ledger records the array as the bytes of `infos`.
    fn infos(&self) -> &[Option<&ActorValueInfo>];
}

impl ActorValueListExt for ActorValueList {
    fn infos(&self) -> &[Option<&ActorValueInfo>] {
        unsafe { ptr_slice(self.infos.as_ptr().cast(), ActorValue::ReflectDamage as u32 + 1) }
    }
}

/// An actor value by editor name through `ActorValueList::LookupActorValueByName` (id 27203),
/// case-insensitive; `None` for an unknown name.
#[must_use]
pub fn lookup_actor_value(name: &str) -> Option<ActorValue> {
    let name = CString::new(name).ok()?;
    let id = unsafe { ActorValueList::lookup_actor_value_by_name(name.as_ptr()) };
    id.ok()
}

#[cfg(test)]
mod tests {
    use super::{HIGH_ACTOR_HANDLES, RefHandle, handles_at};

    #[test]
    fn the_high_array_is_read_from_its_data_and_size() {
        let handles = [0x0010_0001u32, 0x0020_0002, 0x0030_0003];
        let mut lists = [0u8; 0x80];
        let array = &mut lists[HIGH_ACTOR_HANDLES..];
        array[..8].copy_from_slice(&(handles.as_ptr() as usize).to_le_bytes());
        array[8..12].copy_from_slice(&8u32.to_le_bytes());
        array[0x10..0x14].copy_from_slice(&2u32.to_le_bytes());
        let read = unsafe { handles_at(lists.as_ptr().add(HIGH_ACTOR_HANDLES)) };
        assert_eq!(read, [RefHandle(0x0010_0001), RefHandle(0x0020_0002)]);
        assert_eq!(unsafe { handles_at([0u8; 0x18].as_ptr()) }, []);
    }
}
