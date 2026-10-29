//! A minimal section for the camera states' options (from IC's `CameraMenu`, `AdvancedMenu`,
//! `FreeCameraMenu`, `DialogueMenu`, `VanityMenu` and `StabilizationMenu`), enough to test them;
//! the full menu is a later item.

use framework::egui;

use crate::settings::{self, CamOffset, Profile, Stab};

fn offset(ui: &mut egui::Ui, o: &mut CamOffset, label: &str) {
    ui.label(label);
    ui.add(egui::Slider::new(&mut o.forward, -50.0..=50.0).fixed_decimals(1).text("Forward"));
    ui.add(egui::Slider::new(&mut o.up, -50.0..=50.0).fixed_decimals(1).text("Up"));
    ui.add(egui::Slider::new(&mut o.right, -50.0..=50.0).fixed_decimals(1).text("Right"));
}

fn stabilization(ui: &mut egui::Ui, s: &mut Stab, label: &str) {
    ui.checkbox(&mut s.bEnable, label);
    ui.add(egui::Slider::new(&mut s.fHeadFollowStrength, 0.0..=1.0).fixed_decimals(2).text("Head Follow Strength"));
    ui.add(egui::Slider::new(&mut s.fSmoothingSpeed, 0.1..=40.0).fixed_decimals(1).text("Smoothing Speed"));
    ui.add(egui::Slider::new(&mut s.fSmoothingMaxOffset, 0.0..=30.0).fixed_decimals(1).text("Smoothing Max Offset"));
}

pub fn draw(ui: &mut egui::Ui) {
    let before = *settings::profile();
    let mut p: Profile = before;
    ui.heading("General");
    ui.checkbox(&mut p.General.bEnableFakeFirstPerson, "Fake First Person");
    ui.checkbox(&mut p.General.bEnableHeadSpectator, "Spectator Head");
    ui.checkbox(&mut p.Advanced.bCameraTransition, "Camera Transition");
    ui.checkbox(&mut p.Advanced.bHeadbob, "Headbob");
    ui.checkbox(&mut p.Advanced.bHeadbobCombat, "Headbob (Combat)");
    ui.checkbox(&mut p.Body.bYawSmoothing, "Body Yaw Smoothing");
    ui.add(egui::Slider::new(&mut p.Body.fAnchorForward, -30.0..=30.0).fixed_decimals(1).text("Anchor Forward"));
    ui.add(egui::Slider::new(&mut p.Body.fAnchorUp, -30.0..=30.0).fixed_decimals(1).text("Anchor Up"));
    ui.heading("Free Camera");
    ui.checkbox(&mut p.FreeCamera.bEnable, "Enable");
    ui.heading("Dialogue");
    ui.checkbox(&mut p.Dialogue.bEnable, "Enable");
    ui.checkbox(&mut p.Dialogue.bForceFirstPerson, "Force First Person");
    ui.checkbox(&mut p.Dialogue.bForceThirdPerson, "Force Third Person");
    ui.checkbox(&mut p.Dialogue.bSmoothTransition, "Smooth Transition");
    ui.heading("Idle Camera");
    ui.checkbox(&mut p.Vanity.bEnable, "Enable");
    ui.add(egui::Slider::new(&mut p.Vanity.fIdleDelay, 5.0..=600.0).fixed_decimals(0).text("Idle Delay"));
    ui.add(egui::Slider::new(&mut p.Vanity.fFOV, 20.0..=120.0).fixed_decimals(0).text("FOV"));
    ui.heading("Offsets");
    let mut first = CamOffset { forward: p.Firstperson.forward, up: p.Firstperson.up, right: p.Firstperson.right };
    offset(ui, &mut first, "First Person");
    (p.Firstperson.forward, p.Firstperson.up, p.Firstperson.right) = (first.forward, first.up, first.right);
    offset(ui, &mut p.Firstperson.Combat, "First Person (Combat)");
    let mut horse = p.Horse.offset();
    offset(ui, &mut horse, "Horse");
    (p.Horse.forward, p.Horse.up, p.Horse.right) = (horse.forward, horse.up, horse.right);
    let c = &mut p.Camera;
    for (o, label) in [(&mut c.Dragon, "Dragon"), (&mut c.VampireLord, "Vampire Lord"), (&mut c.Werewolf, "Werewolf"), (&mut c.Lich, "Lich"), (&mut c.Sex, "Sex"), (&mut c.Spectator, "Spectator")] {
        offset(ui, o, label);
    }
    ui.heading("Stabilization");
    let s = &mut p.Stabilization;
    for (st, label) in [
        (&mut s.Horse, "Horse"),
        (&mut s.Dragon, "Dragon"),
        (&mut s.VampireLord, "Vampire Lord"),
        (&mut s.Werewolf, "Werewolf"),
        (&mut s.Lich, "Lich"),
        (&mut s.Sex, "Sex"),
        (&mut s.Furniture, "Furniture"),
        (&mut s.Crafting, "Crafting"),
        (&mut s.Gathering, "Gathering"),
        (&mut s.Bleedout, "Bleedout"),
        (&mut s.Transition, "Transition"),
    ] {
        stabilization(ui, st, label);
    }
    if p != before {
        crate::apply_profile(&p);
    }
}
