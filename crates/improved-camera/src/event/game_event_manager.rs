//! `event/GameEventManager`: the object-loaded sink (when an actor's 3D loads, its persistent
//! skeleton and race adjustments go back on; the photo mode reconciles the actor on the next game
//! tick) and the menu sink's two fixes: the body hidden while the console is open in first person
//! unless "Body in Console" is on, and the race menu's closing (a drawn weapon sheathed, the
//! first-person root shown). The menu sink only notes what happened; the frame applies it.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use framework::Host;
use skyrim::BSFixedString;
use skyrim::prelude::*;
use sre::{BSEventNotifyControl, MenuOpenCloseEvent, TESObjectLoadedEvent};

use crate::settings;
use crate::util::skyrim_util as util;

static LOADED_SINK: AtomicBool = AtomicBool::new(false);
static MENU_SINK: AtomicBool = AtomicBool::new(false);

const CONSOLE_NONE: u8 = 0;
const CONSOLE_OPENED: u8 = 1;
const CONSOLE_CLOSED: u8 = 2;
static CONSOLE: AtomicU8 = AtomicU8::new(CONSOLE_NONE);
static RACE_MENU_CLOSED: AtomicBool = AtomicBool::new(false);

/// `Register`'s sinks. The script event holder exists from the data load on, so a first try
/// before it is repeated at `DATA_LOADED`.
pub fn register(host: &Host) {
    if !LOADED_SINK.load(Ordering::Relaxed) {
        let added = host.add_sink::<TESObjectLoadedEvent>(|event, _| {
            if event.loaded != 0 {
                crate::camera::studio::manager::queue_loaded(event.formID);
            }
            BSEventNotifyControl::Continue
        });
        LOADED_SINK.store(added.is_ok(), Ordering::Relaxed);
    }
    if !MENU_SINK.load(Ordering::Relaxed) {
        let added = host.add_sink::<MenuOpenCloseEvent>(|event, _| {
            let name = BSFixedString::view(&event.menuName);
            if name == "Console" {
                CONSOLE.store(if event.opening != 0 { CONSOLE_OPENED } else { CONSOLE_CLOSED }, Ordering::Relaxed);
            } else if name == "RaceSex Menu" && event.opening == 0 {
                RACE_MENU_CLOSED.store(true, Ordering::Relaxed);
            }
            BSEventNotifyControl::Continue
        });
        MENU_SINK.store(added.is_ok(), Ordering::Relaxed);
    }
}

/// The menu fixes noted since the last frame, on the main thread.
pub fn apply_menu_events() {
    let Some(player) = util::player() else { return };
    if RACE_MENU_CLOSED.swap(false, Ordering::Relaxed) {
        if util::is_weapon_drawn(player) {
            player.reset_actor_weapon_state_and_camera();
        }
        if util::is_engine_first_person()
            && let Some(first) = player.first_person_root()
        {
            first.set_hidden(false);
        }
    }
    let console = CONSOLE.swap(CONSOLE_NONE, Ordering::Relaxed);
    let general = settings::profile().General;
    if console == CONSOLE_NONE || !general.bEnableBody || util::is_menu_open("Main Menu") || util::is_menu_open("RaceSex Menu") || !util::is_first_person() {
        return;
    }
    let Some(third) = player.third_person_root() else { return };
    if console == CONSOLE_OPENED && !general.bEnableBodyConsole {
        third.set_hidden(true);
        skyrim::store(&third.fadeAmount, 0.0);
    } else if console == CONSOLE_CLOSED && third.is_hidden() {
        third.set_hidden(false);
        skyrim::store(&third.fadeAmount, 1.0);
    }
}
