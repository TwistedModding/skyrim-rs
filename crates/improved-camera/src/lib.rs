//! `ImprovedCamera` on the skyrim-rs framework, ported module by module from IC's sources (the
//! module docs name the file each one ports). So far: the third-person body in first person with
//! its shadow, the head, arms and sheathed gear hidden from the player's own view, the body states
//! that decide it, the camera manager with every camera state, its transitions and head anchor,
//! the body anchored to the camera with the first-person free look, the idle camera's orbit, the
//! dialogue's framing and speaker focus shots, the race menu orbit, the world, hands and tween
//! menu FOV with the zoom effect, the pitch-driven world near plane, and the gameplay sites: the
//! magic casters' and effect shaders' root, the crosshair pick, the movement input buffer, the
//! scroll wheel, the POV toggle and the scripts' forced first and third person; foot IK kept on
//! for the anchored body, FSMP told when the body is camera-driven; the look limits, the sitting
//! look-down limit, the body in interior rooms and the camera's fades skipped; IC's menu as the
//! tab's sections, its profiles kept in the plugin's settings with the importer of IC's own files,
//! its `ic` console command, its modder API served under IC's own plugin name, and the photo mode (the studio: pose layers, persistent skeleton and
//! race adjustments, the orbit camera, bone markers, guides, the light rig, the sun, colour
//! grading, pose packs, bookmarks, scenes and PNG capture).

mod api;
mod body;
mod camera;
mod console;
mod event;
mod handoff;
mod hook;
mod import;
mod plugin;
mod profiles;
mod settings;
mod ui;
mod util;

use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

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

pub(crate) static HOST: OnceLock<&'static Host> = OnceLock::new();
/// IC's zoom key (`Effects.Zoom.iKey`, unbound by default).
pub(crate) static ZOOM: OnceLock<Action> = OnceLock::new();
/// IC's free camera keys (`FreeCamera.iKeyRollLeft`, `iKeyRollRight`, `iKeyReset`): Q, E and R.
pub(crate) static FREE_CAMERA: OnceLock<[Action; 3]> = OnceLock::new();
/// IC's photo mode keys (`Studio.iToggleInputKey`, `iCaptureKey`): Insert brings the studio's
/// menu back, F10 takes a shot.
pub(crate) static PHOTO_MODE: OnceLock<[Action; 2]> = OnceLock::new();
/// What IC's free camera swallows while its controls run: the menu keys (`is_blackListedEvent`)
/// and the shoulders it rolls with.
static FREE_CAMERA_FILTER: OnceLock<InputFilter> = OnceLock::new();
/// Left shift, the free camera's fixed wheel modifier in IC.
const LEFT_SHIFT: u32 = 0x2A;
/// `fPlayerDeathReloadTime` as the engine had it before the first write, as bits; 0 before it.
static DEATH_RELOAD_ORIGINAL: AtomicU32 = AtomicU32::new(0);

