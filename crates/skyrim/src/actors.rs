//! References and actors (subsystem pass 2): `TESObjectREFR` with its transform, cell, 3D, extra
//! data and handle, `Actor` with its process, actor values, state and inventory, `Character`,
//! `PlayerCharacter`, the `TESNPC` actor base and the actor value table.
//!
//! The same contract as `forms`: every wrapper is `#[repr(transparent)]` over its layout struct,
//! fields are read on the game's main thread without locking, and engine functions are called with
//! the signatures `export/semantic/actors.json` rates high. Only what pass 2 proved at runtime tier
//! has an accessor; the rest stays reachable through `raw()`. Items rated medium say so.

use core::ffi::{CStr, c_char};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::fmt;

use layout::v1_7_104::{classes, enums, fns, globals, vtables};

use crate::forms::{BGSKeyword, BGSLocation, FormFlags, FormType, TESClass, TESForm, TESNPC, TESObjectCELL, TESRace, TESWorldSpace, ptr_slice};

pub use enums::{ActorValue, Actor_boolFlags as ActorFlags, ActorState_flags1 as StateFlags, TESActorBaseData_flags as ActorBaseFlags};

/// `LookupREFRByHandle` (id 12331): `bool (uint* handle, NiPointer<TESObjectREFR>* out)`; the
/// `NiPointer` is one pointer slot that receives a referenced object or null.
type LookupRefrByHandle = unsafe extern "C" fn(handle: *mut u32, out: *mut *mut classes::TESObjectREFR) -> bool;

pub(crate) struct Fns {
    get_handle: fns::TESObjectREFR__GetHandle_fn,
    lookup_by_handle: LookupRefrByHandle,
    get_scale: fns::TESObjectREFR__GetScale_fn,
    get_inventory_changes: fns::TESObjectREFR__GetInventoryChanges_fn,
    get_container: fns::TESObjectREFR__GetContainer_fn,
    get_current_location: fns::TESObjectREFR__GetCurrentLocation_fn,
    get_worldspace: fns::TESObjectREFR__GetWorldspace_fn,
    extra_has_type: fns::BaseExtraList__HasType_fn,
    extra_get_by_type: fns::BaseExtraList__GetByType_fn,
    item_count: fns::InventoryChanges__GetItemCount_fn,
    container_count: fns::TESContainer__CountObjectsInContainer_fn,
    equipped_object: fns::Actor__GetEquippedObject_fn,
    equipped_entry: fns::AIProcess__GetEquippedEntry_fn,
    actor_level: fns::Actor__GetLevel_fn,
    is_running: fns::Actor__IsRunning_fn,
    is_on_mount: fns::Actor__IsOnMount_fn,
    has_flags: fns::ActorState__HasFlags_fn,
    npc_sex: fns::TESNPC__GetSex_fn,
    npc_height: fns::TESNPC__GetHeight_fn,
    base_level: fns::TESActorBaseData__GetLevel_fn,
    av_by_name: fns::ActorValueList__LookupActorValueByName_fn,
    av_info: fns::ActorValueList__GetActorValueInfo_fn,
    player: *const *const classes::PlayerCharacter,
    av_list: *const *const classes::ActorValueList,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                get_handle: crate::resolve(reloc, fns::TESObjectREFR__GetHandle)?,
                lookup_by_handle: crate::resolve(reloc, fns::LookupREFRByHandle)?,
                get_scale: crate::resolve(reloc, fns::TESObjectREFR__GetScale)?,
                get_inventory_changes: crate::resolve(reloc, fns::TESObjectREFR__GetInventoryChanges)?,
                get_container: crate::resolve(reloc, fns::TESObjectREFR__GetContainer)?,
                get_current_location: crate::resolve(reloc, fns::TESObjectREFR__GetCurrentLocation)?,
                get_worldspace: crate::resolve(reloc, fns::TESObjectREFR__GetWorldspace)?,
                extra_has_type: crate::resolve(reloc, fns::BaseExtraList__HasType)?,
                extra_get_by_type: crate::resolve(reloc, fns::BaseExtraList__GetByType)?,
                item_count: crate::resolve(reloc, fns::InventoryChanges__GetItemCount)?,
                container_count: crate::resolve(reloc, fns::TESContainer__CountObjectsInContainer)?,
                equipped_object: crate::resolve(reloc, fns::Actor__GetEquippedObject)?,
                equipped_entry: crate::resolve(reloc, fns::AIProcess__GetEquippedEntry)?,
                actor_level: crate::resolve(reloc, fns::Actor__GetLevel)?,
                is_running: crate::resolve(reloc, fns::Actor__IsRunning)?,
                is_on_mount: crate::resolve(reloc, fns::Actor__IsOnMount)?,
                has_flags: crate::resolve(reloc, fns::ActorState__HasFlags)?,
                npc_sex: crate::resolve(reloc, fns::TESNPC__GetSex)?,
                npc_height: crate::resolve(reloc, fns::TESNPC__GetHeight)?,
                base_level: crate::resolve(reloc, fns::TESActorBaseData__GetLevel)?,
                av_by_name: crate::resolve(reloc, fns::ActorValueList__LookupActorValueByName)?,
                av_info: crate::resolve(reloc, fns::ActorValueList__GetActorValueInfo)?,
                player: reloc.address(globals::g_PlayerCharacter_ptr)? as *const *const classes::PlayerCharacter,
                av_list: reloc.address(globals::g_ActorValueList_ptr)? as *const *const classes::ActorValueList,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().actors
}

