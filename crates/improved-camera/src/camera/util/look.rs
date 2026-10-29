//! `camera/util/CameraLook`: the look controller, the camera's yaw (as a player heading) and pitch
//! while a first- or third-person look mode is on. IC's base yaw (the camera actor's heading at a
//! reset) is only read by IC's public API and a debug line, so it is not kept.

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
}

/// `NormalizePlayerAngle`: into 0..2 pi.
fn normalize_player_angle(angle: f32) -> f32 {
    angle.rem_euclid(TAU)
}

impl LookController {
    pub fn reset(&mut self) {
        if self.mode != LookMode::Disabled {
            self.state = LookState::default();
        }
    }

    /// From the camera root's orientation angles (radians, `yaw` from +x).
    pub fn update(&mut self, yaw: f32, pitch: f32) {
        if self.mode != LookMode::Disabled {
            self.state = LookState { yaw: normalize_player_angle(-yaw + FRAC_PI_2), pitch };
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
        look.reset();
        assert_eq!(look.state.yaw, 0.0);
    }
}
