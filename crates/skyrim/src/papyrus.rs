//! Papyrus VM and natives (subsystem pass 4): the [`VirtualMachine`] behind `g_SkyrimVM`, the
//! [`HandlePolicy`] that encodes forms as script handles, script values ([`Variable`],
//! [`TypeInfo`], [`Object`], [`ObjectTypeInfo`]) and the registration of Rust functions as the
//! natives a `.pex` declares.
//!
//! A native is a 0x58-byte `NativeFunctionBase` whose `IFunction` vtable the engine calls; this
//! module builds one per registered function, with the engine's own implementations of slots 0-20
//! copied from `NativeFunctionBase::vftable` and `HasBinding`/`CallBinding` of its own. `CallBinding` reads the
//! arguments out of the stack frame through `Stack::GetPageForFrame` and `GetStackFrameVariable`,
//! converts them with [`FromVariable`], calls the Rust function and writes the result back with
//! `Variable::assign`. Registration goes through `BindNativeMethod` (slot 24) and works before the
//! script exists; a `.pex` whose declaration matches (name, `global`, parameter count and types,
//! return type) must be in `Data\Scripts` for the native to be callable, or the Papyrus log reports
//! "does not match existing signature" when the script links.
//!
//! Calling Papyrus from Rust must happen on a game thread: the VM allocates stacks through the
//! per-thread memory manager, and pass 4 crashed the engine twice from a foreign thread.
//! [`VirtualMachine::dispatch_static_call`] therefore takes a [`GameThread`] token, which exists
//! inside a native callback ([`GameThread::current`]) and is asserted with
//! [`GameThread::assume`] from an SKSE message handler, task or event sink. Dispatch takes typed
//! arguments (`IFunctionArguments` filling the engine's `BSScrapArray<Variable>`) and a closure the
//! engine calls with the result (an `IStackCallbackFunctor` of this module's own) once the call
//! completes, on the VM thread that completed it (not the main thread), so the closure synchronizes
//! what it touches. A script object of a mod's own class is
//! created with [`VirtualMachine::create_object`] and bound to a form with
//! [`VirtualMachine::bind_object`], after which the engine and `DevBench` call its methods,
//! natives included, like any attached script's.
//!
//! Request 4e added the rest. Arrays are `BSScript::Array`s (a 0x20 header with the element type and
//! size, the Variables inline) that `CreateArray` (slot 22) makes on the global heap; `Vec<T>` is a
//! native parameter or return of type `T[]`. A latent native ([`VirtualMachine::register_latent`])
//! receives a [`Latent`] and returns at once; the calling script waits until [`Latent::complete`]
//! hands `ReturnFromLatent` (slot 43) the value, from inside the native or later from any thread.
//! [`VirtualMachine::send_event`] is `SendEvent` (slot 36), the call behind every engine event a
//! script receives, and [`VirtualMachine::relay_event`] the engine's second half of an event, which
//! reaches the aliases a reference fills and an actor's scripted active effects (request 4f;
//! [`alias_handles`] and [`active_effect_handles`] name them). The VM's own work (its update, the stacks, the completions) runs on rotating
//! worker threads, never on the main thread.

use core::cell::Cell;
use core::marker::PhantomData;
use core::ffi::{c_char, c_void};
use core::ptr::NonNull;
use std::ffi::CString;
use std::fmt;
use std::sync::{PoisonError, RwLock};

use sre::raw::{fns, globals, vtables};

use crate::actors::{Actor, TESObjectREFR};
use crate::forms::{BGSKeyword, BGSListForm, BGSLocation, FormId, TESClass, TESForm, TESGlobal, TESNPC, TESObjectCELL, TESObjectWEAP, TESRace, TESWorldSpace};
pub use crate::string::BSFixedString;
pub use sre::{PapyrusType, VMTypeID};

type VmRaw = sre::BSScript__Internal__VirtualMachine;
type VariableRaw = sre::BSScript__Variable;
type TypeRaw = sre::BSScript__ObjectTypeInfo;
type PolicyRaw = sre::SkyrimScript__HandlePolicy;

type GetPageForFrame = unsafe extern "C" fn(stack: *mut sre::BSScript__Stack, frame: *const sre::BSScript__StackFrame) -> u32;
type GetStackFrameVariable = unsafe extern "C" fn(stack: *mut sre::BSScript__Stack, frame: *const sre::BSScript__StackFrame, index: u32, page: u32) -> *mut VariableRaw;
type VariableAssign = unsafe extern "C" fn(this: *mut VariableRaw, other: *const VariableRaw);
type VariableReset = unsafe extern "C" fn(this: *mut VariableRaw);
type ObjectGetHandle = unsafe extern "C" fn(this: *mut sre::BSScript__Object) -> u64;
type ObjectDecRef = unsafe extern "C" fn(this: *mut sre::BSScript__Object) -> u32;
type PackFormToVariable = unsafe extern "C" fn(out: *mut VariableRaw, form: *mut sre::TESForm, vm: *mut VmRaw) -> *mut VariableRaw;
type TypeGetFunction = unsafe extern "C" fn(this: *mut TypeRaw, name: *const sre::BSFixedString, out: *mut *mut c_void, is_static: bool) -> u8;

type GetScriptObjectType = unsafe extern "C" fn(vm: *mut VmRaw, name: *const sre::BSFixedString, out: *mut *mut TypeRaw) -> u8;
type GetScriptObjectTypeByID = unsafe extern "C" fn(vm: *mut VmRaw, type_id: u32, out: *mut *mut TypeRaw) -> u8;
type GetTypeIDForScriptObject = unsafe extern "C" fn(vm: *mut VmRaw, name: *const sre::BSFixedString, out: *mut u32) -> u8;
type BindNativeMethod = unsafe extern "C" fn(vm: *mut VmRaw, function: *mut c_void) -> u8;
type SetCallableFromTasklets = unsafe extern "C" fn(vm: *mut VmRaw, class: *const c_char, name: *const c_char, callable: bool);
type FindBoundObject = unsafe extern "C" fn(vm: *mut VmRaw, handle: u64, class: *const c_char, out: *mut *mut sre::BSScript__Object) -> u8;
type DispatchStaticCall = unsafe extern "C" fn(vm: *mut VmRaw, class: *const sre::BSFixedString, name: *const sre::BSFixedString, args: *mut c_void, callback: *mut *mut c_void) -> u8;
type DispatchMethodCall = unsafe extern "C" fn(vm: *mut VmRaw, object: *mut *mut sre::BSScript__Object, name: *const sre::BSFixedString, args: *mut c_void, callback: *mut *mut c_void) -> u8;
type CreateObject = unsafe extern "C" fn(vm: *mut VmRaw, class: *const sre::BSFixedString, out: *mut *mut sre::BSScript__Object) -> u8;
type GetObjectBindPolicy = unsafe extern "C" fn(vm: *mut VmRaw) -> *mut c_void;
type BindObject = unsafe extern "C" fn(policy: *mut c_void, object: *mut *mut sre::BSScript__Object, handle: u64);
type ResizeVariableArray = unsafe extern "C" fn(array: *mut c_void, count: u64) -> u8;
type GetObjectHandlePolicy = unsafe extern "C" fn(vm: *mut VmRaw) -> *mut PolicyRaw;
type CreateArray = unsafe extern "C" fn(vm: *mut VmRaw, element: *const sre::BSScript__TypeInfo, size: u32, out: *mut *mut sre::BSScript__Array) -> u8;
type ReturnFromLatent = unsafe extern "C" fn(vm: *mut VmRaw, stack_id: u32, result: *const VariableRaw);
type SendEvent = unsafe extern "C" fn(vm: *mut VmRaw, handle: u64, event: *const sre::BSFixedString, args: *mut c_void);

type HandleIsType = unsafe extern "C" fn(this: *mut PolicyRaw, type_id: u32, handle: u64) -> u8;
type IsHandleObjectAvailable = unsafe extern "C" fn(this: *mut PolicyRaw, handle: u64) -> u8;
type GetInvalidHandle = unsafe extern "C" fn(this: *mut PolicyRaw) -> u64;
type GetHandleForObject = unsafe extern "C" fn(this: *mut PolicyRaw, type_id: u32, object: *const c_void) -> u64;
type GetObjectForHandle = unsafe extern "C" fn(this: *mut PolicyRaw, type_id: u32, handle: u64) -> *mut c_void;

type EffectListFn = unsafe extern "C" fn(magic_target: *mut c_void) -> *const EffectNode;

/// A node of the actor's `BSSimpleList<ActiveEffect*>`.
#[repr(C)]
struct EffectNode {
    effect: *const sre::ActiveEffect,
    next: *const EffectNode,
}

/// `ExtraAliasInstanceArray`'s extra data type.
const ALIAS_INSTANCES: u32 = 0x88;
/// `Actor`'s `MagicTarget` subobject.
const MAGIC_TARGET: usize = 0xa0;

