//! One test per `skyrim::camera` and the pass 6 part of `skyrim::render`, against the RE repo's
//! `docs/subsystems/frame-verify.md`. `install` detours `TESCamera::Update` and `PresentFrame` at
//! `DataLoaded`; the camera detour keeps one sample per camera state (and one for the orbit, a
//! third-person sample with free rotation) of what the state wrote; `TESTS` reads the scene graph,
//! the shaders, the accumulators, the window and the sky; `SAMPLE_TESTS` checks the samples at the
//! save that ends the scenario (third person, orbit, first person, free camera).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU32, Ordering};

use sre::raw::vtables;
use skyrim::actors::PlayerCharacter;
use skyrim::camera::{CameraState, CameraUpdateFn, PlayerCamera, frame_delta, frame_delta_real, main_camera};
use skyrim::render::{AccumulatorRenderMode, BSShaderAccumulator, PresentFn, RenderWindow, ShaderFlags, ShaderMaterialType, ShadowSceneNode, Sky, frame_counter, world_scene_graph};

use crate::{Test, check};

pub const TESTS: &[(&str, Test)] = &[("names", names), ("shaders", shaders), ("accumulators", accumulators), ("window", window), ("sky", sky), ("frame_camera", frame_camera), ("presents", presents)];
pub const SAMPLE_TESTS: &[(&str, Test)] = &[("third_person", third_person), ("orbit", orbit), ("first_person", first_person), ("free_camera", free_camera), ("frame_time", frame_time)];

static UPDATE: OnceLock<hook::Hook<CameraUpdateFn>> = OnceLock::new();
static PRESENT: OnceLock<hook::Hook<PresentFn>> = OnceLock::new();
static PRESENTS: AtomicU32 = AtomicU32::new(0);
static COUNTER_AT_INSTALL: AtomicU32 = AtomicU32::new(0);
static UPDATES: AtomicU32 = AtomicU32::new(0);

/// Third-person fields: target and current shoulder offsets, yaw, zoom, free rotation and its enable.
type ThirdPersonFields = ([f32; 3], [f32; 3], f32, f32, [f32; 2], bool);

/// What the camera detour reads right after a state's update ran.
#[derive(Clone, Debug)]
struct Sample {
    state: CameraState,
    stack_count: u32,
    cached_position: [f32; 3],
    root_world: [f32; 3],
    root_local: [f32; 3],
    frame_camera_world: [f32; 3],
    frame_camera_under_root: bool,
    world_fov: f32,
    frustum: [f32; 6],
    viewport: [f32; 4],
    yaw: f32,
    third: Option<ThirdPersonFields>,
    free: Option<([f32; 3], f32, f32)>,
    delta: f32,
    delta_real: f32,
}

static SAMPLES: Mutex<BTreeMap<&'static str, Sample>> = Mutex::new(BTreeMap::new());

fn sample(camera: &PlayerCamera) -> Option<(&'static str, Sample)> {
    let state = camera.current_state()?;
    let id = state.id()?;
    let root = camera.camera_root()?;
    let frame_camera = main_camera()?;
    let third = state.as_third_person().map(|t| (t.target_offset(), t.current_offset(), t.current_yaw(), t.current_zoom(), t.free_rotation(), t.free_rotation_enabled()));
    let free = state.as_free().map(|f| (f.position(), f.pitch(), f.yaw()));
    let key = match (id, third) {
        (CameraState::ThirdPerson, Some((_, _, _, _, [yaw, _], true))) if yaw.abs() > 0.5 => "orbit",
        (CameraState::ThirdPerson, _) => "third",
        (CameraState::FirstPerson, _) => "first",
        (CameraState::Free, _) => "free",
        _ => "other",
    };
    Some((
        key,
        Sample {
            state: id,
            stack_count: camera.state_stack_count(),
            cached_position: camera.cached_position(),
            root_world: root.world().translation,
            root_local: root.local().translation,
            frame_camera_world: frame_camera.world().translation,
            frame_camera_under_root: frame_camera.parent().is_some_and(|p| core::ptr::eq(p.raw(), root.raw())),
            world_fov: camera.world_fov(),
            frustum: frame_camera.frustum(),
            viewport: frame_camera.viewport(),
            yaw: camera.yaw(),
            third,
            free,
            delta: frame_delta(),
            delta_real: frame_delta_real(),
        },
    ))
}

