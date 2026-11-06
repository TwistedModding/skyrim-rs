// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Papyrus (subsystem pass 4): Rust functions registered as the natives a `.pex` declares, typed
//! script values ([`Variable`], [`TypeInfo`]), script object handles that release ([`ObjectPtr`]),
//! and calls into Papyrus with a closure for the result, as [`VirtualMachineExt`] methods on the
//! generated `BSScript::Internal::VirtualMachine` behind `g_SkyrimVM`.
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
//! [`VirtualMachineExt::call_static`] therefore takes a [`GameThread`] token, which exists inside a
//! native callback ([`GameThread::current`]) and is asserted with [`GameThread::assume`] from an
//! SKSE message handler, task or event sink. A call takes typed arguments (`IFunctionArguments`
//! filling the engine's `BSScrapArray<Variable>`) and a closure the engine calls with the result (an
//! `IStackCallbackFunctor` of this module's own) once the call completes, on the VM thread that
//! completed it (not the main thread), so the closure synchronizes what it touches. A script object
//! of a mod's own class is created with [`VirtualMachineExt::new_object`] and bound to a form with
//! [`VirtualMachineExt::bind_to_handle`], after which the engine and `DevBench` call its methods,
//! natives included, like any attached script's.
//!
//! Request 4e added the rest. Arrays are `BSScript::Array`s (a 0x20 header with the element type and
//! size, the Variables inline) that `CreateArray` (slot 22) makes on the global heap; `Vec<T>` is a
//! native parameter or return of type `T[]`. A latent native ([`VirtualMachineExt::register_latent`])
//! receives a [`Latent`] and returns at once; the calling script waits until [`Latent::complete`]
//! hands `ReturnFromLatent` (slot 43) the value, from inside the native or later from any thread.
//! [`VirtualMachineExt::send_script_event`] is `SendEvent` (slot 36), the call behind every engine
//! event a script receives, and [`VirtualMachineExt::relay_script_event`] the engine's second half
//! of an event, which reaches the aliases a reference fills and an actor's scripted active effects
//! (request 4f; [`alias_handles`] and [`active_effect_handles`] name them). The VM's own work (its
//! update, the stacks, the completions) runs on rotating worker threads, never on the main thread.

use core::cell::Cell;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::ffi::CString;
use std::fmt;
use std::sync::{OnceLock, PoisonError, RwLock};

use sre::raw::fns;
use sre::raw::vtables::BSScript__NF_util__NativeFunctionBase_vtable as NativeVtable;
use sre::{Actor, PapyrusType, TESForm, TESObjectREFR, VMTypeID};

use crate::BSFixedString;
use crate::forms::{FormClass, TESFormExt};

/// The VM the SKSE `PapyrusInterface` hands a registration callback and `g_SkyrimVM + 0x210` holds.
pub type VirtualMachine = sre::BSScript__Internal__VirtualMachine;

type VariableRaw = sre::BSScript__Variable;
type ObjectTypeInfo = sre::BSScript__ObjectTypeInfo;

type CallBindingSlot = unsafe extern "C" fn(this: *mut NativeHeader, self_: *mut VariableRaw, vm: *mut VirtualMachine, stack_id: u32, result: *mut VariableRaw, frame: *mut sre::BSScript__StackFrame) -> bool;
type HasBindingSlot = unsafe extern "C" fn(this: *mut NativeHeader) -> bool;

fn type_info(bits: u64) -> sre::BSScript__TypeInfo {
    let mut info: sre::BSScript__TypeInfo = unsafe { core::mem::zeroed() };
    info.rawTypeOrObjectType = bits;
    info
}

fn variable(type_bits: u64, value: u64) -> VariableRaw {
    let mut raw: VariableRaw = unsafe { core::mem::zeroed() };
    raw.type_ = type_info(type_bits);
    raw.value = value;
    raw
}

/// `Variable::assign` (id 104297): releases what `target` held and takes a reference to `value`'s.
unsafe fn assign(target: *mut VariableRaw, value: &VariableRaw) {
    let f: fns::BSScript__Variable__assign_fn = unsafe { crate::engine_fn(sre::id!(fns::BSScript__Variable__assign)) };
    unsafe { f(target, value) }
}

/// `ExtraAliasInstanceArray`'s extra data type.
const ALIAS_INSTANCES: u32 = 0x88;
const IFUNCTION_SLOTS: usize = 23;
/// Natives take at most this many parameters, the arity the [`NativeFn`] impls cover.
pub const MAX_PARAMS: usize = 4;

/// The `IFunction` vtable of every native: the engine's `NativeFunctionBase` slots, then `HasBinding`
/// and `CallBinding` of this module.
struct Vtable([*const c_void; IFUNCTION_SLOTS]);

unsafe impl Send for Vtable {}
unsafe impl Sync for Vtable {}

