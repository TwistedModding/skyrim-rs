//! `body/BodyBumper`: while the camera sits away from the player's own eyes, the player's convex
//! controller shape is widened by the profile's forward, back and side bumper offsets so the body
//! keeps clear of walls and doorways the camera would otherwise clip into; the werewolf, whose
//! controller is two capsules, has both lengthened and thickened by the werewolf offset instead.
//! The original goes back when the camera returns to first person, on unload and before a reload.
//! It runs on the main thread, where Havok start-up installs a memory router (RE `camera.md` IC9 E).

use std::sync::Mutex;

use skyrim::physics::HAVOK_SCALE;
use skyrim::physics::controller::{self, Capsule, ConvexBuild, Replacement, ShapeExt};
use sre::{PlayerCharacter, bhkCharacterController, hkpShape, hkpShapeType};

use crate::settings::Advanced;

/// IC's build options, as Variadic Collision Dynamics builds its hulls.
const BUILD: ConvexBuild = ConvexBuild {
    connectivity: true,
    shrink_by_radius: true,
    optimized_shrinking: true,
    convex_radius: 0.05,
    max_vertices: 0,
    max_relative_shrink: 0.0,
    max_shrink_displacement: 0.0,
    max_cos_bevel_angle: 0.999,
};

/// The most vertices IC caches.
const MAX_VERTICES: usize = 1024;
/// The material the werewolf's two controller capsules carry (RE `camera.md` IC9 E; the vampire
/// lord has the human convex shape and bumper capsule).
const WEREWOLF_MATERIAL: u32 = 0x5ed4_dfd3;

#[derive(Default)]
struct Bumper {
    /// The controller the original vertices belong to.
    controller: usize,
    /// The original hull's vertices.
    original: Vec<[f32; 4]>,
    replacement: Option<Replacement>,
    /// The werewolf's capsules while lengthened: each shape's address and its capsule as it was.
    capsules: Vec<(usize, Capsule)>,
}

// The replacement holds Havok references; it is built, checked and released on the main thread.
unsafe impl Send for Bumper {}

static BUMPER: Mutex<Option<Bumper>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Bumper) -> R) -> R {
    f(BUMPER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(Bumper::default))
}

/// `FindControllerConvexShape`: the root's first child when the root is a list, else the root.
fn controller_convex(root: &hkpShape) -> Option<&hkpShape> {
    let shape = match root.shape_type()? {
        hkpShapeType::List => unsafe { root.children().first()?.shape.as_ref() }?,
        _ => root,
    };
    (shape.shape_type()? == hkpShapeType::ConvexVertices).then_some(shape)
}

/// `FindWerewolfCapsules`: the root list's first two children, both capsules of the werewolf's material.
fn werewolf_capsules(root: &hkpShape) -> Option<[&hkpShape; 2]> {
    let [back, front, ..] = root.children() else { return None };
    let pair = [unsafe { back.shape.as_ref() }?, unsafe { front.shape.as_ref() }?];
    pair.iter().all(|s| s.material() == Some(WEREWOLF_MATERIAL)).then_some(pair)
}

/// `ModifyCapsule`: the first end point lowered, the second raised and the radius grown by `offset`.
#[must_use]
pub fn lengthened(capsule: Capsule, offset: f32) -> Capsule {
    let Capsule { mut a, mut b, radius } = capsule;
    a[2] -= offset;
    b[2] += offset;
    Capsule { a, b, radius: radius + offset }
}

/// `CreateModifiedConvexShape`'s vertices: stretched along y, the front by `forward` and the back
/// by `back` in proportion to where each vertex sits between the two, and pushed out sideways by
/// `side`. `None` for a hull with no depth.
#[must_use]
pub fn widened(vertices: &[[f32; 4]], forward: f32, back: f32, side: f32) -> Option<Vec<[f32; 4]>> {
    let (min_y, max_y) = vertices.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v[1]), hi.max(v[1])));
    let range = max_y - min_y;
    if range <= f32::EPSILON {
        return None;
    }
    Some(
        vertices
            .iter()
            .map(|&[x, y, z, w]| {
                let t = (y - min_y) / range;
                let x = if x > 0.0 {
                    x + side
                } else if x < 0.0 {
                    x - side
                } else {
                    x
                };
                [x, y + t * forward - (1.0 - t) * back, z, w]
            })
            .collect(),
    )
}

