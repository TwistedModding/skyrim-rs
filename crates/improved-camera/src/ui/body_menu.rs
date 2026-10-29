//! `ui/menu/BodyMenu`, the part this port has so far: the body's General and Hide options, its
//! global placement and the free look, in the host's tab.

use framework::egui;

use crate::settings::{self, Profile};

fn drag(ui: &mut egui::Ui, value: &mut f32, label: &str) {
    ui.add(egui::DragValue::new(value).speed(0.1).range(-100.0..=100.0).prefix(format!("{label}: ")));
}

pub fn draw(ui: &mut egui::Ui) {
    let before = *settings::profile();
    let mut p: Profile = before;
    let g = &mut p.General;
    ui.heading("General");
    ui.checkbox(&mut g.bEnableBody, "Enable body");
    ui.checkbox(&mut g.bEnableBodyConsole, "Enable body with the console open");
    ui.checkbox(&mut g.bEnableHead, "Show head");
    ui.checkbox(&mut g.bEnableHeadCombat, "Show head in combat");
    ui.checkbox(&mut g.bEnableHeadHorse, "Show head on horseback");
    ui.checkbox(&mut g.bEnableHeadDragon, "Show head on a dragon");
    ui.checkbox(&mut g.bEnableHeadVampireLord, "Show head as a vampire lord");
    ui.checkbox(&mut g.bEnableHeadWerewolf, "Show head as a werewolf");
    ui.checkbox(&mut g.bEnableHeadSex, "Show head in scenes");
    ui.checkbox(&mut g.bEnableFirstPersonArms, "First person arms");
    ui.checkbox(&mut g.bEnableFirstPersonArmsMovement, "First person arms while moving");
    ui.checkbox(&mut g.bEnableThirdPersonShield, "Third person shield arm");
    ui.checkbox(&mut g.bEnableThirdPersonShieldBlock, "Third person shield arm when blocking");
    ui.checkbox(&mut g.bEnableThirdPersonTorch, "Third person torch arm");
    ui.checkbox(&mut g.bEnableThirdPersonTorchBlock, "Third person torch arm when blocking");
    ui.checkbox(&mut g.bEnableThirdPersonBow, "Third person bow arms");
    ui.checkbox(&mut g.bEnableThirdPersonBowAim, "Third person bow arms when aiming");
    ui.checkbox(&mut g.bEnableThirdPersonCrossbow, "Third person crossbow arms");
    ui.checkbox(&mut g.bEnableThirdPersonCrossbowAim, "Third person crossbow arms when aiming");

    let h = &mut p.Hide;
    ui.heading("Hide");
    ui.checkbox(&mut h.bBody, "Body");
    ui.checkbox(&mut h.bWeaponDrawn, "Weapon drawn");
    ui.checkbox(&mut h.bAiming, "Aiming");
    ui.checkbox(&mut h.bPowerAttack, "Power attacking");
    ui.checkbox(&mut h.bJumping, "Jumping");
    ui.checkbox(&mut h.bJumpCombat, "Jumping in combat");
    ui.checkbox(&mut h.bSneakRoll, "Sneak rolling");
    ui.checkbox(&mut h.bSwimming, "Swimming");
    ui.checkbox(&mut h.bTwoHanded, "Sheathed two-handed weapons");
    ui.checkbox(&mut h.bBow, "Sheathed bow");
    ui.checkbox(&mut h.bQuiver, "Quiver");

    let b = &mut p.Body;
    ui.heading("Placement");
    drag(ui, &mut b.fForward, "Forward");
    drag(ui, &mut b.fUp, "Up");
    drag(ui, &mut b.fRight, "Right");
    drag(ui, &mut b.Combat.fForward, "Combat forward");
    drag(ui, &mut b.Combat.fUp, "Combat up");
    drag(ui, &mut b.Combat.fRight, "Combat right");

    ui.heading("Free Look");
    ui.checkbox(&mut b.bFreeLook, "Enable free look");
    ui.checkbox(&mut b.bFreeLookSpine, "Spine twist");
    ui.add(egui::Slider::new(&mut b.fFreeLookMaxAngle, 0.0..=180.0).text("Look Threshold (deg)"));
    ui.add(egui::Slider::new(&mut b.fFreeLookSettleAngle, 0.0..=90.0).text("Settle Angle (deg)"));
    ui.add(egui::Slider::new(&mut b.fFreeLookCatchupSpeed, 1.0..=30.0).text("Shuffle Speed (x10 deg/s)"));
    ui.add(egui::Slider::new(&mut b.fFreeLookTwistStrength, 0.1..=3.0).text("Twist Strength"));

    if p != before {
        crate::apply_profile(&p);
    }
}
