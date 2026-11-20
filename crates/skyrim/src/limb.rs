// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Turning a limb of two bones so its end is somewhere else, and the vectors and rotations for
//! it in the engine's convention: row-major matrices that turn a column vector
//! (`world = R * local`), as [`Transform`](crate::render::Transform) composes them. Nothing
//! here reads the game.

pub use crate::render::{Matrix3, mul3};

/// A vector as the engine's `NiPoint3` holds it.
pub type Vec3 = [f32; 3];

/// The turn that turns nothing.
pub const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[must_use]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[must_use]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[must_use]
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[must_use]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[must_use]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[must_use]
pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` at unit length; `None` when it is too short to have a direction.
#[must_use]
pub fn normalize(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    (l > 1e-6).then(|| scale(a, 1.0 / l))
}

#[must_use]
pub fn transpose(m: &Matrix3) -> Matrix3 {
    core::array::from_fn(|i| core::array::from_fn(|j| m[j][i]))
}

#[must_use]
pub fn rotate(m: &Matrix3, v: Vec3) -> Vec3 {
    core::array::from_fn(|i| dot(m[i], v))
}

/// The turn by `angle` radians about the unit `axis`, counter-clockwise looking down the axis.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn axis_angle(axis: Vec3, angle: f32) -> Matrix3 {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = axis;
    [[t * x * x + c, t * x * y - s * z, t * x * z + s * y], [t * x * y + s * z, t * y * y + c, t * y * z - s * x], [t * x * z - s * y, t * y * z + s * x, t * z * z + c]]
}

/// The smallest turn taking direction `from` onto direction `to`; the identity when either has
/// no direction.
#[must_use]
pub fn rotation_between(from: Vec3, to: Vec3) -> Matrix3 {
    let (Some(a), Some(b)) = (normalize(from), normalize(to)) else { return IDENTITY };
    let c = dot(a, b).clamp(-1.0, 1.0);
    match normalize(cross(a, b)) {
        Some(axis) => axis_angle(axis, c.acos()),
        None if c > 0.0 => IDENTITY,
        None => {
            let helper = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            axis_angle(normalize(cross(a, helper)).unwrap_or([0.0, 0.0, 1.0]), core::f32::consts::PI)
        }
    }
}

/// How close to straight a limb may be pulled, as a share of its full reach.
const MAX_REACH: f32 = 0.999;

/// World turns for a limb: `root` for its first bone about the joint it hangs from (a hip, a
/// shoulder), then `joint` for everything below the middle joint (a knee, an elbow) about that
/// joint as it stands after the first turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimbTurn {
    pub root: Matrix3,
    pub joint: Matrix3,
}

/// The sine of the least bend at the knee whose bend plane gives a usable hinge.
const MIN_BEND: f32 = 0.1;

/// The knee's hinge for a leg standing `hip`, `knee`, `end`: the normal of the plane it bends in,
/// pointing so that a positive turn about it bends the knee further; `None` when the leg is too
/// straight for that plane to be steady.
#[must_use]
pub fn hinge_of(hip: Vec3, knee: Vec3, end: Vec3) -> Option<Vec3> {
    let (t, s) = (normalize(sub(knee, hip))?, normalize(sub(end, knee))?);
    let n = cross(t, s);
    (length(n) > MIN_BEND).then(|| normalize(n)).flatten()
}

/// Solves the leg `hip`, `knee`, `end` (the tip of the rigid piece below the knee: the ankle or
/// the foot) so that `end` reaches `target`, or as near as the leg's length allows along the
/// line from the hip. The knee turns only about `hinge` (world; the skeleton's own when it has
/// one, else the plane the leg bends in now, else square to `pole`, the side a straight leg's
/// knee goes to) to make the hip-to-end distance right; then the whole leg swings at the hip by
/// the least turn onto the target, so the knee points where the animation points it. An arm is
/// solved the same way, as its shoulder, elbow and wrist. `None` for a degenerate limb.
#[must_use]
pub fn solve(hip: Vec3, knee: Vec3, end: Vec3, target: Vec3, hinge: Option<Vec3>, pole: Vec3) -> Option<LimbTurn> {
    let thigh = sub(knee, hip);
    let shin = sub(end, knee);
    let l1 = length(thigh);
    if l1 < 1e-4 || length(shin) < 1e-4 {
        return None;
    }
    let dir = scale(thigh, 1.0 / l1);
    let square = |v: Vec3| normalize(sub(v, scale(dir, dot(v, dir))));
    let hinge = hinge.and_then(square).or_else(|| hinge_of(hip, knee, end)).or_else(|| square(cross(pole, dir))).or_else(|| square([1.0, 0.0, 0.0]))?;
    // The shin's part in the bend plane, at angle `now` from the thigh's direction; its part along the hinge does not turn.
    let side = cross(hinge, dir);
    let (along, across) = (dot(shin, dir), dot(shin, side));
    let radius = along.hypot(across);
    let now = across.atan2(along);
    let reach = |cos: f32| (l1 * l1 + dot(shin, shin) + 2.0 * l1 * radius * cos).max(0.0).sqrt();
    let distance = length(sub(target, hip)).clamp(reach(-1.0) + 1e-3, reach(1.0) * MAX_REACH);
    let bend = ((distance * distance - l1 * l1 - dot(shin, shin)) / (2.0 * l1 * radius.max(1e-4))).clamp(-1.0, 1.0).acos();
    let knee_turn = axis_angle(hinge, bend - now);
    let bent_end = add(thigh, rotate(&knee_turn, shin));
    let hip_turn = rotation_between(bent_end, normalize(sub(target, hip)).unwrap_or(bent_end));
    Some(LimbTurn { root: hip_turn, joint: mul3(&hip_turn, &mul3(&knee_turn, &transpose(&hip_turn))) })
}

/// The local rotation of a bone whose world rotation `world` turns by `turn` in world space,
/// under a parent whose world rotation is now `parent`.
#[must_use]
pub fn local_after(parent: &Matrix3, world: &Matrix3, turn: &Matrix3) -> Matrix3 {
    mul3(&transpose(parent), &mul3(turn, world))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        length(sub(a, b)) < eps
    }

    /// Whether the knee of a solved leg stays on the side of `plane_normal` it was on.
    fn bends_same_side(before: Vec3, after: Vec3, plane_normal: Vec3) -> bool {
        dot(before, plane_normal) * dot(after, plane_normal) >= 0.0
    }
    
    /// The normal of the plane a leg bends in.
    fn bend_normal(hip: Vec3, knee: Vec3, end: Vec3) -> Option<Vec3> {
        normalize(cross(sub(knee, hip), sub(end, hip)))
    }

    fn apply(turn: &LimbTurn, hip: Vec3, knee: Vec3, end: Vec3) -> (Vec3, Vec3) {
        let k = add(hip, rotate(&turn.root, sub(knee, hip)));
        let e = add(k, rotate(&turn.joint, rotate(&turn.root, sub(end, knee))));
        (k, e)
    }

    const HIP: Vec3 = [0.0, 0.0, 100.0];
    const KNEE: Vec3 = [0.0, 8.0, 52.0];
    const ANKLE: Vec3 = [0.0, 0.0, 6.0];

    #[test]
    fn a_reachable_target_is_reached_and_the_bones_keep_their_lengths() {
        for target in [[0.0, 0.0, 10.0], [0.0, 0.0, 14.0], [5.0, -4.0, 20.0], [0.0, 10.0, 30.0]] {
            let turn = solve(HIP, KNEE, ANKLE, target, None, [0.0, 1.0, 0.0]).unwrap();
            let (k, e) = apply(&turn, HIP, KNEE, ANKLE);
            assert!(close(e, target, 1e-2), "{target:?}: reached {e:?}");
            assert!((length(sub(k, HIP)) - length(sub(KNEE, HIP))).abs() < 1e-3);
            assert!((length(sub(e, k)) - length(sub(ANKLE, KNEE))).abs() < 1e-3);
        }
    }

    #[test]
    fn the_knee_keeps_its_side() {
        let normal = bend_normal(HIP, KNEE, ANKLE).unwrap();
        let turn = solve(HIP, KNEE, ANKLE, [0.0, 0.0, 30.0], None, [0.0, 1.0, 0.0]).unwrap();
        let (k, _) = apply(&turn, HIP, KNEE, ANKLE);
        assert!(k[1] > 0.0, "the knee went behind: {k:?}");
        assert!(bends_same_side(sub(KNEE, HIP), sub(k, HIP), [0.0, 1.0, 0.0]));
        assert!(normal[0].abs() > 0.99);
    }

    #[test]
    fn the_knee_turns_only_about_its_hinge() {
        let hinge = bend_normal(HIP, KNEE, ANKLE).unwrap();
        for target in [[5.0, -4.0, 20.0], [-6.0, 3.0, 12.0], [8.0, 8.0, 30.0]] {
            let turn = solve(HIP, KNEE, ANKLE, target, None, [0.0, 1.0, 0.0]).unwrap();
            let relative = mul3(&transpose(&turn.root), &mul3(&turn.joint, &turn.root));
            assert!(close(rotate(&relative, hinge), hinge, 1e-4), "{target:?}: the shin twisted about the knee");
        }
    }

    #[test]
    fn a_nearly_straight_leg_bends_about_the_skeletons_hinge_whichever_way_it_leans() {
        let hinge = [1.0, 0.0, 0.0];
        let target = [0.0, 0.0, 20.0];
        let knees: Vec<Vec3> = [0.3, -0.3]
            .into_iter()
            .map(|lean| {
                let (hip, knee, end) = ([0.0, 0.0, 100.0], [lean, 0.0, 50.0], [0.0, 0.0, 0.0]);
                let turn = solve(hip, knee, end, target, Some(hinge), [0.0, 1.0, 0.0]).unwrap();
                let (k, e) = apply(&turn, hip, knee, end);
                assert!(close(e, target, 1e-2), "{lean}: reached {e:?}");
                k
            })
            .collect();
        assert!(knees.iter().all(|k| k[1] < -1.0), "{knees:?}");
        assert!(close(knees[0], knees[1], 1.0), "the knee jumped with the lean: {knees:?}");
    }

    #[test]
    fn the_hinge_of_a_straight_leg_is_unknown() {
        assert!(hinge_of([0.0, 0.0, 100.0], [0.0, 0.0, 50.0], [0.0, 0.0, 0.0]).is_none());
        assert!(close(hinge_of(HIP, KNEE, ANKLE).unwrap(), [-1.0, 0.0, 0.0], 1e-5));
    }

    #[test]
    fn an_unreachable_target_stretches_the_leg_toward_it() {
        let turn = solve(HIP, KNEE, ANKLE, [0.0, 0.0, -200.0], None, [0.0, 1.0, 0.0]).unwrap();
        let (_, e) = apply(&turn, HIP, KNEE, ANKLE);
        assert!(e[2] < ANKLE[2] && e[2] > -2.0, "{e:?}");
        assert!(e[0].abs() < 1e-3 && e[1].abs() < 0.5);
    }

    #[test]
    fn a_straight_leg_bends_toward_the_pole() {
        let (hip, knee, end) = ([0.0, 0.0, 100.0], [0.0, 0.0, 50.0], [0.0, 0.0, 0.0]);
        let turn = solve(hip, knee, end, [0.0, 0.0, 20.0], None, [0.0, 1.0, 0.0]).unwrap();
        let (k, e) = apply(&turn, hip, knee, end);
        assert!(k[1] > 1.0 && close(e, [0.0, 0.0, 20.0], 1e-2), "{k:?} {e:?}");
    }

    #[test]
    fn the_target_where_the_end_is_changes_nothing() {
        let turn = solve(HIP, KNEE, ANKLE, ANKLE, None, [0.0, 1.0, 0.0]).unwrap();
        let (k, e) = apply(&turn, HIP, KNEE, ANKLE);
        assert!(close(k, KNEE, 1e-2) && close(e, ANKLE, 1e-2));
    }

    #[test]
    fn a_world_turn_becomes_the_local_that_produces_it() {
        let parent = axis_angle([0.0, 0.0, 1.0], 0.7);
        let local = axis_angle([1.0, 0.0, 0.0], 0.3);
        let world = mul3(&parent, &local);
        let turn = axis_angle([0.0, 1.0, 0.0], 0.4);
        let new_local = local_after(&parent, &world, &turn);
        let got = mul3(&parent, &new_local);
        let want = mul3(&turn, &world);
        for r in 0..3 {
            assert!(close(got[r], want[r], 1e-5));
        }
    }

    #[test]
    fn rotation_between_maps_the_first_direction_onto_the_second() {
        for (a, b) in [([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), ([0.0, 0.0, 1.0], [0.3, -0.2, 0.9]), ([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]), ([0.0, 0.0, 1.0], [0.0, 0.0, -1.0])] {
            let r = rotation_between(a, b);
            assert!(close(rotate(&r, normalize(a).unwrap()), normalize(b).unwrap(), 1e-5), "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn axis_angle_turns_counter_clockwise_about_z() {
        let r = axis_angle([0.0, 0.0, 1.0], core::f32::consts::FRAC_PI_2);
        assert!(close(rotate(&r, [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0], 1e-6));
    }
}
