//! One test per `skyrim::animation` API, against the RE repo's `docs/subsystems/animation-verify.md`.
//! `install` post-hooks `ApplyPoseToBoneNodes` and detours `PlayerCharacter::UpdateNodesAfterRender`
//! at `DataLoaded`; `register` adds a sink on the player's graph 0 after a load; the hooks sample
//! the player's head bone at the pose write and at the world update. `TESTS` reads the graphs after
//! a load and at the save; `RUN_TESTS` checks what the scenario (a walk, a weapon draw, an event,
//! first person) produced.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use skyrim::actors::PlayerCharacter;
use skyrim::animation::{AnimationGraph, AnimationGraphHolder, ApplyPoseFn, HkQsTransform, UpdateNodesAfterRenderFn, current_speed, graph_event_source, graph_manager};
use skyrim::events::{BSAnimationGraphEvent, BSEventNotifyControl, BSTEventSource, EventSink, Registration, Sink};
use skyrim::render::{NiAVObject, Transform};
use skyrim::string::BSFixedString;

use crate::{Test, check};

pub const TESTS: &[(&str, Test)] = &[("graphs", graphs), ("head_bone", head_bone)];
pub const RUN_TESTS: &[(&str, Test)] = &[("variables", variables), ("weapon_state", weapon_state), ("events", events), ("pose_write", pose_write), ("world_update", world_update)];

static APPLY: OnceLock<hook::Hook<ApplyPoseFn>> = OnceLock::new();
static UPDATE: OnceLock<hook::Hook<UpdateNodesAfterRenderFn>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PLAYER: AtomicPtr<skre::TESObjectREFR> = AtomicPtr::new(core::ptr::null_mut());
static HEAD: AtomicPtr<skre::NiAVObject> = AtomicPtr::new(core::ptr::null_mut());
static POSE_WRITES: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static POSE_WRITES_OTHER: AtomicU32 = AtomicU32::new(0);
static UPDATES: AtomicU32 = AtomicU32::new(0);
static LOCAL_MATCHES: AtomicU32 = AtomicU32::new(0);
static LOCAL_DIFFERS: AtomicU32 = AtomicU32::new(0);
static WORLD_CHANGED: AtomicU32 = AtomicU32::new(0);
static MAX_SPEED_BITS: AtomicU32 = AtomicU32::new(0);
static LAST_POSE: Mutex<Option<(Transform, HkQsTransform)>> = Mutex::new(None);
static TAGS: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());
static REGISTRATION: Mutex<Option<Registration<BSAnimationGraphEvent>>> = Mutex::new(None);

struct TagCounter;

impl EventSink<BSAnimationGraphEvent> for TagCounter {
    fn process(&self, event: &BSAnimationGraphEvent, _: &BSTEventSource<BSAnimationGraphEvent>) -> BSEventNotifyControl {
        let tag = event.tag().to_str().unwrap_or("?").to_owned();
        let owner_is_player = event.owner().is_some_and(|o| core::ptr::eq(o.as_ptr(), PLAYER.load(Ordering::Relaxed)));
        let mut tags = TAGS.lock().unwrap();
        if tags.len() < 64 || tags.contains_key(&tag) {
            *tags.entry(if owner_is_player { tag } else { format!("{tag} (other owner)") }).or_insert(0) += 1;
        }
        BSEventNotifyControl::Continue
    }
}

static SINK: Sink<BSAnimationGraphEvent, TagCounter> = Sink::new(TagCounter);

fn head_index(graph: &AnimationGraph) -> Option<usize> {
    let head = HEAD.load(Ordering::Relaxed);
    graph.bone_nodes().iter().position(|e| core::ptr::eq(e.node, head))
}

