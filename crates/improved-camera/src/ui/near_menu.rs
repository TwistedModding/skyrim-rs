//! The near-distance part of `ui/menu/AdvancedMenu`: the default near and the pitch ranges,
//! greyed out while the body is off as in IC, and the port's choice of how the near plane is
//! handled.

use framework::egui;

use crate::settings::{self, Profile};

fn near(ui: &mut egui::Ui, value: &mut f32, label: &str) {
    ui.add(egui::Slider::new(value, 2.0..=30.0).fixed_decimals(0).text(label));
}

fn angle(ui: &mut egui::Ui, value: &mut f32, label: &str) {
    ui.add(egui::Slider::new(value, -90.0..=0.0).fixed_decimals(0).text(label));
}

pub fn draw(ui: &mut egui::Ui) {
    let before = *settings::profile();
    let mut p: Profile = before;
    let enable_body = p.General.bEnableBody;
    let a = &mut p.Advanced;
    ui.heading("Method");
    ui.radio_value(&mut a.bNearFirstPersonPass, false, "Consistent writes: the near follows the pitch");
    ui.radio_value(&mut a.bNearFirstPersonPass, true, "First-person pass: the world near stays the engine's (body routing not built yet)");
    let writes = !a.bNearFirstPersonPass;
    ui.add_enabled_ui(writes, |ui| {
        ui.heading("Near Distance");
        near(ui, &mut a.fNearDistanceDefault, "Near Distance (Default)");
    });
    ui.add_enabled_ui(writes && enable_body, |ui| {
        ui.heading("Near Distance Limits");
        near(ui, &mut a.fNearDistanceMax, "Near Distance Max");
        near(ui, &mut a.fNearDistanceMin, "Near Distance Min");
        angle(ui, &mut a.fNearDistanceStartAngle, "Near Distance Start Angle");
        angle(ui, &mut a.fNearDistanceEndAngle, "Near Distance End Angle");
        ui.heading("Near Distance (Combat)");
        near(ui, &mut a.fNearDistanceCombatMax, "Near Distance Combat Max");
        near(ui, &mut a.fNearDistanceCombatMin, "Near Distance Combat Min");
        angle(ui, &mut a.fNearDistanceCombatStartAngle, "Near Distance Combat Start Angle");
        angle(ui, &mut a.fNearDistanceCombatEndAngle, "Near Distance Combat End Angle");
    });

    if p != before {
        crate::apply_profile(&p);
    }
}
