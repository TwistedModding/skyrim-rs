//! The ui suite through `skyrim::ui` and the generated UI classes, against the RE repo's `docs/subsystems/ui-verify.md`. `install`
//! registers the suite's own menu and detours `RenderMenuStack`, `UIMessageQueue::AddMessage` and
//! `ShowHUDMessage` at `DataLoaded`; the draw detour keeps the latest stack sample per set of open
//! menus and, on the first frame after a load with only the HUD open, runs the Scaleform probe,
//! sends a notification of its own and shows the suite's menu, which it hides again after 60
//! draws; `TESTS` read the singleton, the table, the queue and the HUD after a load; `SAVE_TESTS`
//! check the samples, the queued messages, the notifications, the probe and the menu's life.

use core::ffi::{CStr, c_char};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use skyrim::BSFixedString;
use skyrim::events::{BSTEventSource, Event, EventSink, Sink};
use skyrim::prelude::*;
use skyrim::ui::{AddMessageFn, Function, FunctionHandler, Menu, MenuBase, MessageType, Params, RenderMenuStackFn, ShowHudMessageFn, Value, ValueType};
use sre::{BSEventNotifyControl, EventSourceHolder_MenuOpenClose as Ui, IMenu, IMenu_flags, MenuOpenCloseEvent, UIMessage, UIMessageQueue, UIMessageResult};

use crate::{Test, check, not_run};

/// The menu's class without its `::vftable` suffix, `?` when the ledger has no vtable for it.
fn class_name(menu: &IMenu) -> &'static str {
    skyrim::class_of(menu).map_or("?", |c| c.class.strip_suffix("::vftable").unwrap_or(c.class))
}

pub const MENU: &str = "SkyrimRsTestMenu";
const DEPTH: u8 = 6;
const NOTE: &str = "skyrim-rs-note";
const OWN_FRAMES: u32 = 60;

pub const TESTS: &[(&str, Test)] = &[("singleton", singleton), ("table", table), ("queue", queue), ("hud", hud)];
pub const SAVE_TESTS: &[(&str, Test)] = &[("stack_play", stack_play), ("stack_tween", stack_tween), ("stack_box", stack_box), ("stack_own", stack_own), ("messages", messages), ("notification", notification), ("own_menu", own_menu), ("scaleform", scaleform)];

static RENDER: OnceLock<hook::Hook<RenderMenuStackFn>> = OnceLock::new();
static ADD: OnceLock<hook::Hook<AddMessageFn>> = OnceLock::new();
static HUD: OnceLock<hook::Hook<ShowHudMessageFn>> = OnceLock::new();

/// What the draw detour reads under the stack lock: `(name, depth, flags)` per menu, the counters, the thread.
#[derive(Clone, Debug)]
struct Sample {
    menus: Vec<(String, u8, u32)>,
    pauses: u32,
    modal: bool,
    allowing_save: u32,
    thread: String,
}

static SAMPLES: Mutex<BTreeMap<String, Sample>> = Mutex::new(BTreeMap::new());
static MESSAGES: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
static NOTIFICATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static PROBE: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static PROBE_PENDING: AtomicBool = AtomicBool::new(false);
static HIDE_PENDING: AtomicBool = AtomicBool::new(false);
/// A show (1) or hide (2) of the suite's menu asked for by the input suite's console command.
static REQUEST: AtomicU32 = AtomicU32::new(0);

static CREATED: AtomicU32 = AtomicU32::new(0);
static POST_CREATED: AtomicU32 = AtomicU32::new(0);
static SHOWN: AtomicU32 = AtomicU32::new(0);
static HIDDEN: AtomicU32 = AtomicU32::new(0);
static ADVANCES: AtomicU32 = AtomicU32::new(0);
static DISPLAYS: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);
static OPEN_EVENTS: AtomicU32 = AtomicU32::new(0);
static CLOSE_EVENTS: AtomicU32 = AtomicU32::new(0);
static OWN: AtomicPtr<sre::IMenu> = AtomicPtr::new(core::ptr::null_mut());

struct TestMenu;

impl Menu for TestMenu {
    fn create(base: &mut MenuBase) -> Self {
        CREATED.fetch_add(1, Ordering::Relaxed);
        base.set_depth(DEPTH);
        base.set_flags(IMenu_flags(0));
        OWN.store(core::ptr::from_ref::<IMenu>(base).cast_mut(), Ordering::Relaxed);
        Self
    }

