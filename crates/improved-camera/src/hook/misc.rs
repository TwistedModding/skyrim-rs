//! `hook/MiscHook`: the body's per-frame entry, before the third-person model update in
//! `PlayerCharacter::UpdateNodesAfterRender` (+0x94; the first-person update follows at +0xd7);
//! the movement controller's third-person test, so first person with head bob buffers its input
//! as third person does; and the Papyrus `Game.ForceFirstPerson` and `Game.ForceThirdPerson`,
//! rebound to tell the camera manager.

use framework::Host;
use framework::hooks::ops::Call;
use framework::hooks::{Error, Location, Options};
use skyrim::papyrus::VirtualMachine;
use sre::NiAVObject;
use sre::raw::fns;

use super::IsThirdPerson;
use crate::body::manager::update_animation_third_person;
use crate::camera::manager;
use crate::settings;
use crate::util::skyrim_util as util;

const UPDATE_THIRD_PERSON_CALL: usize = 0x94;

/// `MovementHandlerAgentPlayerControls::HandleMovement` +0x303: `IsThirdPerson` on the player,
/// which feeds the controller's direction buffer (`fControllerBufferDepth:Control`) only in third
/// person (RE `camera.md` sections 10 and 12).
const MOVEMENT_VIEW_CALL: usize = 0x303;

type UpdateNode = unsafe extern "C" fn(usize, usize) -> usize;

pub fn install(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    hooks.before(Call::<UpdateNode>::at(Location::id(fns::PlayerCharacter__UpdateNodesAfterRender, UPDATE_THIRD_PERSON_CALL)), Options::new(host.name()), |args| {
        if util::is_body_valid()
            && !util::is_free_camera()
            && !util::is_menu_open("MapMenu")
            && let Some(third) = unsafe { (args.0 as *const NiAVObject).as_ref() }
        {
            update_animation_third_person(third, false);
        }
    })?;
    hooks.replace(Call::<IsThirdPerson>::at(Location::id(fns::MovementHandlerAgentPlayerControls__HandleMovement, MOVEMENT_VIEW_CALL)), Options::new(host.name()), |_, _| usize::from(smooth_animation_transitions()))?;
    host.on_papyrus(register_natives);
    Ok(())
}

/// `SmoothAnimationTransitions`: first person with the body and head bob (with a weapon drawn,
/// combat head bob) counts as third person while the movement controls are on.
fn smooth_animation_transitions() -> bool {
    let Some(player) = util::player() else { return false };
    if util::is_engine_first_person() {
        let profile = settings::profile();
        let a = &profile.Advanced;
        let headbob = if util::is_weapon_drawn(player) { a.bHeadbobCombat } else { a.bHeadbob };
        if profile.General.bEnableBody && headbob && util::movement_controls_enabled() {
            return true;
        }
    }
    util::third_person_flag(player)
}

/// IC's `ForceFirstPerson` and `ForceThirdPerson`, in place of the engine's natives: the camera's
/// own switch, then the manager's note. They run on Papyrus VM threads, as the engine's do.
fn register_natives(vm: &'static VirtualMachine) {
    let Some(host) = crate::HOST.get() else { return };
    let natives: [(&str, fn()); 2] = [("ForceFirstPerson", force_first_person), ("ForceThirdPerson", force_third_person)];
    for (name, f) in natives {
        if let Err(e) = host.register_native(vm, "Game", name, f) {
            crate::log(&format!("Game.{name} not bound: {e}"));
        }
    }
}

fn force_first_person() {
    if let Some(camera) = util::camera() {
        camera.force_first_person();
    }
    crate::log("ForceFirstPerson called.");
    manager::force_first_person();
}

fn force_third_person() {
    if let Some(camera) = util::camera() {
        camera.force_third_person();
    }
    crate::log("ForceThirdPerson called.");
    manager::force_third_person();
}
