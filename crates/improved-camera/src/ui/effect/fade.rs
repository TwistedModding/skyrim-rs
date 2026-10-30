//! `ui/effect/FadeEffect`: the whole screen, HUD included, fades to black and back, for the
//! dialogue's speaker focus shots. Requested from the camera update, stepped and painted at the
//! present.

use std::sync::Mutex;

use framework::egui;

use crate::camera::snapshot as camera_snapshot;
use crate::camera::state::CameraType;
use crate::util::skyrim_util as util;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    alpha: f32,
    target: f32,
    speed: f32,
}

static FADE: Mutex<Fade> = Mutex::new(Fade { alpha: 0.0, target: 0.0, speed: 4.0 });

fn with<R>(f: impl FnOnce(&mut Fade) -> R) -> R {
    f(&mut FADE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// `FadeTo`: eases toward `target` (0 clear, 1 black) at `speed` per second.
pub fn fade_to(target: f32, speed: f32) {
    with(|f| {
        f.target = target.clamp(0.0, 1.0);
        f.speed = speed.max(0.1);
    });
}

/// `IsBlack`.
pub fn is_black() -> bool {
    with(|f| f.alpha >= 0.999)
}

/// `Update`'s step: linear, so full black comes in a predictable time. Outside the dialogue
/// camera, its only client, the target drops to clear so a left fade never strands the screen.
fn step(f: &mut Fade, dialogue: bool, dt: f32) -> f32 {
    if f.target > 0.0 && !dialogue {
        f.target = 0.0;
    }
    let step = dt * f.speed;
    f.alpha = if f.target > f.alpha { (f.alpha + step).min(f.target) } else { (f.alpha - step).max(f.target) };
    f.alpha
}

pub fn paint(painter: &egui::Painter, screen: egui::Rect) {
    let dialogue = camera_snapshot::get().current == CameraType::Dialogue;
    let alpha = with(|f| step(f, dialogue, util::delta_world_time()));
    if alpha > 0.003 {
        painter.rect_filled(screen, 0.0, egui::Color32::BLACK.gamma_multiply(alpha));
    }
}

#[cfg(test)]
mod tests {
    use super::{Fade, step};

    #[test]
    fn steps_linearly_and_clears_outside_the_dialogue() {
        let mut f = Fade { alpha: 0.0, target: 1.0, speed: 4.0 };
        assert!((step(&mut f, true, 0.1) - 0.4).abs() < 1e-6);
        assert!((step(&mut f, true, 0.2) - 1.0).abs() < 1e-6, "clamped at the target");
        f.target = 0.0;
        assert!((step(&mut f, true, 0.05) - 0.8).abs() < 1e-6);
        f.target = 1.0;
        assert!((step(&mut f, false, 0.05) - 0.6).abs() < 1e-6, "outside the dialogue it fades back");
        assert!(f.target.abs() < f32::EPSILON);
    }
}