unsafe extern "C" fn update_detour(camera: *mut sre::TESCamera) {
    unsafe { UPDATE.get().expect("stored before enabling").original()(camera) }
    if PlayerCamera::is(camera)
        && let Some(player_camera) = PlayerCamera::get()
        && let Some((key, s)) = sample(player_camera)
    {
        UPDATES.fetch_add(1, Ordering::Relaxed);
        SAMPLES.lock().unwrap().insert(key, s);
    }
}

unsafe extern "C" fn present_detour() {
    unsafe { PRESENT.get().expect("stored before enabling").original()() }
    PRESENTS.fetch_add(1, Ordering::Relaxed);
}

/// Creates both detours, stores them and enables them; called once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    let update = unsafe { skyrim::camera::hook_update(update_detour) }.map_err(|e| format!("TESCamera::Update: {e}"))?;
    let present = unsafe { skyrim::render::hook_present(present_detour) }.map_err(|e| format!("PresentFrame: {e}"))?;
    let (update_target, present_target) = (update.target(), present.target());
    let _ = UPDATE.set(update);
    let _ = PRESENT.set(present);
    UPDATE.get().expect("just set").enable().map_err(|e| format!("enable TESCamera::Update: {e}"))?;
    PRESENT.get().expect("just set").enable().map_err(|e| format!("enable PresentFrame: {e}"))?;
    COUNTER_AT_INSTALL.store(frame_counter(), Ordering::Relaxed);
    Ok(format!("TESCamera::Update at {update_target:#x}, PresentFrame at {present_target:#x}, g_RenderFrameCounter {}", COUNTER_AT_INSTALL.load(Ordering::Relaxed)))
}

/// Clears the samples; called after a load.
pub fn reset() {
    SAMPLES.lock().unwrap().clear();
}

fn player() -> Result<&'static PlayerCharacter, String> {
    PlayerCharacter::get().ok_or_else(|| "g_PlayerCharacter_ptr is null".to_owned())
}

fn names() -> Result<String, String> {
    let p = player()?;
    let third = p.third_person_root().ok_or("loadedData->node3D is null")?;
    let name = third.name().to_str().unwrap_or("?").to_owned();
    check(name == "Skeleton.nif", &format!("the third-person root is named {name:?}"))?;
    let counts = third.children_counts();
    check(counts[0] == 8 && counts[1] == 8 && counts[2] == 8 && counts[3] == 1, &format!("children counts {counts:?}"))?;
    let mut kids = Vec::new();
    for (i, child) in third.children().iter().enumerate() {
        let child = child.ok_or_else(|| format!("child {i} is null"))?;
        check(child.parent_index() as usize == i, &format!("child {i} has parentIndex {}", child.parent_index()))?;
        check(child.parent().is_some_and(|q| core::ptr::eq(q.raw(), third.raw())), &format!("child {i} has another parent"))?;
        kids.push(format!("{}={}", child.class().map_or("?", |c| c.class), child.name().to_str().unwrap_or("?")));
    }
    let bumper = skyrim::string::BSFixedString::new("CharacterBumper");
    let found = third.object_by_name(&bumper).ok_or("GetObjectByName(CharacterBumper) returned null")?;
    check(found.name().to_str() == Some("CharacterBumper"), "GetObjectByName returned another node")?;
    Ok(format!("{name} counts {counts:?} children [{}]; CharacterBumper found at {:p}", kids.join(", "), found.as_ptr()))
}

fn shaders() -> Result<String, String> {
    let p = player()?;
    let third = p.third_person_root().ok_or("loadedData->node3D is null")?;
    let mut seen = Vec::new();
    for child in third.children().iter().flatten() {
        let Some(geometry) = child.as_geometry() else { continue };
        let property = geometry.shader_property().ok_or("a skin without a shader property")?;
        let class = property.class().ok_or("the property's vtable is not in the ledger")?;
        check(class.id == vtables::BSLightingShaderProperty_vtable::ID, &format!("a skin's property is a {}", class.class))?;
        let flags = property.flags();
        let required = [ShaderFlags::SKINNED, ShaderFlags::RECEIVE_SHADOWS, ShaderFlags::CAST_SHADOWS, ShaderFlags::Z_BUFFER_TEST, ShaderFlags::Z_BUFFER_WRITE];
        let forbidden = [ShaderFlags::REFRACTION, ShaderFlags::REFRACTION2, ShaderFlags::DECAL, ShaderFlags::DYNAMIC_DECAL];
        check(required.iter().all(|f| flags.contains(*f)), &format!("flags {:#x} lack a required bit", flags.bits()))?;
        check(!forbidden.iter().any(|f| flags.contains(*f)), &format!("flags {:#x} carry a refraction or decal bit", flags.bits()))?;
        let material = property.material().ok_or("a skin without a material")?;
        check(material.material_type() == Some(ShaderMaterialType::Lighting), &format!("material type {:?}", material.material_type()))?;
        let lighting = material.as_lighting().ok_or("not a lighting material")?;
        let set = lighting.texture_set().ok_or("no texture set")?;
        let diffuse = set.path(0).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
        check(diffuse.to_ascii_lowercase().ends_with(".dds"), &format!("diffuse path {diffuse:?}"))?;
        seen.push(format!("{}: flags {:#x} state {:#x} alpha {} feature {:?} diffuse {diffuse}", child.name().to_str().unwrap_or("?"), flags.bits(), property.last_render_pass_state(), property.alpha(), material.feature()));
    }
    check(seen.len() >= 2, &format!("{} skins under the root", seen.len()))?;
    Ok(seen.join(" | "))
}

