//! Plugin a: runs its `Before` before b's and takes one more point of damage, observes the result
//! in an `After`, replaces the outer `ActorValueOwner_ModActorValue` (delegating to the original),
//! counts `TESObjectREFR::GetScale` calls in an `After` (the site the compat suite's legacy plugin
//! overwrites), writes a managed patch by id and offset (the same bytes, so the game runs on as it
//! was) and reads the dispatcher's diagnostics at each save.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use framework::hooks::ops::{Actor__ActorValueOwner_ModActorValue, Actor__ModActorValueImpl, TESObjectREFR__GetScale};
use framework::hooks::{Hooks, Options, Outcome, Phase, Validation, validate};
use framework::Host;
use sre::raw::fns;

use crate::{A, B, ModArgs, OwnerModArgs, check, log, player_health_damage, run};

static HOOKS: OnceLock<&'static Hooks> = OnceLock::new();
static BEFORE: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static AFTER: Mutex<Vec<(f32, Outcome)>> = Mutex::new(Vec::new());
static REPLACED: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static OUTER_AFTER: Mutex<Vec<(f32, Outcome)>> = Mutex::new(Vec::new());
static CALLS: Mutex<Vec<(i32, f32)>> = Mutex::new(Vec::new());
static SCALE_CALLS: AtomicU64 = AtomicU64::new(0);
static SCALE_AT_DATA_LOADED: AtomicU64 = AtomicU64::new(0);

pub fn load(host: &'static Host) -> bool {
    if !crate::init(host) {
        return false;
    }
    let _ = HOOKS.set(host.hooks());
    run("aLoad", &[("a_pristine", pristine), ("a_scale", scale), ("a_subscribe", subscribe), ("a_patch", patch)]);
    host.on_message(|msg| {
        if msg.kind == skse::MessagingInterface::DATA_LOADED {
            SCALE_AT_DATA_LOADED.store(SCALE_CALLS.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        if msg.kind == skse::MessagingInterface::SAVE_GAME {
            static SAVES: Mutex<u32> = Mutex::new(0);
            let mut saves = SAVES.lock().unwrap();
            *saves += 1;
            match *saves {
                1 => run("aSave1", &[("a_before", before_seen), ("a_after", after_seen), ("a_outer", outer_seen), ("a_scale_calls", scale_calls), ("a_diagnostics", diagnostics)]),
                2 => run("aSave2", &[("a_before_final", before_final), ("a_after_final", after_final), ("a_diagnostics_final", diagnostics_final)]),
                _ => {}
            }
        }
    });
    true
}

/// What an unhooked entry reads as: 1.7.104's file holds its code as loaded, the other runtimes' encrypt it.
fn unhooked() -> Validation {
    if crate::RUNTIME.get() == Some(&skse::runtime::pack(1, 7, 104, 0)) { Validation::Pristine } else { Validation::Plausible }
}

/// The live entry bytes of the functions about to be hooked, and one that is not, against the file on disk.
fn pristine() -> Result<String, String> {
    let reloc = skyrim::reloc();
    let mut results = Vec::new();
    for (name, id) in [("Actor::ModActorValueImpl", sre::id!(fns::Actor__ModActorValueImpl)), ("Actor::ActorValueOwner_ModActorValue", sre::id!(fns::Actor__ActorValueOwner_ModActorValue)), ("TESObjectREFR::GetScale", sre::id!(fns::TESObjectREFR__GetScale))] {
        let address = reloc.address(id).map_err(|e| e.to_string())?;
        results.push((format!("{name} at {address:#x}"), validate(address, None)));
    }
    let detail = results.iter().map(|(name, v)| format!("{name}: {v}")).collect::<Vec<_>>().join("; ");
    check(results.iter().all(|(_, v)| *v == unhooked()), &detail)?;
    Ok(detail)
}

/// A patch stated for 1.7.104 goes through the profile; here it writes what is already there.
fn patch() -> Result<String, String> {
    const OFFSET: usize = 4;
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let at = skyrim::reloc().address(sre::id!(fns::Actor__GetLevel)).map_err(|e| e.to_string())? + OFFSET;
    let bytes = unsafe { core::slice::from_raw_parts(at as *const u8, 2) }.to_vec();
    let id = unsafe { hooks.patch(A, framework::location!(fns::Actor__GetLevel, OFFSET), &bytes, &bytes) }.map_err(|e| e.to_string())?;
    let report = hooks.patches().iter().find(|p| p.id == id).map(ToString::to_string).ok_or("the patch is not reported")?;
    check(hooks.unpatch(id), "the patch could not be removed")?;
    Ok(report)
}

fn scale() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let id = hooks
        .after(TESObjectREFR__GetScale, Options::new(A), |_, _, _| {
            SCALE_CALLS.fetch_add(1, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("after {id:?} on TESObjectREFR::GetScale"))
}

fn scale_calls() -> Result<String, String> {
    let (all, at_data) = (SCALE_CALLS.load(Ordering::Relaxed), SCALE_AT_DATA_LOADED.load(Ordering::Relaxed));
    check(all > at_data, format!("GetScale's After ran {all} times, {at_data} by DATA_LOADED and none since"))?;
    Ok(format!("GetScale's After ran {all} times, {} since DATA_LOADED", all - at_data))
}

fn subscribe() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let before = hooks
        .before(Actor__ModActorValueImpl, Options::new(A).before(&[B]), |args: &mut ModArgs| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                BEFORE.lock().unwrap().push(args.3);
                args.3 -= 1.0;
            }
            if args.2 == sre::ActorValue::Health as u32 && sre::PlayerCharacter::singleton().is_some_and(|p| core::ptr::eq(core::ptr::from_ref(p).cast(), args.0)) {
                CALLS.lock().unwrap().push((args.1, args.3));
            }
        })
        .map_err(|e| e.to_string())?;
    let after = hooks
        .after(Actor__ModActorValueImpl, Options::new(A), |args: &ModArgs, (), outcome| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                AFTER.lock().unwrap().push((args.3, outcome));
            }
        })
        .map_err(|e| e.to_string())?;
    let replace = hooks
        .replace(Actor__ActorValueOwner_ModActorValue, Options::new(A), |args: OwnerModArgs, original| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                REPLACED.lock().unwrap().push(args.3);
            }
            unsafe { original.call(args) }
        })
        .map_err(|e| e.to_string())?;
    let outer_after = hooks
        .after(Actor__ActorValueOwner_ModActorValue, Options::new(A), |args: &OwnerModArgs, (), outcome| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                OUTER_AFTER.lock().unwrap().push((args.3, outcome));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("before {before:?}, after {after:?} on ModActorValueImpl; replace {replace:?}, after {outer_after:?} on ActorValueOwner_ModActorValue"))
}

