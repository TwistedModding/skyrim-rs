//! `camera/CameraSnapshot`: the camera manager's state of the last update, read by the body and
//! the render step without the manager's lock.

use std::sync::Mutex;

use super::state::{CameraMode, CameraType};
use crate::util::point::{IDENTITY, M3, P3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Data {
    pub current: CameraType,
    pub previous: CameraType,
    /// The presented camera's mode.
    pub mode: CameraMode,
    /// The type a transition blends from, `Unknown` when none runs (`transitionID`).
    pub transition: CameraType,
    /// The presented camera root's local position and rotation.
    pub position: P3,
    pub rotation: M3,
}

impl Data {
    pub const NONE: Self = Self { current: CameraType::Unknown, previous: CameraType::Unknown, mode: CameraMode::None, transition: CameraType::Unknown, position: [0.0; 3], rotation: IDENTITY };
}

static SNAPSHOT: Mutex<Data> = Mutex::new(Data::NONE);

pub fn set(data: Data) {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = data;
}

#[must_use]
pub fn get() -> Data {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
