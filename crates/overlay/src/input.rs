//! The input source's first sink: the overlay's keys (the toggle, the close key, the keys bound to
//! the overlay and a key being captured), and while the overlay is open every event of the frame as
//! `egui` events, hidden from the sinks after it unless [`Capture::Focus`] lets the frame through to
//! the game. A frame that reaches the game passes its button events through the host's filter
//! ([`filter_game_buttons`]) first; one that does not reaches `PlayerControls` as an empty list.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use egui::{Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2};
use skyrim::events::{BSTEventSource, Event as _, EventSink, InputEventList, Sink};
use skyrim::prelude::*;
use sre::{BSEventNotifyControl, ButtonEvent, InputDevice, InputEvent};

/// The key that opens and closes the overlay unless the host binds another: F11.
pub const DEFAULT_TOGGLE: Hotkey = Hotkey::new(0x57);
/// The key that closes the overlay, or first releases the focused widget, unless the host binds another: Escape.
pub const DEFAULT_CLOSE: Hotkey = Hotkey::new(0x01);

const WHEEL_UP: u32 = 8;
const WHEEL_DOWN: u32 = 9;
const TAB: u32 = 0x0F;

/// What the overlay takes from the game while it is open. The sink sees one frame's events as a
/// list and can only pass or stop the whole list, so the rule is per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Capture {
    /// Every event (the default).
    All,
    /// The keyboard goes to the game while no widget has the keyboard focus (nothing Tab or a
    /// click on a text field reached, or Escape released it): a frame of keyboard events passes,
    /// and a frame with a mouse event or Tab is the overlay's. A frame that releases a key the game
    /// saw pressed passes, so no key stays held in the game; its mouse events then reach the game
    /// too. A frame holding a key the overlay took never passes.
    Focus,
}

/// A keyboard key and the modifiers held with it, written as the settings file and the Keys
/// section show it: `F11`, `Escape`, `Ctrl+Shift+K`. A key the name table lacks is written as its
/// scan code (`0x2A`); either form is read back. A press matches only with exactly these modifiers.
/// Laid out as C and frozen: the framework's plugin ABI writes it across.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    /// The key's `DirectInput` scan code.
    pub scan_code: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

const _: () = assert!(size_of::<Hotkey>() == 8 && core::mem::offset_of!(Hotkey, ctrl) == 4 && core::mem::offset_of!(Hotkey, shift) == 5 && core::mem::offset_of!(Hotkey, alt) == 6);

impl Hotkey {
    #[must_use]
    pub const fn new(scan_code: u32) -> Self {
        Self { scan_code, ctrl: false, shift: false, alt: false }
    }

    fn held(scan_code: u32, m: Modifiers) -> Self {
        Self { scan_code, ctrl: m.ctrl, shift: m.shift, alt: m.alt }
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [(self.ctrl, "Ctrl+"), (self.shift, "Shift+"), (self.alt, "Alt+")] {
            if held {
                f.write_str(name)?;
            }
        }
        match key_name(self.scan_code) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#04X}", self.scan_code),
        }
    }
}

impl FromStr for Hotkey {
    type Err = String;

    /// Modifiers (`Ctrl`, `Shift`, `Alt`, any case) and a key name or scan code, joined by `+`.
    fn from_str(text: &str) -> Result<Self, String> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = parts.pop().unwrap_or_default();
        let mut hotkey = Self::new(scan_code_named(key).ok_or_else(|| format!("no key named \"{key}\""))?);
        for part in parts {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => hotkey.ctrl = true,
                "shift" => hotkey.shift = true,
                "alt" => hotkey.alt = true,
                _ => return Err(format!("\"{part}\" is not Ctrl, Shift or Alt")),
            }
        }
        Ok(hotkey)
    }
}

/// Who has a key when an action bound to it fires; the values cross the framework's plugin ABI.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The overlay, open with no widget focused; while it is open the key never reaches the game.
    Overlay = 0,
    /// The game: the frame of the press reached the game's handlers.
    Game = 1,
}

/// What the overlay did with a key it took, as [`on_key`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyUse {
    /// The toggle key opened (`true`) or closed the overlay.
    Toggled(Hotkey, bool),
    /// The close key closed the overlay.
    Closed(Hotkey),
    /// The close key released the focused widget; the overlay stays open.
    FocusReleased(Hotkey),
    /// The key pressed while [`capture_next_key`] was armed.
    Captured(Hotkey),
}

static CAPTURE: AtomicU8 = AtomicU8::new(Capture::All as u8);

