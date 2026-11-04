//! The host's [`HostApi`] entries: each finds its plugin by the table it is called with and
//! forwards to the host's registries, the hook dispatcher's among them.

use core::ffi::{c_char, c_void};
use core::mem::ManuallyDrop;

use overlay::{Hotkey, Scope};
use skyrim::events::ErasedSource;
use skyrim::input::ErasedExecute;
use skyrim::papyrus::{ErasedNativeFn, NativeCall, NativeFunction, Variable};
use sre::InputDevice;

use super::{FRAMES, alias, filters, gui, interface, keys};
use crate::abi::{self, API, Alloc, CommandCallback, Crossing, FrameCallback, HostApi, MessageCallback, NativeCallback, PapyrusCallback, PluginRecord, RawArg, RawButton, RawCommandContext, RawCommandRequest, RawCommandSlot, RawError, RawMouse, RawNativeCall, RawNativeRequest, RawPad, RawPress, RawSetting, Record, Sink, SinkCallback, Status, Str, c_str, read_sized, slice};
use crate::hooks::raw::{AfterCallback, AroundCallback, BeforeCallback, RawPatch, RawPatchReport, RawSiteReport, RawSubscription, ReplaceCallback, put_site};
use crate::hooks::{self, PatchId, Subscriber, SubscriptionId, registry};
use crate::ui::{RawFamily, RawFont, UiCallback};
use crate::{Error, Method, settings};

/// The table for the plugin `name`, made once and kept for every generation of it.
pub(super) fn build(name: &'static str, skse: *const skse::Interface) -> &'static HostApi {
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
        legacy: core::ptr::null(),
        add_sink: Some(add_sink),
        add_player_graph_sink: Some(add_player_graph_sink),
        register_native: Some(register_native),
        register_command: Some(register_command),
        replace_command: Some(replace_command),
        hook_before: Some(hook_before),
        hook_around: Some(hook_around),
        hook_replace: Some(hook_replace),
        hook_after: Some(hook_after),
        hook_unsubscribe: Some(hook_unsubscribe),
        hook_remove_owner: Some(hook_remove_owner),
        hook_set_enabled: Some(hook_set_enabled),
        hook_diagnostics: Some(hook_diagnostics),
        hook_messages: Some(hook_messages),
        hook_patch: Some(hook_patch),
        hook_set_patch_enabled: Some(hook_set_patch_enabled),
        hook_unpatch: Some(hook_unpatch),
        hook_patches: Some(hook_patches),
        add_section: Some(add_section),
        add_hud: Some(add_hud),
        fonts: Some(fonts),
    }))
}

/// The plugin a table belongs to; its name was leaked with it.
fn owner(api: *const HostApi) -> &'static str {
    unsafe { (*api).name.as_str() }
}

/// Writes a registration's handle to `out`, or reports why there is none.
fn report<T, E: Crossing>(result: Result<T, E>, out: *mut T, error: &Sink<'_, *const RawError>) -> u32 {
    match result {
        Ok(handle) => {
            unsafe { out.write(handle) };
            Status::Ok as u32
        }
        Err(e) => e.fail(error),
    }
}

