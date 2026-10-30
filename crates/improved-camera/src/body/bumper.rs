//! `body/BodyBumper`: while the camera sits away from the player's own eyes, the player's convex
//! controller shape is widened by the profile's forward, back and side bumper offsets so the body
//! keeps clear of walls and doorways the camera would otherwise clip into; the original goes back
//! when the camera returns to first person, on unload and before a reload.

use std::sync::Mutex;

use skyrim::physics::HAVOK_SCALE;
use skyrim::physics::controller::{self, ConvexBuild, Replacement, ShapeExt};
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

#[derive(Default)]
struct Bumper {
    /// The controller the original vertices belong to.
    controller: usize,
    /// The original hull's vertices.
    original: Vec<[f32; 4]>,
    replacement: Option<Replacement>,
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
        if self.replacement.as_ref().is_some_and(|r| r.is_current(controller)) {
            return;
        }
        self.replacement = None;
        let Some(convex) = controller::root_shape(controller).and_then(controller_convex) else { return };
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
    use super::widened;

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
