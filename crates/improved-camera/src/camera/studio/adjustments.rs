//! `camera/studio/StudioAdjustments`: layered bone edits re-asserted every frame over the
//! animation. A layer holds per-bone deltas (position added in parent space, rotation
//! post-composed in the bone's local frame from XYZ Euler degrees, scale multiplied); an actor's
//! stack applies its layers in order over the animation's locals, captured each frame, and hands a
//! bone back to the animation when its edit goes.

use std::collections::{BTreeMap, HashMap};

use skyrim::actors::{ActorExt, PlayerCharacterExt, RefHandle, TESObjectREFRExt};
use skyrim::forms::TESFormExt;
use skyrim::render::{NiAVObjectExt, Transform, from_euler_xyz, to_euler_xyz};
use sre::{Actor, ActorState_flags1, NiAVObject, PlayerCharacter, TESNPC, TESObjectREFR};

use super::io::{self, Persistent};
use crate::util::point::{self, M3, P3};
use crate::util::skyrim_util as util;

const EPSILON: f32 = 0.0001;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneDelta {
    pub position: P3,
    /// Degrees, `SetEulerAnglesXYZ`'s order.
    pub rotation: P3,
    pub scale: f32,
}

impl Default for BoneDelta {
    fn default() -> Self {
        Self { position: [0.0; 3], rotation: [0.0; 3], scale: 1.0 }
    }
}

impl BoneDelta {
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// `RotationFromEulerDegrees`.
#[must_use]
pub fn rotation_from_degrees(degrees: P3) -> M3 {
    from_euler_xyz(degrees[0].to_radians(), degrees[1].to_radians(), degrees[2].to_radians())
}

/// `EulerDegreesFromRotation`, the inverse of [`rotation_from_degrees`].
#[must_use]
pub fn degrees_from_rotation(m: &M3) -> P3 {
    to_euler_xyz(m).map(f32::to_degrees)
}

/// `NiMatrix3::MakeRotation`: `angle` radians about the unit `axis`, turning as the engine's
/// elementary turns do (counter-clockwise about the negative axis).
#[must_use]
pub fn axis_angle(angle: f32, axis: P3) -> M3 {
    let (sin, cos) = (-angle).sin_cos();
    let k = 1.0 - cos;
    let [ax, ay, az] = axis;
    [[k * ax * ax + cos, k * ax * ay - sin * az, k * ax * az + sin * ay], [k * ax * ay + sin * az, k * ay * ay + cos, k * ay * az - sin * ax], [k * ax * az - sin * ay, k * ay * az + sin * ax, k * az * az + cos]]
}

/// `ComposeBoneDelta`: one delta over a local transform.
#[must_use]
pub fn compose(local: &Transform, delta: &BoneDelta) -> Transform {
    Transform { rotation: point::mul(&local.rotation, &rotation_from_degrees(delta.rotation)), translation: point::add(local.translation, delta.position), scale: local.scale * delta.scale }
}

fn transforms_equal(a: &Transform, b: &Transform) -> bool {
    (a.scale - b.scale).abs() <= EPSILON
        && (0..3).all(|i| (a.translation[i] - b.translation[i]).abs() <= EPSILON && (0..3).all(|j| (a.rotation[i][j] - b.rotation[i][j]).abs() <= EPSILON))
}

/// `ImportPose`'s delta: what turns the animation's local `anim` into the pose file's `file`, or
/// `None` below the noise IC drops.
#[must_use]
pub fn pose_delta(anim: &Transform, file: &Transform) -> Option<BoneDelta> {
    let delta = BoneDelta {
        rotation: degrees_from_rotation(&point::mul(&point::transpose(&anim.rotation), &file.rotation)),
        position: point::sub(file.translation, anim.translation),
        scale: if anim.scale > EPSILON { file.scale / anim.scale } else { 1.0 },
    };
    let near = delta.position.iter().all(|p| p.abs() < 0.001) && delta.rotation.iter().all(|r| r.abs() < 0.01) && (delta.scale - 1.0).abs() < 0.001;
    (!near).then_some(delta)
}

/// A named layer of bone edits.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Adjustment {
    pub name: String,
    /// The file it was saved to or loaded from, if any.
    pub file: String,
    pub enabled: bool,
    /// The one layer a loaded pose occupies.
    pub is_pose: bool,
    /// Applies outside the studio too (a skeleton adjustment).
    pub persistent: bool,
    /// Baked into exported pose files.
    pub include_in_pose: bool,
    pub bones: BTreeMap<String, BoneDelta>,
}