/// A C string the engine owns, `None` when the pointer is null.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string that outlives `'a`.
unsafe fn c_str<'a>(p: *const c_char) -> Option<&'a CStr> {
    if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }) }
}

/// A `#[repr(transparent)]` wrapper with `raw()`, `as_ptr()` and `Deref` to the base wrapper.
macro_rules! wrapper {
    ($(#[$doc:meta] $name:ident($layout:ident) : $base:ident) => {
        $(#[$doc]
        #[repr(transparent)]
        pub struct $name(classes::$layout);

        impl $name {
            #[must_use]
            pub fn raw(&self) -> &classes::$layout {
                &self.0
            }

            #[must_use]
            pub fn as_ptr(&self) -> *const classes::$layout {
                &raw const self.0
            }
        }

        impl core::ops::Deref for $name {
            type Target = $base;

            // A packed layout struct has alignment 1; the live object is 8-aligned.
            #[allow(clippy::cast_ptr_alignment)]
            fn deref(&self) -> &$base {
                unsafe { &*(&raw const self.0).cast::<$base>() }
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                TESForm::fmt(self, f)
            }
        }
    };
}

wrapper!(
    /// A placed reference (`REFR`, `ACHR`, ...): a form with a base object, a transform, a cell, a
    /// handle and an extra data list. Reached through [`TESForm::as_reference`] or [`RefHandle::lookup`].
    TESObjectREFR(TESObjectREFR): TESForm
);
wrapper!(
    /// A reference with AI, actor values, inventory and state; `Character` and `PlayerCharacter` derive from it.
    Actor(Actor): TESObjectREFR
);
wrapper!(
    /// An NPC actor (`ACHR`); adds nothing to [`Actor`].
    Character(Character): Actor
);
wrapper!(
    /// The player, one instance behind `g_PlayerCharacter_ptr`.
    PlayerCharacter(PlayerCharacter): Actor
);

impl TESObjectREFR {
    fn vtable(&self) -> &vtables::TESObjectREFR_vtable {
        unsafe { &*self.0.__vftable }
    }

    #[must_use]
    pub fn base_object(&self) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr(self.0.baseObject) }
    }

    /// World position `[x, y, z]`.
    #[must_use]
    pub fn position(&self) -> [f32; 3] {
        [self.0.posX, self.0.posY, self.0.posZ]
    }

    /// Rotation `[x, y, z]` in radians; `z` is the heading.
    #[must_use]
    pub fn rotation(&self) -> [f32; 3] {
        [self.0.rotX, self.0.rotY, self.0.rotZ]
    }

    /// The cell the reference is in, `None` when it is not loaded.
    #[must_use]
    pub fn parent_cell(&self) -> Option<&TESObjectCELL> {
        unsafe { self.0.parentCell.cast::<TESObjectCELL>().as_ref() }
    }

    /// The root of the reference's 3D through vtable slot 112 (`Get3D`), `None` when not loaded.
    #[must_use]
    pub fn node_3d(&self) -> Option<&NiNode> {
        let get = self.vtable().Get3D()?;
        unsafe { get(self.as_ptr()).cast::<NiNode>().as_ref() }
    }

    /// The root node `loadedData->node3D` holds, `None` when the reference has no loaded data.
    #[must_use]
    pub fn loaded_node_3d(&self) -> Option<&NiNode> {
        let loaded = unsafe { self.0.loadedData.as_ref() }?;
        unsafe { loaded.node3D.cast::<NiNode>().as_ref() }
    }

    #[must_use]
    pub fn extra_list(&self) -> &BaseExtraList {
        unsafe { &*(&raw const self.0.extraList).cast::<BaseExtraList>() }
    }

    /// The scale in percent, as stored.
    #[must_use]
    pub fn ref_scale(&self) -> u16 {
        self.0.refScale
    }

    /// The effective scale through `TESObjectREFR::GetScale` (id 19664): `ref_scale / 100` times
    /// the race and NPC height for an actor.
    #[must_use]
    pub fn scale(&self) -> f32 {
        unsafe { (fns().get_scale)(self.as_ptr().cast_mut()) }
    }

    /// Whether the reference or its base carries the keyword, through vtable slot 72 (`HasKeyword`);
    /// shadows [`TESForm::has_keyword`], which only knows the form's own keyword component.
    #[must_use]
    pub fn has_keyword(&self, keyword: &BGSKeyword) -> bool {
        self.vtable().HasKeyword().is_some_and(|has| unsafe { has(self.as_ptr(), core::ptr::from_ref(keyword.raw())) })
    }

    /// Form flag bit 11.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.flags().contains(FormFlags::DISABLED)
    }

    /// Bits 0-9 of the `refCount` word: the number of `NiPointer`s to the object.
    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.refCount & 0x3FF
    }

    /// The reference's handle through `TESObjectREFR::GetHandle` (id 19846), created on first use;
    /// [`RefHandle::INVALID`] for an object nothing references.
    #[must_use]
    pub fn handle(&self) -> RefHandle {
        let mut handle = 0;
        unsafe { (fns().get_handle)(self.as_ptr().cast_mut(), &raw mut handle) };
        RefHandle(handle)
    }

    /// The inventory changes through `TESObjectREFR::GetInventoryChanges` (id 16039), `None` while
    /// the reference has none (the function creates nothing).
    #[must_use]
    pub fn inventory_changes(&self) -> Option<&InventoryChanges> {
        unsafe { (fns().get_inventory_changes)(self.as_ptr().cast_mut()).cast::<InventoryChanges>().as_ref() }
    }

    /// The base object's container component through `TESObjectREFR::GetContainer` (id 19702),
    /// `None` for a base without one.
    #[must_use]
    pub fn container(&self) -> Option<&TESContainer> {
        unsafe { (fns().get_container)(self.as_ptr().cast_mut()).cast::<TESContainer>().as_ref() }
    }

    /// How many of `item` the reference holds: `InventoryChanges::GetItemCount` when it has
    /// changes, otherwise the base container's count.
    #[must_use]
    pub fn item_count(&self, item: &TESForm) -> i32 {
        match self.inventory_changes() {
            Some(changes) => changes.item_count(item),
            None => self.container().map_or(0, |c| c.count(item)),
        }
    }

    /// Through `TESObjectREFR::GetCurrentLocation` (id 19812).
    #[must_use]
    pub fn current_location(&self) -> Option<&BGSLocation> {
        unsafe { (fns().get_current_location)(self.as_ptr().cast_mut()).cast::<BGSLocation>().as_ref() }
    }

    /// Through `TESObjectREFR::GetWorldspace` (id 19816); `None` in interiors.
    #[must_use]
    pub fn worldspace(&self) -> Option<&TESWorldSpace> {
        unsafe { (fns().get_worldspace)(self.as_ptr().cast_mut()).cast::<TESWorldSpace>().as_ref() }
    }

    /// The reference as an [`Actor`] when its type is `ACHR`.
    #[must_use]
    pub fn as_actor(&self) -> Option<&Actor> {
        (self.form_type() == Some(FormType::ACHR)).then(|| unsafe { &*self.as_ptr().cast::<Actor>() })
    }
}

