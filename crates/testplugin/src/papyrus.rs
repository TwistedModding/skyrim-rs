//! One test per `skyrim::papyrus` API. `REGISTER` runs inside the SKSE `PapyrusInterface`
//! callback: the VM's identity, the type lookups and the binding of the natives
//! `papyrus/SkyrimRsTest.psc` and `SkyrimRsTestRef.psc` declare. `TESTS` runs after a game is
//! loaded (handles, script types, bound objects, then a `SkyrimRsTestRef` object bound to the
//! player) and `CALLS` at the save that ends the scenario of the RE repo's
//! `docs/subsystems/papyrus-verify.md`, checking what the natives received from `DevBench`'s calls
//! and what the dispatches from inside `Ping` (`Tick()`, `Scale(1.5, true)`, the bound object's
//! `SelfFormId()`) did.

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use layout::v1_7_104::{classes, globals};
use skyrim::actors::{Actor, ActorValue, TESObjectREFR};
use skyrim::forms::TESForm;
use skyrim::papyrus::{BSFixedString, GameThread, Handle, HandlePolicy, NativeFunction, VMTypeID, Variable, VirtualMachine, current_stack_id};

use crate::{IRON_SWORD, PLAYER, Test, check, form};

const CLASS: &str = "SkyrimRsTest";
const REF_CLASS: &str = "SkyrimRsTestRef";

/// The registry pointer the SKSE callback received.
pub static REGISTRY: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static NATIVES: Mutex<Vec<(&'static str, &'static NativeFunction)>> = Mutex::new(Vec::new());
static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());
static TICKS: AtomicU32 = AtomicU32::new(0);
static DISPATCHES_OK: AtomicU32 = AtomicU32::new(0);

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

pub const CALLS_TESTS: &[(&str, Test)] = &[("calls", calls), ("dispatch", dispatch), ("stack_ids", stack_ids)];

struct Call {
    name: &'static str,
    stack_id: u32,
    detail: String,
}

fn record(name: &'static str, detail: String) {
    let stack_id = current_stack_id().unwrap_or(0);
    log!("native {name} (stack {stack_id}): {detail}");
    CALLS.lock().unwrap().push(Call { name, stack_id, detail });
}

fn ping(x: i32) -> i32 {
    let dispatched = match (GameThread::current(), VirtualMachine::get()) {
        (Some(thread), Some(vm)) => {
            let tick = vm.dispatch_static_call(&thread, CLASS, "Tick", &[]);
            let scale = vm.dispatch_static_call(&thread, CLASS, "Scale", &[Variable::float(1.5), Variable::bool(true)]);
            let method = vm.find_bound_object(Handle::form(PLAYER), REF_CLASS).is_some_and(|object| vm.dispatch_method_call(&thread, &object, "SelfFormId", &[]));
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
    let id = form.map_or(0, |f| f.form_id().0.cast_signed());
    record("FormId", format!("form={form:?} -> {id}"));
    id
}

fn player_health(actor: Option<&'static Actor>) -> f32 {
    let health = actor.map_or(-1.0, |a| a.actor_value_owner().get(ActorValue::Health));
    record("PlayerHealth", format!("actor={actor:?} -> {health}"));
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
    let player = PLAYER.lookup();
    record("Player", format!("-> {player:?}"));
    player
}

fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    record("Tick", format!("tick {}", TICKS.load(Ordering::Relaxed)));
}

#[allow(clippy::needless_pass_by_value)]
fn describe(reference: Option<&'static TESObjectREFR>, count: i32, suffix: String, flag: bool) -> String {
    let out = format!("{} x{count} {suffix} flag={flag}", reference.map_or_else(|| "none".to_owned(), |r| r.form_id().to_string()));
    record("Describe", format!("ref={reference:?} count={count} suffix={suffix:?} flag={flag} -> {out:?}"));
    out
}

fn self_form_id(this: &TESObjectREFR) -> i32 {
    let id = this.form_id().0.cast_signed();
    record("SelfFormId", format!("self={this:?} -> {id}"));
    id
}

fn registry() -> Result<&'static VirtualMachine, String> {
    unsafe { VirtualMachine::from_ptr(REGISTRY.load(Ordering::Relaxed)) }.ok_or_else(|| "the registry pointer is not a BSScript::Internal::VirtualMachine".to_owned())
}

fn vm() -> Result<&'static VirtualMachine, String> {
    VirtualMachine::get().ok_or_else(|| "g_SkyrimVM + 0x210 is null".to_owned())
}

fn native(name: &str) -> Result<&'static NativeFunction, String> {
    NATIVES.lock().unwrap().iter().rev().find(|(n, _)| *n == name).map(|(_, f)| *f).ok_or_else(|| format!("{name} was not registered"))
}