type CallBindingSlot = unsafe extern "C" fn(this: *mut NativeHeader, self_: *mut VariableRaw, vm: *mut VmRaw, stack_id: u32, result: *mut VariableRaw, frame: *mut sre::BSScript__StackFrame) -> bool;
type HasBindingSlot = unsafe extern "C" fn(this: *mut NativeHeader) -> bool;

const IFUNCTION_SLOTS: usize = 23;
/// Natives take at most this many parameters, the arity the [`NativeFn`] impls cover.
pub const MAX_PARAMS: usize = 4;

pub(crate) struct Fns {
    page_for_frame: GetPageForFrame,
    frame_variable: GetStackFrameVariable,
    assign: VariableAssign,
    reset: VariableReset,
    object_handle: ObjectGetHandle,
    object_dec_ref: ObjectDecRef,
    pack_form: PackFormToVariable,
    type_function: TypeGetFunction,
    bind_object: BindObject,
    resize_args: ResizeVariableArray,
    relay: fns::SkyrimVM__RelayEventToRefAndEffects_fn,
    effect_list: EffectListFn,
    lock_read: fns::BSReadWriteLock__LockForRead_fn,
    unlock_read: fns::BSReadWriteLock__UnlockForRead_fn,
    skyrim_vm: *const sre::SkyrimVM,
    vtable: [*const c_void; IFUNCTION_SLOTS],
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        let base_vtable = reloc.address(vtables::BSScript__NF_util__NativeFunctionBase_vtable::ID)? as *const *const c_void;
        let mut vtable = [core::ptr::null(); IFUNCTION_SLOTS];
        for (i, entry) in vtable.iter_mut().enumerate().take(vtables::BSScript__NF_util__NativeFunctionBase_vtable::SLOT_HasBinding) {
            *entry = unsafe { *base_vtable.add(i) };
        }
        vtable[vtables::BSScript__NF_util__NativeFunctionBase_vtable::SLOT_HasBinding] = has_binding as HasBindingSlot as *const c_void;
        vtable[vtables::BSScript__NF_util__NativeFunctionBase_vtable::SLOT_CallBinding] = call_binding as CallBindingSlot as *const c_void;
        unsafe {
            Ok(Self {
                page_for_frame: crate::resolve(reloc, fns::BSScript__Stack__GetPageForFrame)?,
                frame_variable: crate::resolve(reloc, fns::BSScript__Stack__GetStackFrameVariable)?,
                assign: crate::resolve(reloc, fns::BSScript__Variable__assign)?,
                reset: crate::resolve(reloc, fns::BSScript__Variable__reset)?,
                object_handle: crate::resolve(reloc, fns::BSScript__Object__GetHandle)?,
                object_dec_ref: crate::resolve(reloc, fns::BSScript__Object__DecRef)?,
                pack_form: crate::resolve(reloc, fns::Papyrus__PackFormToVariable)?,
                type_function: crate::resolve(reloc, fns::BSScript__ObjectTypeInfo__GetFunction)?,
                bind_object: crate::resolve(reloc, fns::BSScript__ObjectBindPolicy__BindObject)?,
                resize_args: crate::resolve(reloc, fns::ResizeVariableArray)?,
                relay: crate::resolve(reloc, fns::SkyrimVM__RelayEventToRefAndEffects)?,
                effect_list: crate::resolve(reloc, fns::Actor__GetActiveEffectList)?,
                lock_read: crate::resolve(reloc, fns::BSReadWriteLock__LockForRead)?,
                unlock_read: crate::resolve(reloc, fns::BSReadWriteLock__UnlockForRead)?,
                skyrim_vm: reloc.address(globals::g_SkyrimVM)? as *const sre::SkyrimVM,
                vtable,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().papyrus
}

/// Slot `index` of the vtable an object's first word points at, as a function pointer of type `F`.
///
/// # Safety
/// `object` must point to a live object with a vtable of at least `index + 1` slots, and `F` must
/// match the slot's signature.
unsafe fn vslot<F: Copy>(object: *const c_void, index: usize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
    let table = unsafe { *object.cast::<*const *const c_void>() };
    unsafe { core::mem::transmute_copy::<*const c_void, F>(&*table.add(index)) }
}

#[derive(Debug)]
pub enum Error {
    /// `GetScriptObjectTypeByID` knows no script class for this type id.
    UnknownType(VMTypeID),
    /// `BindNativeMethod` refused the function (its class name or native flag did not pass).
    Bind { class: String, name: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownType(id) => write!(f, "the VM has no script type for {id:?}"),
            Self::Bind { class, name } => write!(f, "BindNativeMethod refused {class}.{name}"),
        }
    }
}

impl std::error::Error for Error {}

/// A script handle: `formID | 0xFFFF << 32` for a form, `questID | aliasID << 32` for an alias,
/// bit 48 set for an inventory item, bit 49 for an active effect; [`INVALID`](Self::INVALID) is
/// `GetInvalidHandle`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub u64);

impl Handle {
    pub const INVALID: Self = Self(0xFFFF << 32);

    #[must_use]
    pub const fn form(id: FormId) -> Self {
        Self(id.0 as u64 | 0xFFFF << 32)
    }

    /// A quest alias's handle: the quest's form id and the alias id (request 4f).
    #[must_use]
    pub const fn alias(quest: FormId, alias_id: u16) -> Self {
        Self(quest.0 as u64 | (alias_id as u64) << 32)
    }

    /// An active effect's handle: the target actor's form id and the effect's unique id (request 4f).
    #[must_use]
    pub const fn active_effect(target: FormId, unique_id: u16) -> Self {
        Self(target.0 as u64 | (unique_id as u64) << 32 | 2 << 48)
    }

    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.0 == Self::INVALID.0
    }

    /// The form id of a form handle, `None` for the invalid handle and the other encodings.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn form_id(self) -> Option<FormId> {
        let low = self.0 as u32;
        if self.0 >> 32 == 0xFFFF && low != 0 { Some(FormId(low)) } else { None }
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({:012X})", self.0)
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:012X}", self.0)
    }
}

/// `SkyrimScript::HandlePolicy`, the VM's `IObjectHandlePolicy` at `g_SkyrimVM + 0x338`; the same
/// object sits behind `g_IObjectHandlePolicy_ptr` once the VM has finished constructing (it is
/// still null during the SKSE registration callback).
#[repr(transparent)]
pub struct HandlePolicy(PolicyRaw);

impl HandlePolicy {
    /// The VM's policy through [`VirtualMachine::handle_policy`], `None` before the VM exists.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        VirtualMachine::get().map(VirtualMachine::handle_policy)
    }

    #[must_use]
    pub fn raw(&self) -> &PolicyRaw {
        &self.0
    }

    fn as_ptr(&self) -> *mut PolicyRaw {
        (&raw const self.0).cast_mut()
    }

    fn slot<F: Copy>(&self, index: usize) -> F {
        unsafe { vslot(self.as_ptr().cast(), index) }
    }

    /// Slot 1 `HandleIsType`: whether the handle names an object of the script class `type_id`
    /// (`Form` accepts every form, `ObjectReference` every reference).
    #[must_use]
    pub fn is_type(&self, type_id: VMTypeID, handle: Handle) -> bool {
        unsafe { self.slot::<HandleIsType>(vtables::SkyrimScript__HandlePolicy_vtable::SLOT_HandleIsType)(self.as_ptr(), type_id as u32, handle.0) != 0 }
    }

    /// Slot 2 `IsHandleObjectAvailable`: the object exists and is not flagged unavailable.
    #[must_use]
    pub fn is_available(&self, handle: Handle) -> bool {
        unsafe { self.slot::<IsHandleObjectAvailable>(vtables::SkyrimScript__HandlePolicy_vtable::SLOT_IsHandleObjectAvailable)(self.as_ptr(), handle.0) != 0 }
    }

    /// Slot 3 `GetInvalidHandle`.
    #[must_use]
    pub fn invalid_handle(&self) -> Handle {
        Handle(unsafe { self.slot::<GetInvalidHandle>(vtables::SkyrimScript__HandlePolicy_vtable::SLOT_GetInvalidHandle)(self.as_ptr()) })
    }

    /// Slot 4 `GetHandleForObject` with the `Form` type id, which encodes every form the same way.
    #[must_use]
    pub fn handle_for_form(&self, form: &TESForm) -> Handle {
        Handle(unsafe { self.slot::<GetHandleForObject>(vtables::SkyrimScript__HandlePolicy_vtable::SLOT_GetHandleForObject)(self.as_ptr(), VMTypeID::Form as u32, form.as_ptr().cast()) })
    }

    /// The form a handle names when it is of class `type_id` and available: slots 1, 2 and 8
    /// (`GetObjectForHandle`), the sequence the engine's own natives run on an object argument.
    #[must_use]
    pub fn form_for_handle(&self, type_id: VMTypeID, handle: Handle) -> Option<&'static TESForm> {
        if !self.is_type(type_id, handle) || !self.is_available(handle) {
            return None;
        }
        let p = unsafe { self.slot::<GetObjectForHandle>(vtables::SkyrimScript__HandlePolicy_vtable::SLOT_GetObjectForHandle)(self.as_ptr(), type_id as u32, handle.0) };
        unsafe { TESForm::from_ptr(p.cast()) }
    }
}