    fn post_create(&mut self, _: &mut MenuBase) {
        POST_CREATED.fetch_add(1, Ordering::Relaxed);
    }

    fn process_message(&mut self, base: &mut MenuBase, message: &UIMessage) -> UIMessageResult {
        match MessageType::from_engine(message.type_) {
            Some(MessageType::show) => SHOWN.fetch_add(1, Ordering::Relaxed),
            Some(MessageType::hide) => HIDDEN.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
        base.default_process_message(message)
    }

    fn advance(&mut self, base: &mut MenuBase, interval: f32, time_ms: u32) {
        ADVANCES.fetch_add(1, Ordering::Relaxed);
        base.advance_movie(interval, time_ms);
    }

    fn display(&mut self, base: &mut MenuBase) {
        DISPLAYS.fetch_add(1, Ordering::Relaxed);
        base.display_movie();
    }
}

impl Drop for TestMenu {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        OWN.store(core::ptr::null_mut(), Ordering::Relaxed);
    }
}

struct MenuEvents;

impl EventSink<MenuOpenCloseEvent> for MenuEvents {
    fn process(&self, event: &MenuOpenCloseEvent, _: &BSTEventSource<MenuOpenCloseEvent>) -> BSEventNotifyControl {
        if *BSFixedString::view(&event.menuName) == *MENU {
            if event.opening != 0 { &OPEN_EVENTS } else { &CLOSE_EVENTS }.fetch_add(1, Ordering::Relaxed);
        }
        BSEventNotifyControl::Continue
    }
}

static MENU_SINK: Sink<MenuOpenCloseEvent, MenuEvents> = Sink::new(MenuEvents);

struct Counter {
    calls: AtomicU32,
    last: AtomicU64,
}

impl FunctionHandler for Counter {
    fn call(&self, params: &mut Params) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if let Some(n) = params.args().first().and_then(Value::as_number) {
            self.last.store(n.to_bits(), Ordering::Relaxed);
        }
        params.set_return(&Value::number(42.0));
    }
}

static CALLBACK: Function<Counter> = Function::new(Counter { calls: AtomicU32::new(0), last: AtomicU64::new(0) });

fn name_of(menu: &IMenu) -> String {
    if core::ptr::eq(menu, OWN.load(Ordering::Relaxed)) { MENU.to_owned() } else { class_name(menu).to_owned() }
}

unsafe extern "C" fn render_detour(ui: *mut sre::EventSourceHolder_MenuOpenClose) {
    unsafe { RENDER.get().expect("stored before enabling").original()(ui) }
    let Some(ui) = Ui::singleton() else { return };
    let hud_only = if let Some(stack) = ui.try_stack() {
        let menus: Vec<(String, u8, u32)> = stack.iter().map(|m| (name_of(m), m.depth, m.flags)).collect();
        drop(stack);
        let mut names: Vec<&str> = menus.iter().map(|m| m.0.as_str()).filter(|n| *n != "?").collect();
        names.sort_unstable();
        let key = names.join("+");
        let sample = Sample { menus, pauses: ui.numPausesGame, modal: ui.modalMenuOpen != 0, allowing_save: ui.numMenusAllowingSave, thread: format!("{:?}", std::thread::current().id()) };
        let hud_only = key == "HUDMenu";
        SAMPLES.lock().unwrap().insert(key, sample);
        hud_only
    } else {
        ui.is_open("HUD Menu") && !["Fader Menu", "Mist Menu", "Loading Menu", "Main Menu", "Cursor Menu"].iter().any(|menu| ui.is_open(menu))
    };
    if hud_only && PROBE_PENDING.swap(false, Ordering::Relaxed) {
        *PROBE.lock().unwrap() = Some(probe(ui));
        skyrim::ui::show_hud_message(NOTE, None, false);
        if let Some(queue) = UIMessageQueue::singleton() {
            queue.show(MENU);
            HIDE_PENDING.store(true, Ordering::Relaxed);
        }
    }
    if DISPLAYS.load(Ordering::Relaxed) >= OWN_FRAMES
        && HIDE_PENDING.swap(false, Ordering::Relaxed)
        && let Some(queue) = UIMessageQueue::singleton()
    {
        queue.hide(MENU);
    }
    match (REQUEST.swap(0, Ordering::Relaxed), UIMessageQueue::singleton()) {
        (1, Some(queue)) => queue.show(MENU),
        (2, Some(queue)) => queue.hide(MENU),
        _ => {}
    }
}

