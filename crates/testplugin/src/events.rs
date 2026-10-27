//! One test per `skyrim::events` API. `SETUP` runs at `DataLoaded`: the holder and UI structure,
//! add/remove on a scratch sink, the `BSFixedString` round trip, then the registration of the
//! recorders. `TESTS` runs at the save that ends the scenario of the RE repo's
//! `docs/subsystems/events-verify.md` and checks what the recorders received, in order.

use core::marker::PhantomData;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use sre::raw::globals;
use skyrim::actors::{PlayerCharacter, RefHandle};
use skyrim::events::{
    BGSActorCellEvent, BGSFootstepEvent, BSEventNotifyControl, BSFixedString, BSTEventSource, Event, EventSink, EventSourceHolderMenuOpenClose, EventSourceHolderTes,
    InputEventList, MenuModeChangeEvent, MenuOpenCloseEvent, Registration, Sink, TESActivateEvent, TESActorLocationChangeEvent, TESCellAttachDetachEvent, TESCombatEvent,
    TESContainerChangedEvent, TESEquipEvent, TESFurnitureEvent, TESHitEvent, TESInitScriptEvent, TESLoadGameEvent, TESLockChangedEvent, TESObjectLoadedEvent, TESPackageEvent,
};
use skyrim::forms::{FormId, find_form_by_editor_id};

use crate::{PLAYER, Test, check};

const GOLD: FormId = FormId(0xF);
const TORCH: FormId = FormId(0x1D4EC);
const FLAMES: FormId = FormId(0x12FCD);
const FLAMES_PROJECTILE: FormId = FormId(0x12FCF);
const RIVERWOOD_CELL: FormId = FormId(0x9732);
const MORA_TAPINELLA: FormId = FormId(0xECE8D);

/// Game loads since `DataLoaded`, counted by the SKSE message handler.
pub static LOADS: AtomicU32 = AtomicU32::new(0);

pub const SETUP: &[(&str, Test)] = &[("holder", holder), ("ui", ui), ("add_remove", add_remove), ("fixed_string", fixed_string), ("register", register)];

pub const TESTS: &[(&str, Test)] = &[
    ("source_identity", source_identity),
    ("menu_open_close", menu_open_close),
    ("menu_mode_change", menu_mode_change),
    ("equip", equip),
    ("container_changed", container_changed),
    ("remove_sink", remove_sink),
    ("activate", activate),
    ("hit", hit),
    ("load_game", load_game),
    ("cell_attach_detach", cell_attach_detach),
    ("actor_cell", actor_cell),
    ("object_loaded", object_loaded),
    ("footsteps", footsteps),
    ("npc_events", npc_events),
    ("fixed_string", fixed_string),
];

struct Record {
    kind: &'static str,
    detail: String,
    source_ok: bool,
}

static RECEIVED: Mutex<Vec<Record>> = Mutex::new(Vec::new());

/// Remembers every event of `E` it receives for the tests and logs the first `logged` of them
/// (every one when `logged` is `u32::MAX`).
struct Recorder<E> {
    kind: &'static str,
    logged: u32,
    seen: AtomicU32,
    _event: PhantomData<fn(&E)>,
}

impl<E> Recorder<E> {
    const fn new(kind: &'static str, logged: u32) -> Self {
        Self { kind, logged, seen: AtomicU32::new(0), _event: PhantomData }
    }
}

impl<E: Event + fmt::Debug> EventSink<E> for Recorder<E> {
    fn process(&self, event: &E, source: &BSTEventSource<E>) -> BSEventNotifyControl {
        let detail = format!("{event:?}");
        if self.seen.fetch_add(1, Ordering::Relaxed) < self.logged {
            log!("event {}: {detail}", self.kind);
        }
        let source_ok = E::source().is_some_and(|s| core::ptr::eq(s, source));
        RECEIVED.lock().unwrap().push(Record { kind: self.kind, detail, source_ok });
        BSEventNotifyControl::Continue
    }
}

/// A `TESContainerChangedEvent` sink that unregisters itself from inside its handler when the
/// scenario's `AddItem(gold, 10)` arrives.
struct SelfRemoving;

static CONTAINER_B_REGISTRATION: Mutex<Option<Registration<TESContainerChangedEvent>>> = Mutex::new(None);

