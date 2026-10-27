//! Camera demo: one console command, `CameraDemo <mode> [value]`, exercises the pass 12 wrappers
//! of `skyrim::camera` and `skyrim::render` and reads every result back from the engine. `orbit`
//! and `offset` place the camera from the state-step hook (the two `call`s to `TESCamera::Update`)
//! and read the camera root, the frame camera and the cached position; `fov` sets the world FOV
//! to 70 through `PlayerCamera` and the hands FOV to 60 at the two projection sites, read back
//! from the two frustums; `near <n>` makes the consistent near-plane write and reads the setting,
//! the frame camera, the depth shift's cached near and the GPU snapshot; `shadow` admits the
//! hidden third-person root into the shadow culls in first person and counts the cascade
//! registrations of the body's skins. `report <label>` writes the `PASS`/`FAIL` lines and a
//! `summary camera <label>` line `scripts/verify.ps1 -Suite camera` reads; `off` ends every mode.

use core::ffi::CStr;
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use skyrim::actors::PlayerCharacter;
use skyrim::camera::{HandsProjectionHook, PlayerCamera, StateStepHook, frame_delta_real, hands_camera, hands_fov, main_camera};
use skyrim::input::{Arg, Command, CommandContext, Param, ScriptParamType};
use skyrim::render::{BSShaderAccumulator, CullFn, CullingProcess, FrameState, NiAVObject, ProcessCullObjectListFn, RegisterDepthGeometryFn, SceneGraph, ShadowSceneNode, Transform};

skse::plugin! {
    name: "skyrim-rs-camera-demo",
    author: "TwistedModding",
    version: 1,
    load: load,
}

const WORLD_FOV: f32 = 70.0;
const HANDS_FOV: f32 = 60.0;
const ORBIT_SPEED: f32 = 0.6;
const ORBIT_CENTER_HEIGHT: f32 = 100.0;
const OFFSET: [f32; 3] = [0.0, 0.0, 60.0];

const MODE_OFF: u8 = 0;
const MODE_ORBIT: u8 = 1;
const MODE_OFFSET: u8 = 2;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static STEP: OnceLock<StateStepHook> = OnceLock::new();
static HANDS: OnceLock<HandsProjectionHook> = OnceLock::new();
static CULL: OnceLock<hook::Hook<CullFn>> = OnceLock::new();
static REGISTER: OnceLock<hook::Hook<RegisterDepthGeometryFn>> = OnceLock::new();
static REPLAY: OnceLock<hook::Hook<ProcessCullObjectListFn>> = OnceLock::new();

static MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
/// World and hands FOV targets as `f32` bits; 0 leaves the engine's values alone.
static WORLD_FOV_TARGET: AtomicU32 = AtomicU32::new(0);
static HANDS_FOV_TARGET: AtomicU32 = AtomicU32::new(0);
/// A near plane to write at the next state step, as `f32` bits; 0 when none is pending.
static NEAR_PENDING: AtomicU32 = AtomicU32::new(0);
static SHADOW: AtomicBool = AtomicBool::new(false);
/// With the shadow mode: the cascade replays also keep hidden list entries.
static REPLAY_EXCEPTION: AtomicBool = AtomicBool::new(false);
static FIRST_PERSON: AtomicBool = AtomicBool::new(false);
static THIRD_ROOT: AtomicPtr<skre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());

static STEPS: AtomicU32 = AtomicU32::new(0);
static ORBIT_ANGLE: AtomicU32 = AtomicU32::new(0);
static ENGINE_POS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static WRITTEN_POS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static CENTER: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static HANDS_CALLS: AtomicU32 = AtomicU32::new(0);
static HANDS_FOV_SEEN: AtomicU32 = AtomicU32::new(0);
static NEAR_FRAME: AtomicU32 = AtomicU32::new(0);
/// Shadow counters: visits of the hidden root admitted to shadow cullers, of those by the sun's
/// list walk, visits of the root by other cameras with its hidden bit clear, body skins
/// registered into the cascades, into the focus maps, into local maps, into the depth pre-pass,
/// cascade replays, replays whose skip was lifted.
static ADMITTED: AtomicU32 = AtomicU32::new(0);
/// Of the admissions: by a culler of the sun's list walk, by a culler whose camera is the sun's
/// light camera, by one whose camera is a cascade camera.
static ADMITTED_SUN_WALK: AtomicU32 = AtomicU32::new(0);
static ADMITTED_LIGHT_CAMERA: AtomicU32 = AtomicU32::new(0);
static ADMITTED_CASCADE_CAMERA: AtomicU32 = AtomicU32::new(0);
static BODY_SHOWN_TO_CAMERA: AtomicU32 = AtomicU32::new(0);
static CASCADE_REGISTRATIONS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static FOCUS_REGISTRATIONS: AtomicU32 = AtomicU32::new(0);
static LOCAL_REGISTRATIONS: AtomicU32 = AtomicU32::new(0);
static PREPASS_REGISTRATIONS: AtomicU32 = AtomicU32::new(0);
static REPLAYS: AtomicU32 = AtomicU32::new(0);
static REPLAYS_UNSKIPPED: AtomicU32 = AtomicU32::new(0);

