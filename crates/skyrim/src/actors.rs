//! References and actors (subsystem pass 2): the reference handle and the counted pointer its
//! lookup returns, and what the generated `TESObjectREFR`, `InventoryChanges`, `TESContainer`,
//! `TESFullName` and `ActorValueList` methods leave `unsafe`: the handle out-parameter, the
//! inventory list, the base container's entries, the full name as `&CStr`, the actor value table
//! as a slice and a lookup by name; the mount a rider sits on, the motion mode, a graph pause, the
//! display name, the biped's objects and the process lists' high actor handles; the references in
//! range of a point or of a reference, and placing and deleting a reference.
//!
//! Read and write on the game's main thread, as with forms.

use core::ffi::{CStr, c_void};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::fmt;

use sre::raw::{fns, globals};
use sre::{AIProcess, Actor, ActorValue, BipedAnim, BipedObject, ActorValueInfo, ActorValueList, ContainerEntry, InventoryChanges, InventoryEntry, MotionDriveMode, NiAVObject, NiPoint3, PlayerCharacter, TESBoundObject, TESContainer, TESForm, TESForm_formFlags, TESFullName, TESObjectCELL, TESObjectREFR};

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
    /// The name the game shows for the reference (`GetDisplayFullName`, id 19781). Main thread: it
    /// may add extra data (RE `camera.md` IC8 C).
    fn display_name(&self) -> Option<&CStr>;
}

impl TESObjectREFRExt for TESObjectREFR {
    fn handle(&self) -> RefHandle {
        let mut handle = 0;
        unsafe { self.get_handle(&raw mut handle) };
        RefHandle(handle)
    }

    fn display_name(&self) -> Option<&CStr> {
        let name = self.get_display_full_name();
        if name.is_null() { None } else { Some(unsafe { CStr::from_ptr(name) }) }
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
    /// The movement controller's motion mode (`MovementControllerNPC`+0x1c0, the value
    /// `IsMotionDriven` tests; RE `camera.md` request IC8 M); `None` without a controller or for a
    /// value outside the enum.
    fn motion_mode(&self) -> Option<MotionDriveMode>;
    /// Stops the actor's animation graph updates as the engine does (`boolFlags2` bit 8 cleared
    /// and bit 17, one forced update, set in one store; RE `camera.md` IC8 A), or lets them run
    /// again with one forced update. The engine re-enables them, so a pause is re-asserted every
    /// frame. Main thread.
    fn set_graph_paused(&self, paused: bool);
    /// The third-person biped's 42 objects (`Character`+0x268, `BipedAnim`+0x10; RE `camera.md`
    /// IC8 I): 0-31 the armor slot mask bits (0 head, 1 hair), 32-40 the right hand's weapon by
    /// type, 41 ammo. Empty without a biped. Main thread.
    fn biped_objects(&self) -> &[BipedObject];
}

/// `boolFlags2` bit 8: the animation graph updates.
const GRAPH_UPDATE: u32 = 1 << 8;
/// `boolFlags2` bit 17: one graph update is forced (cleared by `PrepareAnimationUpdate`).
const FORCE_GRAPH_UPDATE: u32 = 1 << 17;

/// `boolFlags2` with the graph paused or running.
#[must_use]
const fn graph_flags(flags: u32, paused: bool) -> u32 {
    if paused { (flags & !GRAPH_UPDATE) | FORCE_GRAPH_UPDATE } else { flags | GRAPH_UPDATE | FORCE_GRAPH_UPDATE }
}

impl ActorExt for Actor {
    fn occupied_furniture(&self) -> Option<RefPtr> {
        type GetFn = unsafe extern "C" fn(process: *const AIProcess, out: *mut u32) -> *mut u32;
        let process = self.current_process()?;
        let get: GetFn = unsafe { crate::engine_fn(sre::id!(fns::GetField8RefHandle)) };
        let mut handle = 0;
        unsafe { get(process, &raw mut handle) };
        RefHandle(handle).lookup()
    }

    fn mount(&self) -> Option<RefPtr> {
        let mut out: *mut TESObjectREFR = core::ptr::null_mut();
        unsafe { self.get_mount_ref((&raw mut out).cast()) };
        NonNull::new(out).map(RefPtr)
    }