impl Default for Adjustment {
    fn default() -> Self {
        Self { name: "New Adjustment".to_owned(), file: String::new(), enabled: true, is_pose: false, persistent: false, include_in_pose: true, bones: BTreeMap::new() }
    }
}

#[derive(Clone, Copy, Debug)]
struct BoneState {
    anim_local: Transform,
    last_written: Transform,
}

impl BoneState {
    /// The animation's fresh local when it wrote one since our last write; the graph never writes
    /// scale, so a scale still equal to ours keeps the captured one.
    fn capture(&mut self, local: &Transform) {
        if transforms_equal(local, &self.last_written) {
            return;
        }
        let scale = if (local.scale - self.last_written.scale).abs() <= EPSILON { self.anim_local.scale } else { local.scale };
        self.anim_local = Transform { scale, ..*local };
    }

    /// What the bone goes back to once its edit is gone: the captured local when nothing wrote
    /// over ours, else only the scale when that is still ours; `None` to leave it.
    fn release(&self, local: &Transform) -> Option<Transform> {
        if transforms_equal(local, &self.last_written) {
            Some(self.anim_local)
        } else if (local.scale - self.last_written.scale).abs() <= EPSILON {
            Some(Transform { scale: self.anim_local.scale, ..*local })
        } else {
            None
        }
    }
}

/// The third-person skeleton of a reference: the player's third-person root whatever the camera.
#[must_use]
pub fn third_person_3d(reference: &TESObjectREFR) -> Option<&'static NiAVObject> {
    match reference.cast::<PlayerCharacter>() {
        Some(player) => player.third_person_root().map(|r| unsafe { &*core::ptr::from_ref(r) }),
        None => reference.get3_d().map(|n| &**n),
    }
}

/// An actor's stack of layers and the per-bone capture that lets them re-assert every frame.
#[derive(Clone, Debug)]
pub struct ActorAdjustments {
    handle: RefHandle,
    pub stack: Vec<Adjustment>,
    states: HashMap<String, BoneState>,
}

impl ActorAdjustments {
    #[must_use]
    pub fn new(handle: RefHandle) -> Self {
        Self { handle, stack: Vec::new(), states: HashMap::new() }
    }

