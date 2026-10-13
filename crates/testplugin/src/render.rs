//! One test per `skyrim::render` API, against the RE repo's `docs/subsystems/render-verify.md`.
//! `install` detours `NiAVObject::Cull` and `PlayerCharacter::SetFirstPerson` at `DataLoaded`;
//! `TESTS` reads the scene, the sun, the player's roots and the camera after a load and at every
//! save (the scenario saves outdoors in first person, then in Dragonsreach); `CASTER_TESTS` reports at
//! the saves how often each player root was visited per view and per pass.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use layout::v1_7_104::{classes, vtables};
use skyrim::actors::PlayerCharacter;
use skyrim::camera::{CameraState, PlayerCamera};
use skyrim::render::{CullFn, CullingProcess, NiAVObject, SetFirstPersonFn, ShadowSceneNode, frame_counter};

use crate::{Test, check};

pub const TESTS: &[(&str, Test)] = &[("scene", scene), ("sun", sun), ("lights", lights), ("roots", roots), ("view", view), ("frame_counter", frame_counter_test), ("hooks", hooks)];
pub const CASTER_TESTS: &[(&str, Test)] = &[("casters", casters), ("switches", switches)];

/// Saves since the load, numbering the `save<N>` phases.
pub static SAVES: AtomicU32 = AtomicU32::new(0);

static CULL: OnceLock<hook::Hook<CullFn>> = OnceLock::new();
static SWITCH: OnceLock<hook::Hook<SetFirstPersonFn>> = OnceLock::new();
/// Counting is on between `PostLoadGame` and the next `PreLoadGame`/`NewGame`: outside that
/// window the player's roots are being replaced and must not be read from the job threads.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static THIRD_ROOT: AtomicPtr<classes::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static FIRST_ROOT: AtomicPtr<classes::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
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