/// The handles of the quest aliases `reference` fills, from its `ExtraAliasInstanceArray` read under
/// the array's lock. A reference whose aliases live on another reference (through its
/// `ExtraReferenceHandle`, which the engine's relay follows) answers its own array only.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn alias_handles(reference: &TESObjectREFR) -> Vec<Handle> {
    let Some(extra) = reference.extra_list().get_by_type(ALIAS_INSTANCES) else { return Vec::new() };
    let array = core::ptr::from_ref(extra).cast::<sre::ExtraAliasInstanceArray>().cast_mut();
    let lock = unsafe { &raw mut (*array).lock };
    unsafe { (fns().lock_read)(lock) };
    let (entries, count) = unsafe { ((*array).entries.cast::<*const sre::ExtraAliasInstanceArray__Entry>(), (*array).count as usize) };
    let handles = (0..count)
        .filter_map(|i| {
            let entry = unsafe { (*entries.add(i)).as_ref() }?;
            let (quest, alias) = (unsafe { TESForm::from_ptr(entry.quest.cast()) }?, unsafe { entry.alias.as_ref() }?);
            Some(Handle::alias(quest.form_id(), alias.aliasID as u16))
        })
        .collect();
    unsafe { (fns().unlock_read)(lock) };
    handles
}

/// The handles of `actor`'s active effects that have one: an effect gets a unique id, and so a
/// handle, only when its `EffectSetting` is scripted (form flag bit 22). The list is the one
/// `MagicTarget` slot 7 returns, empty for an actor without middle-high process data.
#[must_use]
pub fn active_effect_handles(actor: &Actor) -> Vec<Handle> {
    let target = unsafe { actor.as_ptr().cast::<u8>().add(MAGIC_TARGET).cast_mut().cast() };
    let mut node = unsafe { (fns().effect_list)(target) };
    let mut handles = Vec::new();
    while let Some(n) = unsafe { node.as_ref() } {
        if let Some(effect) = unsafe { n.effect.as_ref() }
            && effect.uniqueID != 0
        {
            handles.push(Handle::active_effect(actor.form_id(), effect.uniqueID));
        }
        node = n.next;
    }
    handles
}

/// A form class a native can take or return: the script type id the handle policy checks and the
/// downcast from the form the policy resolves.
pub trait FormClass: 'static {
    const TYPE_ID: VMTypeID;
    fn from_form(form: &TESForm) -> Option<&Self>;
    fn as_form(&self) -> &TESForm;
}

impl FormClass for TESForm {
    const TYPE_ID: VMTypeID = VMTypeID::Form;
    fn from_form(form: &TESForm) -> Option<&Self> {
        Some(form)
    }
    fn as_form(&self) -> &TESForm {
        self
    }
}

impl FormClass for TESObjectREFR {
    const TYPE_ID: VMTypeID = VMTypeID::ObjectReference;
    fn from_form(form: &TESForm) -> Option<&Self> {
        form.as_reference()
    }
    fn as_form(&self) -> &TESForm {
        self
    }
}

impl FormClass for Actor {
    const TYPE_ID: VMTypeID = VMTypeID::Actor;
    fn from_form(form: &TESForm) -> Option<&Self> {
        form.as_reference()?.as_actor()
    }
    fn as_form(&self) -> &TESForm {
        self
    }
}

macro_rules! form_classes {
    ($($ty:ident = $id:ident / $as:ident),* $(,)?) => {$(
        impl FormClass for $ty {
            const TYPE_ID: VMTypeID = VMTypeID::$id;
            fn from_form(form: &TESForm) -> Option<&Self> {
                form.$as()
            }
            fn as_form(&self) -> &TESForm {
                self
            }
        }
    };
}

form_classes!(
    BGSKeyword = Keyword / as_keyword,
    BGSListForm = FormList / as_form_list,
    TESGlobal = GlobalVariable / as_global,
    TESObjectWEAP = Weapon / as_weapon,
    TESNPC = ActorBase / as_npc,
    TESRace = Race / as_race,
    TESClass = Class / as_class,
    TESObjectCELL = Cell / as_cell,
    TESWorldSpace = WorldSpace / as_worldspace,
    BGSLocation = Location / as_location,
);

/// `BSScript::TypeInfo`: a [`PapyrusType`] below 0x10, else an [`ObjectTypeInfo`] pointer with bit 0
/// marking an array of that class.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TypeInfo(u64);

impl TypeInfo {
    pub const NONE: Self = Self::raw(PapyrusType::None);

    #[must_use]
    pub const fn raw(ty: PapyrusType) -> Self {
        Self(ty as u64)
    }

    #[must_use]
    pub fn object(ty: &ObjectTypeInfo) -> Self {
        Self(ty.as_ptr() as u64)
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// The raw type for values below 0x10, `None` for an object class.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn papyrus_type(self) -> Option<PapyrusType> {
        if self.0 < 0x10 { PapyrusType::from_u8(self.0 as u8) } else { None }
    }

    /// The array type of this element type: raw 1-5 become 11-15, an object class gets bit 0.
    #[must_use]
    pub const fn array_of(self) -> Self {
        if self.0 < 0x10 { Self(self.0 + 10) } else { Self(self.0 | 1) }
    }

    #[must_use]
    pub const fn is_array(self) -> bool {
        (self.0 >= 0xB && self.0 <= 0xF) || (self.0 >= 0x10 && self.0 & 1 != 0)
    }

    /// The script class for an object type (or an array of it).
    #[must_use]
    pub fn object_type(self) -> Option<&'static ObjectTypeInfo> {
        if self.0 < 0x10 {
            return None;
        }
        unsafe { ((self.0 & !1) as *const ObjectTypeInfo).as_ref() }
    }
}

impl fmt::Debug for TypeInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.papyrus_type(), self.object_type()) {
            (Some(ty), _) => write!(f, "{ty:?}"),
            (None, Some(class)) => write!(f, "{}{}", class.name(), if self.is_array() { "[]" } else { "" }),
            (None, None) => write!(f, "TypeInfo({:#x})", self.0),
        }
    }
}

/// `BSScript::ObjectTypeInfo`: a script class, one per `.pex` or native class.
///
/// The lookups that return one add a reference the engine's smart pointer would drop; this
/// module keeps it, since every registered native holds its parameter classes for the life of the
/// game anyway.
#[repr(transparent)]
pub struct ObjectTypeInfo(TypeRaw);

impl ObjectTypeInfo {
    #[must_use]
    pub fn raw(&self) -> &TypeRaw {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const TypeRaw {
        &raw const self.0
    }

    #[must_use]
    pub fn name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.name) }
    }

    #[must_use]
    pub fn parent(&self) -> Option<&ObjectTypeInfo> {
        unsafe { self.0.parent.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.refCount
    }

    /// Bits 0-1 of `linkedAndVariables`: 0 unlinked (natives wait in a list), 3 fully linked.
    #[must_use]
    pub fn link_state(&self) -> u32 {
        self.0.linkedAndVariables & 3
    }

    #[must_use]
    pub fn is_linked(&self) -> bool {
        self.link_state() == 3
    }

    /// Bits 8-17 of `linkedAndVariables`: the class's own variable count.
    #[must_use]
    pub fn variable_count(&self) -> u32 {
        (self.0.linkedAndVariables >> 8) & 0x3FF
    }

    /// The `IFunction` the class binds under `name` through `ObjectTypeInfo::GetFunction`
    /// (id 104311): on a linked type any function of the tables, on an unlinked one only the
    /// non-static natives bound so far. The pointer is the native object `register_native`
    /// returned when that native is bound. The reference the lookup adds is kept.
    #[must_use]
    pub fn function(&self, name: &str, is_static: bool) -> Option<NonNull<c_void>> {
        let name = BSFixedString::new(name);
        let mut out = core::ptr::null_mut();
        let found = unsafe { (fns().type_function)((&raw const self.0).cast_mut(), name.raw(), &raw mut out, is_static) };
        if found == 0 {
            return None;
        }
        NonNull::new(out)
    }
}

impl fmt::Debug for ObjectTypeInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (link state {}, {} variables)", self.name(), self.link_state(), self.variable_count())
    }
}

/// `BSScript::Object`: a script object bound to a handle, as the payload of an object [`Variable`].
#[repr(transparent)]
pub struct Object(sre::BSScript__Object);

impl Object {
    #[must_use]
    pub fn raw(&self) -> &sre::BSScript__Object {
        &self.0
    }

    /// The handle through `Object::GetHandle` (id 104247), read under the object's lock bit.
    #[must_use]
    pub fn handle(&self) -> Handle {
        Handle(unsafe { (fns().object_handle)((&raw const self.0).cast_mut()) })
    }

