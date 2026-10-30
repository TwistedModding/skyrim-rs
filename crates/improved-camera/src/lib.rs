//! `ImprovedCamera` on the skyrim-rs framework, ported module by module from IC's sources (the
//! module docs name the file each one ports). So far: the third-person body in first person with
//! its shadow, the head, arms and sheathed gear hidden from the player's own view, the body states
//! that decide it, the camera manager with every camera state, its transitions and head anchor,
//! the body anchored to the camera with the first-person free look, the idle camera's orbit, the
//! dialogue's framing and speaker focus shots, the race menu orbit, the world, hands and tween
//! menu FOV with the zoom effect, the pitch-driven world near plane, and the gameplay sites: the
//! magic casters' and effect shaders' root, the crosshair pick, the movement input buffer, the
//! scroll wheel, the POV toggle and the scripts' forced first and third person; foot IK kept on
//! for the anchored body, and FSMP told when the body is camera-driven.

mod api;
mod body;
mod camera;
mod event;
mod handoff;
mod hook;
mod plugin;
mod settings;
mod ui;
mod util;

use std::io::Write as _;
use std::sync::OnceLock;

use framework::{Action, Host, InputFilter, Scope};
use sre::InputDevice;
use skyrim::prelude::*;

framework::plugin! {
    name: "improved-camera",
    load: load,
    unload: unload,
    save_state: save_state,
    restore_state: restore_state,
}

static HOST: OnceLock<&'static Host> = OnceLock::new();
/// IC's zoom key (`Effects.Zoom.iKey`, unbound by default).
static ZOOM: OnceLock<Action> = OnceLock::new();
/// IC's free camera keys (`FreeCamera.iKeyRollLeft`, `iKeyRollRight`, `iKeyReset`): Q, E and R.
static FREE_CAMERA: OnceLock<[Action; 3]> = OnceLock::new();
/// What IC's free camera swallows while its controls run: the menu keys (`is_blackListedEvent`)
/// and the shoulders it rolls with.
static FREE_CAMERA_FILTER: OnceLock<InputFilter> = OnceLock::new();
/// Left shift, the free camera's fixed wheel modifier in IC.
const LEFT_SHIFT: u32 = 0x2A;

/// The frame's input for the camera states: the game's mouse and gamepad, the free camera's keys
/// and the game's Toggle POV key released.
pub(crate) fn frame_input() -> camera::states::Input {
    let (Some(host), Some([left, right, reset])) = (HOST.get(), FREE_CAMERA.get()) else { return camera::states::Input::default() };
    camera::states::Input {
        mouse: host.mouse(),
        pad: host.gamepad(),
        pov_released: plugin::info().keyboard_pov.and_then(|key| host.key_released(key)),
        roll_left: left.held(),
        roll_right: right.held(),
        reset: reset.fired(),
        modifier: host.key_held(LEFT_SHIFT),
    }
}

/// Starts or stops swallowing the free camera's keys.
pub(crate) fn swallow_free_camera_input(active: bool) {
    if let Some(filter) = FREE_CAMERA_FILTER.get()
        && filter.active() != active
    {
        filter.set_active(active);
    }
}

