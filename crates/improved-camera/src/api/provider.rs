//! `api/ImprovedCameraProvider` with the protocol of `api/ImprovedCameraAPI.h`: IC's modder API,
//! served under IC's SKSE plugin name through the host (`Host::serve_alias`, with the stub
//! `skyrim_rs_improved_camera_stub.dll` installed) so mods built against IC's header find it. A
//! consumer dispatches `RequestInterface` to `ImprovedCameraSE` and receives the interface object,
//! which the host owns (`Host::export_interface`) so it outlives a reload of this DLL. Camera and
//! crosshair ownership, the horse aim unlock and the event callbacks are held here and handed to
//! the next build.

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use framework::{Alias, Host, Method, Returns};
use skyrim::render::NiAVObjectExt;

use crate::camera::snapshot;
use crate::camera::state::CameraType;
use crate::util::skyrim_util as util;

/// `ImprovedCameraPluginName`.
pub const PLUGIN_NAME: &str = "ImprovedCameraSE";
/// `PluginCommand::header`.
const HEADER: u32 = 0x1C0F_CA50;
/// `kInvalidPluginHandle`.
const INVALID: u32 = u32::MAX;
/// `InterfaceVersion::V2`, the newest IC speaks.
const V2: u8 = 1;

/// `APIResult`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiResult {
    Ok,
    NotOwner,
    AlreadyOwned,
    AlreadyTaken,
    Invalid,
}

/// `PluginCommand`.
#[repr(C)]
struct PluginCommand {
    header: u32,
    kind: u8,
    command: *const u8,
}

/// `PluginResponse`: `Error` 0, `InterfaceProvider` 1.
#[repr(C)]
struct PluginResponse {
    kind: u8,
    data: *mut c_void,
}

const RESPONSE_SIZE: u32 = 16;
const _: () = assert!(size_of::<PluginResponse>() == RESPONSE_SIZE as usize);

/// `InterfaceContainer`.
#[repr(C)]
struct InterfaceContainer {
    instance: *const c_void,
    version: u8,
}

/// `CameraEvent`: `StateChanged` 0, `POVChanged` 1, then the endpoints and the first person after it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraEvent {
    kind: u32,
    previous: u32,
    current: u32,
    first_person: bool,
}

/// `CameraEventCallback`, which takes the event by reference.
type EventCallback = unsafe extern "C" fn(*const CameraEvent);

static ALIAS: OnceLock<Alias> = OnceLock::new();
/// The object the host exports, 0 until it does.
static INSTANCE: AtomicUsize = AtomicUsize::new(0);
static CAMERA_OWNER: AtomicU32 = AtomicU32::new(INVALID);
static CROSSHAIR_OWNER: AtomicU32 = AtomicU32::new(INVALID);
static UNLOCKED_HORSE_AIM: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// `m_povFirstPerson`: -1 until the first sample seeds it.
static POV: AtomicI8 = AtomicI8::new(-1);
static CALLBACKS: Mutex<Vec<(u32, usize)>> = Mutex::new(Vec::new());

/// `ToAPICameraType`, fixed: IC casts, but its own enum renamed the possession camera `Spectator`
/// and moved it after `RaceMenu`, so the API's `Possession` 23 and `RaceMenu` 24 are IC's
/// `Spectator` 24 and `RaceMenu` 23; every other type has the same value in both.
#[must_use]
pub const fn api_camera_type(camera: CameraType) -> u32 {
    match camera {
        CameraType::RaceMenu => 24,
        CameraType::Spectator => 23,
        other => other as u32,
    }
}

/// Serves IC's name and exports the interface object; a refusal is logged and the API is off.
pub fn install(host: &Host) {
    match host.serve_alias(PLUGIN_NAME, handle_message) {
        Ok(alias) => {
            let _ = ALIAS.set(alias);
        }
        Err(e) => crate::log(&format!("API: {e}")),
    }
    match host.export_interface("ImprovedCameraAPI", &methods()) {
        Ok(object) => INSTANCE.store(object as usize, Ordering::Release),
        Err(e) => crate::log(&format!("API: {e}")),
    }
}

