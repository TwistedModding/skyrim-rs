//! The render suite through `skyrim::render` and the generated scene graph, against the RE repo's `docs/subsystems/render-verify.md`.
//! `install` detours `NiAVObject::Cull` and `PlayerCharacter::SetFirstPerson` at `DataLoaded`;
//! `TESTS` reads the scene, the sun, the player's roots and the camera after a load and at every
//! save (the scenario saves outdoors in first person, then in Dragonsreach); `CASTER_TESTS` reports at
//! the saves how often each player root was visited per view and per pass.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use skyrim::prelude::*;
use skyrim::render::{CullFn, HIDDEN, SetFirstPersonFn, floats, frame_counter, shadow_scene_node};
use sre::raw::vtables;
use sre::{CameraState, NiAVObject, NiNode, PlayerCamera, PlayerCharacter, ShadowMapData, ShadowSceneNode};

use crate::{Test, check};

fn is_hidden(node: &NiAVObject) -> bool {
    node.flags & HIDDEN != 0
}

fn class_name<T>(object: &T) -> &'static str {
    skyrim::class_of(object).map_or("?", |c| c.class)
}

/// `[left, right, top, bottom]` in pixels.
fn viewport(map: &ShadowMapData) -> [u32; 4] {
    core::array::from_fn(|i| u32::from_le_bytes(map.viewport[i * 4..i * 4 + 4].try_into().unwrap()))
}

/// The third-person root, `loadedData->node3D`.
fn third_person_root(player: &PlayerCharacter) -> Option<&NiNode> {
    player.loaded_data()?.node3_d()
}

pub const TESTS: &[(&str, Test)] = &[("scene", scene), ("sun", sun), ("lights", lights), ("roots", roots), ("view", view), ("frame_counter", frame_counter_test), ("hooks", hooks)];
pub const CASTER_TESTS: &[(&str, Test)] = &[("casters", casters), ("switches", switches)];

/// Saves since the load, numbering the `save<N>` phases.
pub static SAVES: AtomicU32 = AtomicU32::new(0);

static CULL: OnceLock<hook::Hook<CullFn>> = OnceLock::new();
static SWITCH: OnceLock<hook::Hook<SetFirstPersonFn>> = OnceLock::new();
/// Counting is on between `PostLoadGame` and the next `PreLoadGame`/`NewGame`: outside that
/// window the player's roots are being replaced and must not be read from the job threads.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static THIRD_ROOT: AtomicPtr<sre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static FIRST_ROOT: AtomicPtr<sre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static FIRST_PERSON: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicU32 = AtomicU32::new(0);
static FRAME_AT_LOAD: AtomicU32 = AtomicU32::new(0);

/// Calls of `NiAVObject::Cull` on the player's roots and their descendants, indexed by
/// [view][root][pass][hidden][self]: view 0 third person, 1 first person; root 0 third-person,
/// 1 first-person; pass 0 a shadow camera, 1 any other; hidden = bit 0 set at entry; self = the
/// root itself rather than a descendant.
static COUNTS: [AtomicU32; 32] = [const { AtomicU32::new(0) }; 32];
/// Calls on `BSFaceGenNiNode` under the third-person root, [view][pass].
static HEAD: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

fn index(view: usize, root: usize, pass: usize, hidden: usize, is_self: usize) -> usize {
    (((view * 2 + root) * 2 + pass) * 2 + hidden) * 2 + is_self
}

fn count(view: usize, root: usize, pass: usize, hidden: usize, is_self: usize) -> u32 {
    COUNTS[index(view, root, pass, hidden, is_self)].load(Ordering::Relaxed)
}