    #[must_use]
    pub fn root(&self) -> Option<&'static NiAVObject> {
        self.handle.lookup().and_then(|r| third_person_3d(&r))
    }

    pub fn new_adjustment(&mut self) -> &mut Adjustment {
        let name = format!("Adjustment {}", self.stack.len() + 1);
        self.stack.push(Adjustment { name, ..Adjustment::default() });
        self.stack.last_mut().expect("just pushed")
    }

    pub fn delete_adjustment(&mut self, index: usize) {
        if index < self.stack.len() {
            self.stack.remove(index);
        }
    }

    pub fn move_adjustment(&mut self, index: usize, up: bool) {
        let other = if up { index.checked_sub(1) } else { Some(index + 1) };
        if let Some(other) = other.filter(|o| *o < self.stack.len() && index < self.stack.len()) {
            self.stack.swap(index, other);
        }
    }

    /// `EnsurePoseLayer`: the pose layer, created at the bottom of the stack.
    fn pose_layer(&mut self) -> &mut Adjustment {
        let index = self.stack.iter().position(|a| a.is_pose).unwrap_or_else(|| {
            self.stack.insert(0, Adjustment { is_pose: true, ..Adjustment::default() });
            0
        });
        &mut self.stack[index]
    }

    /// `InstallPose`: ready-made deltas (a SAM pose) in the pose layer.
    pub fn install_pose(&mut self, name: &str, bones: BTreeMap<String, BoneDelta>) {
        let layer = self.pose_layer();
        layer.name = format!("Pose: {name}");
        layer.enabled = true;
        layer.bones = bones;
    }

    /// `ImportPose`: a pose file's absolute locals as deltas against the captured animation locals,
    /// in the pose layer. Returns how many bones differ, `None` without 3D.
    pub fn import_pose(&mut self, name: &str, pose: &BTreeMap<String, Transform>) -> Option<usize> {
        let root = self.root()?;
        let mut bones = BTreeMap::new();
        for (bone, file) in pose {
            let Some(node) = util::object_by_name(root, bone) else { continue };
            let anim = self.states.get(bone).map_or_else(|| node.local_transform(), |s| s.anim_local);
            if let Some(delta) = pose_delta(&anim, file) {
                bones.insert(bone.clone(), delta);
            }
        }
        let count = bones.len();
        self.install_pose(name, bones);
        Some(count)
    }

    fn edits(&self, layer: impl Fn(&Adjustment) -> bool) -> BTreeMap<String, Vec<BoneDelta>> {
        let mut edits: BTreeMap<String, Vec<BoneDelta>> = BTreeMap::new();
        for adjustment in self.stack.iter().filter(|a| a.enabled && layer(a)) {
            for (name, delta) in adjustment.bones.iter().filter(|(_, d)| !d.is_identity()) {
                edits.entry(name.clone()).or_default().push(*delta);
            }
        }
        edits
    }

    /// `Apply`, once a frame on the main thread: the enabled layers (outside the studio only the
    /// persistent ones) over freshly captured animation locals, the bones whose edits went handed
    /// back, then one world update.
    pub fn apply(&mut self, apply_all: bool) {
        let Some(root) = self.root() else {
            self.states.clear();
            return;
        };
        let edits = self.edits(|a| apply_all || a.persistent);
        let mut dirty = false;
        for (name, deltas) in &edits {
            let Some(node) = util::object_by_name(root, name) else { continue };
            let local = node.local_transform();
            let state = self.states.entry(name.clone()).or_insert(BoneState { anim_local: local, last_written: local });
            state.capture(&local);
            let result = deltas.iter().fold(state.anim_local, |t, d| compose(&t, d));
            node.set_local_transform(result);
            state.last_written = result;
            dirty = true;
        }
        self.states.retain(|name, state| {
            if edits.contains_key(name) {
                return true;
            }
            if let Some(node) = util::object_by_name(root, name)
                && let Some(local) = state.release(&node.local_transform())
            {
                node.set_local_transform(local);
                dirty = true;
            }
            false
        });
        if dirty {
            root.update_transforms();
        }
    }

    /// `RevertAll`: every touched bone back to its captured animation local.
    pub fn revert_all(&mut self) {
        if let Some(root) = self.root() {
            let mut dirty = false;
            for (name, state) in &self.states {
                if let Some(node) = util::object_by_name(root, name)
                    && let Some(local) = state.release(&node.local_transform())
                {
                    node.set_local_transform(local);
                    dirty = true;
                }
            }
            if dirty {
                root.update_transforms();
            }
        }
        self.states.clear();
    }

    /// `AppendPoseLocals`: the final locals of the layers marked for pose export, over the captured
    /// animation locals; touched bones they leave out export their animation local.
    pub fn pose_locals(&self, out: &mut BTreeMap<String, Transform>) {
        let edits = self.edits(|a| a.include_in_pose);
        for (name, state) in self.states.iter().filter(|(n, _)| !edits.contains_key(*n)) {
            out.insert(name.clone(), state.anim_local);
        }
        let root = self.root();
        for (name, deltas) in edits {
            let base = match self.states.get(&name) {
                Some(state) => state.anim_local,
                None => match root.and_then(|r| util::object_by_name(r, &name)) {
                    Some(node) => node.local_transform(),
                    None => continue,
                },
            };
            out.insert(name, deltas.iter().fold(base, |t, d| compose(&t, d)));
        }
    }
}

