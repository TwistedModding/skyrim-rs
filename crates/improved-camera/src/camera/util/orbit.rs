//! `camera/util/CameraOrbit`: the mouse orbit some states offer around the player. Holding the
//! right button turns the camera around the player, the middle button pans, the wheel zooms, and a
//! middle click without a drag resets. While idle the camera keeps the engine's transform, which
//! the controller captures as its base; it writes the root only while it is moved off that base.

use framework::GameMouse;

use crate::util::point::{self, M3, P3};
use crate::util::quaternion_util::Quat;

/// `OrbitController::Settings`, IC's defaults (IC constructs every controller with them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub rotate_sensitivity: f32,
    pub zoom_step: f32,
    pub reset_click_slop: f32,
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub pitch_limit: f32,
    pub min_pivot_distance: f32,
    pub max_pan_distance: f32,
    pub min_camera_distance: f32,
    pub max_camera_distance: f32,
    pub rotate_button: u32,
    pub pan_button: u32,
    pub zoom_in_button: u32,
    pub zoom_out_button: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rotate_sensitivity: 0.01,
            zoom_step: 1.08,
            reset_click_slop: 4.0,
            min_zoom: 0.2,
            max_zoom: 5.0,
            pitch_limit: 1.35,
            min_pivot_distance: 20.0,
            max_pan_distance: 250.0,
            min_camera_distance: 16.0,
            max_camera_distance: 600.0,
            rotate_button: 1,
            pan_button: 2,
            zoom_in_button: 8,
            zoom_out_button: 9,
        }
    }
}

const POSITION_THRESHOLD: f32 = 0.01;
const ROTATION_THRESHOLD: f32 = 0.001;

/// A camera root transform, local.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: P3,
    pub rotation: M3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OrbitController {
    pub settings: Settings,
    base: Option<Pose>,
    written: Option<Pose>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    look_yaw: f32,
    pan_step: f32,
    pan_offset: P3,
    rotating: bool,
    panning: bool,
    drag_distance: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self { position: [0.0; 3], rotation: point::IDENTITY }
    }
}

/// `OrbitPanStep`: world units per pixel, so a drag stays screen-consistent.
fn pan_step(distance: f32) -> f32 {
    distance.max(30.0) * 0.0015
}

/// `OrbitRightFromYaw`: the screen-horizontal drag axis for a look yaw.
fn right_from_yaw(look_yaw: f32) -> P3 {
    [look_yaw.cos(), -look_yaw.sin(), 0.0]
}

/// IC's `std::remainder`.
fn remainder(x: f32, y: f32) -> f32 {
    x - y * (x / y).round_ties_even()
}

impl OrbitController {
    #[must_use]
    pub fn new() -> Self {
        Self { settings: Settings::default(), zoom: 1.0, pan_step: 1.0, ..Self::default() }
    }

    /// `Update`: the pose to write over the root's `current` this frame, if any, orbiting `center`.
    pub fn update(&mut self, current: Pose, center: P3) -> Option<Pose> {
        if !self.active() {
            if self.written.is_none() || self.changed(current) {
                self.base = Some(current);
            }
            return self.release();
        }
        let base = *self.base.get_or_insert(current);
        self.transform(base, [center[0], center[1], base.position[2]])
    }

    /// `Release`: the base to write back when the controller had moved the root.
    pub fn release(&mut self) -> Option<Pose> {
        self.written.take()?;
        self.base.take()
    }

    /// `CancelInteraction`: drops a drag in progress.
    pub fn cancel_interaction(&mut self) {
        self.rotating = false;
        self.panning = false;
    }

    pub fn reset(&mut self) {
        *self = Self { settings: self.settings, base: self.base, written: self.written, pan_step: self.pan_step, ..Self::new() };
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.rotating || self.panning || self.yaw.abs() > 0.0001 || self.pitch.abs() > 0.0001 || (self.zoom - 1.0).abs() > 0.0001 || point::dot(self.pan_offset, self.pan_offset) > 0.0001
    }

    /// The frame's mouse: buttons in order, then the movement.
    pub fn input(&mut self, mouse: &GameMouse) {
        for &(button, down) in &mouse.buttons {
            self.button(button, down);
        }
        if mouse.dx != 0 || mouse.dy != 0 {
            self.moved(mouse.dx, mouse.dy);
        }
    }

    /// `MouseButtonEvent`; true when the controller used it.
    pub fn button(&mut self, button: u32, down: bool) -> bool {
        let s = self.settings;
        if button == s.zoom_in_button || button == s.zoom_out_button {
            if down {
                let step = if button == s.zoom_in_button { 1.0 / s.zoom_step } else { s.zoom_step };
                self.zoom = (self.zoom * step).clamp(s.min_zoom, s.max_zoom);
            }
            return true;
        }
        if button != s.pan_button && button != s.rotate_button {
            return false;
        }
        let pan = button == s.pan_button;
        let dragging = if pan { self.panning } else { self.rotating };
        if down && !dragging {
            if pan { self.panning = true } else { self.rotating = true }
            self.drag_distance = 0.0;
            return true;
        }
        if dragging && !down {
            if pan {
                self.panning = false;
            } else {
                self.rotating = false;
            }
            if pan && self.drag_distance < s.reset_click_slop {
                self.reset();
            }
        }
        dragging
    }

