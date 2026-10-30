//! `ui/menu/HideMenu` ("Hide Settings"): what the first-person body hides.

use framework::egui;

use super::widgets::{rows, section_header, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        if !p.General.bEnableBody {
            ui.disable();
        }
        let h = &mut p.Hide;
        section_header(ui, "Worn On Back");
        rows(ui, "HideTableTop", |ui| {
            toggle(ui, "Bow", &mut h.bBow, "Enable/Disable hiding of the bow on your back");
            toggle(ui, "Quiver", &mut h.bQuiver, "Enable/Disable hiding of the quiver on your back");
            toggle(ui, "Two Handed", &mut h.bTwoHanded, "Enable/Disable hiding of two handed weapon on your back");
        });
        section_header(ui, "First Person Body");
        rows(ui, "HideTableBottom", |ui| {
            toggle(ui, "Body", &mut h.bBody, "Enable/Disable hiding of the body");
            toggle(ui, "Weapon Drawn", &mut h.bWeaponDrawn, "Enable/Disable hiding of the body when weapons are drawn");
            toggle(ui, "Aiming", &mut h.bAiming, "Enable/Disable hiding of the body when aiming with bow/crossbow");
            toggle(ui, "Power Attacking", &mut h.bPowerAttack, "Enable/Disable hiding of the body when power attacking");
            toggle(ui, "Jumping/Falling", &mut h.bJumping, "Enable/Disable hiding of the body when jumping/falling");
            toggle(ui, "Jump Combat", &mut h.bJumpCombat, "Same as above but only when weapons are drawn");
            toggle(ui, "Sneak Rolling", &mut h.bSneakRoll, "Enable/Disable hiding of the body when sneak rolling");
            toggle(ui, "Swimming", &mut h.bSwimming, "Enable/Disable hiding of the body when swimming");
        });
    });
}