unsafe extern "C" fn cull_detour(node: *mut classes::NiAVObject, culler: *mut classes::partial::NiCullingProcess, plane_mask: u32) {
    let original = CULL.get().expect("stored before enabling").original();
    if ACTIVE.load(Ordering::Acquire)
        && let (Some(n), Some(c)) = (unsafe { NiAVObject::from_ptr(node) }, unsafe { CullingProcess::from_ptr(culler) })
    {
        let roots = [THIRD_ROOT.load(Ordering::Relaxed), FIRST_ROOT.load(Ordering::Relaxed)];
        for (root_index, root) in roots.iter().enumerate() {
            let Some(root) = (unsafe { NiAVObject::from_ptr(*root) }) else { continue };
            if n.is_under(root, 24) {
                let view = usize::from(FIRST_PERSON.load(Ordering::Relaxed));
                let pass = usize::from(!c.is_shadow_pass());
                let hidden = usize::from(n.is_hidden());
                let is_self = usize::from(core::ptr::eq(n.as_ptr(), root.as_ptr()));
                COUNTS[index(view, root_index, pass, hidden, is_self)].fetch_add(1, Ordering::Relaxed);
                if root_index == 0 && n.class().is_some_and(|c| c.id == vtables::BSFaceGenNiNode_vtable::ID) {
                    HEAD[view * 2 + pass].fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
        }
    }
    unsafe { original(node, culler, plane_mask) }
}

unsafe extern "C" fn switch_detour(player: *mut classes::PlayerCharacter, first_person: bool) {
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
    if let Some(p) = PlayerCharacter::get() {
        FIRST_PERSON.store(!p.is_third_person(), Ordering::Relaxed);
        THIRD_ROOT.store(p.third_person_root().map_or(core::ptr::null_mut(), |r| r.as_ptr().cast_mut().cast()), Ordering::Relaxed);
        FIRST_ROOT.store(p.first_person_root().map_or(core::ptr::null_mut(), |r| r.as_ptr().cast_mut().cast()), Ordering::Relaxed);
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
    PlayerCharacter::get().ok_or_else(|| "g_PlayerCharacter_ptr is null".to_owned())
}

fn world() -> Result<&'static ShadowSceneNode, String> {
    ShadowSceneNode::world().ok_or_else(|| "g_ShadowSceneNode[0] is null".to_owned())
}

fn scene() -> Result<String, String> {
    let w = world()?;
    check(w.scene_index() == 0, &format!("sceneIndex {} on the world node", w.scene_index()))?;
    let node_class = w.node().class().ok_or("the scene node's vtable is not in the ledger")?;
    check(node_class.id == vtables::ShadowSceneNode_vtable::ID, &format!("the world node is a {}", node_class.class))?;
    let lights = w.lights();
    let shadow = w.shadow_lights();
    check(lights.iter().all(Option::is_some) && shadow.iter().all(Option::is_some), "a null entry in the light lists")?;
    check(lights.iter().flatten().all(|l| !l.is_shadow_light()), "a shadow light in `lights`")?;
    check(shadow.iter().flatten().all(|l| l.is_shadow_light()), "a plain light in `shadowLights`")?;
    let others: Vec<String> = (1..4).map(|i| ShadowSceneNode::get(i).map_or("null".to_owned(), |n| format!("{:p} index {}", n.raw(), n.scene_index()))).collect();
    Ok(format!(
        "world at {:p}, {} lights, {} shadow lights, eye ({:.1}, {:.1}, {:.1}), slots 1-3: {}",
        w.raw(),
        lights.len(),
        shadow.len(),
        w.eye_position()[0],
        w.eye_position()[1],
        w.eye_position()[2],
        others.join(", ")
    ))
}

/// The sun's cascade records are filled by the first frame of the loaded cell; before it (at
/// `PostLoadGame`) only the counts and the bias are checked.
fn sun() -> Result<String, String> {
    let w = world()?;
    let sun = w.sun_shadow_light().ok_or("sunShadowLight is null")?;
    let class = sun.class().ok_or("the sun's vtable is not in the ledger")?;
    check(class.id == vtables::BSShadowDirectionalLight_vtable::ID, &format!("the sun is a {}", class.class))?;
    check(sun.is_shadow_light(), "IsShadowLight false on the sun")?;
    check(sun.shadow_map_count() == 2, &format!("shadowMapCount {}", sun.shadow_map_count()))?;
    let maps = sun.shadow_maps();
    check(maps.len() == 2, &format!("{} shadow map records", maps.len()))?;
    check(maps[0].slice() == 0 && maps[1].slice() == 1, &format!("slices {} and {}", maps[0].slice(), maps[1].slice()))?;
    let cams = [maps[0].raw().camera, maps[1].raw().camera];
    if frame_counter() == FRAME_AT_LOAD.load(Ordering::Relaxed) {
        return Ok(format!("sun {} at {:p} before the first frame: maps {}, biasScale {}, cascade cameras {:p} {:p}", class.class, sun.raw(), maps.len(), sun.bias_scale(), cams[0], cams[1]));
    }
    check(maps.iter().all(|m| m.viewport() == [0, 4096, 4096, 0]), &format!("viewports {:?} {:?}", maps[0].viewport(), maps[1].viewport()))?;
    check(!cams[0].is_null() && !cams[1].is_null() && cams[0] != cams[1], &format!("cascade cameras {:p} {:p}", cams[0], cams[1]))?;
    check(maps[0].raw().accumulator != maps[1].raw().accumulator, "the two cascades share an accumulator")?;
    let light_camera = sun.light_camera().ok_or("lightCamera is null")?;
    check(light_camera.class().is_some_and(|c| c.id == vtables::NiCamera_vtable::ID), "lightCamera is not a NiCamera")?;
    let d = sun.light_direction();
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    check((len - 1.0).abs() < 0.01, &format!("lightDirection {d:?} has length {len}"))?;
    let sun_light = w.sun_light().ok_or("sunLight is null")?;
    let focus = sun.focus_shadow_maps();
    Ok(format!(
        "sun {} at {:p}: maps {} (slices {}/{}, viewport {:?}, targets {:?}/{:?}), focus active {}, biasScale {}, direction ({:.3}, {:.3}, {:.3}), lightCamera {:p}, focus cameras {}, sunLight {} shadow={}",
        class.class,
        sun.raw(),
        maps.len(),
        maps[0].slice(),
        maps[1].slice(),
        maps[0].viewport(),
        maps[0].depth_target(),
        maps[1].depth_target(),
        sun.focus_shadows_active(),
        sun.bias_scale(),
        d[0],
        d[1],
        d[2],
        light_camera.raw(),
        focus.iter().filter(|m| m.camera().is_some()).count(),
        sun_light.class().map_or("?", |c| c.class),
        sun_light.is_shadow_light()
    ))
}

fn roots() -> Result<String, String> {
    let p = player()?;
    let third = p.third_person_root().ok_or("loadedData->node3D is null")?;
    let first = p.first_person_root().ok_or("firstPersonRoot is null")?;
    check(!std::ptr::eq(third, first), "the two roots are the same node")?;
    for (name, r) in [("third", third), ("first", first)] {
        check(r.class().is_some_and(|c| c.id == vtables::BSFadeNode_vtable::ID), &format!("the {name}-person root is not a BSFadeNode"))?;
    }
    let for_first = p.node_3d_for(true).ok_or("Get3DFor(true) is null")?;
    let for_third = p.node_3d_for(false).ok_or("Get3DFor(false) is null")?;
    check(std::ptr::eq(for_first, first) && std::ptr::eq(for_third, third), "Get3DFor does not return firstPersonRoot / loadedData->node3D")?;
    let get3d = p.node_3d().ok_or("Get3D is null")?;
    let expected: &skyrim::render::NiNode = if p.is_third_person() { third } else { first };
    check(std::ptr::eq(get3d.raw(), expected.raw()), "Get3D does not follow IsThirdPerson")?;
    let describe = |r: &skyrim::render::NiNode| {
        let kids: Vec<String> = r.children().iter().map(|c| c.map_or("null".to_owned(), |c| c.class().map_or("?", |v| v.class).to_owned())).collect();
        format!("{:p} flags {:#x} children [{}]", r.raw(), r.flags(), kids.join(", "))
    };
    Ok(format!("third {} | first {} | Get3D follows IsThirdPerson={}", describe(third), describe(first), p.is_third_person()))
}

fn view() -> Result<String, String> {
    let p = player()?;
    let third = p.third_person_root().ok_or("loadedData->node3D is null")?;
    let first = p.first_person_root().ok_or("firstPersonRoot is null")?;
    let camera = PlayerCamera::get().ok_or("g_PlayerCamera_ptr is null")?;
    let state = camera.state();
    let third_person = p.is_third_person();
    check(p.raw().viewFlags & 1 == u8::from(third_person), "IsThirdPerson disagrees with viewFlags bit 0")?;
    if third_person {
        check(matches!(state, Some(CameraState::ThirdPerson | CameraState::ThirdPersonAlt | CameraState::AutoVanity)), &format!("third person with camera state {state:?}"))?;
        check(!third.is_hidden() && first.is_hidden(), &format!("third person with root flags third {:#x} first {:#x}", third.flags(), first.flags()))?;
    } else {
        check(state == Some(CameraState::FirstPerson), &format!("first person with camera state {state:?}"))?;
        check(third.is_hidden(), &format!("first person with the third-person root visible ({:#x})", third.flags()))?;
    }
    Ok(format!(
        "third_person={third_person} state={state:?} currentState {:p} cameraRoot {:p}, flags third {:#x} first {:#x}",
        camera.current_state_ptr(),
        camera.camera_root().map_or(std::ptr::null(), |n| n.raw()),
        third.flags(),
        first.flags()
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
    let portal_graph = unsafe { core::ptr::read_unaligned(&raw const w.raw().portalGraph) };
    let described: Vec<String> = w.shadow_lights().iter().flatten().map(|l| format!("{} maps {} pos ({:.0}, {:.0}, {:.0})", l.class().map_or("?", |c| c.class), l.shadow_maps().len(), l.position()[0], l.position()[1], l.position()[2])).collect();
    Ok(format!("{} lights, {} shadow lights [{}], sun focus active {}, portal graph {:p}", w.lights().len(), w.shadow_lights().len(), described.join("; "), sun.focus_shadows_active(), portal_graph))
}

fn switches() -> Result<String, String> {
    let n = SWITCHES.load(Ordering::Relaxed);
    check(n >= 2, &format!("SetFirstPerson ran {n} times since DataLoaded"))?;
    Ok(format!("SetFirstPerson ran {n} times, first_person now {}", FIRST_PERSON.load(Ordering::Relaxed)))
}
