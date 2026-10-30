//! `ui/menu/StabilizationMenu` ("Stabilization Settings"): how fake first person rides the
//! animated head in each camera.

use framework::egui;

use super::widgets::{rows, section_header, slider, toggle};
use crate::settings::Stab;

/// `StabilizationSection`: head follow only reaches the cameras aimed down the head.
fn section(ui: &mut egui::Ui, label: &str, s: &mut Stab, head_follow: bool) {
    section_header(ui, label);
    rows(ui, label, |ui| {
        toggle(ui, "Position Smoothing", &mut s.bEnable, "Damp the animated head motion carrying the camera");
        let follow_hint = if head_follow {
            "How hard the view chases the head: 1 tracks it exactly, lower damps the animation wobble out. The view stays on the head at any setting"
        } else {
            "This camera does not aim down the head"
        };
        ui.label("Head Follow Strength");
        ui.add_enabled(head_follow, egui::Slider::new(&mut s.fHeadFollowStrength, 0.0..=1.0).fixed_decimals(2))
            .on_hover_text(follow_hint)
            .on_disabled_hover_text(follow_hint);
        ui.end_row();
        if !s.bEnable {
            ui.disable();
        }
        slider(ui, "Smoothing Speed", &mut s.fSmoothingSpeed, 2.0, 40.0, 0, "How tightly the camera chases the head, lower is smoother");
        slider(ui, "Max Offset", &mut s.fSmoothingMaxOffset, 0.0, 20.0, 0, "How far the camera may ever lag behind the head");
    });
}

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        let s = &mut p.Stabilization;
        super::halves(ui, |ui, right| {
            section(ui, "Horse", &mut s.Horse, true);
            section(ui, "Dragon", &mut s.Dragon, true);
            section(ui, "Vampire Lord", &mut s.VampireLord, true);
            section(ui, "Werewolf", &mut s.Werewolf, true);
            section(ui, "Lich", &mut s.Lich, true);
            section(ui, "Sex", &mut s.Sex, true);
            let ui = right;
            section(ui, "Furniture", &mut s.Furniture, false);
            section(ui, "Crafting", &mut s.Crafting, false);
            section(ui, "Gathering", &mut s.Gathering, false);
            section(ui, "Bleedout", &mut s.Bleedout, false);
            section(ui, "Transition", &mut s.Transition, false);
        });
    });
}