unsafe extern "C" fn cull_detour(node: *mut sre::NiAVObject, culler: *mut sre::NiCullingProcess, plane_mask: u32) {
    let original = CULL.get().expect("stored before enabling").original();
    if ACTIVE.load(Ordering::Acquire)
        && let (Some(n), Some(c)) = (unsafe { node.as_ref() }, unsafe { culler.cast::<sre::BSCullingProcess>().as_ref() })
    {
        let roots = [THIRD_ROOT.load(Ordering::Relaxed), FIRST_ROOT.load(Ordering::Relaxed)];
        for (root_index, root) in roots.iter().enumerate() {
            let Some(root) = (unsafe { root.as_ref() }) else { continue };
            if n.is_under(root, 24) {
                let view = usize::from(FIRST_PERSON.load(Ordering::Relaxed));
                let pass = usize::from(!c.is_shadow_pass());
                let hidden = usize::from(is_hidden(n));
                let is_self = usize::from(core::ptr::eq(n, root));
                COUNTS[index(view, root_index, pass, hidden, is_self)].fetch_add(1, Ordering::Relaxed);
                if root_index == 0 && skyrim::class_of(n).is_some_and(|c| c.id == sre::id!(vtables::BSFaceGenNiNode_vtable::ID)) {
                    HEAD[view * 2 + pass].fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
        }
    }
    unsafe { original(node, culler, plane_mask) }
}

unsafe extern "C" fn switch_detour(player: *mut sre::PlayerCharacter, first_person: bool) {
    unsafe { SWITCH.get().expect("stored before enabling").original()(player, first_person) }
    FIRST_PERSON.store(first_person, Ordering::Relaxed);
    SWITCHES.fetch_add(1, Ordering::Relaxed);
}

/// Creates both detours, stores them and enables them; called once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    let cull = unsafe { skyrim::render::hook_cull(cull_detour) }.map_err(|e| format!("NiAVObject::Cull: {e}"))?;
    let switch = unsafe { skyrim::render::hook_set_first_person(switch_detour) }.map_err(|e| format!("SetFirstPerson: {e}"))?;
    let (cull_target, switch_target) = (cull.target(), switch.target());
    let _ = CULL.set(cull);
    let _ = SWITCH.set(switch);
    CULL.get().expect("just set").enable().map_err(|e| format!("enable NiAVObject::Cull: {e}"))?;
    SWITCH.get().expect("just set").enable().map_err(|e| format!("enable SetFirstPerson: {e}"))?;
    Ok(format!("NiAVObject::Cull at {cull_target:#x}, PlayerCharacter::SetFirstPerson at {switch_target:#x}"))
}

/// Stops counting; called at `PreLoadGame` and `NewGame`, before the roots are replaced.
pub fn suspend() {
    ACTIVE.store(false, Ordering::Release);
}

/// Caches the roots and the view from the player, clears the counters and starts counting when
/// `count` is set; called after a load.
pub fn reset(count: bool) {
    if let Some(p) = sre::PlayerCharacter::singleton() {
        FIRST_PERSON.store(!p.is_third_person(), Ordering::Relaxed);
        THIRD_ROOT.store(third_person_root(p).map_or(core::ptr::null_mut(), |r| core::ptr::from_ref::<NiAVObject>(r).cast_mut()), Ordering::Relaxed);
        FIRST_ROOT.store(p.first_person_root().map_or(core::ptr::null_mut(), |r| core::ptr::from_ref::<NiAVObject>(r).cast_mut()), Ordering::Relaxed);
    }
    for c in &COUNTS {
        c.store(0, Ordering::Relaxed);
    }
    for c in &HEAD {
        c.store(0, Ordering::Relaxed);
    }
    FRAME_AT_LOAD.store(frame_counter(), Ordering::Relaxed);
    ACTIVE.store(count, Ordering::Release);
}

fn player() -> Result<&'static PlayerCharacter, String> {
    sre::PlayerCharacter::singleton().ok_or_else(|| "g_PlayerCharacter_ptr is null".to_owned())
}

fn world() -> Result<&'static ShadowSceneNode, String> {
    shadow_scene_node(0).ok_or_else(|| "g_ShadowSceneNode[0] is null".to_owned())
}