/// Sets what the overlay takes from the game while it is open; the host of the overlay decides.
pub fn set_capture(capture: Capture) {
    CAPTURE.store(capture as u8, Ordering::Relaxed);
}

/// The capture rule in force.
#[must_use]
pub fn capture() -> Capture {
    if CAPTURE.load(Ordering::Relaxed) == Capture::Focus as u8 { Capture::Focus } else { Capture::All }
}

struct Keys {
    toggle: Option<Hotkey>,
    close: Option<Hotkey>,
    /// Keys of actions bound to the overlay.
    overlay: Vec<Hotkey>,
    capturing: bool,
}

/// The mouse as the game saw it between two presents: the summed movement in the device's counts
/// and every button press (`true`) and release (`false`) in order. Buttons: 0 left, 1 right,
/// 2 middle, 3-7 the others; 8 and 9 are the wheel's steps up and down, pressed once per step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameMouse {
    pub dx: i32,
    pub dy: i32,
    pub buttons: Vec<(u32, bool)>,
}

/// The gamepad as the game saw it between two presents: the button presses (`true`) and releases
/// (`false`) in order, and the buttons, triggers and sticks as the last event left them. Buttons
/// are `XInput` masks: 0x1 d-pad up, 0x2 down, 0x4 left, 0x8 right, 0x10 start, 0x20 back, 0x40
/// left stick, 0x80 right stick, 0x100 left shoulder, 0x200 right shoulder, 0x1000 A, 0x2000 B,
/// 0x4000 X, 0x8000 Y.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GamePad {
    pub buttons: Vec<(u32, bool)>,
    /// The buttons down, as a mask.
    pub held: u32,
    /// Left and right trigger (button ids 9 and 10), 0 released to 1.
    pub triggers: [f32; 2],
    /// Left and right stick (thumbstick ids 0xb and 0xc), x and y.
    pub sticks: [[f32; 2]; 2],
}

impl GamePad {
    const IDLE: Self = Self { buttons: Vec::new(), held: 0, triggers: [0.0; 2], sticks: [[0.0; 2]; 2] };

    /// A button event: the triggers carry their axis in the value; a button is pressed on its
    /// first frame and released when its value drops to 0.
    fn button(&mut self, id: u32, value: f32, held_secs: f32) {
        match id {
            TRIGGER_LEFT | TRIGGER_RIGHT => self.triggers[(id - TRIGGER_LEFT) as usize] = value,
            _ if value != 0.0 && held_secs == 0.0 => {
                self.buttons.push((id, true));
                self.held |= id;
            }
            _ if value == 0.0 => {
                self.buttons.push((id, false));
                self.held &= !id;
            }
            _ => {}
        }
    }

    fn stick(&mut self, id: u32, x: f32, y: f32) {
        if let Some(stick) = id.checked_sub(STICK_LEFT).and_then(|i| self.sticks.get_mut(i as usize)) {
            *stick = [x, y];
        }
    }
}

const TRIGGER_LEFT: u32 = 9;
const TRIGGER_RIGHT: u32 = 10;
const STICK_LEFT: u32 = 0xB;

/// What one present reports: key presses with who had each, the game's mouse and gamepad, and
/// the keyboard releases that reached the game with the seconds each key was held.
struct Reported {
    presses: Vec<(Hotkey, Scope)>,
    mouse: GameMouse,
    pad: GamePad,
    releases: Vec<(u32, f32)>,
}

impl Reported {
    const EMPTY: Self = Self { presses: Vec::new(), mouse: GameMouse { dx: 0, dy: 0, buttons: Vec::new() }, pad: GamePad::IDLE, releases: Vec::new() };
}

/// Those since the last present, and those the current present reports.
struct Presses {
    pending: Reported,
    current: Reported,
}

struct Pending {
    events: Vec<Event>,
    /// The modifiers held in the frame being read.
    modifiers: Modifiers,
    pointer: Pos2,
    screen: Vec2,
    pixels_per_point: f32,
    /// Keys whose press reached the game and whose release has not yet.
    game_held: [bool; 256],
    /// Keys whose press the overlay took for itself: their holds and release stay hidden too.
    taken: [bool; 256],
}

static PENDING: Mutex<Pending> = Mutex::new(Pending { events: Vec::new(), modifiers: Modifiers::NONE, pointer: Pos2::ZERO, screen: Vec2::ZERO, pixels_per_point: 1.0, game_held: [false; 256], taken: [false; 256] });
static KEYS: Mutex<Keys> = Mutex::new(Keys { toggle: Some(DEFAULT_TOGGLE), close: Some(DEFAULT_CLOSE), overlay: Vec::new(), capturing: false });
static PRESSES: Mutex<Presses> = Mutex::new(Presses { pending: Reported::EMPTY, current: Reported::EMPTY });
static LISTENER: OnceLock<fn(KeyUse)> = OnceLock::new();
static FILTER: OnceLock<fn(&ButtonEvent) -> bool> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);
static SINK: Sink<InputEventList, Gate> = Sink::new(Gate);

