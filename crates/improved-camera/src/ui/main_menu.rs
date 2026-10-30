//! `ui/menu/MainMenu`'s File menu ("Profiles", "Save", "Save As...", "Delete") and the profile
//! name of its title bar, with the importer of IC's own files beside them.

use framework::egui;

use super::widgets::section_header;
use crate::profiles::{self, DEFAULT};

/// What the save dialog (`m_viewSaveWindow`) shows: nothing, Save, or Save As.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Dialog {
    #[default]
    Closed,
    Save,
    SaveAs,
}

#[derive(Clone, Default)]
struct State {
    dialog: Dialog,
    name: String,
    /// The last import's outcome.
    imported: String,
}

pub fn draw(ui: &mut egui::Ui) {
    let id = egui::Id::unique("ImprovedCameraMain");
    let mut state = ui.data(|d| d.get_temp::<State>(id)).unwrap_or_default();
    let active = crate::active_profile();
    ui.horizontal(|ui| {
        ui.label("Profile Active");
        ui.strong(&active);
    });

    section_header(ui, "Profiles");
    for name in crate::profile_names() {
        if ui.add_enabled(name != active, egui::Button::new(&name)).clicked() {
            crate::switch_profile(&name);
        }
    }

    section_header(ui, "File");
    ui.horizontal(|ui| {
        if ui.button("Save").on_hover_text("Ctrl+S in IC's menu").clicked() && state.dialog == Dialog::Closed {
            state.dialog = Dialog::Save;
        }
        if ui.button("Save As...").clicked() && state.dialog == Dialog::Closed {
            state.dialog = Dialog::SaveAs;
        }
        if ui.add_enabled(active != DEFAULT, egui::Button::new("Delete")).clicked() {
            crate::delete_profile();
        }
    });
    if state.dialog != Dialog::Closed {
        ui.group(|ui| {
            ui.label("Save Settings");
            if active == DEFAULT || state.dialog == Dialog::SaveAs {
                ui.horizontal(|ui| {
                    ui.label("Profile Name:");
                    ui.add(egui::TextEdit::singleline(&mut state.name).desired_width(200.0).char_limit(64));
                });
                state.name.retain(|c| c.is_ascii_alphanumeric());
                ui.horizontal(|ui| {
                    if ui.add_enabled(profiles::valid_name(&state.name), egui::Button::new("OK")).clicked() {
                        crate::save_profile(&state.name);
                        state.dialog = Dialog::Closed;
                        state.name.clear();
                    }
                    if ui.button("Cancel").clicked() {
                        state.dialog = Dialog::Closed;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        crate::save_profile(&active);
                        state.dialog = Dialog::Closed;
                    }
                    if ui.button("Cancel").clicked() {
                        state.dialog = Dialog::Closed;
                    }
                });
            }
        });
    }

    section_header(ui, "Import");
    ui.label("Reads Improved Camera's ImprovedCameraSE.json and Profiles folder into these profiles, replacing those of the same name. Its files are not changed.");
    if ui.button("Import from Improved Camera").clicked() {
        state.imported = crate::import_profiles();
    }
    if !state.imported.is_empty() {
        ui.label(&state.imported);
    }
    ui.data_mut(|d| d.insert_temp(id, state));
}