/// Frames a moving actor gets to blend back to a stand before its freeze engages (~0.75 s).
const SETTLE_TICKS: u32 = 45;

/// `PoseEngine`: the stacks of every actor the studio touched, by form id, and the freeze intents.
#[derive(Clone, Debug, Default)]
pub struct PoseEngine {
    pub actors: BTreeMap<u32, ActorAdjustments>,
    frozen: BTreeMap<u32, RefHandle>,
    /// Freezes staged for moving actors: AI stopped, the pause engages once settled.
    pending_freeze: BTreeMap<u32, (RefHandle, u32)>,
    /// Unfreezes retried until the actor resolves.
    pending_unfreeze: BTreeMap<u32, RefHandle>,
    /// `Persistent.json`, read on first use.
    persistent: Option<Persistent>,
}

/// `ActorKey`: the actor's plugin and local id; empty for one created at runtime, which has no
/// file and no id that survives a save.
#[must_use]
pub fn actor_key(actor: &Actor) -> String {
    super::environment::form_key(actor.formID)
}

/// `RaceKey`: the race's editor id (races keep theirs on this runtime) and the base's sex.
#[must_use]
pub fn race_key(actor: &Actor) -> String {
    let (Some(race), Some(npc)) = (actor.race(), actor.base_object().and_then(|b| b.cast::<TESNPC>())) else { return String::new() };
    let editor_id = race.editor_id().map_or_else(|| "UnknownRace".to_owned(), |id| id.to_string_lossy().into_owned());
    format!("{editor_id}|{}", if npc.get_sex() == 1 { "F" } else { "M" })
}

fn is_moving(actor: &Actor) -> bool {
    let flags = ActorState_flags1(actor.as_actor_state().flags1);
    [ActorState_flags1::WALKING, ActorState_flags1::RUNNING, ActorState_flags1::SPRINTING, ActorState_flags1::SWIMMING].into_iter().any(|f| flags.contains(f))
}

fn with_actor(handle: RefHandle, f: impl FnOnce(&Actor)) -> bool {
    handle.lookup().is_some_and(|r| r.cast::<Actor>().map(f).is_some())
}

/// `WriteAnimGraphFlag` and `EnableAI`: the graph paused or running, the AI with it (never the
/// player's).
fn hold(actor: &Actor, frozen: bool) {
    actor.set_graph_paused(frozen);
    if actor.cast::<PlayerCharacter>().is_none() {
        actor.set_ai_enabled(!frozen);
    }
}

impl PoseEngine {
    /// `SetActorFrozen`: an intent the game tick carries out; a moving actor's freeze is staged.
    pub fn set_actor_frozen(&mut self, actor: &Actor, frozen: bool) {
        let id = actor.formID;
        if frozen {
            self.pending_unfreeze.remove(&id);
            if is_moving(actor) {
                self.pending_freeze.insert(id, (actor.handle(), SETTLE_TICKS));
                crate::log(&format!("<Photo Mode> Actor {id:08X} freeze staged (settling)"));
            } else {
                self.frozen.entry(id).or_insert_with(|| actor.handle());
                crate::log(&format!("<Photo Mode> Actor {id:08X} frozen"));
            }
        } else if self.frozen.remove(&id).is_some() || self.pending_freeze.remove(&id).is_some() {
            self.pending_unfreeze.insert(id, actor.handle());
        }
    }