fn native_vtable() -> &'static Vtable {
    static VTABLE: OnceLock<Vtable> = OnceLock::new();
    VTABLE.get_or_init(|| {
        sre::fixed_slots!("BSScript::NF_util::NativeFunctionBase", 0x0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22);
        let base = crate::reloc().address(sre::id!(NativeVtable::ID)).expect("NativeFunctionBase vtable id") as *const *const c_void;
        let mut slots = [core::ptr::null(); IFUNCTION_SLOTS];
        for (i, slot) in slots.iter_mut().enumerate().take(NativeVtable::SLOT_HasBinding) {
            *slot = unsafe { *base.add(i) };
        }
        slots[NativeVtable::SLOT_HasBinding] = has_binding as HasBindingSlot as *const c_void;
        slots[NativeVtable::SLOT_CallBinding] = call_binding as CallBindingSlot as *const c_void;
        Vtable(slots)
    })
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
    pub const fn form(form_id: u32) -> Self {
        Self(form_id as u64 | 0xFFFF << 32)
    }

    /// A quest alias's handle: the quest's form id and the alias id (request 4f).
    #[must_use]
    pub const fn alias(quest: u32, alias_id: u16) -> Self {
        Self(quest as u64 | (alias_id as u64) << 32)
    }

    /// An active effect's handle: the target actor's form id and the effect's unique id (request 4f).
    #[must_use]
    pub const fn active_effect(target: u32, unique_id: u16) -> Self {
        Self(target as u64 | (unique_id as u64) << 32 | 2 << 48)
    }

    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.0 == Self::INVALID.0
    }

    /// The form id of a form handle, `None` for the invalid handle and the other encodings.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn form_id(self) -> Option<u32> {
        let low = self.0 as u32;
        if self.0 >> 32 == 0xFFFF && low != 0 { Some(low) } else { None }
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

/// The handles of the quest aliases `reference` fills, from its `ExtraAliasInstanceArray` read under
/// the array's lock. A reference whose aliases live on another reference (through its
/// `ExtraReferenceHandle`, which the engine's relay follows) answers its own array only.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn alias_handles(reference: &TESObjectREFR) -> Vec<Handle> {
    let Some(extra) = reference.extraList.get_by_type(ALIAS_INSTANCES) else { return Vec::new() };
    let array = unsafe { &*core::ptr::from_ref(extra).cast::<sre::ExtraAliasInstanceArray>() };
    array.lock.lock_for_read();
    let entries = array.entries.cast::<*const sre::ExtraAliasInstanceArray__Entry>();
    let handles = (0..array.count as usize)
        .filter_map(|i| {
            let entry = unsafe { (*entries.add(i)).as_ref() }?;
            Some(Handle::alias(entry.quest()?.formID, entry.alias()?.aliasID as u16))
        })
        .collect();
    array.lock.unlock_for_read();
    handles
}

/// `actor`'s active effects, walked as `MagicTarget::HasActiveEffectWithBase` walks them: the list
/// `MagicTarget` slot 7 returns (empty for an actor without middle-high process data), up to a null
/// node or one whose effect and next are both null, null effects skipped (RE `camera.md` request
/// IC3 G). The list changes on the main thread, where it is read.
#[must_use]
pub fn active_effects(actor: &Actor) -> Vec<&'static sre::ActiveEffect> {
    let mut node = unsafe { actor.get_active_effect_list() }.cast_const().cast::<sre::BSSimpleListNode>();
    let mut effects = Vec::new();
    while let Some(n) = unsafe { node.as_ref() }.filter(|n| !n.item.is_null() || !n.next.is_null()) {
        effects.extend(unsafe { n.item.cast::<sre::ActiveEffect>().as_ref() });
        node = n.next.cast();
    }
    effects
}

/// The handles of `actor`'s active effects that have one: an effect gets a unique id, and so a
/// handle, only when its `EffectSetting` is scripted (form flag bit 22).
#[must_use]
pub fn active_effect_handles(actor: &Actor) -> Vec<Handle> {
    active_effects(actor).into_iter().filter(|e| e.uniqueID != 0).map(|e| Handle::active_effect(actor.formID, e.uniqueID)).collect()
}

/// `BSScript::TypeInfo`: a [`PapyrusType`] below 0x10, else an `ObjectTypeInfo` pointer with bit 0
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
        Self(core::ptr::from_ref(ty) as u64)
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
            (None, Some(class)) => write!(f, "{}{}", BSFixedString::view(&class.name), if self.is_array() { "[]" } else { "" }),
            (None, None) => write!(f, "TypeInfo({:#x})", self.0),
        }
    }
}

