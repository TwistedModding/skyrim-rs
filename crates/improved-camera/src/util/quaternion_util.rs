//! `util/QuaternionUtil`: IC's `NiQuaternion`, the part the camera uses: construction from an
//! axis and angle or a rotation matrix, the product, slerp, the rotated axes and the orientation
//! angles.

use core::ops::{Add, Mul, Neg};

use super::point::{self, M3, P3};

/// `UTIL::NiQuaternion` (w, x, y, z).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// `NiQuaternion::Orientation`, the angles IC reads (radians).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Orientation {
    pub forward: P3,
    pub right: P3,
    pub up: P3,
    pub pitch: f32,
    pub yaw: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };

    /// `NiQuaternion(axis, angle)`; a zero axis gives the identity.
    #[must_use]
    pub fn from_axis_angle(axis: P3, angle: f32) -> Self {
        if point::length(axis) < f32::EPSILON {
            return Self::IDENTITY;
        }
        let (axis, _) = point::unitize(axis);
        let (s, w) = (angle * 0.5).sin_cos();
        Self { w, x: axis[0] * s, y: axis[1] * s, z: axis[2] * s }
    }

    /// `CreateFromRotationMatrix`.
    #[must_use]
    pub fn from_matrix(m: &M3) -> Self {
        let trace = m[0][0] + m[1][1] + m[2][2];
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Self { w: 0.25 * s, x: (m[2][1] - m[1][2]) / s, y: (m[0][2] - m[2][0]) / s, z: (m[1][0] - m[0][1]) / s }
        } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
            let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
            Self { w: (m[2][1] - m[1][2]) / s, x: 0.25 * s, y: (m[0][1] + m[1][0]) / s, z: (m[0][2] + m[2][0]) / s }
        } else if m[1][1] > m[2][2] {
            let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
            Self { w: (m[0][2] - m[2][0]) / s, x: (m[0][1] + m[1][0]) / s, y: 0.25 * s, z: (m[1][2] + m[2][1]) / s }
        } else {
            let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
            Self { w: (m[1][0] - m[0][1]) / s, x: (m[0][2] + m[2][0]) / s, y: (m[1][2] + m[2][1]) / s, z: 0.25 * s }
        };
        q.normalized()
    }

    /// `GetRotationMatrix`.
    #[must_use]
    pub fn matrix(self) -> M3 {
        let Self { w, x, y, z } = self;
        let (xx, yy, zz, xy, xz, yz, wx, wy, wz) = (x * x, y * y, z * z, x * y, x * z, y * z, w * x, w * y, w * z);
        [
            [1.0 - 2.0 * (yy + zz), 2.0 * (xy - wz), 2.0 * (xz + wy)],
            [2.0 * (xy + wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz - wx)],
            [2.0 * (xz - wy), 2.0 * (yz + wx), 1.0 - 2.0 * (xx + yy)],
        ]
    }

    #[must_use]
    pub fn dot(self, o: Self) -> f32 {
        self.w * o.w + self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let sqr = self.dot(self);
        if sqr <= f32::EPSILON { Self::IDENTITY } else { self * (1.0 / sqr.sqrt()) }
    }

    #[must_use]
    pub fn inverse(self) -> Self {
        let sqr = self.dot(self);
        if sqr <= f32::EPSILON { Self::IDENTITY } else { Self { w: self.w, x: -self.x, y: -self.y, z: -self.z } * (1.0 / sqr) }
    }

    /// `Slerp`: the shorter arc, `factor` clamped to 0..1, a normalized lerp when the two are within 0.9995.
    #[must_use]
    pub fn slerp(source: Self, target: Self, factor: f32) -> Self {
        let source = source.normalized();
        let mut target = target.normalized();
        let factor = factor.clamp(0.0, 1.0);
        let mut dot = source.dot(target);
        if dot < 0.0 {
            target = -target;
            dot = -dot;
        }
        let dot = dot.clamp(-1.0, 1.0);
        if dot > 0.9995 {
            return (source * (1.0 - factor) + target * factor).normalized();
        }
        let theta = dot.acos();
        let sin = theta.sin();
        (source * (((1.0 - factor) * theta).sin() / sin) + target * ((factor * theta).sin() / sin)).normalized()
    }

    /// `operator*(NiPoint3)`: the point rotated by the normalized quaternion.
    #[must_use]
    pub fn rotate(self, v: P3) -> P3 {
        let q = self.normalized();
        let axis = [q.x, q.y, q.z];
        let c = point::cross(axis, v);
        point::add(point::add(v, point::scale(c, 2.0 * q.w)), point::scale(point::cross(axis, c), 2.0))
    }

    #[must_use]
    pub fn forward(self) -> P3 {
        self.rotate([0.0, 1.0, 0.0])
    }

    #[must_use]
    pub fn right(self) -> P3 {
        self.rotate([1.0, 0.0, 0.0])
    }

    #[must_use]
    pub fn up(self) -> P3 {
        self.rotate([0.0, 0.0, 1.0])
    }

    /// `GetOrientation`: `pitch = asin(-forward.z)` (positive looking down), `yaw = atan2(forward.y, forward.x)`.
    #[must_use]
    pub fn orientation(self) -> Orientation {
        let forward = self.forward();
        Orientation { forward, right: self.right(), up: self.up(), pitch: (-forward[2]).clamp(-1.0, 1.0).asin(), yaw: forward[1].atan2(forward[0]) }
    }
}