pub(crate) fn log(line: &str) {
    if let Some(host) = HOST.get()
        && let Ok(mut f) = skse::log::append(host.skse().runtime_version, "improved-camera")
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Makes `profile` the active one and stores it; a change of the body option applies or restores
/// the `SetFirstPerson` patch and the root's visibility at once, and turning it off puts the
/// engine's FOV and near plane back. Switching the first-person arms makes the magic casters find
/// their node again, and turning them off turns head bob off (IC's General menu).
fn apply_profile(profile: &settings::Profile) {
    let before = *settings::profile();
    let mut profile = *profile;
    let arms = before.General.bEnableFirstPersonArms != profile.General.bEnableFirstPersonArms;
    if arms && !profile.General.bEnableFirstPersonArms {
        profile.Advanced.bHeadbob = false;
        profile.Advanced.bHeadbobCombat = false;
    }
    let profile = &profile;
    settings::set_profile(profile);
    if arms {
        util::skyrim_util::reset_magic_nodes();
    }
    let Some(host) = HOST.get() else { return };
    profile.write(&host.settings());
    let body = profile.General.bEnableBody;
    if before.General.bEnableBody != body {
        hook::fixes::set_body_patch(host, body);
        hook::fixes::apply_body_visibility();
        if !body {
            camera::manager::reset_fov();
            camera::manager::reset_near();
        }
    }
}

fn load(host: &'static Host) -> bool {
    if skyrim::init(host.skse()).is_err() {
        return false;
    }
    let _ = HOST.set(host);
    settings::set_profile(&settings::Profile::read(&host.settings()));
    if let Err(e) = hook::install(host) {
        log(&format!("hooks refused: {e}"));
        return false;
    }
    host.on_message(|msg| match msg.kind {
        skse::MessagingInterface::DATA_LOADED => plugin::detect_mods(),
        skse::MessagingInterface::POST_LOAD_GAME => hook::fixes::apply_body_visibility(),
        _ => {}
    });
    host.on_plugin_message(api::fsmp::SENDER, api::fsmp::handle_message);
    let _ = ZOOM.set(host.action("zoom", "", Scope::Game));
    let _ = FREE_CAMERA.set([host.action("free-camera-roll-left", "Q", Scope::Game), host.action("free-camera-roll-right", "E", Scope::Game), host.action("free-camera-reset", "R", Scope::Game)]);
    let shoulders = [camera::base::PAD_LEFT_SHOULDER, camera::base::PAD_RIGHT_SHOULDER].map(|mask| (InputDevice::Gamepad, mask));
    let _ = FREE_CAMERA_FILTER.set(host.input_filter("free-camera", &["Favorites", "Journal", "Quick Map", "Quick Stats", "Wait"], &shoulders));
    host.on_frame(|| {
        if ZOOM.get().is_some_and(Action::fired) {
            camera::manager::with(|m| m.zoom_active = !m.zoom_active);
        }
        hook::cameras::on_frame();
        camera::extended::apply_deferred();
    });
    host.add_tab("Improved Camera").section("Body", ui::body_menu::draw).section("FOV", ui::fov_menu::draw).section("Near plane", ui::near_menu::draw).section("Camera", ui::camera_menu::draw);
    hook::fixes::apply_body_visibility();
    log("loaded");
    true
}

/// Puts the body back as the engine keeps it: full scales, the third-person root hidden in first
/// person and its foot IK parked; the engine's world FOV, FOV offset and near plane; and FSMP
/// simulating the player as usual.
fn unload() {
    camera::manager::release();
    body::free_look::release();
    body::manager::release_foot_ik();
    api::fsmp::set_player_body_camera_driven(false);
    camera::manager::reset_fov();
    camera::manager::reset_near();
    let Some(player) = util::skyrim_util::player() else { return };
    body::manager::with(|m| m.parts.restore(player));
    if !player.is_third_person()
        && let Some(third) = player.third_person_root()
    {
        third.set_hidden(true);
    }
}

fn save_state() -> Vec<u8> {
    let body = body::manager::with(|m| *m);
    let camera = camera::manager::with(|m| m.clone());
    let world_fov = util::skyrim_util::camera().map_or(0.0, |c| c.worldFOV);
    handoff::save(&body, &camera, handoff::Written { world_fov, near: skyrim::render::near_distance() }, api::fsmp::saved())
}

/// Takes the old build's state, FSMP's interface it received, and the world FOV and near plane it
/// had written over the ones `unload` put back.
fn restore_state(bytes: &[u8]) {
    if let Some((body, camera, written, fsmp)) = handoff::restore(bytes) {
        api::fsmp::restore(fsmp);
        body::manager::with(|m| *m = body);
        camera::manager::with(|m| *m = camera);
        if let Some(c) = util::skyrim_util::camera() {
            if written.world_fov > 0.0 {
                skyrim::store(&c.worldFOV, written.world_fov);
            }
            if written.near > 0.0 && written.near.to_bits() != skyrim::render::near_distance().to_bits() {
                skyrim::render::set_near_plane(written.near);
            }
        }
    }
}