/// `BodyBumper::Update`: on while the camera is off the eyes, else the original shape back.
pub fn update(player: &PlayerCharacter, enabled: bool, advanced: &Advanced) {
    let Some(controller) = player.get_char_controller() else { return };
    with(|b| {
        if !enabled {
            b.restore(controller);
            return;
        }
        b.apply(controller, advanced);
    });
}

impl Bumper {
    fn apply(&mut self, controller: &bhkCharacterController, advanced: &Advanced) {
        let address = core::ptr::from_ref(controller) as usize;
        if self.controller != address {
            *self = Self { controller: address, ..Self::default() };
        }
        let Some(root) = controller::root_shape(controller) else { return };
        if let Some(pair) = werewolf_capsules(root) {
            self.replacement = None;
            let addresses = pair.map(|s| core::ptr::from_ref(s) as usize);
            if self.capsules.iter().map(|(a, _)| *a).ne(addresses) {
                let offset = advanced.fWerewolfBumperOffset * HAVOK_SCALE;
                self.capsules = pair.iter().filter_map(|s| Some((core::ptr::from_ref(*s) as usize, s.capsule()?))).collect();
                for (shape, (_, original)) in pair.iter().zip(&self.capsules) {
                    controller::set_capsule(controller, shape, lengthened(*original, offset));
                }
            }
            return;
        }
        self.capsules.clear();
        if self.replacement.as_ref().is_some_and(|r| r.is_current(controller)) {
            return;
        }
        self.replacement = None;
        let Some(convex) = controller_convex(root) else { return };
        if self.original.is_empty() {
            let vertices = convex.convex_vertices();
            if vertices.is_empty() || vertices.len() > MAX_VERTICES {
                return;
            }
            self.original = vertices;
        }
        let scale = |offset: f32| offset * HAVOK_SCALE;
        let Some(points) = widened(&self.original, scale(advanced.fForwardBumperOffset), scale(advanced.fBackBumperOffset), scale(advanced.fSideBumperOffset)) else { return };
        let Some(shape) = controller::build_convex(&points, &BUILD) else {
            crate::log("bumper: no Havok heap on this thread, the controller shape is unchanged");
            return;
        };
        self.replacement = controller::replace(controller, convex, shape);
    }

    fn restore(&mut self, controller: &bhkCharacterController) {
        if let Some(replacement) = self.replacement.take() {
            replacement.restore(controller);
        }
        let capsules = core::mem::take(&mut self.capsules);
        if let Some(pair) = controller::root_shape(controller).and_then(werewolf_capsules) {
            for shape in pair {
                if let Some((_, original)) = capsules.iter().find(|(a, _)| *a == core::ptr::from_ref(shape) as usize) {
                    controller::set_capsule(controller, shape, *original);
                }
            }
        }
    }
}

/// Puts the player's original controller shape back (unload, and before a reload: the new build
/// widens it again on its first frame away from the eyes).
pub fn release() {
    if let Some(controller) = crate::util::skyrim_util::player().and_then(|p| p.get_char_controller()) {
        with(|b| b.restore(controller));
    }
    *BUMPER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
mod tests {
    use skyrim::physics::controller::Capsule;

    use super::{lengthened, widened};

    #[test]
    fn the_werewolf_capsule_grows_at_both_ends_and_around() {
        let capsule = Capsule { a: [0.0, 0.25, -0.5], b: [0.0, 0.25, 0.5], radius: 0.375 };
        assert_eq!(lengthened(capsule, 0.25), Capsule { a: [0.0, 0.25, -0.75], b: [0.0, 0.25, 0.75], radius: 0.625 });
        assert_eq!(lengthened(capsule, 0.0), capsule);
    }

    #[test]
    fn the_hull_stretches_front_and_back_and_out_to_the_sides() {
        let hull = [[1.0, -1.0, 0.0, 0.0], [-1.0, 1.0, 2.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let out = widened(&hull, 0.5, 0.25, 0.1).unwrap();
        assert_eq!(out[0], [1.1, -1.25, 0.0, 0.0]);
        assert_eq!(out[1], [-1.1, 1.5, 2.0, 0.0]);
        assert_eq!(out[2], [0.0, 0.125, 1.0, 0.0]);
        assert_eq!(widened(&hull, 0.0, 0.0, 0.0).unwrap(), hull);
        assert!(widened(&[[0.0, 1.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]], 1.0, 1.0, 1.0).is_none());
    }
}
