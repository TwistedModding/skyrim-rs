//! `ui/menu/FOVMenu`: the FOV of each camera, greyed out while the body is off, and the zoom
//! effect's FOV from `ui/menu/EffectsMenu` (its key is the `improved-camera.zoom` action).

use framework::egui;

use crate::settings::{self, Profile};

fn slider(ui: &mut egui::Ui, value: &mut f32, label: &str) {
    ui.add(egui::Slider::new(value, 1.0..=165.0).fixed_decimals(0).text(label));
}

pub fn draw(ui: &mut egui::Ui) {
    let before = *settings::profile();
    let mut p: Profile = before;
    let enable_body = p.General.bEnableBody;
    let f = &mut p.FOV;
    ui.add_enabled_ui(enable_body, |ui| {
        ui.heading("First Person");
        slider(ui, &mut f.fFirstPersonHands, "Hands");
        slider(ui, &mut f.fFirstPerson, "Default");
        slider(ui, &mut f.fFirstPersonCombat, "Combat");
        ui.heading("Activities");
        slider(ui, &mut f.fFurniture, "Furniture");
        slider(ui, &mut f.fCrafting, "Crafting");
        slider(ui, &mut f.fKillmove, "Killmove");
        slider(ui, &mut f.fRagdoll, "Ragdoll");
        slider(ui, &mut f.fDeath, "Death");
        slider(ui, &mut f.fCannibal, "Cannibal");
        ui.heading("Horse");
        slider(ui, &mut f.fHorse, "Riding");
        slider(ui, &mut f.fHorseCombat, "Combat");
        slider(ui, &mut f.fHorseTransition, "Transition");
        ui.heading("Dragon");
        slider(ui, &mut f.fDragon, "Riding");
        slider(ui, &mut f.fDragonCombat, "Combat");
        slider(ui, &mut f.fDragonTransition, "Transition");
        ui.heading("Transformations");
        slider(ui, &mut f.fVampireLord, "Vampire Lord");
        slider(ui, &mut f.fWerewolf, "Werewolf");
        slider(ui, &mut f.fLich, "Lich");
        ui.heading("Other");
        slider(ui, &mut f.fSex, "Sex");
        slider(ui, &mut f.fThirdPerson, "Third Person");
    });
    ui.heading("Zoom");
    ui.add(egui::Slider::new(&mut p.Zoom.fFOV, -60.0..=-5.0).fixed_decimals(0).text("FOV"));

    if p != before {
        crate::apply_profile(&p);
    }
}