impl EventSink<TESContainerChangedEvent> for SelfRemoving {
    fn process(&self, event: &TESContainerChangedEvent, source: &BSTEventSource<TESContainerChangedEvent>) -> BSEventNotifyControl {
        RECEIVED.lock().unwrap().push(Record { kind: "container B", detail: format!("{event:?}"), source_ok: true });
        if event.item_count() == 10 && event.new_container() == PLAYER {
            let registration = CONTAINER_B_REGISTRATION.lock().unwrap().take();
            drop(registration);
            log!("sink B removed itself while notifying={}: pendingUnregisterCount={} sinkCount={}", source.is_notifying(), source.raw().pendingUnregisterCount, source.sink_count());
        }
        BSEventNotifyControl::Continue
    }
}

const ALL: u32 = u32::MAX;

/// Records the footstep events of the global source: the actor's handle and the tag.
struct FootstepProbe {
    seen: AtomicU32,
}

impl EventSink<BGSFootstepEvent> for FootstepProbe {
    fn process(&self, event: &BGSFootstepEvent, source: &BSTEventSource<BGSFootstepEvent>) -> BSEventNotifyControl {
        let detail = format!("{} {}", event.actor_handle(), event.tag().to_str().unwrap_or("?"));
        if self.seen.fetch_add(1, Ordering::Relaxed) < 3 {
            log!("event footstep: {event:?}");
        }
        let source_ok = BGSFootstepEvent::source().is_some_and(|s| core::ptr::eq(s, source));
        RECEIVED.lock().unwrap().push(Record { kind: "footstep", detail, source_ok });
        BSEventNotifyControl::Continue
    }
}

static MENU: Sink<MenuOpenCloseEvent, Recorder<MenuOpenCloseEvent>> = Sink::new(Recorder::new("menu", ALL));
static MENU_MODE: Sink<MenuModeChangeEvent, Recorder<MenuModeChangeEvent>> = Sink::new(Recorder::new("menu mode", ALL));
static EQUIP: Sink<TESEquipEvent, Recorder<TESEquipEvent>> = Sink::new(Recorder::new("equip", ALL));
static CONTAINER_A: Sink<TESContainerChangedEvent, Recorder<TESContainerChangedEvent>> = Sink::new(Recorder::new("container A", ALL));
static CONTAINER_B: Sink<TESContainerChangedEvent, SelfRemoving> = Sink::new(SelfRemoving);
static ACTIVATE: Sink<TESActivateEvent, Recorder<TESActivateEvent>> = Sink::new(Recorder::new("activate", ALL));
static HIT: Sink<TESHitEvent, Recorder<TESHitEvent>> = Sink::new(Recorder::new("hit", ALL));
static LOAD_GAME: Sink<TESLoadGameEvent, Recorder<TESLoadGameEvent>> = Sink::new(Recorder::new("load game", ALL));
static CELL: Sink<TESCellAttachDetachEvent, Recorder<TESCellAttachDetachEvent>> = Sink::new(Recorder::new("cell", 3));
static ACTOR_CELL: Sink<BGSActorCellEvent, Recorder<BGSActorCellEvent>> = Sink::new(Recorder::new("actor cell", ALL));
static OBJECT_LOADED: Sink<TESObjectLoadedEvent, Recorder<TESObjectLoadedEvent>> = Sink::new(Recorder::new("object loaded", 3));
static FOOTSTEP: Sink<BGSFootstepEvent, FootstepProbe> = Sink::new(FootstepProbe { seen: AtomicU32::new(0) });
static INIT_SCRIPT: Sink<TESInitScriptEvent, Recorder<TESInitScriptEvent>> = Sink::new(Recorder::new("init script", 3));
static COMBAT: Sink<TESCombatEvent, Recorder<TESCombatEvent>> = Sink::new(Recorder::new("combat", ALL));
static FURNITURE: Sink<TESFurnitureEvent, Recorder<TESFurnitureEvent>> = Sink::new(Recorder::new("furniture", ALL));
static PACKAGE: Sink<TESPackageEvent, Recorder<TESPackageEvent>> = Sink::new(Recorder::new("package", 3));
static LOCATION: Sink<TESActorLocationChangeEvent, Recorder<TESActorLocationChangeEvent>> = Sink::new(Recorder::new("location", 3));
static LOCK: Sink<TESLockChangedEvent, Recorder<TESLockChangedEvent>> = Sink::new(Recorder::new("lock", ALL));
static INPUT: Sink<InputEventList, Recorder<InputEventList>> = Sink::new(Recorder::new("input", 3));
static SCRATCH: Sink<TESEquipEvent, Recorder<TESEquipEvent>> = Sink::new(Recorder::new("scratch", 0));