macro_rules! log {
    ($($arg:tt) => {
        if let Some(f) = LOG.lock().unwrap().as_mut() {
            use ::std::io::Write;
            let _ = writeln!(f, $($arg);
        }
    };
}

fn store3(cells: &[AtomicU32; 3], v: [f32; 3]) {
    for (c, x) in cells.iter().zip(v) {
        c.store(x.to_bits(), Ordering::Relaxed);
    }
}

fn load3(cells: &[AtomicU32; 3]) -> [f32; 3] {
    core::array::from_fn(|i| f32::from_bits(cells[i].load(Ordering::Relaxed)))
}

fn load_f32(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    core::array::from_fn(|r| core::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn apply(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|r| (0..3).map(|k| m[r][k] * v[k]).sum())
}

fn rotation_z(angle: f32) -> [[f32; 3]; 3] {
    let (s, c) = angle.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn near3(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < tol)
}

/// After the state's own update: the FOV and near writes, then the placement of the camera root.
unsafe extern "C" fn state_step(camera: *mut skre::TESCamera) {
    unsafe { STEP.get().expect("stored before enabling").original()(camera) }
    if !PlayerCamera::is(camera) {
        return;
    }
    let (Some(pc), Some(player)) = (PlayerCamera::get(), PlayerCharacter::get()) else { return };
    let Some(root) = pc.camera_root() else { return };
    STEPS.fetch_add(1, Ordering::Relaxed);
    FIRST_PERSON.store(pc.is_first_person(), Ordering::Relaxed);
    THIRD_ROOT.store(player.third_person_root().map_or(core::ptr::null_mut(), |r| r.as_ptr().cast_mut().cast()), Ordering::Release);
    let fov = WORLD_FOV_TARGET.load(Ordering::Relaxed);
    if fov != 0 {
        pc.set_world_fov(f32::from_bits(fov));
    }
    let near = NEAR_PENDING.swap(0, Ordering::Relaxed);
    if near != 0 {
        skyrim::render::set_near_plane(f32::from_bits(near));
        NEAR_FRAME.store(skyrim::render::frame_counter(), Ordering::Relaxed);
    }
    let engine = root.local();
    store3(&ENGINE_POS, engine.translation);
    let placed = match MODE.load(Ordering::Relaxed) {
        MODE_ORBIT => {
            let angle = load_f32(&ORBIT_ANGLE) + ORBIT_SPEED * frame_delta_real();
            ORBIT_ANGLE.store(angle.to_bits(), Ordering::Relaxed);
            let center = add(player.position(), [0.0, 0.0, ORBIT_CENTER_HEIGHT]);
            store3(&CENTER, center);
            let rz = rotation_z(angle);
            Transform { rotation: mul(rz, engine.rotation), translation: add(center, apply(rz, sub(engine.translation, center))), scale: engine.scale }
        }
        MODE_OFFSET => Transform { translation: add(engine.translation, OFFSET), ..engine },
        _ => return,
    };
    root.set_local(placed);
    root.update();
    store3(&WRITTEN_POS, placed.translation);
}

unsafe extern "C" fn hands_projection(camera: *mut skre::NiCamera, fov: f32, near: f32, far: f32, width: u32, height: u32, spans_width: bool, set_lod: bool, reference: f32) {
    HANDS_CALLS.fetch_add(1, Ordering::Relaxed);
    HANDS_FOV_SEEN.store(fov.to_bits(), Ordering::Relaxed);
    let target = HANDS_FOV_TARGET.load(Ordering::Relaxed);
    let fov = if target == 0 { fov } else { f32::from_bits(target) };
    unsafe { HANDS.get().expect("stored before enabling").original()(camera, fov, near, far, width, height, spans_width, set_lod, reference) }
}

unsafe extern "C" fn cull(node: *mut skre::NiAVObject, culler: *mut skre::NiCullingProcess, ordered_group: u32) {
    if SHADOW.load(Ordering::Relaxed)
        && FIRST_PERSON.load(Ordering::Relaxed)
        && let (Some(n), Some(c)) = (unsafe { NiAVObject::from_ptr(node) }, unsafe { CullingProcess::from_ptr(culler) })
        && core::ptr::eq(n.as_ptr(), THIRD_ROOT.load(Ordering::Acquire))
    {
        if c.is_shadow_pass() {
            if n.is_hidden() {
                ADMITTED.fetch_add(1, Ordering::Relaxed);
                if c.is_sun_list_walk() {
                    ADMITTED_SUN_WALK.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(sun) = ShadowSceneNode::world().and_then(ShadowSceneNode::sun_shadow_light) {
                    if core::ptr::eq(sun.raw().lightCamera, c.camera_ptr()) {
                        ADMITTED_LIGHT_CAMERA.fetch_add(1, Ordering::Relaxed);
                    } else if sun.owns_camera(c.camera_ptr()) {
                        ADMITTED_CASCADE_CAMERA.fetch_add(1, Ordering::Relaxed);
                    }
                }
                c.cull_node(n, ordered_group);
                return;
            }
        } else if !n.is_hidden() {
            BODY_SHOWN_TO_CAMERA.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe { CULL.get().expect("stored before enabling").original()(node, culler, ordered_group) }
}

unsafe extern "C" fn register_depth_geometry(accumulator: *mut skre::BSShaderAccumulator, geometry: *mut skre::BSGeometry, property: *mut skre::BSShaderProperty) -> bool {
    if SHADOW.load(Ordering::Relaxed)
        && FIRST_PERSON.load(Ordering::Relaxed)
        && let (Some(a), Some(g), Some(root)) = (unsafe { BSShaderAccumulator::from_ptr(accumulator) }, unsafe { NiAVObject::from_ptr(geometry.cast()) }, unsafe { NiAVObject::from_ptr(THIRD_ROOT.load(Ordering::Acquire)) })
        && g.is_under(root, 24)
    {
        match a.render_mode_raw() {
            0xe if a.is_cascade_accumulator() => CASCADE_REGISTRATIONS[usize::from(a.cascade_index() != 0)].fetch_add(1, Ordering::Relaxed),
            0xe => FOCUS_REGISTRATIONS.fetch_add(1, Ordering::Relaxed),
            0xd | 0xf => LOCAL_REGISTRATIONS.fetch_add(1, Ordering::Relaxed),
            _ => PREPASS_REGISTRATIONS.fetch_add(1, Ordering::Relaxed),
        };
    }
    unsafe { REGISTER.get().expect("stored before enabling").original()(accumulator, geometry, property) }
}

unsafe extern "C" fn replay(culler: *mut skre::BSCullingProcess, objects: *mut skre::BSTArray, camera: *mut skre::NiCamera, skip_hidden: bool, parallel: bool) {
    let mut skip = skip_hidden;
    if skip_hidden
        && SHADOW.load(Ordering::Relaxed)
        && FIRST_PERSON.load(Ordering::Relaxed)
        && ShadowSceneNode::world().and_then(ShadowSceneNode::sun_shadow_light).is_some_and(|sun| sun.is_shadow_camera(camera))
    {
        REPLAYS.fetch_add(1, Ordering::Relaxed);
        if REPLAY_EXCEPTION.load(Ordering::Relaxed) {
            REPLAYS_UNSKIPPED.fetch_add(1, Ordering::Relaxed);
            skip = false;
        }
    }
    unsafe { REPLAY.get().expect("stored before enabling").original()(culler, objects, camera, skip, parallel) }
}

fn install() -> Result<(), String> {
    let step = unsafe { skyrim::camera::hook_state_step(state_step) }.map_err(|e| format!("state step: {e}"))?;
    let hands = unsafe { skyrim::camera::hook_hands_projection(hands_projection) }.map_err(|e| format!("hands projection: {e}"))?;
    let cull_hook = unsafe { skyrim::render::hook_cull(cull) }.map_err(|e| format!("NiAVObject::Cull: {e}"))?;
    let register = unsafe { skyrim::render::hook_register_depth_geometry(register_depth_geometry) }.map_err(|e| format!("RegisterDepthGeometry: {e}"))?;
    let replay_hook = unsafe { skyrim::render::hook_process_cull_object_list(replay) }.map_err(|e| format!("ProcessCullObjectList: {e}"))?;
    log!("state step sites {:x?}, hands projection sites {:x?}, Cull {:#x}, RegisterDepthGeometry {:#x}, ProcessCullObjectList {:#x}", step.sites(), hands.sites(), cull_hook.target(), register.target(), replay_hook.target());
    let _ = STEP.set(step);
    let _ = HANDS.set(hands);
    let _ = CULL.set(cull_hook);
    let _ = REGISTER.set(register);
    let _ = REPLAY.set(replay_hook);
    STEP.get().expect("just set").enable().map_err(|e| format!("enable state step: {e}"))?;
    HANDS.get().expect("just set").enable().map_err(|e| format!("enable hands projection: {e}"))?;
    CULL.get().expect("just set").enable().map_err(|e| format!("enable Cull: {e}"))?;
    REGISTER.get().expect("just set").enable().map_err(|e| format!("enable RegisterDepthGeometry: {e}"))?;
    REPLAY.get().expect("just set").enable().map_err(|e| format!("enable ProcessCullObjectList: {e}"))?;
    Ok(())
}

fn reset_shadow_counters() {
    for c in [&ADMITTED, &ADMITTED_SUN_WALK, &ADMITTED_LIGHT_CAMERA, &ADMITTED_CASCADE_CAMERA, &BODY_SHOWN_TO_CAMERA, &CASCADE_REGISTRATIONS[0], &CASCADE_REGISTRATIONS[1], &FOCUS_REGISTRATIONS, &LOCAL_REGISTRATIONS, &PREPASS_REGISTRATIONS, &REPLAYS, &REPLAYS_UNSKIPPED] {
        c.store(0, Ordering::Relaxed);
    }
}

fn all_off() {
    MODE.store(MODE_OFF, Ordering::Relaxed);
    WORLD_FOV_TARGET.store(0, Ordering::Relaxed);
    HANDS_FOV_TARGET.store(0, Ordering::Relaxed);
    SHADOW.store(false, Ordering::Relaxed);
    REPLAY_EXCEPTION.store(false, Ordering::Relaxed);
}

type Check = (&'static str, Result<String, String>);

fn check(ok: bool, detail: String) -> Result<String, String> {
    if ok { Ok(detail) } else { Err(detail) }
}

fn placement_checks(expected: [f32; 3]) -> Vec<Check> {
    let written = load3(&WRITTEN_POS);
    let root = PlayerCamera::get().and_then(|c| c.camera_root()).map(|r| r.world().translation);
    let frame = main_camera().map(|c| c.world().translation);
    let cached = PlayerCamera::get().map(PlayerCamera::cached_position);
    vec![
        ("steps", check(STEPS.load(Ordering::Relaxed) > 0, format!("{} state steps", STEPS.load(Ordering::Relaxed)))),
        ("written", check(near3(written, expected, 0.5), format!("written {written:?} expected {expected:?}"))),
        ("root", check(root.is_some_and(|r| near3(r, written, 0.01)), format!("root world {root:?} written {written:?}"))),
        ("frame_camera", check(frame.is_some_and(|f| near3(f, written, 0.01)), format!("frame camera world {frame:?}"))),
        ("cached_position", check(cached.is_some_and(|c| near3(c, written, 0.01)), format!("cached position {cached:?}"))),
    ]
}

fn report(label: &str) {
    let mut checks: Vec<Check> = Vec::new();
    let console = match label {
        "orbit" => {
            let engine = load3(&ENGINE_POS);
            let written = load3(&WRITTEN_POS);
            let center = load3(&CENTER);
            let angle = load_f32(&ORBIT_ANGLE);
            checks = placement_checks(written);
            checks.push(("angle", check(angle > 0.5, format!("orbit angle {angle:.3} rad"))));
            checks.push(("radius", check((length(sub(written, center)) - length(sub(engine, center))).abs() < 0.5, format!("radius written {:.1} engine {:.1} around {center:?}", length(sub(written, center)), length(sub(engine, center))))));
            checks.push(("moved", check(length(sub(written, engine)) > 10.0, format!("engine {engine:?} written {written:?}"))));
            format!("camera-demo orbit: root ({:.2}, {:.2}, {:.2}) center ({:.2}, {:.2}, {:.2}) radius {:.2}", written[0], written[1], written[2], center[0], center[1], center[2], length(sub(written, center)))
        }
        "offset" => {
            let engine = load3(&ENGINE_POS);
            let expected = add(engine, OFFSET);
            checks = placement_checks(expected);
            let written = load3(&WRITTEN_POS);
            format!("camera-demo offset: root ({:.2}, {:.2}, {:.2}) engine ({:.2}, {:.2}, {:.2})", written[0], written[1], written[2], engine[0], engine[1], engine[2])
        }
        "fov" => {
            let pc = PlayerCamera::get();
            let world = pc.map_or(0.0, PlayerCamera::world_fov);
            let frame = main_camera().map(|c| (c.frustum(), c.lod_adjust()));
            let hands = hands_camera().map(skyrim::render::NiCamera::frustum);
            let offset = skyrim::camera::fov_offset();
            let expected_world = ((WORLD_FOV + offset).to_radians() / 2.0).tan();
            let expected_hands = (HANDS_FOV.to_radians() / 2.0).tan();
            let seen = load_f32(&HANDS_FOV_SEEN);
            let copy = hands_fov();
            checks.push(("world_fov", check((world - WORLD_FOV).abs() < 0.01, format!("PlayerCamera.worldFOV {world}, offset {offset}"))));
            checks.push(("frame_frustum", check(frame.is_some_and(|(f, _)| (f[1] - expected_world).abs() < 0.001 && (f[4] - 15.0).abs() < 0.01), format!("frame frustum {:?} expected right {expected_world:.4}", frame.map(|f| f.0)))));
            checks.push(("lod_adjust", check(frame.is_some_and(|(_, l)| (l - WORLD_FOV / 80.0).abs() < 0.01), format!("lodAdjust {:?}", frame.map(|f| f.1)))));
            checks.push(("first_person", check(pc.is_some_and(PlayerCamera::is_first_person), format!("state {:?}", pc.and_then(PlayerCamera::state)))));
            checks.push(("hands_sites", check(HANDS_CALLS.load(Ordering::Relaxed) >= 2, format!("{} projection calls, fov argument {seen}, hands copy {copy}", HANDS_CALLS.load(Ordering::Relaxed)))));
            checks.push(("hands_frustum", check(hands.is_some_and(|f| (f[1] - expected_hands).abs() < 0.001 && (f[4] - 5.0).abs() < 0.01), format!("hands frustum {hands:?} expected right {expected_hands:.4}"))));
            checks.push(("hands_copy", check((copy - seen).abs() < 0.01 && (copy - pc.map_or(0.0, PlayerCamera::first_person_fov)).abs() < 0.01, format!("firstPersonFOV {:?} copy {copy} seen at the sites {seen}", pc.map(PlayerCamera::first_person_fov)))));
            format!("camera-demo fov: world {world} frame right {:.4} hands right {:.4}", frame.map_or(0.0, |f| f.0[1]), hands.map_or(0.0, |f| f[1]))
        }
        "near5" | "near15" => {
            let expected = if label == "near5" { 5.0 } else { 15.0 };
            let setting = skyrim::render::near_distance();
            let frame = main_camera().map(|c| (c.frustum(), c.far_near_ratio()));
            let shift = skyrim::render::depth_shift_near();
            let slot = SceneGraph::get().map_or(0.0, SceneGraph::near);
            let gpu: Vec<f32> = FrameState::get().map(|f| f.camera_entries().iter().filter(|e| main_camera().is_some_and(|c| core::ptr::eq(c.as_ptr(), e.camera_ptr()))).map(skyrim::render::CameraMatrixEntry::near).collect()).unwrap_or_default();
            let cascades = ShadowSceneNode::world().and_then(ShadowSceneNode::sun_shadow_light).map(|s| (s.cascade_near(), s.cascade_far()));
            let far = frame.map_or(0.0, |f| f.0[5]);
            checks.push(("setting", check((setting - expected).abs() < 0.01, format!("fNearDistance {setting}"))));
            checks.push(("frame_near", check(frame.is_some_and(|(f, r)| (f[4] - expected).abs() < 0.01 && (r - f[5] / expected).abs() < 1.0), format!("frame frustum {:?} far/near {:?}", frame.map(|f| f.0), frame.map(|f| f.1)))));
            checks.push(("depth_shift", check((shift - expected).abs() < 0.01, format!("g_DepthShiftNear {shift}, slot 63 {slot}"))));
            checks.push(("gpu", check(gpu.len() == 2 && gpu.iter().all(|n| (n - expected).abs() < 0.01), format!("GPU snapshot nears {gpu:?} (frame {} since the write at {})", skyrim::render::frame_counter(), NEAR_FRAME.load(Ordering::Relaxed)))));
            checks.push(("cascades", check(cascades.is_some_and(|(n, _)| (n[0] - expected).abs() < 0.01), format!("cascade near {:?} far {:?}", cascades.map(|c| c.0), cascades.map(|c| c.1)))));
            format!("camera-demo {label}: setting {setting} frame {:.2} shift {shift} gpu {gpu:?} far {far}", frame.map_or(0.0, |f| f.0[4]))
        }
        "shadow" => {
            let pc = PlayerCamera::get();
            let root = unsafe { NiAVObject::from_ptr(THIRD_ROOT.load(Ordering::Acquire)) };
            let skins: Vec<(u32, Option<u32>)> = root
                .and_then(NiAVObject::as_node)
                .map(|n| n.children().iter().flatten().filter_map(|c| c.as_geometry()).filter_map(|g| g.shader_property().map(|p| (p.last_render_pass_state(), p.receiver_mask()))).collect())
                .unwrap_or_default();
            let cascades = [CASCADE_REGISTRATIONS[0].load(Ordering::Relaxed), CASCADE_REGISTRATIONS[1].load(Ordering::Relaxed)];
            checks.push(("first_person", check(pc.is_some_and(PlayerCamera::is_first_person), format!("state {:?}", pc.and_then(PlayerCamera::state)))));
            checks.push(("root_hidden", check(root.is_some_and(NiAVObject::is_hidden), format!("third-person root {:?} flags {:#x?}", root.map(NiAVObject::as_ptr), root.map(NiAVObject::flags)))));
            checks.push(("admitted", check(ADMITTED.load(Ordering::Relaxed) > 0, format!("hidden root admitted {} times to shadow cullers: {} by the sun's list walk, {} with the sun's light camera, {} with a cascade camera", ADMITTED.load(Ordering::Relaxed), ADMITTED_SUN_WALK.load(Ordering::Relaxed), ADMITTED_LIGHT_CAMERA.load(Ordering::Relaxed), ADMITTED_CASCADE_CAMERA.load(Ordering::Relaxed)))));
            checks.push(("main_view", check(BODY_SHOWN_TO_CAMERA.load(Ordering::Relaxed) == 0 && PREPASS_REGISTRATIONS.load(Ordering::Relaxed) == 0, format!("root shown to other cameras {} times, body skins in the depth pre-pass {}", BODY_SHOWN_TO_CAMERA.load(Ordering::Relaxed), PREPASS_REGISTRATIONS.load(Ordering::Relaxed)))));
            checks.push(("cascades", check(cascades[0] > 0 && cascades[1] > 0, format!("body skins registered into cascade 0 {} and cascade 1 {} times, focus {}, local {}; replays {} ({} with the skip lifted, exception {})", cascades[0], cascades[1], FOCUS_REGISTRATIONS.load(Ordering::Relaxed), LOCAL_REGISTRATIONS.load(Ordering::Relaxed), REPLAYS.load(Ordering::Relaxed), REPLAYS_UNSKIPPED.load(Ordering::Relaxed), REPLAY_EXCEPTION.load(Ordering::Relaxed)))));
            checks.push(("receivers", check(!skins.is_empty() && skins.iter().all(|(_, m)| m.is_some_and(|m| m & 3 == 3)), format!("skins (lastRenderPassState, receiver mask) {skins:x?}: the cascade bits stay set while the main view never registers the skins"))));
            format!("camera-demo shadow: admitted {} cascade registrations {:?} main view {}", ADMITTED.load(Ordering::Relaxed), cascades, BODY_SHOWN_TO_CAMERA.load(Ordering::Relaxed))
        }
        _ => format!("camera-demo: unknown report {label}"),
    };
    let (mut pass, mut fail) = (0, 0);
    for (name, result) in &checks {
        match result {
            Ok(d) => {
                pass += 1;
                log!("PASS {name}: {d}");
            }
            Err(d) => {
                fail += 1;
                log!("FAIL {name}: {d}");
            }
        }
    }
    log!("summary camera {label}: {pass} pass, {fail} fail");
    skyrim::input::print(&console);
}

struct DemoCommand;

static PARAMS: [Param; 2] = [Param::new(c"mode", ScriptParamType::String, false), Param::new(c"value", ScriptParamType::String, true)];

impl Command for DemoCommand {
    const NAME: &'static CStr = c"CameraDemo";
    const HELP: &'static CStr = c"skyrim-rs camera demo: orbit | offset | fov | fovoff | near <n> | shadow [1 = replay exception] | report <label> | off";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let mode = match context.args().first() {
            Some(Arg::Str(s)) => (*s).to_owned(),
            _ => String::new(),
        };
        let value = match context.args().get(1) {
            Some(Arg::Str(s)) => (*s).to_owned(),
            _ => String::new(),
        };
        let number = value.parse::<f32>().ok();
        log!("command: {mode} {value}");
        match mode.to_ascii_lowercase().as_str() {
            "orbit" => {
                ORBIT_ANGLE.store(0, Ordering::Relaxed);
                MODE.store(MODE_ORBIT, Ordering::Relaxed);
            }
            "offset" => MODE.store(MODE_OFFSET, Ordering::Relaxed),
            "fov" => {
                HANDS_CALLS.store(0, Ordering::Relaxed);
                WORLD_FOV_TARGET.store(WORLD_FOV.to_bits(), Ordering::Relaxed);
                HANDS_FOV_TARGET.store(HANDS_FOV.to_bits(), Ordering::Relaxed);
            }
            "fovoff" => {
                WORLD_FOV_TARGET.store(80.0f32.to_bits(), Ordering::Relaxed);
                HANDS_FOV_TARGET.store(0, Ordering::Relaxed);
            }
            "near" => NEAR_PENDING.store(number.unwrap_or(15.0).to_bits(), Ordering::Relaxed),
            "shadow" => {
                reset_shadow_counters();
                REPLAY_EXCEPTION.store(number.is_some_and(|v| v != 0.0), Ordering::Relaxed);
                SHADOW.store(true, Ordering::Relaxed);
            }
            "off" => all_off(),
            "report" => report(&value.to_ascii_lowercase()),
            other => skyrim::input::print(&format!("camera-demo: unknown mode {other}")),
        }
        true
    }
}

unsafe extern "C" fn on_message(msg: *mut skse::Message) {
    match unsafe { (*msg).kind } {
        skse::MessagingInterface::DATA_LOADED => {
            if let Err(e) = install() {
                log!("FAIL install: {e}");
            }
            match skyrim::input::register::<DemoCommand>() {
                Ok(slot) => log!("CameraDemo took over console opcode {:#x} ({})", slot.opcode, slot.replaced),
                Err(e) => log!("FAIL register CameraDemo: {e}"),
            }
        }
        skse::MessagingInterface::PRE_LOAD_GAME | skse::MessagingInterface::NEW_GAME => {
            all_off();
            THIRD_ROOT.store(core::ptr::null_mut(), Ordering::Release);
        }
        _ => {}
    }
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(file) = skse::log::open(skse.runtime_version, "skyrim-rs-camera-demo") else {
        return false;
    };
    *LOG.lock().unwrap() = Some(file);
    if let Err(e) = skyrim::init(skse) {
        log!("FAIL init: {e}");
        return false;
    }
    match skse.messaging() {
        Some(m) if m.listen_skse(skse.plugin_handle(), on_message) => {
            log!("listening for SKSE messages");
            true
        }
        _ => {
            log!("FAIL init: no messaging interface");
            false
        }
    }
}
