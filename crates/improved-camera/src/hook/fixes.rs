//! `hook/FixesHook`: the body in first person. `SetFirstPerson` keeps the third-person root
//! visible through a managed patch and IC's after-switch visibility check; after the shadow maps
//! are drawn (`RenderWorldScene` +0x30a) the parts the player should not see are shrunk for the
//! main view, with the skins' palettes and the rigid gear's previous transforms fixed up. The
//! hands camera takes the hands FOV, and the tween menu starts from the live world FOV.

use std::collections::HashMap;
use std::sync::Mutex;

use framework::Host;
use framework::hooks::ops::{Call, PlayerCharacter__SetFirstPerson};
use sre::raw::fns::{NiCamera__SetPerspectiveFromFov_fn as SetPerspectiveFromFov, PlayerCamera__GetDefaultFOVForCurrentState_fn as GetDefaultFov};
use framework::hooks::{Error, Location, Options, PatchId};
use skyrim::prelude::*;
use skyrim::render::Transform;
use sre::raw::fns;
use sre::{BSFadeNode, BSGeometry, NiAVObject, NiNode};

use crate::body::manager::sync_bone_tree;
use crate::body::parts::{self, SHEATHED_NODES, SHRUNK};
use crate::settings;
use crate::util::skyrim_util as util;

/// `SetFirstPerson` +0xc5, `or ecx, 1`: the bit that hides the third-person root on the switch to
/// first person. A 3-byte NOP keeps every other flag of the root (`camera.md` section 10).
const SET_FIRST_PERSON_HIDE: usize = 0xc5;
const HIDE_ORIGINAL: [u8; 3] = [0x83, 0xC9, 0x01];
const HIDE_NOP: [u8; 3] = [0x0F, 0x1F, 0x00];

/// `RenderWorldScene` +0x30a, the call to `RenderShadowMaps`: every shadow map is drawn when it returns.
const RENDER_SHADOW_MAPS_CALL: usize = 0x30a;

type RenderShadowMaps = unsafe extern "C" fn() -> usize;

/// The two calls that set the hands camera's projection every first-person frame, both with the
/// engine's hands FOV (`camera.md` section 7): `SetupFirstPersonCamera` +0x106, which IC hooks,
/// and `RenderWorldDepthPrepass` +0x7fd, which sets it again before the first-person draws.
const HANDS_PROJECTIONS: [(u64, usize); 2] = [(fns::SetupFirstPersonCamera, 0x106), (fns::RenderWorldDepthPrepass, 0x7fd)];

/// `TweenMenuCameraState::Begin` +0x11, the call for the tween camera's base FOV, which always
/// answers `fDefaultWorldFOV:Display` there.
const TWEEN_BASE_FOV_CALL: usize = 0x11;

static BODY_PATCH: Mutex<Option<PatchId>> = Mutex::new(None);

pub fn install(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    let patch = unsafe { hooks.patch(host.name(), Location::id(fns::PlayerCharacter__SetFirstPerson, SET_FIRST_PERSON_HIDE), &HIDE_ORIGINAL, &HIDE_NOP) }?;
    *BODY_PATCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(patch);
    set_body_patch(host, settings::profile().General.bEnableBody);
    hooks.after(PlayerCharacter__SetFirstPerson, Options::new(host.name()), |args, (), _| {
        if args.1 {
            apply_body_visibility();
        }
    })?;
    hooks.after(Call::<RenderShadowMaps>::at(Location::id(fns::RenderWorldScene, RENDER_SHADOW_MAPS_CALL)), Options::new(host.name()), |(), _, _| register_scene_accumulators())?;
    for (function, offset) in HANDS_PROJECTIONS {
        hooks.before(Call::<SetPerspectiveFromFov>::at(Location::id(function, offset)), Options::new(host.name()), |args| args.1 = settings::profile().FOV.fFirstPersonHands)?;
    }
    hooks.replace(Call::<GetDefaultFov>::at(Location::id(fns::TweenMenuCameraState__Begin, TWEEN_BASE_FOV_CALL)), Options::new(host.name()), |args, original| {
        util::camera().map_or_else(|| unsafe { original.call(args) }, |camera| camera.worldFOV)
    })?;
    Ok(())
}

/// Applies or restores the `SetFirstPerson` patch for the body option.
pub fn set_body_patch(host: &Host, enabled: bool) {
    if let Some(id) = *BODY_PATCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
        let _ = host.hooks().set_patch_enabled(id, enabled);
    }
}

