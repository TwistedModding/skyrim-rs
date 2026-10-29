//! `hook/GraphicsHook`'s HUD post display (slot 6 of the HUD and journal menus): while a menu pauses
//! the game the model update stops, so the body is frozen where it stood (the map and race menus
//! run their own cameras).

use framework::Host;
use framework::hooks::ops::Vtable;
use framework::hooks::{Error, Location, Options};
use skyrim::prelude::*;
use sre::raw::vtables;

use crate::body::manager::update_animation_third_person;
use crate::util::skyrim_util as util;

const POST_DISPLAY: usize = 6;

type PostDisplay = unsafe extern "C" fn(usize) -> usize;

fn post_display() {
    if util::game_is_paused()
        && !util::is_menu_open("MapMenu")
        && !util::is_menu_open("RaceSex Menu")
        && let Some(third) = util::player().and_then(PlayerCharacterExt::third_person_root)
    {
        update_animation_third_person(third, true);
    }
}

pub fn install(host: &'static Host) -> Result<(), Error> {
    for table in [vtables::HUDMenu_vtable::ID, vtables::JournalMenu_vtable::ID] {
        host.hooks().after(Vtable::<PostDisplay>::slot(Location::id(table, 0), POST_DISPLAY), Options::new(host.name()), |_, _, _| post_display())?;
    }
    Ok(())
}
