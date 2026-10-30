//! `hook/FixesHook`: the body in first person. `SetFirstPerson` keeps the third-person root
//! visible through a managed patch and IC's after-switch visibility check; after the shadow maps
//! are drawn (`RenderWorldScene` +0x30a) the parts the player should not see are shrunk for the
//! main view, with the skins' palettes and the rigid gear's previous transforms fixed up. The
//! hands camera takes the hands FOV, and the tween menu starts from the live world FOV. The magic
//! casters and the effect shaders on the player pick the root IC's arms are on, the crosshair pick
//! starts at the presented camera, and the scroll wheel zooms with the POV switch off. The cell
//! places the body in interior rooms, the camera fades neither the player nor the actors it
//! passes, and seated players look down to 90 degrees.

use std::collections::HashMap;
use std::sync::Mutex;

use framework::Host;
use framework::hooks::ops::{Call, PlayerCharacter__SetFirstPerson};
use framework::hooks::{Error, Location, Next, Options, PatchId, Register};
use skyrim::prelude::*;
use skyrim::render::Transform;
use sre::raw::fns;
use sre::raw::fns::{NiCamera__SetPerspectiveFromFov_fn as SetPerspectiveFromFov, PlayerCamera__GetDefaultFOVForCurrentState_fn as GetDefaultFov};
use sre::{BSFadeNode, BSGeometry, NiAVObject, NiNode};

use super::IsThirdPerson;
use crate::body::manager::{sync_bone_tree, third_person_arms};
use crate::body::parts::{self, SHEATHED_NODES, SHRUNK};
use crate::camera::manager::{self, PitchLimits};
use crate::camera::snapshot;
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

/// `ActorMagicCaster::FindCasterNode` +0x51: `IsThirdPerson` on the player (the call is reached for
/// the player's casters only), which chooses the root the caster's magic node is found in.
const CASTER_VIEW_CALL: usize = 0x51;

/// `PlayerCharacter::UpdateCrosshairPick` +0x1e7, the pick cast `(pick, world, origin*, direction*)`:
/// it only reads the origin and runs on the "Pick reference" worker (RE `camera.md` section 12).
const CROSSHAIR_PICK_CALL: usize = 0x1e7;

type CrosshairPick = unsafe extern "C" fn(usize, usize, usize, usize) -> usize;

/// `ControlMap::MapInputEvents` +0x3cc, `mov eax, [rax+0x120]`: the enabled controls each mapping's
/// required controls (ecx) are tested against. The cave adds `and cl, 0xdf`, so a mapping no
/// longer needs the POV switch (bit 5) and the wheel's zoom stays mapped while IC's fake first
/// person turns the POV switch off.
const ENABLED_CONTROLS_LOAD: usize = 0x3cc;
const ENABLED_CONTROLS: [u8; 6] = [0x8B, 0x80, 0x20, 0x01, 0x00, 0x00];
const ANY_POV_SWITCH: [u8; 9] = [0x8B, 0x80, 0x20, 0x01, 0x00, 0x00, 0x80, 0xE1, 0xDF];

/// `ShaderReferenceEffect::Update`: +0xe1 calls `vfunc_40_sub4` `bool (effect)` and
/// +0x1f5 its slot 59 `UpdatePosition`, both choosing the root the effect goes on by the player's
/// third-person flag; rbx holds the effect's target reference at both (RE `camera.md` section 12).
const EFFECT_SHADER_CALLS: [usize; 2] = [0xe1, 0x1f5];

type EffectUpdate = unsafe extern "C" fn(usize, usize) -> usize;

/// `TESObjectCELL::AttachReference3D` +0x7a, `call [rax+0x468]` (the reference's slot 141): the 3D
/// placed in the cell's rooms and portals, which for the player is the view's root, so with the
/// third-person root not in view the body is left out of interior rooms (`camera.md` section 10).
const CELL_ATTACH_3D_CALL: usize = 0x7a;

type Reference3D = unsafe extern "C" fn(usize) -> usize;

/// `ComputeCameraCasterCollision` +0x4dd, `movss xmm6, [...]` at the start of the overlap test of
/// the camera with the target; `EB 58` jumps to the occluder bookkeeping at +0x537, so the player
/// is never faded for the camera's closeness (`camera.md` section 9).
const PLAYER_FADE_TEST: usize = 0x4dd;
const PLAYER_FADE_ORIGINAL: [u8; 8] = [0xF3, 0x0F, 0x10, 0x35, 0xB3, 0xF9, 0x7B, 0x01];
const PLAYER_FADE_SKIP: [u8; 8] = [0xEB, 0x58, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90];

/// `ComputeCameraCasterCollision` +0x38e, the call to `CastCameraLinearAndCollectHits` `(holder,
/// world, from*, to*, hit*, Actor** occluder, float radius)`; the radius is the seventh argument, on
/// the stack, so the raw shape carries it untouched.
const CAMERA_CAST_CALL: usize = 0x38e;