fn scene() -> Result<String, String> {
    let w = world()?;
    let scene_index = w.sceneIndex;
    check(scene_index == 0, &format!("sceneIndex {scene_index} on the world node"))?;
    let node_class = skyrim::class_of(w).ok_or("the scene node's vtable is not in the ledger")?;
    check(node_class.id == sre::id!(vtables::ShadowSceneNode_vtable::ID), &format!("the world node is a {}", node_class.class))?;
    let lights = skyrim::render::ShadowSceneNodeExt::lights(w);
    let shadow = w.shadow_lights();
    check(lights.iter().all(Option::is_some) && shadow.iter().all(Option::is_some), "a null entry in the light lists")?;
    check(lights.iter().flatten().all(|l| !l.is_shadow_light()), "a shadow light in `lights`")?;
    check(shadow.iter().flatten().all(|l| l.is_shadow_light()), "a plain light in `shadowLights`")?;
    let others: Vec<String> = (1..4).map(|i| shadow_scene_node(i).map_or("null".to_owned(), |n| format!("{n:p} index {}", { n.sceneIndex }))).collect();
    let eye = unsafe { (&raw const w.eyePosition).read_unaligned() };
    Ok(format!("world at {w:p}, {} lights, {} shadow lights, eye ({:.1}, {:.1}, {:.1}), slots 1-3: {}", lights.len(), shadow.len(), eye.x, eye.y, eye.z, others.join(", ")))
}

/// The sun's cascade records are filled by the first frame of the loaded cell; before it (at
/// `PostLoadGame`) only the counts and the bias are checked.
fn sun() -> Result<String, String> {
    let w = world()?;
    let sun = w.sun_shadow_light().ok_or("sunShadowLight is null")?;
    let class = skyrim::class_of(sun).ok_or("the sun's vtable is not in the ledger")?;
    check(class.id == sre::id!(vtables::BSShadowDirectionalLight_vtable::ID), &format!("the sun is a {}", class.class))?;
    check(sun.is_shadow_light(), "IsShadowLight false on the sun")?;
    let map_count = sun.shadowMapCount;
    check(map_count == 2, &format!("shadowMapCount {map_count}"))?;
    let maps = sun.shadow_maps();
    check(maps.len() == 2, &format!("{} shadow map records", maps.len()))?;
    let cams = [maps[0].camera, maps[1].camera];
    let bias = sun.biasScale;
    if frame_counter() == FRAME_AT_LOAD.load(Ordering::Relaxed) {
        return Ok(format!("sun {} at {sun:p} before the first frame: maps {}, biasScale {bias}, cascade cameras {:p} {:p}", class.class, maps.len(), cams[0], cams[1]));
    }
    check(maps.iter().all(|m| viewport(m) == [0, 4096, 4096, 0]), &format!("viewports {:?} {:?}", viewport(&maps[0]), viewport(&maps[1])))?;
    check(!cams[0].is_null() && !cams[1].is_null() && cams[0] != cams[1], &format!("cascade cameras {:p} {:p}", cams[0], cams[1]))?;
    check(maps[0].accumulator != maps[1].accumulator, "the two cascades share an accumulator")?;
    let light_camera = sun.light_camera().ok_or("lightCamera is null")?;
    check(skyrim::class_of(light_camera).is_some_and(|c| c.id == sre::id!(vtables::NiCamera_vtable::ID)), "lightCamera is not a NiCamera")?;
    let d = floats::<3>(&sun.lightDirection);
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    check((len - 1.0).abs() < 0.01, &format!("lightDirection {d:?} has length {len}"))?;
    let sun_light = w.sun_light().ok_or("sunLight is null")?;
    let focus = sun.focus_shadow_maps();
    let target = |m: &ShadowMapData| (m.depthTarget != u32::MAX).then_some(m.depthTarget);
    Ok(format!(
        "sun {} at {sun:p}: maps {} (viewport {:?}, targets {:?}/{:?}), focus active {}, biasScale {bias}, direction ({:.3}, {:.3}, {:.3}), lightCamera {light_camera:p}, focus cameras {}, sunLight {} shadow={}",
        class.class,
        maps.len(),
        viewport(&maps[0]),
        target(&maps[0]),
        target(&maps[1]),
        { sun.focusShadowsActive } != 0,
        d[0],
        d[1],
        d[2],
        focus.iter().filter(|m| m.camera().is_some()).count(),
        class_name(sun_light),
        sun_light.is_shadow_light()
    ))
}

