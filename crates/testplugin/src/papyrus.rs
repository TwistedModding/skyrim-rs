// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The papyrus suite through `skyrim::papyrus` and the generated VM. `REGISTER` runs inside the SKSE `PapyrusInterface`
//! callback: the VM's identity, the type lookups and the binding of the natives
//! `papyrus/SkyrimRsTest.psc` and `SkyrimRsTestRef.psc` declare. `TESTS` runs after a game is
//! loaded (handles, script types, bound objects, then a `SkyrimRsTestRef` object bound to the
//! player) and `CALLS` at the save that ends the scenario of the RE repo's
//! `docs/subsystems/papyrus-verify.md`, checking what the natives received from `DevBench`'s calls
//! and what the dispatches from inside `Ping` (`Tick()`, `Scale(1.5, true)`, the bound object's
//! `SelfFormId()`) did and returned to their callbacks. Request 4e adds array natives (`Halves`,
//! `Forms`), two latent natives (`DoubleNow` completes inside the native, `DoubleLater` 250 ms later
//! from a thread of its own) and an `OnSkyrimRsEvent` each `Ping` sends to the bound object, whose
//! handler reports back through `RecordEvent`. Request 4f adds `SkyrimRsTestAlias` and
//! `SkyrimRsTestEffect` objects bound to one of the player's quest aliases and one of its scripted
//! active effects (the scenario casts a scripted spell on the player first); `Ping` relays the same
//! event to them, and their handlers report through `RecordRelay`.

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use skyrim::BSFixedString;
use skyrim::papyrus::{GameThread, Handle, Latent, NativeFunction, TypeInfo, Variable, VirtualMachine, active_effect_handles, alias_handles, current_stack_id, function_of};
use skyrim::prelude::*;
use sre::raw::globals;
use sre::{Actor, ActorValue, BSScript__Object, BSScript__ObjectTypeInfo, PapyrusType, TESForm, TESObjectREFR, VMTypeID};

use crate::{IRON_SWORD, PLAYER, Test, check, describe as form_name, form};

const CLASS: &str = "SkyrimRsTest";
const REF_CLASS: &str = "SkyrimRsTestRef";
const ALIAS_CLASS: &str = "SkyrimRsTestAlias";
const EFFECT_CLASS: &str = "SkyrimRsTestEffect";

/// The registry pointer the SKSE callback received.
pub static REGISTRY: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static NATIVES: Mutex<Vec<(&'static str, &'static NativeFunction)>> = Mutex::new(Vec::new());
static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
static TICKS: AtomicU32 = AtomicU32::new(0);
static DISPATCHES_OK: AtomicU32 = AtomicU32::new(0);
/// What the dispatch callbacks received: `(function, result, thread)`.
static RESULTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// `DoubleLater`'s completions, from its own thread.
static COMPLETIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// What `OnSkyrimRsEvent` passed to `RecordEvent`.
static EVENTS: Mutex<Vec<(i32, String)>> = Mutex::new(Vec::new());
/// What the relayed handlers passed to `RecordRelay`: `(alias or effect, count)`.
static RELAYED: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());
/// The alias and effect handles the suite's objects were bound to.
static RELAY_TARGETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
const LATER_MS: u64 = 250;

fn os_thread() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

fn type_name(ty: &BSScript__ObjectTypeInfo) -> &BSFixedString {
    BSFixedString::view(&ty.name)
}

fn is_linked(ty: &BSScript__ObjectTypeInfo) -> bool {
    ty.linkedAndVariables & 3 == 3
}

/// A script class as the logs name it: its name, link state and own variable count.
fn describe_type(ty: &BSScript__ObjectTypeInfo) -> String {
    format!("{} (link state {}, {} variables)", type_name(ty), ty.linkedAndVariables & 3, (ty.linkedAndVariables >> 8) & 0x3FF)
}

/// A script object as the logs name it: its class, handle and reference count.
fn describe_object(object: &BSScript__Object) -> String {
    let class = object.type_().map_or_else(|| "?".to_owned(), |t| type_name(t).to_string());
    format!("{class} object {} (refs {})", Handle(object.get_handle()), object.refCountAndLock & 0x7FFF_FFFF)
}

/// What `g_IObjectHandlePolicy_ptr` holds.
fn handle_policy_global() -> Result<usize, String> {
    let at = skyrim::reloc().address(sre::id!(globals::g_IObjectHandlePolicy_ptr)).map_err(|e| e.to_string())?;
    Ok(unsafe { *(at as *const usize) })
}