    #[must_use]
    pub fn type_info(&self) -> Option<&ObjectTypeInfo> {
        unsafe { self.0.type_.cast::<ObjectTypeInfo>().as_ref() }
    }

    /// Bits 0-30 of `refCountAndLock`.
    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.refCountAndLock & 0x7FFF_FFFF
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} object {} (refs {})", self.type_info().map_or_else(|| "?".to_owned(), |t| t.name().to_string()), self.handle(), self.ref_count())
    }
}

/// A script object held by one reference, taken by [`VirtualMachine::find_bound_object`] and
/// dropped through `Object::DecRef` (id 104253). The VM's own table keeps a bound object alive, so
/// the count never reaches zero here.
pub struct ObjectPtr(NonNull<Object>);

impl core::ops::Deref for ObjectPtr {
    type Target = Object;

    fn deref(&self) -> &Object {
        unsafe { self.0.as_ref() }
    }
}

impl Drop for ObjectPtr {
    fn drop(&mut self) {
        unsafe { (fns().object_dec_ref)(self.0.as_ptr().cast()) };
    }
}

impl fmt::Debug for ObjectPtr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Object::fmt(self, f)
    }
}

/// `BSScript::Array` (request 4e): the creator's element type, the size and `size` Variables inline
/// after the 0x20 header, each carrying its own type. Held through the Variables that point at it;
/// the last reference resets the elements and frees the block.
#[repr(transparent)]
pub struct Array(sre::BSScript__Array);

impl Array {
    #[must_use]
    pub fn raw(&self) -> &sre::BSScript__Array {
        &self.0
    }

    /// The element type the creator passed; None for the arrays the script extender's natives make.
    #[must_use]
    pub fn element_type(&self) -> TypeInfo {
        TypeInfo(self.0.elementType.rawTypeOrObjectType)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.size as usize
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn data(&self) -> *mut VariableRaw {
        unsafe { (&raw const self.0).add(1).cast::<VariableRaw>().cast_mut() }
    }

    #[must_use]
    pub fn elements(&self) -> &[Variable] {
        unsafe { core::slice::from_raw_parts(self.data().cast::<Variable>(), self.len()) }
    }

    /// Copies `value` into element `index` through `Variable::assign`; false past the end.
    #[must_use]
    pub fn set(&self, index: usize, value: &Variable) -> bool {
        if index >= self.len() {
            return false;
        }
        unsafe { value.assign_to(self.data().add(index)) };
        true
    }
}

impl fmt::Debug for Array {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.elements()).finish()
    }
}

/// `BSScript::Variable`: a type tag and an 8-byte payload. Owned values release their payload
/// (string, object) through `Variable::reset` on drop; views into engine memory come from
/// [`from_ptr`](Self::from_ptr).
#[repr(transparent)]
pub struct Variable(VariableRaw);

impl Variable {
    #[must_use]
    pub const fn none() -> Self {
        Self::scalar(PapyrusType::None, 0)
    }

    const fn scalar(ty: PapyrusType, value: u64) -> Self {
        Self(VariableRaw { type_: sre::BSScript__TypeInfo { rawTypeOrObjectType: ty as u64 }, value })
    }

    #[must_use]
    pub const fn int(value: i32) -> Self {
        Self::scalar(PapyrusType::Int, value.cast_unsigned() as u64)
    }

    #[must_use]
    pub const fn float(value: f32) -> Self {
        Self::scalar(PapyrusType::Float, value.to_bits() as u64)
    }

    #[must_use]
    pub const fn bool(value: bool) -> Self {
        Self::scalar(PapyrusType::Bool, value as u64)
    }

    /// A String holding its own reference to the interned text.
    #[must_use]
    pub fn string(text: &BSFixedString) -> Self {
        Self::scalar(PapyrusType::String, text.clone().into_raw().data as u64)
    }

    /// The object Variable for a form through `Papyrus::PackFormToVariable` (id 55511): the
    /// form's own script class, its handle, the bound object or a new one; None for a null form.
    #[must_use]
    pub fn form(form: Option<&TESForm>, vm: &VirtualMachine) -> Self {
        let mut out = Self::none();
        if let Some(form) = form {
            unsafe { (fns().pack_form)(&raw mut out.0, form.as_ptr().cast_mut(), vm.as_ptr()) };
        }
        out
    }

    /// # Safety
    /// `p` must be null or point to a `Variable` that stays valid for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const VariableRaw) -> Option<&'a Self> {
        unsafe { p.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &VariableRaw {
        &self.0
    }

    #[must_use]
    pub fn type_info(&self) -> TypeInfo {
        TypeInfo(self.0.type_.rawTypeOrObjectType)
    }

    fn is(&self, ty: PapyrusType) -> bool {
        self.type_info() == TypeInfo::raw(ty)
    }

    /// The low 32 bits of the payload, where Int, Float and Bool keep their value.
    #[allow(clippy::cast_possible_truncation)]
    fn word(&self) -> u32 {
        self.0.value as u32
    }

    #[must_use]
    pub fn as_int(&self) -> Option<i32> {
        self.is(PapyrusType::Int).then_some(self.word().cast_signed())
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f32> {
        self.is(PapyrusType::Float).then_some(f32::from_bits(self.word()))
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        self.is(PapyrusType::Bool).then_some(self.word() & 0xFF != 0)
    }

    #[must_use]
    pub fn as_string(&self) -> Option<&BSFixedString> {
        self.is(PapyrusType::String).then(|| unsafe { BSFixedString::from_ptr((&raw const self.0.value).cast()) })
    }

    /// The object of an object-typed Variable, `None` for other types and for a None object.
    #[must_use]
    pub fn as_object(&self) -> Option<&Object> {
        let ty = self.type_info();
        if ty.object_type().is_none() || ty.is_array() {
            return None;
        }
        unsafe { (self.0.value as *const Object).as_ref() }
    }

    /// The array of an array-typed Variable, `None` for other types and for a None array.
    #[must_use]
    pub fn as_array(&self) -> Option<&Array> {
        if !self.type_info().is_array() {
            return None;
        }
        unsafe { (self.0.value as *const Array).as_ref() }
    }

    /// The form behind an object Variable, resolved through the handle policy with `T`'s type id.
    #[must_use]
    pub fn as_form<T: FormClass>(&self) -> Option<&'static T> {
        let handle = self.as_object()?.handle();
        HandlePolicy::get()?.form_for_handle(T::TYPE_ID, handle).and_then(T::from_form)
    }

    /// Copies this value into `target` through `Variable::assign` (id 104297), which releases
    /// what `target` held and adds a reference for a string or object payload.
    ///
    /// # Safety
    /// `target` must point to a `Variable` the engine owns or one that is initialized.
    pub unsafe fn assign_to(&self, target: *mut VariableRaw) {
        unsafe { (fns().assign)(target, &raw const self.0) };
    }
}

impl Drop for Variable {
    fn drop(&mut self) {
        unsafe { (fns().reset)(&raw mut self.0) };
    }
}

impl fmt::Debug for Variable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.type_info().papyrus_type() {
            Some(PapyrusType::None) => f.write_str("None"),
            Some(PapyrusType::Int) => write!(f, "Int {}", self.word().cast_signed()),
            Some(PapyrusType::Float) => write!(f, "Float {}", f32::from_bits(self.word())),
            Some(PapyrusType::Bool) => write!(f, "Bool {}", self.word() & 0xFF != 0),
            Some(PapyrusType::String) => write!(f, "String {:?}", self.as_string().map(ToString::to_string).unwrap_or_default()),
            _ => match self.as_object() {
                Some(object) => write!(f, "{object:?}"),
                None => match self.as_array() {
                    Some(array) => write!(f, "{:?} {array:?}", self.type_info()),
                    None => write!(f, "{:?} {:#x}", self.type_info(), self.0.value),
                },
            },
        }
    }
}

/// The Papyrus type of a native's parameter or return value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamType {
    Int,
    Float,
    Bool,
    String,
    Form(VMTypeID),
    IntArray,
    FloatArray,
    BoolArray,
    StringArray,
    FormArray(VMTypeID),
}

impl ParamType {
    /// The array of this element type; Papyrus has no arrays of arrays.
    #[must_use]
    pub const fn array(self) -> Self {
        match self {
            Self::Int => Self::IntArray,
            Self::Float => Self::FloatArray,
            Self::Bool => Self::BoolArray,
            Self::String => Self::StringArray,
            Self::Form(id) => Self::FormArray(id),
            _ => panic!("Papyrus has no arrays of arrays"),
        }
    }

    fn type_info(self, vm: &VirtualMachine) -> Result<TypeInfo, Error> {
        Ok(match self {
            Self::Int => TypeInfo::raw(PapyrusType::Int),
            Self::Float => TypeInfo::raw(PapyrusType::Float),
            Self::Bool => TypeInfo::raw(PapyrusType::Bool),
            Self::String => TypeInfo::raw(PapyrusType::String),
            Self::Form(id) => TypeInfo::object(vm.script_type_by_id(id).ok_or(Error::UnknownType(id))?),
            Self::IntArray => Self::Int.type_info(vm)?.array_of(),
            Self::FloatArray => Self::Float.type_info(vm)?.array_of(),
            Self::BoolArray => Self::Bool.type_info(vm)?.array_of(),
            Self::StringArray => Self::String.type_info(vm)?.array_of(),
            Self::FormArray(id) => Self::Form(id).type_info(vm)?.array_of(),
        })
    }
}

