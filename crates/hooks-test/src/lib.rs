//! The hooks suite's two framework plugins, `a` and `b`, which both subscribe to the player's
//! health damage path through the host's dispatcher and report what each phase saw in the shared
//! `skyrim-rs-hooks.log` in the test plugin's `PASS`/`FAIL`/`summary` format. `scripts/verify.ps1
//! -Suite hooks` drives the damage through the console and compares the health `DevBench` reads.

use core::ffi::c_void;
use std::fmt::Display;
use std::io::Write as _;
use std::sync::OnceLock;

use layout::v1_7_104::classes;
use skyrim::actors::{ActorValue, Modifier, PlayerCharacter};

pub mod a;
pub mod b;

pub const A: &str = "skyrim-rs-hooks-a";
pub const B: &str = "skyrim-rs-hooks-b";
pub const LOG: &str = "skyrim-rs-hooks";

static RUNTIME: OnceLock<u32> = OnceLock::new();

/// `Actor::ModActorValueImpl(this, modifier, value, delta, source)` as the subscribers see it.
pub type ModArgs = (*mut classes::Actor, i32, i32, f32, *mut c_void);
/// `Actor::ActorValueOwner_ModActorValue(this, modifier, value, delta)`.
pub type OwnerModArgs = (*mut classes::ActorValueOwner, i32, i32, f32);

/// Whether a modifier change damages the player's health.
#[must_use]
pub fn player_health_damage(this: *const c_void, modifier: i32, value: i32, delta: f32) -> bool {
    let player = PlayerCharacter::get().map_or(core::ptr::null(), |p| p.as_ptr().cast::<c_void>());
    let owner = PlayerCharacter::get().map_or(core::ptr::null(), |p| core::ptr::from_ref(p.actor_value_owner()).cast::<c_void>());
    (this == player || this == owner) && value == ActorValue::Health as i32 && modifier == Modifier::Damage as i32 && delta < 0.0
}

pub fn log(line: impl Display) {
    if let Some(&runtime) = RUNTIME.get()
        && let Ok(mut f) = skse::log::append(runtime, LOG)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub type Test = fn() -> Result<String, String>;

pub fn run(phase: &str, tests: &[(&str, Test)]) {
    let (mut pass, mut fail) = (0, 0);
    for (name, test) in tests {
        match test() {
            Ok(detail) => {
                pass += 1;
                log(format!("PASS {name}: {detail}"));
            }
            Err(detail) => {
                fail += 1;
                log(format!("FAIL {name}: {detail}"));
            }
        }
    }
    log(format!("summary hooks {phase}: {pass} pass, {fail} fail"));
}

pub fn check(ok: bool, what: impl Display) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

/// Initializes the game layer from the host's interface and the log; the first thing each plugin's `load` does.
pub fn init(host: &framework::Host) -> bool {
    let _ = RUNTIME.set(host.skse().runtime_version);
    skyrim::init(host.skse()).is_ok()
}
