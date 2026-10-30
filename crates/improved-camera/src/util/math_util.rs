//! `util/MathUtil.h`: the easing helpers the modules share and `SkyrimUtil.h`'s angle wraps.

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

#[cfg(test)]
mod tests {
    use skyrim::render::from_euler_zxy;

    use super::*;
    use crate::util::quaternion_util::Quat;

    #[test]
    fn the_engines_zxy_reads_back_as_the_cameras_orientation() {
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5);
        let forward = |m: &[[f32; 3]; 3]| [m[0][1], m[1][1], m[2][1]];
        let level = from_euler_zxy(0.0, 0.0, 0.0);
        assert!(close(forward(&level), [0.0, 1.0, 0.0]) && close([level[0][0], level[1][0], level[2][0]], [1.0, 0.0, 0.0]));
        assert!(close(forward(&from_euler_zxy(PI / 2.0, 0.0, 0.0)), [1.0, 0.0, 0.0]), "heading pi/2 is east");
        let (pitch, heading) = (0.4_f32, 1.1_f32);
        let o = Quat::from_matrix(&from_euler_zxy(heading, pitch, 0.0)).orientation();
        assert!((o.pitch - pitch).abs() < 1e-5, "the camera's own pitch reads it back");
        assert!((o.yaw - (PI / 2.0 - heading)).abs() < 1e-5);
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