impl Mul for Quat {
    type Output = Self;

    fn mul(self, r: Self) -> Self {
        Self {
            w: self.w * r.w - self.x * r.x - self.y * r.y - self.z * r.z,
            x: self.w * r.x + self.x * r.w + self.y * r.z - self.z * r.y,
            y: self.w * r.y - self.x * r.z + self.y * r.w + self.z * r.x,
            z: self.w * r.z + self.x * r.y - self.y * r.x + self.z * r.w,
        }
    }
}

impl Mul<f32> for Quat {
    type Output = Self;

    fn mul(self, s: f32) -> Self {
        Self { w: self.w * s, x: self.x * s, y: self.y * s, z: self.z * s }
    }
}

impl Add for Quat {
    type Output = Self;

    fn add(self, r: Self) -> Self {
        Self { w: self.w + r.w, x: self.x + r.x, y: self.y + r.y, z: self.z + r.z }
    }
}

impl Neg for Quat {
    type Output = Self;

    fn neg(self) -> Self {
        Self { w: -self.w, x: -self.x, y: -self.y, z: -self.z }
    }
}

/// `NiQuaternion::GetNodeOrientation(node, false).pitch` from the node's local rotation (rows
/// first): `asin(-forward.z)` in radians, with forward the rotation's +Y column, so positive
/// looking down.
#[must_use]
pub fn pitch(rotation: &M3) -> f32 {
    (-rotation[2][1]).clamp(-1.0, 1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::{Quat, pitch};
    use crate::util::point::{self, z_rotation};

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn looking_down_is_a_positive_pitch() {
        let level = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(pitch(&level).abs() < 1e-6);
        let (s, c) = 30f32.to_radians().sin_cos();
        // Rotated about +X: forward (0, 1, 0) goes to (0, c, -s).
        let down = [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]];
        assert!((pitch(&down) - 30f32.to_radians()).abs() < 1e-6);
        assert!((Quat::from_matrix(&down).orientation().pitch - 30f32.to_radians()).abs() < 1e-5);
        assert!((pitch(&[[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -1.5, 0.0]]) - core::f32::consts::FRAC_PI_2).abs() < 1e-6, "clamped");
    }

    #[test]
    fn matrix_round_trips_and_rotates_as_the_matrix() {
        let m = point::mul(&z_rotation(0.7), &Quat::from_axis_angle([1.0, 0.0, 0.0], -0.4).matrix());
        let q = Quat::from_matrix(&m);
        let back = q.matrix();
        for (row, other) in back.iter().zip(&m) {
            assert!(close(*row, *other));
        }
        let v = [0.3, -2.0, 1.5];
        assert!(close(q.rotate(v), point::mul_vec(&m, v)));
        let product = (Quat::from_axis_angle([0.0, 0.0, 1.0], 0.7) * Quat::from_axis_angle([1.0, 0.0, 0.0], -0.4)).matrix();
        assert!(product.iter().zip(&m).all(|(a, b)| close(*a, *b)), "the product composes as the matrices do");
        assert!(close((q * q.inverse()).rotate(v), v));
    }

    #[test]
    fn slerp_takes_the_short_arc_and_hits_the_ends() {
        let a = Quat::from_axis_angle([0.0, 0.0, 1.0], 0.0);
        let b = Quat::from_axis_angle([0.0, 0.0, 1.0], 1.0);
        let half = Quat::slerp(a, b, 0.5);
        assert!(close(half.forward(), Quat::from_axis_angle([0.0, 0.0, 1.0], 0.5).forward()));
        assert!(close(Quat::slerp(a, -b, 1.0).forward(), b.forward()), "a negated end is the same rotation");
        assert!(close(Quat::slerp(a, b, 2.0).forward(), b.forward()), "clamped");
        let yaw = Quat::from_axis_angle([0.0, 0.0, 1.0], 0.5).orientation().yaw;
        assert!((yaw - (core::f32::consts::FRAC_PI_2 + 0.5)).abs() < 1e-5, "{yaw}: atan2 of forward from +x");
    }
}