/// Binds the key that opens and closes the overlay, open or closed; `None` leaves only [`crate::set_open`].
pub fn set_toggle(key: Option<Hotkey>) {
    KEYS.lock().unwrap().toggle = key;
}

/// Binds the key that, while the overlay is open, releases the focused widget or else closes the overlay.
pub fn set_close(key: Option<Hotkey>) {
    KEYS.lock().unwrap().close = key;
}

/// The keys of actions bound to the overlay: while it is open with no widget focused, their
/// presses are recorded for [`pressed`] and never reach the game.
pub fn set_overlay_keys(keys: Vec<Hotkey>) {
    KEYS.lock().unwrap().overlay = keys;
}

/// Arms (or disarms) the capture of the next key pressed with its modifiers, whatever it is bound
/// to; the key goes to [`on_key`] as [`KeyUse::Captured`] and nowhere else.
pub fn capture_next_key(armed: bool) {
    KEYS.lock().unwrap().capturing = armed;
}

/// Whether a key capture is armed.
#[must_use]
pub fn capturing() -> bool {
    KEYS.lock().unwrap().capturing
}

/// Calls `f` on the main thread with every key the overlay took; the first call sets it for the life of the game.
pub fn on_key(f: fn(KeyUse)) {
    let _ = LISTENER.set(f);
}

/// Calls `f` on the input thread with every button event of a frame that reaches the game, before
/// the game's handlers see it; the handlers pass over an event `f` returns `true` for
/// (`ButtonEventExt::skip`). The first call sets it for the life of the game.
pub fn filter_game_buttons(f: fn(&ButtonEvent) -> bool) {
    let _ = FILTER.set(f);
}

/// Whether `key` was pressed while `scope` had it before this frame's present and after the
/// previous one's: true from the start of the present after the press (its frame callbacks and
/// windows) to the start of the next.
#[must_use]
pub fn pressed(key: Hotkey, scope: Scope) -> bool {
    PRESSES.lock().unwrap().current.presses.contains(&(key, scope))
}

/// What the mouse did in the events that reached the game before this frame's present and after
/// the previous one's, reported from the start of the present after them to the start of the next.
/// A frame the overlay keeps from the game is not in it.
#[must_use]
pub fn game_mouse() -> GameMouse {
    PRESSES.lock().unwrap().current.mouse.clone()
}

/// What the gamepad did in the events that reached the game, in the same window as [`game_mouse`];
/// the held buttons, triggers and sticks carry over from window to window.
#[must_use]
pub fn game_pad() -> GamePad {
    PRESSES.lock().unwrap().current.pad.clone()
}

/// Whether the keyboard key `scan_code` was released in the game in the same window as
/// [`game_mouse`], with the seconds it was held (the event's own count, from its press frame).
#[must_use]
pub fn game_key_released(scan_code: u32) -> Option<f32> {
    PRESSES.lock().unwrap().current.releases.iter().rev().find(|(code, _)| *code == scan_code).map(|(_, held)| *held)
}

/// Whether the keyboard key `scan_code` is down in the game: its press reached the game and its
/// release has not yet.
#[must_use]
pub fn game_key_held(scan_code: u32) -> bool {
    PENDING.lock().unwrap().game_held[scan_code as usize & 0xFF]
}

/// Makes the presses since the last present the ones [`pressed`] reports; at the start of every present.
pub(crate) fn begin_frame() {
    let mut presses = PRESSES.lock().unwrap();
    let pad = GamePad { buttons: Vec::new(), ..presses.pending.pad.clone() };
    presses.current = core::mem::replace(&mut presses.pending, Reported { pad, ..Reported::EMPTY });
}

impl Reported {
    /// Adds the mouse and gamepad events and the keyboard releases of a frame's list that reached the game.
    fn record(&mut self, list: &InputEventList) {
        for event in list.iter() {
            match (InputDevice::from_u32(event.device), event.as_button(), event.as_mouse_move()) {
                (Some(InputDevice::Mouse), _, Some(m)) => self.mouse.moved(m.dx.cast_signed(), m.dy.cast_signed()),
                (Some(InputDevice::Mouse), Some(b), _) => self.mouse.button(b.idCode, b.value, b.heldDownSecs),
                (Some(InputDevice::Keyboard), Some(b), _) => self.key(b.idCode, b.value, b.heldDownSecs),
                (Some(InputDevice::Gamepad), Some(b), _) => self.pad.button(b.idCode, b.value, b.heldDownSecs),
                (Some(InputDevice::Gamepad), _, _) => {
                    if let Some(s) = event.as_thumbstick() {
                        self.pad.stick(s.idCode, s.x, s.y);
                    }
                }
                _ => {}
            }
        }
    }

