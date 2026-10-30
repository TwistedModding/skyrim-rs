//! `ui/menu/RestrictAnglesMenu` ("Restrict Angles Settings"): the look limits in IC's first
//! person, greyed out while the body is off.

use framework::egui;

use super::widgets::{rows, section_header, slider};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        let r = &mut p.RestrictAngles;
        section_header(ui, "Standing");
        rows(ui, "RestrictTableTop", |ui| {
            slider(ui, "Pitch Up", &mut r.fPitchUp, 1.0, 89.0, 1, "Restricts upward pitch");
            slider(ui, "Pitch Down", &mut r.fPitchDown, -1.0, -89.0, 1, "Restricts downward pitch");
            slider(ui, "Heading Left", &mut r.fHeadingLeft, -1.0, -89.0, 1, "Restricts leftward heading");
            slider(ui, "Heading Right", &mut r.fHeadingRight, 1.0, 89.0, 1, "Restricts rightward heading");
        });
        section_header(ui, "Interaction");
        rows(ui, "RestrictTableBottom", |ui| {
            slider(ui, "Interaction Pitch Up", &mut r.fInteractionPitchUp, 1.0, 89.0, 1, "Restricts upward pitch when sitting, horseback, dragon, etc");
            slider(ui, "Interaction Pitch Down", &mut r.fInteractionPitchDown, -1.0, -89.0, 1, "Restricts downward pitch when sitting, horseback, dragon, etc");
            slider(ui, "Interaction Heading Left", &mut r.fInteractionHeadingLeft, -1.0, -89.0, 1, "Restricts leftward heading when sitting, horseback, dragon, etc");
            slider(ui, "Interaction Heading Right", &mut r.fInteractionHeadingRight, 1.0, 89.0, 1, "Restricts rightward heading when sitting, horseback, dragon, etc");
        });
    });
}
