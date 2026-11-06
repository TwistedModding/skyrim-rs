// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Plugin b: runs its `Before` after a's and doubles the damage, halves it back in an `Around`
//! that skips the original when the damage is 12, has a cycle and an exclusive conflict refused,
//! and removes all its subscriptions at the first save.

use std::sync::{Mutex, OnceLock};

use framework::hooks::ops::{Actor__ActorValueOwner_ModActorValue, Actor__ModActorValueImpl};
use framework::hooks::{Error, Hooks, Options};
use framework::Host;

use crate::{A, B, ModArgs, OwnerModArgs, check, player_health_damage, run};

static HOOKS: OnceLock<&'static Hooks> = OnceLock::new();
static BEFORE: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static AROUND: Mutex<Vec<(f32, Option<f32>)>> = Mutex::new(Vec::new());

pub fn load(host: &'static Host) -> bool {
    if !crate::init(host) {
        return false;
    }
    let _ = HOOKS.set(host.hooks());
    run("bLoad", &[("b_subscribe", subscribe), ("b_cycle", cycle), ("b_exclusive", exclusive)]);
    host.on_message(|msg| {
        if msg.kind == skse::MessagingInterface::SAVE_GAME {
            static SAVES: Mutex<u32> = Mutex::new(0);
            let mut saves = SAVES.lock().unwrap();
            *saves += 1;
            match *saves {
                1 => run("bSave1", &[("b_before", before_seen), ("b_around", around_seen), ("b_remove", remove)]),
                2 => run("bSave2", &[("b_silent", silent)]),
                _ => {}
            }
        }
    });
    true
}

fn subscribe() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let before = hooks
        .before(Actor__ModActorValueImpl, Options::new(B).after(&[A]), |args: &mut ModArgs| {
            if player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                BEFORE.lock().unwrap().push(args.3);
                args.3 *= 2.0;
            }
        })
        .map_err(|e| e.to_string())?;
    let around = hooks
        .around(Actor__ModActorValueImpl, Options::new(B), |args: ModArgs, next| {
            if !player_health_damage(args.0.cast(), args.1, args.2, args.3) {
                return unsafe { next.call(args) };
            }
            if (args.3 + 12.0).abs() < 0.001 {
                AROUND.lock().unwrap().push((args.3, None));
                return;
            }
            let halved = (args.0, args.1, args.2, args.3 / 2.0, args.4);
            AROUND.lock().unwrap().push((args.3, Some(halved.3)));
            unsafe { next.call(halved) }
        })
        .map_err(|e| e.to_string())?;
    Ok(format!("before {before:?} after {A}, around {around:?} on ModActorValueImpl"))
}

fn cycle() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let refused = hooks.before(Actor__ModActorValueImpl, Options::new(B).before(&[A]), |_: &mut ModArgs| {});
    let expected = Error::Cycle { site: "Actor::ModActorValueImpl".into(), names: vec![A.into(), B.into()] };
    check(refused == Err(expected.clone()), format!("{refused:?}, expected {expected:?}"))?;
    Ok(format!("a Before of {B} before {A}, while {A}'s is before {B}, refused: {expected}"))
}

fn exclusive() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let refused = hooks.replace(Actor__ActorValueOwner_ModActorValue, Options::new(B), |args: OwnerModArgs, original| unsafe { original.call(args) });
    let expected = Error::Exclusive { site: "Actor::ActorValueOwner_ModActorValue".into(), existing: A.into(), refused: B.into() };
    check(refused == Err(expected.clone()), format!("{refused:?}, expected {expected:?}"))?;
    Ok(format!("a second Replace refused: {expected}"))
}

fn before_seen() -> Result<String, String> {
    let seen = BEFORE.lock().unwrap().clone();
    check(seen == [-11.0, -6.0], format!("b's Before saw {seen:?}, expected [-11, -6] (after a's)"))?;
    Ok(format!("b's Before saw deltas {seen:?} after a's edit and doubled them"))
}

fn around_seen() -> Result<String, String> {
    let seen = AROUND.lock().unwrap().clone();
    check(seen == [(-22.0, Some(-11.0)), (-12.0, None)], format!("b's Around saw {seen:?}, expected [(-22, Some(-11)), (-12, None)]"))?;
    Ok(format!("b's Around saw {seen:?}: halved the first and skipped the original on the second"))
}

fn remove() -> Result<String, String> {
    let hooks = HOOKS.get().ok_or("no hooks")?;
    let removed = hooks.remove_owner(B);
    check(removed == 2, format!("remove_owner removed {removed}, expected 2"))?;
    Ok(format!("remove_owner({B}) removed {removed} subscriptions"))
}

fn silent() -> Result<String, String> {
    let before = BEFORE.lock().unwrap().len();
    let around = AROUND.lock().unwrap().len();
    check(before == 2 && around == 2, format!("b's Before ran {before} and Around {around} times, expected 2 each"))?;
    Ok("b's subscribers did not run after their removal".into())
}

