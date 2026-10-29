//! `camera/CameraSnapshot`: the camera manager's state of the last update, read by the body and
//! the render step without the manager's lock.

use std::sync::Mutex;

use super::state::CameraType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Data {
    pub current: CameraType,
    pub previous: CameraType,
}

static SNAPSHOT: Mutex<Data> = Mutex::new(Data { current: CameraType::Unknown, previous: CameraType::Unknown });

pub fn set(data: Data) {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = data;
}

#[must_use]
pub fn get() -> Data {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