/// A reference handle: bits 0-19 the handle table index, 20-25 the entry's age; `0` is the null handle.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RefHandle(pub u32);

impl RefHandle {
    /// `g_InvalidRefHandle` (id 400312) holds 0.
    pub const INVALID: Self = Self(0);

    #[must_use]
    pub const fn index(self) -> u32 {
        self.0 & 0xF_FFFF
    }

    #[must_use]
    pub const fn age(self) -> u32 {
        (self.0 >> 20) & 0x3F
    }

    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.0 == 0
    }

    /// The live reference through `LookupREFRByHandle` (id 12331): `None` when the handle is null,
    /// stale or its table entry inactive. The lookup adds one reference, which the [`RefPtr`]
    /// releases on drop.
    #[must_use]
    pub fn lookup(self) -> Option<RefPtr> {
        let mut handle = self.0;
        let mut out: *mut classes::TESObjectREFR = core::ptr::null_mut();
        let found = unsafe { (fns().lookup_by_handle)(&raw mut handle, &raw mut out) };
        if !found {
            return None;
        }
        NonNull::new(out.cast::<TESObjectREFR>()).map(RefPtr)
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
        let word = unsafe { AtomicU32::from_ptr(&raw mut (*self.0.as_ptr().cast::<classes::TESObjectREFR>()).refCount) };
        word.fetch_sub(1, Ordering::AcqRel);
    }
}

