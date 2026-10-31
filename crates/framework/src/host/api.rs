//! The host's [`HostApi`] entries: each finds its plugin by the table it is called with and
//! forwards to the host's registries.

use core::ffi::{c_char, c_void};

use overlay::{Hotkey, Scope};
use sre::InputDevice;

use super::{FRAMES, TABLE, alias, filters, gui, interface, keys};
use crate::abi::{API, Alloc, FrameCallback, HostApi, MessageCallback, PapyrusCallback, PluginRecord, RawButton, RawError, RawMouse, RawPad, RawPress, RawSetting, Sink, Status, Str, fail, slice};
use crate::hooks::Hooks;
use crate::{Error, Legacy, Method, settings};

/// The table for the plugin `name`, made once and kept for every generation of it.
pub(super) fn build(name: &'static str, skse: *const skse::Interface) -> &'static HostApi {
    let legacy: &'static Legacy = Box::leak(Box::new(Legacy { table: &TABLE, hooks: Hooks::for_plugin(name) }));
    Box::leak(Box::new(HostApi {
        version: API,
        size: u32::try_from(size_of::<HostApi>()).expect("the table is small"),
        name: Str::from(name),
        skse,
        on_message: Some(on_message),
        on_plugin_message: Some(on_plugin_message),
        serve_alias: Some(serve_alias),
        alias_send: Some(alias_send),
        export_interface: Some(export_interface),
        on_frame: Some(on_frame),
        on_papyrus: Some(on_papyrus),
        action: Some(action),
        fired: Some(fired),
        key: Some(key),
        held: Some(held),
        released: Some(released),
        key_held: Some(key_held),
        key_released: Some(key_released),
        mouse: Some(mouse),
        gamepad: Some(gamepad),
        input_filter: Some(input_filter),
        set_filter_active: Some(set_filter_active),
        filter_active: Some(filter_active),
        setting: Some(setting),
        set_setting: Some(set_setting),
        setting_keys: Some(setting_keys),
        remove_settings: Some(remove_settings),
        set_tab_title: Some(set_tab_title),
        show_tab: Some(show_tab),
        show_section: Some(show_section),
        read_frame: Some(read_frame),
        plugins: Some(plugins),
        legacy,
    }))
}

/// The plugin a table belongs to; its name was leaked with it.
fn owner(api: *const HostApi) -> &'static str {
    unsafe { (*api).name.as_str() }
}

/// Writes a registration's handle to `out`, or reports why there is none.
fn report(result: Result<usize, Error>, out: *mut usize, error: &Sink<'_, RawError>) -> u32 {
    match result {
        Ok(handle) => {
            unsafe { out.write(handle) };
            Status::Ok as u32
        }
        Err(e) => fail(&e, error),
    }
}

/// Writes `value` to `out` when there is one.
fn put<T>(value: Option<T>, out: *mut T) -> bool {
    value.map(|v| unsafe { out.write(v) }).is_some()
}

unsafe extern "C" fn on_message(api: *const HostApi, f: MessageCallback) {
    super::on_message(owner(api), f);
}

unsafe extern "C" fn on_plugin_message(api: *const HostApi, sender: Str, f: MessageCallback) {
    super::on_plugin_message(owner(api), unsafe { sender.as_str() }, f);
}

unsafe extern "C" fn serve_alias(api: *const HostApi, name: Str, f: MessageCallback, slot: *mut usize, error: Sink<'_, RawError>) -> u32 {
    report(super::serve_alias(owner(api), unsafe { name.as_str() }, f), slot, &error)
}

unsafe extern "C" fn alias_send(_: *const HostApi, slot: usize, receiver: *const c_char, kind: u32, data: *mut c_void, len: u32) -> bool {
    alias::send(slot, receiver, kind, data, len)
}