fn is_type(vm: &VirtualMachine, type_id: VMTypeID, handle: Handle) -> bool {
    vm.handle_policy().is_some_and(|p| p.handle_is_type(type_id, handle.0))
}

fn on_result(function: &'static str) -> skyrim::papyrus::ResultFn {
    Box::new(move |result| RESULTS.lock().unwrap().push(format!("{function} -> {result:?} on thread {}", os_thread())))
}

pub const REGISTER: &[(&str, Test)] = &[
    ("vm_identity", vm_identity),
    ("type_ids_before", type_ids_before),
    ("script_type_by_id", script_type_by_id),
    ("register", register),
    ("rebind", rebind),
    ("tasklets", tasklets),
];

pub const TESTS: &[(&str, Test)] = &[
    ("handles", handles),
    ("script_types", script_types),
    ("find_bound_object", find_bound_object),
    ("bind_object", bind_object),
    ("game_thread", game_thread),
    ("type_ids_after", type_ids_after),
];

pub const CALLS_TESTS: &[(&str, Test)] = &[("calls", calls), ("dispatch", dispatch), ("stack_ids", stack_ids), ("arrays", arrays), ("latent", latent), ("events", events), ("relay", relay)];

struct Call {
    name: &'static str,
    stack_id: u32,
    detail: String,
}

fn record(name: &'static str, detail: String) {
    let stack_id = current_stack_id().unwrap_or(0);
    log!("native {name} (stack {stack_id}, thread {}): {detail}", os_thread());
    CALLS.lock().unwrap().push(Call { name, stack_id, detail });
}

fn ping(x: i32) -> i32 {
    let dispatched = match (GameThread::current(), VirtualMachine::singleton()) {
        (Some(thread), Some(vm)) => {
            let tick = vm.call_static(&thread, CLASS, "Tick", &[], on_result("Tick"));
            let scale = vm.call_static(&thread, CLASS, "Scale", &[Variable::float(1.5), Variable::bool(true)], on_result("Scale"));
            let method = vm.bound_object(Handle::form(PLAYER), REF_CLASS).is_some_and(|object| vm.call_method(&thread, &object, "SelfFormId", &[], on_result("SelfFormId")));
            let args = [Variable::int(x), Variable::string(&BSFixedString::new("from Ping"))];
            vm.send_script_event(&thread, Handle::form(PLAYER), "OnSkyrimRsEvent", &args);
            bind_relay_targets(vm, &thread);
            vm.relay_script_event(&thread, Handle::form(PLAYER), "OnSkyrimRsEvent", &args);
            [tick, scale, method]
        }
        _ => [false; 3],
    };
    if dispatched == [true; 3] {
        DISPATCHES_OK.fetch_add(1, Ordering::Relaxed);
    }
    record("Ping", format!("x={x} -> {}, dispatched Tick, Scale(1.5, true), SelfFormId: {dispatched:?}", x + 1));
    x + 1
}

fn form_id(form: Option<&'static TESForm>) -> i32 {
    let id = form.map_or(0, |f| f.formID.cast_signed());
    record("FormId", format!("form={:?} -> {id}", form.map(form_name)));
    id
}

fn player_health(actor: Option<&'static Actor>) -> f32 {
    let health = actor.map_or(-1.0, |a| a.as_actor_value_owner().get_actor_value(ActorValue::Health));
    record("PlayerHealth", format!("actor={:?} -> {health}", actor.map(|a| form_name(a))));
    health
}

fn echo(s: BSFixedString) -> BSFixedString {
    record("Echo", format!("s={s:?} refcount {} -> same", s.ref_count()));
    s
}

fn scale(x: f32, double: bool) -> f32 {
    let out = if double { x * 2.0 } else { x };
    record("Scale", format!("x={x} double={double} -> {out}"));
    out
}

fn player() -> Option<&'static TESForm> {
    let player = sre::find_form_by_id(PLAYER);
    record("Player", format!("-> {:?}", player.map(form_name)));
    player
}

fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    record("Tick", format!("tick {}", TICKS.load(Ordering::Relaxed)));
}

#[allow(clippy::needless_pass_by_value)]
fn describe(reference: Option<&'static TESObjectREFR>, count: i32, suffix: String, flag: bool) -> String {
    let out = format!("{} x{count} {suffix} flag={flag}", reference.map_or_else(|| "none".to_owned(), |r| format!("{:08X}", r.formID)));
    record("Describe", format!("ref={:?} count={count} suffix={suffix:?} flag={flag} -> {out:?}", reference.map(|r| form_name(r))));
    out
}