/// Queues a show or hide of the suite's menu from the next menu draw (the thread the RE run saw
/// drawing every menu), for callers on other threads.
pub fn request(show: bool) {
    REQUEST.store(if show { 1 } else { 2 }, Ordering::Relaxed);
}

/// `MenuOpenCloseEvent`s of the suite's menu since the last load: (opening, closing).
pub fn own_menu_events() -> (u32, u32) {
    (OPEN_EVENTS.load(Ordering::Relaxed), CLOSE_EVENTS.load(Ordering::Relaxed))
}

unsafe extern "C" fn add_detour(queue: *mut sre::UIMessageQueue, name: *const sre::BSFixedString, kind: u32, data: *mut sre::IUIMessageData) {
    unsafe { ADD.get().expect("stored before enabling").original()(queue, name, kind, data) }
    if !name.is_null() {
        let name = BSFixedString::view(unsafe { &*name }).to_string();
        let mut messages = MESSAGES.lock().unwrap();
        if messages.len() < 500 {
            messages.push((name, kind));
        }
    }
}

unsafe extern "C" fn hud_detour(text: *const c_char, sound: *const c_char, suppress_repeats: bool) {
    unsafe { HUD.get().expect("stored before enabling").original()(text, sound, suppress_repeats) }
    if !text.is_null() {
        NOTIFICATIONS.lock().unwrap().push(unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned());
    }
}

/// Registers the suite's menu, listens for its open and close events and enables the three detours; once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    let ui = Ui::singleton().ok_or("g_EventSourceHolder_MenuOpenClose_ptr is null")?;
    ui.register_menu::<TestMenu>(MENU);
    MenuOpenCloseEvent::source().ok_or("no MenuOpenCloseEvent source")?.add_sink(&MENU_SINK).leak();
    let render = unsafe { skyrim::ui::hook_render_menu_stack(render_detour) }.map_err(|e| format!("RenderMenuStack: {e}"))?;
    let add = unsafe { skyrim::ui::hook_add_message(add_detour) }.map_err(|e| format!("AddMessage: {e}"))?;
    let hud = unsafe { skyrim::ui::hook_show_hud_message(hud_detour) }.map_err(|e| format!("ShowHUDMessage: {e}"))?;
    let targets = (render.target(), add.target(), hud.target());
    let _ = RENDER.set(render);
    let _ = ADD.set(add);
    let _ = HUD.set(hud);
    RENDER.get().expect("just set").enable().map_err(|e| format!("enable RenderMenuStack: {e}"))?;
    ADD.get().expect("just set").enable().map_err(|e| format!("enable AddMessage: {e}"))?;
    HUD.get().expect("just set").enable().map_err(|e| format!("enable ShowHUDMessage: {e}"))?;
    Ok(format!("{MENU} registered ({}), RenderMenuStack at {:#x}, AddMessage at {:#x}, ShowHUDMessage at {:#x}", ui.is_registered(MENU), targets.0, targets.1, targets.2))
}

/// Clears the samples and counters and arms the Scaleform probe; called after a load.
pub fn reset() {
    SAMPLES.lock().unwrap().clear();
    MESSAGES.lock().unwrap().clear();
    NOTIFICATIONS.lock().unwrap().clear();
    *PROBE.lock().unwrap() = None;
    HIDE_PENDING.store(false, Ordering::Relaxed);
    for counter in [&CREATED, &POST_CREATED, &SHOWN, &HIDDEN, &ADVANCES, &DISPLAYS, &DROPPED, &OPEN_EVENTS, &CLOSE_EVENTS] {
        counter.store(0, Ordering::Relaxed);
    }
    PROBE_PENDING.store(true, Ordering::Relaxed);
}

fn ui() -> Result<&'static Ui, String> {
    Ui::singleton().ok_or_else(|| "g_EventSourceHolder_MenuOpenClose_ptr is null".to_owned())
}