    #[must_use]
    pub fn is_actor_frozen(&self, id: u32) -> bool {
        self.frozen.contains_key(&id) || self.pending_freeze.contains_key(&id)
    }

    /// `FrozenActors`: staged freezes count too.
    #[must_use]
    pub fn frozen_actors(&self) -> Vec<u32> {
        self.frozen.keys().chain(self.pending_freeze.keys().filter(|id| !self.frozen.contains_key(id))).copied().collect()
    }

    /// `UnfreezeAll`: through the pending set, so an unresolvable actor keeps retrying.
    pub fn unfreeze_all(&mut self) {
        crate::log(&format!("<Photo Mode> UnfreezeAll: {} actors", self.frozen.len() + self.pending_freeze.len()));
        self.pending_unfreeze.extend(core::mem::take(&mut self.frozen));
        self.pending_unfreeze.extend(core::mem::take(&mut self.pending_freeze).into_iter().map(|(id, (h, _))| (id, h)));
    }

    /// Every frozen actor resumed now, for an unload.
    pub fn thaw(&mut self) {
        self.unfreeze_all();
        self.update_freezes(false);
    }

    /// The freeze part of `OnGameUpdate`: unfreezes retried until they resolve, then (when
    /// everything applies) staged freezes settled and the pause re-asserted, since the engine keeps
    /// re-enabling graph updates.
    fn update_freezes(&mut self, apply_all: bool) {
        self.pending_unfreeze.retain(|id, handle| {
            let done = with_actor(*handle, |a| hold(a, false));
            if done {
                crate::log(&format!("<Photo Mode> Actor {id:08X} unfrozen"));
            }
            !done
        });
        if !apply_all {
            return;
        }
        let mut settled = Vec::new();
        for (id, (handle, ticks)) in &mut self.pending_freeze {
            let mut still_moving = false;
            if !with_actor(*handle, |a| {
                if a.cast::<PlayerCharacter>().is_none() {
                    a.set_ai_enabled(false);
                }
                still_moving = is_moving(a);
            }) {
                continue;
            }
            if *ticks == 0 || (!still_moving && *ticks < 20) {
                settled.push(*id);
            } else {
                *ticks -= 1;
            }
        }
        for id in settled {
            if let Some((handle, _)) = self.pending_freeze.remove(&id) {
                self.frozen.insert(id, handle);
                crate::log(&format!("<Photo Mode> Actor {id:08X} frozen (settled)"));
            }
        }
        for handle in self.frozen.values() {
            with_actor(*handle, |a| hold(a, true));
        }
    }

    pub fn get_or_create(&mut self, reference: &TESObjectREFR) -> &mut ActorAdjustments {
        let id = reference.formID;
        self.actors.entry(id).or_insert_with(|| {
            crate::log(&format!("<Photo Mode> Pose stack created for {id:08X}"));
            ActorAdjustments::new(reference.handle())
        })
    }

    /// `OnGameUpdate`'s apply sweep: a gone actor whose layers would all come back on their own
    /// is dropped, the rest apply.
    pub fn on_game_update(&mut self, apply_all: bool) {
        self.update_freezes(apply_all);
        self.actors.retain(|_, a| a.handle.lookup().is_some() || !a.stack.iter().all(|l| l.persistent && !l.file.is_empty()));
        for adjustments in self.actors.values_mut() {
            adjustments.apply(apply_all);
        }
    }

    /// `EnsurePersistentConfig`.
    fn config(&mut self) -> &mut Persistent {
        self.persistent.get_or_insert_with(|| {
            let config = io::load_persistent(&io::directory("")).unwrap_or_else(|e| {
                crate::log(&format!("<Photo Mode> Persistent config load failed: {e}"));
                Persistent::default()
            });
            crate::log(&format!("<Photo Mode> Persistent config: {} actor(s), {} race(s)", config.actors.len(), config.races.len()));
            config
        })
    }