fn self_form_id(this: &TESObjectREFR) -> i32 {
    let id = this.formID.cast_signed();
    record("SelfFormId", format!("self={} -> {id}", form_name(this)));
    id
}

#[allow(clippy::needless_pass_by_value, clippy::cast_precision_loss)]
fn halves(values: Vec<i32>) -> Vec<f32> {
    let out: Vec<f32> = values.iter().map(|&v| v as f32 / 2.0).collect();
    record("Halves", format!("values={values:?} -> {out:?}"));
    out
}

fn forms() -> Vec<Option<&'static TESForm>> {
    let out = vec![sre::find_form_by_id(PLAYER), None, sre::find_form_by_id(IRON_SWORD)];
    record("Forms", format!("-> {:?}", out.iter().map(|f| f.map(form_name)).collect::<Vec<_>>()));
    out
}

fn double_now(latent: Latent<i32>, x: i32) {
    record("DoubleNow", format!("x={x}, completed inside the native with {}", x * 2));
    latent.complete(x * 2);
}

fn double_later(latent: Latent<i32>, x: i32) {
    record("DoubleLater", format!("x={x}, completing from a thread of its own in {LATER_MS} ms"));
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(LATER_MS));
        let stack = latent.stack_id();
        latent.complete(x * 2);
        let done = format!("DoubleLater({x}) stack {stack} -> {} completed on thread {} after {} ms", x * 2, os_thread(), started.elapsed().as_millis());
        log!("{done}");
        COMPLETIONS.lock().unwrap().push(done);
    });
}

#[allow(clippy::needless_pass_by_value)]
fn record_event(count: i32, text: String) {
    record("RecordEvent", format!("count={count} text={text:?}"));
    EVENTS.lock().unwrap().push((count, text));
}

fn record_relay(kind: String, count: i32) {
    record("RecordRelay", format!("kind={kind} count={count}"));
    RELAYED.lock().unwrap().push((kind, count));
}

/// Binds a `SkyrimRsTestAlias` to the player's first reference-alias handle and a `SkyrimRsTestEffect`
/// to its first active-effect handle, once each, from the first native that finds them.
fn bind_relay_targets(vm: &VirtualMachine, thread: &GameThread) {
    let mut targets = RELAY_TARGETS.lock().unwrap();
    if !targets.is_empty() {
        return;
    }
    let Some(player) = form(PLAYER).ok().and_then(TESForm::cast::<Actor>) else { return };
    let alias = alias_handles(player).into_iter().find(|&h| is_type(vm, VMTypeID::ReferenceAlias, h));
    let effect = active_effect_handles(player).into_iter().find(|&h| is_type(vm, VMTypeID::ActiveMagicEffect, h));
    for (class, handle) in [(ALIAS_CLASS, alias), (EFFECT_CLASS, effect)] {
        let Some(handle) = handle else {
            targets.push(format!("{class}: no handle"));
            continue;
        };
        let bound = vm.new_object(class).map(|object| {
            vm.bind_to_handle(thread, &object, handle);
            vm.bound_object(handle, class).is_some_and(|b| b.get_handle() == handle.0)
        });
        targets.push(format!("{class} on {handle}: bound {bound:?}"));
    }
}

fn event_count() -> i32 {
    i32::try_from(EVENTS.lock().unwrap().len()).unwrap_or(i32::MAX)
}

fn registry() -> Result<&'static VirtualMachine, String> {
    unsafe { VirtualMachine::from_registry(REGISTRY.load(Ordering::Relaxed)) }.ok_or_else(|| "the registry pointer is not a BSScript::Internal::VirtualMachine".to_owned())
}

fn vm() -> Result<&'static VirtualMachine, String> {
    VirtualMachine::singleton().ok_or_else(|| "g_SkyrimVM + 0x210 is null".to_owned())
}

fn native(name: &str) -> Result<&'static NativeFunction, String> {
    NATIVES.lock().unwrap().iter().rev().find(|(n, _)| *n == name).map(|(_, f)| *f).ok_or_else(|| format!("{name} was not registered"))
}

