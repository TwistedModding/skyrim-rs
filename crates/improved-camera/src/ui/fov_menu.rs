//! `ui/menu/FOVMenu` ("Field of View Settings"): the FOV of each camera, greyed out while the body
//! is off.

use framework::egui;

use super::widgets::{rows, section_header, slider};

fn fov(ui: &mut egui::Ui, label: &str, value: &mut f32, tooltip: &str) {
    slider(ui, label, value, 1.0, 165.0, 0, tooltip);
}

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        let f = &mut p.FOV;
        super::halves(ui, |ui, right| {
            section_header(ui, "First Person");
            rows(ui, "fov_fp", |ui| {
                fov(ui, "Hands", &mut f.fFirstPersonHands, "Adjust field of view for hands");
                fov(ui, "Default", &mut f.fFirstPerson, "Adjust field of view during first person");
                fov(ui, "Combat", &mut f.fFirstPersonCombat, "Adjust field of view during first person combat");
            });
            section_header(ui, "Activities");
            rows(ui, "fov_activities", |ui| {
                fov(ui, "Furniture", &mut f.fFurniture, "Adjust field of view during furniture (bed, chair, bench, etc)");
                fov(ui, "Crafting", &mut f.fCrafting, "Adjust field of view during crafting (forge, smelter. grinding stone, ect)");
                fov(ui, "Killmove", &mut f.fKillmove, "Adjust field of view during killmoves");
                fov(ui, "Ragdoll", &mut f.fRagdoll, "Adjust field of view during ragdolling");
                fov(ui, "Death", &mut f.fDeath, "Adjust field of view during death");
                fov(ui, "Cannibal", &mut f.fCannibal, "Adjust field of view during cannibal");
            });
            section_header(ui, "Horse");
            rows(ui, "fov_horse", |ui| {
                fov(ui, "Riding", &mut f.fHorse, "Adjust field of view during horse riding");
                fov(ui, "Combat", &mut f.fHorseCombat, "Adjust field of view during horse riding combat");
                fov(ui, "Transition", &mut f.fHorseTransition, "Adjust field of view during horse transitions (mounting and dismounting)");
            });
            let ui = right;
            section_header(ui, "Dragon");
            rows(ui, "fov_dragon", |ui| {
                fov(ui, "Riding", &mut f.fDragon, "Adjust field of view during dragon riding");
                fov(ui, "Combat", &mut f.fDragonCombat, "Adjust field of view during dragon riding combat");
                fov(ui, "Transition", &mut f.fDragonTransition, "Adjust field of view during dragon transitions (mounting and dismounting)");
            });
            section_header(ui, "Transformations");
            rows(ui, "fov_transform", |ui| {
                fov(ui, "Vampire Lord", &mut f.fVampireLord, "Adjust field of view during vampire lord");
                fov(ui, "Werewolf", &mut f.fWerewolf, "Adjust field of view during werewolf");
                fov(ui, "Lich", &mut f.fLich, "Adjust field of view during lich");
            });
            section_header(ui, "Other");
            rows(ui, "fov_other", |ui| {
                fov(ui, "Sex", &mut f.fSex, "Adjust field of view during sex");
                fov(ui, "Third Person", &mut f.fThirdPerson, "Adjust field of view during third person");
            });
        });
    });
}