fn describe(menus: &[(String, u8, u32)]) -> String {
    menus.iter().map(|(name, depth, flags)| format!("{name}@{depth}:{flags:#x}")).collect::<Vec<_>>().join(", ")
}

fn singleton() -> Result<String, String> {
    let ui = ui()?;
    let Some(stack) = ui.try_stack() else { return not_run(STACK_NOT_RUN) };
    let menus: Vec<(String, u8, u32)> = stack.iter().map(|m| (name_of(m), m.depth, m.flags)).collect();
    drop(stack);
    check(menus.iter().any(|(name, depth, flags)| name == "HUDMenu" && *depth == 2 && flags & 0x40 != 0), &format!("no open HUDMenu at depth 2 in [{}]", describe(&menus)))?;
    check(ui.menusVisible != 0 && ui.closingAllMenus == 0, "menusVisible or closingAllMenus is off")?;
    Ok(format!("stack [{}], numPausesGame {}, modal {}, allowing save {}, inventory menus {}", describe(&menus), { ui.numPausesGame }, ui.modalMenuOpen != 0, { ui.numMenusAllowingSave }, { ui.numInventoryMenus }))
}

fn table() -> Result<String, String> {
    let ui = ui()?;
    check(ui.is_registered("TweenMenu") && ui.is_registered("HUD Menu") && ui.is_registered("Cursor Menu"), "a vanilla menu name is not registered")?;
    check(ui.is_registered(MENU), &format!("{MENU} is not registered"))?;
    check(!ui.is_registered("SkyrimRsNoSuchMenu"), "an unknown name is registered")?;
    check(!ui.is_open(MENU) && !ui.is_open("TweenMenu"), "a menu reads open before the scenario")?;
    check(ui.is_open("HUD Menu"), "the HUD reads closed")?;
    let hud = ui.menu("HUD Menu").ok_or("the HUD Menu has no live menu")?;
    check(class_name(hud) == "HUDMenu", &format!("HUD Menu is a {}", class_name(hud)))?;
    let tween = ui.menu("TweenMenu").map_or("none".to_owned(), |m| format!("{} refs {} flags {:#x}", class_name(m), m.refCount, m.flags));
    Ok(format!("TweenMenu, HUD Menu, Cursor Menu and {MENU} registered; HUD Menu live as HUDMenu (refs {}, depth {}); TweenMenu live: {tween}", hud.refCount, hud.depth))
}

fn queue() -> Result<String, String> {
    let queue = UIMessageQueue::singleton().ok_or("g_UIMessageQueue_ptr is null")?;
    let used = queue.staticMessagesUsed;
    check(used <= 64, &format!("{used} static messages in use"))?;
    Ok(format!("queue at {queue:p}, {used} static messages used since the last drain"))
}

fn hud() -> Result<String, String> {
    let ui = ui()?;
    let hud = ui.hud().ok_or("HUD Menu is not a live HUDMenu")?;
    check(hud.flags & 0x40 != 0 && hud.depth == 2, &format!("HUD depth {} flags {:#x}", hud.depth, hud.flags))?;
    check(hud.flags == 0x18942, &format!("HUD flags {:#x}", hud.flags))?;
    let movie = hud.movie().ok_or("the HUD has no movie")?;
    check(movie.is_engine_view(), "the HUD movie's vtable is not GFxMovieView_vftable")?;
    let root = Value::view(&hud.root);
    check(root.value_type() == Some(ValueType::DisplayObject) && root.is_managed(), &format!("the HUD root is {root:?}"))?;
    let components = hud.objects.size;
    check(components > 0, "the HUD has no components")?;
    Ok(format!("HUDMenu flags 0x18942 depth 2, movie {movie:p} (engine vtable, {} refs, visible {}), root managed display object, {components} components", { movie.refCount }, movie.visible()))
}