fn vm_identity() -> Result<String, String> {
    let registry = registry()?;
    let global = vm()?;
    check(core::ptr::eq(registry, global), "the registry pointer is not g_SkyrimVM + 0x210")?;
    let skyrim_vm = skyrim::reloc().address(sre::id!(globals::g_SkyrimVM)).map_err(|e| e.to_string())?;
    let policy = registry.handlePolicy as usize;
    let bind = registry.objectBindPolicy as usize;
    let vm = unsafe { &*(skyrim_vm as *const sre::SkyrimVM) };
    check(policy == vm.m_HandlePolicy_338_ptr() as usize, &format!("VM+0x90 is g_SkyrimVM + {:#x}, the member says {:#x}", policy.wrapping_sub(skyrim_vm), (vm.m_HandlePolicy_338_ptr() as usize).wrapping_sub(skyrim_vm)))?;
    check(bind == vm.m_ObjectBindPolicy_3A8_ptr() as usize, &format!("VM+0x98 is g_SkyrimVM + {:#x}, the member says {:#x}", bind.wrapping_sub(skyrim_vm), (vm.m_ObjectBindPolicy_3A8_ptr() as usize).wrapping_sub(skyrim_vm)))?;
    let via_slot = registry.get_object_handle_policy().map_or(0, |p| core::ptr::from_ref(p) as usize);
    check(via_slot == policy && registry.handle_policy().is_some_and(|p| core::ptr::from_ref(p) as usize == policy), "GetObjectHandlePolicy is not VM+0x90")?;
    let global = handle_policy_global()?;
    let refs = registry.refCount;
    Ok(format!("registry {registry:p} = g_SkyrimVM+0x210, refCount {refs}, handle policy {policy:#x} = g_SkyrimVM+0x338 = GetObjectHandlePolicy(), bind policy {bind:#x} = g_SkyrimVM+0x3a8; g_IObjectHandlePolicy_ptr holds {global:#x} during registration"))
}

fn type_ids_before() -> Result<String, String> {
    let vm = registry()?;
    let actor = vm.type_id("Actor");
    let weapon = vm.type_id("Weapon");
    let own = vm.type_id(CLASS);
    check(actor == Some(VMTypeID::Actor as u32), &format!("Actor type id is {actor:?}"))?;
    check(weapon == Some(VMTypeID::Weapon as u32), &format!("Weapon type id is {weapon:?}"))?;
    check(own.is_none(), &format!("{CLASS} has type id {own:?} before anything was registered"))?;
    Ok(format!("Actor={actor:?} Weapon={weapon:?} {CLASS}={own:?}"))
}

fn script_type_by_id() -> Result<String, String> {
    let vm = registry()?;
    let form = vm.script_type_by_id(VMTypeID::Form).ok_or("no script type for Form")?;
    let actor = vm.script_type_by_id(VMTypeID::Actor).ok_or("no script type for Actor")?;
    check(*type_name(form) == *"Form" && form.parent().is_none(), &format!("Form type is {}", describe_type(form)))?;
    let chain: Vec<String> = core::iter::successors(Some(actor), |t| t.parent()).map(|t| type_name(t).to_string()).collect();
    check(chain == ["Actor", "ObjectReference", "Form"], &format!("Actor's parent chain is {chain:?}"))?;
    Ok(format!("Form {} at {form:p}; Actor chain {}; Actor {}", describe_type(form), chain.join(" -> "), describe_type(actor)))
}

