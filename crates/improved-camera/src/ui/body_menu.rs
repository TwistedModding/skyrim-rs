//! `ui/menu/BodyMenu` ("Body Settings"): the body's placement and its offsets per movement,
//! hands, action and race, on IC's tabs.

use framework::egui;

use super::widgets::{offset, rows, section_header, slider, tabs};
use crate::settings::{Offset, Profile};

const TABS: [&str; 6] = ["Placement", "Directional", "Movement", "Hands", "Action", "Race"];

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        match tabs(ui, "BodyTabs", &TABS) {
            0 => placement(ui, p),
            1 => directional(ui, p),
            2 => movement(ui, p),
            3 => hands(ui, p),
            4 => action(ui, p),
            _ => race(ui, p),
        }
    });
}

fn placement(ui: &mut egui::Ui, p: &mut Profile) {
    let b = &mut p.Body;
    section_header(ui, "Camera anchor");
    rows(ui, "place_anchor", |ui| {
        slider(ui, "Camera Pos Forward", &mut b.fAnchorForward, 0.0, 48.0, 2, "");
        slider(ui, "Camera Pos Up", &mut b.fAnchorUp, -16.0, 24.0, 2, "");
    });
    section_header(ui, "Global body offset");
    rows(ui, "place_global", |ui| {
        let mut body = Offset {
            fForward: b.fForward,
            fUp: b.fUp,
            fRight: b.fRight,
        };
        offset(ui, "Body", &mut body);
        (b.fForward, b.fUp, b.fRight) = (body.fForward, body.fUp, body.fRight);
        offset(ui, "Body Combat", &mut b.Combat);
    });
    section_header(ui, "Directional sway");
    rows(ui, "place_sway", |ui| {
        slider(ui, "Sway Intensity", &mut b.fDirectionalIntensity, 0.0, 5.0, 2, "");
        slider(ui, "Sway Speed", &mut b.fDirectionalSpeed, 1.0, 20.0, 2, "");
    });
    section_header(ui, "Directional yaw smoothing");
    ui.checkbox(&mut b.bYawSmoothing, "Enable yaw smoothing");
    rows(ui, "place_yaw", |ui| {
        slider(ui, "Yaw Turn Speed", &mut b.fYawSmoothingSpeed, 1.0, 30.0, 2, "");
        slider(ui, "Yaw Max Twist (deg)", &mut b.fYawSmoothingMaxAngle, 0.0, 90.0, 2, "");
    });
    section_header(ui, "First person free look");
    ui.horizontal(|ui| {
        ui.checkbox(&mut b.bFreeLook, "Enable free look");
        ui.checkbox(&mut b.bFreeLookSpine, "Spine twist");
    });
    rows(ui, "place_freelook", |ui| {
        slider(ui, "Look Threshold (deg)", &mut b.fFreeLookMaxAngle, 0.0, 180.0, 2, "");
        slider(ui, "Settle Angle (deg)", &mut b.fFreeLookSettleAngle, 0.0, 90.0, 2, "");
        slider(ui, "Shuffle Speed (x10 deg/s)", &mut b.fFreeLookCatchupSpeed, 1.0, 30.0, 2, "");
        slider(ui, "Twist Strength", &mut b.fFreeLookTwistStrength, 0.1, 3.0, 2, "");
    });
    section_header(ui, "Turn animation");
    ui.checkbox(&mut b.bDisableTurnAnimation, "Disable turn animation");
}

fn directional(ui: &mut egui::Ui, p: &mut Profile) {
    let d = &mut p.Directional;
    rows(ui, "dir", |ui| {
        offset(ui, "Forward", &mut d.Forward);
        offset(ui, "Forward Right", &mut d.ForwardRight);
        offset(ui, "Right", &mut d.Right);
        offset(ui, "Back Right", &mut d.BackRight);
        offset(ui, "Back", &mut d.Back);
        offset(ui, "Back Left", &mut d.BackLeft);
        offset(ui, "Left", &mut d.Left);
        offset(ui, "Forward Left", &mut d.ForwardLeft);
    });
}

