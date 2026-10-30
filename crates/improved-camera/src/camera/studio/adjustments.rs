//! `camera/studio/StudioAdjustments`: layered bone edits re-asserted every frame over the
//! animation. A layer holds per-bone deltas (position added in parent space, rotation
//! post-composed in the bone's local frame from XYZ Euler degrees, scale multiplied); an actor's
//! stack applies its layers in order over the animation's locals, captured each frame, and hands a
//! bone back to the animation when its edit goes.

use std::collections::{BTreeMap, HashMap};

use skyrim::actors::{PlayerCharacterExt, RefHandle};
use skyrim::forms::TESFormExt;
use skyrim::render::{NiAVObjectExt, Transform};
use sre::{NiAVObject, PlayerCharacter, TESObjectREFR};

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

fn axis_rotation(axis: usize, angle: f32) -> M3 {
    let (s, c) = angle.sin_cos();
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut m = point::IDENTITY;
    m[i][i] = c;
    m[i][j] = -s;
    m[j][i] = s;
    m[j][j] = c;
    m
}

/// `NiMatrix3::SetEulerAnglesXYZ`: X, then Y, then Z, each turning as `MakeZRotation` does.
#[must_use]
pub fn euler_xyz(x: f32, y: f32, z: f32) -> M3 {
    point::mul(&point::mul(&axis_rotation(0, x), &axis_rotation(1, y)), &axis_rotation(2, z))
}

/// `ExtractEulerXYZ`: the angles [`euler_xyz`] builds `m` from (radians); at Y = ±90 degrees Z is 0.
#[must_use]
pub fn extract_euler_xyz(m: &M3) -> P3 {
    let y = m[0][2].clamp(-1.0, 1.0).asin();
    if m[0][2].abs() < 0.999_999 {
        [(-m[1][2]).atan2(m[2][2]), y, (-m[0][1]).atan2(m[0][0])]
    } else {
        [m[2][1].atan2(m[1][1]), y, 0.0]
    }
}

/// `RotationFromEulerDegrees`.
#[must_use]
pub fn rotation_from_degrees(degrees: P3) -> M3 {
    euler_xyz(degrees[0].to_radians(), degrees[1].to_radians(), degrees[2].to_radians())
}

/// `EulerDegreesFromRotation`, the inverse of [`rotation_from_degrees`].
#[must_use]
pub fn degrees_from_rotation(m: &M3) -> P3 {
    extract_euler_xyz(m).map(f32::to_degrees)
}

/// `NiMatrix3::MakeRotation`: `angle` radians about the unit `axis`, turning as [`euler_xyz`] does.
#[must_use]
pub fn axis_angle(angle: f32, axis: P3) -> M3 {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [[t * x * x + c, t * x * y - s * z, t * x * z + s * y], [t * x * y + s * z, t * y * y + c, t * y * z - s * x], [t * x * z - s * y, t * y * z + s * x, t * z * z + c]]
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
    pub form_id: u32,
    handle: RefHandle,
    pub stack: Vec<Adjustment>,
    states: HashMap<String, BoneState>,
}

impl ActorAdjustments {
    #[must_use]
    pub fn new(form_id: u32, handle: RefHandle) -> Self {
        Self { form_id, handle, stack: Vec::new(), states: HashMap::new() }
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
        let index = match self.stack.iter().position(|a| a.is_pose) {
            Some(i) => i,
            None => {
                self.stack.insert(0, Adjustment { is_pose: true, ..Adjustment::default() });
                0
            }
        };
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

/// `PoseEngine`: the stacks of every actor the studio touched, by form id.
#[derive(Clone, Debug, Default)]
pub struct PoseEngine {
    pub actors: BTreeMap<u32, ActorAdjustments>,
}

impl PoseEngine {
    pub fn get_or_create(&mut self, reference: &TESObjectREFR) -> &mut ActorAdjustments {
        let id = reference.formID;
        self.actors.entry(id).or_insert_with(|| {
            crate::log(&format!("<Photo Mode> Pose stack created for {id:08X}"));
            ActorAdjustments::new(id, skyrim::actors::TESObjectREFRExt::handle(reference))
        })
    }

    /// `OnGameUpdate`'s apply sweep: a gone actor whose layers would all come back on their own
    /// is dropped, the rest apply.
    pub fn on_game_update(&mut self, apply_all: bool) {
        self.actors.retain(|_, a| a.handle.lookup().is_some() || !a.stack.iter().all(|l| l.persistent && !l.file.is_empty()));
        for adjustments in self.actors.values_mut() {
            adjustments.apply(apply_all);
        }
    }
}

#[cfg(test)]
mod tests {
    use skyrim::render::Transform;

    use super::{BoneDelta, BoneState, axis_angle, compose, degrees_from_rotation, euler_xyz, extract_euler_xyz, pose_delta, rotation_from_degrees};
    use crate::util::point::{self, IDENTITY};

    fn close(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-5))
    }

    #[test]
    fn euler_angles_round_trip_and_z_turns_as_make_z_rotation() {
        for angles in [[0.3, -0.7, 1.2], [-2.0, 0.4, -0.1], [0.0, 0.0, 0.0], [1.0, 1.4, -3.0]] {
            let m = euler_xyz(angles[0], angles[1], angles[2]);
            let back = extract_euler_xyz(&m);
            assert!(close(&euler_xyz(back[0], back[1], back[2]), &m), "{angles:?} -> {back:?}");
        }
        assert!(close(&euler_xyz(0.0, 0.0, 0.5), &point::z_rotation(0.5)));
        let gimbal = euler_xyz(0.4, core::f32::consts::FRAC_PI_2, 0.0);
        let back = extract_euler_xyz(&gimbal);
        assert!(close(&euler_xyz(back[0], back[1], back[2]), &gimbal) && back[2] == 0.0);
        let d = degrees_from_rotation(&rotation_from_degrees([10.0, 20.0, 30.0]));
        assert!(d.iter().zip([10.0, 20.0, 30.0]).all(|(a, b)| (a - b).abs() < 1e-3), "{d:?}");
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