/// `IVImprovedCamera1` then `IVImprovedCamera2`, in declaration order.
fn methods() -> [Method; 18] {
    use Returns::{Buffer, Float, Value};
    [
        Method::new(get_thread_id as *const (), Value),
        Method::new(request_camera_control as *const (), Value),
        Method::new(release_camera_control as *const (), Value),
        Method::new(get_camera_owner as *const (), Value),
        Method::new(request_crosshair_control as *const (), Value),
        Method::new(release_crosshair_control as *const (), Value),
        Method::new(get_crosshair_owner as *const (), Value),
        Method::new(is_camera_enabled as *const (), Value),
        Method::new(get_last_camera_position as *const (), Buffer),
        Method::new(enable_unlocked_horse_aim as *const (), Value),
        Method::new(get_camera_type as *const (), Value),
        Method::new(get_previous_camera_type as *const (), Value),
        Method::new(is_first_person as *const (), Value),
        Method::new(is_fake_first_person as *const (), Value),
        Method::new(is_first_person_body_enabled as *const (), Value),
        Method::new(get_fov as *const (), Float),
        Method::new(register_camera_event_callback as *const (), Value),
        Method::new(unregister_camera_event_callback as *const (), Value),
    ]
}

/// What `HandleMessage` answers an interface request with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reply {
    Interface(u8),
    /// A version newer than IC speaks.
    Error,
}

/// The answer to a message, `None` when it is not an interface request.
fn reply_kind(data: *const c_void, len: u32) -> Option<Reply> {
    if data.is_null() || len as usize != size_of::<PluginCommand>() {
        return None;
    }
    let command = unsafe { &*data.cast::<PluginCommand>() };
    if command.header != HEADER || command.kind != 0 {
        return None;
    }
    let version = unsafe { command.command.as_ref() }.copied().unwrap_or(0);
    Some(if version <= V2 { Reply::Interface(version) } else { Reply::Error })
}

/// `HandleMessage`: answers an interface request with the object, or with an error for a version
/// newer than IC speaks. The reply is read by the consumer during the dispatch.
fn handle_message(msg: &skse::Message) {
    let Some(reply) = reply_kind(msg.data, msg.data_len) else { return };
    let (Some(alias), false) = (ALIAS.get(), msg.sender.is_null()) else { return };
    let response = Box::leak(Box::new(match reply {
        Reply::Interface(version) => PluginResponse { kind: 1, data: core::ptr::from_mut(Box::leak(Box::new(InterfaceContainer { instance: INSTANCE.load(Ordering::Acquire) as *const c_void, version }))).cast() },
        Reply::Error => PluginResponse { kind: 0, data: core::ptr::null_mut() },
    }));
    let sent = alias.send(msg.sender, 0, core::ptr::from_mut(response).cast(), RESPONSE_SIZE);
    let who = unsafe { core::ffi::CStr::from_ptr(msg.sender.cast::<c_char>()) }.to_string_lossy();
    let what = match reply {
        Reply::Interface(version) => format!("interface V{} handed", version + 1),
        Reply::Error => "a newer interface version refused".to_owned(),
    };
    crate::log(&format!("API: {what} to {who}{}", if sent { "" } else { " (not delivered)" }));
}

fn acquire(owner: &AtomicU32, handle: u32) -> ApiResult {
    match owner.compare_exchange(INVALID, handle, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => ApiResult::Ok,
        Err(current) if current == handle => ApiResult::AlreadyOwned,
        Err(_) => ApiResult::AlreadyTaken,
    }
}

fn release(owner: &AtomicU32, handle: u32) -> ApiResult {
    if owner.compare_exchange(handle, INVALID, Ordering::AcqRel, Ordering::Acquire).is_ok() { ApiResult::Ok } else { ApiResult::NotOwner }
}

extern "C" fn get_thread_id(_: *const c_void) -> u32 {
    THREAD_ID.load(Ordering::Relaxed)
}

extern "C" fn request_camera_control(_: *const c_void, handle: u32) -> ApiResult {
    acquire(&CAMERA_OWNER, handle)
}

extern "C" fn release_camera_control(_: *const c_void, handle: u32) -> ApiResult {
    release(&CAMERA_OWNER, handle)
}

extern "C" fn get_camera_owner(_: *const c_void) -> u32 {
    CAMERA_OWNER.load(Ordering::Acquire)
}

extern "C" fn request_crosshair_control(_: *const c_void, handle: u32) -> ApiResult {
    acquire(&CROSSHAIR_OWNER, handle)
}

extern "C" fn release_crosshair_control(_: *const c_void, handle: u32) -> ApiResult {
    release(&CROSSHAIR_OWNER, handle)
}

extern "C" fn get_crosshair_owner(_: *const c_void) -> u32 {
    CROSSHAIR_OWNER.load(Ordering::Acquire)
}

/// IC answers false: its Follow camera is not on this branch.
extern "C" fn is_camera_enabled(_: *const c_void) -> bool {
    false
}