type CameraCast = unsafe extern "C" fn(usize, usize, usize, usize, usize, usize, usize) -> usize;

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
    hooks.replace(Call::<IsThirdPerson>::at(Location::id(fns::ActorMagicCaster__FindCasterNode, CASTER_VIEW_CALL)), Options::new(host.name()), |_, _| usize::from(effect_node_third_person()))?;
    hooks.before(Call::<CrosshairPick>::at(Location::id(fns::PlayerCharacter__UpdateCrosshairPick, CROSSHAIR_PICK_CALL)), Options::new(host.name()), |args| crosshair_pick(args.2))?;
    unsafe { hooks.patch_cave(host.name(), Location::id(fns::ControlMap__MapInputEvents, ENABLED_CONTROLS_LOAD), &ENABLED_CONTROLS, &ANY_POV_SWITCH) }?;
    for offset in EFFECT_SHADER_CALLS {
        hooks.around(Call::<EffectUpdate>::at(Location::id(fns::ShaderReferenceEffect__Update, offset)).passing(Register::Rbx), Options::new(host.name()), shader_reference_effect)?;
    }
    hooks.replace(Call::<Reference3D>::at(Location::id(fns::TESObjectCELL__AttachReference3D, CELL_ATTACH_3D_CALL)), Options::new(host.name()), |args, original| {
        third_person_3d(args.0).unwrap_or_else(|| unsafe { original.call(args) })
    })?;
    install_fades(host)?;
    manager::raise_pitch_limits(PitchLimits::Sitting);
    Ok(())
}

/// `TESObjectCell`: IC calls `TESObjectREFR::Get3D` there, the loaded third-person 3D; only the
/// player's differs from the slot's answer.
fn third_person_3d(reference: usize) -> Option<usize> {
    let player = util::player()?;
    (core::ptr::from_ref::<sre::PlayerCharacter>(player) as usize == reference).then(|| player.third_person_root().map_or(0, |root| core::ptr::from_ref(root) as usize))
}

/// `DelayInstall`'s fade patches: the player is never faded for the camera's closeness, and no
/// actor between the camera and the player is chosen to fade. A refused patch (another plugin
/// changed the bytes first) is logged and the rest of the plugin runs.
fn install_fades(host: &'static Host) -> Result<(), Error> {
    let hooks = host.hooks();
    if let Err(e) = unsafe { hooks.patch(host.name(), Location::id(fns::ComputeCameraCasterCollision, PLAYER_FADE_TEST), &PLAYER_FADE_ORIGINAL, &PLAYER_FADE_SKIP) } {
        crate::log(&format!("player fade patch refused: {e}"));
    }
    hooks.after(Call::<CameraCast>::at(Location::id(fns::ComputeCameraCasterCollision, CAMERA_CAST_CALL)), Options::new(host.name()), |args, _, _| {
        let occluder = args.5 as *mut usize;
        if !occluder.is_null() {
            unsafe { occluder.write(0) };
        }
    })?;
    Ok(())
}

/// `GetEffectNode_IsThirdPerson`: in first person the casters follow the arms IC shows.
fn effect_node_third_person() -> bool {
    if util::is_engine_first_person() {
        return third_person_arms(&settings::profile().General);
    }
    util::player().is_some_and(util::third_person_flag)
}

/// `CrossHairPickData_Pick`: in first person the pick starts at the presented camera.
fn crosshair_pick(origin: usize) {
    let origin = origin as *mut [f32; 3];
    if !origin.is_null() && util::is_first_person() {
        unsafe { origin.write(snapshot::get().position) };
    }
}

/// `ShaderReferenceEffect_Update`'s `Effect`: an effect on the player in first person or the free
/// camera goes on the third-person root, unless the first-person arms fight with a weapon drawn.
/// IC's `MirrorEffect` afterwards is left out (the effect's fields are not in the RE).
fn shader_reference_effect(args: (usize, usize), next: Next<'_, EffectUpdate>) -> usize {
    let free = util::is_free_camera();
    let player = util::player().filter(|p| core::ptr::from_ref::<sre::PlayerCharacter>(p) as usize == args.1 && (util::is_engine_first_person() || free));
    let Some(player) = player else { return unsafe { next.call(args) } };
    if util::is_weapon_drawn(player) && !third_person_arms(&settings::profile().General) && !free {
        return unsafe { next.call(args) };
    }
    let saved = util::third_person_flag(player);
    util::set_third_person_flag(player, true);
    let ret = unsafe { next.call(args) };
    util::set_third_person_flag(player, saved);
    ret
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
    let focus = crate::camera::snapshot::get().dialogue_focus;
    if hidden.left_arm && !focus {
        shrink(util::object_by_name(third, "NPC L UpperArm [LUar]"));
    }
    if hidden.right_arm && !focus {
        shrink(util::object_by_name(third, "NPC R UpperArm [RUar]"));
    }
    if hidden.head && !focus {
        shrink(util::player().and_then(|p| util::actor_head(p)).and_then(|h| unsafe { h.parent.cast::<NiAVObject>().as_ref() }));
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
