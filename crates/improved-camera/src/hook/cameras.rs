//! `hook/CamerasHook`: the camera manager's update after each camera update that moves the player
//! camera root: the player camera's state step (`PlayerCamera::Update` +0x1a6), the free camera's
//! (`UpdateActorWithCamera` +0x69) and the race menu camera (`RaceSexCamera` slot 2).

use framework::Host;
use framework::hooks::ops::{Call, Vtable};
use framework::hooks::{Error, Location, Options};
use sre::raw::{fns, vtables};

use crate::camera::manager;
use crate::util::skyrim_util as util;

const PLAYER_CAMERA_STATE_STEP: usize = 0x1a6;
const FREE_CAMERA_STATE_STEP: usize = 0x69;
const RACE_SEX_CAMERA_UPDATE: usize = 2;

type CameraUpdate = unsafe extern "C" fn(usize) -> usize;

fn update() {
    if let Some(camera) = util::camera() {
        manager::update(camera);
    }
}

pub fn install(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    let mine = || Options::new(host.name());
    hooks.after(Call::<CameraUpdate>::at(Location::id(fns::PlayerCamera__Update, PLAYER_CAMERA_STATE_STEP)), mine(), |_, _, _| update())?;
    hooks.after(Call::<CameraUpdate>::at(Location::id(fns::UpdateActorWithCamera, FREE_CAMERA_STATE_STEP)), mine(), |_, _, _| update())?;
    hooks.after(Vtable::<CameraUpdate>::slot(Location::id(vtables::RaceSexCamera_vtable::ID, 0), RACE_SEX_CAMERA_UPDATE), mine(), |_, _, _| update())?;
    Ok(())
}