unsafe extern "C" fn apply_detour(pose: *const HkQsTransform, nodes: *mut core::ffi::c_void, count: i32) {
    unsafe { APPLY.get().expect("stored before enabling").original()(pose, nodes, count) }
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Some(graph) = (unsafe { AnimationGraph::from_bone_array(nodes) }) else { return };
    let player = PLAYER.load(Ordering::Relaxed);
    if !graph.owner().is_some_and(|o| core::ptr::eq(o.as_ptr(), player)) {
        POSE_WRITES_OTHER.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Some(player) = (unsafe { player.cast::<PlayerCharacter>().as_ref() }) else { return };
    let index = usize::from(!core::ptr::eq(graph.root_node().map_or(core::ptr::null(), |r| r.raw()), player.third_person_root().map_or(core::ptr::null(), |r| r.raw())));
    POSE_WRITES[index].fetch_add(1, Ordering::Relaxed);
    if index == 0 {
        MAX_SPEED_BITS.fetch_max(current_speed(player).max(0.0).to_bits(), Ordering::Relaxed);
        if let Some(i) = head_index(graph)
            && let (Some(node), Some(entry)) = (graph.bone_node(i), graph.pose_local().get(i))
        {
            *LAST_POSE.lock().unwrap() = Some((node.local(), *entry));
        }
    }
}

unsafe extern "C" fn update_detour(player: *mut skre::PlayerCharacter) {
    let head = unsafe { NiAVObject::from_ptr(HEAD.load(Ordering::Relaxed)) };
    let before = head.map(|h| (h.local(), h.world()));
    unsafe { UPDATE.get().expect("stored before enabling").original()(player) }
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    UPDATES.fetch_add(1, Ordering::Relaxed);
    if let (Some(h), Some((local, world))) = (head, before) {
        let matches = LAST_POSE.lock().unwrap().as_ref().is_some_and(|(written, _)| *written == local);
        if matches { &LOCAL_MATCHES } else { &LOCAL_DIFFERS }.fetch_add(1, Ordering::Relaxed);
        if h.world() != world {
            WORLD_CHANGED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Creates both hooks, stores them and enables them; called once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    let apply = unsafe { skyrim::animation::hook_apply_pose(apply_detour) }.map_err(|e| format!("ApplyPoseToBoneNodes: {e}"))?;
    let update = unsafe { skyrim::animation::hook_update_nodes_after_render(update_detour) }.map_err(|e| format!("UpdateNodesAfterRender: {e}"))?;
    let (apply_target, update_target) = (apply.target(), update.target());
    let _ = APPLY.set(apply);
    let _ = UPDATE.set(update);
    APPLY.get().expect("just set").enable().map_err(|e| format!("enable ApplyPoseToBoneNodes: {e}"))?;
    UPDATE.get().expect("just set").enable().map_err(|e| format!("enable UpdateNodesAfterRender: {e}"))?;
    Ok(format!("ApplyPoseToBoneNodes at {apply_target:#x}, PlayerCharacter::UpdateNodesAfterRender at {update_target:#x}"))
}

/// Stops sampling; called at `PreLoadGame` and `NewGame`.
pub fn suspend() {
    ACTIVE.store(false, Ordering::Release);
    *REGISTRATION.lock().unwrap() = None;
}

/// Caches the player and its head bone, registers the sink on graph 0, clears the counters and
/// starts sampling; called after a load.
pub fn register() -> Result<String, String> {
    let player = PlayerCharacter::get().ok_or("g_PlayerCharacter_ptr is null")?;
    PLAYER.store(core::ptr::from_ref::<skyrim::actors::TESObjectREFR>(player).cast::<skre::TESObjectREFR>().cast_mut(), Ordering::Relaxed);
    let root = player.third_person_root().ok_or("loadedData->node3D is null")?;
    let head = root.object_by_name(&BSFixedString::new("NPC Head [Head]")).ok_or("GetObjectByName(NPC Head [Head]) returned null")?;
    HEAD.store(head.as_ptr().cast_mut(), Ordering::Relaxed);
    for c in POSE_WRITES.iter().chain([&POSE_WRITES_OTHER, &UPDATES, &LOCAL_MATCHES, &LOCAL_DIFFERS, &WORLD_CHANGED, &MAX_SPEED_BITS]) {
        c.store(0, Ordering::Relaxed);
    }
    *LAST_POSE.lock().unwrap() = None;
    TAGS.lock().unwrap().clear();
    let source = graph_event_source(player, Some(0)).ok_or("GetAnimationGraphEventSource(player, 0) failed")?;
    let graph_source = graph_manager(player).and_then(|m| m.graphs().first().copied().flatten()).and_then(AnimationGraph::event_source);
    check(graph_source.is_some_and(|g| core::ptr::eq(g, source)), "GetAnimationGraphEventSource(0) is not graph 0's +0x68")?;
    let sinks_before = source.sink_count();
    *REGISTRATION.lock().unwrap() = Some(source.add_sink(&SINK));
    ACTIVE.store(true, Ordering::Release);
    Ok(format!("head bone {:p} ({}), graph 0 source {:p} sinks {} -> {}", head.as_ptr(), head.name().to_str().unwrap_or("?"), source, sinks_before, source.sink_count()))
}

fn player() -> Result<&'static PlayerCharacter, String> {
    PlayerCharacter::get().ok_or_else(|| "g_PlayerCharacter_ptr is null".to_owned())
}

fn graphs() -> Result<String, String> {
    let p = player()?;
    let manager = graph_manager(p).ok_or("no animation graph manager")?;
    let graphs = manager.graphs();
    check(graphs.len() == 2, &format!("{} graphs", graphs.len()))?;
    let active = manager.active_graph_index();
    check((active == 1) != p.is_third_person(), &format!("active graph {active} with IsThirdPerson {}", p.is_third_person()))?;
    let mut described = Vec::new();
    for (i, g) in graphs.iter().enumerate() {
        let g = g.ok_or_else(|| format!("graph {i} is null"))?;
        check(g.owner().is_some_and(|o| core::ptr::eq(o.as_ptr(), p.as_ptr().cast())), &format!("graph {i} is not owned by the player"))?;
        let bones = g.bone_count();
        let nodes = g.bone_nodes().len();
        let pose = g.pose_local().len();
        let skeleton = g.skeleton_bone_count().unwrap_or(0);
        check(bones == 99 && nodes == 99 && pose == 99 && skeleton == 99, &format!("graph {i}: bones {bones}, nodes {nodes}, pose {pose}, skeleton {skeleton}"))?;
        let expected_root = if i == 0 { p.third_person_root() } else { p.first_person_root() };
        check(g.root_node().zip(expected_root).is_some_and(|(a, b)| core::ptr::eq(a.raw(), b.raw())), &format!("graph {i} does not drive the expected root"))?;
        described.push(format!("graph {i} {:?} at {:p}: 99 bones, root {:p}, source sinks {}", g.project_name().to_str().unwrap_or("?"), g.as_ptr(), g.root_node().map_or(core::ptr::null(), |r| r.raw()), g.event_source().map_or(0, BSTEventSource::sink_count)));
    }
    Ok(format!("2 graphs, active {active}; {}", described.join("; ")))
}

fn head_bone() -> Result<String, String> {
    let p = player()?;
    let graph = graph_manager(p).and_then(|m| m.graphs().first().copied().flatten()).ok_or("no graph 0")?;
    let i = head_index(graph).ok_or("the head bone is not in graph 0's bone array")?;
    let head = unsafe { NiAVObject::from_ptr(HEAD.load(Ordering::Relaxed)) }.ok_or("no head")?;
    check(head.name().to_str() == Some("NPC Head [Head]"), "the cached head has another name")?;
    let local = head.local();
    let pose = graph.pose_local()[i];
    Ok(format!("NPC Head [Head] is skeleton bone {i}; local translation {:?}, pose translation {:?}, pose rotation {:?}", local.translation, &pose.translation[..3], pose.rotation))
}

fn variables() -> Result<String, String> {
    let p = player()?;
    let holder = AnimationGraphHolder::of(p);
    let speed = holder.float(&BSFixedString::new("Speed")).ok_or("GetGraphVariableFloat(Speed) failed")?;
    let attacking = holder.bool(&BSFixedString::new("IsAttacking")).ok_or("GetGraphVariableBool(IsAttacking) failed")?;
    let direct = current_speed(p);
    let max_seen = f32::from_bits(MAX_SPEED_BITS.load(Ordering::Relaxed));
    check(!attacking, "IsAttacking is true")?;
    check((speed - direct).abs() < 1.0, &format!("Speed variable {speed} vs GetCurrentSpeed {direct}"))?;
    check(max_seen > 100.0, &format!("the highest speed seen during the walk was {max_seen}"))?;
    Ok(format!("Speed {speed} (GetCurrentSpeed {direct}), IsAttacking {attacking}, highest Speed during the run {max_seen}"))
}

fn weapon_state() -> Result<String, String> {
    let p = player()?;
    let state = p.state().weapon_state();
    check(state == 3, &format!("weapon state {state} after DrawWeapon"))?;
    Ok(format!("weapon state {state} (drawn)"))
}

fn events() -> Result<String, String> {
    let tags = TAGS.lock().unwrap().clone();
    let count = |t: &str| tags.get(t).copied().unwrap_or(0);
    check(count("FootLeft") >= 2 && count("FootRight") >= 2, &format!("footsteps: left {} right {}", count("FootLeft"), count("FootRight")))?;
    check(count("BeginWeaponDraw") >= 1 && count("weaponDraw") >= 1, &format!("weapon draw: BeginWeaponDraw {} weaponDraw {}", count("BeginWeaponDraw"), count("weaponDraw")))?;
    let listed: Vec<String> = tags.iter().map(|(t, n)| format!("{t} x{n}")).collect();
    Ok(format!("{} distinct tags on graph 0: {}", tags.len(), listed.join(", ")))
}

fn pose_write() -> Result<String, String> {
    let [third, first] = [POSE_WRITES[0].load(Ordering::Relaxed), POSE_WRITES[1].load(Ordering::Relaxed)];
    let other = POSE_WRITES_OTHER.load(Ordering::Relaxed);
    check(third > 100 && first > 100, &format!("pose writes: third-person graph {third}, first-person graph {first}"))?;
    let last = *LAST_POSE.lock().unwrap();
    let (local, pose) = last.ok_or("no head pose sample")?;
    check((0..3).all(|k| (local.translation[k] - pose.translation[k]).abs() < 0.01), &format!("the head's local translation {:?} differs from the pose's {:?}", local.translation, &pose.translation[..3]))?;
    Ok(format!("pose writes: third-person graph {third}, first-person graph {first}, other actors {other}; last head local translation {:?} = pose", local.translation))
}

fn world_update() -> Result<String, String> {
    let updates = UPDATES.load(Ordering::Relaxed);
    let matches = LOCAL_MATCHES.load(Ordering::Relaxed);
    let differs = LOCAL_DIFFERS.load(Ordering::Relaxed);
    let changed = WORLD_CHANGED.load(Ordering::Relaxed);
    check(updates > 100, &format!("UpdateNodesAfterRender ran {updates} times"))?;
    check(matches * 10 >= (matches + differs) * 9, &format!("the head's local at the world update matched the last pose write in {matches} of {} frames", matches + differs))?;
    check(changed > 10, &format!("the head's world transform changed across the update in {changed} frames"))?;
    Ok(format!("{updates} world updates: head local = last pose write in {matches} (differs {differs}), world changed in {changed}"))
}
