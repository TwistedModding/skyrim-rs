//! `ui/menu/VanityMenu` ("Idle Camera Settings"): the idle camera's behaviour, letterbox, orbit,
//! collision and NPC focus (stored, not applied: the focus is not ported).

use framework::egui;

use super::widgets::{combo, rows, section_header, slider, toggle};
use crate::camera::util::interpolation::METHOD_NAMES;

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let v = &mut p.Vanity;
        rows(ui, "VanityMaster", |ui| {
            toggle(ui, "Idle Camera", &mut v.bEnable, "Take over the auto vanity camera once the idle timer fires (requires the first person body)");
        });
        if !v.bEnable {
            ui.disable();
        }
        section_header(ui, "Behaviour");
        rows(ui, "VanityBehaviour", |ui| {
            toggle(ui, "Hide HUD", &mut v.bToggleMenus, "Hide the game HUD while the idle camera runs (toggle menus), restored when it ends");
            slider(ui, "Idle Delay", &mut v.fIdleDelay, 5.0, 300.0, 0, "Seconds without input before the idle camera starts");
        });
        if ui.add(egui::Button::new("Start Now").min_size(egui::vec2(120.0, 0.0))).clicked() {
            crate::camera::base::trigger_idle();
        }
        section_header(ui, "Letterbox");
        rows(ui, "VanityLetterbox", |ui| {
            toggle(ui, "Enabled", &mut v.bLetterbox, "Show letterbox bars while the idle camera runs (requires the letterbox master switch in Effects)");
            if !v.bLetterbox {
                ui.disable();
            }
            slider(ui, "Bar Size", &mut v.fLetterboxRatio, 0.01, 0.25, 2, "Height of each bar as a fraction of the screen");
            slider(ui, "Transition Speed", &mut v.fLetterboxTransitionSpeed, 0.5, 10.0, 1, "How quickly the letterbox animates in and out");
        });
        section_header(ui, "Orbit");
        rows(ui, "VanityOrbit", |ui| {
            slider(ui, "Speed", &mut v.fOrbitSpeed, -30.0, 30.0, 1, "Turn rate in degrees per second, negative reverses the direction");
            slider(ui, "Distance", &mut v.fDistance, 100.0, 600.0, 0, "How far the camera circles from the player");
            slider(ui, "Height", &mut v.fHeight, -50.0, 200.0, 0, "Camera height above the player's head, sets the down angle");
            slider(ui, "FOV", &mut v.fFOV, 30.0, 100.0, 0, "Field of view while the idle camera runs");
        });
        section_header(ui, "Collision");
        rows(ui, "VanityCollision", |ui| {
            toggle(ui, "Smoothing", &mut v.bCollisionSmoothing, "Interpolate the camera toward a new obstruction and back out once it clears instead of snapping");
            if !v.bCollisionSmoothing {
                ui.disable();
            }
            combo(ui, "Pull-In Method", &METHOD_NAMES, &mut v.iCollisionPullInMethod, "Easing curve used to reach a new obstruction");
            slider(ui, "Pull-In Duration", &mut v.fCollisionPullInDuration, 0.05, 1.0, 2, "Seconds to reach a new obstruction; longer values may clip briefly");
            combo(ui, "Recovery Method", &METHOD_NAMES, &mut v.iCollisionRecoveryMethod, "Easing curve used to return once the obstruction is gone");
            slider(ui, "Recovery Duration", &mut v.fCollisionRecoveryDuration, 0.05, 2.0, 2, "Seconds to return to full distance once the obstruction is gone");
        });
        section_header(ui, "NPC Focus");
        rows(ui, "VanityFocus", |ui| {
            toggle(ui, "Enabled", &mut v.bNPCFocus, "Swing the aim onto a point beside the player toward a random visible NPC, animal or creature while orbiting");
            if !v.bNPCFocus {
                ui.disable();
            }
            slider(ui, "Radius", &mut v.fFocusRadius, 250.0, 4000.0, 0, "How far away a focus subject may be");
            slider(ui, "Chance", &mut v.fFocusChance, 0.0, 100.0, 0, "Percent chance to pick a subject each time the cooldown elapses");
            slider(ui, "Aim Offset", &mut v.fFocusOffset, 25.0, 400.0, 0, "How far from the player, toward the subject, the fixed aim point sits");
            slider(ui, "Duration", &mut v.fFocusDuration, 1.0, 20.0, 1, "Seconds the aim holds before easing back to the player");
            slider(ui, "Cooldown", &mut v.fFocusCooldown, 1.0, 60.0, 1, "Seconds between focus shots");
            slider(ui, "Blend Time", &mut v.fFocusBlendTime, 0.2, 5.0, 1, "Seconds to swing the aim to and from the subject");
            slider(ui, "FOV Offset", &mut v.fFocusFOVOffset, -30.0, 0.0, 0, "Extra zoom while a subject holds the focus");
        });
    });
}