/// Writes `Advanced.fPlayerDeathReloadTime` into the game (IC: at install and from the menu),
/// keeping the engine's value for unload.
fn write_death_reload_time(seconds: f32) {
    let setting = util::skyrim_util::death_reload_time();
    let _ = DEATH_RELOAD_ORIGINAL.compare_exchange(0, setting.to_bits(), Ordering::Relaxed, Ordering::Relaxed);
    skyrim::store(setting, seconds);
}

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
    ui::console_menu::push(line);
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
    write_death_reload_time(profile.Advanced.fPlayerDeathReloadTime);
    if arms {
        util::skyrim_util::reset_magic_nodes();
    }
    if before.General.bEnableFakeFirstPerson && !profile.General.bEnableFakeFirstPerson {
        leave_fake_first_person();
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

/// `GeneralMenu`'s reaction to "Fake First Person" turned off: a camera held at the fake
/// first-person zoom goes to the engine's first person, and the POV switch is allowed again.
fn leave_fake_first_person() {
    if let Some(camera) = util::skyrim_util::camera()
        && let Some(third) = camera.third_person()
        && third.targetZoom.to_bits() == util::skyrim_util::FAKE_FIRST_PERSON_ZOOM.to_bits()
    {
        util::skyrim_util::set_zoom(third, camera::base::THIRD_PERSON_ZOOM);
        camera.force_first_person();
        camera::manager::with(|m| m.forced_first_person = true);
    }
    util::skyrim_util::set_pov_switch(true);
}

/// The active profile's name.
pub(crate) fn active_profile() -> String {
    HOST.get().and_then(|h| profiles::active(&h.settings())).unwrap_or_else(|| profiles::DEFAULT.to_owned())
}

pub(crate) fn profile_names() -> Vec<String> {
    HOST.get().map(|h| profiles::names(&h.settings())).unwrap_or_default()
}

/// The menu's and `ic profile`'s switch (`ReadProfile` under the new active name): the saved
/// profile replaces the one in use from the next frame, after IC's `Reset_FirstPerson`.
pub(crate) fn switch_profile(name: &str) -> bool {
    let Some(host) = HOST.get() else { return false };
    let Some(profile) = profiles::switch(&host.settings(), name) else { return false };
    camera::manager::reset_fov();
    camera::manager::reset_near();
    apply_profile(&profile);
    log(&format!("Profile {name} loaded"));
    true
}

/// `WriteProfile`: the profile in use saved as `name`, which becomes the active one.
pub(crate) fn save_profile(name: &str) {
    let Some(host) = HOST.get() else { return };
    let store = host.settings();
    profiles::save(&store, name, &settings::profile());
    profiles::set_active(&store, name);
    log(&format!("Profile {name} saved"));
}

/// `DeleteProfile`: the active profile goes and Default is loaded.
pub(crate) fn delete_profile() {
    let Some(host) = HOST.get() else { return };
    let name = active_profile();
    if name == profiles::DEFAULT {
        return;
    }
    profiles::delete(&host.settings(), &name);
    switch_profile(profiles::DEFAULT);
    log(&format!("Profile {name} deleted"));
}

/// Imports IC's files, logs every note and reloads the active profile when the import replaced it.
/// Returns a one-line summary.
pub(crate) fn import_profiles() -> String {
    let Some(host) = HOST.get() else { return String::new() };
    let Some(dir) = import::ic_directory() else { return "no working directory".to_owned() };
    let summary = match import::import(&dir, &host.settings()) {
        Ok(report) => {
            for note in &report.notes {
                log(&format!("Import: {note}"));
            }
            let active = active_profile();
            if report.imported.contains(&active) {
                switch_profile(&active);
            }
            format!("{} profile(s) imported from {} ({} note(s) in the console)", report.imported.len(), dir.display(), report.notes.len())
        }
        Err(e) => format!("nothing imported: {e}"),
    };
    log(&format!("Import: {summary}"));
    summary
}

/// `ReadConfig` and `ReadProfile` at load. On the first run (no active profile stored yet) IC's
/// own files are imported and IC's active profile is used; then the active profile is validated
/// (`Default` when it is gone, written from the profile in use when missing).
fn start_profiles(host: &Host) {
    let store = host.settings();
    let first = profiles::active(&store).is_none();
    if first && let Some(dir) = import::ic_directory().filter(|d| d.is_dir()) {
        match import::import(&dir, &store) {
            Ok(report) => {
                for note in &report.notes {
                    log(&format!("Import: {note}"));
                }
                log(&format!("Import: {} profile(s) from {}", report.imported.len(), dir.display()));
                profiles::set_active(&store, report.active.as_deref().unwrap_or(profiles::DEFAULT));
            }
            Err(e) => log(&format!("Import: {e}")),
        }
    }
    let live = settings::Profile::read(&store);
    let name = profiles::validate(&store, &live);
    let profile = if first { profiles::load(&store, &name).unwrap_or(live) } else { live };
    settings::set_profile(&profile);
    profile.write(&store);
    write_death_reload_time(profile.Advanced.fPlayerDeathReloadTime);
    log(&format!("Profile {name} active"));
}

fn load(host: &'static Host) -> bool {
    if skyrim::init(host.skse()).is_err() {
        return false;
    }
    let _ = HOST.set(host);
    start_profiles(host);
    if let Err(e) = hook::install(host) {
        log(&format!("hooks refused: {e}"));
        return false;
    }
    host.on_message(|msg| match msg.kind {
        skse::MessagingInterface::DATA_LOADED => {
            plugin::detect_mods();
            if let Some(host) = HOST.get() {
                event::game_event_manager::register(host);
                if let Err(e) = host.register_command::<console::commands::Ic>() {
                    log(&format!("ic command refused: {e}"));
                }
                if let Err(e) = host.replace_command(c"SetCameraFOV", console::commands::set_camera_fov) {
                    log(&format!("SetCameraFOV not replaced: {e}"));
                }
            }
        }
        skse::MessagingInterface::POST_LOAD_GAME | skse::MessagingInterface::NEW_GAME => {
            camera::studio::manager::with(camera::studio::manager::Manager::on_session_reset);
            util::skyrim_util::clear_idle();
            hook::fixes::apply_body_visibility();
        }
        _ => {}
    });
    event::game_event_manager::register(host);
    host.on_plugin_message(api::fsmp::SENDER, api::fsmp::handle_message);
    api::provider::install(host);
    let _ = ZOOM.set(host.action("zoom", "", Scope::Game));
    let _ = FREE_CAMERA.set([host.action("free-camera-roll-left", "Q", Scope::Game), host.action("free-camera-roll-right", "E", Scope::Game), host.action("free-camera-reset", "R", Scope::Game)]);
    let _ = PHOTO_MODE.set([host.action("photo-mode-input", "Insert", Scope::Game), host.action("photo-mode-capture", "F10", Scope::Game)]);
    let shoulders = [camera::base::PAD_LEFT_SHOULDER, camera::base::PAD_RIGHT_SHOULDER].map(|mask| (InputDevice::Gamepad, mask));
    let _ = FREE_CAMERA_FILTER.set(host.input_filter("free-camera", &["Favorites", "Journal", "Quick Map", "Quick Stats", "Wait"], &shoulders));
    host.on_frame(|| {
        if ZOOM.get().is_some_and(Action::fired) {
            camera::manager::with(|m| m.zoom_active = !m.zoom_active);
        }
        console::commands::run_pending();
        body::manager::sheathe_while_swimming();
        hook::cameras::on_frame();
        camera::extended::apply_deferred();
        studio_frame();
    });
    host.add_hud(ui::effect::paint);
    host.add_hud(ui::studio_guides::paint);
    host.add_tab("Improved Camera")
        .section("Profiles", ui::main_menu::draw)
        .section("General", ui::general_menu::draw)
        .section("Advanced", ui::advanced_menu::draw)
        .section("Hide", ui::hide_menu::draw)
        .section("Restrict Angles", ui::restrict_angles_menu::draw)
        .section("Stabilization", ui::stabilization_menu::draw)
        .section("Body", ui::body_menu::draw)
        .section("Dialogue", ui::dialogue_menu::draw)
        .section("Free Camera", ui::free_camera_menu::draw)
        .section("Idle Camera", ui::vanity_menu::draw)
        .section("Effects", ui::effects_menu::draw)
        .section("FOV", ui::fov_menu::draw)
        .section("Camera", ui::camera_menu::draw)
        .section(ui::studio_menu::SECTION, ui::studio_menu::draw)
        .section("Console", ui::console_menu::draw);
    hook::fixes::apply_body_visibility();
    log("loaded");
    true
}

/// The photo mode's frame: its keys, its queued engine work and the capture.
fn studio_frame() {
    use camera::studio::manager::{self, Manager};
    ui::studio_menu::tick();
    if let Some([input, shoot]) = PHOTO_MODE.get()
        && manager::with(|m| m.active)
    {
        if input.fired()
            && let Some(host) = HOST.get()
        {
            host.show_section(ui::studio_menu::SECTION);
        }
        if shoot.fired() {
            manager::with(Manager::request_shot);
        }
    }
    manager::on_frame();
}

/// Puts the body back as the engine keeps it: full scales, the third-person root hidden in first
/// person, its foot IK parked and its original controller shape; the engine's world FOV, FOV offset, near plane and death
/// reload time; and FSMP simulating the player as usual.
fn unload() {
    camera::studio::manager::release();
    camera::manager::release();
    body::free_look::release();
    body::manager::release_foot_ik();
    body::bumper::release();
    api::fsmp::set_player_body_camera_driven(false);
    camera::manager::reset_fov();
    camera::manager::reset_near();
    match DEATH_RELOAD_ORIGINAL.load(Ordering::Relaxed) {
        0 => {}
        bits => skyrim::store(util::skyrim_util::death_reload_time(), f32::from_bits(bits)),
    }
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
    handoff::save(&body, &camera, handoff::Written { world_fov, near: skyrim::render::near_distance() }, api::fsmp::saved(), &api::provider::saved())
}

/// Takes the old build's state, FSMP's interface it received, the API's owners and callbacks, and the world FOV and near plane it
/// had written over the ones `unload` put back.
fn restore_state(bytes: &[u8]) {
    if let Some((body, camera, written, fsmp, provider)) = handoff::restore(bytes) {
        api::fsmp::restore(fsmp);
        api::provider::restore(&provider);
        body::manager::with(|m| *m = body);
        camera::manager::with(|m| *m = camera);
        camera::manager::notify_free_look();
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