    fn save_config(&mut self) {
        let config = self.config().clone();
        if let Err(e) = io::save_persistent(&io::directory(""), &config) {
            crate::log(&format!("<Photo Mode> {e}"));
        }
    }

    #[must_use]
    pub fn is_actor_persistent(&mut self, actor: &Actor, file: &str) -> bool {
        io::has_assignment(&self.config().actors, &actor_key(actor), file)
    }

    #[must_use]
    pub fn is_race_persistent(&mut self, actor: &Actor, file: &str) -> bool {
        io::has_assignment(&self.config().races, &race_key(actor), file)
    }

    /// `SetActorPersistent`: the file follows this actor through every load.
    pub fn set_actor_persistent(&mut self, actor: &Actor, file: &str, enable: bool) {
        io::toggle_assignment(&mut self.config().actors, &actor_key(actor), file, enable);
        self.save_config();
        self.sync_assignment(file, actor.formID);
    }

    /// `SetRacePersistent`: the file goes on every actor of this race and sex as it loads.
    pub fn set_race_persistent(&mut self, actor: &Actor, file: &str, enable: bool) {
        io::toggle_assignment(&mut self.config().races, &race_key(actor), file, enable);
        self.save_config();
        self.sync_assignment(file, actor.formID);
    }

    /// `SyncAssignment`: the stacks of the loaded actors follow the config at once. An assigned
    /// actor's copies become persistent (and leave pose exports); the edited actor keeps its layer,
    /// no longer persistent; elsewhere the auto-applied copies go and their bones are handed back.
    fn sync_assignment(&mut self, file: &str, keep: u32) {
        if file.is_empty() {
            return;
        }
        let config = self.config().clone();
        for (id, adjustments) in &mut self.actors {
            let Some(reference) = adjustments.handle.lookup() else { continue };
            let Some(actor) = reference.cast::<Actor>() else { continue };
            let assigned = io::has_assignment(&config.actors, &actor_key(actor), file) || io::has_assignment(&config.races, &race_key(actor), file);
            let copies = adjustments.stack.iter_mut().filter(|l| l.file == file);
            if assigned {
                for layer in copies {
                    layer.persistent = true;
                    layer.include_in_pose = false;
                }
            } else if *id == keep {
                copies.for_each(|layer| layer.persistent = false);
            } else {
                let before = adjustments.stack.len();
                adjustments.stack.retain(|l| !(l.file == file && l.persistent));
                if adjustments.stack.len() != before {
                    crate::log(&format!("<Photo Mode> Persistent adjustment '{file}' removed from {id:08X}"));
                }
            }
        }
    }

    /// `OnActorLoaded`'s first half: the persistent layers the config no longer covers come off
    /// (an actor away when its assignment went still carries one), and the assigned files not on
    /// the actor yet are returned to be read.
    pub fn persistent_pending(&mut self, actor: &Actor) -> Vec<String> {
        let config = self.config();
        let mut assigned: Vec<String> = config.actors.get(&actor_key(actor)).into_iter().chain(config.races.get(&race_key(actor))).flatten().cloned().collect();
        if let Some(existing) = self.actors.get_mut(&actor.formID) {
            let before = existing.stack.len();
            existing.stack.retain(|l| !l.persistent || assigned.contains(&l.file));
            if existing.stack.len() != before {
                crate::log(&format!("<Photo Mode> {} stale persistent layer(s) removed from {:08X} on load", before - existing.stack.len(), actor.formID));
            }
            assigned.retain(|f| !existing.stack.iter().any(|l| l.file == *f));
        }
        assigned
    }