extern "C" fn get_last_camera_position(_: *const c_void, out: *mut [f32; 3]) -> *mut [f32; 3] {
    let position = util::camera().and_then(|c| c.camera_root()).map_or([0.0; 3], |root| root.world_transform().translation);
    unsafe { out.write(position) };
    out
}

extern "C" fn enable_unlocked_horse_aim(_: *const c_void, enable: bool) {
    UNLOCKED_HORSE_AIM.store(enable, Ordering::Relaxed);
}

extern "C" fn get_camera_type(_: *const c_void) -> u32 {
    api_camera_type(snapshot::get().current)
}

extern "C" fn get_previous_camera_type(_: *const c_void) -> u32 {
    api_camera_type(snapshot::get().previous)
}

extern "C" fn is_first_person(_: *const c_void) -> bool {
    util::is_first_person()
}

extern "C" fn is_fake_first_person(_: *const c_void) -> bool {
    util::is_fake_first_person()
}

extern "C" fn is_first_person_body_enabled(_: *const c_void) -> bool {
    crate::settings::profile().General.bEnableBody
}

extern "C" fn get_fov(_: *const c_void) -> f32 {
    snapshot::get().fov
}

fn register(callbacks: &mut Vec<(u32, usize)>, handle: u32, callback: usize) -> ApiResult {
    if callback == 0 {
        return ApiResult::Invalid;
    }
    if callbacks.iter().any(|(h, _)| *h == handle) {
        return ApiResult::AlreadyOwned;
    }
    callbacks.push((handle, callback));
    ApiResult::Ok
}

fn unregister(callbacks: &mut Vec<(u32, usize)>, handle: u32) -> ApiResult {
    let before = callbacks.len();
    callbacks.retain(|(h, _)| *h != handle);
    if callbacks.len() < before { ApiResult::Ok } else { ApiResult::NotOwner }
}

extern "C" fn register_camera_event_callback(_: *const c_void, handle: u32, callback: Option<EventCallback>) -> ApiResult {
    register(&mut CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner), handle, callback.map_or(0, |f| f as usize))
}

extern "C" fn unregister_camera_event_callback(_: *const c_void, handle: u32) -> ApiResult {
    unregister(&mut CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner), handle)
}

/// `DispatchCameraEvent`: outside the lock, so a callback may (un)register.
fn dispatch(event: &CameraEvent) {
    let callbacks = CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for (_, callback) in callbacks {
        let f: EventCallback = unsafe { core::mem::transmute::<usize, EventCallback>(callback) };
        unsafe { f(event) };
    }
}

/// `NotifyCameraState`, from `SetCamera`.
pub fn notify_camera_state(previous: CameraType, current: CameraType, first_person: bool) {
    dispatch(&CameraEvent { kind: 0, previous: api_camera_type(previous), current: api_camera_type(current), first_person });
}

/// `NotifyPOV`, every camera update: a flip of the effective first person, real or fake. The
/// update's thread is what `GetThreadId` reports.
pub fn notify_pov(current: CameraType, first_person: bool) {
    THREAD_ID.store(skyrim::thread_id(), Ordering::Relaxed);
    if let Some(event) = pov_event(&POV, current, first_person) {
        dispatch(&event);
    }
}

fn pov_event(latch: &AtomicI8, current: CameraType, first_person: bool) -> Option<CameraEvent> {
    let pov = i8::from(first_person);
    let previous = latch.swap(pov, Ordering::Relaxed);
    if previous == pov || previous < 0 {
        return None;
    }
    let camera = api_camera_type(current);
    Some(CameraEvent { kind: 1, previous: camera, current: camera, first_person })
}

/// What the next build takes over: the owners, the horse aim, the POV latch, the thread and the
/// consumers' callbacks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub camera_owner: u32,
    pub crosshair_owner: u32,
    pub unlocked_horse_aim: bool,
    pub pov: i8,
    pub thread_id: u32,
    pub callbacks: Vec<(u32, u64)>,
}

#[must_use]
pub fn saved() -> Saved {
    Saved {
        camera_owner: CAMERA_OWNER.load(Ordering::Acquire),
        crosshair_owner: CROSSHAIR_OWNER.load(Ordering::Acquire),
        unlocked_horse_aim: UNLOCKED_HORSE_AIM.load(Ordering::Relaxed),
        pov: POV.load(Ordering::Relaxed),
        thread_id: THREAD_ID.load(Ordering::Relaxed),
        callbacks: CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|&(h, f)| (h, f as u64)).collect(),
    }
}

