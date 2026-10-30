//! `camera/util/CameraLook`: the look controller, the camera's yaw (as a player heading) and pitch
//! while a first- or third-person look mode is on, and the base yaw the look restriction measures
//! the yaw from (the camera actor's heading, taken at a reset).

use core::f32::consts::{FRAC_PI_2, TAU};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LookMode {
    #[default]
    Disabled,
    FirstPerson,
    ThirdPerson,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookState {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookController {
    pub mode: LookMode,
    pub state: LookState,
    pub base_yaw: f32,
}

/// `NormalizePlayerAngle`: into 0..2 pi.
fn normalize_player_angle(angle: f32) -> f32 {
    angle.rem_euclid(TAU)
}

/// The heading a camera yaw (radians from +x) looks along.
#[must_use]
pub fn camera_heading(yaw: f32) -> f32 {
    normalize_player_angle(-yaw + FRAC_PI_2)
}

impl LookController {
    /// `Reset`: the look back to zero and the base yaw to `base`, the camera actor's heading or
    /// [`camera_heading`] without one.
    pub fn reset(&mut self, base: Option<f32>) {
        if self.mode != LookMode::Disabled {
            self.state = LookState::default();
            self.reset_yaw(base);
        }
    }

    /// `ResetYaw`: the base yaw alone.
    pub fn reset_yaw(&mut self, base: Option<f32>) {
        if self.mode != LookMode::Disabled
            && let Some(base) = base
        {
            self.base_yaw = base;
        }
    }

    /// From the camera root's orientation angles (radians, `yaw` from +x).
    pub fn update(&mut self, yaw: f32, pitch: f32) {
        if self.mode != LookMode::Disabled {
            self.state = LookState { yaw: camera_heading(yaw), pitch };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LookController, LookMode};

    #[test]
    fn the_look_follows_the_camera_as_a_heading_only_while_enabled() {
        let mut look = LookController::default();
        look.update(1.0, 0.2);
        assert_eq!(look.state.pitch, 0.0, "disabled");
        look.mode = LookMode::ThirdPerson;
        look.update(core::f32::consts::FRAC_PI_2, 0.2);
        assert!(look.state.yaw.abs() < 1e-6 && (look.state.pitch - 0.2).abs() < f32::EPSILON, "camera facing +y is heading 0");
        look.update(0.0, 0.0);
        assert!((look.state.yaw - core::f32::consts::FRAC_PI_2).abs() < 1e-6, "facing +x is heading pi/2");
        look.reset(Some(1.5));
        assert_eq!((look.state.yaw, look.base_yaw), (0.0, 1.5));
        look.reset_yaw(None);
        assert_eq!(look.base_yaw, 1.5, "no camera root keeps the base");
        look.mode = LookMode::Disabled;
        look.reset_yaw(Some(0.5));
        assert_eq!(look.base_yaw, 1.5, "disabled");
    }
}
