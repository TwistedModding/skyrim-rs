// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Per-frame timing of the frame's shadow and depth steps (RE `render.md` section 1, `graphics.md`
//! sections 4 to 6) and counts of the per-geometry registration calls, written as one CSV row per
//! frame while a labelled recording runs. The step detours run on the frame thread; the
//! registration detours also on the job threads (depth pre-pass mode 0xc), so every counter is an
//! atomic. Times are `rdtsc` ticks converted with the ratio measured against the system clock
//! when a recording starts.

use core::arch::x86_64::_rdtsc;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use sre::raw::fns;
use sre::{BSGeometry, BSLightingShaderProperty, BSShaderAccumulator, BSShaderProperty, BSShadowDirectionalLight, BSShadowLight, BSTArray, NiCamera, ShadowMapData};

use crate::cache;
use crate::detour::Detour;

/// `BSLightingShaderProperty` slot 43 as the ledger spells it: `(this, geometry, renderMode, accumulator) -> RenderPass*`.
pub type GetDepthRenderPassesFn = unsafe extern "C" fn(*mut BSLightingShaderProperty, *mut BSGeometry, u32, *mut BSShaderAccumulator) -> *mut c_void;

const REGIONS: &[&str] = &[
    "render_world",
    "cull_scene",
    "sun_update",
    "sun_lists",
    "cull_lights",
    "sun_casters",
    "map_cull_sun",
    "map_cull_local",
    "focus_cull",
    "shadow_maps",
    "map_draw_sun",
    "map_draw_other",
    "prepass",
    "shadow_masks",
    "world_color",
    "reg_depth_c",
    "reg_depth_d",
    "reg_depth_e",
    "reg_depth_f",
    "reg_depth_other",
    "get_passes",
];
const RENDER_WORLD: usize = 0;
const CULL_SCENE: usize = 1;
const SUN_UPDATE: usize = 2;
const SUN_LISTS: usize = 3;
const CULL_LIGHTS: usize = 4;
const SUN_CASTERS: usize = 5;
const MAP_CULL_SUN: usize = 6;
const MAP_CULL_LOCAL: usize = 7;
const FOCUS_CULL: usize = 8;
const SHADOW_MAPS: usize = 9;
const MAP_DRAW_SUN: usize = 10;
const MAP_DRAW_OTHER: usize = 11;
const PREPASS: usize = 12;
const SHADOW_MASKS: usize = 13;
const WORLD_COLOR: usize = 14;
const REG_DEPTH: usize = 15;
const REG_DEPTH_OTHER: usize = 19;
const GET_PASSES: usize = 20;

/// Call counts without a time: `RegisterGeometry`, passes drawn inside a shadow map draw, passes
/// drawn elsewhere, the cascade registrations the memo answered and the first-cascade drops it recorded.
const COUNTS: &[&str] = &["reg_geom", "passes_shadow", "passes_other", "cache_skip", "cache_drop"];
const COUNT_REG_GEOM: usize = 0;
const COUNT_PASSES_SHADOW: usize = 1;
const COUNT_PASSES_OTHER: usize = 2;
const COUNT_CACHE_SKIP: usize = 3;
const COUNT_CACHE_DROP: usize = 4;

struct Region {
    ticks: AtomicU64,
    calls: AtomicU32,
}

impl Region {
    const fn new() -> Self {
        Self { ticks: AtomicU64::new(0), calls: AtomicU32::new(0) }
    }