fn register() -> Result<String, String> {
    let vm = registry()?;
    let mut natives = NATIVES.lock().unwrap();
    let mut summary = Vec::new();
    let mut bind = |name: &'static str, result: Result<&'static NativeFunction, skyrim::papyrus::Error>| -> Result<(), String> {
        let native = result.map_err(|e| format!("{name}: {e}"))?;
        check(native.ref_count() == 2 && *native.name() == *name && (*native.class_name() == *CLASS || *native.class_name() == *REF_CLASS), &format!("{name}: refCount {} name {} class {}", native.ref_count(), native.name(), native.class_name()))?;
        summary.push(format!("{}.{name}({:?}) -> {:?} at {:p}", native.class_name(), native.param_types(), native.return_type(), native.as_ptr()));
        natives.push((name, native));
        Ok(())
    };
    bind("Ping", vm.register_native(CLASS, "Ping", ping))?;
    bind("FormId", vm.register_native(CLASS, "FormId", form_id))?;
    bind("PlayerHealth", vm.register_native(CLASS, "PlayerHealth", player_health))?;
    bind("Echo", vm.register_native(CLASS, "Echo", echo))?;
    bind("Scale", vm.register_native(CLASS, "Scale", scale))?;
    bind("Player", vm.register_native(CLASS, "Player", player))?;
    bind("Tick", vm.register_native(CLASS, "Tick", tick))?;
    bind("Describe", vm.register_native(CLASS, "Describe", describe))?;
    bind("SelfFormId", vm.register_method(REF_CLASS, "SelfFormId", self_form_id))?;
    bind("Halves", vm.register_native(CLASS, "Halves", halves))?;
    bind("Forms", vm.register_native(CLASS, "Forms", forms))?;
    bind("DoubleNow", vm.register_latent(CLASS, "DoubleNow", double_now))?;
    bind("DoubleLater", vm.register_latent(CLASS, "DoubleLater", double_later))?;
    bind("RecordEvent", vm.register_native(CLASS, "RecordEvent", record_event))?;
    bind("EventCount", vm.register_native(CLASS, "EventCount", event_count))?;
    bind("RecordRelay", vm.register_native(CLASS, "RecordRelay", record_relay))?;
    Ok(format!("{} natives bound: {}", summary.len(), summary.join("; ")))
}

fn rebind() -> Result<String, String> {
    let vm = registry()?;
    let first = native("Ping")?;
    let second = vm.register_native(CLASS, "Ping", ping).map_err(|e| e.to_string())?;
    check(!core::ptr::eq(first, second) && second.ref_count() == 2, "the second Ping object was not bound")?;
    NATIVES.lock().unwrap().push(("Ping", second));
    Ok(format!("Ping bound twice: {:p} then {:p}, first refCount now {}", first.as_ptr(), second.as_ptr(), first.ref_count()))
}

fn tasklets() -> Result<String, String> {
    let vm = registry()?;
    for name in ["Ping", "FormId", "PlayerHealth", "Echo", "Scale", "Player", "Tick", "Describe"] {
        vm.allow_tasklets(CLASS, name, true);
    }
    Ok("SetCallableFromTasklets(SkyrimRsTest, each, true) returned".into())
}

fn handles() -> Result<String, String> {
    let vm = vm()?;
    let policy = vm.handle_policy().ok_or("the VM has no handle policy")?;
    let global = handle_policy_global()?;
    check(global == core::ptr::from_ref(policy) as usize, &format!("g_IObjectHandlePolicy_ptr holds {global:#x}, not the VM's policy"))?;
    let is_type = |ty: VMTypeID, h: Handle| policy.handle_is_type(ty, h.0);
    let is_available = |h: Handle| policy.is_handle_object_available(h.0);
    let invalid = Handle(policy.get_invalid_handle());
    check(invalid == Handle::INVALID && invalid.0 == 0xFFFF_0000_0000 && invalid.is_invalid() && invalid.form_id().is_none(), &format!("invalid handle is {invalid}"))?;
    let player = form(PLAYER)?;
    let sword = form(IRON_SWORD)?;
    let h = vm.handle_for_form(player);
    check(h == Handle::form(PLAYER) && h.0 == 0xFFFF_0000_0014 && h.form_id() == Some(PLAYER), &format!("player handle is {h}"))?;
    check(is_type(VMTypeID::Form, h) && is_type(VMTypeID::ObjectReference, h) && is_type(VMTypeID::Actor, h) && !is_type(VMTypeID::Weapon, h), "HandleIsType on the player handle")?;
    check(is_available(h), "the player handle is not available")?;
    let back = vm.form_for_handle(VMTypeID::Actor, h).ok_or("GetObjectForHandle(Actor, player) is null")?;
    check(core::ptr::eq(back, player), "GetObjectForHandle(Actor) is not the player")?;
    check(vm.form_for_handle(VMTypeID::ObjectReference, h).is_some_and(|f| core::ptr::eq(f, player)), "GetObjectForHandle(ObjectReference) is not the player")?;
    check(vm.form_for_handle(VMTypeID::Weapon, h).is_none(), "GetObjectForHandle(Weapon, player) returned a form")?;
    let hs = vm.handle_for_form(sword);
    check(hs == Handle::form(IRON_SWORD) && hs.0 == 0xFFFF_0001_2EB7, &format!("sword handle is {hs}"))?;
    check(vm.form_for_handle(VMTypeID::Weapon, hs).is_some_and(|f| core::ptr::eq(f, sword)), "GetObjectForHandle(Weapon, sword) is not the sword")?;
    check(!is_type(VMTypeID::Actor, hs) && vm.form_for_handle(VMTypeID::Actor, hs).is_none(), "the sword handle passes as an Actor")?;
    check(vm.form_for_handle(VMTypeID::Form, invalid).is_none(), "the invalid handle resolved to a form")?;
    Ok(format!("g_IObjectHandlePolicy_ptr {global:x?}, the VM's policy; invalid={invalid} (HandleIsType(Form)={} IsHandleObjectAvailable={}, no form); player={h} (Form/ObjectReference/Actor yes, Weapon no) -> {}; sword={hs} -> Weapon yes, Actor no", is_type(VMTypeID::Form, invalid), is_available(invalid), form_name(back)))
}

