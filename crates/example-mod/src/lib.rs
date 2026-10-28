//! The example mod of `docs/MODS.md`: a framework plugin that scales the health damage the player
//! takes. The factor is a setting that persists, shown as a slider in the plugin's tab of the
//! host's window, applied by a hook on `Actor::ModActorValueImpl` and settable from Papyrus through
//! the native `SkyrimRsExample.SetDamageScale`. A hotkey, Shift+K unless the player binds another,
//! opens the window on the tab from the game.

use core::ffi::c_void;
use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use framework::hooks::Options;
use framework::hooks::ops::Actor__ModActorValueImpl;
use framework::{Action, Host, Scope, egui, theme};
use skyrim::actors::Modifier;
use skyrim::papyrus::VirtualMachine;

framework::plugin! {
    name: "skyrim-rs-example",
    load: load,
}

/// The settings key of the damage factor.
const SCALE: &str = "damage_scale";

static HOST: OnceLock<&'static Host> = OnceLock::new();
static OPEN_TAB: OnceLock<Action> = OnceLock::new();
/// The factor as `f32` bits: the hook reads it on every call without a lock.
static SCALE_BITS: AtomicU32 = AtomicU32::new(0x3F80_0000);
/// The last damage scaled, before and after, as `f32` bits.
static LAST: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

fn log(line: &str) {
    if let Some(host) = HOST.get()
        && let Ok(mut f) = skse::log::append(host.skse().runtime_version, "skyrim-rs-example")
    {
        let _ = writeln!(f, "{line}");
    }
}

fn scale() -> f32 {
    f32::from_bits(SCALE_BITS.load(Ordering::Relaxed))
}

/// Sets the factor, clamped to 0 to 3, and stores it in the plugin's settings.
fn set_scale(value: f32) -> f32 {
    let value = if value.is_finite() { value.clamp(0.0, 3.0) } else { 1.0 };
    SCALE_BITS.store(value.to_bits(), Ordering::Relaxed);
    if let Some(host) = HOST.get() {
        host.settings().set(SCALE, value);
    }
    value
}

fn load(host: &'static Host) -> bool {
    if skyrim::init(host.skse()).is_err() {
        return false;
    }
    let _ = HOST.set(host);
    SCALE_BITS.store(host.settings().get_or(SCALE, 1.0f32).to_bits(), Ordering::Relaxed);
    log(&format!("loaded, damage scale {}", scale()));

    // Actor::ModActorValueImpl(actor, modifier, value, delta, source): the player's health damage is multiplied by the factor.
    let subscribed = host.hooks().before(Actor__ModActorValueImpl, Options::new(host.name()), |args: &mut (*mut sre::Actor, i32, u32, f32, *mut c_void)| {
        let is_player = sre::PlayerCharacter::singleton().is_some_and(|p| core::ptr::eq(args.0.cast_const().cast(), p));
        if is_player && args.1 == Modifier::Damage as i32 && args.2 == sre::ActorValue::Health as u32 && args.3 < 0.0 {
            let scaled = args.3 * scale();
            LAST[0].store(args.3.to_bits(), Ordering::Relaxed);
            LAST[1].store(scaled.to_bits(), Ordering::Relaxed);
            args.3 = scaled;
        }
    });
    if let Err(e) = subscribed {
        log(&format!("hook refused: {e}"));
        return false;
    }

    host.on_papyrus(|vm: &'static VirtualMachine| {
        let host = HOST.get().expect("set in load");
        if let Err(e) = host.register_native(vm, "SkyrimRsExample", "SetDamageScale", set_scale) {
            log(&format!("SetDamageScale not bound: {e}"));
        }
    });

    host.add_tab("Example mod").section("Damage", damage).section("About", about);
    let _ = OPEN_TAB.set(host.action("open-tab", "Shift+K", Scope::Game));
    host.on_frame(open_tab);
    true
}

fn damage(ui: &mut egui::Ui) {
    ui.label("Health damage the player takes is multiplied by");
    let mut value = scale();
    if ui.add(egui::Slider::new(&mut value, 0.0..=3.0).step_by(0.05).text("damage scale")).changed() {
        set_scale(value);
    }
    let (before, after) = (f32::from_bits(LAST[0].load(Ordering::Relaxed)), f32::from_bits(LAST[1].load(Ordering::Relaxed)));
    ui.label(egui::RichText::new(format!("Last hit: {before} became {after}")).color(theme::DIM));
}

fn about(ui: &mut egui::Ui) {
    ui.label("The worked example of the skyrim-rs framework: a hook, a setting, a Papyrus native and this tab.");
    ui.label(egui::RichText::new("Papyrus: SkyrimRsExample.SetDamageScale(float) sets the factor and returns it.").color(theme::DIM));
    if let Some(key) = OPEN_TAB.get().and_then(Action::key) {
        ui.label(egui::RichText::new(format!("{key} in the game opens this tab.")).color(theme::DIM));
    }
}

/// Opens the host's window on this tab on the action's key; polled every frame.
fn open_tab() {
    let (Some(host), Some(action)) = (HOST.get(), OPEN_TAB.get()) else { return };
    if action.fired() {
        host.show_tab();
        log(&format!("open-tab fired on {}: tab shown", action.key().map(|k| k.to_string()).unwrap_or_default()));
    }
}