fn accumulators() -> Result<String, String> {
    let [depth, world, first] = BSShaderAccumulator::world();
    let depth = depth.ok_or("g_WorldDepthAccumulator is null")?;
    let world = world.ok_or("g_WorldAccumulator is null")?;
    let first = first.ok_or("g_FirstPersonAccumulator is null")?;
    check(depth.render_mode() == Some(AccumulatorRenderMode::DepthPrepass), &format!("depth accumulator mode {:#x}", depth.render_mode_raw()))?;
    check(world.render_mode() == Some(AccumulatorRenderMode::Standard), &format!("world accumulator mode {:#x}", world.render_mode_raw()))?;
    check(first.render_mode() == Some(AccumulatorRenderMode::FirstPerson), &format!("first-person accumulator mode {:#x}", first.render_mode_raw()))?;
    Ok(format!("modes depth {:#x} world {:#x} first person {:#x}; world camera {:p}", depth.render_mode_raw(), world.render_mode_raw(), first.render_mode_raw(), world.camera().map_or(core::ptr::null(), |c| c.raw())))
}

fn window() -> Result<String, String> {
    let w = RenderWindow::current().ok_or("g_CurrentRenderWindow is null")?;
    let [width, height] = w.size();
    check(width > 0 && height > 0 && w.hwnd() != 0, &format!("window {width}x{height} hwnd {:#x}", w.hwnd()))?;
    check(!w.swap_chain().is_null() && !w.back_buffer_rtv().is_null(), "the swap chain or the back buffer view is null")?;
    Ok(format!("window {width}x{height} hwnd {:#x}, swap chain {:p}, back buffer RTV {:p}", w.hwnd(), w.swap_chain(), w.back_buffer_rtv()))
}

fn sky() -> Result<String, String> {
    let sky = Sky::get().ok_or("g_Sky_ptr is null")?;
    let hour = sky.current_game_hour();
    let sun = sky.sun_light().ok_or("the sky has no sun light")?;
    let scene = ShadowSceneNode::world().ok_or("g_ShadowSceneNode[0] is null")?;
    let scene_sun = scene.sun_light().and_then(|l| l.light()).ok_or("the scene node has no sun light")?;
    check(core::ptr::eq(sun.as_ptr(), scene_sun.as_ptr()), "the sky's sun light is not the scene node's")?;
    Ok(format!("game hour {hour:.2}, sun light {:p} = sunLight->light", sun.as_ptr()))
}

fn frame_camera() -> Result<String, String> {
    let camera = main_camera().ok_or("GetMainCamera returned null")?;
    let graph = world_scene_graph().ok_or("g_SceneGraph is null")?;
    check(camera.class().is_some_and(|c| c.id == vtables::NiCamera_vtable::ID), "the main camera is not a NiCamera")?;
    let root = PlayerCamera::get().and_then(|c| c.camera_root()).ok_or("the player camera has no root")?;
    check(camera.parent().is_some_and(|p| core::ptr::eq(p.raw(), root.raw())), "the frame camera is not a child of the camera root")?;
    let f = camera.frustum();
    let fov = PlayerCamera::get().map_or(0.0, PlayerCamera::world_fov);
    let expected_right = (fov.to_radians() / 2.0).tan();
    check((f[1] - expected_right).abs() < 0.01 && (f[0] + expected_right).abs() < 0.01, &format!("frustum {f:?} against fov {fov}"))?;
    check((f[4] - 15.0).abs() < 0.01, &format!("near {}", f[4]))?;
    check(full_viewport(camera.viewport()), &format!("viewport {:?}", camera.viewport()))?;
    Ok(format!("NiCamera {:p} under the camera root, scene graph {:p}, frustum {f:?}, fov {fov}", camera.raw(), graph.raw()))
}

