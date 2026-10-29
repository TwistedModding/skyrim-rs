//! References and actors (subsystem pass 2): the reference handle and the counted pointer its
//! lookup returns, and what the generated `TESObjectREFR`, `InventoryChanges`, `TESContainer`,
//! `TESFullName` and `ActorValueList` methods leave `unsafe`: the handle out-parameter, the
//! inventory list, the base container's entries, the full name as `&CStr`, the actor value table
//! as a slice and a lookup by name.
//!
//! Read and write on the game's main thread, as with forms.

use core::ffi::CStr;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::fmt;

use sre::{ActorValue, ActorValueInfo, ActorValueList, ContainerEntry, InventoryChanges, InventoryEntry, NiAVObject, PlayerCharacter, TESContainer, TESFullName, TESObjectREFR};

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