fn source<E: Event>(name: &str) -> Result<&'static BSTEventSource<E>, String> {
    E::source().ok_or_else(|| format!("{name}::source() is None"))
}

fn records(kind: &str) -> Vec<String> {
    RECEIVED.lock().unwrap().iter().filter(|r| r.kind == kind).map(|r| r.detail.clone()).collect()
}

fn count(kind: &str) -> usize {
    RECEIVED.lock().unwrap().iter().filter(|r| r.kind == kind).count()
}

fn holder() -> Result<String, String> {
    let holder = EventSourceHolderTes::get();
    let global = skyrim::reloc().address(globals::g_EventSourceHolder_TES).map_err(|e| e.to_string())?;
    check(core::ptr::from_ref(holder.raw()) as usize == global, "GetSingleton did not return g_EventSourceHolder_TES")?;
    let counts: Vec<u32> = (0..56).map(|i| holder.source_at(i).map_or(0, |s| s.sinkCount)).collect();
    let empty: Vec<usize> = (1..56).filter(|&i| counts[i] == 0).collect();
    check(empty.len() <= 2, &format!("sources without a sink at the main menu: {empty:?}"))?;
    let vm = skyrim::reloc().address(globals::g_SkyrimVM).map_err(|e| e.to_string())?;
    let activate = source::<TESActivateEvent>("TESActivateEvent")?;
    check(activate.sinks().first().copied() == Some((vm + 0x10) as *const _), "sinks[0] of the TESActivateEvent source is not g_SkyrimVM + 0x10")?;
    let equip = source::<TESEquipEvent>("TESEquipEvent")?;
    check(core::ptr::from_ref(equip.raw()) as usize == global + 0x478, "the TESEquipEvent source is not holder + 0x478")?;
    check(holder.source_at(56).is_none(), "source_at(56) returned a source")?;
    Ok(format!("holder at {global:#x} = g_EventSourceHolder_TES, sink counts {counts:?} (none at {empty:?}); TESActivateEvent sinks[0] = g_SkyrimVM + 0x10, TESEquipEvent source = holder + 0x478"))
}

fn ui() -> Result<String, String> {
    let ui = EventSourceHolderMenuOpenClose::get().ok_or("g_EventSourceHolder_MenuOpenClose_ptr is null")?;
    let open_close = source::<MenuOpenCloseEvent>("MenuOpenCloseEvent")?;
    let mode = source::<MenuModeChangeEvent>("MenuModeChangeEvent")?;
    let base = core::ptr::from_ref(ui.raw()) as usize;
    check(core::ptr::from_ref(open_close.raw()) as usize == base + 8 && core::ptr::from_ref(mode.raw()) as usize == base + 0x60, "the UI sources are not at +8 and +0x60")?;
    let counts = [open_close.sink_count(), mode.sink_count(), ui.raw().source_unknown_B8.sinkCount];
    // The script extender adds a sink of its own to the open/close source; the standalone host does not.
    let skse = !unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(windows_sys::w!("skse64_1_7_104.dll")) }.is_null();
    let expected = [5 + u32::from(skse), 4, 0];
    check(counts == expected, &format!("UI sink counts {counts:?}, expected {expected:?} at the main menu with DevBench (script extender loaded: {skse})"))?;
    let input = source::<InputEventList>("InputEventList")?;
    let footstep = source::<BGSFootstepEvent>("BGSFootstepEvent")?;
    let player = PlayerCharacter::get().ok_or("no player")?;
    let actor_cell = source::<BGSActorCellEvent>("BGSActorCellEvent")?;
    check(core::ptr::from_ref(actor_cell.raw()) as usize == player.as_ptr() as usize + 0x2E0, "the BGSActorCellEvent source is not player + 0x2e0")?;
    Ok(format!(
        "UI at {base:#x}: sinks +8={} +0x60={} +0xb8={} numPausesGame={}; input source sinks={}, footstep source sinks={}, player BGSActorCellEvent source sinks={}",
        counts[0],
        counts[1],
        counts[2],
        ui.num_pauses_game(),
        input.sink_count(),
        footstep.sink_count(),
        actor_cell.sink_count()
    ))
}