fn presents() -> Result<String, String> {
    let presents = PRESENTS.load(Ordering::Relaxed);
    let counter = frame_counter().wrapping_sub(COUNTER_AT_INSTALL.load(Ordering::Relaxed));
    check(presents.abs_diff(counter) <= 16, &format!("{presents} PresentFrame calls but the counter grew by {counter}"))?;
    Ok(format!("{presents} presents, counter +{counter}, {} player camera updates", UPDATES.load(Ordering::Relaxed)))
}

fn get(key: &str) -> Result<Sample, String> {
    SAMPLES.lock().unwrap().get(key).cloned().ok_or_else(|| format!("no {key} sample"))
}

fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < tol)
}

fn common(s: &Sample) -> Result<(), String> {
    check(near(s.cached_position, s.root_world, 0.01) && near(s.cached_position, s.frame_camera_world, 0.01), &format!("cached {:?} root {:?} frame camera {:?}", s.cached_position, s.root_world, s.frame_camera_world))?;
    check(s.frame_camera_under_root, "the frame camera is not under the camera root")?;
    check((s.world_fov - 80.0).abs() < 0.01, &format!("world fov {}", s.world_fov))?;
    check(full_viewport(s.viewport), &format!("viewport {:?}", s.viewport))
}

fn full_viewport(v: [f32; 4]) -> bool {
    [0.0, 1.0, 1.0, 0.0].iter().zip(v).all(|(e, a)| (e - a).abs() < 1e-6)
}

fn third_person() -> Result<String, String> {
    let s = get("third")?;
    common(&s)?;
    check(s.state == CameraState::ThirdPerson && s.stack_count == 0, &format!("state {:?} stack {}", s.state, s.stack_count))?;
    let (target, current, yaw, zoom, free, _) = s.third.ok_or("no third-person fields")?;
    check(near(target, [30.0, 0.0, -10.0], 0.01) && near(current, [30.0, 0.0, -10.0], 0.01), &format!("offsets {target:?} {current:?}"))?;
    check((yaw - s.yaw).abs() < 0.01, &format!("state yaw {yaw} camera yaw {}", s.yaw))?;
    Ok(format!("state 9 stack 0, offsets {target:?}, yaw {yaw:.3}, zoom {zoom:.3}, free rotation {free:?}, position {:?}", s.cached_position))
}

fn orbit() -> Result<String, String> {
    let s = get("orbit")?;
    common(&s)?;
    let (_, _, yaw, _, free, enabled) = s.third.ok_or("no third-person fields")?;
    check(enabled && (free[0].abs() - core::f32::consts::PI).abs() < 0.2, &format!("free rotation {free:?} enabled {enabled}"))?;
    check((s.yaw - yaw).abs() < 0.01, &format!("camera yaw {} state yaw {yaw}", s.yaw))?;
    Ok(format!("orbit: free rotation {free:?} (pi around the player), state yaw {yaw:.3} = camera yaw {:.3}", s.yaw))
}

fn first_person() -> Result<String, String> {
    let s = get("first")?;
    common(&s)?;
    check(s.state == CameraState::FirstPerson && s.stack_count == 0, &format!("state {:?} stack {}", s.state, s.stack_count))?;
    Ok(format!("state 0 stack 0, position {:?}, frustum {:?}", s.cached_position, s.frustum))
}

fn free_camera() -> Result<String, String> {
    let s = get("free")?;
    check(s.state == CameraState::Free && s.stack_count == 1, &format!("state {:?} stack {}", s.state, s.stack_count))?;
    let (position, pitch, yaw) = s.free.ok_or("no free camera fields")?;
    check(near(position, s.root_local, 0.01), &format!("state position {position:?} root local {:?}", s.root_local))?;
    Ok(format!("state 3 stack 1, position {position:?} = root local, pitch {pitch:.3}, yaw {yaw:.3}"))
}

fn frame_time() -> Result<String, String> {
    let s = get("third")?;
    check(s.delta > 0.0 && s.delta < 0.5 && s.delta_real > 0.0, &format!("delta {} real {}", s.delta, s.delta_real))?;
    Ok(format!("frame delta {:.4} s (real {:.4} s) at the third-person sample", s.delta, s.delta_real))
}