fn probe(ui: &Ui) -> Result<String, String> {
    let hud = ui.hud().ok_or("HUD Menu is not a live HUDMenu")?;
    let movie = hud.movie().ok_or("the HUD has no movie")?;
    let root = movie.variable("_root").ok_or("_root does not resolve")?;
    check(root.is_object() && root.is_managed(), &format!("_root is {root:?}"))?;
    let text = movie.create_string("probe");
    check(text.value_type() == Some(ValueType::String) && text.is_managed(), &format!("the created string is {text:?}"))?;
    let object = movie.create_object();
    check(object.value_type() == Some(ValueType::Object) && object.is_managed(), &format!("the created object is {object:?}"))?;
    check(object.set_member("x", &Value::number(42.0)), "set_member(x) failed")?;
    let x = object.get_member("x").ok_or("get_member(x) failed")?;
    check(x.as_number() == Some(42.0), &format!("x read back as {x:?}"))?;
    check(object.has_member("x") && !object.has_member("y"), "has_member disagrees with the members set")?;
    check(movie.set_variable("_root.skyrimRsProbe", &Value::number(7.0)), "set_variable(_root.skyrimRsProbe) failed")?;
    let probe = movie.variable("_root.skyrimRsProbe").ok_or("variable(_root.skyrimRsProbe) failed")?;
    check(probe.as_number() == Some(7.0), &format!("_root.skyrimRsProbe read back as {probe:?}"))?;
    let function = movie.create_function(&CALLBACK);
    check(function.is_object() && function.is_managed(), &format!("the function value is {function:?}"))?;
    check(root.set_member("skyrimRsCall", &function), "set_member(skyrimRsCall) on _root failed")?;
    let returned = movie.invoke("_root.skyrimRsCall", &[Value::number(3.0)]).ok_or("invoke(_root.skyrimRsCall) failed")?;
    let calls = CALLBACK.handler().calls.load(Ordering::Relaxed);
    let last = f64::from_bits(CALLBACK.handler().last.load(Ordering::Relaxed));
    check(calls == 1 && (last - 3.0).abs() < f64::EPSILON, &format!("the callback ran {calls} times, last argument {last}"))?;
    check(returned.as_number() == Some(42.0), &format!("the callback's return read back as {returned:?}"))?;
    let hud_root = Value::view(&hud.root);
    check(hud_root.has_member("ShowMessage"), "the HUD root has no ShowMessage")?;
    Ok(format!("_root managed object; created string and object managed; x = 42 round trip; _root.skyrimRsProbe = 7 round trip; callback called once with 3, returned 42; HUD root has ShowMessage; on {:?}", std::thread::current().id()))
}

const STACK_NOT_RUN: &str = "the menu stack's count has no ledger row on this runtime";

fn sample(key: &str) -> Result<Sample, String> {
    if ui()?.try_stack().is_none() {
        return Err(format!("{}{STACK_NOT_RUN}", crate::NOT_RUN));
    }
    let samples = SAMPLES.lock().unwrap();
    samples.get(key).cloned().ok_or_else(|| format!("no sample {key}; have [{}]", samples.keys().cloned().collect::<Vec<_>>().join("; ")))
}

fn menu_in(s: &Sample, name: &str) -> Result<(u8, u32), String> {
    s.menus.iter().find(|m| m.0 == name).map(|m| (m.1, m.2)).ok_or_else(|| format!("{name} missing in [{}]", describe(&s.menus)))
}

fn stack_play() -> Result<String, String> {
    let s = sample("HUDMenu")?;
    let (depth, flags) = menu_in(&s, "HUDMenu")?;
    check(depth == 2 && flags == 0x18942, &format!("HUDMenu depth {depth} flags {flags:#x}"))?;
    check(s.pauses == 0 && !s.modal, &format!("numPausesGame {} modal {}", s.pauses, s.modal))?;
    Ok(format!("[{}], numPausesGame 0, modal false, allowing save {}, drawn on {}", describe(&s.menus), s.allowing_save, s.thread))
}

fn stack_tween() -> Result<String, String> {
    let s = sample("CursorMenu+HUDMenu+TweenMenu")?;
    let (tween_depth, tween_flags) = menu_in(&s, "TweenMenu")?;
    let (cursor_depth, cursor_flags) = menu_in(&s, "CursorMenu")?;
    check(tween_depth == 0 && tween_flags == 0xcc45, &format!("TweenMenu depth {tween_depth} flags {tween_flags:#x}"))?;
    check(cursor_depth == 13 && cursor_flags == 0x8840, &format!("CursorMenu depth {cursor_depth} flags {cursor_flags:#x}"))?;
    check(s.pauses == 1 && !s.modal, &format!("numPausesGame {} modal {}", s.pauses, s.modal))?;
    Ok(format!("[{}], numPausesGame 1, modal false, allowing save {}", describe(&s.menus), s.allowing_save))
}