/// The `IFunction` `class` binds under `name` through `ObjectTypeInfo::GetFunction` (id 104311): on
/// a linked type any function of the tables, on an unlinked one only the non-static natives bound
/// so far. The pointer is the native object a registration returned when that native is bound.
/// The reference the lookup adds is kept, as every registered native holds its classes for the
/// life of the game anyway.
#[must_use]
pub fn function_of(class: &ObjectTypeInfo, name: &str, is_static: bool) -> Option<NonNull<c_void>> {
    let name = BSFixedString::new(name);
    let mut out = core::ptr::null_mut::<c_void>();
    if !unsafe { class.get_function(name.raw(), (&raw mut out).cast(), is_static) } {
        return None;
    }
    NonNull::new(out)
}

/// A script object held by one reference, taken by [`VirtualMachineExt::bound_object`] or
/// [`VirtualMachineExt::new_object`] and dropped through `Object::DecRef` (id 104253). The VM's own
/// table keeps a bound object alive, so the count never reaches zero here.
pub struct ObjectPtr(NonNull<sre::BSScript__Object>);

impl core::ops::Deref for ObjectPtr {
    type Target = sre::BSScript__Object;

    fn deref(&self) -> &sre::BSScript__Object {
        unsafe { self.0.as_ref() }
    }
}

impl Drop for ObjectPtr {
    fn drop(&mut self) {
        self.dec_ref();
    }
}

/// `BSScript::Variable`: a type tag and an 8-byte payload. Owned values release their payload
/// (string, object) through `Variable::reset` on drop; views into engine memory come from
/// [`view`](Self::view).
#[repr(transparent)]
pub struct Variable(VariableRaw);

impl Variable {
    #[must_use]
    pub fn none() -> Self {
        Self::scalar(PapyrusType::None, 0)
    }

    fn scalar(ty: PapyrusType, value: u64) -> Self {
        Self(variable(ty as u64, value))
    }

    #[must_use]
    pub fn int(value: i32) -> Self {
        Self::scalar(PapyrusType::Int, u64::from(value.cast_unsigned()))
    }

    #[must_use]
    pub fn float(value: f32) -> Self {
        Self::scalar(PapyrusType::Float, u64::from(value.to_bits()))
    }

    #[must_use]
    pub fn bool(value: bool) -> Self {
        Self::scalar(PapyrusType::Bool, u64::from(value))
    }

    /// A String holding its own reference to the interned text.
    #[must_use]
    pub fn string(text: &BSFixedString) -> Self {
        Self::scalar(PapyrusType::String, text.clone().into_raw().data as u64)
    }

    /// The object Variable for a form, what the engine's form-returning natives build: the form's
    /// own script class, its handle, the bound object or a new one; None for a null form or a form
    /// type with no script class.
    #[must_use]
    pub fn form(form: Option<&TESForm>, vm: &VirtualMachine) -> Self {
        let Some(form) = form else { return Self::none() };
        let Some(class) = VMTypeID::from_u32(u32::from(form.formType)).and_then(|id| vm.script_type_by_id(id)) else { return Self::none() };
        let name = BSFixedString::view(&class.name).to_string();
        let handle = vm.handle_for_form(form);
        let object = vm.bound_object(handle, &name).or_else(|| {
            let object = vm.new_object(&name)?;
            vm.bind_to_handle(&GameThread(()), &object, handle);
            Some(object)
        });
        let Some(object) = object else { return Self::none() };
        let mut out = Self::none();
        unsafe { assign(&raw mut out.0, &variable(core::ptr::from_ref(class) as u64, object.0.as_ptr() as u64)) };
        out
    }

    /// A `Variable` the engine owns, viewed without taking its payload.
    #[must_use]
    pub fn view(raw: &VariableRaw) -> &Self {
        unsafe { &*core::ptr::from_ref(raw).cast::<Self>() }
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
        self.is(PapyrusType::String).then(|| unsafe { &*(&raw const self.0.value).cast::<BSFixedString>() })
    }

    /// The object of an object-typed Variable, `None` for other types and for a None object.
    #[must_use]
    pub fn as_object(&self) -> Option<&sre::BSScript__Object> {
        let ty = self.type_info();
        if ty.object_type().is_none() || ty.is_array() {
            return None;
        }
        unsafe { (self.0.value as *const sre::BSScript__Object).as_ref() }
    }

    /// The array of an array-typed Variable, `None` for other types and for a None array.
    #[must_use]
    pub fn as_array(&self) -> Option<&sre::BSScript__Array> {
        if !self.type_info().is_array() {
            return None;
        }
        unsafe { (self.0.value as *const sre::BSScript__Array).as_ref() }
    }

    /// The elements of an array-typed Variable, inline after the array's 0x20-byte header.
    #[must_use]
    pub fn elements(&self) -> Option<&[Variable]> {
        let array = self.as_array()?;
        sre::fixed_size!("BSScript::Variable", VariableRaw);
        Some(unsafe { core::slice::from_raw_parts(core::ptr::from_ref(array).byte_add(sre::class_size!("BSScript::Array", sre::BSScript__Array)).cast::<Variable>(), array.size as usize) })
    }