fn roots() -> Result<String, String> {
    let p = player()?;
    let third = third_person_root(p).ok_or("loadedData->node3D is null")?;
    let first: &NiNode = p.first_person_root().ok_or("firstPersonRoot is null")?;
    check(!std::ptr::eq(third, first), "the two roots are the same node")?;
    for (name, r) in [("third", third), ("first", first)] {
        check(skyrim::class_of(r).is_some_and(|c| c.id == sre::id!(vtables::BSFadeNode_vtable::ID)), &format!("the {name}-person root is not a BSFadeNode"))?;
    }
    let for_first = p.get3_d_for(true).ok_or("Get3DFor(true) is null")?;
    let for_third = p.get3_d_for(false).ok_or("Get3DFor(false) is null")?;
    check(std::ptr::eq(for_first, first) && std::ptr::eq(for_third, third), "Get3DFor does not return firstPersonRoot / loadedData->node3D")?;
    let get3d = p.get3_d().ok_or("Get3D is null")?;
    let expected = if p.is_third_person() { third } else { first };
    check(std::ptr::eq(get3d, expected), "Get3D does not follow IsThirdPerson")?;
    let describe = |r: &NiNode| {
        let kids: Vec<String> = r.children().iter().map(|c| c.map_or("null".to_owned(), |c| class_name(c).to_owned())).collect();
        format!("{r:p} flags {:#x} children [{}]", r.flags, kids.join(", "))
    };
    Ok(format!("third {} | first {} | Get3D follows IsThirdPerson={}", describe(third), describe(first), p.is_third_person()))
}

fn view() -> Result<String, String> {
    let p = player()?;
    let third = third_person_root(p).ok_or("loadedData->node3D is null")?;
    let first: &NiNode = p.first_person_root().ok_or("firstPersonRoot is null")?;
    let camera = PlayerCamera::singleton().ok_or("g_PlayerCamera_ptr is null")?;
    let state = camera.state();
    let third_person = p.is_third_person();
    check(p.viewFlags() & 1 == u8::from(third_person), "IsThirdPerson disagrees with viewFlags bit 0")?;
    if third_person {
        check(matches!(state, Some(CameraState::ThirdPerson | CameraState::ThirdPersonAlt | CameraState::AutoVanity)), &format!("third person with camera state {state:?}"))?;
        check(!is_hidden(third) && is_hidden(first), &format!("third person with root flags third {:#x} first {:#x}", third.flags, first.flags))?;
    } else {
        check(state == Some(CameraState::FirstPerson), &format!("first person with camera state {state:?}"))?;
        check(is_hidden(third), &format!("first person with the third-person root visible ({:#x})", third.flags))?;
    }
    Ok(format!(
        "third_person={third_person} state={state:?} currentState {:p} cameraRoot {:p}, flags third {:#x} first {:#x}",
        camera.currentState,
        camera.camera_root().map_or(std::ptr::null(), core::ptr::from_ref),
        third.flags,
        first.flags
    ))
}

fn frame_counter_test() -> Result<String, String> {
    let now = frame_counter();
    let at_load = FRAME_AT_LOAD.load(Ordering::Relaxed);
    check(now >= at_load, &format!("g_RenderFrameCounter went from {at_load} to {now}"))?;
    Ok(format!("g_RenderFrameCounter={now} (at load {at_load})"))
}