impl fmt::Debug for RefPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        TESForm::fmt(self, f)
    }
}

/// A scene graph node, reached through [`TESObjectREFR::node_3d`]; only its class is described.
#[repr(transparent)]
pub struct NiNode(classes::NiNode);

impl NiNode {
    #[must_use]
    pub fn raw(&self) -> &classes::NiNode {
        &self.0
    }

    /// The node's dynamic class from its vtable pointer (`BSFadeNode` for an actor's root).
    #[must_use]
    pub fn class(&self) -> Option<&'static layout::Vtable> {
        unsafe { crate::class_of((&raw const self.0).cast()) }
    }
}

/// The extra data list of a reference; only the typed lookups are wrapped, the node walk needs
/// the list's lock, which pass 2 did not describe.
#[repr(transparent)]
pub struct BaseExtraList(classes::BaseExtraList);

impl BaseExtraList {
    #[must_use]
    pub fn raw(&self) -> &classes::BaseExtraList {
        &self.0
    }

    /// Whether a node of extra data type `ty` is present, through `BaseExtraList::HasType` (id 11582).
    #[must_use]
    pub fn has_type(&self, ty: u32) -> bool {
        unsafe { (fns().extra_has_type)(&raw const self.0, ty) }
    }

    /// The node of extra data type `ty` through `BaseExtraList::GetByType` (id 12329).
    #[must_use]
    pub fn get_by_type(&self, ty: u32) -> Option<&classes::BSExtraData> {
        unsafe { (fns().extra_get_by_type)((&raw const self.0).cast_mut(), ty).as_ref() }
    }
}

/// The items a reference gained or lost relative to its base container.
#[repr(transparent)]
pub struct InventoryChanges(classes::InventoryChanges);

impl InventoryChanges {
    #[must_use]
    pub fn raw(&self) -> &classes::InventoryChanges {
        &self.0
    }

    #[must_use]
    pub fn owner(&self) -> Option<&TESObjectREFR> {
        unsafe { self.0.owner.cast::<TESObjectREFR>().as_ref() }
    }

    /// The entry list; null list items are skipped.
    pub fn entries(&self) -> impl Iterator<Item = &InventoryEntryData> {
        let mut node = unsafe { self.0.entries.as_ref() };
        core::iter::from_fn(move || {
            let n = node?;
            node = unsafe { n.next.cast::<classes::BSSimpleListNode>().as_ref() };
            Some(unsafe { n.item.cast::<InventoryEntryData>().as_ref() })
        })
        .flatten()
    }

    /// The base container's count plus the entry's delta, through `InventoryChanges::GetItemCount` (id 16108).
    #[must_use]
    pub fn item_count(&self, item: &TESForm) -> i32 {
        unsafe { (fns().item_count)((&raw const self.0).cast_mut(), item.as_ptr().cast_mut()) }
    }
}

/// One inventory entry: the object, its count relative to the base container and the per-stack
/// extra lists (`raw().extraLists`, a list pass 2 did not walk).
#[repr(transparent)]
pub struct InventoryEntryData(classes::InventoryEntryData);

impl InventoryEntryData {
    #[must_use]
    pub fn raw(&self) -> &classes::InventoryEntryData {
        &self.0
    }

    #[must_use]
    pub fn object(&self) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr(self.0.object) }
    }

    /// Count relative to the base container; negative when items were removed.
    #[must_use]
    pub fn count_delta(&self) -> i32 {
        self.0.countDelta.cast_signed()
    }
}