    /// Copies `value` into element `index` of an array-typed Variable through `Variable::assign`;
    /// false past the end or for another type.
    #[must_use]
    pub fn set_element(&self, index: usize, value: &Variable) -> bool {
        self.elements().and_then(|e| e.get(index)).is_some_and(|slot| {
            unsafe { assign(core::ptr::from_ref(&slot.0).cast_mut(), &value.0) };
            true
        })
    }

    /// The form behind an object Variable, resolved through the handle policy with `T`'s type id.
    #[must_use]
    pub fn as_form<T: FormClass>(&self) -> Option<&'static T> {
        let handle = Handle(self.as_object()?.get_handle());
        VirtualMachine::singleton()?.form_for_handle(T::TYPE_ID, handle)?.cast::<T>()
    }
}

impl Drop for Variable {
    fn drop(&mut self) {
        let reset: fns::BSScript__Variable__reset_fn = unsafe { crate::engine_fn(sre::id!(fns::BSScript__Variable__reset)) };
        unsafe { reset(&raw mut self.0) };
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
            _ => match (self.as_object(), self.elements()) {
                (Some(object), _) => {
                    let class = object.type_().map_or_else(|| "?".to_owned(), |t| BSFixedString::view(&t.name).to_string());
                    write!(f, "{class} object {} (refs {})", Handle(object.get_handle()), object.refCountAndLock & 0x7FFF_FFFF)
                }
                (None, Some(elements)) => write!(f, "{:?} {elements:?}", self.type_info()),
                (None, None) => write!(f, "{:?} {:#x}", self.type_info(), self.0.value),
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
        v.elements().map(|e| e.iter().map(T::from_variable).collect()).unwrap_or_default()
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

/// A new array through [`VirtualMachineExt::new_array`]; elements that convert to None keep the
/// array's initial `{element type, 0}`, which is how the engine holds a None form in a `Form[]`.
impl<T: IntoVariable> IntoVariable for Vec<T> {
    const TYPE: Option<ParamType> = match T::TYPE {
        Some(t) => Some(t.array()),
        None => panic!("a Papyrus array needs an element type"),
    };
    fn into_variable(self, vm: &VirtualMachine) -> Variable {
        let Some(Ok(element)) = T::TYPE.map(|t| t.type_info(vm)) else { return Variable::none() };
        let Some(array) = u32::try_from(self.len()).ok().and_then(|len| vm.new_array(element, len)) else { return Variable::none() };
        for (i, value) in self.into_iter().enumerate() {
            let value = value.into_variable(vm);
            if value.type_info() != TypeInfo::NONE {
                let _ = array.set_element(i, &value);
            }
        }
        array
    }
}

/// A started latent call ([`VirtualMachineExt::register_latent`]): complete it once, from the
/// native or later from any thread, with the value the waiting script receives.
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
        let Some(vm) = VirtualMachine::singleton() else { return };
        let value = value.into_variable(vm);
        vm.return_from_latent(self.stack_id, &value.0);
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
        impl<F, R, $($a),*> LatentFn<(Latent<R>, $($a,)*)> for F
        where
            F: Fn(Latent<R>, $($a),*) + Copy + Send + Sync + 'static,
            R: IntoVariable,
            $($a: FromVariable,)*
        {
            type Output = R;
            const PARAMS: &'static [ParamType] = &[$($a::TYPE),*];
            #[allow(unused_variables)]
            fn start(&self, latent: Latent<R>, args: &[&Variable]) {
                self(latent, $($a::from_variable(args[$i])),*);
            }
        }

        impl<F, R, $($a),*> NativeFn<(R, $($a,)*)> for F
        where
            F: Fn($($a),*) -> R + Copy + Send + Sync + 'static,
            R: IntoVariable,
            $($a: FromVariable,)*
        {
            const PARAMS: &'static [ParamType] = &[$($a::TYPE),*];
            const RETURN: Option<ParamType> = R::TYPE;
            #[allow(unused_variables)]
            fn invoke(&self, vm: &VirtualMachine, args: &[&Variable]) -> Variable {
                self($($a::from_variable(args[$i])),*).into_variable(vm)
            }
        }

        impl<F, S, R, $($a),*> NativeMethod<(S, R, $($a,)*)> for F
        where
            F: for<'s> Fn(&'s S, $($a),*) -> R + Copy + Send + Sync + 'static,
            S: FormClass,
            R: IntoVariable,
            $($a: FromVariable,)*
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

    /// The object the VM holds, for comparison with [`function_of`].
    #[must_use]
    pub fn as_ptr(&self) -> NonNull<c_void> {
        NonNull::from(&self.0).cast()
    }

    #[must_use]
    pub fn name(&self) -> &BSFixedString {
        BSFixedString::view(&self.0.base.name)
    }

    #[must_use]
    pub fn class_name(&self) -> &BSFixedString {
        BSFixedString::view(&self.0.base.objectTypeName)
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
        sre::fixed_size!("BSScript::NF_util::ParamInfo", sre::BSScript__NF_util__ParamInfo);
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
    /// Runs `f` as a global function ([`VirtualMachineExt::register_native`]).
    #[must_use]
    pub fn global<F: NativeFn<M>, M>(&self, f: F) -> Option<Variable> {
        self.on_stack(|| Some(f.invoke(self.vm, self.args)))
    }

    /// Runs `f` as a method ([`VirtualMachineExt::register_method`]); `None` when `self` is not bound to its form class.
    #[must_use]
    pub fn method<F: NativeMethod<M>, M>(&self, f: F) -> Option<Variable> {
        self.on_stack(|| Some(f.invoke(self.vm, self.self_.as_form::<F::Receiver>()?, self.args)))
    }

    /// Starts `f` as a latent function ([`VirtualMachineExt::register_latent`]) and answers the true Bool `Call` requires before the stack waits.
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

unsafe extern "C" fn call_binding(this: *mut NativeHeader, self_: *mut VariableRaw, vm: *mut VirtualMachine, stack_id: u32, result: *mut VariableRaw, frame: *mut sre::BSScript__StackFrame) -> bool {
    let (Some(this), Some(self_), Some(vm), Some(frame)) = (unsafe { this.as_ref() }, unsafe { self_.as_ref() }, unsafe { vm.as_ref() }, unsafe { frame.as_ref() }) else {
        return false;
    };
    let count = usize::from(this.base.paramCount);
    let Some(stack) = frame.owning_stack() else { return false };
    if count > MAX_PARAMS {
        return false;
    }
    let page = stack.get_page_for_frame(frame);
    let none = Variable::none();
    let mut args = [&none; MAX_PARAMS];
    for (i, arg) in (0u32..).zip(args.iter_mut().take(count)) {
        let Some(v) = stack.get_stack_frame_variable(frame, i, page) else { return false };
        *arg = Variable::view(v);
    }
    let call = NativeCall { vm, self_: Variable::view(self_), args: &args[..count], stack_id };
    match unsafe { (this.invoke)(this, &call) } {
        Some(out) => {
            unsafe { assign(result, &out.0) };
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
        sre::fixed_slots!("BSScript::IStackCallbackFunctor", 0x0, 0, 1, 2, 3);
        Box::into_raw(Box::new(Self {
            header: sre::BSScript__IStackCallbackFunctor { __vftable: core::ptr::from_ref(Self::VTABLE).cast(), refCount: 1, unknown_C: [0; 4], __engine: sre::Engine::new() },
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
        let value = unsafe { result.as_ref() }.map_or(&none, Variable::view);
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
        sre::fixed_slots!("BSScript::IFunctionArguments", 0x0, 0, 1);
        Self { vtable: Self::VTABLE, values: values.as_ptr(), len: values.len() }
    }

    fn as_raw(&mut self) -> *mut sre::BSScript__IFunctionArguments {
        core::ptr::from_mut(self).cast()
    }

    unsafe extern "C" fn dtor(this: *mut Self, _: u32) -> *mut Self {
        this
    }

    unsafe extern "C" fn supply(this: *mut Self, out: *mut c_void) -> bool {
        let this = unsafe { &*this };
        let Ok(len) = u32::try_from(this.len) else { return false };
        if !unsafe { sre::resize_variable_array(out, len) } {
            return false;
        }
        let data = unsafe { (*out.cast::<sre::BSScrapArray>()).data.cast::<VariableRaw>() };
        for i in 0..this.len {
            unsafe { assign(data.add(i), &(*this.values.add(i)).0) };
        }
        true
    }
}

/// What the generated `VirtualMachine` methods leave raw: lookups by `&str` and out-parameters, the
/// handle policy's form handles, native registration, and calls and events with typed arguments.
pub trait VirtualMachineExt {
    /// The VM through `g_SkyrimVM` (id 403618) + 0x210, `None` before `SkyrimVM::ctor` stored it.
    fn singleton() -> Option<&'static Self>;
    /// The registry pointer of an SKSE `PapyrusInterface` callback as the VM, after checking that
    /// its vtable is `BSScript::Internal::VirtualMachine`'s.
    ///
    /// # Safety
    /// `p` must be null or point to a live object with a vtable.
    unsafe fn from_registry(p: *mut c_void) -> Option<&'static Self>;
    /// The handle policy's slot 4 `GetHandleForObject` with the `Form` type id, which encodes every form the same way.
    fn handle_for_form(&self, form: &TESForm) -> Handle;
    /// The form a handle names when it is of class `type_id` and available: the policy's slots 1,
    /// 2 and 8, the sequence the engine's own natives run on an object argument.
    fn form_for_handle(&self, type_id: VMTypeID, handle: Handle) -> Option<&'static TESForm>;
    /// Slot 13 `GetTypeIDForScriptObject`: the type id a script class was registered with, `None`
    /// for a class the engine did not register (every mod script).
    fn type_id(&self, class: &str) -> Option<u32>;
    /// Slot 9 `GetScriptObjectType`: the class by name, loading and linking `Data\Scripts\<class>.pex`
    /// when it is missing or unlinked; `None` when no script exists.
    fn script_type(&self, class: &str) -> Option<&'static ObjectTypeInfo>;
    /// Slot 10 `GetScriptObjectTypeByID` (medium in `papyrus.json`; the lookup the engine's own
    /// native constructors use to type form parameters).
    fn script_type_by_id(&self, id: VMTypeID) -> Option<&'static ObjectTypeInfo>;
    /// Slot 28 `FindBoundObject`: the script object of class `class` bound to `handle`, `None`
    /// when none is (no script of that class is attached to the form).
    fn bound_object(&self, handle: Handle, class: &str) -> Option<ObjectPtr>;
    /// Slot 26 `SetCallableFromTasklets`: lets the tasklet VM run the native without a context
    /// switch, what the engine sets for most of its own natives after binding them.
    fn allow_tasklets(&self, class: &str, name: &str, callable: bool);
    /// Registers `f` as `class.name`, a `native global` function. The `.pex` must declare it with
    /// the same parameter and return types ([`ParamType`] per Rust type) or the engine refuses the
    /// binding when the script links.
    fn register_native<F: NativeFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error>;
    /// Registers `f` as `class.name`, a latent `native global` function: the calling script waits
    /// until `f`'s [`Latent`] is completed. The `.pex` declares it like any native, returning the
    /// type of `F::Output`.
    fn register_latent<F: LatentFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error>;
    /// Registers `f` as `class.name`, a `native` method of a script that extends the form class
    /// of `f`'s first parameter; a call whose object is not bound to such a form fails with the
    /// engine's "no native object bound" error.
    fn register_method<F: NativeMethod<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error>;
    /// Registers `class.name` with the declared shape and a handler that can be replaced later
    /// ([`ErasedNative::set`]); `params` and `ret` are what the typed registrations take from
    /// their `F` ([`NativeFn::PARAMS`] and [`NativeFn::RETURN`], a latent's `Output` type).
    fn register_erased(&self, class: &str, name: &str, kind: NativeKind, params: &[ParamType], ret: Option<ParamType>, f: ErasedNativeFn) -> Result<ErasedNative, Error>;
    /// Slot 38 `DispatchStaticCall` of `class.function(args)`: the call is queued on a new stack and
    /// `on_result` runs with its result when it completes, on the VM thread that ran it (at once with
    /// None for an empty function). Returns what the slot returned. The class's script must be
    /// loadable (`script_type` semantics apply).
    fn call_static(&self, thread: &GameThread, class: &str, function: &str, args: &[Variable], on_result: ResultFn) -> bool;
    /// Slot 39 `DispatchMethodCall` of `function(args)` on `object`, as [`call_static`](Self::call_static).
    fn call_method(&self, thread: &GameThread, object: &ObjectPtr, function: &str, args: &[Variable], on_result: ResultFn) -> bool;
    /// Slot 22 `CreateArray`: a new array of `len` elements of type `element`, each `{element, 0}`,
    /// as an array Variable holding the creator's reference (the VM's tracking list holds another
    /// until its next update). It allocates from the global heap, not the per-thread scrap heap.
    fn new_array(&self, element: TypeInfo, len: u32) -> Option<Variable>;
    /// Slot 36 `SendEvent`: calls `event(args)` on every script object bound to exactly `handle`
    /// whose script declares that event, each as a new stack. The arguments are read during the call,
    /// once per receiving object. A reference's aliases and an actor's active effects have handles of
    /// their own; the engine reaches them through `SkyrimVM::RelayEventToRefAndEffects` after this.
    fn send_script_event(&self, thread: &GameThread, handle: Handle, event: &str, args: &[Variable]);
    /// `SkyrimVM::RelayEventToRefAndEffects` (id 54033): sends `event(args)` to every alias the
    /// reference behind `reference` fills and, for an actor, to every active effect with a handle,
    /// each through `SendEvent`; not to the reference itself. The engine sends to the reference first
    /// with [`send_script_event`](Self::send_script_event), then relays, and a mod reaches what the
    /// engine reaches by doing the same.
    fn relay_script_event(&self, thread: &GameThread, reference: Handle, event: &str, args: &[Variable]);
    /// Slot 21 `CreateObject`: a new, unbound script object of `class` (its `.pex` must load).
    fn new_object(&self, class: &str) -> Option<ObjectPtr>;
    /// Binds `object` to the form `handle` names (`VirtualMachine::BindObject`, id 104820): from
    /// then on [`bound_object`](Self::bound_object) returns it and calls with that form as `self`
    /// run its methods. The engine ORs the call's flag into the pointer it stores; this passes
    /// `false`, as the engine does for forms and its bind policy for every object, a mod's own
    /// included (`true` marks only some script objects the engine loads).
    fn bind_to_handle(&self, thread: &GameThread, object: &ObjectPtr, handle: Handle);
}

impl VirtualMachineExt for VirtualMachine {
    fn singleton() -> Option<&'static Self> {
        unsafe { (sre::g_skyrim_vm().field_210() as *const Self).as_ref() }
    }

    unsafe fn from_registry(p: *mut c_void) -> Option<&'static Self> {
        let object = unsafe { p.cast::<Self>().as_ref() }?;
        (crate::class_of(object)?.id == sre::id!(sre::raw::vtables::BSScript__Internal__VirtualMachine_vtable::ID)).then_some(object)
    }

    fn handle_for_form(&self, form: &TESForm) -> Handle {
        self.handle_policy().map_or(Handle::INVALID, |policy| Handle(unsafe { policy.get_handle_for_object(VMTypeID::Form, core::ptr::from_ref(form).cast()) }))
    }

    fn form_for_handle(&self, type_id: VMTypeID, handle: Handle) -> Option<&'static TESForm> {
        let policy = self.handle_policy()?;
        if !policy.handle_is_type(type_id, handle.0) || !policy.is_handle_object_available(handle.0) {
            return None;
        }
        unsafe { policy.get_object_for_handle(type_id, handle.0).cast::<TESForm>().as_ref() }
    }

    fn type_id(&self, class: &str) -> Option<u32> {
        let name = BSFixedString::new(class);
        let mut id = 0u32;
        unsafe { self.get_type_id_for_script_object(name.raw(), (&raw mut id).cast()) }.then_some(id)
    }

    fn script_type(&self, class: &str) -> Option<&'static ObjectTypeInfo> {
        let name = BSFixedString::new(class);
        let mut out = core::ptr::null_mut::<ObjectTypeInfo>();
        if !unsafe { self.get_script_object_type(name.raw(), (&raw mut out).cast()) } {
            return None;
        }
        unsafe { out.as_ref() }
    }

    fn script_type_by_id(&self, id: VMTypeID) -> Option<&'static ObjectTypeInfo> {
        let mut out = core::ptr::null_mut::<ObjectTypeInfo>();
        if !unsafe { self.get_script_object_type_by_id(id, (&raw mut out).cast()) } {
            return None;
        }
        unsafe { out.as_ref() }
    }

    fn bound_object(&self, handle: Handle, class: &str) -> Option<ObjectPtr> {
        let class = CString::new(class).ok()?;
        let mut out = core::ptr::null_mut::<sre::BSScript__Object>();
        if !unsafe { self.find_bound_object(handle.0, class.as_ptr(), (&raw mut out).cast()) } {
            return None;
        }
        NonNull::new(out).map(ObjectPtr)
    }

    fn allow_tasklets(&self, class: &str, name: &str, callable: bool) {
        let (Ok(class), Ok(name)) = (CString::new(class), CString::new(name)) else { return };
        unsafe { self.set_callable_from_tasklets(class.as_ptr(), name.as_ptr(), callable) };
    }

    fn register_native<F: NativeFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        bind(self, class, name, NativeKind::Global, F::PARAMS, F::RETURN, invoke_fn::<F, M>, f).map(Native::function)
    }

    fn register_latent<F: LatentFn<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        bind(self, class, name, NativeKind::Latent, F::PARAMS, <F::Output as IntoVariable>::TYPE, invoke_latent::<F, M>, f).map(Native::function)
    }

