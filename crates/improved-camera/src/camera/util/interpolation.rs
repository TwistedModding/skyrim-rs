//! `camera/util/CameraInterpolation`: the easing catalog indexed by the profile's method numbers
//! and the timed collision pull-in the idle camera eases its orbit with. IC's `Spring` is used
//! only by its follow camera, which is not ported.

use core::f32::consts::PI;

use crate::util::math_util::lerp;

/// `METHOD_NAMES`, in the order of the profile's method numbers.
pub const METHOD_NAMES: [&str; 22] = [
    "Linear",
    "Quadratic In",
    "Quadratic Out",
    "Quadratic In/Out",
    "Cubic In",
    "Cubic Out",
    "Cubic In/Out",
    "Quartic In",
    "Quartic Out",
    "Quartic In/Out",
    "Quintic In",
    "Quintic Out",
    "Quintic In/Out",
    "Sine In",
    "Sine Out",
    "Sine In/Out",
    "Circular In",
    "Circular Out",
    "Circular In/Out",
    "Exponential In",
    "Exponential Out",
    "Exponential In/Out",
];

/// `Ease`: method `method` at `t` (clamped to 0..=1); an unknown method is linear.
#[must_use]
#[allow(clippy::float_cmp)]
pub fn ease(method: i32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match method {
        1 => t * t,
        2 => -t * (t - 2.0),
        3 => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                -2.0 * t * t + 4.0 * t - 1.0
            }
        }
        4 => t * t * t,
        5 => (t - 1.0).powi(3) + 1.0,
        6 => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                0.5 * (2.0 * t - 2.0).powi(3) + 1.0
            }
        }
        7 => t.powi(4),
        8 => 1.0 - (t - 1.0).powi(4),
        9 => {
            if t < 0.5 {
                8.0 * t.powi(4)
            } else {
                1.0 - 8.0 * (t - 1.0).powi(4)
            }
        }
        10 => t.powi(5),
        11 => (t - 1.0).powi(5) + 1.0,
        12 => {
            if t < 0.5 {
                16.0 * t.powi(5)
            } else {
                0.5 * (2.0 * t - 2.0).powi(5) + 1.0
            }
        }
        13 => 1.0 - (t * PI * 0.5).cos(),
        14 => (t * PI * 0.5).sin(),
        15 => 0.5 * (1.0 - (t * PI).cos()),
        16 => 1.0 - (1.0 - t * t).sqrt(),
        17 => (t * (2.0 - t)).sqrt(),
        18 => {
            if t < 0.5 {
                0.5 * (1.0 - (1.0 - 4.0 * t * t).sqrt())
            } else {
                f32::midpoint((-(2.0 * t - 3.0) * (2.0 * t - 1.0)).sqrt(), 1.0)
            }
        }
        19 => {
            if t == 0.0 {
                0.0
            } else {
                2f32.powf(10.0 * (t - 1.0))
            }
        }
        20 => {
            if t == 1.0 {
                1.0
            } else {
                1.0 - 2f32.powf(-10.0 * t)
            }
        }
        21 => {
            if t == 0.0 || t == 1.0 {
                t
            } else if t < 0.5 {
                0.5 * 2f32.powf(20.0 * t - 10.0)
            } else {
                1.0 - 0.5 * 2f32.powf(-20.0 * t + 10.0)
            }
        }
        _ => t,
    }
}

/// `Interpolation::Collision`: a timed eased chase of the collision pull-in fraction. A change of
/// direction starts over from the current value with that direction's method and duration; a
/// target that moves the same way only moves the target, so it still lands on the clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collision {
    pub current: f32,
    start: f32,
    target: f32,
    elapsed: f32,
    duration: f32,
    method: i32,
    running: bool,
}

impl Default for Collision {
    fn default() -> Self {
        Self { current: 1.0, start: 1.0, target: 1.0, elapsed: 0.0, duration: 0.0, method: 0, running: false }
    }
}

/// One direction's easing: method and seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Easing {
    pub method: i32,
    pub duration: f32,
}

impl Collision {
    /// `Snap`.
    pub fn snap(&mut self, fraction: f32) {
        (self.current, self.start, self.target) = (fraction, fraction, fraction);
        self.elapsed = 0.0;
        self.running = false;
    }

    /// `Update`: the fraction this frame chasing `hit`, pulled in with `pull_in` and recovering
    /// with `recovery`.
    #[allow(clippy::float_cmp)]
    pub fn update(&mut self, hit: f32, pull_in: Easing, recovery: Easing, dt: f32) -> f32 {
        if hit != self.target {
            let pulling_in = hit < self.current;
            if !self.running || (self.target < self.start) != pulling_in {
                let easing = if pulling_in { pull_in } else { recovery };
                (self.start, self.elapsed, self.method, self.duration, self.running) = (self.current, 0.0, easing.method, easing.duration, true);
            }
            self.target = hit;
        }
        if self.running {
            self.elapsed += dt;
            let s = (self.elapsed / self.duration.max(0.01)).clamp(0.0, 1.0);
            if s < 1.0 {
                self.current = lerp(self.start, self.target, ease(self.method, s));
            } else {
                self.current = self.target;
                self.running = false;
            }
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::{Collision, Easing, METHOD_NAMES, ease};

    #[test]
    fn every_method_runs_from_zero_to_one() {
        for (method, _) in (0..).zip(METHOD_NAMES) {
            assert!(ease(method, 0.0).abs() < 1e-4, "{method}");
            assert!((ease(method, 1.0) - 1.0).abs() < 1e-4, "{method}");
            assert!((0.0..=1.0001).contains(&ease(method, 0.5)), "{method}");
        }
        assert!((ease(99, 0.3) - 0.3).abs() < f32::EPSILON);
        assert!((ease(2, 0.5) - 0.75).abs() < 1e-6);
        assert!((ease(15, 0.5) - 0.5).abs() < 1e-6);
        assert!((ease(1, 2.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_pull_in_lands_on_its_clock_and_recovery_starts_over_from_where_it_is() {
        let pull_in = Easing { method: 0, duration: 0.2 };
        let recovery = Easing { method: 0, duration: 0.6 };
        let mut c = Collision::default();
        assert!((c.update(0.5, pull_in, recovery, 0.1) - 0.75).abs() < 1e-6);
        assert!((c.update(0.4, pull_in, recovery, 0.05) - 0.55).abs() < 1e-6);
        assert!((c.update(0.4, pull_in, recovery, 0.05) - 0.4).abs() < 1e-6);
        assert!((c.update(1.0, pull_in, recovery, 0.3) - 0.7).abs() < 1e-6);
        c.snap(0.25);
        assert!((c.update(0.25, pull_in, recovery, 1.0) - 0.25).abs() < f32::EPSILON);
    }
}
