//! The compat suite's two plain SKSE plugins, which hook engine functions with `MinHook` detours of
//! their own the way plugins built without the framework do. `a` (`skyrim_legacy_patch_a.dll`,
//! loaded before `skyrim_rs.dll` by file name) hooks `Actor::ModActorValueImpl` in its
//! `SKSEPlugin_Load` and logs the player's health damage it receives; `z`
//! (`skyrim_rs_legacy_patch_z.dll`, loaded after it) hooks `TESObjectREFR::GetScale` at
//! `PLUGINS_LOADED_2`, once the host has checked its sites, and counts the calls.

use core::ffi::c_void;
use std::fmt::Display;
use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use sre::raw::fns;
use skyrim::actors::Modifier;

pub const A: &str = "skyrim-rs-legacy-patch-a";
pub const Z: &str = "skyrim-rs-legacy-patch-z";

static RUNTIME: OnceLock<u32> = OnceLock::new();
static MOD_ACTOR_VALUE: OnceLock<hook::Hook<fns::Actor__ModActorValueImpl_fn>> = OnceLock::new();
static GET_SCALE: OnceLock<hook::Hook<fns::TESObjectREFR__GetScale_fn>> = OnceLock::new();
static SCALE_CALLS: AtomicU64 = AtomicU64::new(0);

fn log(plugin: &str, line: impl Display) {
    if let Some(&runtime) = RUNTIME.get()
        && let Ok(mut f) = skse::log::append(runtime, plugin)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Starts the plugin's log and the game layer.
fn start(skse: &skse::Interface, plugin: &str) -> bool {
    let _ = RUNTIME.set(skse.runtime_version);
    if skse::log::open(skse.runtime_version, plugin).is_err() {
        return false;
    }
    match skyrim::init(skse) {
        Ok(()) => true,
        Err(e) => {
            log(plugin, format_args!("game layer: {e}"));
            false
        }
    }
}

fn install<F: Copy>(plugin: &str, cell: &'static OnceLock<hook::Hook<F>>, name: &str, id: u64, detour: F) -> bool {
    let installed = skyrim::reloc().address(id).map_err(|e| e.to_string()).and_then(|address| {
        let hook = unsafe { hook::Hook::create(address, detour) }.map_err(|e| e.to_string())?;
        let _ = cell.set(hook);
        cell.get().expect("just set").enable().map_err(|e| e.to_string())?;
        Ok(address)
    });
    match installed {
        Ok(address) => log(plugin, format_args!("hooked {name} at {address:#x}")),
        Err(e) => log(plugin, format_args!("{name}: {e}")),
    }
    cell.get().is_some()
}

unsafe extern "C" fn mod_actor_value(this: *mut sre::Actor, modifier: i32, value: u32, delta: f32, source: *mut c_void) {
    let player = sre::PlayerCharacter::singleton().is_some_and(|p| core::ptr::eq(core::ptr::from_ref(p).cast(), this));
    if player && value == sre::ActorValue::Health as u32 && modifier == Modifier::Damage as i32 && delta < 0.0 {
        log(A, format_args!("player health damage {delta}"));
    }
    unsafe { MOD_ACTOR_VALUE.get().expect("set before enabling").original()(this, modifier, value, delta, source) }
}

unsafe extern "C" fn get_scale(this: *mut sre::TESObjectREFR) -> f32 {
    SCALE_CALLS.fetch_add(1, Ordering::Relaxed);
    unsafe { GET_SCALE.get().expect("set before enabling").original()(this) }
}

#[must_use]
pub fn load_a(skse: &skse::Interface) -> bool {
    start(skse, A) && install(A, &MOD_ACTOR_VALUE, "Actor::ModActorValueImpl", fns::Actor__ModActorValueImpl, mod_actor_value as fns::Actor__ModActorValueImpl_fn)
}

#[must_use]
pub fn load_z(skse: &skse::Interface) -> bool {
    start(skse, Z) && skse.messaging().is_some_and(|m| m.listen_skse(skse.plugin_handle(), on_message_z))
}

unsafe extern "C" fn on_message_z(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    match msg.kind {
        skse::MessagingInterface::PLUGINS_LOADED_2 => {
            install(Z, &GET_SCALE, "TESObjectREFR::GetScale", fns::TESObjectREFR__GetScale, get_scale as fns::TESObjectREFR__GetScale_fn);
        }
        skse::MessagingInterface::SAVE_GAME => log(Z, format_args!("GetScale ran {} times", SCALE_CALLS.load(Ordering::Relaxed))),
        _ => {}
    }
}
