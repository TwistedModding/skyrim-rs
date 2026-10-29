//! `hook/MiscHook`: the body's per-frame entry, before the third-person model update in
//! `PlayerCharacter::UpdateNodesAfterRender` (+0x94); the first-person update follows at +0xd7.

use framework::Host;
use framework::hooks::ops::Call;
use framework::hooks::{Error, Location, Options};
use sre::NiAVObject;
use sre::raw::fns;

use crate::body::manager::update_animation_third_person;
use crate::util::skyrim_util as util;

const UPDATE_THIRD_PERSON_CALL: usize = 0x94;

type UpdateNode = unsafe extern "C" fn(usize, usize) -> usize;

pub fn install(host: &'static Host) -> Result<(), Error> {
    host.hooks().before(Call::<UpdateNode>::at(Location::id(fns::PlayerCharacter__UpdateNodesAfterRender, UPDATE_THIRD_PERSON_CALL)), Options::new(host.name()), |args| {
        if util::is_body_valid()
            && !util::is_free_camera()
            && !util::is_menu_open("MapMenu")
            && let Some(third) = unsafe { (args.0 as *const NiAVObject).as_ref() }
        {
            update_animation_third_person(third, false);
        }
    })?;
    Ok(())
}