fn movement(ui: &mut egui::Ui, p: &mut Profile) {
    let m = &mut p.Movement;
    section_header(ui, "Base offsets");
    rows(ui, "mv_base", |ui| {
        offset(ui, "Idle", &mut m.Idle);
        offset(ui, "Walk", &mut m.WalkBase);
        offset(ui, "Run", &mut m.RunBase);
        offset(ui, "Sprint", &mut m.SprintBase);
    });
    section_header(ui, "Walk / Run directional");
    rows(ui, "mv_dir", |ui| {
        offset(ui, "Forward", &mut m.DirForward);
        offset(ui, "Forward Right", &mut m.DirForwardRight);
        offset(ui, "Forward Left", &mut m.DirForwardLeft);
        offset(ui, "Back", &mut m.DirBack);
        offset(ui, "Back Right", &mut m.DirBackRight);
        offset(ui, "Back Left", &mut m.DirBackLeft);
        offset(ui, "Right", &mut m.DirRight);
        offset(ui, "Left", &mut m.DirLeft);
        offset(ui, "Default", &mut m.DirDefault);
    });
    section_header(ui, "Sprint directional");
    rows(ui, "mv_sprint", |ui| {
        offset(ui, "Forward", &mut m.SprintForward);
        offset(ui, "Forward Right", &mut m.SprintForwardRight);
        offset(ui, "Forward Left", &mut m.SprintForwardLeft);
        offset(ui, "Back", &mut m.SprintBack);
        offset(ui, "Back Right", &mut m.SprintBackRight);
        offset(ui, "Back Left", &mut m.SprintBackLeft);
        offset(ui, "Right", &mut m.SprintRight);
        offset(ui, "Left", &mut m.SprintLeft);
        offset(ui, "Default", &mut m.SprintDefault);
    });
    section_header(ui, "Timing");
    rows(ui, "mv_timing", |ui| {
        slider(ui, "Idle Lerp", &mut m.fIdleLerp, 0.5, 40.0, 2, "");
        slider(ui, "Walk Lerp", &mut m.fWalkLerp, 0.5, 40.0, 2, "");
        slider(ui, "Run Lerp", &mut m.fRunLerp, 0.5, 40.0, 2, "");
        slider(ui, "Sprint Lerp", &mut m.fSprintLerp, 0.5, 40.0, 2, "");
        slider(ui, "Walk Intensity", &mut m.fWalkIntensity, 0.0, 5.0, 2, "");
        slider(ui, "Run Intensity", &mut m.fRunIntensity, 0.0, 5.0, 2, "");
        slider(ui, "Sprint Intensity", &mut m.fSprintIntensity, 0.0, 5.0, 2, "");
    });
}

fn hands(ui: &mut egui::Ui, p: &mut Profile) {
    let h = &mut p.Hands;
    rows(ui, "hands_tbl", |ui| {
        offset(ui, "Sheath", &mut h.Sheath);
        offset(ui, "Show", &mut h.Show);
        offset(ui, "Aim", &mut h.Aim);
        offset(ui, "Attack", &mut h.Attack);
        offset(ui, "Block", &mut h.Block);
        offset(ui, "Cast", &mut h.Cast);
        offset(ui, "Power Attack", &mut h.PowerAttack);
        slider(ui, "Lerp", &mut h.fLerp, 0.5, 40.0, 2, "");
    });
}

fn action(ui: &mut egui::Ui, p: &mut Profile) {
    let a = &mut p.Action;
    rows(ui, "action_tbl", |ui| {
        for (label, o) in [
            ("AI", &mut a.AI),
            ("Animation", &mut a.Animation),
            ("Bathing", &mut a.Bathing),
            ("Boat Ride", &mut a.BoatRide),
            ("Cart Ride", &mut a.CartRide),
            ("Craft", &mut a.Craft),
            ("Dialogue", &mut a.Dialogue),
            ("Fall", &mut a.Fall),
            ("Fish", &mut a.Fish),
            ("Gather", &mut a.Gather),
            ("Giantess", &mut a.Giantess),
            ("Parkour", &mut a.Parkour),
            ("Ride", &mut a.Ride),
            ("Sex", &mut a.Sex),
            ("Sit", &mut a.Sit),
            ("Sleep", &mut a.Sleep),
            ("Sneak", &mut a.Sneak),
            ("Stagger", &mut a.Stagger),
            ("Swim", &mut a.Swim),
            ("View", &mut a.View),
            ("Transform (Vampire Lord)", &mut a.VampirelordTransition),
            ("Transform (Werewolf)", &mut a.WerewolfTransition),
        ] {
            offset(ui, label, o);
        }
        slider(ui, "Lerp", &mut a.fLerp, 0.5, 40.0, 2, "");
    });
}

fn race(ui: &mut egui::Ui, p: &mut Profile) {
    let r = &mut p.Race;
    rows(ui, "race_tbl", |ui| {
        for (label, o) in [
            ("Argonian", &mut r.Argonian),
            ("Argonian Vampire", &mut r.ArgonianVampire),
            ("Breton", &mut r.Breton),
            ("Breton Vampire", &mut r.BretonVampire),
            ("Dark Elf", &mut r.Darkelf),
            ("Dark Elf Vampire", &mut r.DarkelfVampire),
            ("High Elf", &mut r.Highelf),
            ("High Elf Vampire", &mut r.HighelfVampire),
            ("Imperial", &mut r.Imperial),
            ("Imperial Vampire", &mut r.ImperialVampire),
            ("Khajiit", &mut r.Khajiit),
            ("Khajiit Vampire", &mut r.KhajiitVampire),
            ("Nord", &mut r.Nord),
            ("Nord Vampire", &mut r.NordVampire),
            ("Orc", &mut r.Orc),
            ("Orc Vampire", &mut r.OrcVampire),
            ("Redguard", &mut r.Redguard),
            ("Redguard Vampire", &mut r.RedguardVampire),
            ("Wood Elf", &mut r.Woodelf),
            ("Wood Elf Vampire", &mut r.WoodelfVampire),
            ("Werewolf", &mut r.Werewolf),
            ("Vampire Lord", &mut r.Vampirelord),
            ("Werebear", &mut r.Werebear),
            ("Lich", &mut r.Lich),
            ("Half Dragon", &mut r.HalfDragon),
            ("Half Dragon Vampire", &mut r.HalfDragonVampire),
        ] {
            offset(ui, label, o);
        }
    });
}