fn script_types() -> Result<String, String> {
    let vm = vm()?;
    let actor = vm.script_type("Actor").ok_or("GetScriptObjectType(Actor) failed")?;
    check(*type_name(actor) == *"Actor" && is_linked(actor), &format!("Actor is {}", describe_type(actor)))?;
    let missing = vm.script_type("NoSuchScript");
    check(missing.is_none_or(|t| !is_linked(t)), &format!("NoSuchScript resolved to a linked type {:?}", missing.map(describe_type)))?;
    let own = vm.script_type(CLASS).ok_or("GetScriptObjectType(SkyrimRsTest) failed: is SkyrimRsTest.pex in Data\\Scripts?")?;
    check(is_linked(own) && own.parent().is_none(), &format!("{CLASS} is {}", describe_type(own)))?;
    for name in ["Ping", "FormId", "PlayerHealth", "Echo", "Scale", "Player", "Tick", "Describe"] {
        let bound = function_of(own, name, true).ok_or_else(|| format!("{CLASS}.{name} has no function"))?;
        check(bound == native(name)?.as_ptr(), &format!("{CLASS}.{name} is bound to {bound:?}, not the registered native"))?;
    }
    check(function_of(own, "Nope", true).is_none(), "GetFunction found Nope")?;
    let ping_as_method = function_of(own, "Ping", false);
    let reference = vm.script_type(REF_CLASS).ok_or("GetScriptObjectType(SkyrimRsTestRef) failed")?;
    check(is_linked(reference) && reference.parent().is_some_and(|p| *type_name(p) == *"ObjectReference"), &format!("{REF_CLASS} is {}", describe_type(reference)))?;
    let method = native("SelfFormId")?;
    let found = [false, true].map(|is_static| function_of(reference, "SelfFormId", is_static));
    let (actor, missing, own, reference) = (describe_type(actor), missing.map(describe_type), describe_type(own), describe_type(reference));
    Ok(format!("Actor {actor}; NoSuchScript {missing:?}; {CLASS} {own} with 8 natives bound to the registered objects (GetFunction(Ping, isStatic=false) = {ping_as_method:?}); {REF_CLASS} {reference} extends ObjectReference; GetFunction(SelfFormId) for isStatic false/true = {found:?} (non-static functions are not returned for a linked type), the native {:?} has refCount {} after linking", method.as_ptr(), method.ref_count()))
}

fn find_bound_object() -> Result<String, String> {
    let vm = vm()?;
    let h = Handle::form(PLAYER);
    let object = vm.bound_object(h, "Actor").ok_or("FindBoundObject(player, Actor) failed")?;
    check(object.get_handle() == h.0, &format!("the Actor object's handle is {}", Handle(object.get_handle())))?;
    check(object.type_().is_some_and(|t| *type_name(t) == *"Actor"), "the object's type is not Actor")?;
    check(vm.bound_object(h, REF_CLASS).is_none(), "FindBoundObject(player, SkyrimRsTestRef) found an object")?;
    check(vm.bound_object(Handle::INVALID, "Actor").is_none(), "FindBoundObject(invalid, Actor) found an object")?;
    Ok(format!("player -> {}; SkyrimRsTestRef none; invalid none", describe_object(&object)))
}