fn add_remove() -> Result<String, String> {
    let src = source::<TESEquipEvent>("TESEquipEvent")?;
    let before = src.sink_count();
    let first = src.add_sink(&SCRATCH);
    check(src.sink_count() == before + 1 && src.contains(&SCRATCH), "AddEventSink did not append the sink")?;
    check(src.sinks().last().copied() == Some(core::ptr::from_ref(&SCRATCH).cast()), "the sink is not the last entry")?;
    let second = src.add_sink(&SCRATCH);
    check(src.sink_count() == before + 1, "a second AddEventSink added the sink again")?;
    second.leak();
    drop(first);
    check(src.sink_count() == before && !src.contains(&SCRATCH), "RemoveEventSink did not restore the array")?;
    Ok(format!("TESEquipEvent sinks {before} -> {} -> {} (duplicate ignored) -> {before}", before + 1, before + 1))
}

fn fixed_string() -> Result<String, String> {
    let keyword = find_form_by_editor_id("VendorItemWeapon").and_then(|f| f.as_keyword()).ok_or("VendorItemWeapon did not resolve")?;
    let name = keyword.name();
    let before = name.ref_count();
    let s = BSFixedString::new("VendorItemWeapon");
    let header = s.entry().map(|e| format!("next={:p} flags={:#x} length={}", e.next, e.flags, e.length));
    check(s.to_str() == Some("VendorItemWeapon") && s.len() == 16 && !s.is_empty(), &format!("constructed {s:?}, header {header:?}"))?;
    check(!s.is_wide(), &format!("the entry is flagged wide: {header:?}"))?;
    check(s.ptr_eq(name) && s == *name, "the constructed string is not the keyword's entry")?;
    let pinned = before == skyrim::string::PINNED;
    let after = s.ref_count();
    let copy = s.clone();
    let after_copy = s.ref_count();
    drop(copy);
    let after_drop = s.ref_count();
    if !pinned {
        check(after == before + 1 && after_copy == before + 2 && after_drop == before + 1, &format!("refcount {before} -> {after} -> {after_copy} -> {after_drop}"))?;
    }
    drop(s);
    let released = name.ref_count();
    check(pinned || released == before, &format!("refcount after release {released}, before {before}"))?;
    let folded = BSFixedString::new("vendoritemweapon");
    check(folded.ptr_eq(name), "interning is not case-folded")?;
    let fresh = BSFixedString::new("skyrim-rs events probe");
    check(fresh.ref_count() == 1 && fresh.len() == 22, &format!("fresh entry {fresh:?} refcount {}", fresh.ref_count()))?;
    let empty = BSFixedString::new("");
    check(!empty.ptr_eq(&fresh), "the empty string shares the probe's entry")?;
    Ok(format!(
        "VendorItemWeapon interned at {:p} ({}): len 16, narrow, refcount {before} -> {after} -> {after_copy} -> {after_drop} -> {released}{}; case-folded lookup shares the entry; fresh entry refcount 1; \"\" -> {empty:?}",
        name.as_ptr(),
        header.unwrap_or_default(),
        if pinned { " (pinned)" } else { "" }
    ))
}