fn vm_identity() -> Result<String, String> {
    let registry = registry()?;
    let global = vm()?;
    check(core::ptr::eq(registry, global), "the registry pointer is not g_SkyrimVM + 0x210")?;
    let skyrim_vm = skyrim::reloc().address(globals::g_SkyrimVM).map_err(|e| e.to_string())?;
    let policy = registry.raw().handlePolicy as usize;
    let bind = registry.raw().objectBindPolicy as usize;
    check(policy == skyrim_vm + core::mem::offset_of!(classes::partial::SkyrimVM, m_HandlePolicy_338), "VM+0x90 is not g_SkyrimVM + 0x338")?;
    check(bind == skyrim_vm + core::mem::offset_of!(classes::partial::SkyrimVM, m_ObjectBindPolicy_3A8), "VM+0x98 is not g_SkyrimVM + 0x3a8")?;
    let via_slot = registry.handle_policy();
    check(core::ptr::from_ref(via_slot) as usize == policy && HandlePolicy::get().is_some_and(|p| core::ptr::eq(p, via_slot)), "GetObjectHandlePolicy is not VM+0x90")?;
    let global = unsafe { *(skyrim::reloc().address(globals::g_IObjectHandlePolicy_ptr).map_err(|e| e.to_string())? as *const usize) };
    let refs = registry.raw().refCount;
    Ok(format!("registry {:p} = g_SkyrimVM+0x210, refCount {refs}, handle policy {policy:#x} = g_SkyrimVM+0x338 = GetObjectHandlePolicy(), bind policy {bind:#x} = g_SkyrimVM+0x3a8; g_IObjectHandlePolicy_ptr holds {global:#x} during registration", registry.as_ptr()))
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
    check(*form.name() == *"Form" && form.parent().is_none(), &format!("Form type is {form:?}"))?;
    let chain: Vec<String> = core::iter::successors(Some(actor), |t| t.parent()).map(|t| t.name().to_string()).collect();
    check(chain == ["Actor", "ObjectReference", "Form"], &format!("Actor's parent chain is {chain:?}"))?;
    Ok(format!("Form {form:?} at {:p}; Actor chain {}; Actor {actor:?}", form.as_ptr(), chain.join(" -> ")))
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
        vm.set_callable_from_tasklets(CLASS, name, true);
    }
    Ok("SetCallableFromTasklets(SkyrimRsTest, each, true) returned".into())
}

fn handles() -> Result<String, String> {
    let policy = HandlePolicy::get().ok_or("no VM")?;
    let global = unsafe { *(skyrim::reloc().address(globals::g_IObjectHandlePolicy_ptr).map_err(|e| e.to_string())? as *const usize) };
    check(global == core::ptr::from_ref(policy) as usize, &format!("g_IObjectHandlePolicy_ptr holds {global:#x}, not the VM's policy"))?;
    let invalid = policy.invalid_handle();
    check(invalid == Handle::INVALID && invalid.0 == 0xFFFF_0000_0000 && invalid.is_invalid() && invalid.form_id().is_none(), &format!("invalid handle is {invalid}"))?;
    let player = form(PLAYER)?;
    let sword = form(IRON_SWORD)?;
    let h = policy.handle_for_form(player);
    check(h == Handle::form(PLAYER) && h.0 == 0xFFFF_0000_0014 && h.form_id() == Some(PLAYER), &format!("player handle is {h}"))?;
    check(policy.is_type(VMTypeID::Form, h) && policy.is_type(VMTypeID::ObjectReference, h) && policy.is_type(VMTypeID::Actor, h) && !policy.is_type(VMTypeID::Weapon, h), "HandleIsType on the player handle")?;
    check(policy.is_available(h), "the player handle is not available")?;
    let back = policy.form_for_handle(VMTypeID::Actor, h).ok_or("GetObjectForHandle(Actor, player) is null")?;
    check(core::ptr::eq(back, player), "GetObjectForHandle(Actor) is not the player")?;
    check(policy.form_for_handle(VMTypeID::ObjectReference, h).is_some_and(|f| core::ptr::eq(f, player)), "GetObjectForHandle(ObjectReference) is not the player")?;
    check(policy.form_for_handle(VMTypeID::Weapon, h).is_none(), "GetObjectForHandle(Weapon, player) returned a form")?;
    let hs = policy.handle_for_form(sword);
    check(hs == Handle::form(IRON_SWORD) && hs.0 == 0xFFFF_0001_2EB7, &format!("sword handle is {hs}"))?;
    check(policy.form_for_handle(VMTypeID::Weapon, hs).is_some_and(|f| core::ptr::eq(f, sword)), "GetObjectForHandle(Weapon, sword) is not the sword")?;
    check(!policy.is_type(VMTypeID::Actor, hs) && policy.form_for_handle(VMTypeID::Actor, hs).is_none(), "the sword handle passes as an Actor")?;
    check(policy.form_for_handle(VMTypeID::Form, invalid).is_none(), "the invalid handle resolved to a form")?;
    Ok(format!("g_IObjectHandlePolicy_ptr = the VM's policy; invalid={invalid} (HandleIsType(Form)={} IsHandleObjectAvailable={}, no form); player={h} (Form/ObjectReference/Actor yes, Weapon no) -> {back:?}; sword={hs} -> Weapon yes, Actor no", policy.is_type(VMTypeID::Form, invalid), policy.is_available(invalid)))
}