unsafe extern "C" fn export_interface(api: *const HostApi, name: Str, methods: *const Method, len: usize, object: *mut usize, error: Sink<'_, RawError>) -> u32 {
    report(interface::export(owner(api), unsafe { name.as_str() }, unsafe { slice(methods, len) }), object, &error)
}

unsafe extern "C" fn on_frame(api: *const HostApi, f: FrameCallback) {
    FRAMES.add(owner(api), f);
}

unsafe extern "C" fn on_papyrus(api: *const HostApi, f: PapyrusCallback) {
    super::on_papyrus(owner(api), f);
}

unsafe extern "C" fn action(api: *const HostApi, name: Str, default: Str, scope: u32) -> usize {
    let scope = match scope {
        s if s == Scope::Overlay as u32 => Scope::Overlay,
        s if s == Scope::Game as u32 => Scope::Game,
        _ => return usize::MAX,
    };
    keys::declare(owner(api), unsafe { name.as_str() }, unsafe { default.as_str() }, scope)
}

unsafe extern "C" fn fired(_: *const HostApi, index: usize) -> bool {
    keys::fired(index)
}

unsafe extern "C" fn key(_: *const HostApi, index: usize, out: *mut Hotkey) -> bool {
    put(keys::key(index), out)
}

unsafe extern "C" fn held(_: *const HostApi, index: usize) -> bool {
    keys::held(index)
}

unsafe extern "C" fn released(_: *const HostApi, index: usize, secs: *mut f32) -> bool {
    put(keys::released(index), secs)
}

unsafe extern "C" fn key_held(_: *const HostApi, scan_code: u32) -> bool {
    overlay::game_key_held(scan_code)
}

unsafe extern "C" fn key_released(_: *const HostApi, scan_code: u32, secs: *mut f32) -> bool {
    put(overlay::game_key_released(scan_code), secs)
}

unsafe extern "C" fn mouse(_: *const HostApi, out: *mut RawMouse, presses: Sink<'_, RawPress>) {
    let mouse = overlay::game_mouse();
    unsafe { out.write(RawMouse { dx: mouse.dx, dy: mouse.dy }) };
    mouse.buttons.into_iter().for_each(|(id, pressed)| presses.put(RawPress { id, pressed }));
}

unsafe extern "C" fn gamepad(_: *const HostApi, out: *mut RawPad, presses: Sink<'_, RawPress>) {
    let pad = overlay::game_pad();
    unsafe { out.write(RawPad { held: pad.held, triggers: pad.triggers, sticks: pad.sticks }) };
    pad.buttons.into_iter().for_each(|(id, pressed)| presses.put(RawPress { id, pressed }));
}

unsafe extern "C" fn input_filter(api: *const HostApi, name: Str, user_events: *const Str, events_len: usize, buttons: *const RawButton, buttons_len: usize) -> usize {
    let user_events: Vec<&str> = unsafe { slice(user_events, events_len) }.iter().map(|e| unsafe { e.as_str() }).collect();
    let buttons: Vec<(InputDevice, u32)> = unsafe { slice(buttons, buttons_len) }.iter().filter_map(|b| Some((InputDevice::from_u32(b.device)?, b.id))).collect();
    filters::declare(owner(api), unsafe { name.as_str() }, &user_events, &buttons)
}

unsafe extern "C" fn set_filter_active(_: *const HostApi, index: usize, active: bool) {
    filters::set_active(index, active);
}

unsafe extern "C" fn filter_active(_: *const HostApi, index: usize) -> bool {
    filters::is_active(index)
}

unsafe extern "C" fn setting(api: *const HostApi, key: Str, value: Sink<'_, RawSetting>) {
    if let Some(found) = settings::get(owner(api), unsafe { key.as_str() }) {
        value.put(RawSetting::new(&found));
    }
}

unsafe extern "C" fn set_setting(api: *const HostApi, key: Str, value: RawSetting) {
    if let Some(value) = unsafe { value.to_setting() } {
        settings::set(owner(api), unsafe { key.as_str() }, value);
    }
}