fn stack_box() -> Result<String, String> {
    let s = sample("CursorMenu+HUDMenu+MessageBoxMenu")?;
    let (depth, flags) = menu_in(&s, "MessageBoxMenu")?;
    check(depth == 10 && flags == 0x455, &format!("MessageBoxMenu depth {depth} flags {flags:#x}"))?;
    check(s.pauses == 1 && s.modal, &format!("numPausesGame {} modal {}", s.pauses, s.modal))?;
    Ok(format!("[{}], numPausesGame 1, modal true", describe(&s.menus)))
}

fn stack_own() -> Result<String, String> {
    let s = sample(&format!("HUDMenu+{MENU}"))?;
    let (depth, flags) = menu_in(&s, MENU)?;
    check(depth == DEPTH && flags == 0x40, &format!("{MENU} depth {depth} flags {flags:#x}"))?;
    check(s.pauses == 0 && !s.modal, &format!("numPausesGame {} modal {}", s.pauses, s.modal))?;
    Ok(format!("[{}]: {MENU} drawn at depth {DEPTH} with flags 0x40 only, numPausesGame 0", describe(&s.menus)))
}

fn messages() -> Result<String, String> {
    let seen = MESSAGES.lock().unwrap().clone();
    let expected = [("TweenMenu", 1), ("Cursor Menu", 1), ("TweenMenu", 3), ("Cursor Menu", 3), ("HUD Menu", 0), ("MessageBoxMenu", 1), ("MessageBoxMenu", 3), (MENU, 1), (MENU, 3)];
    let missing: Vec<String> = expected.iter().filter(|(n, k)| !seen.iter().any(|(sn, sk)| sn == n && sk == k)).map(|(n, k)| format!("{{{n}, {k}}}")).collect();
    check(missing.is_empty(), &format!("not queued: {}", missing.join(" ")))?;
    let mut distinct: Vec<String> = Vec::new();
    for (n, k) in &seen {
        let s = format!("{{{n}, {k}}}");
        if !distinct.contains(&s) {
            distinct.push(s);
        }
    }
    Ok(format!("{} messages queued, distinct in order: {}", seen.len(), distinct.join(" ")))
}

fn notification() -> Result<String, String> {
    let seen = NOTIFICATIONS.lock().unwrap().clone();
    for text in ["probe-hud", NOTE] {
        let count = seen.iter().filter(|t| *t == text).count();
        check(count == 1, &format!("{text} shown {count} times in {seen:?}"))?;
    }
    Ok(format!("ShowHUDMessage ran {} times: {seen:?}", seen.len()))
}

fn own_menu() -> Result<String, String> {
    let ui = ui()?;
    let counts = [&CREATED, &POST_CREATED, &SHOWN, &HIDDEN, &ADVANCES, &DISPLAYS, &DROPPED, &OPEN_EVENTS, &CLOSE_EVENTS].map(|c| c.load(Ordering::Relaxed));
    let [created, post_created, shown, hidden, advances, displays, dropped, opened, closed] = counts;
    check(created == 1 && post_created == 1, &format!("created {created}, post_create {post_created}"))?;
    check(shown == 1 && hidden == 1, &format!("show messages {shown}, hide messages {hidden}"))?;
    check(advances > 0 && displays >= OWN_FRAMES, &format!("advanced {advances} times, drawn {displays} times"))?;
    check(opened == 1 && closed == 1, &format!("MenuOpenCloseEvent opening {opened}, closing {closed}"))?;
    check(dropped == 1, &format!("destroyed {dropped} times"))?;
    check(!ui.is_open(MENU) && ui.menu(MENU).is_none(), "the menu is still live after its hide")?;
    Ok(format!("{MENU}: created and post-created once, show and hide delivered, advanced {advances} and drawn {displays} frames, one open and one close event, destroyed after the hide, table entry empty again"))
}

fn scaleform() -> Result<String, String> {
    PROBE.lock().unwrap().clone().ok_or_else(|| "the probe did not run (no RenderMenuStack after the load)".to_owned())?
}
