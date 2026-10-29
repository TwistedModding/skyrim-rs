//! `ImprovedCamera` on the skyrim-rs framework, ported module by module from IC's sources (the
//! module docs name the file each one ports). So far: the third-person body in first person with
//! its shadow, the head, arms and sheathed gear hidden from the player's own view, the body states
//! that decide it, the camera manager with every camera state, its transitions and head anchor,
//! the body anchored to the camera, the world, hands and tween
//! menu FOV with the zoom effect, and the pitch-driven world near plane.

mod body;
mod camera;
mod handoff;
mod hook;
mod plugin;
mod settings;
mod ui;
mod util;

use std::io::Write as _;
use std::sync::OnceLock;

use framework::{Action, Host, Scope};
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
/// Left shift, the free camera's fixed wheel modifier in IC.
const LEFT_SHIFT: u32 = 0x2A;

/// The frame's input for the camera states: the game's mouse and the free camera's keys.
pub(crate) fn frame_input() -> camera::states::Input {
    let (Some(host), Some([left, right, reset])) = (HOST.get(), FREE_CAMERA.get()) else { return camera::states::Input::default() };
    camera::states::Input { mouse: host.mouse(), roll_left: left.held(), roll_right: right.held(), reset: reset.fired(), modifier: host.key_held(LEFT_SHIFT) }
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
/// engine's FOV and near plane back, as does choosing the first-person pass for the near plane.
fn apply_profile(profile: &settings::Profile) {
    let before = *settings::profile();
    settings::set_profile(profile);
    let Some(host) = HOST.get() else { return };
    profile.write(&host.settings());
    let body = profile.General.bEnableBody;
    if before.General.bEnableBody != body {
        hook::fixes::set_body_patch(host, body);
        hook::fixes::apply_body_visibility();
        if !body {
            camera::manager::reset_fov();
        }
    }
    let pass = profile.Advanced.bNearFirstPersonPass;
    if (before.General.bEnableBody && !body) || (!before.Advanced.bNearFirstPersonPass && pass) {
        camera::manager::reset_near();
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
    let _ = ZOOM.set(host.action("zoom", "", Scope::Game));
    let _ = FREE_CAMERA.set([host.action("free-camera-roll-left", "Q", Scope::Game), host.action("free-camera-roll-right", "E", Scope::Game), host.action("free-camera-reset", "R", Scope::Game)]);
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

/// Puts the body back as the engine keeps it: full scales, and the third-person root hidden in
/// first person; and the engine's world FOV, FOV offset and near plane.
fn unload() {
    camera::manager::release();
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
    handoff::save(&body, &camera,handoff::Written { world_fov, near: skyrim::render::near_distance() })
}

/// Takes the old build's state, and the world FOV and near plane it had written over the ones
/// `unload` put back.
fn restore_state(bytes: &[u8]) {
    if let Some((body, camera, written)) = handoff::restore(bytes) {
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
