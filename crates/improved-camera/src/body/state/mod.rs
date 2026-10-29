//! `body/state`: the four state machines the body manager blends.

pub mod action;
pub mod hands;
pub mod movement;
pub mod race;

use crate::util::math_util::{damped_alpha, lerp};

/// IC's frame-rate independent ease: moves `from` toward `to` by `1 - e^(-speed * dt)`.
#[must_use]
pub fn approach(from: [f32; 3], to: [f32; 3], speed: f32, dt: f32) -> [f32; 3] {
    let t = damped_alpha(speed, dt);
    [0, 1, 2].map(|i| lerp(from[i], to[i], t))
}
