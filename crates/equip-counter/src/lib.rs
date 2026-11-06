// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Equip counter: the first mod on skyrim-rs. Logs every item the player equips through a
//! `TESEquipEvent` sink and exposes the count and the last item to scripts as
//! `SkyrimRsEquipCounter.GetEquipCount()` and `GetLastEquipped()` (`papyrus/SkyrimRsEquipCounter.psc`).

use core::ffi::c_void;
use std::fs::File;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use skyrim::events::{BSTEventSource, Event, EventSink, Sink};
use skyrim::papyrus::VirtualMachine;
use skyrim::prelude::*;
use sre::{BSEventNotifyControl, TESEquipEvent, TESForm};

skse::plugin! {
    name: "skyrim-rs-equip-counter",
    author: "TwistedModding",
    version: 1,
    load: load,
}

const PLAYER: u32 = 0x14;
const CLASS: &str = "SkyrimRsEquipCounter";

static LOG: Mutex<Option<File>> = Mutex::new(None);
static COUNT: AtomicU32 = AtomicU32::new(0);
static LAST: AtomicU32 = AtomicU32::new(0);

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg)*);
        }
    };
}

struct Counter;

impl EventSink<TESEquipEvent> for Counter {
    fn process(&self, event: &TESEquipEvent, _: &BSTEventSource<TESEquipEvent>) -> BSEventNotifyControl {
        if event.equipped != 0 && event.actor().is_some_and(|a| a.formID == PLAYER) {
            let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            LAST.store(event.baseObject, Ordering::Relaxed);
            log!("equip {count}: {:08X}", event.baseObject);
        }
        BSEventNotifyControl::Continue
    }
}

static SINK: Sink<TESEquipEvent, Counter> = Sink::new(Counter);

fn get_equip_count() -> i32 {
    i32::try_from(COUNT.load(Ordering::Relaxed)).unwrap_or(i32::MAX)
}

fn get_last_equipped() -> Option<&'static TESForm> {
    sre::find_form_by_id(LAST.load(Ordering::Relaxed))
}

unsafe extern "C" fn on_register(registry: *mut c_void) {
    let Some(vm) = (unsafe { VirtualMachine::from_registry(registry) }) else {
        log!("the registry is not the VM");
        return;
    };
    for (name, result) in [
        ("GetEquipCount", vm.register_native(CLASS, "GetEquipCount", get_equip_count).map(|_| ())),
        ("GetLastEquipped", vm.register_native(CLASS, "GetLastEquipped", get_last_equipped).map(|_| ())),
    ] {
        match result {
            Ok(()) => log!("registered {CLASS}.{name}"),
            Err(e) => log!("failed to register {CLASS}.{name}: {e}"),
        }
    }
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    if unsafe { (*msg).kind } == skse::MessagingInterface::DATA_LOADED {
        match TESEquipEvent::source() {
            Some(source) => {
                source.add_sink(&SINK).leak();
                log!("listening for TESEquipEvent");
            }
            None => log!("no TESEquipEvent source"),
        }
    }
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-equip-counter") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    if let Err(e) = skyrim::init(skse) {
        log!("skyrim::init failed: {e}");
        return false;
    }
    let (Some(messaging), Some(papyrus)) = (skse.messaging(), skse.papyrus()) else {
        log!("no messaging or papyrus interface");
        return false;
    };
    messaging.listen_skse(skse.plugin_handle(), on_message) && papyrus.register(on_register)
}