pub fn restore(saved: &Saved) {
    CAMERA_OWNER.store(saved.camera_owner, Ordering::Release);
    CROSSHAIR_OWNER.store(saved.crosshair_owner, Ordering::Release);
    UNLOCKED_HORSE_AIM.store(saved.unlocked_horse_aim, Ordering::Relaxed);
    POV.store(saved.pov, Ordering::Relaxed);
    THREAD_ID.store(saved.thread_id, Ordering::Relaxed);
    *CALLBACKS.lock().unwrap_or_else(PoisonError::into_inner) = saved.callbacks.iter().filter_map(|&(h, f)| Some((h, usize::try_from(f).ok()?))).collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_names_every_camera_as_its_header_does() {
        assert_eq!(api_camera_type(CameraType::MapMenu), 22);
        assert_eq!(api_camera_type(CameraType::Spectator), 23, "Possession");
        assert_eq!(api_camera_type(CameraType::RaceMenu), 24);
        assert_eq!(api_camera_type(CameraType::VampirelordTransition), 25);
        assert_eq!(api_camera_type(CameraType::Unknown), 34);
        for value in (0..=34).filter(|v| ![23, 24].contains(v)) {
            assert_eq!(api_camera_type(CameraType::from_u32(value)), value);
        }
    }

    #[test]
    fn the_records_have_the_header_layout() {
        assert_eq!(size_of::<PluginCommand>(), 16);
        assert_eq!(size_of::<InterfaceContainer>(), 16);
        assert_eq!(size_of::<CameraEvent>(), 16);
        assert_eq!(core::mem::offset_of!(CameraEvent, first_person), 12);
    }

    #[test]
    fn only_an_interface_request_is_answered() {
        let ask = |header, kind, version: Option<u8>| {
            let version = version.map(Box::new);
            let command = PluginCommand { header, kind, command: version.as_deref().map_or(core::ptr::null(), core::ptr::from_ref) };
            reply_kind((&raw const command).cast(), 16)
        };
        assert_eq!(ask(HEADER, 0, Some(1)), Some(Reply::Interface(1)));
        assert_eq!(ask(HEADER, 0, None), Some(Reply::Interface(0)), "no request structure is V1");
        assert_eq!(ask(HEADER, 0, Some(2)), Some(Reply::Error), "a newer version gets an error");
        assert_eq!(ask(HEADER + 1, 0, Some(1)), None);
        assert_eq!(ask(HEADER, 1, Some(1)), None);
        assert_eq!(reply_kind(core::ptr::null(), 16), None);
        let command = PluginCommand { header: HEADER, kind: 0, command: core::ptr::null() };
        assert_eq!(reply_kind((&raw const command).cast(), 8), None, "FSMP's and other broadcasts pass by");
    }

    #[test]
    fn ownership_is_first_come_and_released_only_by_its_owner() {
        let owner = AtomicU32::new(INVALID);
        assert_eq!(acquire(&owner, 7), ApiResult::Ok);
        assert_eq!(acquire(&owner, 7), ApiResult::AlreadyOwned);
        assert_eq!(acquire(&owner, 9), ApiResult::AlreadyTaken);
        assert_eq!(release(&owner, 9), ApiResult::NotOwner);
        assert_eq!(release(&owner, 7), ApiResult::Ok);
        assert_eq!(owner.load(Ordering::Relaxed), INVALID);
    }

    #[test]
    fn one_callback_per_handle() {
        let mut callbacks = Vec::new();
        assert_eq!(register(&mut callbacks, 3, 0), ApiResult::Invalid);
        assert_eq!(register(&mut callbacks, 3, 0x1000), ApiResult::Ok);
        assert_eq!(register(&mut callbacks, 3, 0x2000), ApiResult::AlreadyOwned);
        assert_eq!(unregister(&mut callbacks, 4), ApiResult::NotOwner);
        assert_eq!(unregister(&mut callbacks, 3), ApiResult::Ok);
        assert_eq!(callbacks, []);
    }

    #[test]
    fn the_pov_event_fires_on_a_flip_after_the_first_sample() {
        let latch = AtomicI8::new(-1);
        assert_eq!(pov_event(&latch, CameraType::FirstPerson, true), None, "the first sample seeds");
        assert_eq!(pov_event(&latch, CameraType::FirstPerson, true), None);
        assert_eq!(pov_event(&latch, CameraType::ThirdPerson, false), Some(CameraEvent { kind: 1, previous: 9, current: 9, first_person: false }));
        assert_eq!(pov_event(&latch, CameraType::Spectator, true), Some(CameraEvent { kind: 1, previous: 23, current: 23, first_person: true }));
    }
}