fn bind_object() -> Result<String, String> {
    let vm = vm()?;
    let thread = unsafe { GameThread::assume() };
    let h = Handle::form(PLAYER);
    let created = vm.new_object(REF_CLASS).ok_or("CreateObject(SkyrimRsTestRef) failed")?;
    check(created.get_handle() != h.0 && created.type_().is_some_and(|t| *type_name(t) == *REF_CLASS), &format!("the new object is {}", describe_object(&created)))?;
    vm.bind_to_handle(&thread, &created, h);
    let bound = vm.bound_object(h, REF_CLASS).ok_or("FindBoundObject(player, SkyrimRsTestRef) found nothing after BindObject")?;
    check(core::ptr::from_ref(&*bound) == core::ptr::from_ref(&*created) && bound.get_handle() == h.0, &format!("found {}, created {}", describe_object(&bound), describe_object(&created)))?;
    Ok(format!("created {REF_CLASS} unbound, BindObject(player) -> FindBoundObject returns it: {}; main thread {}", describe_object(&bound), os_thread()))
}

fn game_thread() -> Result<String, String> {
    check(GameThread::current().is_none() && current_stack_id().is_none(), "a native call is reported outside any native")?;
    Ok("no native running: GameThread::current() = None, current_stack_id() = None".into())
}

fn type_ids_after() -> Result<String, String> {
    let vm = vm()?;
    Ok(format!("{CLASS}={:?} after linking (only RegisterObjectType classes have one)", vm.type_id(CLASS)))
}

fn calls() -> Result<String, String> {
    let calls = CALLS.lock().unwrap();
    let find = |name: &str, needle: &str| calls.iter().find(|c| c.name == name && c.detail.contains(needle)).map(|c| c.detail.clone()).ok_or_else(|| format!("no {name} call with {needle:?} among {}", calls.len()));
    let ping = find("Ping", "x=41 -> 42")?;
    find("Ping", "x=1 -> 2")?;
    let player_id = find("FormId", "-> 20")?;
    find("FormId", "-> 77495")?;
    let health = find("PlayerHealth", "ACHR 00000014")?;
    find("Echo", "\"hello skyrim\"")?;
    find("Scale", "x=2.5 double=true -> 5")?;
    find("Player", "ACHR 00000014")?;
    find("Describe", "-> \"00000014 x3 swords flag=false\"")?;
    check(player_id.contains("ACHR 00000014"), "FormId did not see the player as a form")?;
    let method = find("SelfFormId", "-> 20")?;
    Ok(format!("{} calls: {ping}; {player_id}; {health}; {method}; every expected call seen", calls.len()))
}

fn dispatch() -> Result<String, String> {
    let ticks = TICKS.load(Ordering::Relaxed);
    let ok = DISPATCHES_OK.load(Ordering::Relaxed);
    let calls = CALLS.lock().unwrap();
    let count = |name: &str, needle: &str| calls.iter().filter(|c| c.name == name && c.detail.contains(needle)).count();
    let pings = count("Ping", "");
    let scaled = count("Scale", "x=1.5 double=true -> 3");
    let methods = count("SelfFormId", "-> 20");
    check(pings == 2 && ok == 2 && ticks == 2, &format!("{pings} Ping calls, {ok} with all three dispatches true, {ticks} Tick calls"))?;
    check(scaled == 2, &format!("Scale(1.5, true) ran {scaled} times"))?;
    check(methods == 3, &format!("SelfFormId on the player ran {methods} times (2 dispatched, 1 from DevBench)"))?;
    let results = RESULTS.lock().unwrap().clone();
    let got = |prefix: &str| results.iter().filter(|r| r.starts_with(prefix)).count();
    check(got("Tick -> None") == 2 && got("Scale -> Float 3") == 2 && got("SelfFormId -> Int 20") == 2, &format!("callbacks received {results:?}"))?;
    Ok(format!("from inside Ping, twice: DispatchStaticCall(Tick) and (Scale, [1.5, true]) and DispatchMethodCall(bound SkyrimRsTestRef, SelfFormId) all returned true; Tick ran {ticks}, Scale(1.5, true) {scaled}, SelfFormId -> 20 {methods} times with DevBench's own call; callbacks {results:?}"))
}

fn stack_ids() -> Result<String, String> {
    let calls = CALLS.lock().unwrap();
    // Event handlers run on stacks SendEvent creates together, in either order; the calls ordered by
    // their callers are the others.
    let ids: Vec<u32> = calls.iter().filter(|c| c.name != "RecordEvent" && c.name != "RecordRelay").map(|c| c.stack_id).collect();
    check(ids.iter().all(|&id| id != 0), &format!("a call had stack id 0: {ids:?}"))?;
    let sorted = ids.windows(2).all(|w| w[0] < w[1]);
    check(sorted, &format!("stack ids are not increasing: {ids:?}"))?;
    Ok(format!("{} calls with increasing stack ids {ids:?}", ids.len()))
}

