// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The template for a new mod: an SKSE plugin that, once the game data is loaded, reads the player
//! and a form, logs every menu that opens or closes, and adds a console command.

use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

use skyrim::BSFixedString;
use skyrim::events::{BSTEventSource, Event, EventSink, Sink};
use skyrim::input::{Command, CommandContext, Param};
use skyrim::prelude::*;
use sre::{ActorValue, BSEventNotifyControl, MenuOpenCloseEvent, PlayerCharacter, TESNPC};

skse::plugin! {
    name: "skyrim-rs-sample",
    author: "TwistedModding",
    version: 1,
    load: load,
}

static LOG: Mutex<Option<File>> = Mutex::new(None);

macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            let _ = writeln!(f, $($arg)*);
        }
    };
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-sample") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    if let Err(e) = skyrim::init(skse) {
        log!("{e}");
        return false;
    }
    skse.messaging().is_some_and(|m| m.listen_skse(skse.plugin_handle(), on_message))
}

unsafe extern "C" fn on_message(message: *mut skse::Message) {
    if unsafe { (*message).kind } == skse::MessagingInterface::DATA_LOADED {
        data_loaded();
    }
}

fn data_loaded() {
    log!("{}", describe_player().unwrap_or_else(|| "no player yet".into()));

    if let Some(form) = sre::find_form_by_id(0x8F958) {
        log!("form 0008F958 is {:?}, a {:?}", form.editor_id(), form.get_form_type());
    }

    match MenuOpenCloseEvent::source() {
        Some(menus) => menus.add_sink(&MENUS).leak(),
        None => log!("no menu event source"),
    }

    match skyrim::input::register::<SampleCommand>() {
        Ok(slot) => log!("console command SkyrimRsSample replaces {}", slot.replaced),
        Err(e) => log!("console command not registered: {e}"),
    }
}

/// The player's name, race, health and race keywords.
fn describe_player() -> Option<String> {
    let player = PlayerCharacter::singleton()?;
    let name = player.base_object()?.cast::<TESNPC>()?.as_tes_full_name().full_name()?;
    let race = player.race()?;
    let health = player.as_actor_value_owner().get_actor_value(ActorValue::Health);
    let keywords: Vec<&CStr> = race.keywords().iter().flatten().filter_map(|k| k.editor_id()).collect();
    Some(format!("player {name:?}, race {:?} {:08X}, health {health}, race keywords {keywords:?}", race.editor_id()?, race.formID))
}

struct MenuLogger;

impl EventSink<MenuOpenCloseEvent> for MenuLogger {
    fn process(&self, event: &MenuOpenCloseEvent, _: &BSTEventSource<MenuOpenCloseEvent>) -> BSEventNotifyControl {
        let name = BSFixedString::view(&event.menuName);
        log!("menu {} {}", name.to_str().unwrap_or("?"), if event.opening != 0 { "opened" } else { "closed" });
        BSEventNotifyControl::Continue
    }
}

static MENUS: Sink<MenuOpenCloseEvent, MenuLogger> = Sink::new(MenuLogger);

struct SampleCommand;

impl Command for SampleCommand {
    const NAME: &'static CStr = c"SkyrimRsSample";
    const HELP: &'static CStr = c"skyrim-rs sample: prints the player's name, race and health";
    const PARAMS: &'static [Param] = &[];

    fn execute(_: &mut CommandContext<'_>) -> bool {
        let line = describe_player().unwrap_or_else(|| "no player".into());
        skyrim::input::print(&line);
        log!("command: {line}");
        true
    }
}