    fn register_method<F: NativeMethod<M>, M>(&self, class: &str, name: &str, f: F) -> Result<&'static NativeFunction, Error> {
        bind(self, class, name, NativeKind::Method, F::PARAMS, F::RETURN, invoke_method::<F, M>, f).map(Native::function)
    }

    fn register_erased(&self, class: &str, name: &str, kind: NativeKind, params: &[ParamType], ret: Option<ParamType>, f: ErasedNativeFn) -> Result<ErasedNative, Error> {
        bind(self, class, name, kind, params, ret, invoke_erased, RwLock::new(f)).map(ErasedNative)
    }

    fn call_static(&self, _: &GameThread, class: &str, function: &str, args: &[Variable], on_result: ResultFn) -> bool {
        let class = BSFixedString::new(class);
        let function = BSFixedString::new(function);
        let mut args = Args::new(args);
        let mut callback = Callback::new(on_result);
        let queued = unsafe { self.dispatch_static_call(class.raw(), function.raw(), args.as_raw(), (&raw mut callback).cast()) };
        Callback::release(callback);
        queued
    }

    fn call_method(&self, _: &GameThread, object: &ObjectPtr, function: &str, args: &[Variable], on_result: ResultFn) -> bool {
        let function = BSFixedString::new(function);
        let mut args = Args::new(args);
        let mut target = object.0.as_ptr();
        let mut callback = Callback::new(on_result);
        let queued = unsafe { self.dispatch_method_call((&raw mut target).cast(), function.raw(), args.as_raw(), (&raw mut callback).cast()) };
        Callback::release(callback);
        queued
    }

    fn new_array(&self, element: TypeInfo, len: u32) -> Option<Variable> {
        let mut out = core::ptr::null_mut::<sre::BSScript__Array>();
        let ty = type_info(element.bits());
        if !unsafe { self.create_array(&raw const ty, len, (&raw mut out).cast()) } || out.is_null() {
            return None;
        }
        Some(Variable(variable(element.array_of().bits(), out as u64)))
    }

    fn send_script_event(&self, _: &GameThread, handle: Handle, event: &str, args: &[Variable]) {
        let event = BSFixedString::new(event);
        let mut args = Args::new(args);
        self.send_event(handle.0, event.raw(), unsafe { &*args.as_raw() });
    }

    fn relay_script_event(&self, _: &GameThread, reference: Handle, event: &str, args: &[Variable]) {
        let event = BSFixedString::new(event);
        let mut args = Args::new(args);
        unsafe { sre::g_skyrim_vm().relay_event_to_ref_and_effects(reference.0, event.raw(), args.as_raw(), core::ptr::null_mut()) };
    }

    fn new_object(&self, class: &str) -> Option<ObjectPtr> {
        let class = BSFixedString::new(class);
        let mut out = core::ptr::null_mut::<sre::BSScript__Object>();
        if !unsafe { self.create_object(class.raw(), (&raw mut out).cast()) } {
            return None;
        }
        NonNull::new(out).map(ObjectPtr)
    }

    fn bind_to_handle(&self, _: &GameThread, object: &ObjectPtr, handle: Handle) {
        let mut target = object.0.as_ptr();
        unsafe { self.bind_object((&raw mut target).cast(), handle.0, false) };
    }
}

