//! `ui/menu/GeneralMenu` ("General Settings"): the body, the head and the arms. The side effects
//! of its toggles are `apply_profile`'s.

use framework::egui;

use super::widgets::{rows, section_header, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let g = &mut p.General;
        super::halves(ui, |ui, right| {
            section_header(ui, "First Person");
            rows(ui, "GeneralLeftBody", |ui| {
                toggle(ui, "First Person Body", &mut g.bEnableBody, "Enable/Disable first person body");
                if !g.bEnableBody {
                    ui.disable();
                }
                toggle(ui, "Body in Console", &mut g.bEnableBodyConsole, "Enable/disable first person body when console is open");
                toggle(ui, "Fake First Person", &mut g.bEnableFakeFirstPerson, "Enable/Disable fake first person, similar to IFPV");
            });
            ui.add_enabled_ui(g.bEnableBody, |ui| {
                section_header(ui, "Head");
                rows(ui, "GeneralLeftHead", |ui| {
                    toggle(ui, "Default", &mut g.bEnableHead, "Enable/Disable first person head (May make hair visible)");
                    toggle(
                        ui,
                        "Combat",
                        &mut g.bEnableHeadCombat,
                        "Enable/Disable first person head when weapons are drawn (May make hair visible)",
                    );
                    toggle(ui, "Horse", &mut g.bEnableHeadHorse, "Enable/Disable first person head when mounted (May make hair visible)");
                    toggle(ui, "Dragon", &mut g.bEnableHeadDragon, "Enable/Disable first person head when mounted (May make hair visible)");
                    toggle(ui, "Vampire Lord", &mut g.bEnableHeadVampireLord, "Enable/Disable first person head for Vampire Lord");
                    toggle(ui, "Werewolf", &mut g.bEnableHeadWerewolf, "Enable/Disable first person head for Werewolf");
                    toggle(ui, "Sex", &mut g.bEnableHeadSex, "Enable/Disable first person head during sex animations");
                    toggle(ui, "Spectator", &mut g.bEnableHeadSpectator, "Enable/Disable first person head for spectator");
                });
            });
            let ui = right;
            if g.bEnableFakeFirstPerson || !g.bEnableBody {
                ui.disable();
            }
            section_header(ui, "Arms");
            rows(ui, "GeneralRightArms", |ui| {
                toggle(ui, "First Person Arms", &mut g.bEnableFirstPersonArms, "Enable/Disable First Person arms");
                if !g.bEnableFirstPersonArms {
                    ui.disable();
                }
                toggle(
                    ui,
                    "Arms for Movement",
                    &mut g.bEnableFirstPersonArmsMovement,
                    "Enable/Disable first person arms for movement (Sprinting, Swimming and Jumping)",
                );
            });
            section_header(ui, "Third Person Equipment");
            rows(ui, "GeneralRightThird", |ui| {
                if !g.bEnableFirstPersonArms {
                    ui.disable();
                }
                toggle(ui, "Torch", &mut g.bEnableThirdPersonTorch, "Enable/Disable third person torch when using first person arms");
                toggle(
                    ui,
                    "Torch Block",
                    &mut g.bEnableThirdPersonTorchBlock,
                    "Enable/Disable third person torch blocking when using first person arms",
                );
                toggle(ui, "Shield", &mut g.bEnableThirdPersonShield, "Enable/Disable third person shield when using first person arms");
                toggle(
                    ui,
                    "Shield Block",
                    &mut g.bEnableThirdPersonShieldBlock,
                    "Enable/Disable third person shield blocking when using first person arms",
                );
                toggle(ui, "Bow", &mut g.bEnableThirdPersonBow, "Enable/Disable third person bow when using first person arms");
                toggle(ui, "Bow Aim", &mut g.bEnableThirdPersonBowAim, "Enable/Disable third person bow aiming when using first person arms");
                toggle(ui, "Crossbow", &mut g.bEnableThirdPersonCrossbow, "Enable/Disable third person crossbow when using first person arms");
                toggle(
                    ui,
                    "Crossbow Aim",
                    &mut g.bEnableThirdPersonCrossbowAim,
                    "Enable/Disable third person crossbow aiming when using first person arms",
                );
            });
        });
    });
}
