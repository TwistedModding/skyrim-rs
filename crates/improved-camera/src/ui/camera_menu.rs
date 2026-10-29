//! A minimal section for the camera states' options (from IC's `CameraMenu`, `AdvancedMenu`,
//! `FreeCameraMenu`, `DialogueMenu`, `VanityMenu` and `StabilizationMenu`), enough to test them;
//! the full menu is a later item.

use framework::egui;

use crate::camera::util::interpolation::METHOD_NAMES;
use crate::settings::{self, CamOffset, Profile, Stab};

fn slider(ui: &mut egui::Ui, value: &mut f32, range: core::ops::RangeInclusive<f32>, decimals: usize, label: &str) {
    ui.add(egui::Slider::new(value, range).fixed_decimals(decimals).text(label));
}

fn method(ui: &mut egui::Ui, value: &mut i32, label: &str) {
    let selected = usize::try_from(*value).ok().and_then(|i| METHOD_NAMES.get(i)).copied().unwrap_or(METHOD_NAMES[0]);
    egui::ComboBox::from_label(label).selected_text(selected).show_ui(ui, |ui| {
        for (i, name) in (0..).zip(METHOD_NAMES) {
            ui.selectable_value(value, i, name);
        }
    });
}

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
    let dialogue = &mut p.Dialogue;
    ui.checkbox(&mut dialogue.bFaceSpeaker, "Face Speaker");
    ui.checkbox(&mut dialogue.bHeadTracking, "Head Tracking");
    ui.checkbox(&mut dialogue.bHideDialogueMenu, "Hide Dialogue Menu");
    ui.checkbox(&mut dialogue.bSwitchTarget, "Speaker Focus");
    ui.checkbox(&mut dialogue.bLockOn, "Lock On");
    slider(ui, &mut dialogue.fCameraSpeed, 50.0..=2000.0, 0, "Camera Speed");
    slider(ui, &mut dialogue.fFirstPersonZoom, 10.0..=200.0, 0, "Zoom: First Person");
    slider(ui, &mut dialogue.fThirdPersonZoom, 10.0..=300.0, 0, "Zoom: Third Person");
    slider(ui, &mut dialogue.fDragonZoom, 50.0..=500.0, 0, "Zoom: Dragon");
    slider(ui, &mut dialogue.fDefaultWorldFOV, 30.0..=120.0, 0, "Default World FOV");
    slider(ui, &mut dialogue.fPlayerShoulderPosX, -200.0..=200.0, 0, "Player Shoulder: Side");
    slider(ui, &mut dialogue.fPlayerShoulderPosY, -200.0..=200.0, 0, "Player Shoulder: Forward");
    slider(ui, &mut dialogue.fPlayerShoulderPosZ, -200.0..=200.0, 0, "Player Shoulder: Up");
    slider(ui, &mut dialogue.fNpcShoulderPosX, -200.0..=200.0, 0, "NPC Shoulder: Side");
    slider(ui, &mut dialogue.fNpcShoulderPosY, -200.0..=200.0, 0, "NPC Shoulder: Forward");
    slider(ui, &mut dialogue.fNpcShoulderPosZ, -200.0..=200.0, 0, "NPC Shoulder: Up");
    slider(ui, &mut dialogue.fHumanCamOffsetZ, -50.0..=50.0, 0, "Focus Anchor: Human Height");
    slider(ui, &mut dialogue.fCreatureCamOffsetZ, -100.0..=100.0, 0, "Focus Anchor: Creature Height");
    ui.heading("Idle Camera");
    let vanity = &mut p.Vanity;
    ui.checkbox(&mut vanity.bEnable, "Enable");
    ui.checkbox(&mut vanity.bToggleMenus, "Hide Menus");
    slider(ui, &mut vanity.fIdleDelay, 5.0..=300.0, 0, "Idle Delay");
    slider(ui, &mut vanity.fOrbitSpeed, -30.0..=30.0, 1, "Orbit: Speed");
    slider(ui, &mut vanity.fDistance, 100.0..=600.0, 0, "Orbit: Distance");
    slider(ui, &mut vanity.fHeight, -50.0..=200.0, 0, "Orbit: Height");
    slider(ui, &mut vanity.fFOV, 30.0..=100.0, 0, "Orbit: FOV");
    ui.checkbox(&mut vanity.bCollisionSmoothing, "Collision Smoothing");
    method(ui, &mut vanity.iCollisionPullInMethod, "Pull-In Method");
    slider(ui, &mut vanity.fCollisionPullInDuration, 0.05..=1.0, 2, "Pull-In Duration");
    method(ui, &mut vanity.iCollisionRecoveryMethod, "Recovery Method");
    slider(ui, &mut vanity.fCollisionRecoveryDuration, 0.05..=2.0, 2, "Recovery Duration");
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
