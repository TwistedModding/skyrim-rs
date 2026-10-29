//! `util/QuaternionUtil`: the part ported so far, the pitch of a node's orientation.

/// `NiQuaternion::GetNodeOrientation(node, false).pitch` from the node's local rotation (rows
/// first): `asin(-forward.z)` in radians, with forward the rotation's +Y column, so positive
/// looking down.
#[must_use]
pub fn pitch(rotation: &[[f32; 3]; 3]) -> f32 {
    (-rotation[2][1]).clamp(-1.0, 1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::pitch;

    #[test]
    fn looking_down_is_a_positive_pitch() {
        let level = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(pitch(&level).abs() < 1e-6);
        let (s, c) = 30f32.to_radians().sin_cos();
        // Rotated about +X: forward (0, 1, 0) goes to (0, c, -s).
        let down = [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]];
        assert!((pitch(&down) - 30f32.to_radians()).abs() < 1e-6);
        assert!((pitch(&[[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -1.5, 0.0]]) - core::f32::consts::FRAC_PI_2).abs() < 1e-6, "clamped");
    }
}