fn arrays() -> Result<String, String> {
    let calls = CALLS.lock().unwrap();
    let find = |name: &str| calls.iter().find(|c| c.name == name).map(|c| c.detail.clone()).ok_or_else(|| format!("{name} was never called"));
    let halves = find("Halves")?;
    check(halves == "values=[3, 8, -5] -> [1.5, 4.0, -2.5]", &format!("Halves: {halves}"))?;
    let forms = find("Forms")?;
    check(forms.contains("ACHR 00000014") && forms.contains("None") && forms.contains("00012EB7"), &format!("Forms: {forms}"))?;
    let made = vm()?.new_array(TypeInfo::raw(PapyrusType::Int), 3).ok_or("CreateArray(Int, 3) failed")?;
    let array = made.as_array().ok_or("CreateArray gave no array")?;
    let refs = array.refCount;
    check(array.size == 3 && refs >= 1 && made.elements().is_some_and(|e| e.iter().all(|e| e.as_int() == Some(0))), &format!("new Int[3] is {made:?}, refs {refs}"))?;
    Ok(format!("Halves {halves}; Forms {forms}; CreateArray(Int, 3) at the save = {made:?} with {refs} references"))
}

fn latent() -> Result<String, String> {
    let calls = CALLS.lock().unwrap();
    let now = calls.iter().find(|c| c.name == "DoubleNow" && c.detail.contains("x=21")).ok_or("DoubleNow(21) was never called")?;
    let later = calls.iter().find(|c| c.name == "DoubleLater" && c.detail.contains("x=5")).ok_or("DoubleLater(5) was never called")?;
    let completions = COMPLETIONS.lock().unwrap().clone();
    let prefix = format!("DoubleLater(5) stack {} -> 10", later.stack_id);
    let done = completions.iter().find(|c| c.starts_with(&prefix)).ok_or_else(|| format!("no completion of stack {} among {completions:?}", later.stack_id))?;
    let ms: u64 = done.rsplit(" after ").next().and_then(|t| t.trim_end_matches(" ms").parse().ok()).unwrap_or(0);
    check(ms >= LATER_MS, &format!("completed after {ms} ms"))?;
    for name in ["DoubleNow", "DoubleLater"] {
        check(native(name)?.raw().isLatent == 1, &format!("{name} is not latent"))?;
    }
    Ok(format!("DoubleNow stack {}: {}; DoubleLater stack {}: {}; {done}", now.stack_id, now.detail, later.stack_id, later.detail))
}

fn events() -> Result<String, String> {
    let events = EVENTS.lock().unwrap().clone();
    let pings: Vec<i32> = CALLS.lock().unwrap().iter().filter(|c| c.name == "Ping").filter_map(|c| c.detail.strip_prefix("x=")?.split(' ').next()?.parse().ok()).collect();
    let mut got: Vec<i32> = events.iter().map(|(n, _)| *n).collect();
    let mut want = pings.clone();
    got.sort_unstable();
    want.sort_unstable();
    check(got == want && events.iter().all(|(_, t)| t == "from Ping"), &format!("events {events:?} for Pings {pings:?}"))?;
    Ok(format!("OnSkyrimRsEvent reached the bound SkyrimRsTestRef once per Ping {pings:?}: {events:?}"))
}

fn relay() -> Result<String, String> {
    let targets = RELAY_TARGETS.lock().unwrap().clone();
    check(targets.len() == 2 && targets.iter().all(|t| t.ends_with("bound Some(true)")), &format!("relay targets {targets:?}"))?;
    let relayed = RELAYED.lock().unwrap().clone();
    let pings: Vec<i32> = CALLS.lock().unwrap().iter().filter(|c| c.name == "Ping").filter_map(|c| c.detail.strip_prefix("x=")?.split(' ').next()?.parse().ok()).collect();
    for kind in ["alias", "effect"] {
        // The string pool folds case, so "alias" comes back as the pool's existing "Alias".
        let mut got: Vec<i32> = relayed.iter().filter(|(k, _)| k.eq_ignore_ascii_case(kind)).map(|(_, n)| *n).collect();
        let mut want = pings.clone();
        got.sort_unstable();
        want.sort_unstable();
        check(got == want, &format!("{kind} handler received {got:?} for Pings {pings:?}"))?;
    }
    Ok(format!("{targets:?}; relayed {relayed:?}"))
}
