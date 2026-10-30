//! `hook/MiscHook`: the body's per-frame entry, before the third-person model update in
//! `PlayerCharacter::UpdateNodesAfterRender` (+0x94; the first-person update follows at +0xd7);
//! the movement controller's third-person test, so first person with head bob buffers its input
//! as third person does; the events sent into the player's graph and the tags it sends, for
//! `Is_Animation_Playing`; the photo mode's sun override after `Sky::Update`; and the Papyrus
//! `Game.ForceFirstPerson` and `Game.ForceThirdPerson`, rebound to tell the camera manager.

use framework::Host;
use framework::hooks::ops::{Call, Raw, Sky__Update};
use framework::hooks::{Error, Location, Options};
use skyrim::papyrus::VirtualMachine;
use skyrim::BSFixedString;
use sre::{BSEventNotifyControl, NiAVObject, TESObjectREFR};
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

/// `IAnimationGraphManagerHolder::NotifyAnimationGraph(holder, const BSFixedString& event) -> bool`,
/// where `AIProcess::PlayIdle` sends an idle's event into the graph (RE `camera.md` IC6 C).
type NotifyGraph = unsafe extern "C" fn(usize, usize) -> usize;

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
    let notify = skyrim::reloc().address(fns::IAnimationGraphManagerHolder__NotifyAnimationGraph).map_err(|e| Error::Raw { address: 0, detail: e.to_string() })?;
    hooks.before(Raw::<NotifyGraph>::at(notify), Options::new(host.name()), |args| {
        if let (Some(player), Some(event)) = (util::player(), unsafe { (args.1 as *const sre::BSFixedString).as_ref() })
            && args.0 == core::ptr::from_ref(player.as_i_animation_graph_manager_holder()) as usize
        {
            util::on_player_graph_event(BSFixedString::view(event).as_bytes());
        }
    })?;
    hooks.after(Sky__Update, Options::new(host.name()), |args, (), _| {
        if let Some(sky) = unsafe { args.0.as_ref() } {
            crate::camera::studio::environment::apply_sun(sky);
        }
    })?;
    host.add_player_graph_sink(|event| {
        if util::player().is_some_and(|p| core::ptr::eq(event.owner.cast_const(), core::ptr::from_ref::<TESObjectREFR>(p))) {
            util::on_player_graph_event(BSFixedString::view(&event.tag).as_bytes());
        }
        BSEventNotifyControl::Continue
    });
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