/// A plugin's request as its writer laid it out; a null one is refused.
fn read<T: Record, E: Crossing>(request: *const T) -> Result<T, E> {
    unsafe { read_sized(request) }.ok_or_else(|| E::unsupported("a null request".to_owned()))
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

unsafe extern "C" fn serve_alias(api: *const HostApi, name: Str, f: MessageCallback, slot: *mut usize, error: Sink<'_, *const RawError>) -> u32 {
    report(super::serve_alias(owner(api), unsafe { name.as_str() }, f), slot, &error)
}

unsafe extern "C" fn alias_send(_: *const HostApi, slot: usize, receiver: *const c_char, kind: u32, data: *mut c_void, len: u32) -> bool {
    alias::send(slot, receiver, kind, data, len)
}

unsafe extern "C" fn export_interface(api: *const HostApi, name: Str, methods: *const Method, len: usize, object: *mut usize, error: Sink<'_, *const RawError>) -> u32 {
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
    let buttons: Vec<(InputDevice, u32)> = unsafe { slice(buttons, buttons_len) }.iter().filter_map(|b| Some((InputDevice::from_engine(b.device)?, b.id))).collect();
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

unsafe extern "C" fn plugins(_: *const HostApi, records: Sink<'_, *const PluginRecord>, before: Sink<'_, Str>, after: Sink<'_, Str>) {
    for p in super::plugins() {
        records.put(&PluginRecord { size: abi::size::<PluginRecord>(), name: Str::from(p.name.as_str()), version: Str::from(p.version.as_str()), file: Str::from(p.file.as_str()), priority: p.constraints.priority });
        p.constraints.before.iter().for_each(|n| before.put(Str::from(n.as_str())));
        p.constraints.after.iter().for_each(|n| after.put(Str::from(n.as_str())));
    }
}

unsafe extern "C" fn add_sink(api: *const HostApi, source: ErasedSource, f: SinkCallback) {
    super::add_sink(owner(api), source, f);
}

unsafe extern "C" fn add_player_graph_sink(api: *const HostApi, f: SinkCallback) {
    super::add_player_graph_sink(owner(api), f);
}

unsafe extern "C" fn register_native(api: *const HostApi, request: *const RawNativeRequest, f: NativeCallback, function: *mut *const NativeFunction, error: Sink<'_, *const RawError>) -> u32 {
    let registered = read::<_, Error>(request).and_then(|request| {
        let params = unsafe { request.params() }?;
        super::register_native(owner(api), &unsafe { request.to_request(&params) }?, native_fn(f))
    });
    report(registered.map(core::ptr::from_ref), function, &error)
}

unsafe extern "C" fn register_command(api: *const HostApi, request: *const RawCommandRequest, f: CommandCallback, slot: *mut RawCommandSlot, error: Sink<'_, *const RawError>) -> u32 {
    let registered = read::<_, Error>(request).and_then(|request| super::register_command(owner(api), &unsafe { request.to_request() }, execute_fn(f)));
    report(registered.map(RawCommandSlot::new), slot, &error)
}

unsafe extern "C" fn replace_command(api: *const HostApi, name: *const c_char, f: CommandCallback, slot: *mut RawCommandSlot, error: Sink<'_, *const RawError>) -> u32 {
    report(super::replace_command(owner(api), unsafe { c_str(name) }, execute_fn(f)).map(RawCommandSlot::new), slot, &error)
}

/// `given` as the owner of a subscription or patch made through the table of `api`; the host's
/// own table, whose name is empty, may name any owner.
fn check_owner(api: *const HostApi, given: &str) -> Result<(), hooks::Error> {
    match owner(api) {
        "" => Ok(()),
        plugin if plugin == given => Ok(()),
        plugin => Err(hooks::Error::Owner { given: given.to_owned(), plugin: plugin.to_owned() }),
    }
}

fn subscribe(api: *const HostApi, request: *const RawSubscription, subscriber: Subscriber, id: *mut u64, error: &Sink<'_, *const RawError>) -> u32 {
    let result = read(request).and_then(|request| unsafe { request.to_request(subscriber) }).and_then(|request| {
        check_owner(api, &request.owner)?;
        registry::subscribe(request)
    });
    report(result.map(SubscriptionId::get), id, error)
}

unsafe extern "C" fn hook_before(api: *const HostApi, request: *const RawSubscription, f: BeforeCallback, id: *mut u64, error: Sink<'_, *const RawError>) -> u32 {
    subscribe(api, request, Subscriber::Before(f), id, &error)
}

unsafe extern "C" fn hook_around(api: *const HostApi, request: *const RawSubscription, f: AroundCallback, id: *mut u64, error: Sink<'_, *const RawError>) -> u32 {
    subscribe(api, request, Subscriber::Around(f), id, &error)
}

unsafe extern "C" fn hook_replace(api: *const HostApi, request: *const RawSubscription, f: ReplaceCallback, id: *mut u64, error: Sink<'_, *const RawError>) -> u32 {
    subscribe(api, request, Subscriber::Replace(f), id, &error)
}

unsafe extern "C" fn hook_after(api: *const HostApi, request: *const RawSubscription, f: AfterCallback, id: *mut u64, error: Sink<'_, *const RawError>) -> u32 {
    subscribe(api, request, Subscriber::After(f), id, &error)
}

unsafe extern "C" fn hook_unsubscribe(_: *const HostApi, id: u64) -> bool {
    registry::unsubscribe(SubscriptionId::new(id))
}

unsafe extern "C" fn hook_remove_owner(_: *const HostApi, owner: Str) -> usize {
    registry::remove_owner(unsafe { owner.as_str() })
}

unsafe extern "C" fn hook_set_enabled(_: *const HostApi, id: u64, enabled: bool) -> bool {
    registry::set_enabled(SubscriptionId::new(id), enabled)
}

unsafe extern "C" fn hook_diagnostics(_: *const HostApi, reports: Sink<'_, *const RawSiteReport>) {
    for report in registry::diagnostics() {
        put_site(&report, &reports);
    }
}

unsafe extern "C" fn hook_messages(_: *const HostApi, lines: Sink<'_, Str>) {
    for line in registry::messages() {
        lines.put(Str::from(line.as_str()));
    }
}

unsafe extern "C" fn hook_patch(api: *const HostApi, request: *const RawPatch, id: *mut u64, error: Sink<'_, *const RawError>) -> u32 {
    let result = read(request).and_then(|request| unsafe { request.to_request() }).and_then(|request| {
        check_owner(api, &request.owner)?;
        registry::patch(request)
    });
    report(result.map(PatchId::get), id, &error)
}

unsafe extern "C" fn hook_set_patch_enabled(_: *const HostApi, id: u64, enabled: bool, error: Sink<'_, *const RawError>) -> u32 {
    report(registry::set_patch_enabled(PatchId::new(id), enabled), &mut (), &error)
}

unsafe extern "C" fn hook_unpatch(_: *const HostApi, id: u64) -> bool {
    registry::unpatch(PatchId::new(id))
}

unsafe extern "C" fn hook_patches(_: *const HostApi, reports: Sink<'_, *const RawPatchReport>) {
    for patch in registry::patches() {
        reports.put(&RawPatchReport::new(&patch));
    }
}

unsafe extern "C" fn add_section(api: *const HostApi, name: Str, f: UiCallback) {
    gui::add_section(owner(api), unsafe { name.as_str() }, f);
}

unsafe extern "C" fn add_hud(api: *const HostApi, f: UiCallback) {
    gui::add_hud(owner(api), f);
}

unsafe extern "C" fn fonts(_: *const HostApi, files: Sink<'_, *const RawFont>, families: Sink<'_, *const RawFamily>) {
    let Some(fonts) = overlay::font_definitions() else { return };
    for (name, data) in &fonts.font_data {
        files.put(&RawFont { size: abi::size::<RawFont>(), name: Str::from(name.as_str()), bytes: Str::new((*data.font).as_ref()) });
    }
    for (family, names) in &fonts.families {
        let names: Vec<Str> = names.iter().map(|n| Str::from(n.as_str())).collect();
        families.put(&RawFamily::new(family, &names));
    }
}

/// A plugin's native as the host binds it; the plugin writes the value over a None it owns nothing in.
fn native_fn(f: NativeCallback) -> ErasedNativeFn {
    Box::new(move |call: &NativeCall<'_>| {
        // Captures the whole callback, which is Send, rather than its raw fields.
        let f = &f;
        let mut out = ManuallyDrop::new(Variable::none());
        (f.call)(f.ctx, &RawNativeCall::new(call), (&raw mut out).cast()).then(|| ManuallyDrop::into_inner(out))
    })
}

/// A plugin's command as the host's console entry runs it.
fn execute_fn(f: CommandCallback) -> ErasedExecute {
    Box::new(move |context| {
        let f = &f;
        let args: Vec<RawArg> = context.args().iter().map(RawArg::new).collect();
        let mut raw = RawCommandContext { size: abi::size::<RawCommandContext>(), this_ref: context.this_ref().map_or(core::ptr::null(), core::ptr::from_ref), args: args.as_ptr(), len: args.len(), result: context.result_ptr() };
        (f.call)(f.ctx, &raw mut raw)
    })
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use skyrim::input::{Arg, Command, CommandContext};
    use skyrim::papyrus::VirtualMachine;
    use sre::BSEventNotifyControl;

    use super::*;
    use crate::abi::{self, Api, Callback, call, entry, names, status};
    use crate::settings::{Setting, Settings};
    use crate::{Action, Error, Host};

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
        assert!(d.build.is_null());
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

    /// The first entry and the last of api 1 to 4: a plugin built against any of them finds its
    /// entries at the offsets it was built with on every later loader.
    #[test]
    fn the_table_keeps_every_versions_offsets() {
        use core::mem::offset_of;
        let offsets = [offset_of!(HostApi, on_message), offset_of!(HostApi, plugins), offset_of!(HostApi, replace_command), offset_of!(HostApi, hook_patches), offset_of!(HostApi, fonts), size_of::<HostApi>()];
        assert_eq!(offsets, [32, 248, 296, 400, 424, 432]);
    }

    #[test]
    fn an_older_loader_answers_for_what_its_table_lacks() {
        let full = build("abi-test-older", &raw const SKSE);
        open("abi-test-older");
        let older = HostApi { size: u32::try_from(core::mem::offset_of!(HostApi, serve_alias)).unwrap(), ..*full };
        let (full, older) = (Api(full), Api(&raw const older));
        assert!(entry!(older, on_plugin_message).is_some());
        assert!(entry!(older, serve_alias).is_none() && entry!(older, action).is_none() && entry!(older, plugins).is_none());
        assert!(Host::new(older.0).is_some() && Host::new(full.0).is_some());

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

        let stage_1 = unsafe { HostApi { size: u32::try_from(core::mem::offset_of!(HostApi, add_sink)).unwrap(), ..*full.0 } };
        let host = Host::new(&raw const stage_1).expect("a stage 1 loader");
        let vm: &VirtualMachine = Box::leak(Box::new(unsafe { core::mem::zeroed() }));
        assert!(matches!(host.register_native(vm, "Mod", "One", || 1i32), Err(Error::Unsupported { what }) if what == "native Mod.One"));
        assert!(matches!(host.register_command::<Test>(), Err(Error::Unsupported { .. })));
        assert!(matches!(host.replace_command(c"SetCameraFOV", |_| true), Err(Error::Unsupported { .. })));
        host.add_player_graph_sink(|_| BSEventNotifyControl::Continue);

        let stage_3 = unsafe { HostApi { size: u32::try_from(core::mem::offset_of!(HostApi, add_section)).unwrap(), ..*full.0 } };
        let stage_3 = Api(&raw const stage_3);
        assert!(!crate::ui::add_section(stage_3, "Main", |_| {}) && !crate::ui::add_hud(stage_3, |_, _| {}), "a stage 3 loader takes no UI");
        let host = Host::new(stage_3.0).expect("a stage 3 loader");
        host.add_tab("Older").section("Main", |ui| _ = ui.label("never drawn")).section("Other", |_| {});
        host.add_hud(|_, _| {});
    }

    struct Test;

    impl Command for Test {
        const NAME: &'static core::ffi::CStr = c"SkyrimRsAbiTest";
        const HELP: &'static core::ffi::CStr = c"";
        const PARAMS: &'static [skyrim::input::Param] = &[];
        fn execute(_: &mut CommandContext<'_>) -> bool {
            true
        }
    }

    /// Counts its drops, to see a plugin's closure freed exactly once.
    struct Counted(&'static AtomicU32);

    impl Counted {
        fn alive(&self) -> bool {
            self.0.load(Ordering::Relaxed) == 0
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_sink_closure_runs_through_the_host_and_drops_once() {
        static DROPS: AtomicU32 = AtomicU32::new(0);
        let counted = Counted(&DROPS);
        let handlers = super::super::Owned::new();
        handlers.add("sink-test", abi::sink(move |event, _| if event.is_null() && counted.alive() { BSEventNotifyControl::Stop } else { BSEventNotifyControl::Continue }));
        assert_eq!(super::super::dispatch(&handlers, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Stop);
        assert_eq!(DROPS.load(Ordering::Relaxed), 0, "a call keeps the closure");
        assert_eq!(handlers.remove_owner("sink-test"), 1);
        assert_eq!(DROPS.load(Ordering::Relaxed), 1);
        assert_eq!(super::super::dispatch(&handlers, core::ptr::null(), core::ptr::null()), BSEventNotifyControl::Continue);
    }

    #[test]
    fn a_native_closure_runs_through_the_host_and_drops_once() {
        static DROPS: AtomicU32 = AtomicU32::new(0);
        let counted = Counted(&DROPS);
        let f = native_fn(abi::native(move |call| {
            let first = call.args.first()?.as_int().filter(|_| counted.alive())?;
            Some(Variable::int(first + i32::try_from(call.args.len() + call.stack_id as usize).unwrap()))
        }));
        let vm: &VirtualMachine = Box::leak(Box::new(unsafe { core::mem::zeroed() }));
        // The engine resets a Variable on drop; there is no engine here.
        let (none, five, text) = (ManuallyDrop::new(Variable::none()), ManuallyDrop::new(Variable::int(5)), ManuallyDrop::new(Variable::bool(true)));
        let value = ManuallyDrop::new(f(&NativeCall { vm, self_: &none, args: &[&five, &text], stack_id: 7 }).expect("the native answers"));
        assert_eq!(value.as_int(), Some(14));
        assert!(f(&NativeCall { vm, self_: &none, args: &[&text], stack_id: 7 }).is_none(), "a native that fails the call writes nothing");
        assert_eq!(DROPS.load(Ordering::Relaxed), 0);
        drop(f);
        assert_eq!(DROPS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_command_closure_runs_through_the_host_and_drops_once_when_replaced() {
        static FIRST: AtomicU32 = AtomicU32::new(0);
        static SECOND: AtomicU32 = AtomicU32::new(0);
        let command = |counted: Counted| {
            execute_fn(abi::command(move |context| {
                let read = counted.alive() && matches!(context.args(), [Arg::Int(3), Arg::Str("pos"), Arg::Missing]) && context.this_ref().is_none();
                context.set_result(if read { 1.0 } else { -1.0 });
                read
            }))
        };
        let args = [Arg::Int(3), Arg::Str("pos"), Arg::Missing];
        let run = |execute: &ErasedExecute| {
            let mut result = 0.0;
            execute(&mut CommandContext::new(core::ptr::null_mut(), &args, &raw mut result)) && (result - 1.0).abs() < f64::EPSILON
        };
        let mut execute = command(Counted(&FIRST));
        assert!(run(&execute), "the arguments and the result cross");
        execute = command(Counted(&SECOND));
        assert_eq!((FIRST.load(Ordering::Relaxed), SECOND.load(Ordering::Relaxed)), (1, 0), "a later generation's command drops the earlier one");
        assert!(run(&execute));
        drop(execute);
        assert_eq!((FIRST.load(Ordering::Relaxed), SECOND.load(Ordering::Relaxed)), (1, 1));
    }

    #[inline(never)]
    unsafe extern "C" fn hooked(a: usize, b: usize) -> usize {
        core::hint::black_box("abi-test hooked");
        core::hint::black_box(a) * b
    }

    #[test]
    fn hooks_go_through_the_plugins_table() {
        use crate::hooks::ops::Raw;
        use crate::hooks::{self, Hooks, Location, Options};
        static DROPS: AtomicU32 = AtomicU32::new(0);
        static KEPT: AtomicU32 = AtomicU32::new(0);
        let target = || Raw::<unsafe extern "C" fn(usize, usize) -> usize>::at(hooked as *const () as usize);
        let full = build("abi-test-hooks", &raw const SKSE);
        let stage_2 = HostApi { size: u32::try_from(core::mem::offset_of!(HostApi, hook_before)).unwrap(), ..*full };
        let older = Hooks::new(&raw const stage_2);
        let counted = Counted(&DROPS);
        assert_eq!(older.before(target(), Options::new("abi-test-hooks"), move |_| _ = counted.alive()), Err(hooks::Error::Unsupported { what: "hook subscriptions".to_owned() }));
        assert_eq!(DROPS.load(Ordering::Relaxed), 1, "the closure a loader without the entry never saw is dropped");
        assert!(matches!(unsafe { older.patch("abi-test-hooks", Location::Address(0), &[0], &[0]) }, Err(hooks::Error::Unsupported { .. })));
        assert!(matches!(older.set_patch_enabled(hooks::PatchId::new(1), false), Err(hooks::Error::Unsupported { .. })));
        assert!(older.diagnostics().is_empty() && older.messages().is_empty() && older.patches().is_empty());
        assert_eq!(older.remove_owner("abi-test-hooks"), 0);
        assert!(!older.unsubscribe(SubscriptionId::new(1)) && !older.set_enabled(SubscriptionId::new(1), false) && !older.unpatch(PatchId::new(1)));

        let plugin = Hooks::new(full);
        let api_4 = HostApi { version: 4, ..*full };
        let counted = Counted(&DROPS);
        assert_eq!(Hooks::new(&raw const api_4).before(target(), Options::new("abi-test-hooks").exclusive(), move |_| _ = counted.alive()), Err(hooks::Error::Unsupported { what: "exclusive hook subscriptions".to_owned() }));
        assert_eq!(DROPS.load(Ordering::Relaxed), 2, "an exclusive request is never silently downgraded on an older loader");
        let foreign = plugin.before(target(), Options::new("abi-test-other"), |_| {}).unwrap_err();
        assert_eq!(foreign, hooks::Error::Owner { given: "abi-test-other".to_owned(), plugin: "abi-test-hooks".to_owned() });
        assert!(matches!(unsafe { plugin.patch("abi-test-other", Location::Address(0), &[0], &[0]) }, Err(hooks::Error::Owner { .. })));
        let counted = Counted(&KEPT);
        plugin.before(target(), Options::new("abi-test-hooks"), move |args| args.0 += usize::from(counted.alive())).unwrap();
        assert_eq!(unsafe { core::hint::black_box(hooked as unsafe extern "C" fn(usize, usize) -> usize)(2, 3) }, 9);
        assert_eq!(KEPT.load(Ordering::Relaxed), 0);
        assert!(super::super::detach("abi-test-hooks").starts_with("1 hook subscriptions"));
        assert_eq!(KEPT.load(Ordering::Relaxed), 1, "detach drops the subscriber once, before the DLL would go");
        assert_eq!(unsafe { core::hint::black_box(hooked as unsafe extern "C" fn(usize, usize) -> usize)(2, 3) }, 6);
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