    /// `OnActorLoaded`'s last half: the layers read, unless a layer of the same file got there meanwhile.
    pub fn add_persistent(&mut self, reference: &TESObjectREFR, layers: Vec<Adjustment>) {
        let id = reference.formID;
        let adjustments = self.get_or_create(reference);
        for layer in layers {
            if !adjustments.stack.iter().any(|l| l.file == layer.file) {
                crate::log(&format!("<Photo Mode> Persistent adjustment '{}' applied to {id:08X}", layer.file));
                adjustments.stack.push(layer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use skyrim::render::Transform;

    use skyrim::render::from_euler_xyz as euler_xyz;

    use super::{BoneDelta, BoneState, axis_angle, compose, degrees_from_rotation, pose_delta, rotation_from_degrees};
    use crate::util::point::{self, IDENTITY};

    fn close(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-5))
    }

    #[test]
    fn bone_degrees_go_through_the_engines_xyz() {
        let d = degrees_from_rotation(&rotation_from_degrees([10.0, 20.0, 30.0]));
        assert!(d.iter().zip([10.0, 20.0, 30.0]).all(|(a, b)| (a - b).abs() < 1e-3), "{d:?}");
        let (s, c) = 30f32.to_radians().sin_cos();
        assert!(close(&rotation_from_degrees([0.0, 0.0, 30.0]), &[[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]]), "Z(a) of IC8 O");
    }

    #[test]
    fn an_axis_angle_about_a_basis_axis_is_that_euler_turn() {
        assert!(close(&axis_angle(0.7, [1.0, 0.0, 0.0]), &euler_xyz(0.7, 0.0, 0.0)));
        assert!(close(&axis_angle(-0.2, [0.0, 1.0, 0.0]), &euler_xyz(0.0, -0.2, 0.0)));
        assert!(close(&axis_angle(1.1, [0.0, 0.0, 1.0]), &euler_xyz(0.0, 0.0, 1.1)));
    }

    #[test]
    fn deltas_compose_in_the_bones_frame_and_import_back() {
        let anim = Transform { rotation: euler_xyz(0.2, 0.1, -0.4), translation: [1.0, 2.0, 3.0], scale: 1.5 };
        let delta = BoneDelta { position: [0.5, 0.0, -1.0], rotation: [15.0, -30.0, 45.0], scale: 1.2 };
        let posed = compose(&anim, &delta);
        assert!(close(&posed.rotation, &point::mul(&anim.rotation, &rotation_from_degrees(delta.rotation))));
        assert_eq!(posed.translation, [1.5, 2.0, 2.0]);
        assert!((posed.scale - 1.8).abs() < 1e-6);
        let back = pose_delta(&anim, &posed).unwrap();
        assert!(back.position.iter().zip(delta.position).all(|(a, b)| (a - b).abs() < 1e-4));
        assert!(back.rotation.iter().zip(delta.rotation).all(|(a, b)| (a - b).abs() < 1e-3), "{back:?}");
        assert!((back.scale - 1.2).abs() < 1e-5);
        assert_eq!(pose_delta(&anim, &anim), None, "no delta below the noise");
        assert!(BoneDelta::default().is_identity());
    }

    #[test]
    fn the_capture_keeps_its_scale_and_hands_bones_back() {
        let anim = Transform { rotation: IDENTITY, translation: [0.0; 3], scale: 1.0 };
        let ours = Transform { scale: 2.0, translation: [1.0, 0.0, 0.0], ..anim };
        let mut state = BoneState { anim_local: anim, last_written: ours };
        state.capture(&ours);
        assert_eq!(state.anim_local, anim, "our own write is not the animation's");
        let animated = Transform { translation: [0.0, 5.0, 0.0], ..ours };
        state.capture(&animated);
        assert_eq!(state.anim_local, Transform { translation: [0.0, 5.0, 0.0], ..anim }, "the graph never writes scale");
        assert_eq!(state.release(&ours), Some(state.anim_local));
        assert_eq!(state.release(&animated).map(|t| t.scale), Some(1.0), "only the scale goes back");
        assert_eq!(state.release(&Transform { scale: 3.0, ..animated }), None);
    }
}