    /// A keyboard event: only the release frame, value 0 with the held time, is kept.
    fn key(&mut self, scan_code: u32, value: f32, held_secs: f32) {
        if value == 0.0 {
            self.releases.push((scan_code, held_secs));
        }
    }
}

impl GameMouse {
    fn moved(&mut self, dx: i32, dy: i32) {
        self.dx = self.dx.saturating_add(dx);
        self.dy = self.dy.saturating_add(dy);
    }

    /// A button event: down on its first frame, up when its value drops to 0; the held frames between add nothing.
    fn button(&mut self, id: u32, value: f32, held_secs: f32) {
        if value != 0.0 && held_secs == 0.0 {
            self.buttons.push((id, true));
        } else if value == 0.0 {
            self.buttons.push((id, false));
        }
    }
}

pub(crate) fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Adds the sink once the input manager exists; called from the frame on the main thread.
pub(crate) fn ensure_sink() {
    if installed() {
        return;
    }
    if let Some(source) = InputEventList::source() {
        source.add_sink_first(&SINK).leak();
        INSTALLED.store(true, Ordering::Relaxed);
    }
}

/// The events since the last frame, the pointer and the modifiers held, with the screen size in
/// points the next moves are clamped to and its scale.
pub(crate) fn take(screen: Vec2, pixels_per_point: f32) -> (Vec<Event>, Pos2, Modifiers) {
    let mut p = PENDING.lock().unwrap();
    p.pixels_per_point = pixels_per_point;
    if p.screen != screen {
        p.screen = screen;
        p.pointer = (screen / 2.0).to_pos2();
    }
    (core::mem::take(&mut p.events), p.pointer, p.modifiers)
}