/// A Rust parameter type of a native and how it reads its [`Variable`]. The VM checks the
/// declared types when the script links, so a value of another type reads as the default.
pub trait FromVariable: Sized {
    const TYPE: ParamType;
    fn from_variable(v: &Variable) -> Self;
}

impl FromVariable for i32 {
    const TYPE: ParamType = ParamType::Int;
    fn from_variable(v: &Variable) -> Self {
        v.as_int().unwrap_or(0)
    }
}

impl FromVariable for f32 {
    const TYPE: ParamType = ParamType::Float;
    fn from_variable(v: &Variable) -> Self {
        v.as_float().unwrap_or(0.0)
    }
}

impl FromVariable for bool {
    const TYPE: ParamType = ParamType::Bool;
    fn from_variable(v: &Variable) -> Self {
        v.as_bool().unwrap_or(false)
    }
}

impl FromVariable for BSFixedString {
    const TYPE: ParamType = ParamType::String;
    fn from_variable(v: &Variable) -> Self {
        v.as_string().map_or_else(BSFixedString::empty, Clone::clone)
    }
}

impl FromVariable for String {
    const TYPE: ParamType = ParamType::String;
    fn from_variable(v: &Variable) -> Self {
        v.as_string().map(ToString::to_string).unwrap_or_default()
    }
}

impl<T: FormClass> FromVariable for Option<&'static T> {
    const TYPE: ParamType = ParamType::Form(T::TYPE_ID);
    fn from_variable(v: &Variable) -> Self {
        v.as_form::<T>()
    }
}

impl<T: FromVariable> FromVariable for Vec<T> {
    const TYPE: ParamType = T::TYPE.array();
    fn from_variable(v: &Variable) -> Self {
        v.as_array().map(|a| a.elements().iter().map(T::from_variable).collect()).unwrap_or_default()
    }
}

/// A Rust return type of a native and the [`Variable`] it becomes; `()` is a `Function` without
/// a return type.
pub trait IntoVariable {
    const TYPE: Option<ParamType>;
    fn into_variable(self, vm: &VirtualMachine) -> Variable;
}

impl IntoVariable for () {
    const TYPE: Option<ParamType> = None;
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::none()
    }
}

impl IntoVariable for i32 {
    const TYPE: Option<ParamType> = Some(ParamType::Int);
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::int(self)
    }
}

impl IntoVariable for f32 {
    const TYPE: Option<ParamType> = Some(ParamType::Float);
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::float(self)
    }
}

impl IntoVariable for bool {
    const TYPE: Option<ParamType> = Some(ParamType::Bool);
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::bool(self)
    }
}

impl IntoVariable for BSFixedString {
    const TYPE: Option<ParamType> = Some(ParamType::String);
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::string(&self)
    }
}

impl IntoVariable for &str {
    const TYPE: Option<ParamType> = Some(ParamType::String);
    fn into_variable(self, _: &VirtualMachine) -> Variable {
        Variable::string(&BSFixedString::new(self))
    }
}

impl IntoVariable for String {
    const TYPE: Option<ParamType> = Some(ParamType::String);
    fn into_variable(self, vm: &VirtualMachine) -> Variable {
        self.as_str().into_variable(vm)
    }
}

impl<T: FormClass> IntoVariable for Option<&T> {
    const TYPE: Option<ParamType> = Some(ParamType::Form(T::TYPE_ID));
    fn into_variable(self, vm: &VirtualMachine) -> Variable {
        Variable::form(self.map(FormClass::as_form), vm)
    }
}

/// A new array through [`VirtualMachine::create_array`]; elements that convert to None keep the
/// array's initial `{element type, 0}`, which is how the engine holds a None form in a `Form[]`.
impl<T: IntoVariable> IntoVariable for Vec<T> {
    const TYPE: Option<ParamType> = match T::TYPE {
        Some(t) => Some(t.array()),
        None => panic!("a Papyrus array needs an element type"),
    };
    fn into_variable(self, vm: &VirtualMachine) -> Variable {
        let Some(Ok(element)) = T::TYPE.map(|t| t.type_info(vm)) else { return Variable::none() };
        let Some(array) = u32::try_from(self.len()).ok().and_then(|len| vm.create_array(element, len)) else { return Variable::none() };
        if let Some(a) = array.as_array() {
            for (i, value) in self.into_iter().enumerate() {
                let value = value.into_variable(vm);
                if value.type_info() != TypeInfo::NONE {
                    let _ = a.set(i, &value);
                }
            }
        }
        array
    }
}

/// A started latent call ([`VirtualMachine::register_latent`]): complete it once, from the native
/// or later from any thread, with the value the waiting script receives.
#[must_use = "a latent call never completed leaves its script waiting until the VM is reset by a load"]
pub struct Latent<R> {
    stack_id: u32,
    result: PhantomData<fn(R)>,
}

impl<R: IntoVariable> Latent<R> {
    #[must_use]
    pub fn stack_id(&self) -> u32 {
        self.stack_id
    }

    /// Slot 43 `ReturnFromLatent`: the VM copies the value into the waiting stack and completes it,
    /// on this thread when the latent native was the stack's only frame (a dispatch), else in a later
    /// VM update on a worker.
    pub fn complete(self, value: R) {
        let Some(vm) = VirtualMachine::get() else { return };
        let value = value.into_variable(vm);
        unsafe { vm.slot::<ReturnFromLatent>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_ReturnFromLatent)(vm.as_ptr(), self.stack_id, &raw const value.0) };
    }
}

/// A Rust function registrable as a latent global function: its first parameter is the [`Latent`]
/// it completes with a value of type `Output`, the declared return type.
pub trait LatentFn<M>: Copy + Send + Sync + 'static {
    type Output: IntoVariable;
    const PARAMS: &'static [ParamType];
    fn start(&self, latent: Latent<Self::Output>, args: &[&Variable]);
}

/// A Rust function registrable as a global (`native global`) Papyrus function: any `Fn` of up to
/// [`MAX_PARAMS`] [`FromVariable`] parameters returning an [`IntoVariable`]. `M` is inferred.
pub trait NativeFn<M>: Copy + Send + Sync + 'static {
    const PARAMS: &'static [ParamType];
    const RETURN: Option<ParamType>;
    fn invoke(&self, vm: &VirtualMachine, args: &[&Variable]) -> Variable;
}

/// A Rust function registrable as a method (`native`, no `global`) on a script class that extends
/// a form class: the first parameter is the form the script object is bound to.
pub trait NativeMethod<M>: Copy + Send + Sync + 'static {
    type Receiver: FormClass;
    const PARAMS: &'static [ParamType];
    const RETURN: Option<ParamType>;
    fn invoke(&self, vm: &VirtualMachine, receiver: &Self::Receiver, args: &[&Variable]) -> Variable;
}