fn register() -> Result<String, String> {
    source::<MenuOpenCloseEvent>("MenuOpenCloseEvent")?.add_sink(&MENU).leak();
    source::<MenuModeChangeEvent>("MenuModeChangeEvent")?.add_sink(&MENU_MODE).leak();
    source::<TESEquipEvent>("TESEquipEvent")?.add_sink(&EQUIP).leak();
    let container = source::<TESContainerChangedEvent>("TESContainerChangedEvent")?;
    container.add_sink(&CONTAINER_A).leak();
    *CONTAINER_B_REGISTRATION.lock().unwrap() = Some(container.add_sink(&CONTAINER_B));
    source::<TESActivateEvent>("TESActivateEvent")?.add_sink(&ACTIVATE).leak();
    source::<TESHitEvent>("TESHitEvent")?.add_sink(&HIT).leak();
    source::<TESLoadGameEvent>("TESLoadGameEvent")?.add_sink(&LOAD_GAME).leak();
    source::<TESCellAttachDetachEvent>("TESCellAttachDetachEvent")?.add_sink(&CELL).leak();
    source::<BGSActorCellEvent>("BGSActorCellEvent")?.add_sink(&ACTOR_CELL).leak();
    source::<TESObjectLoadedEvent>("TESObjectLoadedEvent")?.add_sink(&OBJECT_LOADED).leak();
    source::<BGSFootstepEvent>("BGSFootstepEvent")?.add_sink(&FOOTSTEP).leak();
    source::<TESInitScriptEvent>("TESInitScriptEvent")?.add_sink(&INIT_SCRIPT).leak();
    source::<TESCombatEvent>("TESCombatEvent")?.add_sink(&COMBAT).leak();
    source::<TESFurnitureEvent>("TESFurnitureEvent")?.add_sink(&FURNITURE).leak();
    source::<TESPackageEvent>("TESPackageEvent")?.add_sink(&PACKAGE).leak();
    source::<TESActorLocationChangeEvent>("TESActorLocationChangeEvent")?.add_sink(&LOCATION).leak();
    source::<TESLockChangedEvent>("TESLockChangedEvent")?.add_sink(&LOCK).leak();
    source::<InputEventList>("InputEventList")?.add_sink(&INPUT).leak();
    check(container.sink_count() >= 2 && container.contains(&CONTAINER_A) && container.contains(&CONTAINER_B), "the two container sinks are not registered")?;
    Ok(format!("20 sinks registered; TESContainerChangedEvent sinks={} MenuOpenCloseEvent sinks={}", container.sink_count(), source::<MenuOpenCloseEvent>("")?.sink_count()))
}

fn source_identity() -> Result<String, String> {
    let received = RECEIVED.lock().unwrap();
    let mismatched: Vec<&Record> = received.iter().filter(|r| !r.source_ok).collect();
    if let Some(first) = mismatched.first() {
        return Err(format!("{} events arrived from another source, first {}: {}", mismatched.len(), first.kind, first.detail));
    }
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    for r in received.iter() {
        match kinds.iter_mut().find(|k| k.0 == r.kind) {
            Some(k) => k.1 += 1,
            None => kinds.push((r.kind, 1)),
        }
    }
    Ok(format!("{} events, every one from the source its type names: {}", received.len(), kinds.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join(" ")))
}

fn menu_sequence(kind: &str, flag: &str) -> Vec<String> {
    records(kind)
        .iter()
        .filter_map(|d| {
            let name = d.split('"').nth(1)?;
            let value = d.split(&format!("{flag}: ")).nth(1)?.trim_end_matches(" }");
            Some(format!("{name}:{}", u8::from(value == "true")))
        })
        .collect()
}

fn pointer_of(detail: &str) -> Option<&str> {
    detail.split('@').nth(1)?.split(',').next()
}

fn menu_open_close() -> Result<String, String> {
    let menus = records("menu");
    let open = menus.iter().find(|d| d.starts_with("MenuOpenCloseEvent { menuName: \"TweenMenu\"@") && d.ends_with("opening: true }")).ok_or("no TweenMenu open")?;
    let close = menus.iter().find(|d| d.starts_with("MenuOpenCloseEvent { menuName: \"TweenMenu\"@") && d.ends_with("opening: false }")).ok_or("no TweenMenu close")?;
    check(menus.iter().position(|d| d == open) < menus.iter().position(|d| d == close), "the close came before the open")?;
    let pointer = pointer_of(open).ok_or("no pointer in the record")?;
    check(pointer_of(close) == Some(pointer), "open and close carried different menuName pointers")?;
    let interned = BSFixedString::new("TweenMenu");
    check(format!("{:p}", interned.as_ptr()) == pointer, &format!("BSFixedString::new(\"TweenMenu\") is at {:p}, the event's name at {pointer}", interned.as_ptr()))?;
    let sequence = menu_sequence("menu", "opening");
    Ok(format!("{} menu events; TweenMenu open then close with the same interned name {pointer} = BSFixedString::new(\"TweenMenu\"); sequence=[{}]", menus.len(), sequence.join(",")))
}

fn menu_mode_change() -> Result<String, String> {
    let sequence = menu_sequence("menu mode", "inMenuMode");
    let enter = sequence.iter().position(|s| s == "TweenMenu:1").ok_or("no TweenMenu enter")?;
    let leave = sequence.iter().position(|s| s == "TweenMenu:0").ok_or("no TweenMenu leave")?;
    check(enter < leave, "the leave came before the enter")?;
    check(sequence.contains(&"Loading Menu:1".to_owned()) && sequence.contains(&"Loading Menu:0".to_owned()), "no Loading Menu transitions")?;
    Ok(format!("{} mode changes; TweenMenu 1 then 0; sequence=[{}]", sequence.len(), sequence.join(",")))
}

