//! The point and matrix arithmetic IC writes with the engine's `NiPoint3` and `NiMatrix3`
//! operators, on `[f32; 3]` points and row-major `[[f32; 3]; 3]` matrices.

pub use skyrim::physics::lerp;
pub use skyrim::render::mul3 as mul;

pub type P3 = [f32; 3];
pub type M3 = [[f32; 3]; 3];

pub const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[must_use]
pub fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[must_use]
pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[must_use]
pub fn scale(a: P3, s: f32) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[must_use]
pub fn dot(a: P3, b: P3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[must_use]
pub fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[must_use]
pub fn length(a: P3) -> f32 {
    dot(a, a).sqrt()
}

/// `NiPoint3::Unitize`: the unit vector and the length it had; a zero vector stays zero.
#[must_use]
pub fn unitize(a: P3) -> (P3, f32) {
    let len = length(a);
    if len > 0.0 { (scale(a, 1.0 / len), len) } else { ([0.0; 3], 0.0) }
}

/// `matrix * point`.
#[must_use]
pub fn mul_vec(m: &M3, v: P3) -> P3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

#[must_use]
pub fn transpose(m: &M3) -> M3 {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

/// A matrix whose columns are `right`, `forward` and `up`: the engine's node basis.
#[must_use]
pub fn from_columns(right: P3, forward: P3, up: P3) -> M3 {
    [[right[0], forward[0], up[0]], [right[1], forward[1], up[1]], [right[2], forward[2], up[2]]]
}

/// `NiMatrix3::MakeZRotation`.
#[must_use]
pub fn z_rotation(angle: f32) -> M3 {
    let (s, c) = angle.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_follow_the_engine_order() {
        let r = z_rotation(core::f32::consts::FRAC_PI_2);
        let v = mul_vec(&r, [1.0, 0.0, 0.0]);
        assert!(v[0].abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6);
        let rr = mul(&r, &transpose(&r));
        for (i, row) in rr.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                assert!((x - IDENTITY[i][j]).abs() < 1e-6);
            }
        }
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(unitize([0.0, 3.0, 4.0]), ([0.0, 0.6, 0.8], 5.0));
        assert_eq!(from_columns([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0])[1], [2.0, 5.0, 8.0]);
    }
}