macro_rules! native_fns {
    ($($a:ident $i:tt),*) => {
        impl<F, R, $($a),*> LatentFn<(Latent<R>, $($a,)> for F
        where
            F: Fn(Latent<R>, $($a),*) + Copy + Send + Sync + 'static,
            R: IntoVariable,
            $($a: FromVariable,
        {
            type Output = R;
            const PARAMS: &'static [ParamType] = &[$($a::TYPE),*];
            #[allow(unused_variables)]
            fn start(&self, latent: Latent<R>, args: &[&Variable]) {
                self(latent, $($a::from_variable(args[$i])),*);
            }
        }

        impl<F, R, $($a),*> NativeFn<(R, $($a,)> for F
        where
            F: Fn($($a),*) -> R + Copy + Send + Sync + 'static,
            R: IntoVariable,
            $($a: FromVariable,
        {
            const PARAMS: &'static [ParamType] = &[$($a::TYPE),*];
            const RETURN: Option<ParamType> = R::TYPE;
            #[allow(unused_variables)]
            fn invoke(&self, vm: &VirtualMachine, args: &[&Variable]) -> Variable {
                self($($a::from_variable(args[$i])),*).into_variable(vm)
            }
        }

        impl<F, S, R, $($a),*> NativeMethod<(S, R, $($a,)> for F
        where
            F: for<'s> Fn(&'s S, $($a),*) -> R + Copy + Send + Sync + 'static,
            S: FormClass,
            R: IntoVariable,
            $($a: FromVariable,
        {
            type Receiver = S;
            const PARAMS: &'static [ParamType] = &[$($a::TYPE),*];
            const RETURN: Option<ParamType> = R::TYPE;
            #[allow(unused_variables)]
            fn invoke(&self, vm: &VirtualMachine, receiver: &S, args: &[&Variable]) -> Variable {
                self(receiver, $($a::from_variable(args[$i])),*).into_variable(vm)
            }
        }
    };
}

native_fns!();
native_fns!(A0 0);
native_fns!(A0 0, A1 1);
native_fns!(A0 0, A1 1, A2 2);
native_fns!(A0 0, A1 1, A2 2, A3 3);

/// The engine-visible part of a registered native: the 0x58-byte `NativeFunctionBase` followed by
/// the Rust entry `run` dispatches to.
#[repr(C)]
struct NativeHeader {
    base: sre::BSScript__NF_util__NativeFunctionBase,
    invoke: unsafe fn(this: *const NativeHeader, call: &NativeCall<'_>) -> Option<Variable>,
}

#[repr(C)]
struct Native<F> {
    header: NativeHeader,
    f: F,
}

impl<F> Native<F> {
    fn function(&'static self) -> &'static NativeFunction {
        unsafe { &*core::ptr::from_ref(&self.header).cast::<NativeFunction>() }
    }
}

/// A registered native, leaked for the life of the game because the VM keeps its pointer.
#[repr(transparent)]
pub struct NativeFunction(NativeHeader);

unsafe impl Send for NativeFunction {}
unsafe impl Sync for NativeFunction {}

impl NativeFunction {
    #[must_use]
    pub fn raw(&self) -> &sre::BSScript__NF_util__NativeFunctionBase {
        &self.0.base
    }

    /// The object the VM holds, for comparison with [`ObjectTypeInfo::function`].
    #[must_use]
    pub fn as_ptr(&self) -> NonNull<c_void> {
        NonNull::from(&self.0).cast()
    }

    #[must_use]
    pub fn name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.base.name) }
    }

    #[must_use]
    pub fn class_name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr(&raw const self.0.base.objectTypeName) }
    }

    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.0.base.refCount
    }

    #[must_use]
    pub fn return_type(&self) -> TypeInfo {
        TypeInfo(self.0.base.returnType.rawTypeOrObjectType)
    }

    #[must_use]
    pub fn param_types(&self) -> Vec<TypeInfo> {
        (0..usize::from(self.0.base.paramCount)).map(|i| TypeInfo(unsafe { (*self.0.base.params.add(i)).type_.rawTypeOrObjectType })).collect()
    }
}

/// A native call as the VM makes it: the VM, the `self` variable, the arguments and the stack.
pub struct NativeCall<'a> {
    pub vm: &'a VirtualMachine,
    pub self_: &'a Variable,
    pub args: &'a [&'a Variable],
    pub stack_id: u32,
}

impl NativeCall<'_> {
    /// Runs `f` as a global function ([`VirtualMachine::register_native`]).
    #[must_use]
    pub fn global<F: NativeFn<M>, M>(&self, f: F) -> Option<Variable> {
        self.on_stack(|| Some(f.invoke(self.vm, self.args)))
    }

    /// Runs `f` as a method ([`VirtualMachine::register_method`]); `None` when `self` is not bound to its form class.
    #[must_use]
    pub fn method<F: NativeMethod<M>, M>(&self, f: F) -> Option<Variable> {
        self.on_stack(|| Some(f.invoke(self.vm, self.self_.as_form::<F::Receiver>()?, self.args)))
    }

    /// Starts `f` as a latent function ([`VirtualMachine::register_latent`]) and answers the true Bool `Call` requires before the stack waits.
    #[must_use]
    pub fn latent<F: LatentFn<M>, M>(&self, f: F) -> Option<Variable> {
        self.on_stack(|| {
            f.start(Latent { stack_id: self.stack_id, result: PhantomData }, self.args);
            Some(Variable::bool(true))
        })
    }

    /// Makes the stack id visible to [`GameThread::current`] and [`current_stack_id`] in this copy of the crate while `f` runs.
    fn on_stack<R>(&self, f: impl FnOnce() -> R) -> R {
        let previous = STACK_ID.replace(Some(self.stack_id));
        let r = f();
        STACK_ID.set(previous);
        r
    }
}

unsafe fn invoke_fn<F: NativeFn<M>, M>(this: *const NativeHeader, call: &NativeCall<'_>) -> Option<Variable> {
    call.global(unsafe { &*this.cast::<Native<F>>() }.f)
}

unsafe fn invoke_method<F: NativeMethod<M>, M>(this: *const NativeHeader, call: &NativeCall<'_>) -> Option<Variable> {
    call.method(unsafe { &*this.cast::<Native<F>>() }.f)
}

unsafe fn invoke_latent<F: LatentFn<M>, M>(this: *const NativeHeader, call: &NativeCall<'_>) -> Option<Variable> {
    call.latent(unsafe { &*this.cast::<Native<F>>() }.f)
}

unsafe fn invoke_erased(this: *const NativeHeader, call: &NativeCall<'_>) -> Option<Variable> {
    let native = unsafe { &*this.cast::<Native<RwLock<ErasedNativeFn>>>() };
    let f = native.f.read().unwrap_or_else(PoisonError::into_inner);
    f(call)
}

unsafe extern "C" fn has_binding(_: *mut NativeHeader) -> bool {
    true
}

thread_local! {
    static STACK_ID: Cell<Option<u32>> = const { Cell::new(None) };
}

unsafe extern "C" fn call_binding(this: *mut NativeHeader, self_: *mut VariableRaw, vm: *mut VmRaw, stack_id: u32, result: *mut VariableRaw, frame: *mut sre::BSScript__StackFrame) -> bool {
    let (Some(this), Some(self_), Some(vm), Some(frame)) = (unsafe { this.as_ref() }, unsafe { Variable::from_ptr(self_) }, unsafe { vm.cast::<VirtualMachine>().as_ref() }, unsafe { frame.as_ref() }) else {
        return false;
    };
    let count = usize::from(this.base.paramCount);
    if count > MAX_PARAMS || (frame.size as usize) < count {
        return false;
    }
    let page = unsafe { (fns().page_for_frame)(frame.owningStack, frame) };
    let none = Variable::none();
    let mut args = [&none; MAX_PARAMS];
    for (i, arg) in (0u32..).zip(args.iter_mut().take(count)) {
        let Some(v) = (unsafe { Variable::from_ptr((fns().frame_variable)(frame.owningStack, frame, i, page)) }) else {
            return false;
        };
        *arg = v;
    }
    let call = NativeCall { vm, self_, args: &args[..count], stack_id };
    match unsafe { (this.invoke)(this, &call) } {
        Some(out) => {
            unsafe { out.assign_to(result) };
            true
        }
        None => false,
    }
}

/// What an [`ErasedNative`] runs: `None` fails the call as a native without a binding does.
pub type ErasedNativeFn = Box<dyn Fn(&NativeCall<'_>) -> Option<Variable> + Send + Sync>;

/// A native whose Rust side is chosen at run time, for a host that binds on behalf of code it may
/// unload: the VM keeps the host's function object while [`set`](Self::set) decides what runs behind it.
#[derive(Clone, Copy)]
pub struct ErasedNative(&'static Native<RwLock<ErasedNativeFn>>);

unsafe impl Send for ErasedNative {}
unsafe impl Sync for ErasedNative {}

impl ErasedNative {
    /// Replaces the handler; returns once no call is inside the previous one.
    pub fn set(&self, f: ErasedNativeFn) {
        *self.0.f.write().unwrap_or_else(PoisonError::into_inner) = f;
    }

    #[must_use]
    pub fn function(&self) -> &'static NativeFunction {
        self.0.function()
    }
}

/// How a native is declared to the VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeKind {
    /// `native global`.
    Global,
    /// `native` on a script that extends a form class.
    Method,
    /// `native global`, completing later through a [`Latent`].
    Latent,
}

/// Proof of running on a game thread, which Papyrus dispatch requires.
pub struct GameThread(());

impl GameThread {
    /// `Some` while a native registered through this module runs on the current thread.
    #[must_use]
    pub fn current() -> Option<Self> {
        STACK_ID.get().map(|_| Self(()))
    }

    /// # Safety
    /// The caller must be on a game thread: an SKSE message handler or task, an event sink the
    /// engine notifies from its main thread, or a hook in engine code.
    #[must_use]
    pub unsafe fn assume() -> Self {
        Self(())
    }
}

/// The stack id of the native call running on this thread, the value `ReturnFromLatent` takes.
#[must_use]
pub fn current_stack_id() -> Option<u32> {
    STACK_ID.get()
}

#[repr(C)]
struct ArgsVtable {
    dtor: unsafe extern "C" fn(this: *mut Args, flags: u32) -> *mut Args,
    supply: unsafe extern "C" fn(this: *mut Args, out: *mut c_void) -> bool,
}

/// What a dispatch's callback does with the result: called once, on the VM thread that completed
/// the call, with a value valid only during the call.
pub type ResultFn = Box<dyn FnOnce(&Variable) + Send>;