/// The base container component of an `NPC_` or `CONT` record (`TESNPC` +0x88).
#[repr(transparent)]
pub struct TESContainer(classes::TESContainer);

impl TESContainer {
    #[must_use]
    pub fn raw(&self) -> &classes::TESContainer {
        &self.0
    }

    #[must_use]
    pub fn entries(&self) -> &[Option<&ContainerObject>] {
        unsafe { ptr_slice(self.0.entries, self.0.numEntries) }
    }

    /// Through `TESContainer::CountObjectsInContainer` (id 14529).
    #[must_use]
    pub fn count(&self, item: &TESForm) -> i32 {
        unsafe { (fns().container_count)((&raw const self.0).cast_mut(), item.as_ptr().cast_mut()) }
    }
}

/// One base container entry.
#[repr(transparent)]
pub struct ContainerObject(classes::ContainerObject);

impl ContainerObject {
    #[must_use]
    pub fn raw(&self) -> &classes::ContainerObject {
        &self.0
    }

    #[must_use]
    pub fn count(&self) -> u32 {
        self.0.count
    }

    #[must_use]
    pub fn form(&self) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr(self.0.form) }
    }
}

impl Actor {
    fn vtable(&self) -> &vtables::Actor_vtable {
        unsafe { &*self.0.__vftable }
    }

    #[must_use]
    pub fn flags(&self) -> ActorFlags {
        ActorFlags(self.0.boolFlags)
    }

    /// `boolFlags` bit 26.
    #[must_use]
    pub fn is_player_teammate(&self) -> bool {
        self.flags().contains(ActorFlags::PLAYER_TEAMMATE)
    }

    /// The AI process, `None` for actors without one.
    #[must_use]
    pub fn process(&self) -> Option<&AIProcess> {
        unsafe { self.0.currentProcess.cast::<AIProcess>().as_ref() }
    }

    /// The `ActorValueOwner` sub-object at +0xB8.
    #[must_use]
    pub fn actor_value_owner(&self) -> &ActorValueOwner {
        unsafe { &*(&raw const self.0.__vftable_B8).cast::<ActorValueOwner>() }
    }

    /// The `ActorState` sub-object at +0xC0.
    #[must_use]
    pub fn state(&self) -> &ActorState {
        unsafe { &*(&raw const self.0.__vftable_C0).cast::<ActorState>() }
    }

    #[must_use]
    pub fn race(&self) -> Option<&TESRace> {
        unsafe { self.0.race.cast::<TESRace>().as_ref() }
    }

    /// The base object as an `NPC_` form.
    #[must_use]
    pub fn base_npc(&self) -> Option<&TESNPC> {
        self.base_object()?.as_npc()
    }

    /// Through `Actor::GetLevel` (id 37334).
    #[must_use]
    pub fn level(&self) -> u16 {
        unsafe { (fns().actor_level)(self.as_ptr().cast_mut()) }
    }

    /// Through vtable slot 153 (`IsDead`): dead or dying, with bleedout counted unless ignored.
    #[must_use]
    pub fn is_dead(&self, ignore_bleedout: bool) -> bool {
        self.vtable().IsDead().is_some_and(|f| unsafe { f(self.as_ptr(), ignore_bleedout) })
    }

    /// Through vtable slot 227 (`IsInCombat`).
    #[must_use]
    pub fn is_in_combat(&self) -> bool {
        self.vtable().IsInCombat().is_some_and(|f| unsafe { f(self.as_ptr()) })
    }

    /// Through `Actor::IsRunning` (id 37234).
    #[must_use]
    pub fn is_running(&self) -> bool {
        unsafe { (fns().is_running)(self.as_ptr().cast_mut()) }
    }

    /// Through `Actor::IsOnMount` (id 17965).
    #[must_use]
    pub fn is_on_mount(&self) -> bool {
        unsafe { (fns().is_on_mount)(self.as_ptr().cast_mut()) }
    }