fn equip() -> Result<String, String> {
    let player: Vec<String> = records("equip").into_iter().filter(|d| d.contains("actor: ACHR 00000014")).collect();
    let on = player.iter().position(|d| d.contains("baseObject: 0001D4EC, originalRefr: 00000000, uniqueID: 0, equipped: true")).ok_or("no torch equip by the player")?;
    let off = player.iter().position(|d| d.contains("baseObject: 0001D4EC, originalRefr: 00000000, uniqueID: 0, equipped: false")).ok_or("no torch unequip by the player")?;
    check(on < off, "the unequip came before the equip")?;
    Ok(format!("{} equip events by the player, {} in all; torch {TORCH} equipped then unequipped: {} | {}", player.len(), count("equip"), player[on], player[off]))
}

fn container_changed() -> Result<String, String> {
    let a = records("container A");
    let added = a.iter().position(|d| d.contains("oldContainer: 00000000, newContainer: 00000014, baseObject: 0000000F, itemCount: 10, reference: 00000000, uniqueID: 0")).ok_or("no AddItem(gold, 10)")?;
    let removed = a.iter().position(|d| d.contains("oldContainer: 00000014, newContainer: 00000000, baseObject: 0000000F, itemCount: 5, reference: 00000000, uniqueID: 0")).ok_or("no RemoveItem(gold, 5)")?;
    check(added < removed, "the removal came before the addition")?;
    Ok(format!("{} container events; gold {GOLD} +10 then -5 on the player: {} | {}", a.len(), a[added], a[removed]))
}

fn remove_sink() -> Result<String, String> {
    let b = records("container B");
    check(b.iter().any(|d| d.contains("itemCount: 10")), "sink B never saw the AddItem(gold, 10)")?;
    check(!b.iter().any(|d| d.contains("itemCount: 5")), "sink B still received the RemoveItem(gold, 5) after unregistering")?;
    check(CONTAINER_B_REGISTRATION.lock().unwrap().is_none(), "sink B's registration is still held")?;
    let src = source::<TESContainerChangedEvent>("TESContainerChangedEvent")?;
    check(!src.contains(&CONTAINER_B) && src.contains(&CONTAINER_A), "the source still lists sink B, or lost sink A")?;
    let last = b.last().ok_or("no records")?;
    check(last.contains("itemCount: 10"), &format!("sink B's last event is not the AddItem: {last}"))?;
    Ok(format!("sink B received {} events up to the AddItem(gold, 10), unregistered itself from inside ProcessEvent and received nothing after; sink A received {}", b.len(), count("container A")))
}

fn activate() -> Result<String, String> {
    let all = records("activate");
    let plant = all.iter().find(|d| d.contains("objectActivated: REFR 000ECE8D") && d.contains("actionRef: ACHR 00000014")).ok_or("no activation of the Mora Tapinella by the player")?;
    check(all.iter().all(|d| !d.contains("objectActivated: null")), "an activate event had a null objectActivated")?;
    let by_npcs = all.iter().filter(|d| !d.contains("actionRef: ACHR 00000014")).count();
    Ok(format!("{} activate events, {by_npcs} by NPCs; {MORA_TAPINELLA} activated by the player: {plant}; first {}", all.len(), all[0]))
}

fn hit() -> Result<String, String> {
    let hits = records("hit");
    let flames: Vec<&String> = hits.iter().filter(|d| d.contains("cause: ACHR 00000014, source: 00012FCD, projectile: 00012FCF, flags: ")).collect();
    check(!flames.is_empty(), &format!("no Flames hit by the player among {} hits", hits.len()))?;
    check(flames.iter().all(|d| !d.contains("target: null")), "a Flames hit had a null target")?;
    let attack_bits = |d: &str| d.rsplit("flags: 0x").next().and_then(|h| u32::from_str_radix(h.trim_end_matches(" }"), 16).ok()).map_or(0xF, |f| f & 0xF);
    check(flames.iter().all(|d| attack_bits(d) == 0), "a Flames hit carried power, sneak, bash or blocked bits")?;
    Ok(format!("{} hit events, {} by the player's Flames {FLAMES} with projectile {FLAMES_PROJECTILE} and no attack flags; first {}", hits.len(), flames.len(), flames[0]))
}

