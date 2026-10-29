//! `body/state`: the four state machines the body manager blends.

pub mod action;
pub mod hands;
pub mod movement;
pub mod race;

/// IC's frame-rate independent ease: moves `from` toward `to` by `1 - e^(-speed * dt)`.
#[must_use]
pub fn approach(from: [f32; 3], to: [f32; 3], speed: f32, dt: f32) -> [f32; 3] {
    let t = 1.0 - (-speed * dt).exp();
    [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * t)
}
