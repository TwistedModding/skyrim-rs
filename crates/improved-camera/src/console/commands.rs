//! `console/ConsoleCommands`: the `ic` command (IC renames `Show1stPerson` to it; here it is a
//! command of its own through the host): `ic [version]`, `ic enable`, `ic disable`, `ic menu`,
//! `ic console`, `ic resetstate`, `ic profile [name]`, `ic spec`/`ic spectate` and `ic pm`/`ic photomode`; and
//! `SetCameraFOV`, whose engine function IC replaces. The commands run on the console queue's
//! thread, so what changes the camera or the profile is handed to the next frame.

use std::ffi::CStr;
use std::sync::Mutex;

use skyrim::actors::RefHandle;
use skyrim::input::{Arg, Command, CommandContext, Param};
use skyrim::prelude::*;
use skyrim::ui::UIMessageQueueExt;
use sre::{ScriptParamType, TESObjectREFR, UIMessageQueue};

use crate::camera::extended;
use crate::util::skyrim_util as util;

const NAME: &str = "improved-camera";

/// What a command asks of the main thread.
#[derive(Clone, Debug, PartialEq)]
enum Request {
    Body(bool),
    Menu,
    Profile(String),
    PhotoMode,
    ResetState,
    /// `SetCameraFOV`: the world FOV and, when given, the hands FOV.
    CameraFov(f32, Option<f32>),
}

static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());

fn request(r: Request) {
    PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(r);
}