fn before_seen() -> Result<String, String> {
    let seen = BEFORE.lock().unwrap().clone();
    log(format!("player health calls (modifier, delta after a's Before): {:?}", CALLS.lock().unwrap()));
    check(seen == [-10.0, -5.0], format!("a's Before saw {seen:?}, expected [-10, -5]"))?;
    Ok(format!("a's Before saw deltas {seen:?} and passed on {:?}", seen.iter().map(|d| d - 1.0).collect::<Vec<_>>()))
}

fn after_seen() -> Result<String, String> {
    let seen = AFTER.lock().unwrap().clone();
    check(seen == [(-11.0, Outcome::Original), (-12.0, Outcome::Skipped)], format!("a's After saw {seen:?}, expected [(-11, Original), (-12, Skipped)]"))?;
    Ok(format!("a's After saw {seen:?}"))
}

fn outer_seen() -> Result<String, String> {
    let replaced = REPLACED.lock().unwrap().clone();
    let after = OUTER_AFTER.lock().unwrap().clone();
    check(replaced == [-10.0, -5.0], format!("a's Replace saw {replaced:?}, expected [-10, -5]"))?;
    check(after == [(-10.0, Outcome::Replaced), (-5.0, Outcome::Replaced)], format!("a's outer After saw {after:?}, expected Replaced twice"))?;
    Ok(format!("a's Replace on ActorValueOwner_ModActorValue saw {replaced:?} and delegated; its After saw {after:?}"))
}

fn report(expected: &[(&str, Phase)]) -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let reports = hooks.diagnostics();
    for r in &reports {
        log(format!("diagnostics: {r}"));
    }
    let site = reports.iter().find(|r| r.name == "Actor::ModActorValueImpl").ok_or("no report for Actor::ModActorValueImpl")?;
    let entries: Vec<(&str, Phase)> = site.entries.iter().map(|e| (e.owner.as_str(), e.phase)).collect();
    check(entries == expected, format!("entries {entries:?}, expected {expected:?}"))?;
    check(site.validation == unhooked() && matches!(site.current, Validation::Managed { .. }), format!("validation {}, now {}", site.validation, site.current))?;
    check(site.timing.calls >= 2 && site.entries.iter().all(|e| e.timing.calls >= 2), "fewer than two calls counted")?;
    check(site.last_outcome.is_some(), "no outcome recorded")?;
    Ok(format!("{} entries {entries:?}, {} calls, last outcome {:?}, entry {}", site.name, site.timing.calls, site.last_outcome, site.current))
}

fn diagnostics() -> Result<String, String> {
    report(&[(A, Phase::Before), (B, Phase::Before), (B, Phase::Around), (A, Phase::After)])
}

fn diagnostics_final() -> Result<String, String> {
    report(&[(A, Phase::Before), (A, Phase::After)])
}

fn before_final() -> Result<String, String> {
    let seen = BEFORE.lock().unwrap().clone();
    check(seen == [-10.0, -5.0, -10.0], format!("a's Before saw {seen:?}, expected [-10, -5, -10]"))?;
    Ok(format!("a's Before saw deltas {seen:?}"))
}

fn after_final() -> Result<String, String> {
    let seen = AFTER.lock().unwrap().clone();
    check(seen.last() == Some(&(-11.0, Outcome::Original)), format!("a's After saw {seen:?}, expected to end with (-11, Original)"))?;
    Ok(format!("a's After saw {seen:?}; with b gone the delta reached the original unhalved"))
}

