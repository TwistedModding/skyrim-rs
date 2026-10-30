//! `ui/effect/LetterboxEffect`: black bars at the top and bottom of the screen during dialogue
//! and the idle camera's shot, eased in and out. The bars are painted over the frame in both of
//! IC's modes: a painter cannot resample the scene into the band between them, which IC's scale
//! mode does (with its cinematic FOV), so that mode draws as overlay mode.

use std::sync::Mutex;

use framework::egui;

use crate::camera::snapshot as camera_snapshot;
use crate::camera::state::CameraType;
use crate::settings::Profile;
use crate::util::skyrim_util as util;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Letterbox {
    current: f32,
    bar: f32,
    speed: f32,
}

static LETTERBOX: Mutex<Letterbox> = Mutex::new(Letterbox { current: 0.0, bar: 0.12, speed: 2.5 });

/// What the frame asks of the bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wanted {
    dialogue: bool,
    idle: bool,
    /// The console is open: no bars.
    console: bool,
}

/// `Update`: the feature that wants the bars sets their size and speed (the fade out keeps the
/// last one's); the ratio eases toward it and snaps within 0.0001. Returns the bar ratio to paint.
fn step(l: &mut Letterbox, profile: &Profile, wanted: Wanted, dt: f32) -> f32 {
    let dialogue = profile.Dialogue.bLetterbox && wanted.dialogue;
    let idle = profile.Vanity.bLetterbox && wanted.idle;
    if idle {
        (l.bar, l.speed) = (profile.Vanity.fLetterboxRatio, profile.Vanity.fLetterboxTransitionSpeed);
    } else if dialogue {
        (l.bar, l.speed) = (profile.Dialogue.fLetterboxRatio, profile.Dialogue.fLetterboxTransitionSpeed);
    }
    let show = profile.Letterbox.bEnable && !wanted.console && (dialogue || idle);
    let share = (1.0 - (-dt * l.speed * 5.0).exp()).clamp(0.0, 1.0);
    let target = if show { l.bar } else { 0.0 };
    l.current += (target - l.current) * share;
    if (l.current - target).abs() < 0.0001 {
        l.current = target;
    }
    l.current
}

pub fn paint(painter: &egui::Painter, screen: egui::Rect) {
    let snapshot = camera_snapshot::get();
    let wanted = Wanted {
        dialogue: snapshot.current == CameraType::Dialogue && !util::is_menu_open("BarterMenu"),
        idle: snapshot.current == CameraType::Vanity && snapshot.idle_shot,
        console: util::is_menu_open("Console"),
    };
    let ratio = step(&mut LETTERBOX.lock().unwrap_or_else(std::sync::PoisonError::into_inner), &crate::settings::profile(), wanted, util::delta_world_time());
    if ratio > 0.001 {
        let bar = screen.height() * ratio;
        painter.rect_filled(egui::Rect::from_min_size(screen.min, egui::vec2(screen.width(), bar)), 0.0, egui::Color32::BLACK);
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(screen.min.x, screen.max.y - bar), screen.max), 0.0, egui::Color32::BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::{Letterbox, Profile, Wanted, step};

    #[test]
    fn eases_in_for_the_feature_that_wants_it_and_out_with_its_size() {
        let mut p = Profile::default();
        p.Letterbox.bEnable = true;
        p.Dialogue.fLetterboxRatio = 0.2;
        let mut l = Letterbox { current: 0.0, bar: 0.12, speed: 2.5 };
        let dialogue = Wanted { dialogue: true, idle: false, console: false };
        let first = step(&mut l, &p, dialogue, 0.1);
        assert!(first > 0.0 && first < 0.2);
        for _ in 0..200 {
            step(&mut l, &p, dialogue, 0.1);
        }
        assert!((l.current - 0.2).abs() < f32::EPSILON, "snapped to the bar");
        let none = Wanted { dialogue: false, idle: false, console: false };
        for _ in 0..200 {
            step(&mut l, &p, none, 0.1);
        }
        assert!(l.current.abs() < f32::EPSILON && (l.bar - 0.2).abs() < f32::EPSILON);
        p.Letterbox.bEnable = false;
        assert!(step(&mut l, &p, dialogue, 0.1).abs() < f32::EPSILON, "the master switch");
        p.Letterbox.bEnable = true;
        assert!(step(&mut l, &p, Wanted { console: true, ..dialogue }, 0.1).abs() < f32::EPSILON, "never over the console");
    }
}