/// From the frame callback: what the commands asked since the last frame.
pub fn run_pending() {
    let pending = core::mem::take(&mut *PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for r in pending {
        match r {
            Request::Body(on) => {
                let mut p = *crate::settings::profile();
                p.General.bEnableBody = on;
                crate::apply_profile(&p);
            }
            Request::Menu => {
                if let Some(host) = crate::HOST.get() {
                    host.show_tab();
                }
            }
            Request::Profile(name) => {
                crate::switch_profile(&name);
            }
            Request::PhotoMode => crate::camera::studio::manager::with(|m| if m.active { m.exit() } else { m.enter() }),
            Request::CameraFov(world, hands) => apply_camera_fov(world, hands),
            Request::ResetState => util::reset_player_state(),
        }
    }
}

/// `CheckConsole`: commands that change the game run outside the main and loading menus; with
/// `close` the console is hidden first.
fn check_console(close: bool) -> bool {
    if close && let Some(queue) = UIMessageQueue::singleton() {
        queue.hide("Console");
    }
    !util::is_menu_open("Main Menu") && !util::is_menu_open("Loading Menu")
}

/// `ic spec`: the console's selection, if a humanoid actor, becomes the camera's target, or stops
/// being it when it already is; with no selection the player is the target again.
fn spectate(selected: Option<&TESObjectREFR>) {
    let Some(camera) = util::camera() else { return };
    let Some(selected) = selected else {
        extended::request_target(None);
        return;
    };
    let Some(actor) = util::actor_of(selected).filter(|a| util::is_humanoid(a)) else { return };
    if RefHandle(camera.targetHandle) == actor.handle() {
        extended::request_target(None);
        print("spectating >> disabled");
    } else if actor.get3_d().is_some() {
        extended::request_target(Some(actor.handle()));
        let name = actor.display_name().and_then(|n| n.to_str().ok()).unwrap_or_default();
        print(&format!("spectating >> {name}"));
    }
}

/// IC's reading of `SetCameraFOV`'s line: one value is the world FOV, a second the hands FOV
/// (the world FOV then stays 80, as in IC), each held to 1-165; no value or more than two puts
/// the world FOV at 80.
fn camera_fov(values: &[f32]) -> (f32, Option<f32>) {
    let clamp = |v: f32| v.clamp(1.0, 165.0);
    match values {
        [world] => (clamp(*world), None),
        [_, hands] => (80.0, Some(clamp(*hands))),
        _ => (80.0, None),
    }
}

/// `SetCameraFOV` in place of the engine's: the profile's FOVs follow the world FOV (not in the
/// free camera) and the hands FOV, and the camera takes both.
pub fn set_camera_fov(context: &mut CommandContext<'_>) -> bool {
    let values: Vec<f32> = context
        .args()
        .iter()
        .filter_map(|a| match a {
            Arg::Float(v) => Some(*v),
            Arg::Int(v) => i16::try_from(*v).ok().map(f32::from),
            _ => None,
        })
        .collect();
    let (world, hands) = camera_fov(&values);
    request(Request::CameraFov(world, hands));
    true
}

fn apply_camera_fov(world: f32, hands: Option<f32>) {
    let Some(camera) = util::camera() else { return };
    let mut p = *crate::settings::profile();
    if let Some(hands) = hands {
        p.FOV.fFirstPersonHands = hands;
        skyrim::store(&camera.firstPersonFOV, hands);
    }
    if !util::is_free_camera() {
        let f = &mut p.FOV;
        for fov in [
            &mut f.fFirstPerson,
            &mut f.fFirstPersonCombat,
            &mut f.fThirdPerson,
            &mut f.fCannibal,
            &mut f.fCrafting,
            &mut f.fDeath,
            &mut f.fDragon,
            &mut f.fDragonCombat,
            &mut f.fDragonTransition,
            &mut f.fFurniture,
            &mut f.fHorse,
            &mut f.fHorseCombat,
            &mut f.fHorseTransition,
            &mut f.fKillmove,
            &mut f.fLich,
            &mut f.fRagdoll,
            &mut f.fSex,
            &mut f.fVampireLord,
            &mut f.fWerewolf,
        ] {
            *fov = world;
        }
    }
    crate::apply_profile(&p);
    skyrim::store(&camera.worldFOV, world);
}

fn print(text: &str) {
    skyrim::input::print(&format!("{NAME}: {text}"));
}

/// What `ic <command> [argument]` does, apart from the spectator; `active` is the active
/// profile's name and `saved` the saved profiles'.
fn run(command: &str, argument: Option<&str>, free_camera: bool, in_game: bool, body: bool, active: &str, saved: &[String]) -> (String, Option<Request>) {
    let version = format!("version >> {}", env!("CARGO_PKG_VERSION"));
    match command {
        "" | "version" => (version, None),
        "pm" | "photomode" => ("photo mode toggled".to_owned(), Some(Request::PhotoMode)),
        _ if free_camera => ("commands are disabled during TFC mode".to_owned(), None),
        "disable" => {
            let off = in_game && body;
            (format!("enabled >> {}", if off || !body { "FALSE" } else { "TRUE" }), off.then_some(Request::Body(false)))
        }
        "enable" => ("enabled >> TRUE".to_owned(), Some(Request::Body(true))),
        "menu" | "console" => ("opening the menu".to_owned(), Some(Request::Menu)),
        "resetstate" => (format!("resetstate >> {}", if in_game { "TRUE" } else { "FALSE" }), in_game.then_some(Request::ResetState)),
        "profile" => match argument {
            None => (format!("profile active >> {active}"), None),
            Some(name) if name == active => (format!("profile is already active '{active}'"), None),
            Some(name) if saved.iter().any(|s| s == name) => (format!("profile set >> {name}"), Some(Request::Profile(name.to_owned()))),
            Some(name) => (format!("profile not found '{name}'"), None),
        },
        other => (format!("unknown command '{other}'"), None),
    }
}

pub struct Ic;

static PARAMS: [Param; 2] = [Param::new(c"command", ScriptParamType::String, true), Param::new(c"argument", ScriptParamType::String, true)];

impl Command for Ic {
    const NAME: &'static CStr = c"IC";
    const HELP: &'static CStr = c"Improved Camera: ic [version|enable|disable|menu|resetstate|profile [name]|spec|pm]";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let text = |i: usize| match context.args().get(i) {
            Some(Arg::Str(s)) => Some((*s).to_owned()),
            _ => None,
        };
        let (command, argument) = (text(0).unwrap_or_default(), text(1));
        if matches!(command.as_str(), "spec" | "spectate") {
            spectate(context.this_ref());
            return true;
        }
        let close = matches!(command.as_str(), "menu" | "resetstate");
        let in_game = check_console(close);
        let (line, request) = run(&command, argument.as_deref(), util::is_free_camera(), in_game, crate::settings::profile().General.bEnableBody, &crate::active_profile(), &crate::profile_names());
        print(&line);
        if let Some(r) = request {
            self::request(r);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, camera_fov, run};

    #[test]
    fn set_camera_fov_reads_its_line_as_ic_does() {
        assert_eq!(camera_fov(&[]), (80.0, None));
        assert_eq!(camera_fov(&[95.0]), (95.0, None));
        assert_eq!(camera_fov(&[200.0]), (165.0, None));
        assert_eq!(camera_fov(&[0.0]), (1.0, None));
        assert_eq!(camera_fov(&[95.0, 70.0]), (80.0, Some(70.0)));
        assert_eq!(camera_fov(&[95.0, 300.0]), (80.0, Some(165.0)));
        assert_eq!(camera_fov(&[1.0, 2.0, 3.0]), (80.0, None));
    }

    fn ic(command: &str, argument: Option<&str>) -> (String, Option<Request>) {
        run(command, argument, false, true, true, "Default", &["Default".to_owned(), "Combat".to_owned()])
    }

    #[test]
    fn answers_as_ic_does() {
        assert_eq!(ic("", None).0, format!("version >> {}", env!("CARGO_PKG_VERSION")));
        assert_eq!(ic("version", None).1, None);
        assert_eq!(ic("disable", None), ("enabled >> FALSE".to_owned(), Some(Request::Body(false))));
        assert_eq!(run("disable", None, false, false, true, "Default", &[]), ("enabled >> TRUE".to_owned(), None), "not in the main or loading menu");
        assert_eq!(ic("enable", None), ("enabled >> TRUE".to_owned(), Some(Request::Body(true))));
        assert_eq!(ic("menu", None).1, Some(Request::Menu));
        assert_eq!(ic("profile", None).0, "profile active >> Default");
        assert_eq!(ic("profile", Some("Default")).0, "profile is already active 'Default'");
        assert_eq!(ic("profile", Some("Combat")), ("profile set >> Combat".to_owned(), Some(Request::Profile("Combat".to_owned()))));
        assert_eq!(ic("profile", Some("Cold")), ("profile not found 'Cold'".to_owned(), None));
        assert_eq!(ic("dance", None).0, "unknown command 'dance'");
        assert_eq!(ic("resetstate", None), ("resetstate >> TRUE".to_owned(), Some(Request::ResetState)));
        assert_eq!(run("resetstate", None, false, false, true, "Default", &[]).1, None, "not in the main or loading menu");
        assert_eq!(run("pm", None, true, true, true, "Default", &[]).1, Some(Request::PhotoMode), "photo mode toggles in TFC");
        assert_eq!(run("enable", None, true, true, true, "Default", &[]), ("commands are disabled during TFC mode".to_owned(), None));
        assert_eq!(run("version", None, true, true, true, "Default", &[]).1, None, "the version answers in TFC");
    }
}
