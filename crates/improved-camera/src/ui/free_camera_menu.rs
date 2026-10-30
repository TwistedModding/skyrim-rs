//! `ui/menu/FreeCameraMenu` ("Free Camera Settings"): the free camera's controls; its keys are
//! the host's hotkeys.

use framework::egui;

use super::widgets::{key, rows, section_header, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let keys = crate::FREE_CAMERA.get();
        section_header(ui, "Controls");
        rows(ui, "FreeCameraTable", |ui| {
            toggle(ui, "Enabled", &mut p.FreeCamera.bEnable, "Enable/Disable additional tfc controls and features");
            key(ui, "Roll Left", keys.map(|k| &k[0]));
            key(ui, "Roll Right", keys.map(|k| &k[1]));
            key(ui, "Reset", keys.map(|k| &k[2]));
        });
    });
}