fn hooks() -> Result<String, String> {
    check(CULL.get().is_some() && SWITCH.get().is_some(), "the detours are not installed")?;
    let total: u32 = COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum();
    Ok(format!("{total} Cull calls on the player's roots since the load, {} switches", SWITCHES.load(Ordering::Relaxed)))
}

fn table() -> String {
    let mut rows = Vec::new();
    for view in 0..2 {
        for root in 0..2 {
            for pass in 0..2 {
                let line = format!(
                    "{}/{}/{}: self visible {} hidden {}, descendants visible {} hidden {}",
                    ["third-person", "first-person"][view],
                    ["3rd root", "1st root"][root],
                    ["shadow", "other"][pass],
                    count(view, root, pass, 0, 1),
                    count(view, root, pass, 1, 1),
                    count(view, root, pass, 0, 0),
                    count(view, root, pass, 1, 0)
                );
                rows.push(line);
            }
        }
    }
    rows.push(format!(
        "head (BSFaceGenNiNode): third-person shadow {} other {}, first-person shadow {} other {}",
        HEAD[0].load(Ordering::Relaxed),
        HEAD[1].load(Ordering::Relaxed),
        HEAD[2].load(Ordering::Relaxed),
        HEAD[3].load(Ordering::Relaxed)
    ));
    rows.join(" | ")
}

fn casters() -> Result<String, String> {
    let t = table();
    check(count(0, 0, 0, 0, 1) > 0, &format!("third person: the third-person root was never visited by a shadow camera ({t})"))?;
    check(count(0, 0, 1, 0, 1) > 0, &format!("third person: the third-person root was never visited by the frame camera ({t})"))?;
    check(count(0, 1, 0, 0, 1) == 0 && count(0, 1, 0, 0, 0) == 0, &format!("third person: the first-person root was visited by a shadow camera ({t})"))?;
    check(count(1, 0, 0, 0, 1) == 0 && count(1, 0, 0, 0, 0) == 0, &format!("first person: the third-person root was visited by a shadow camera ({t})"))?;
    check(count(1, 0, 1, 0, 1) == 0 && count(1, 0, 1, 0, 0) == 0, &format!("first person: the third-person root was visited by the frame camera ({t})"))?;
    check(count(1, 0, 0, 1, 1) > 0, &format!("first person: no shadow camera reached the hidden third-person root ({t})"))?;
    check(count(1, 1, 0, 0, 1) == 0 && count(1, 1, 0, 0, 0) == 0, &format!("first person: the first-person root was visited by a shadow camera ({t})"))?;
    check(count(1, 1, 1, 0, 0) > 0, &format!("first person: the first-person root's children were never visited by the first-person camera ({t})"))?;
    Ok(t)
}

fn lights() -> Result<String, String> {
    let w = world()?;
    let sun = w.sun_shadow_light().ok_or("sunShadowLight is null")?;
    let portal_graph = w.portalGraph;
    let described: Vec<String> = w
        .shadow_lights()
        .iter()
        .flatten()
        .map(|l| {
            let pos = floats::<3>(&l.position);
            format!("{} maps {} pos ({:.0}, {:.0}, {:.0})", class_name(*l), l.shadow_maps().len(), pos[0], pos[1], pos[2])
        })
        .collect();
    Ok(format!("{} lights, {} shadow lights [{}], sun focus active {}, portal graph {portal_graph:p}", skyrim::render::ShadowSceneNodeExt::lights(w).len(), w.shadow_lights().len(), described.join("; "), { sun.focusShadowsActive } != 0))
}

fn switches() -> Result<String, String> {
    let n = SWITCHES.load(Ordering::Relaxed);
    check(n >= 2, &format!("SetFirstPerson ran {n} times since DataLoaded"))?;
    Ok(format!("SetFirstPerson ran {n} times, first_person now {}", FIRST_PERSON.load(Ordering::Relaxed)))
}