    /// `MouseMoveEvent`; true when a drag used it.
    #[allow(clippy::cast_precision_loss)]
    pub fn moved(&mut self, dx: i32, dy: i32) -> bool {
        if !self.rotating && !self.panning {
            return false;
        }
        let (dx, dy) = (dx as f32, dy as f32);
        self.drag_distance += dx.abs() + dy.abs();
        let s = self.settings;
        if self.rotating {
            self.yaw = remainder(self.yaw + dx * s.rotate_sensitivity, core::f32::consts::TAU);
            self.pitch = (self.pitch + dy * s.rotate_sensitivity).clamp(-s.pitch_limit, s.pitch_limit);
        }
        if self.panning {
            let right = right_from_yaw(self.look_yaw);
            self.pan_offset = point::add(self.pan_offset, point::scale(right, dx * self.pan_step));
            self.pan_offset[2] += dy * self.pan_step;
            let len = point::length(self.pan_offset);
            if len > s.max_pan_distance {
                self.pan_offset = point::scale(self.pan_offset, s.max_pan_distance / len);
            }
        }
        true
    }

    fn changed(&self, current: Pose) -> bool {
        let Some(written) = self.written else { return true };
        let moved = point::sub(current.position, written.position);
        if point::dot(moved, moved) > POSITION_THRESHOLD * POSITION_THRESHOLD {
            return true;
        }
        1.0 - Quat::from_matrix(&current.rotation).dot(Quat::from_matrix(&written.rotation)).abs() > ROTATION_THRESHOLD
    }

    fn transform(&mut self, base: Pose, pivot: P3) -> Option<Pose> {
        let s = self.settings;
        let mut offset = [base.position[0] - pivot[0], base.position[1] - pivot[1], 0.0];
        let mut distance = point::length(offset);
        if distance < 1.0 {
            return None;
        }
        if distance < s.min_pivot_distance {
            offset = point::scale(offset, s.min_pivot_distance / distance);
            distance = s.min_pivot_distance;
        }
        let camera_distance = (distance * self.zoom).clamp(s.min_camera_distance, s.max_camera_distance);
        self.look_yaw = (-offset[0]).atan2(-offset[1]) + self.yaw;
        self.pan_step = pan_step(camera_distance);
        let yaw = Quat::from_axis_angle([0.0, 0.0, 1.0], -self.yaw);
        let yawed = yaw.rotate(offset);
        let pitch = Quat::from_axis_angle([yawed[1] / distance, -yawed[0] / distance, 0.0], self.pitch);
        let delta = (pitch * yaw).normalized().matrix();
        let position = point::add(point::add(pivot, point::scale(point::mul_vec(&delta, offset), camera_distance / distance)), self.pan_offset);
        let pose = Pose { position, rotation: point::mul(&delta, &base.rotation) };
        self.written = Some(pose);
        Some(pose)
    }
}

#[cfg(test)]
mod tests {
    use super::{OrbitController, Pose};
    use crate::util::point::{self, IDENTITY};

    fn behind() -> Pose {
        Pose { position: [0.0, -100.0, 50.0], rotation: IDENTITY }
    }

    #[test]
    fn idle_it_keeps_the_engine_camera() {
        let mut o = OrbitController::new();
        assert_eq!(o.update(behind(), [0.0; 3]), None);
        assert!(!o.active());
    }

    #[test]
    fn a_right_drag_orbits_at_the_same_distance_and_height_and_releases_to_the_base() {
        let mut o = OrbitController::new();
        assert!(o.button(1, true));
        assert!(o.moved(314, 0), "a half turn at 0.01 rad per count");
        let pose = o.update(behind(), [0.0; 3]).unwrap();
        assert!((pose.position[1] - 100.0).abs() < 0.2 && pose.position[0].abs() < 0.2, "{pose:?}: now in front");
        assert!((pose.position[2] - 50.0).abs() < 1e-3, "the pivot sits at the camera's height");
        let facing = point::mul_vec(&pose.rotation, [0.0, 1.0, 0.0]);
        assert!(facing[1] < -0.99, "{facing:?}: turned to look back at the player");
        o.button(1, false);
        assert!(o.active(), "the orbit stays where it was left");
        assert!(o.button(2, true));
        o.button(2, false);
        assert!(!o.active(), "a middle click without a drag resets");
        assert_eq!(o.update(pose, [0.0; 3]), Some(behind()), "and writes the engine's pose back once");
        assert_eq!(o.update(behind(), [0.0; 3]), None);
    }

    #[test]
    fn the_wheel_zooms_within_its_limits_and_the_pitch_is_capped() {
        let mut o = OrbitController::new();
        for _ in 0..100 {
            o.button(9, true);
        }
        let far = o.update(behind(), [0.0; 3]).unwrap();
        assert!((point::length([far.position[0], far.position[1], 0.0]) - 500.0).abs() < 0.1, "5x zoom of 100");
        o.button(1, true);
        o.moved(0, 10_000);
        let low = o.update(behind(), [0.0; 3]).unwrap();
        assert!(low.position[2] > 400.0, "{low:?}: dragging down raises the camera over the player, at most 1.35 rad");
        assert!(o.moved(0, 1));
        o.button(1, false);
        assert!(!o.moved(0, 1), "released, the mouse moves nothing");
    }
}