    /// The weapon equipped in a hand through `Actor::GetEquippedObject` (id 38574); only `WEAP`
    /// entries are returned by the engine.
    #[must_use]
    pub fn equipped_object(&self, left_hand: bool) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr((fns().equipped_object)(self.as_ptr().cast_mut(), left_hand)) }
    }

    /// The `[permanent, temporary, damage]` modifiers kept in the actor for Health, Magicka,
    /// Stamina and `VoicePoints`; `None` for every other value, whose modifiers live in `avStorage`.
    #[must_use]
    pub fn modifiers(&self, value: ActorValue) -> Option<[f32; 3]> {
        let bytes = match value {
            ActorValue::Health => &raw const self.0.healthModifiers,
            ActorValue::Magicka => &raw const self.0.magickaModifiers,
            ActorValue::Stamina => &raw const self.0.staminaModifiers,
            ActorValue::VoicePoints => &raw const self.0.voicePointsModifiers,
            _ => return None,
        };
        Some(unsafe { bytes.cast::<[f32; 3]>().read_unaligned() })
    }

    /// The actor as a [`Character`] when its vtable is `Character`'s or `PlayerCharacter`'s.
    #[must_use]
    pub fn as_character(&self) -> Option<&Character> {
        let id = self.class()?.id;
        (id == vtables::Character_vtable::ID || id == vtables::PlayerCharacter_vtable::ID).then(|| unsafe { &*self.as_ptr().cast::<Character>() })
    }

    /// The actor as the [`PlayerCharacter`] when its vtable is `PlayerCharacter`'s.
    #[must_use]
    pub fn as_player(&self) -> Option<&PlayerCharacter> {
        (self.class()?.id == vtables::PlayerCharacter_vtable::ID).then(|| unsafe { &*self.as_ptr().cast::<PlayerCharacter>() })
    }
}

impl PlayerCharacter {
    /// The player behind `g_PlayerCharacter_ptr` (id 403521); `None` before it is constructed.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().player).cast::<Self>().as_ref() }
    }
}

/// The modifier slots of an actor value.
#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modifier {
    Permanent = 0,
    Temporary = 1,
    Damage = 2,
}

/// The actor value interface of an actor (`Actor` +0xB8): current = base + the three modifiers,
/// permanent = base + the permanent modifier.
#[repr(transparent)]
pub struct ActorValueOwner(classes::ActorValueOwner);

impl ActorValueOwner {
    fn vtable(&self) -> &vtables::ActorValueOwner_vtable {
        unsafe { &*self.0.__vftable }
    }

    fn as_ptr(&self) -> *mut classes::ActorValueOwner {
        (&raw const self.0).cast_mut()
    }

    /// Slot 1, `GetActorValue`.
    #[must_use]
    pub fn get(&self, value: ActorValue) -> f32 {
        let f = self.vtable().GetActorValue().expect("ActorValueOwner slot 1");
        unsafe { f(self.as_ptr(), value as i32) }
    }

    /// Slot 2, `GetPermanentActorValue`.
    #[must_use]
    pub fn get_permanent(&self, value: ActorValue) -> f32 {
        let f = self.vtable().GetPermanentActorValue().expect("ActorValueOwner slot 2");
        unsafe { f(self.as_ptr(), value as i32) }
    }

    /// Slot 3, `GetBaseActorValue`.
    #[must_use]
    pub fn get_base(&self, value: ActorValue) -> f32 {
        let f = self.vtable().GetBaseActorValue().expect("ActorValueOwner slot 3");
        unsafe { f(self.as_ptr(), value as i32) }
    }

    /// Slot 4, `SetBaseActorValue`.
    pub fn set_base(&self, value: ActorValue, amount: f32) {
        let f = self.vtable().SetBaseActorValue().expect("ActorValueOwner slot 4");
        unsafe { f(self.as_ptr(), value as i32, amount) }
    }

    /// Slot 5, `ModBaseActorValue`.
    pub fn mod_base(&self, value: ActorValue, delta: f32) {
        let f = self.vtable().ModBaseActorValue().expect("ActorValueOwner slot 5");
        unsafe { f(self.as_ptr(), value as i32, delta) }
    }

    /// Slot 6, `ModActorValue`: adds `delta` to one modifier of the value.
    pub fn mod_(&self, modifier: Modifier, value: ActorValue, delta: f32) {
        let f = self.vtable().ModActorValue().expect("ActorValueOwner slot 6");
        unsafe { f(self.as_ptr(), modifier as i32, value as i32, delta) }
    }

    /// Slot 7, `SetActorValue`; the implementation on an actor forwards to [`set_base`](Self::set_base).
    pub fn set(&self, value: ActorValue, amount: f32) {
        let f = self.vtable().SetActorValue().expect("ActorValueOwner slot 7");
        unsafe { f(self.as_ptr(), value as i32, amount) }
    }
}