fn script_types() -> Result<String, String> {
    let vm = vm()?;
    let actor = vm.script_type("Actor").ok_or("GetScriptObjectType(Actor) failed")?;
    check(*actor.name() == *"Actor" && actor.is_linked(), &format!("Actor is {actor:?}"))?;
    let missing = vm.script_type("NoSuchScript");
    check(missing.is_none_or(|t| !t.is_linked()), &format!("NoSuchScript resolved to a linked type {missing:?}"))?;
    let own = vm.script_type(CLASS).ok_or("GetScriptObjectType(SkyrimRsTest) failed: is SkyrimRsTest.pex in Data\\Scripts?")?;
    check(own.is_linked() && own.parent().is_none(), &format!("{CLASS} is {own:?}"))?;
    for name in ["Ping", "FormId", "PlayerHealth", "Echo", "Scale", "Player", "Tick", "Describe"] {
        let bound = own.function(name, true).ok_or_else(|| format!("{CLASS}.{name} has no function"))?;
        check(bound == native(name)?.as_ptr(), &format!("{CLASS}.{name} is bound to {bound:?}, not the registered native"))?;
    }
    check(own.function("Nope", true).is_none(), "GetFunction found Nope")?;
    let ping_as_method = own.function("Ping", false);
    let reference = vm.script_type(REF_CLASS).ok_or("GetScriptObjectType(SkyrimRsTestRef) failed")?;
    check(reference.is_linked() && reference.parent().is_some_and(|p| *p.name() == *"ObjectReference"), &format!("{REF_CLASS} is {reference:?}"))?;
    let method = native("SelfFormId")?;
    let found = [false, true].map(|is_static| reference.function("SelfFormId", is_static));
    Ok(format!("Actor {actor:?}; NoSuchScript {missing:?}; {CLASS} {own:?} with 8 natives bound to the registered objects (GetFunction(Ping, isStatic=false) = {ping_as_method:?}); {REF_CLASS} {reference:?} extends ObjectReference; GetFunction(SelfFormId) for isStatic false/true = {found:?} (non-static functions are not returned for a linked type), the native {:?} has refCount {} after linking", method.as_ptr(), method.ref_count()))
}

fn find_bound_object() -> Result<String, String> {
    let vm = vm()?;
    let h = Handle::form(PLAYER);
    let object = vm.find_bound_object(h, "Actor").ok_or("FindBoundObject(player, Actor) failed")?;
    check(object.handle() == h, &format!("the Actor object's handle is {}", object.handle()))?;
    check(object.type_info().is_some_and(|t| *t.name() == *"Actor"), "the object's type is not Actor")?;
    check(vm.find_bound_object(h, REF_CLASS).is_none(), "FindBoundObject(player, SkyrimRsTestRef) found an object")?;
    check(vm.find_bound_object(Handle::INVALID, "Actor").is_none(), "FindBoundObject(invalid, Actor) found an object")?;
    Ok(format!("player -> {object:?}; SkyrimRsTestRef none; invalid none"))
}

fn bind_object() -> Result<String, String> {
    let vm = vm()?;
    let thread = unsafe { GameThread::assume() };
    let h = Handle::form(PLAYER);
    let created = vm.create_object(REF_CLASS).ok_or("CreateObject(SkyrimRsTestRef) failed")?;
    check(created.handle() != h && created.type_info().is_some_and(|t| *t.name() == *REF_CLASS), &format!("the new object is {created:?}"))?;
    vm.bind_object(&thread, &created, h);
    let bound = vm.find_bound_object(h, REF_CLASS).ok_or("FindBoundObject(player, SkyrimRsTestRef) found nothing after BindObject")?;
    check(core::ptr::from_ref(&*bound) == core::ptr::from_ref(&*created) && bound.handle() == h, &format!("found {bound:?}, created {created:?}"))?;
    Ok(format!("created {REF_CLASS} unbound, BindObject(player) -> FindBoundObject returns it: {bound:?}"))
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
    Ok(format!("{} calls: {ping}; {player_id}; {health}; every expected call seen", calls.len()))
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
    Ok(format!("from inside Ping, twice: DispatchStaticCall(Tick) and (Scale, [1.5, true]) and DispatchMethodCall(bound SkyrimRsTestRef, SelfFormId) all returned true; Tick ran {ticks}, Scale(1.5, true) {scaled}; the native method SelfFormId ran {methods} times (2 dispatched and DevBench's call; request 4d)"))
}

fn stack_ids() -> Result<String, String> {
    let calls = CALLS.lock().unwrap();
    let ids: Vec<u32> = calls.iter().map(|c| c.stack_id).collect();
    check(ids.iter().all(|&id| id != 0), &format!("a call had stack id 0: {ids:?}"))?;
    let sorted = ids.windows(2).all(|w| w[0] < w[1]);
    check(sorted, &format!("stack ids are not increasing: {ids:?}"))?;
    Ok(format!("{} calls with increasing stack ids {ids:?}", ids.len()))
}