unsafe extern "C" fn setting_keys(api: *const HostApi, prefix: Str, keys: Sink<'_, Str>) {
    for k in settings::keys(owner(api), unsafe { prefix.as_str() }) {
        keys.put(Str::from(k.as_str()));
    }
}

unsafe extern "C" fn remove_settings(api: *const HostApi, prefix: Str) -> usize {
    settings::remove(owner(api), unsafe { prefix.as_str() })
}

unsafe extern "C" fn set_tab_title(api: *const HostApi, title: Str) {
    gui::set_title(owner(api), unsafe { title.as_str() });
}

unsafe extern "C" fn show_tab(api: *const HostApi) {
    gui::show_tab(owner(api));
}

unsafe extern "C" fn show_section(api: *const HostApi, name: Str) {
    gui::show_section(owner(api), unsafe { name.as_str() });
}

unsafe extern "C" fn read_frame(_: *const HostApi, alloc: Alloc<'_>, error: Sink<'_, Str>) -> bool {
    match overlay::read_back() {
        Ok(frame) => {
            let out = alloc.get(frame.width, frame.height);
            let len = frame.rgba.len().min(frame.width * frame.height * 4);
            if !out.is_null() {
                unsafe { core::ptr::copy_nonoverlapping(frame.rgba.as_ptr(), out, len) };
            }
            !out.is_null()
        }
        Err(e) => {
            error.put(Str::from(e.as_str()));
            false
        }
    }
}

