//! `util/MathUtil.h`: the easing helpers the modules share, `SkyrimUtil.h`'s angle wraps, and the
//! rotation of a reference's angles.

use core::f32::consts::{PI, TAU};

/// `DampedAlpha`: the frame-rate independent share `1 - e^(-speed * dt)` an ease moves per frame.
#[must_use]
pub fn damped_alpha(speed: f32, dt: f32) -> f32 {
    1.0 - (-speed * dt).exp()
}

/// `std::lerp`.
#[must_use]
pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// `SmootherStep`: the quintic ease, unclamped.
#[must_use]
pub fn smoother_step(t: f32) -> f32 {
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

/// `Normalize_Radian`: into -pi..=pi.
#[must_use]
pub fn normalize_radian(angle: f32) -> f32 {
    let a = angle % TAU;
    if a.abs() > PI { a - TAU * a.signum() } else { a }
}

/// `NormalizeRelativeAngle`: `std::remainder(angle, 2 pi)`.
#[must_use]
pub fn normalize_relative_angle(angle: f32) -> f32 {
    angle - TAU * (angle / TAU).round_ties_even()
}

/// The rotation of a reference's angles (`rotX` pitch, positive looking down; `rotY` roll; `rotZ`
/// heading, clockwise from +Y seen from above) as a node rotation whose columns are right,
/// forward and up: heading about -Z, then pitch about -X, then roll about -Y, so heading 0 looks
/// along +Y and heading pi/2 along +X.
#[must_use]
pub fn rotation_from_angles(pitch: f32, roll: f32, heading: f32) -> [[f32; 3]; 3] {
    let (sx, cx) = (-pitch).sin_cos();
    let (sy, cy) = (-roll).sin_cos();
    let (sz, cz) = (-heading).sin_cos();
    let z = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let x = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let y = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    super::point::mul(&super::point::mul(&z, &x), &y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::quaternion_util::Quat;

    #[test]
    fn angles_make_the_reference_facing() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        let forward = |m: &[[f32; 3]; 3]| [m[0][1], m[1][1], m[2][1]];
        let level = rotation_from_angles(0.0, 0.0, 0.0);
        assert!(close(forward(&level), [0.0, 1.0, 0.0]) && close([level[0][0], level[1][0], level[2][0]], [1.0, 0.0, 0.0]));
        assert!(close(forward(&rotation_from_angles(0.0, 0.0, PI / 2.0)), [1.0, 0.0, 0.0]), "heading pi/2 is east");
        let (pitch, heading) = (0.4_f32, 1.1_f32);
        let m = rotation_from_angles(pitch, 0.0, heading);
        assert!(close(forward(&m), [heading.sin() * pitch.cos(), heading.cos() * pitch.cos(), -pitch.sin()]));
        let o = Quat::from_matrix(&m).orientation();
        assert!((o.pitch - pitch).abs() < 1e-5, "the camera's own pitch reads it back");
        assert!((o.yaw - (PI / 2.0 - heading)).abs() < 1e-5);
        let rolled = rotation_from_angles(0.0, 0.3, 0.0);
        assert!(close(forward(&rolled), [0.0, 1.0, 0.0]), "roll turns about the forward axis");
        let det = |m: [[f32; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        assert!((det(rotation_from_angles(0.3, 0.2, -2.0)) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn angles_wrap_into_half_turns() {
        assert!((normalize_radian(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-5);
        assert!((normalize_radian(-3.0 * PI / 2.0) - PI / 2.0).abs() < 1e-5);
        assert!((normalize_radian(0.5) - 0.5).abs() < 1e-6);
        assert!((normalize_relative_angle(7.0) - (7.0 - TAU)).abs() < 1e-5);
        assert!((normalize_relative_angle(-4.0) - (-4.0 + TAU)).abs() < 1e-5);
    }

    #[test]
    fn the_quintic_ease_is_flat_at_both_ends() {
        assert!(smoother_step(0.0).abs() < f32::EPSILON && (smoother_step(1.0) - 1.0).abs() < 1e-6);
        assert!((smoother_step(0.5) - 0.5).abs() < 1e-6);
        assert!(smoother_step(0.01) < 0.0001);
    }
}
