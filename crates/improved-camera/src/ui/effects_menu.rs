//! `ui/menu/EffectsMenu` ("Effects Settings"): the letterbox's master switch and the zoom effect.

use framework::egui;

use super::widgets::{key, rows, section_header, slider, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let l = &mut p.Letterbox;
        section_header(ui, "Letterbox");
        rows(ui, "EffectsTableLetterbox", |ui| {
            toggle(ui, "Enabled", &mut l.bEnable, "Master switch for letterbox bars; each feature is configured in its own menu (Dialogue, Idle Camera)");
            if !l.bEnable {
                ui.disable();
            }
            toggle(ui, "Overlay Mode", &mut l.bOverlay, "Draw the bars over the frame instead of scaling the scene between them (crops the view, no FOV change)");
        });
        section_header(ui, "Zoom");
        rows(ui, "EffectsTableZoom", |ui| {
            key(ui, "Key", crate::ZOOM.get());
            slider(ui, "FOV", &mut p.Zoom.fFOV, -60.0, -5.0, 0, "FOV change while the zoom is active");
        });
    });
}
