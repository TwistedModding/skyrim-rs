//! `util/MathUtil.h`: the easing helpers the modules share.

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
