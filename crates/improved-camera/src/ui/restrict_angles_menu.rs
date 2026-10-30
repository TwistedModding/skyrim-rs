//! `ui/menu/RestrictAnglesMenu`: the look limits in IC's first person, greyed out while the body
//! is off.

use framework::egui;

use crate::settings::{self, Profile};

fn slider(ui: &mut egui::Ui, value: &mut f32, range: core::ops::RangeInclusive<f32>, label: &str, hint: &str) {
    ui.add(egui::Slider::new(value, range).fixed_decimals(1).text(label)).on_hover_text(hint);
}

pub fn draw(ui: &mut egui::Ui) {
    let before = *settings::profile();
    let mut p: Profile = before;
    let r = &mut p.RestrictAngles;
    ui.add_enabled_ui(before.General.bEnableBody, |ui| {
        ui.heading("Standing");
        slider(ui, &mut r.fPitchUp, 1.0..=89.0, "Pitch Up", "Restricts upward pitch");
        slider(ui, &mut r.fPitchDown, -89.0..=-1.0, "Pitch Down", "Restricts downward pitch");
        slider(ui, &mut r.fHeadingLeft, -89.0..=-1.0, "Heading Left", "Restricts leftward heading");
        slider(ui, &mut r.fHeadingRight, 1.0..=89.0, "Heading Right", "Restricts rightward heading");
        ui.heading("Interaction");
        slider(ui, &mut r.fInteractionPitchUp, 1.0..=89.0, "Interaction Pitch Up", "Restricts upward pitch when sitting, horseback, dragon, etc");
        slider(ui, &mut r.fInteractionPitchDown, -89.0..=-1.0, "Interaction Pitch Down", "Restricts downward pitch when sitting, horseback, dragon, etc");
        slider(ui, &mut r.fInteractionHeadingLeft, -89.0..=-1.0, "Interaction Heading Left", "Restricts leftward heading when sitting, horseback, dragon, etc");
        slider(ui, &mut r.fInteractionHeadingRight, 1.0..=89.0, "Interaction Heading Right", "Restricts rightward heading when sitting, horseback, dragon, etc");
    });
    if p != before {
        crate::apply_profile(&p);
    }
}