fn load_game() -> Result<String, String> {
    let n = count("load game");
    let loads = LOADS.load(Ordering::Relaxed);
    check(n == 2 && loads == 2, &format!("{n} TESLoadGameEvent for {loads} loads, expected 2 and 2"))?;
    Ok(format!("{n} TESLoadGameEvent for {loads} game loads since DataLoaded"))
}

fn cell_attach_detach() -> Result<String, String> {
    let cells = records("cell");
    let attached = cells.iter().filter(|d| d.ends_with("attached: true }")).count();
    let detached = cells.len() - attached;
    check(attached > 0 && detached > 0, &format!("{attached} attached, {detached} detached"))?;
    let bad: Vec<&String> = cells.iter().filter(|d| !(d.contains("reference: REFR ") || d.contains("reference: ACHR "))).collect();
    if let Some(first) = bad.first() {
        return Err(format!("{} events with a reference that is not REFR or ACHR, first {first}", bad.len()));
    }
    Ok(format!("{} cell events: {attached} attached, {detached} detached, every reference a REFR or ACHR; first {}", cells.len(), cells[0]))
}

fn actor_cell() -> Result<String, String> {
    let events = records("actor cell");
    let player = PlayerCharacter::get().ok_or("no player")?;
    let handle = player.handle();
    let entered = events.iter().position(|d| d == &format!("BGSActorCellEvent {{ actorHandle: {handle}, cell: {RIVERWOOD_CELL}, attached: true }}")).ok_or_else(|| format!("no enter of Riverwood by handle {handle}: {events:?}"))?;
    let left = events.iter().position(|d| d.contains("attached: false")).ok_or("no detach")?;
    Ok(format!("{} actor cell events: entered Riverwood {RIVERWOOD_CELL} with the player's handle {handle} at #{entered}, a detach at #{left}; sequence {events:?}", events.len()))
}

fn object_loaded() -> Result<String, String> {
    let events = records("object loaded");
    let loaded = events.iter().filter(|d| d.ends_with("loaded: true }")).count();
    let unloaded = events.len() - loaded;
    check(loaded > 0, "no object loaded events")?;
    let player: Vec<&String> = events.iter().filter(|d| d.contains("formID: 00000014,")).collect();
    Ok(format!("{} object loaded events: {loaded} loaded, {unloaded} unloaded; the player's: {player:?}; first {}", events.len(), events[0]))
}

/// The footstep source re-sends the graph events whose tag is in its filter list, as
/// `{actor handle, tag}`; the first one must name a live actor and an ASCII tag.
fn footsteps() -> Result<String, String> {
    let events = records("footstep");
    check(!events.is_empty(), "no footstep events through the global source")?;
    let (handle, tag) = events[0].split_once(' ').ok_or_else(|| format!("could not parse {}", events[0]))?;
    let handle = RefHandle(u32::from_str_radix(handle, 16).map_err(|e| format!("handle {handle}: {e}"))?);
    let actor = handle.lookup().ok_or_else(|| format!("{handle} is not a live handle"))?;
    let actor = format!("{:?}", *actor);
    check(!tag.is_empty() && tag.is_ascii(), &format!("tag {tag:?}"))?;
    let distinct = {
        let mut tags: Vec<&str> = events.iter().filter_map(|d| d.split(' ').nth(1)).collect();
        tags.sort_unstable();
        tags.dedup();
        tags.len()
    };
    Ok(format!("{} footstep events, {distinct} distinct tags; first: handle {handle} -> {actor}, tag {tag:?}", events.len()))
}

fn npc_events() -> Result<String, String> {
    let kinds = ["combat", "furniture", "package", "location", "lock", "init script", "input"];
    let counts: Vec<String> = kinds.iter().map(|k| format!("{k}={}", count(k))).collect();
    let null_actor = RECEIVED.lock().unwrap().iter().filter(|r| ["combat", "furniture", "package", "location"].contains(&r.kind) && r.detail.contains("actor: null")).count();
    check(null_actor == 0, &format!("{null_actor} NPC events with a null actor"))?;
    check(count("init script") > 0, "no TESInitScriptEvent")?;
    let input = records("input");
    let empty = input.iter().filter(|d| d.contains("head: 0x0,")).count();
    Ok(format!("{}; every actor non-null; {empty} input notifications with a null head, first {:?}", counts.join(" "), input.first()))
}