#[allow(clippy::too_many_arguments)]
fn bind<F: Send + Sync + 'static>(
    vm: &VirtualMachine,
    class: &str,
    name: &str,
    kind: NativeKind,
    params: &[ParamType],
    ret: Option<ParamType>,
    invoke: unsafe fn(*const NativeHeader, &NativeCall<'_>) -> Option<Variable>,
    f: F,
) -> Result<&'static Native<F>, Error> {
    let return_type = ret.map_or(Ok(TypeInfo::NONE), |t| t.type_info(vm))?;
    let count = u16::try_from(params.len()).expect("at most MAX_PARAMS parameters");
    let mut param_infos = Vec::with_capacity(1 + 2 * params.len());
    param_infos.push(params.len() as u64);
    for (i, param) in params.iter().enumerate() {
        param_infos.push(BSFixedString::new(&format!("a{i}")).into_raw().data as u64);
        param_infos.push(param.type_info(vm)?.bits());
    }
    let param_infos = Box::leak(param_infos.into_boxed_slice());
    let mut base: sre::BSScript__NF_util__NativeFunctionBase = unsafe { core::mem::zeroed() };
    base.__vftable = (&raw const native_vtable().0).cast();
    base.refCount = 1;
    base.name = BSFixedString::new(name).into_raw();
    base.objectTypeName = BSFixedString::new(class).into_raw();
    base.stateName = BSFixedString::new("").into_raw();
    base.returnType = type_info(return_type.bits());
    base.params = param_infos[1..].as_mut_ptr().cast();
    base.paramCount = count;
    base.variableCount = count;
    base.isStatic = u8::from(kind != NativeKind::Method);
    base.isLatent = u8::from(kind == NativeKind::Latent);
    base.docString = BSFixedString::empty().into_raw();
    base.binding = invoke as *mut c_void;
    let native = Box::leak(Box::new(Native { header: NativeHeader { base, invoke }, f }));
    let function = unsafe { &*(&raw const native.header).cast::<sre::BSScript__IFunction>() };
    if !vm.bind_native_method(Some(function)) {
        return Err(Error::Bind { class: class.to_owned(), name: name.to_owned() });
    }
    Ok(native)
}
