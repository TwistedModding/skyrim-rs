//! `hook/CamerasHook`: the camera manager's update after each camera update that moves the player
//! camera root: the player camera's state step (`PlayerCamera::Update` +0x1a6), the free camera's
//! (`UpdateActorWithCamera` +0x69), the race menu camera (`RaceSexCamera` slot 2) and the map menu
//! camera (`MapMenu::UpdateMapCamera` +0x5, a tail jump); and before the skills menu's message
//! handler (`StatsMenu` slot 4), the only per-frame place while that menu is open alone.

use std::sync::OnceLock;
use std::thread::ThreadId;

use framework::Host;
use framework::hooks::ops::{Call, Vtable};
use framework::hooks::{Error, Location, Options};
use sre::raw::{fns, vtables};

use crate::camera::manager;
use crate::util::skyrim_util as util;

const PLAYER_CAMERA_STATE_STEP: usize = 0x1a6;
const FREE_CAMERA_STATE_STEP: usize = 0x69;
const RACE_SEX_CAMERA_UPDATE: usize = 2;
const MAP_CAMERA_UPDATE: usize = 0x5;
const STATS_MENU_PROCESS_MESSAGE: usize = 4;

type CameraUpdate = unsafe extern "C" fn(usize) -> usize;
type ProcessMessage = unsafe extern "C" fn(usize, usize) -> usize;

/// The main thread, as the frame callback first saw it.
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Records the main thread; from the frame callback.
pub fn on_frame() {
    let _ = MAIN_THREAD.get_or_init(|| std::thread::current().id());
}

fn update() {
    if let Some(camera) = util::camera() {
        manager::update(camera);
    }
}

/// IC updates the camera before every skills menu message. Over the tween menu the tween menu's
/// post display already runs `PlayerCamera::Update` every frame (RE `camera.md`, request IC2 A),
/// so the update here is only for the menu opened alone, and only on the main thread: the menu's
/// show and first update can run on a worker job.
fn stats_menu_update() {
    if MAIN_THREAD.get() == Some(&std::thread::current().id()) && !util::is_menu_open("TweenMenu") {
        update();
    }
}

pub fn install(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    let mine = || Options::new(host.name());
    hooks.after(Call::<CameraUpdate>::at(Location::id(fns::PlayerCamera__Update, PLAYER_CAMERA_STATE_STEP)), mine(), |_, _, _| update())?;
    hooks.after(Call::<CameraUpdate>::at(Location::id(fns::UpdateActorWithCamera, FREE_CAMERA_STATE_STEP)), mine(), |_, _, _| update())?;
    hooks.after(Vtable::<CameraUpdate>::slot(Location::id(vtables::RaceSexCamera_vtable::ID, 0), RACE_SEX_CAMERA_UPDATE), mine(), |_, _, _| update())?;
    hooks.after(Call::<CameraUpdate>::at(Location::id(fns::MapMenu__UpdateMapCamera, MAP_CAMERA_UPDATE)), mine(), |_, _, _| update())?;
    hooks.before(Vtable::<ProcessMessage>::slot(Location::id(vtables::StatsMenu_vtable::ID, 0), STATS_MENU_PROCESS_MESSAGE), mine(), |_| stats_menu_update())?;
    Ok(())
}