#[repr(C)]
struct CallbackVtable {
    dtor: unsafe extern "C" fn(this: *mut Callback, flags: u32) -> *mut Callback,
    call: unsafe extern "C" fn(this: *mut Callback, result: *mut VariableRaw),
    keep: unsafe extern "C" fn(this: *mut Callback) -> bool,
    set_object: unsafe extern "C" fn(this: *mut Callback, object: *mut c_void),
}

/// An `IStackCallbackFunctor`: the engine's 0x10-byte header (vtable, reference count at +8), then
/// the closure. The engine holds it through a `BSTSmartPointer` (locked add and subtract on the
/// count) and calls slot 0 with 1 when its count reaches zero (RE request 4c).
#[repr(C)]
struct Callback {
    header: sre::BSScript__IStackCallbackFunctor,
    f: Option<ResultFn>,
}

impl Callback {
    const VTABLE: &'static CallbackVtable = &CallbackVtable { dtor: Self::dtor, call: Self::call, keep: Self::keep, set_object: Self::set_object };

    /// A callback holding the one reference the dispatch call borrows.
    fn new(f: ResultFn) -> *mut Self {
        Box::into_raw(Box::new(Self {
            header: sre::BSScript__IStackCallbackFunctor { __vftable: core::ptr::from_ref(Self::VTABLE).cast(), refCount: 1, unknown_C: [0; 4] },
            f: Some(f),
        }))
    }

    fn count(this: *mut Self) -> &'static core::sync::atomic::AtomicU32 {
        unsafe { core::sync::atomic::AtomicU32::from_ptr(&raw mut (*this).header.refCount) }
    }

    /// Drops the reference [`Callback::new`] gave the caller.
    fn release(this: *mut Self) {
        if Self::count(this).fetch_sub(1, core::sync::atomic::Ordering::AcqRel) == 1 {
            unsafe { Self::dtor(this, 1) };
        }
    }

    unsafe extern "C" fn dtor(this: *mut Self, flags: u32) -> *mut Self {
        if flags & 1 != 0 {
            drop(unsafe { Box::from_raw(this) });
        }
        this
    }

    unsafe extern "C" fn call(this: *mut Self, result: *mut VariableRaw) {
        let none = Variable::none();
        let value = unsafe { Variable::from_ptr(result) }.unwrap_or(&none);
        if let Some(f) = unsafe { (*this).f.take() } {
            f(value);
        }
    }

    unsafe extern "C" fn keep(_: *mut Self) -> bool {
        false
    }

    unsafe extern "C" fn set_object(_: *mut Self, _: *mut c_void) {}
}

/// An `IFunctionArguments` over a slice of Variables: its slot 1 sizes the engine's
/// `BSScrapArray<Variable>` (data +8) with `ResizeVariableArray` (id 53915) and assigns each value.
#[repr(C)]
struct Args {
    vtable: &'static ArgsVtable,
    values: *const Variable,
    len: usize,
}

impl Args {
    const VTABLE: &'static ArgsVtable = &ArgsVtable { dtor: Self::dtor, supply: Self::supply };

    fn new(values: &[Variable]) -> Self {
        Self { vtable: Self::VTABLE, values: values.as_ptr(), len: values.len() }
    }

    unsafe extern "C" fn dtor(this: *mut Self, _: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn supply(this: *mut Self, out: *mut c_void) -> bool {
        let this = unsafe { &*this };
        if unsafe { (fns().resize_args)(out, this.len as u64) } == 0 {
            return false;
        }
        let data = unsafe { (*out.cast::<sre::BSScrapArray>()).data.cast::<VariableRaw>() };
        for i in 0..this.len {
            unsafe { (fns().assign)(data.add(i), &raw const (*this.values.add(i)).0) };
        }
        true
    }
}

/// `BSScript::Internal::VirtualMachine`, the object `g_SkyrimVM + 0x210` points at and the SKSE
/// `PapyrusInterface` hands to a registration callback.
#[repr(transparent)]
pub struct VirtualMachine(VmRaw);

impl VirtualMachine {
    /// The VM through `g_SkyrimVM` (id 403618) + 0x210, `None` before `SkyrimVM::ctor` stored it.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        let holder = (fns().skyrim_vm as usize + core::mem::offset_of!(sre::SkyrimVM, field_210)) as *const *const Self;
        unsafe { (*holder).as_ref() }
    }