    fn add(&self, start: u64) {
        self.ticks.fetch_add(now().wrapping_sub(start), Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    fn take(&self) -> (u64, u32) {
        (self.ticks.swap(0, Ordering::Relaxed), self.calls.swap(0, Ordering::Relaxed))
    }
}

static REGION: [Region; REGIONS.len()] = [const { Region::new() }; REGIONS.len()];
static COUNT: [AtomicU32; COUNTS.len()] = [const { AtomicU32::new(0) }; COUNTS.len()];
/// The previous frame's values, for the console.
static LAST_REGION: [(AtomicU64, AtomicU32); REGIONS.len()] = [const { (AtomicU64::new(0), AtomicU32::new(0)) }; REGIONS.len()];
static LAST_COUNT: [AtomicU32; COUNTS.len()] = [const { AtomicU32::new(0) }; COUNTS.len()];
static LAST_FRAME_START: AtomicU64 = AtomicU64::new(0);
static LAST_INTERVAL: AtomicU64 = AtomicU64::new(0);
/// Ticks per millisecond, 0 while no recording runs.
static TICKS_PER_MS: AtomicU64 = AtomicU64::new(0);
static IN_SHADOW_MAP: AtomicU32 = AtomicU32::new(0);

struct Recording {
    label: String,
    frames: u32,
    out: BufWriter<File>,
}

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

fn now() -> u64 {
    unsafe { _rdtsc() }
}

fn count(index: usize) {
    COUNT[index].fetch_add(1, Ordering::Relaxed);
}

macro_rules! timed {
    ($hook:ident, $detour:ident, $region:expr, fn($($a:ident: $t:ty),*) $(-> $r:ty)?) => {
        static $hook: Detour<unsafe extern "C" fn($($t),*) $(-> $r)?> = Detour::new();
        unsafe extern "C" fn $detour($($a: $t),*) $(-> $r)? {
            let start = now();
            let out = unsafe { $hook.original()($($a),*) };
            REGION[$region].add(start);
            out
        }
    };
}

timed!(RENDER_WORLD_HOOK, render_world, RENDER_WORLD, fn(owner: *mut c_void, flags: u32));
timed!(CULL_SCENE_HOOK, cull_scene, CULL_SCENE, fn(camera: *mut NiCamera, a: u64, b: u64, c: u64));
timed!(SUN_UPDATE_HOOK, sun_update, SUN_UPDATE, fn(this: *mut BSShadowDirectionalLight, camera: *mut NiCamera, a: u64, b: u64) -> u64);
timed!(SUN_LISTS_HOOK, sun_lists, SUN_LISTS, fn(this: *mut BSShadowDirectionalLight, lists: *mut BSTArray, portals: *mut c_void));
timed!(CULL_LIGHTS_HOOK, cull_lights, CULL_LIGHTS, fn());
timed!(SUN_CASTERS_HOOK, sun_casters, SUN_CASTERS, fn(this: *mut BSShadowDirectionalLight, counter: *mut u32));
timed!(FOCUS_CULL_HOOK, focus_cull, FOCUS_CULL, fn(this: *mut BSShadowLight, counter: *mut u32));
timed!(SHADOW_MAPS_HOOK, shadow_maps, SHADOW_MAPS, fn());
timed!(PREPASS_HOOK, prepass, PREPASS, fn(first_person: bool, control_driven: bool));
timed!(SHADOW_MASKS_HOOK, shadow_masks, SHADOW_MASKS, fn(on: bool));
timed!(WORLD_COLOR_HOOK, world_color, WORLD_COLOR, fn(paused: bool));

static MAP_CULL_HOOK: Detour<fns::BSShadowLight__CullMapCasters_fn> = Detour::new();
static MAP_DRAW_HOOK: Detour<fns::BSShadowLight__RenderShadowMap_fn> = Detour::new();
static REG_DEPTH_HOOK: Detour<fns::BSShaderAccumulator__RegisterDepthGeometry_fn> = Detour::new();
static REG_GEOM_HOOK: Detour<fns::BSShaderAccumulator__RegisterGeometry_fn> = Detour::new();
static GET_PASSES_HOOK: Detour<GetDepthRenderPassesFn> = Detour::new();
static PASS_HOOK: Detour<fns::RenderPassImmediately_fn> = Detour::new();

/// Whether the map belongs to the sun's cascades (its accumulator is a cascade accumulator).
fn is_sun_map(map: *mut ShadowMapData) -> bool {
    unsafe { map.as_ref().and_then(|m| m.accumulator()).is_some_and(|a| a.isCascadeAccumulator != 0) }
}

unsafe extern "C" fn map_cull(this: *mut BSShadowLight, map: *mut ShadowMapData, counter: *mut u32, lists: *mut BSTArray, tag: u32) {
    let start = now();
    unsafe { MAP_CULL_HOOK.original()(this, map, counter, lists, tag) };
    REGION[if is_sun_map(map) { MAP_CULL_SUN } else { MAP_CULL_LOCAL }].add(start);
}

unsafe extern "C" fn map_draw(this: *mut BSShadowLight, map: *mut ShadowMapData, counter: *mut u32, flags: u32) {
    let start = now();
    IN_SHADOW_MAP.fetch_add(1, Ordering::Relaxed);
    unsafe { MAP_DRAW_HOOK.original()(this, map, counter, flags) };
    IN_SHADOW_MAP.fetch_sub(1, Ordering::Relaxed);
    REGION[if is_sun_map(map) { MAP_DRAW_SUN } else { MAP_DRAW_OTHER }].add(start);
}

unsafe extern "C" fn reg_depth(this: *mut BSShaderAccumulator, geometry: *mut BSGeometry, property: *mut BSShaderProperty) -> bool {
    let start = now();
    if cache::skip(this, geometry as usize) {
        count(COUNT_CACHE_SKIP);
        return true;
    }
    let out = unsafe { REG_DEPTH_HOOK.original()(this, geometry, property) };
    let mode = unsafe { this.as_ref() }.map_or(0, |a| a.renderMode);
    REGION[if (0xc..=0xf).contains(&mode) { REG_DEPTH + (mode - 0xc) as usize } else { REG_DEPTH_OTHER }].add(start);
    out
}

unsafe extern "C" fn reg_geom(this: *mut BSShaderAccumulator, geometry: *mut BSGeometry, arg: *mut c_void) -> bool {
    count(COUNT_REG_GEOM);
    unsafe { REG_GEOM_HOOK.original()(this, geometry, arg) }
}

unsafe extern "C" fn get_passes(this: *mut BSLightingShaderProperty, geometry: *mut BSGeometry, mode: u32, accumulator: *mut BSShaderAccumulator) -> *mut c_void {
    let start = now();
    let out = unsafe { GET_PASSES_HOOK.original()(this, geometry, mode, accumulator) };
    REGION[GET_PASSES].add(start);
    if out.is_null() && cache::record_drop(accumulator, geometry as usize) {
        count(COUNT_CACHE_DROP);
    }
    out
}

unsafe extern "C" fn pass(pass: *mut c_void, technique: u32, alpha_test: bool, flags: u32) {
    count(if IN_SHADOW_MAP.load(Ordering::Relaxed) != 0 { COUNT_PASSES_SHADOW } else { COUNT_PASSES_OTHER });
    unsafe { PASS_HOOK.original()(pass, technique, alpha_test, flags) }
}

/// Hooks every step and counter; the hooks stay in until [`remove`].
pub fn install() -> Result<(), String> {
    unsafe {
        RENDER_WORLD_HOOK.install(sre::id!(fns::RenderWorld), render_world, "RenderWorld")?;
        CULL_SCENE_HOOK.install(sre::id!(fns::CullShadowCastersAndScene), cull_scene, "CullShadowCastersAndScene")?;
        SUN_UPDATE_HOOK.install(sre::id!(fns::BSShadowDirectionalLight__UpdateForCamera), sun_update, "BSShadowDirectionalLight::UpdateForCamera")?;
        SUN_LISTS_HOOK.install(sre::id!(fns::BSShadowDirectionalLight__CullGeometryLists), sun_lists, "BSShadowDirectionalLight::CullGeometryLists")?;
        CULL_LIGHTS_HOOK.install(sre::id!(fns::CullShadowLights), cull_lights, "CullShadowLights")?;
        SUN_CASTERS_HOOK.install(sre::id!(fns::BSShadowDirectionalLight__CullCasters), sun_casters, "BSShadowDirectionalLight::CullCasters")?;
        MAP_CULL_HOOK.install(sre::id!(fns::BSShadowLight__CullMapCasters), map_cull, "BSShadowLight::CullMapCasters")?;
        FOCUS_CULL_HOOK.install(sre::id!(fns::BSShadowLight__CullFocusShadowCasters), focus_cull, "BSShadowLight::CullFocusShadowCasters")?;
        SHADOW_MAPS_HOOK.install(sre::id!(fns::RenderShadowMaps), shadow_maps, "RenderShadowMaps")?;
        MAP_DRAW_HOOK.install(sre::id!(fns::BSShadowLight__RenderShadowMap), map_draw, "BSShadowLight::RenderShadowMap")?;
        PREPASS_HOOK.install(sre::id!(fns::RenderWorldDepthPrepass), prepass, "RenderWorldDepthPrepass")?;
        SHADOW_MASKS_HOOK.install(sre::id!(fns::RenderShadowMasks), shadow_masks, "RenderShadowMasks")?;
        WORLD_COLOR_HOOK.install(sre::id!(fns::RenderWorldColor), world_color, "RenderWorldColor")?;
        REG_DEPTH_HOOK.install(sre::id!(fns::BSShaderAccumulator__RegisterDepthGeometry), reg_depth, "BSShaderAccumulator::RegisterDepthGeometry")?;
        REG_GEOM_HOOK.install(sre::id!(fns::BSShaderAccumulator__RegisterGeometry), reg_geom, "BSShaderAccumulator::RegisterGeometry")?;
        GET_PASSES_HOOK.install(sre::id!(fns::BSLightingShaderProperty__GetDepthRenderPasses), get_passes, "BSLightingShaderProperty::GetDepthRenderPasses")?;
        PASS_HOOK.install(sre::id!(fns::RenderPassImmediately), pass, "RenderPassImmediately")?;
    }
    Ok(())
}

pub fn remove() {
    stop();
    RENDER_WORLD_HOOK.remove();
    CULL_SCENE_HOOK.remove();
    SUN_UPDATE_HOOK.remove();
    SUN_LISTS_HOOK.remove();
    CULL_LIGHTS_HOOK.remove();
    SUN_CASTERS_HOOK.remove();
    MAP_CULL_HOOK.remove();
    FOCUS_CULL_HOOK.remove();
    SHADOW_MAPS_HOOK.remove();
    MAP_DRAW_HOOK.remove();
    PREPASS_HOOK.remove();
    SHADOW_MASKS_HOOK.remove();
    WORLD_COLOR_HOOK.remove();
    REG_DEPTH_HOOK.remove();
    REG_GEOM_HOOK.remove();
    GET_PASSES_HOOK.remove();
    PASS_HOOK.remove();
}

/// Starts a recording into `<SKSE log folder>\drawcall-fix-<label>.csv`, ending the previous one.
/// Measures the tick rate over 200 ms first, so it is called from the console thread, not the frame.
pub fn start(runtime: u32, label: &str) -> Result<(), String> {
    stop();
    let (t0, c0) = (Instant::now(), now());
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (t1, c1) = (Instant::now(), now());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let ticks_per_ms = ((c1 - c0) as f64 / (t1 - t0).as_secs_f64() / 1000.0) as u64;
    let path = skse::log::dir(runtime).map_err(|e| e.to_string())?.join(format!("drawcall-fix-{label}.csv"));
    let mut out = BufWriter::new(File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    let header = REGIONS.iter().flat_map(|r| [format!("{r}_ms"), format!("{r}_n")]).chain(COUNTS.iter().map(|c| format!("{c}_n"))).collect::<Vec<_>>().join(",");
    writeln!(out, "label,frame,interval_ms,{header}").map_err(|e| e.to_string())?;
    *RECORDING.lock().unwrap_or_else(PoisonError::into_inner) = Some(Recording { label: label.into(), frames: 0, out });
    TICKS_PER_MS.store(ticks_per_ms.max(1), Ordering::Relaxed);
    Ok(())
}

/// Ends the recording and flushes its file; the frames written.
pub fn stop() -> u32 {
    TICKS_PER_MS.store(0, Ordering::Relaxed);
    let Some(mut recording) = RECORDING.lock().unwrap_or_else(PoisonError::into_inner).take() else { return 0 };
    let _ = recording.out.flush();
    recording.frames
}

/// Takes the frame's counters and writes its row when a recording runs; called at the end of
/// every frame on the frame thread.
pub fn end_frame() {
    let start = now();
    let previous = LAST_FRAME_START.swap(start, Ordering::Relaxed);
    LAST_INTERVAL.store(if previous == 0 { 0 } else { start - previous }, Ordering::Relaxed);
    for (region, last) in REGION.iter().zip(&LAST_REGION) {
        let (ticks, calls) = region.take();
        last.0.store(ticks, Ordering::Relaxed);
        last.1.store(calls, Ordering::Relaxed);
    }
    for (count, last) in COUNT.iter().zip(&LAST_COUNT) {
        last.store(count.swap(0, Ordering::Relaxed), Ordering::Relaxed);
    }
    let ticks_per_ms = TICKS_PER_MS.load(Ordering::Relaxed);
    if ticks_per_ms == 0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let ms = |ticks: u64| ticks as f64 / ticks_per_ms as f64;
    let mut guard = RECORDING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(recording) = guard.as_mut() else { return };
    let mut row = format!("{},{},{:.3}", recording.label, skyrim::render::frame_counter(), ms(LAST_INTERVAL.load(Ordering::Relaxed)));
    for last in &LAST_REGION {
        let _ = write!(row, ",{:.3},{}", ms(last.0.load(Ordering::Relaxed)), last.1.load(Ordering::Relaxed));
    }
    for last in &LAST_COUNT {
        let _ = write!(row, ",{}", last.load(Ordering::Relaxed));
    }
    recording.frames += 1;
    let _ = writeln!(recording.out, "{row}");
    if recording.frames % 300 == 0 {
        let _ = recording.out.flush();
    }
}

/// The previous frame's counters as one line, for the console (ticks where no recording gives a rate).
pub fn status() -> String {
    let recording = RECORDING.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map_or("off".to_string(), |r| format!("{} ({} frames)", r.label, r.frames));
    let regions = REGIONS.iter().zip(&LAST_REGION).map(|(name, last)| format!("{name} {}x{}", last.1.load(Ordering::Relaxed), last.0.load(Ordering::Relaxed))).collect::<Vec<_>>().join(" ");
    let counts = COUNTS.iter().zip(&LAST_COUNT).map(|(name, last)| format!("{name} {}", last.load(Ordering::Relaxed))).collect::<Vec<_>>().join(" ");
    format!("recording {recording}; last frame {} ticks: {regions}; {counts}", LAST_INTERVAL.load(Ordering::Relaxed))
}