/// `PlayerSwitchSkeleton`'s check after the switch: in first person the third-person root is shown
/// when the body option (with the console open, the console option) is on, hidden otherwise.
pub fn apply_body_visibility() {
    let Some(player) = util::player() else { return };
    if player.is_third_person() {
        return;
    }
    let Some(third) = player.third_person_root() else { return };
    let general = settings::profile().General;
    let shown = if util::is_menu_open("Console") { general.bEnableBodyConsole } else { general.bEnableBody };
    third.set_hidden(!shown);
}

/// `RegisterSceneAccumulators`: after the shadow pass cast the full body.
fn register_scene_accumulators() {
    if !util::is_first_person() || !settings::profile().General.bEnableBody {
        return;
    }
    let Some(third) = util::player().and_then(PlayerCharacterExt::third_person_root) else { return };
    force_gear_fade_in(third);
    shrink_view_hidden_parts(third);
    restore_rigid_previous_worlds(third);
}

/// `ForceGearFadeIn`: the shadow pass drops geometry whose fade node has not faded in, and gear
/// outside the player's view never fades in; skips what the Hide options fade out.
fn force_gear_fade_in(object: &NiAVObject) {
    if object.fadeAmount < 1.0 {
        return;
    }
    if let Some(fade) = object.cast::<BSFadeNode>()
        && fade.currentFade < 1.0
    {
        skyrim::store(&fade.currentFade, 1.0);
    }
    if let Some(node) = object.cast::<NiNode>() {
        for child in node.children().iter().flatten() {
            force_gear_fade_in(child);
        }
    }
}

/// `ShrinkViewHiddenParts`: the hidden arms, head and sheathed gear go to `SHRUNK` for the main
/// view; the body parts restore them before the next shadow pass.
fn shrink_view_hidden_parts(third: &NiAVObject) {
    let hidden = parts::hidden_parts();
    let mut any = false;
    let mut shrink = |node: Option<&NiAVObject>| {
        if let Some(node) = node
            && node.local.scale > 0.5
        {
            node.set_local_scale(SHRUNK);
            node.update_transforms();
            any = true;
        }
    };
    if hidden.left_arm {
        shrink(util::object_by_name(third, "NPC L UpperArm [LUar]"));
    }
    if hidden.right_arm {
        shrink(util::object_by_name(third, "NPC R UpperArm [RUar]"));
    }
    if hidden.head {
        shrink(util::player().and_then(util::head_node).and_then(|h| unsafe { h.parent.cast::<NiAVObject>().as_ref() }));
    }
    for (name, group) in SHEATHED_NODES {
        if group.hidden(hidden) {
            shrink(util::object_by_name(third, name));
        }
    }
    if !any {
        return;
    }
    if let Some(root) = util::object_by_name(third, "NPC Root [Root]") {
        sync_bone_tree(root);
    }
    force_reskin_next_render(third);
}

/// `ForceReskinNextRender`: the skins the shadow pass built this frame rebuild at their next draw.
fn force_reskin_next_render(third: &NiAVObject) {
    let frame = skyrim::render::frame_counter();
    third.for_each(&mut |object| {
        if let Some(skin) = object.cast::<BSGeometry>().and_then(|g| unsafe { g.skinInstance.as_ref() }) {
            skin.rewind_palette(frame);
        }
    });
}

/// The world transform each rigid geometry was drawn with, and the render frame.
static DRAWN_WORLDS: Mutex<Option<HashMap<usize, (Transform, u32)>>> = Mutex::new(None);

/// `RestoreRigidPreviousWorlds`: rigid gear on the body takes last frame's drawn transform as its
/// previous one, so motion vectors do not inherit the camera's move.
fn restore_rigid_previous_worlds(third: &NiAVObject) {
    let frame = skyrim::render::frame_counter();
    let mut guard = DRAWN_WORLDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let drawn = guard.get_or_insert_with(HashMap::new);
    third.for_each(&mut |object| {
        if let Some(geometry) = object.cast::<BSGeometry>()
            && geometry.skinInstance.is_null()
        {
            let entry = drawn.entry(core::ptr::from_ref(geometry) as usize).or_insert((Transform::from(&geometry.world), 0));
            if entry.1 == frame.wrapping_sub(1) {
                object.set_previous_world(entry.0);
            }
            *entry = (object.world_transform(), frame);
        }
    });
    drawn.retain(|_, (_, f)| *f == frame);
}
