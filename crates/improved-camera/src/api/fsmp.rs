//! `api/FSMPAPI`: Faster HDT-SMP's plugin interface, which it dispatches as message 0 from the
//! SKSE plugin `hdtsmp64`. From interface 2.1 on, IC declares the player's body camera-driven
//! while the anchor holds it to the camera, and FSMP then simulates the player in body space.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const SENDER: &str = "hdtsmp64";
const MSG_STARTUP: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct Version {
    major: i32,
    minor: i32,
    patch: i32,
}

#[repr(C)]
struct VersionInfo {
    interface: Version,
    bullet: Version,
}

/// The interface's virtual table: the destructor, `getVersionInfo`, the four listener slots, then
/// `setPlayerBodyCameraDriven` (since 2.1).
#[repr(C)]
struct Vtable {
    dtor: usize,
    version_info: unsafe extern "C" fn(*const Interface) -> *const VersionInfo,
    listeners: [usize; 4],
    set_player_body_camera_driven: unsafe extern "C" fn(*const Interface, bool),
}

#[repr(C)]
struct Interface {
    vtable: *const Vtable,
}

static INTERFACE: AtomicUsize = AtomicUsize::new(0);
static CAMERA_DRIVEN: AtomicBool = AtomicBool::new(false);

/// Whether an interface of this version takes the camera-driven declaration.
const fn supported(version: Version) -> bool {
    version.major == 2 && version.minor >= 1
}

fn interface() -> Option<&'static Interface> {
    unsafe { (INTERFACE.load(Ordering::Acquire) as *const Interface).as_ref() }
}

/// `HandleMessage`: keeps FSMP's interface when it is new enough, and replays the declaration
/// the anchor may have made before it arrived.
pub fn handle_message(msg: &skse::Message) {
    if msg.kind != MSG_STARTUP || msg.data.is_null() {
        return;
    }
    let fsmp = msg.data.cast::<Interface>();
    let version = unsafe { (*((*(*fsmp).vtable).version_info)(fsmp)).interface };
    if !supported(version) {
        crate::log(&format!("FSMP interface v{}.{}.{} found, camera-driven body declaration needs v2.1+", version.major, version.minor, version.patch));
        return;
    }
    INTERFACE.store(fsmp as usize, Ordering::Release);
    if CAMERA_DRIVEN.load(Ordering::Relaxed) {
        unsafe { ((*(*fsmp).vtable).set_player_body_camera_driven)(fsmp, true) };
    }
    crate::log(&format!("FSMP interface v{}.{}.{} found", version.major, version.minor, version.patch));
}

/// `SetPlayerBodyCameraDriven`: only a change reaches FSMP; before its interface arrives the
/// state is kept for the replay.
pub fn set_player_body_camera_driven(active: bool) {
    if CAMERA_DRIVEN.swap(active, Ordering::Relaxed) == active {
        return;
    }
    if let Some(fsmp) = interface() {
        unsafe { ((*fsmp.vtable).set_player_body_camera_driven)(fsmp, active) };
    }
}

/// The interface the old build kept, for the handoff (0 for none).
pub fn saved() -> u64 {
    INTERFACE.load(Ordering::Acquire) as u64
}

pub fn restore(interface: u64) {
    INTERFACE.store(usize::try_from(interface).unwrap_or(0), Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{Version, supported};

    #[test]
    fn the_declaration_needs_interface_2_1() {
        let v = |major, minor| Version { major, minor, patch: 0 };
        assert!(supported(v(2, 1)) && supported(v(2, 4)));
        assert!(!supported(v(2, 0)) && !supported(v(1, 9)) && !supported(v(3, 0)));
    }
}
