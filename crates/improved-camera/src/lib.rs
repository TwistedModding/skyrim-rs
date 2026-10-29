//! `ImprovedCamera` on the skyrim-rs framework, ported module by module from IC's sources (the
//! module docs name the file each one ports). So far: the third-person body in first person with
//! its shadow, the head, arms and sheathed gear hidden from the player's own view, the body states
//! that decide it, the camera manager's frame order they read, and the world, hands and tween
//! menu FOV with the zoom effect.

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

fn log(line: &str) {
    if let Some(host) = HOST.get()
        && let Ok(mut f) = skse::log::append(host.skse().runtime_version, "improved-camera")
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Makes `profile` the active one and stores it; a change of the body option applies or restores
/// the `SetFirstPerson` patch and the root's visibility at once, and turning it off puts the
/// engine's FOV back.
fn apply_profile(profile: &settings::Profile) {
    let body_changed = settings::profile().General.bEnableBody != profile.General.bEnableBody;
    settings::set_profile(profile);
    let Some(host) = HOST.get() else { return };
    profile.write(&host.settings());
    if body_changed {
        hook::fixes::set_body_patch(host, profile.General.bEnableBody);
        hook::fixes::apply_body_visibility();
        if !profile.General.bEnableBody {
            camera::manager::reset_fov();
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
    let _ = ZOOM.set(host.action("zoom", "", Scope::Game));
    host.on_frame(|| {
        if ZOOM.get().is_some_and(Action::fired) {
            camera::manager::with(|m| m.zoom_active = !m.zoom_active);
        }
    });
    host.add_tab("Improved Camera").section("Body", ui::body_menu::draw).section("FOV", ui::fov_menu::draw);
    hook::fixes::apply_body_visibility();
    log("loaded");
    true
}

/// Puts the body back as the engine keeps it: full scales, and the third-person root hidden in
/// first person; and the engine's world FOV and FOV offset.
fn unload() {
    camera::manager::reset_fov();
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
    let camera = camera::manager::with(|m| *m);
    let world_fov = util::skyrim_util::camera().map_or(0.0, |c| c.worldFOV);
    handoff::save(&body, camera, world_fov)
}

/// Takes the old build's state, and the world FOV it had written over the one `unload` put back.
fn restore_state(bytes: &[u8]) {
    if let Some((body, camera, world_fov)) = handoff::restore(bytes) {
        body::manager::with(|m| *m = body);
        camera::manager::with(|m| *m = camera);
        if let Some(c) = util::skyrim_util::camera().filter(|_| world_fov > 0.0) {
            skyrim::store(&c.worldFOV, world_fov);
        }
    }
}