/// The movement and weapon state of an actor (`Actor` +0xC0).
#[repr(transparent)]
pub struct ActorState(classes::ActorState);

impl ActorState {
    #[must_use]
    pub fn raw(&self) -> &classes::ActorState {
        &self.0
    }

    /// Movement flags: the named bits plus bits 18-20 flying state and 21-24 life state.
    #[must_use]
    pub fn flags1(&self) -> StateFlags {
        StateFlags(self.0.flags1)
    }

    /// State flags; bits 5-7 are the weapon state.
    #[must_use]
    pub fn flags2(&self) -> u32 {
        self.0.flags2
    }

    #[must_use]
    pub fn is_sneaking(&self) -> bool {
        self.flags1().contains(StateFlags::SNEAKING)
    }

    #[must_use]
    pub fn is_sprinting(&self) -> bool {
        self.flags1().contains(StateFlags::SPRINTING)
    }

    /// Bits 5-7 of `flags2`; 3 and above means drawn.
    #[must_use]
    pub fn weapon_state(&self) -> u32 {
        (self.0.flags2 >> 5) & 7
    }

    #[must_use]
    pub fn is_weapon_drawn(&self) -> bool {
        self.weapon_state() >= 3
    }

    /// Bits 21-24 of `flags1`.
    #[must_use]
    pub fn life_state(&self) -> u32 {
        (self.0.flags1 >> 21) & 0xF
    }

    /// Whether every bit of `mask` is set, through `ActorState::HasFlags` (id 38986), which only
    /// looks at the low 14 bits.
    #[must_use]
    pub fn has_flags(&self, mask: StateFlags) -> bool {
        let mask = u16::try_from(mask.bits() & 0x3FFF).unwrap_or(0);
        unsafe { (fns().has_flags)(&raw const self.0, mask) }
    }
}

/// The AI process of an actor (SKSE's `ActorProcessManager`).
#[repr(transparent)]
pub struct AIProcess(classes::AIProcess);

impl AIProcess {
    #[must_use]
    pub fn raw(&self) -> &classes::AIProcess {
        &self.0
    }

    #[must_use]
    pub fn middle_high(&self) -> Option<&MiddleHighProcessData> {
        unsafe { self.0.middleHigh.cast::<MiddleHighProcessData>().as_ref() }
    }

    /// The object in the left hand (field +0xF0).
    #[must_use]
    pub fn equipped_left(&self) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr(self.0.equippedLeft) }
    }

    /// The object in the right hand (field +0xF8).
    #[must_use]
    pub fn equipped_right(&self) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr(self.0.equippedRight) }
    }

    /// The equipped weapon entry of a hand through `AIProcess::GetEquippedEntry` (id 39806); the
    /// process owns a copy of the inventory entry.
    #[must_use]
    pub fn equipped_entry(&self, left_hand: bool) -> Option<&InventoryEntryData> {
        unsafe { (fns().equipped_entry)((&raw const self.0).cast_mut(), left_hand).cast::<InventoryEntryData>().as_ref() }
    }
}

/// Middle-high process data; only the two equipped entries are described (size rated medium).
#[repr(transparent)]
pub struct MiddleHighProcessData(classes::MiddleHighProcessData);

impl MiddleHighProcessData {
    #[must_use]
    pub fn raw(&self) -> &classes::MiddleHighProcessData {
        &self.0
    }

    #[must_use]
    pub fn left_hand_entry(&self) -> Option<&InventoryEntryData> {
        unsafe { self.0.leftHandEntry.cast::<InventoryEntryData>().as_ref() }
    }

    #[must_use]
    pub fn right_hand_entry(&self) -> Option<&InventoryEntryData> {
        unsafe { self.0.rightHandEntry.cast::<InventoryEntryData>().as_ref() }
    }
}

/// `TESNPC::GetSex` results.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sex {
    Male,
    Female,
}

impl TESNPC {
    fn actor_base_data(&self) -> *const classes::TESActorBaseData {
        (&raw const self.raw().__vftable_30).cast()
    }

    /// The full name through `TESFullName` vtable slot 5 (`GetFullName`) on the component at +0xD8.
    #[must_use]
    pub fn full_name(&self) -> Option<&CStr> {
        let name: *const classes::TESFullName = (&raw const self.raw().__vftable_D8).cast();
        let get = unsafe { (*(*name).__vftable).GetFullName() }?;
        unsafe { c_str(get(name)) }.filter(|s| !s.is_empty())
    }

