//! `ui/menu/AdvancedMenu` ("Advanced Settings"): head bob, transitions, the death reload time,
//! the collision bumpers and the near distance.

use framework::egui;

use super::widgets::{rows, section_header, slider, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let g = p.General;
        let a = &mut p.Advanced;
        super::halves(ui, |ui, right| {
            section_header(ui, "Headbob");
            rows(ui, "AdvancedTable1", |ui| {
                if !g.bEnableBody || g.bEnableFakeFirstPerson || !g.bEnableFirstPersonArms {
                    ui.disable();
                }
                toggle(ui, "Headbob", &mut a.bHeadbob, "Enable/Disable head bobbing");
                if !a.bHeadbob {
                    a.bHeadbobCombat = false;
                    ui.disable();
                }
                toggle(ui, "Headbob Combat", &mut a.bHeadbobCombat, "Enable/Disable head bobbing when weapons are drawn");
            });
            section_header(ui, "Miscellaneous");
            rows(ui, "AdvancedTable2", |ui| {
                toggle(ui, "Camera Transition", &mut a.bCameraTransition, "Enable/Disable camera transitions");
                slider(ui, "Player Death Reload Time", &mut a.fPlayerDeathReloadTime, 5.0, 60.0, 2, "");
            });
            section_header(ui, "Collision Bumpers");
            rows(ui, "AdvancedTable3", |ui| {
                slider(ui, "Forward Bumper Offset", &mut a.fForwardBumperOffset, 0.0, 100.0, 2, "");
                slider(ui, "Side Bumper Offset", &mut a.fSideBumperOffset, 0.0, 100.0, 2, "");
                slider(ui, "Back Bumper Offset", &mut a.fBackBumperOffset, 0.0, 100.0, 2, "");
                slider(ui, "Werewolf Bumper Offset", &mut a.fWerewolfBumperOffset, 0.0, 100.0, 2, "");
            });
            ui.label("Collision changes only take affect when you next go into third person.");
            let ui = right;
            section_header(ui, "Near Distance");
            rows(ui, "AdvancedTable4", |ui| slider(ui, "Near Distance (Default)", &mut a.fNearDistanceDefault, 2.0, 30.0, 0, ""));
            if !g.bEnableBody {
                ui.disable();
            }
            section_header(ui, "Near Distance Limits");
            rows(ui, "AdvancedTable5", |ui| {
                slider(ui, "Near Distance Max", &mut a.fNearDistanceMax, 2.0, 30.0, 0, "");
                slider(ui, "Near Distance Min", &mut a.fNearDistanceMin, 2.0, 30.0, 0, "");
                slider(ui, "Near Distance Start Angle", &mut a.fNearDistanceStartAngle, 0.0, -90.0, 0, "");
                slider(ui, "Near Distance End Angle", &mut a.fNearDistanceEndAngle, 0.0, -90.0, 0, "");
            });
            section_header(ui, "Near Distance (Combat)");
            rows(ui, "AdvancedTable6", |ui| {
                slider(ui, "Near Distance Combat Max", &mut a.fNearDistanceCombatMax, 2.0, 30.0, 0, "");
                slider(ui, "Near Distance Combat Min", &mut a.fNearDistanceCombatMin, 2.0, 30.0, 0, "");
                slider(ui, "Near Distance Combat Start Angle", &mut a.fNearDistanceCombatStartAngle, 0.0, -90.0, 0, "");
                slider(ui, "Near Distance Combat End Angle", &mut a.fNearDistanceCombatEndAngle, 0.0, -90.0, 0, "");
            });
        });
    });
}