/// The game hides the system cursor, so the overlay draws one: the game's (its texture and size in
/// points, its top left corner the point), or an arrow of its own.
pub(crate) fn cursor_shape(at: Pos2, game: Option<(egui::TextureId, Vec2)>) -> egui::Shape {
    if let Some((texture, size)) = game {
        return egui::Shape::image(texture, egui::Rect::from_min_size(at, size), egui::Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    let points = vec![at, at + egui::vec2(0.0, 18.0), at + egui::vec2(5.0, 13.5), at + egui::vec2(12.5, 13.0)];
    egui::Shape::convex_polygon(points, egui::Color32::WHITE, egui::Stroke::new(1.0, egui::Color32::BLACK))
}

/// What the guest drew last frame: its rect and whether it wanted the pointer and the keyboard.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Claim {
    pub rect: egui::Rect,
    pub pointer: bool,
    pub keyboard: bool,
}

/// The pointer across frames: where it is, the buttons down, and whether the guest has them.
#[derive(Default)]
pub(crate) struct Router {
    pointer: Pos2,
    down: u8,
    guest_has: Option<bool>,
}

/// A frame's events split between the overlay's context and the guest, and who got a press.
#[derive(Default)]
pub(crate) struct Routed {
    pub host: Vec<Event>,
    pub guest: Vec<Event>,
    pub host_pressed: bool,
    pub guest_pressed: bool,
}

impl Router {
    /// Splits `events` by the guest's last frame; `host_at` says whether one of the overlay's own
    /// areas is at a point. Without a guest everything is the overlay's.
    pub(crate) fn route(&mut self, events: Vec<Event>, claim: Option<Claim>, host_at: impl Fn(Pos2) -> bool) -> Routed {
        let mut routed = Routed::default();
        let Some(claim) = claim else {
            self.guest_has = None;
            routed.host = events;
            return routed;
        };
        for event in events {
            match event {
                Event::PointerMoved(pos) => {
                    self.pointer = pos;
                    routed.host.push(if self.to_guest(claim, &host_at) { Event::PointerGone } else { event.clone() });
                    routed.guest.push(event);
                }
                Event::PointerButton { pos, button, pressed, .. } => {
                    self.pointer = pos;
                    let guest = self.to_guest(claim, &host_at);
                    let bit = 1 << button as u8;
                    if pressed {
                        self.down |= bit;
                        self.guest_has.get_or_insert(guest);
                        if guest { routed.guest_pressed = true } else { routed.host_pressed = true }
                    } else {
                        self.down &= !bit;
                        if self.down == 0 {
                            self.guest_has = None;
                        }
                    }
                    if guest { routed.guest.push(event) } else { routed.host.push(event) }
                }
                Event::MouseWheel { .. } => {
                    if self.to_guest(claim, &host_at) { routed.guest.push(event) } else { routed.host.push(event) }
                }
                Event::PointerGone => {
                    routed.host.push(event.clone());
                    routed.guest.push(event);
                }
                _ if claim.keyboard => routed.guest.push(event),
                _ => routed.host.push(event),
            }
        }
        routed
    }

    fn to_guest(&self, claim: Claim, host_at: impl Fn(Pos2) -> bool) -> bool {
        self.guest_has.unwrap_or_else(|| claim.pointer || claim.rect.contains(self.pointer) || !host_at(self.pointer))
    }
}

struct Gate;

/// Sends `PlayerControls` an empty list for a list the overlay stopped. It zeroes its look and move
/// input at the start of every list it is sent, so a frame it never saw would keep the last mouse
/// move turning the camera.
fn no_player_input(source: &BSTEventSource<InputEventList>) {
    if let Some(controls) = sre::PlayerControls::singleton() {
        let mut head: *mut InputEvent = core::ptr::null_mut();
        let _ = unsafe { controls.process_event(&raw mut head, core::ptr::from_ref(source).cast_mut().cast()) };
    }
}

impl EventSink<InputEventList> for Gate {
    fn process(&self, list: &InputEventList, source: &BSTEventSource<InputEventList>) -> BSEventNotifyControl {
        let mut keys = KEYS.lock().unwrap();
        let mut p = PENDING.lock().unwrap();
        p.modifiers = held_modifiers(list);
        let focus = crate::keyboard_focus();
        let mut stop = crate::is_open();
        let (mut taken, mut keyboard, mut overlay_only, mut game_release) = (false, false, false, false);
        let mut reports = Vec::new();
        let mut presses = Vec::new();
        for event in list.iter() {
            let device = InputDevice::from_u32(event.device);
            let key = event.as_button().filter(|_| device == Some(InputDevice::Keyboard));
            let mut to_egui = crate::is_open();
            if let Some(b) = key {
                let code = b.idCode as usize & 0xFF;
                let released = !b.is_pressed() && b.value == 0.0;
                let mut took = false;
                if b.is_pressed() {
                    let hotkey = Hotkey::held(b.idCode, p.modifiers);
                    let open = crate::is_open();
                    let used = if keys.capturing && !is_modifier(b.idCode) {
                        keys.capturing = false;
                        Some(KeyUse::Captured(hotkey))
                    } else if keys.toggle == Some(hotkey) {
                        crate::set_open(!open);
                        Some(KeyUse::Toggled(hotkey, !open))
                    } else if open && keys.close == Some(hotkey) {
                        if focus {
                            Some(KeyUse::FocusReleased(hotkey))
                        } else {
                            crate::set_open(false);
                            Some(KeyUse::Closed(hotkey))
                        }
                    } else {
                        None
                    };
                    let action = used.is_none() && open && !focus && keys.overlay.contains(&hotkey);
                    if used.is_some() || action {
                        p.taken[code] = true;
                        took = true;
                        to_egui = matches!(used, Some(KeyUse::FocusReleased(_)));
                    }
                    reports.extend(used);
                    presses.push((hotkey, if action { Scope::Overlay } else { Scope::Game }));
                } else if p.taken[code] {
                    p.taken[code] = !released;
                    took = true;
                }
                if took {
                    taken = true;
                } else {
                    keyboard = true;
                    overlay_only |= b.idCode == TAB;
                    game_release |= released && p.game_held[code];
                }
            } else if device == Some(InputDevice::Mouse) {
                overlay_only = true;
            }
            if to_egui {
                p.translate(event);
            }
        }
        if taken {
            stop = true;
        } else if stop && capture() == Capture::Focus && !focus && keyboard && (game_release || !overlay_only) {
            stop = false;
        }
        if !stop {
            for b in list.iter().filter(|e| InputDevice::from_u32(e.device) == Some(InputDevice::Keyboard)).filter_map(|e| e.as_button()) {
                let held = &mut p.game_held[b.idCode as usize & 0xFF];
                if b.is_pressed() {
                    *held = true;
                } else if b.value == 0.0 {
                    *held = false;
                }
            }
        }
        drop((keys, p));
        if !stop && let Some(filter) = FILTER.get() {
            for b in list.iter().filter_map(|e| e.as_button()).filter(|b| filter(b)) {
                b.skip();
            }
        }
        let mut recorded = PRESSES.lock().unwrap();
        recorded.pending.presses.extend(presses.into_iter().filter(|(_, scope)| *scope == Scope::Overlay || !stop));
        if !stop {
            recorded.pending.record(list);
        }
        drop(recorded);
        if let Some(listener) = LISTENER.get() {
            for report in reports {
                listener(report);
            }
        }
        if !stop {
            return BSEventNotifyControl::Continue;
        }
        no_player_input(source);
        BSEventNotifyControl::Stop
    }
}

impl Pending {
    #[allow(clippy::cast_precision_loss)]
    fn translate(&mut self, event: &InputEvent) {
        let modifiers = self.modifiers;
        if let Some(m) = event.as_mouse_move() {
            let (dx, dy) = (m.dx.cast_signed(), m.dy.cast_signed());
            self.pointer = (self.pointer + egui::vec2(dx as f32, dy as f32) / self.pixels_per_point).clamp(Pos2::ZERO, self.screen.to_pos2());
            self.events.push(Event::PointerMoved(self.pointer));
        } else if let Some(c) = event.as_char() {
            if let Some(c) = char::from_u32(c.charCode).filter(|c| !c.is_control()) {
                self.events.push(Event::Text(c.to_string()));
            }
        } else if let Some(b) = event.as_button() {
            let pressed = b.is_pressed();
            if !pressed && b.value != 0.0 {
                return;
            }
            match InputDevice::from_u32(event.device) {
                Some(InputDevice::Keyboard) => {
                    if let Some(key) = key(b.idCode) {
                        self.events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
                    }
                }
                Some(InputDevice::Mouse) => {
                    let button = match b.idCode {
                        0 => PointerButton::Primary,
                        1 => PointerButton::Secondary,
                        2 => PointerButton::Middle,
                        WHEEL_UP | WHEEL_DOWN if pressed => {
                            let delta = egui::vec2(0.0, if b.idCode == WHEEL_UP { 1.0 } else { -1.0 });
                            self.events.push(Event::MouseWheel { unit: MouseWheelUnit::Line, delta, phase: TouchPhase::Move, source: MouseWheelSource::Wheel, modifiers });
                            return;
                        }
                        _ => return,
                    };
                    self.events.push(Event::PointerButton { pos: self.pointer, button, pressed, modifiers });
                }
                _ => {}
            }
        }
    }
}

fn is_modifier(scan_code: u32) -> bool {
    matches!(scan_code, 0x2A | 0x36 | 0x1D | 0x9D | 0x38 | 0xB8)
}

/// The shift, control and alt keys down in a frame. The engine reports every key down in every
/// frame's list (a press, then a hold event each frame), so a release the game never saw (the
/// window lost the focus while Alt was down) leaves no modifier stuck.
fn held_modifiers(list: &InputEventList) -> Modifiers {
    let mut m = Modifiers::NONE;
    let down = list.iter().filter(|e| InputDevice::from_u32(e.device) == Some(InputDevice::Keyboard)).filter_map(|e| e.as_button()).filter(|b| b.value != 0.0);
    for b in down {
        match b.idCode {
            0x2A | 0x36 => m.shift = true,
            0x1D | 0x9D => {
                m.ctrl = true;
                m.command = true;
            }
            0x38 | 0xB8 => m.alt = true,
            _ => {}
        }
    }
    m
}

/// The name of a scan code: its `egui` key's name, when no lower scan code maps to the same key.
fn key_name(scan_code: u32) -> Option<&'static str> {
    let k = key(scan_code)?;
    ((0..scan_code).all(|c| key(c) != Some(k))).then(|| k.name())
}