unsafe extern "C" fn plugins(_: *const HostApi, records: Sink<'_, PluginRecord>, before: Sink<'_, Str>, after: Sink<'_, Str>) {
    for p in super::plugins() {
        records.put(PluginRecord { name: Str::from(p.name.as_str()), version: Str::from(p.version.as_str()), file: Str::from(p.file.as_str()), priority: p.constraints.priority });
        p.constraints.before.iter().for_each(|n| before.put(Str::from(n.as_str())));
        p.constraints.after.iter().for_each(|n| after.put(Str::from(n.as_str())));
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::abi::{Api, Callback, call, entry, names, status};
    use crate::settings::{Setting, Settings};
    use crate::{Action, BUILD, Host};

    unsafe extern "C" fn query(_: u32) -> *mut c_void {
        core::ptr::null_mut()
    }
    unsafe extern "C" fn handle() -> skse::PluginHandle {
        0
    }
    unsafe extern "C" fn release() -> u32 {
        75
    }
    unsafe extern "C" fn info(_: *const c_char) -> *const skse::PluginInfo {
        core::ptr::null()
    }
    static SKSE: skse::Interface = skse::Interface { skse_version: 0x0203_0010, runtime_version: 0x0107_0680, reserved: [0; 2], query_interface: query, get_plugin_handle: handle, get_release_index: release, get_plugin_info: info };

    fn open(owner: &str) {
        assert!(settings::open(owner, &std::env::temp_dir().join(format!("skyrim-rs-abi-{}", std::process::id()))).is_ok());
    }

    crate::plugin! {
        name: "abi-test-plugin",
        load: load,
        load_before: ["first", "second"],
        load_after: ["*"],
        priority: -3,
        save_state: save_state,
        restore_state: restore_state,
    }

    static RESTORED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    fn load(host: &'static Host) -> bool {
        host.settings().set("loaded", host.name());
        let _tab = host.add_tab("ABI test");
        host.name() == "abi-test-plugin" && host.skse().runtime_version == 0x0107_0680 && !host.key_held(0x10)
    }

    fn save_state() -> Vec<u8> {
        vec![1, 2, 3]
    }

    fn restore_state(state: &[u8]) {
        state.clone_into(&mut RESTORED.lock().unwrap());
    }

    #[test]
    fn a_plugin_descriptor_reads_back_and_loads_through_its_table() {
        let d = &SkyrimRs_Plugin;
        assert_eq!((d.abi, d.api), (crate::ABI, API));
        assert_eq!(unsafe { core::ffi::CStr::from_ptr(d.build) }, BUILD);
        let plugin = super::super::describe(d, "abi-test.dll".to_owned());
        assert_eq!((plugin.name.as_str(), plugin.file.as_str(), plugin.version.as_str()), ("abi-test-plugin", "abi-test.dll", env!("CARGO_PKG_VERSION")));
        assert_eq!((plugin.constraints.before, plugin.constraints.after, plugin.constraints.priority), (vec!["first".to_owned(), "second".to_owned()], vec!["*".to_owned()], -3));
        assert_eq!(unsafe { names(core::ptr::null()) }, Vec::<String>::new());
        assert!(d.unload.is_none());

        open("abi-test-plugin");
        assert!((d.load)(build("abi-test-plugin", &raw const SKSE)));
        assert_eq!(settings::get("abi-test-plugin", "loaded"), Some(Setting::Text("abi-test-plugin".to_owned())));

        let mut state = Vec::new();
        let mut keep = |s: Str| state.extend_from_slice(unsafe { s.bytes() });
        (d.save_state.unwrap())(Sink::new(&mut keep));
        assert_eq!(state, [1, 2, 3]);
        (d.restore_state.unwrap())(state.as_ptr(), state.len());
        assert_eq!(*RESTORED.lock().unwrap(), [1, 2, 3]);
    }

    #[test]
    fn an_older_loader_answers_for_what_its_table_lacks() {
        let full = build("abi-test-older", &raw const SKSE);
        open("abi-test-older");
        let older = HostApi { size: u32::try_from(core::mem::offset_of!(HostApi, serve_alias)).unwrap(), ..*full };
        let (full, older) = (Api(full), Api(&raw const older));
        assert!(entry!(older, on_plugin_message).is_some());
        assert!(entry!(older, serve_alias).is_none() && entry!(older, action).is_none() && entry!(older, plugins).is_none());
        assert!(Host::new(older.0).is_none(), "a stage 1 plugin needs the legacy registrations");
        assert!(Host::new(full.0).is_some());

        Settings::new(full).set("scale", 1.5);
        assert_eq!(Settings::new(full).get::<f64>("scale"), Some(1.5));
        assert_eq!(Settings::new(full).keys(""), ["scale"]);
        assert_eq!(Settings::new(older).get::<f64>("scale"), None);
        assert_eq!(Settings::new(older).keys(""), Vec::<String>::new());
        assert_eq!(Settings::new(older).remove(""), 0);
        Settings::new(older).set("scale", 3.0);
        assert_eq!(settings::get("abi-test-older", "scale"), Some(Setting::Number(1.5)), "a set the loader lacks does nothing");

        let action = Action { index: 0, api: older };
        assert!(!action.fired() && !action.held());
        assert_eq!((action.key(), action.released()), (None, None));
        let mut slot = 0;
        let refused = status("serving a name", |error| call!(older, serve_alias(Str::from("Name"), crate::abi::message(|_| {}), &raw mut slot, error)));
        assert!(matches!(refused, Err(Error::Unsupported { .. })));
    }

    #[test]
    fn the_host_drops_a_callback_when_it_detaches_the_plugin() {
        static DROPPED: AtomicU32 = AtomicU32::new(0);
        extern "C" fn tick(_: *mut c_void) {}
        extern "C" fn dropped(_: *mut c_void) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        let api = build("abi-test-drop", &raw const SKSE);
        unsafe { on_frame(api, Callback { call: tick, ctx: core::ptr::null_mut(), drop: Some(dropped) }) };
        assert_eq!(DROPPED.load(Ordering::Relaxed), 0);
        assert!(super::super::detach("abi-test-drop").contains(", 1 frame callbacks"));
        assert_eq!(DROPPED.load(Ordering::Relaxed), 1);
    }
}
