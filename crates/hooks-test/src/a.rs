//! Plugin a: runs its `Before` before b's and takes one more point of damage, observes the result
//! in an `After`, replaces the outer `ActorValueOwner_ModActorValue` (delegating to the original)
//! and reads the dispatcher's diagnostics at each save.

use std::sync::{Mutex, OnceLock};

use framework::hooks::ops::{Actor__ActorValueOwner_ModActorValue, Actor__ModActorValueImpl};
use framework::hooks::{Hooks, Options, Outcome, Phase, Validation, validate};
use framework::Host;
use layout::v1_7_104::fns;

use crate::{A, B, ModArgs, OwnerModArgs, check, log, player_health_damage, run};

static HOOKS: OnceLock<&'static Hooks> = OnceLock::new();
static BEFORE: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static AFTER: Mutex<Vec<(f32, Outcome)>> = Mutex::new(Vec::new());
static REPLACED: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static OUTER_AFTER: Mutex<Vec<(f32, Outcome)>> = Mutex::new(Vec::new());
static CALLS: Mutex<Vec<(i32, f32)>> = Mutex::new(Vec::new());

pub fn load(host: &'static Host) -> bool {
    if !crate::init(host) {
        return false;
    }
    let _ = HOOKS.set(host.hooks());
    run("aLoad", &[("a_pristine", pristine), ("a_subscribe", subscribe)]);
    host.on_message(|msg| {
        if msg.kind == skse::MessagingInterface::SAVE_GAME {
            static SAVES: Mutex<u32> = Mutex::new(0);
            let mut saves = SAVES.lock().unwrap();
            *saves += 1;
            match *saves {
                1 => run("aSave1", &[("a_before", before_seen), ("a_after", after_seen), ("a_outer", outer_seen), ("a_diagnostics", diagnostics)]),
                2 => run("aSave2", &[("a_before_final", before_final), ("a_after_final", after_final), ("a_diagnostics_final", diagnostics_final)]),
                _ => {}
            }
        }
    });
    true
}

/// The live entry bytes of the functions about to be hooked, and one that is not, against the file on disk.
fn pristine() -> Result<String, String> {
    let reloc = skyrim::reloc();
    let mut results = Vec::new();
    for (name, id) in [("Actor::ModActorValueImpl", fns::Actor__ModActorValueImpl), ("Actor::ActorValueOwner_ModActorValue", fns::Actor__ActorValueOwner_ModActorValue), ("TESObjectREFR::GetScale", fns::TESObjectREFR__GetScale)] {
        let address = reloc.address(id).map_err(|e| e.to_string())?;
        results.push((format!("{name} at {address:#x}"), validate(address, None)));
    }
    let detail = results.iter().map(|(name, v)| format!("{name}: {v}")).collect::<Vec<_>>().join("; ");
    check(results.iter().all(|(_, v)| *v == Validation::Pristine), &detail)?;
    Ok(detail)
}

fn subscribe() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let before = hooks
        .before(Actor__ModActorValueImpl, Options::new(A).before(&[B]), |args: &mut ModArgs| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                BEFORE.lock().unwrap().push(args.3);
                args.3 -= 1.0;
            }
            if args.2 == skyrim::actors::ActorValue::Health as i32 && skyrim::actors::PlayerCharacter::get().is_some_and(|p| core::ptr::eq(p.as_ptr().cast(), args.0)) {
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
    check(site.validation == Validation::Pristine && matches!(site.current, Validation::Managed { .. }), format!("validation {}, now {}", site.validation, site.current))?;
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