    /// The registry pointer of an SKSE `PapyrusInterface` callback as the VM, after checking that
    /// its vtable is `BSScript::Internal::VirtualMachine`'s.
    ///
    /// # Safety
    /// `p` must be null or point to a live object with a vtable.
    #[must_use]
    pub unsafe fn from_ptr(p: *mut c_void) -> Option<&'static Self> {
        if p.is_null() {
            return None;
        }
        let class = unsafe { crate::class_of(p) }?;
        (class.id == vtables::BSScript__Internal__VirtualMachine_vtable::ID).then(|| unsafe { &*p.cast::<Self>() })
    }

    #[must_use]
    pub fn raw(&self) -> &VmRaw {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *mut VmRaw {
        (&raw const self.0).cast_mut()
    }

    fn slot<F: Copy>(&self, index: usize) -> F {
        unsafe { vslot(self.as_ptr().cast(), index) }
    }

    /// Slot 45 `GetObjectHandlePolicy`.
    #[must_use]
    pub fn handle_policy(&self) -> &'static HandlePolicy {
        let p = unsafe { self.slot::<GetObjectHandlePolicy>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_GetObjectHandlePolicy)(self.as_ptr()) };
        unsafe { &*p.cast::<HandlePolicy>() }
    }

    /// Slot 13 `GetTypeIDForScriptObject`: the type id a script class was registered with, `None`
    /// for a class the engine did not register (every mod script).
    #[must_use]
    pub fn type_id(&self, class: &str) -> Option<u32> {
        let name = BSFixedString::new(class);
        let mut id = 0;
        let found = unsafe { self.slot::<GetTypeIDForScriptObject>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_GetTypeIDForScriptObject)(self.as_ptr(), name.raw(), &raw mut id) };
        (found != 0).then_some(id)
    }

    /// Slot 9 `GetScriptObjectType`: the class by name, loading and linking `Data\Scripts\<class>.pex`
    /// when it is missing or unlinked; `None` when no script exists.
    #[must_use]
    pub fn script_type(&self, class: &str) -> Option<&'static ObjectTypeInfo> {
        let name = BSFixedString::new(class);
        let mut out = core::ptr::null_mut();
        let found = unsafe { self.slot::<GetScriptObjectType>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_GetScriptObjectType)(self.as_ptr(), name.raw(), &raw mut out) };
        if found == 0 {
            return None;
        }
        unsafe { out.cast::<ObjectTypeInfo>().as_ref() }
    }

    /// Slot 10 `GetScriptObjectTypeByID` (medium in `papyrus.json`; the lookup the engine's own
    /// native constructors use to type form parameters).
    #[must_use]
    pub fn script_type_by_id(&self, id: VMTypeID) -> Option<&'static ObjectTypeInfo> {
        let mut out = core::ptr::null_mut();
        let found = unsafe { self.slot::<GetScriptObjectTypeByID>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_GetScriptObjectTypeByID)(self.as_ptr(), id as u32, &raw mut out) };
        if found == 0 {
            return None;
        }
        unsafe { out.cast::<ObjectTypeInfo>().as_ref() }
    }

    /// Slot 28 `FindBoundObject`: the script object of class `class` bound to `handle`, `None`
    /// when none is (no script of that class is attached to the form).
    #[must_use]
    pub fn find_bound_object(&self, handle: Handle, class: &str) -> Option<ObjectPtr> {
        let class = CString::new(class).ok()?;
        let mut out = core::ptr::null_mut();
        let found = unsafe { self.slot::<FindBoundObject>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_FindBoundObject)(self.as_ptr(), handle.0, class.as_ptr(), &raw mut out) };
        if found == 0 {
            return None;
        }
        NonNull::new(out.cast::<Object>()).map(ObjectPtr)
    }

    /// Slot 26 `SetCallableFromTasklets`: lets the tasklet VM run the native without a context
    /// switch, what the engine sets for most of its own natives after binding them.
    pub fn set_callable_from_tasklets(&self, class: &str, name: &str, callable: bool) {
        let (Ok(class), Ok(name)) = (CString::new(class), CString::new(name)) else { return };
        unsafe { self.slot::<SetCallableFromTasklets>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_SetCallableFromTasklets)(self.as_ptr(), class.as_ptr(), name.as_ptr(), callable) };
    }

    /// Registers `f` as `class.name`, a `native global` function. The `.pex` must declare it with
    /// the same parameter and return types ([`ParamType`] per Rust type) or the engine refuses the
    /// binding when the script links.
    pub fn register_native<F: NativeFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.bind(class, name, NativeKind::Global, F::PARAMS, F::RETURN, invoke_fn::<F, M>, f).map(Native::function)
    }

    /// Registers `f` as `class.name`, a latent `native global` function: the calling script waits
    /// until `f`'s [`Latent`] is completed. The `.pex` declares it like any native, returning the
    /// type of `F::Output`.
    pub fn register_latent<F: LatentFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.bind(class, name, NativeKind::Latent, F::PARAMS, <F::Output as IntoVariable>::TYPE, invoke_latent::<F, M>, f).map(Native::function)
    }

    /// Registers `f` as `class.name`, a `native` method of a script that extends the form class
    /// of `f`'s first parameter; a call whose object is not bound to such a form fails with the
    /// engine's "no native object bound" error.
    pub fn register_method<F: NativeMethod<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        self.bind(class, name, NativeKind::Method, F::PARAMS, F::RETURN, invoke_method::<F, M>, f).map(Native::function)
    }

    /// Registers `class.name` with the declared shape and a handler that can be replaced later
    /// ([`ErasedNative::set`]); `params` and `ret` are what the typed registrations take from
    /// their `F` ([`NativeFn::PARAMS`] and [`NativeFn::RETURN`], a latent's `Output` type).
    pub fn register_erased(&self, class: &str, name: &str, kind: NativeKind, params: &[ParamType], ret: Option<ParamType>, f: ErasedNativeFn) -> Result<ErasedNative, Error> {
        self.bind(class, name, kind, params, ret, invoke_erased, RwLock::new(f)).map(ErasedNative)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind<F: Send + Sync + 'static>(
        &self,
        class: &str,
        name: &str,
        kind: NativeKind,
        params: &[ParamType],
        ret: Option<ParamType>,
        invoke: unsafe fn(*const NativeHeader, &NativeCall<'_>) -> Option<Variable>,
        f: F,
    ) -> Result<&'static Native<F>, Error> {
        let return_type = ret.map_or(Ok(TypeInfo::NONE), |t| t.type_info(self))?;
        let count = u16::try_from(params.len()).expect("at most MAX_PARAMS parameters");
        let mut param_infos = Vec::with_capacity(1 + 2 * params.len());
        param_infos.push(params.len() as u64);
        for (i, param) in params.iter().enumerate() {
            param_infos.push(BSFixedString::new(&format!("a{i}")).into_raw().data as u64);
            param_infos.push(param.type_info(self)?.bits());
        }
        let param_infos = Box::leak(param_infos.into_boxed_slice());
        let native = Box::leak(Box::new(Native {
            header: NativeHeader {
                base: sre::BSScript__NF_util__NativeFunctionBase {
                    __vftable: (&raw const fns().vtable).cast(),
                    refCount: 1,
                    unknown_C: [0; 4],
                    name: BSFixedString::new(name).into_raw(),
                    objectTypeName: BSFixedString::new(class).into_raw(),
                    stateName: BSFixedString::new("").into_raw(),
                    returnType: sre::BSScript__TypeInfo { rawTypeOrObjectType: return_type.bits() },
                    params: param_infos[1..].as_mut_ptr().cast(),
                    paramCount: count,
                    variableCount: count,
                    unknown_3C: [0; 4],
                    isStatic: u8::from(kind != NativeKind::Method),
                    isCallableFromTasklet: 0,
                    isLatent: u8::from(kind == NativeKind::Latent),
                    unknown_43: [0; 1],
                    userFlags: 0,
                    docString: BSFixedString::empty().into_raw(),
                    binding: invoke as *mut c_void,
                },
                invoke,
            },
            f,
        }));
        let header: *mut NativeHeader = &raw mut native.header;
        let bound = unsafe { self.slot::<BindNativeMethod>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_BindNativeMethod)(self.as_ptr(), header.cast()) };
        if bound == 0 {
            return Err(Error::Bind { class: class.to_owned(), name: name.to_owned() });
        }
        Ok(native)
    }

    /// Slot 38 `DispatchStaticCall` of `class.function(args)`: the call is queued on a new stack and
    /// `on_result` runs with its result when it completes, on the VM thread that ran it (at once with
    /// None for an empty function). Returns what the slot returned. The class's script must be
    /// loadable (`script_type` semantics apply).
    #[must_use]
    pub fn dispatch_static_call(&self, _: &GameThread, class: &str, function: &str, args: &[Variable], on_result: ResultFn) -> bool {
        let class = BSFixedString::new(class);
        let function = BSFixedString::new(function);
        let mut args = Args::new(args);
        let mut callback = Callback::new(on_result);
        let queued = unsafe {
            self.slot::<DispatchStaticCall>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_DispatchStaticCall)(self.as_ptr(), class.raw(), function.raw(), (&raw mut args).cast(), (&raw mut callback).cast()) != 0
        };
        Callback::release(callback);
        queued
    }

    /// Slot 39 `DispatchMethodCall` of `function(args)` on `object`, as
    /// [`dispatch_static_call`](Self::dispatch_static_call).
    #[must_use]
    pub fn dispatch_method_call(&self, _: &GameThread, object: &ObjectPtr, function: &str, args: &[Variable], on_result: ResultFn) -> bool {
        let function = BSFixedString::new(function);
        let mut args = Args::new(args);
        let mut target = object.0.as_ptr().cast::<sre::BSScript__Object>();
        let mut callback = Callback::new(on_result);
        let queued = unsafe {
            self.slot::<DispatchMethodCall>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_DispatchMethodCall)(self.as_ptr(), &raw mut target, function.raw(), (&raw mut args).cast(), (&raw mut callback).cast()) != 0
        };
        Callback::release(callback);
        queued
    }

    /// Slot 22 `CreateArray`: a new array of `len` elements of type `element`, each `{element, 0}`,
    /// as an array Variable holding the creator's reference (the VM's tracking list holds another
    /// until its next update). It allocates from the global heap, not the per-thread scrap heap.
    #[must_use]
    pub fn create_array(&self, element: TypeInfo, len: u32) -> Option<Variable> {
        let mut out = core::ptr::null_mut();
        let ty = sre::BSScript__TypeInfo { rawTypeOrObjectType: element.bits() };
        let created = unsafe { self.slot::<CreateArray>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_CreateArray)(self.as_ptr(), &raw const ty, len, &raw mut out) };
        if created == 0 || out.is_null() {
            return None;
        }
        Some(Variable(VariableRaw { type_: sre::BSScript__TypeInfo { rawTypeOrObjectType: element.array_of().bits() }, value: out as u64 }))
    }

    /// Slot 36 `SendEvent`: calls `event(args)` on every script object bound to exactly `handle`
    /// whose script declares that event, each as a new stack. The arguments are read during the call,
    /// once per receiving object. A reference's aliases and an actor's active effects have handles of
    /// their own; the engine reaches them through `SkyrimVM::RelayEventToRefAndEffects` after this.
    pub fn send_event(&self, _: &GameThread, handle: Handle, event: &str, args: &[Variable]) {
        let event = BSFixedString::new(event);
        let mut args = Args::new(args);
        unsafe { self.slot::<SendEvent>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_SendEvent)(self.as_ptr(), handle.0, event.raw(), (&raw mut args).cast()) };
    }

    /// `SkyrimVM::RelayEventToRefAndEffects` (id 54033): sends `event(args)` to every alias the
    /// reference behind `reference` fills and, for an actor, to every active effect with a handle,
    /// each through `SendEvent`; not to the reference itself. The engine sends to the reference first
    /// with [`send_event`](Self::send_event), then relays, and a mod reaches what the engine reaches by
    /// doing the same.
    pub fn relay_event(&self, _: &GameThread, reference: Handle, event: &str, args: &[Variable]) {
        let event = BSFixedString::new(event);
        let mut args = Args::new(args);
        unsafe { (fns().relay)(fns().skyrim_vm.cast_mut(), reference.0, event.raw(), (&raw mut args).cast(), core::ptr::null_mut()) };
    }

    /// Slot 21 `CreateObject`: a new, unbound script object of `class` (its `.pex` must load).
    #[must_use]
    pub fn create_object(&self, class: &str) -> Option<ObjectPtr> {
        let class = BSFixedString::new(class);
        let mut out = core::ptr::null_mut();
        let created = unsafe { self.slot::<CreateObject>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_CreateObject)(self.as_ptr(), class.raw(), &raw mut out) };
        if created == 0 {
            return None;
        }
        NonNull::new(out.cast::<Object>()).map(ObjectPtr)
    }

    /// Binds `object` to the form `handle` names through the VM's bind policy (slot 47,
    /// `ObjectBindPolicy::BindObject` id 104184): from then on [`find_bound_object`](Self::find_bound_object)
    /// returns it and calls with that form as `self` run its methods. Call it on a game thread.
    pub fn bind_object(&self, _: &GameThread, object: &ObjectPtr, handle: Handle) {
        let policy = unsafe { self.slot::<GetObjectBindPolicy>(vtables::BSScript__Internal__VirtualMachine_vtable::SLOT_GetObjectBindPolicy)(self.as_ptr()) };
        let mut target = object.0.as_ptr().cast::<sre::BSScript__Object>();
        unsafe { (fns().bind_object)(policy, &raw mut target, handle.0) };
    }
}
