//! `ui/menu/DialogueMenu` ("Dialogue Settings"): the dialogue camera's behaviour and framing, on
//! IC's tabs.

use framework::egui;

use super::widgets::{rows, section_header, slider, tabs, toggle};

pub fn draw(ui: &mut egui::Ui) {
    super::edit(ui, |ui, p| {
        let d = &mut p.Dialogue;
        rows(ui, "DialogueMaster", |ui| {
            toggle(ui, "Dialogue Camera", &mut d.bEnable, "Enable/Disable the cinematic dialogue camera");
        });
        let tab = tabs(ui, "DialogueTabs", &["Behaviour", "Framing"]);
        if !d.bEnable {
            ui.disable();
        }
        if tab == 0 {
            rows(ui, "DialogueBehaviour", |ui| {
                toggle(ui, "First Person Dialogue", &mut d.bEnableFirstPerson, "Enable/Disable the dialogue camera when starting in first person");
                toggle(ui, "Third Person Dialogue", &mut d.bEnableThirdPerson, "Enable/Disable the dialogue camera when starting in third person");
                toggle(ui, "Force First Person", &mut d.bForceFirstPerson, "Switch to first person for the conversation (restored afterwards)");
                toggle(ui, "Force Third Person", &mut d.bForceThirdPerson, "Switch to third person for the conversation (restored afterwards, overrides Force First Person)");
                toggle(ui, "Smooth Transition", &mut d.bSmoothTransition, "Blend the camera in/out of dialogue and between speaker focus shots");
                toggle(ui, "SmoothCam API", &mut d.bSmoothCamAPI, "Request camera control from SmoothCam during dialogue");
                toggle(ui, "Face Speaker", &mut d.bFaceSpeaker, "Turn the camera to face the speaker, disable to look around freely");
                toggle(ui, "Head Tracking", &mut d.bHeadTracking, "Turn the player's head toward the speaker");
                toggle(ui, "Hide Topic List", &mut d.bHideDialogueMenu, "Hide the dialogue topic list while the speaker is talking");
                toggle(ui, "Reposition HUD Messages", &mut d.bRepositionHUDMessages, "Move HUD notifications out of the conversation during dialogue");
                toggle(ui, "Speaker Focus", &mut d.bSwitchTarget, "Place the camera over the speaker's shoulder facing the player (requires first person body)");
                toggle(ui, "Speaker Focus Lock On", &mut d.bLockOn, "Keep the speaker focus shot between lines, otherwise only while a line plays");
                toggle(ui, "Speaker Focus Fade", &mut d.bFadeToBlack, "Fade to black whenever the speaker focus framing changes instead of cutting");
                toggle(ui, "Letterbox", &mut d.bLetterbox, "Show letterbox bars during dialogue (requires the letterbox master switch in Effects)");
            });
            return;
        }
        section_header(ui, "Timing");
        rows(ui, "DialogueTiming", |ui| {
            slider(ui, "Camera Speed", &mut d.fCameraSpeed, 50.0, 2000.0, 0, "How quickly the camera turns to face the speaker");
            slider(ui, "Fade Speed", &mut d.fFadeSpeed, 0.5, 10.0, 1, "How quickly the screen fades to black and back for speaker focus changes");
        });
        section_header(ui, "Zoom");
        rows(ui, "DialogueZoom", |ui| {
            slider(ui, "First Person", &mut d.fFirstPersonZoom, 10.0, 200.0, 0, "Preferred framing distance for first person dialogue");
            slider(ui, "Third Person", &mut d.fThirdPersonZoom, 10.0, 300.0, 0, "Preferred framing distance for third person dialogue");
            slider(ui, "Dragon", &mut d.fDragonZoom, 50.0, 500.0, 0, "Preferred framing distance when talking to flying creatures");
            slider(ui, "Default World FOV", &mut d.fDefaultWorldFOV, 30.0, 120.0, 0, "FOV used while no speaker can be framed");
        });
        section_header(ui, "HUD Messages");
        rows(ui, "DialogueHUD", |ui| {
            slider(ui, "Position X", &mut d.fMessagePosX, -601.0, 750.0, 0, "HUD message X position during dialogue (-601 keeps the original position)");
            slider(ui, "Position Y", &mut d.fMessagePosY, -750.0, 750.0, 0, "HUD message Y position during dialogue");
        });
        section_header(ui, "Player Shoulder");
        rows(ui, "DialoguePlayerShoulder", |ui| {
            slider(ui, "Side", &mut d.fPlayerShoulderPosX, -200.0, 200.0, 0, "Sideways offset while the camera sits over the player's shoulder");
            slider(ui, "Forward", &mut d.fPlayerShoulderPosY, -200.0, 200.0, 0, "Forward offset while the camera sits over the player's shoulder");
            slider(ui, "Up", &mut d.fPlayerShoulderPosZ, -200.0, 200.0, 0, "Height offset while the camera sits over the player's shoulder");
        });
        section_header(ui, "NPC Shoulder");
        rows(ui, "DialogueNpcShoulder", |ui| {
            slider(ui, "Side", &mut d.fNpcShoulderPosX, -200.0, 200.0, 0, "Sideways offset while the camera sits over the NPC's shoulder");
            slider(ui, "Forward", &mut d.fNpcShoulderPosY, -200.0, 200.0, 0, "Forward offset while the camera sits over the NPC's shoulder");
            slider(ui, "Up", &mut d.fNpcShoulderPosZ, -200.0, 200.0, 0, "Height offset while the camera sits over the NPC's shoulder");
        });
        section_header(ui, "Focus Anchor");
        rows(ui, "DialogueAnchor", |ui| {
            slider(ui, "Human Height", &mut d.fHumanCamOffsetZ, -50.0, 50.0, 0, "Anchor height above a humanoid speaker's head");
            slider(ui, "Creature Height", &mut d.fCreatureCamOffsetZ, -100.0, 100.0, 0, "Anchor height above a creature speaker");
        });
        section_header(ui, "Letterbox");
        rows(ui, "DialogueLetterbox", |ui| {
            slider(ui, "Bar Size", &mut d.fLetterboxRatio, 0.01, 0.25, 2, "Height of each bar as a fraction of the screen");
            slider(ui, "Transition Speed", &mut d.fLetterboxTransitionSpeed, 0.5, 10.0, 1, "How quickly the letterbox animates in and out");
        });
    });
}
