//! `hook/GraphicsHook`'s HUD post display (slot 6 of the HUD and journal menus): during dialogue
//! the HUD messages move out of the conversation (`Dialogue.bRepositionHUDMessages`) and come back
//! after it; while a menu pauses the game the model update stops, so the body is frozen where it
//! stood (the map and race menus run their own cameras).

use std::sync::Mutex;

use framework::Host;
use framework::hooks::ops::Vtable;
use framework::hooks::{Error, Location, Options};
use skyrim::prelude::*;
use skyrim::ui::Value;
use sre::IMenu;
use sre::raw::vtables;

use crate::body::manager::update_animation_third_person;
use crate::settings;
use crate::util::skyrim_util as util;

const POST_DISPLAY: usize = 6;

type PostDisplay = unsafe extern "C" fn(usize) -> usize;

const MESSAGES_X: &str = "_root.HUDMovieBaseInstance.MessagesBlock._x";
const MESSAGES_Y: &str = "_root.HUDMovieBaseInstance.MessagesBlock._y";
/// `fMessagePosX`'s value for "keep the messages' own x".
const KEEP_X: f32 = -601.0;

/// Where the messages were before the first move, while they are moved.
static MESSAGES_HOME: Mutex<Option<(f64, f64)>> = Mutex::new(None);

fn post_display(hud: Option<&IMenu>) {
    if let Some(hud) = hud {
        reposition_messages(hud);
    }
    if util::game_is_paused()
        && !util::is_menu_open("MapMenu")
        && !util::is_menu_open("RaceSex Menu")
        && let Some(third) = util::player().and_then(PlayerCharacterExt::third_person_root)
    {
        update_animation_third_person(third, true);
    }
}

fn reposition_messages(hud: &IMenu) {
    let Some(movie) = (unsafe { hud.movie.as_ref() }) else { return };
    let dialogue = settings::profile().Dialogue;
    let mut home = MESSAGES_HOME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if dialogue.bEnable && dialogue.bRepositionHUDMessages && util::is_menu_open("Dialogue Menu") {
        if home.is_none() {
            *home = movie.variable(MESSAGES_X).and_then(|x| x.as_number()).zip(movie.variable(MESSAGES_Y).and_then(|y| y.as_number()));
        }
        if let Some((x, _)) = *home {
            #[allow(clippy::float_cmp)]
            let x = if dialogue.fMessagePosX == KEEP_X { x } else { f64::from(dialogue.fMessagePosX) };
            movie.set_variable(MESSAGES_X, &Value::number(x));
            movie.set_variable(MESSAGES_Y, &Value::number(f64::from(dialogue.fMessagePosY)));
        }
    } else if let Some((x, y)) = home.take() {
        movie.set_variable(MESSAGES_X, &Value::number(x));
        movie.set_variable(MESSAGES_Y, &Value::number(y));
    }
}

/// Puts moved HUD messages back, for the unload.
pub fn release() {
    let Some((x, y)) = MESSAGES_HOME.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { return };
    if let Some(movie) = util::ui().and_then(|ui| ui.menu("HUD Menu")).and_then(|menu| unsafe { menu.movie.as_ref() }) {
        movie.set_variable(MESSAGES_X, &Value::number(x));
        movie.set_variable(MESSAGES_Y, &Value::number(y));
    }
}

pub fn install(host: &'static Host) -> Result<(), Error> {
    let slot = |table| Vtable::<PostDisplay>::slot(Location::id(table, 0), POST_DISPLAY);
    host.hooks().after(slot(vtables::HUDMenu_vtable::ID), Options::new(host.name()), |args, _, _| post_display(unsafe { (args.0 as *const IMenu).as_ref() }))?;
    host.hooks().after(slot(vtables::JournalMenu_vtable::ID), Options::new(host.name()), |_, _, _| post_display(None))?;
    Ok(())
}
