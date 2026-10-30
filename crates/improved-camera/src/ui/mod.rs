//! `ui/menu`: IC's menu windows as the sections of the plugin's tab, in the order of IC's View
//! menu, with IC's File menu (profiles) first and its console last.

pub mod advanced_menu;
pub mod body_menu;
pub mod camera_menu;
pub mod console_menu;
pub mod dialogue_menu;
pub mod effect;
pub mod effects_menu;
pub mod fov_menu;
pub mod free_camera_menu;
pub mod general_menu;
pub mod hide_menu;
pub mod main_menu;
pub mod restrict_angles_menu;
pub mod stabilization_menu;
pub mod studio_guides;
pub mod studio_menu;
pub mod vanity_menu;
pub mod widgets;

use framework::egui;

use crate::settings::{self, Profile};

/// Draws a page that edits the profile in use; a change is applied and stored.
pub fn edit(ui: &mut egui::Ui, draw: impl FnOnce(&mut egui::Ui, &mut Profile)) {
    let before = *settings::profile();
    let mut profile = before;
    draw(ui, &mut profile);
    if profile != before {
        crate::apply_profile(&profile);
    }
}

/// IC's two child windows side by side: `draw` gets the left and the right one.
pub fn halves(ui: &mut egui::Ui, draw: impl FnOnce(&mut egui::Ui, &mut egui::Ui)) {
    ui.columns(2, |columns| {
        if let [left, right] = columns {
            draw(left, right);
        }
    });
}