    #[must_use]
    pub fn race(&self) -> Option<&TESRace> {
        unsafe { self.raw().race.cast::<TESRace>().as_ref() }
    }

    #[must_use]
    pub fn npc_class(&self) -> Option<&TESClass> {
        unsafe { self.raw().npcClass.cast::<TESClass>().as_ref() }
    }

    /// Through `TESNPC::GetSex` (id 24374); `None` when the engine answers neither 0 nor 1.
    #[must_use]
    pub fn sex(&self) -> Option<Sex> {
        match unsafe { (fns().npc_sex)(self.raw()) } {
            0 => Some(Sex::Male),
            1 => Some(Sex::Female),
            _ => None,
        }
    }

    /// `TESActorBaseData.flags` (the `ACBS` flags).
    #[must_use]
    pub fn base_flags(&self) -> ActorBaseFlags {
        ActorBaseFlags(self.raw().flags)
    }

    #[must_use]
    pub fn is_female(&self) -> bool {
        self.base_flags().contains(ActorBaseFlags::FEMALE)
    }

    #[must_use]
    pub fn is_essential(&self) -> bool {
        self.base_flags().contains(ActorBaseFlags::ESSENTIAL)
    }

    #[must_use]
    pub fn is_unique(&self) -> bool {
        self.base_flags().contains(ActorBaseFlags::UNIQUE)
    }

    /// The effective level through `TESActorBaseData::GetLevel` (id 14384); the `level` field holds
    /// the multiplier times 1000 when `PC_LEVEL_MULT` is set.
    #[must_use]
    pub fn level(&self) -> u32 {
        unsafe { (fns().base_level)(self.actor_base_data()) }
    }

    /// The height multiplier field.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.raw().height
    }

    /// Race height times the height multiplier, through `TESNPC::GetHeight` (id 24763).
    #[must_use]
    pub fn effective_height(&self) -> f32 {
        unsafe { (fns().npc_height)(self.raw()) }
    }

    /// The weight field (0-100); its meaning is rated medium in `actors.json`.
    #[must_use]
    pub fn weight(&self) -> f32 {
        self.raw().weight
    }

    /// The base container component at +0x88.
    #[must_use]
    pub fn container(&self) -> &TESContainer {
        unsafe { &*(&raw const self.raw().__vftable_88).cast::<TESContainer>() }
    }
}

wrapper!(
    /// An `AVIF` form describing one actor value.
    ActorValueInfo(ActorValueInfo): TESForm
);

impl ActorValueInfo {
    /// The editor name of the value.
    #[must_use]
    pub fn name(&self) -> Option<&CStr> {
        unsafe { c_str(self.0.name.cast::<c_char>().cast_const()) }
    }

    /// The `AVIF` flags from the builder table.
    #[must_use]
    pub fn flags(&self) -> u32 {
        self.0.flags
    }
}

/// The actor value table: one `ActorValueInfo` per [`ActorValue`].
#[repr(transparent)]
pub struct ActorValueList(classes::ActorValueList);

impl ActorValueList {
    /// The singleton behind `g_ActorValueList_ptr` (id 400267).
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().av_list).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::ActorValueList {
        &self.0
    }

    /// The 164 `ActorValueInfo` pointers indexed by [`ActorValue`]; they lie inline from +8, the
    /// generated `infos` field being the first of them (the ledger records the array by its first
    /// element).
    #[must_use]
    pub fn infos(&self) -> &[Option<&ActorValueInfo>] {
        unsafe { ptr_slice((&raw const self.0.infos).cast(), ActorValue::ReflectDamage as u32 + 1) }
    }
}

/// An actor value by editor name through `ActorValueList::LookupActorValueByName` (id 27203),
/// case-insensitive; `None` for an unknown name.
#[must_use]
pub fn lookup_actor_value(name: &str) -> Option<ActorValue> {
    let name = CString::new(name).ok()?;
    let id = unsafe { (fns().av_by_name)(name.as_ptr()) };
    ActorValue::from_u32(u32::try_from(id).ok()?)
}

/// The `AVIF` form of a value through `ActorValueList::GetActorValueInfo` (id 27202).
#[must_use]
pub fn actor_value_info(value: ActorValue) -> Option<&'static ActorValueInfo> {
    unsafe { (fns().av_info)(value as u32).cast::<ActorValueInfo>().as_ref() }
}
