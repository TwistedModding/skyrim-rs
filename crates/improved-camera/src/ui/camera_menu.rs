//! `ui/menu/CameraMenu` ("Camera Settings"): the camera offset of each camera state.

use framework::egui;

use super::widgets::camera_offset;
use crate::settings::CamOffset;

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        let mut player = CamOffset {
            forward: p.Firstperson.forward,
            up: p.Firstperson.up,
            right: p.Firstperson.right,
        };
        let mut horse = p.Horse.offset();
        let c = &mut p.Camera;
        super::halves(ui, |ui, right| {
            camera_offset(ui, "Player", &mut player);
            camera_offset(ui, "Player Combat", &mut p.Firstperson.Combat);
            camera_offset(ui, "Horse", &mut horse);
            camera_offset(ui, "Horse Combat", &mut p.Horse.Combat);
            camera_offset(ui, "Dragon", &mut c.Dragon);
            let ui = right;
            camera_offset(ui, "Vampire Lord", &mut c.VampireLord);
            camera_offset(ui, "Werewolf", &mut c.Werewolf);
            camera_offset(ui, "Lich", &mut c.Lich);
            camera_offset(ui, "Sex", &mut c.Sex);
            camera_offset(ui, "Spectator", &mut c.Spectator);
        });
        (p.Firstperson.forward, p.Firstperson.up, p.Firstperson.right) = (player.forward, player.up, player.right);
        (p.Horse.forward, p.Horse.up, p.Horse.right) = (horse.forward, horse.up, horse.right);
    });
}