    fn motion_mode(&self) -> Option<MotionDriveMode> {
        let controller = unsafe { self.movementController().as_ref() }?;
        MotionDriveMode::from_u32(controller.motionMode)
    }

    fn set_graph_paused(&self, paused: bool) {
        unsafe { self.boolFlags2_ptr().write_volatile(graph_flags(self.boolFlags2(), paused)) };
    }

    fn biped_objects(&self) -> &[BipedObject] {
        const COUNT: usize = 42;
        sre::fixed_size!("BipedObject", BipedObject);
        let biped = self.field_268() as *const BipedAnim;
        match unsafe { biped.as_ref() } {
            Some(b) => unsafe { core::slice::from_raw_parts((core::ptr::from_ref(b) as usize + sre::member_offset!("BipedAnim.objects", 0x10)) as *const BipedObject, COUNT) },
            None => &[],
        }
    }
}

/// A reference walk's callback: collects every reference it is handed and never stops the walk.
unsafe extern "C" fn collect(reference: *mut TESObjectREFR, data: *mut c_void) -> bool {
    unsafe { (*data.cast::<Vec<*mut TESObjectREFR>>()).push(reference) };
    false
}

/// The handles of what a walk collected, disabled and deleted references left out.
fn live_handles(found: Vec<*mut TESObjectREFR>) -> Vec<RefHandle> {
    let skip = TESForm_formFlags::DELETED.bits() | TESForm_formFlags::DISABLED.bits();
    found.into_iter().filter_map(|r| unsafe { r.as_ref() }).filter(|r| r.formFlags & skip == 0).map(TESObjectREFRExt::handle).collect()
}

/// The live references of `cell` within `radius` of `origin`, disabled and deleted ones left out,
/// through `TESObjectCELL::ForEachReferenceInRadius` (id 19068), which walks under the cell's lock
/// and follows load doors into linked cells within the radius (RE `camera.md` IC8 C). Main thread.
#[must_use]
pub fn references_in_radius(cell: &TESObjectCELL, origin: [f32; 3], radius: f32) -> Vec<RefHandle> {
    let mut found: Vec<*mut TESObjectREFR> = Vec::new();
    let [x, y, z] = origin;
    let origin = crate::point(x, y, z);
    unsafe { cell.for_each_reference_in_radius(&raw const origin, radius, &raw const origin, radius, collect as *const c_void, (&raw mut found).cast()) };
    live_handles(found)
}

/// The live references within `radius` of `center`, disabled and deleted ones left out, through
/// `TES::ForEachReferenceInRadiusOfRef` (id 13701): `center`'s cell when that is an interior, else
/// the loaded cells of its worldspace (RE `camera.md` IC8 C). Its first argument is never read
/// and is passed null (IC9 H). Main thread.
#[must_use]
pub fn references_around(center: &TESObjectREFR, radius: f32) -> Vec<RefHandle> {
    let mut found: Vec<*mut TESObjectREFR> = Vec::new();
    let origin = crate::point(center.posX, center.posY, center.posZ);
    unsafe { sre::TES::for_each_reference_in_radius_of_ref(core::ptr::null_mut(), core::ptr::from_ref(center).cast_mut(), radius, &raw const origin, radius, collect as *const c_void, (&raw mut found).cast()) };
    live_handles(found)
}

/// Places a new reference of `base` at `at`'s position and angles, in its cell and worldspace, as
/// Papyrus `PlaceAtMe` places a plain base form at 0xa46f0c: `TESDataHandler::CreateReferenceAtLocation`
/// (id 13723) with no existing reference, primitive or linked reference, `persist` as the
/// force-persist flag and the last flag set (RE `camera.md` IC9 H; the call has no count, the
/// native calls it once per copy). `None` when the engine placed nothing. Main thread.
#[must_use]
pub fn place_at_me(at: &TESObjectREFR, base: &TESBoundObject, persist: bool) -> Option<RefHandle> {
    let holder = crate::reloc().address(sre::id!(globals::g_TESDataHandler_ptr)).ok()? as *const *mut c_void;
    let data = unsafe { *holder };
    if data.is_null() {
        return None;
    }
    let position = (&raw const at.posX).cast::<NiPoint3>();
    let angles = (&raw const at.rotX).cast::<NiPoint3>();
    let worldspace = at.get_worldspace().map_or(core::ptr::null_mut(), |w| core::ptr::from_ref(w).cast_mut());
    let mut handle = RefHandle::INVALID.0;
    let base = core::ptr::from_ref(base).cast_mut();
    unsafe { sre::TESDataHandler::create_reference_at_location(data, (&raw mut handle).cast(), base, position, angles, at.parentCell, worldspace, core::ptr::null_mut(), core::ptr::null_mut(), 0, persist, true) };
    Some(RefHandle(handle)).filter(|h| *h != RefHandle::INVALID)
}

/// Removes a reference as Papyrus `Disable` and `Delete` end: `Disable` (slot 137, which unloads
/// its 3D and sets the disabled bit), then `SetDelete(true)` (slot 35; RE `camera.md` IC8 D).
/// Main thread.
pub fn delete_reference(reference: &TESObjectREFR) {
    type DisableFn = unsafe extern "C" fn(*const TESObjectREFR);
    type SetDeleteFn = unsafe extern "C" fn(*const TESForm, bool);
    let disable: DisableFn = unsafe { core::mem::transmute(sre::vtable_entry(reference, sre::slot_index!("TESObjectREFR", 0x0, 137))) };
    let set_delete: SetDeleteFn = unsafe { core::mem::transmute(sre::vtable_entry(reference, sre::slot_index!("TESObjectREFR", 0x0, 35))) };
    unsafe {
        disable(reference);
        set_delete(core::ptr::from_ref(reference).cast(), true);
    }
}

/// The handles of an engine array of `u32` handles laid out as `{ data, capacity, size }` at `array`.
///
/// # Safety
/// `array` must point to such an array whose data stays readable while this runs.
unsafe fn handles_at(array: *const sre::BSTArray) -> Vec<RefHandle> {
    let array = unsafe { &*array };
    if array.data.is_null() {
        return Vec::new();
    }
    unsafe { core::slice::from_raw_parts(array.data.cast::<u32>(), array.size as usize) }.iter().map(|&h| RefHandle(h)).collect()
}

/// The actors in high process, as the handles of the process lists' high array (`ProcessLists`
/// +0x30), copied under the lists' lock as the engine copies them (RE `camera.md` request IC7 D).
/// The player is never among them. Main thread.
#[must_use]
pub fn high_actor_handles() -> Vec<RefHandle> {
    let reloc = crate::reloc();
    let (Ok(holder), Ok(lock)) = (reloc.address(sre::id!(globals::g_ProcessLists_ptr)), reloc.address(sre::id!(globals::g_ProcessListsHandleLock))) else { return Vec::new() };
    let lists = unsafe { *(holder as *const *const u8) };
    if lists.is_null() {
        return Vec::new();
    }
    let _guard = crate::RwGuard::read(unsafe { &*(lock as *const sre::BSReadWriteLock) });
    unsafe { handles_at(lists.add(sre::member_offset!("ProcessLists.highActorHandles", 0x30)).cast()) }
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
    use super::{RefHandle, graph_flags, handles_at};

    #[test]
    fn a_pause_clears_the_update_bit_and_forces_one_update() {
        assert_eq!(graph_flags(0x3820_21a3, true), (0x3820_21a3 & !0x100) | 0x2_0000);
        assert_eq!(graph_flags(0x2, false), 0x2_0102);
        assert_eq!(graph_flags(graph_flags(0x1_0101, true), false), 0x3_0101);
    }

    #[test]
    fn the_high_array_is_read_from_its_data_and_size() {
        let handles = [0x0010_0001u32, 0x0020_0002, 0x0030_0003];
        let mut array = [0usize; 3];
        array[0] = handles.as_ptr() as usize;
        array[1] = 8;
        array[2] = 2;
        let read = unsafe { handles_at(array.as_ptr().cast()) };
        assert_eq!(read, [RefHandle(0x0010_0001), RefHandle(0x0020_0002)]);
        assert_eq!(unsafe { handles_at([0usize; 3].as_ptr().cast()) }, []);
    }
}