/// The scan code a key name (any case, `egui`'s aliases too) or else a number (`87`, `0x57`) stands for.
fn scan_code_named(name: &str) -> Option<u32> {
    let number = match name.strip_prefix("0x").or_else(|| name.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => name.parse().ok(),
    };
    let wanted = Key::from_name(name);
    let named = (1..0x100).find(|&c| key_name(c).is_some_and(|n| n.eq_ignore_ascii_case(name)) || (wanted.is_some() && key(c) == wanted));
    named.or_else(|| number.filter(|code| (1..0x100).contains(code)))
}

/// The `egui` key for a `DirectInput` scan code (dinput.h's `DIK_*` values, US layout positions).
fn key(scan_code: u32) -> Option<Key> {
    const LETTERS: [(u32, Key); 26] = [
        (0x1E, Key::A),
        (0x30, Key::B),
        (0x2E, Key::C),
        (0x20, Key::D),
        (0x12, Key::E),
        (0x21, Key::F),
        (0x22, Key::G),
        (0x23, Key::H),
        (0x17, Key::I),
        (0x24, Key::J),
        (0x25, Key::K),
        (0x26, Key::L),
        (0x32, Key::M),
        (0x31, Key::N),
        (0x18, Key::O),
        (0x19, Key::P),
        (0x10, Key::Q),
        (0x13, Key::R),
        (0x1F, Key::S),
        (0x14, Key::T),
        (0x16, Key::U),
        (0x2F, Key::V),
        (0x11, Key::W),
        (0x2D, Key::X),
        (0x15, Key::Y),
        (0x2C, Key::Z),
    ];
    const DIGITS: [Key; 10] = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::Num0];
    const FUNCTION: [Key; 10] = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9, Key::F10];
    Some(match scan_code {
        0x02..=0x0B => DIGITS[(scan_code - 0x02) as usize],
        0x3B..=0x44 => FUNCTION[(scan_code - 0x3B) as usize],
        0x01 => Key::Escape,
        0x0C => Key::Minus,
        0x0D => Key::Equals,
        0x0E => Key::Backspace,
        0x0F => Key::Tab,
        0x1A => Key::OpenBracket,
        0x1B => Key::CloseBracket,
        0x1C | 0x9C => Key::Enter,
        0x27 => Key::Semicolon,
        0x28 => Key::Quote,
        0x29 => Key::Backtick,
        0x2B => Key::Backslash,
        0x33 => Key::Comma,
        0x34 => Key::Period,
        0x35 => Key::Slash,
        0x39 => Key::Space,
        0x57 => Key::F11,
        0x58 => Key::F12,
        0xC7 => Key::Home,
        0xC8 => Key::ArrowUp,
        0xC9 => Key::PageUp,
        0xCB => Key::ArrowLeft,
        0xCD => Key::ArrowRight,
        0xCF => Key::End,
        0xD0 => Key::ArrowDown,
        0xD1 => Key::PageDown,
        0xD2 => Key::Insert,
        0xD3 => Key::Delete,
        _ => {
            return LETTERS.iter().find(|(code, _)| *code == scan_code).map(|(_, key)| *key);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_mouse_sums_moves_and_keeps_button_edges() {
        let mut m = GameMouse::default();
        m.moved(3, -2);
        m.moved(4, 5);
        m.button(0, 1.0, 0.0);
        m.button(0, 1.0, 0.016);
        m.button(8, 1.0, 0.0);
        m.button(0, 0.0, 0.2);
        assert_eq!(m, GameMouse { dx: 7, dy: 3, buttons: vec![(0, true), (8, true), (0, false)] });
    }

    #[test]
    fn the_gamepad_keeps_edges_held_buttons_triggers_and_sticks() {
        let mut p = GamePad::IDLE;
        p.button(0x100, 1.0, 0.0);
        p.button(0x100, 1.0, 0.02);
        p.button(0x1000, 1.0, 0.0);
        p.button(TRIGGER_RIGHT, 0.6, 0.0);
        p.button(0x100, 0.0, 0.3);
        p.stick(0xC, 0.5, -1.0);
        p.stick(0xD, 1.0, 1.0);
        assert_eq!(p, GamePad { buttons: vec![(0x100, true), (0x1000, true), (0x100, false)], held: 0x1000, triggers: [0.0, 0.6], sticks: [[0.0, 0.0], [0.5, -1.0]] });
    }

    #[test]
    fn a_key_release_keeps_its_held_time() {
        let mut r = Reported::EMPTY;
        r.key(0x21, 1.0, 0.0);
        r.key(0x21, 1.0, 0.1);
        r.key(0x21, 0.0, 0.25);
        r.key(0x12, 1.0, 0.0);
        assert_eq!(r.releases, vec![(0x21, 0.25)]);
    }

    fn button(x: f32, pressed: bool) -> Event {
        Event::PointerButton { pos: egui::pos2(x, 10.0), button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn key(pressed: bool) -> Event {
        Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
    }

    /// The guest's rect is x 100..200; the overlay's own window covers x 0..300.
    fn claim(pointer: bool, keyboard: bool) -> Claim {
        Claim { rect: egui::Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(200.0, 100.0)), pointer, keyboard }
    }

    fn window(p: Pos2) -> bool {
        p.x < 300.0
    }

    #[test]
    fn pointer_events_reach_the_guest_inside_its_rect_and_while_it_wants_them() {
        let mut router = Router::default();
        let inside = router.route(vec![Event::PointerMoved(egui::pos2(150.0, 10.0)), button(150.0, true), button(150.0, false), key(true)], Some(claim(false, false)), window);
        assert_eq!(inside.guest, [Event::PointerMoved(egui::pos2(150.0, 10.0)), button(150.0, true), button(150.0, false)]);
        assert_eq!(inside.host, [Event::PointerGone, key(true)], "the window sees the pointer leave and keeps the keyboard");
        assert!(inside.guest_pressed && !inside.host_pressed);

        let outside = router.route(vec![Event::PointerMoved(egui::pos2(50.0, 10.0)), button(50.0, true), button(50.0, false)], Some(claim(false, false)), window);
        assert_eq!(outside.guest, [Event::PointerMoved(egui::pos2(50.0, 10.0))], "the guest follows the pointer but gets no press over the window");
        assert_eq!(outside.host.len(), 3);
        assert!(outside.host_pressed && !outside.guest_pressed);

        let popup = router.route(vec![button(50.0, true), button(50.0, false)], Some(claim(true, false)), window);
        assert_eq!((popup.guest.len(), popup.host.len()), (2, 0), "outside its rect while it wants the pointer, over its popup");

        let world = router.route(vec![Event::PointerMoved(egui::pos2(400.0, 10.0)), button(400.0, true), Event::PointerMoved(egui::pos2(50.0, 10.0)), button(50.0, false)], Some(claim(false, false)), window);
        assert_eq!(world.guest.len(), 4, "a press over no window, and its drag across the window to the release");
        assert_eq!(world.host, [Event::PointerGone, Event::PointerGone]);

        let drag = router.route(vec![button(150.0, true), Event::PointerMoved(egui::pos2(50.0, 10.0)), button(50.0, false), button(50.0, true)], Some(claim(false, false)), window);
        assert_eq!(drag.guest.len(), 3, "a drag begun in the rect stays the guest's until released");
        assert_eq!(drag.host, [Event::PointerGone, button(50.0, true)]);
    }

    #[test]
    fn keys_reach_the_guest_only_while_it_wants_the_keyboard() {
        let mut router = Router::default();
        let focused = router.route(vec![key(true), Event::Text("a".to_owned()), key(false)], Some(claim(false, true)), window);
        assert_eq!((focused.guest.len(), focused.host.len()), (3, 0));
        let unfocused = router.route(vec![key(true), Event::Text("a".to_owned())], Some(claim(false, false)), window);
        assert_eq!((unfocused.guest.len(), unfocused.host.len()), (0, 2));
        let alone = router.route(vec![button(150.0, true), key(true)], None, window);
        assert_eq!((alone.guest.len(), alone.host.len()), (0, 2), "without a guest everything is the window's");
    }

    #[test]
    fn hotkeys_read_and_write_by_name() {
        assert_eq!(DEFAULT_TOGGLE.to_string(), "F11");
        assert_eq!(DEFAULT_CLOSE.to_string(), "Escape");
        let k: Hotkey = "Ctrl+Shift+K".parse().unwrap();
        assert_eq!(k, Hotkey { scan_code: 0x25, ctrl: true, shift: true, alt: false });
        assert_eq!(k.to_string(), "Ctrl+Shift+K");
        assert_eq!("shift + k".parse::<Hotkey>(), Ok(Hotkey { shift: true, ..Hotkey::new(0x25) }));
        assert_eq!("87".parse::<Hotkey>(), Ok(DEFAULT_TOGGLE));
        assert_eq!("0x57".parse::<Hotkey>(), Ok(DEFAULT_TOGGLE));
        assert_eq!("Esc".parse::<Hotkey>(), Ok(DEFAULT_CLOSE));
        assert_eq!("escape".parse::<Hotkey>(), Ok(DEFAULT_CLOSE));
        assert_eq!("1".parse::<Hotkey>(), Ok(Hotkey::new(0x02)));
        assert_eq!("Digit1".parse::<Hotkey>(), Ok(Hotkey::new(0x02)));
        assert_eq!(Hotkey::new(0x9C).to_string(), "0x9C");
        assert_eq!(Hotkey::new(0x2A).to_string(), "0x2A");
        assert_eq!("0x2A".parse::<Hotkey>(), Ok(Hotkey::new(0x2A)));
        assert!("Ctrl+".parse::<Hotkey>().is_err());
        assert!("Hyper+K".parse::<Hotkey>().is_err());
        assert!("0x100".parse::<Hotkey>().is_err());
        for code in 1..0x100 {
            let k = Hotkey { ctrl: true, alt: true, ..Hotkey::new(code) };
            assert_eq!(k.to_string().parse::<Hotkey>(), Ok(k), "{k}");
        }
    }
}
