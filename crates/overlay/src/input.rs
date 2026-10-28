//! The input source's first sink: the overlay's keys (the toggle, the close key, the keys bound to
//! the overlay and a key being captured), and while the overlay is open every event of the frame as
//! `egui` events, hidden from the sinks after it unless [`Capture::Focus`] lets the frame through to
//! the game.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

use egui::{Context, Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2};
use skyrim::events::{BSTEventSource, Event as _, EventSink, InputEventList, Sink};
use skyrim::prelude::*;
use sre::{BSEventNotifyControl, InputDevice, InputEvent};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    /// The key's `DirectInput` scan code.
    pub scan_code: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

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

/// Who has a key when an action bound to it fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The overlay, open with no widget focused; while it is open the key never reaches the game.
    Overlay,
    /// The game: the frame of the press reached the game's handlers.
    Game,
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

/// Key presses with who had each: those since the last present, and those the current present reports.
struct Presses {
    pending: Vec<(Hotkey, Scope)>,
    current: Vec<(Hotkey, Scope)>,
}

struct Pending {
    events: Vec<Event>,
    /// The modifiers held, tracked whether the overlay is open or not.
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
static PRESSES: Mutex<Presses> = Mutex::new(Presses { pending: Vec::new(), current: Vec::new() });
static LISTENER: OnceLock<fn(KeyUse)> = OnceLock::new();
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

/// Whether `key` was pressed while `scope` had it before this frame's present and after the
/// previous one's: true from the start of the present after the press (its frame callbacks and
/// windows) to the start of the next.
#[must_use]
pub fn pressed(key: Hotkey, scope: Scope) -> bool {
    PRESSES.lock().unwrap().current.contains(&(key, scope))
}

/// Makes the presses since the last present the ones [`pressed`] reports; at the start of every present.
pub(crate) fn begin_frame() {
    let mut presses = PRESSES.lock().unwrap();
    presses.current = core::mem::take(&mut presses.pending);
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

/// The events since the last frame and the pointer, with the screen size in points the next moves
/// are clamped to and its scale.
pub(crate) fn take(screen: Vec2, pixels_per_point: f32) -> (Vec<Event>, Pos2) {
    let mut p = PENDING.lock().unwrap();
    p.pixels_per_point = pixels_per_point;
    if p.screen != screen {
        p.screen = screen;
        p.pointer = (screen / 2.0).to_pos2();
    }
    (core::mem::take(&mut p.events), p.pointer)
}

/// The game hides the system cursor, so the overlay draws its own.
pub(crate) fn draw_cursor(ctx: &Context, at: Pos2) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("overlay cursor")));
    let points = vec![at, at + egui::vec2(0.0, 18.0), at + egui::vec2(5.0, 13.5), at + egui::vec2(12.5, 13.0)];
    painter.add(egui::Shape::convex_polygon(points, egui::Color32::WHITE, egui::Stroke::new(1.0, egui::Color32::BLACK)));
}

struct Gate;

impl EventSink<InputEventList> for Gate {
    fn process(&self, list: &InputEventList, _: &BSTEventSource<InputEventList>) -> BSEventNotifyControl {
        let mut keys = KEYS.lock().unwrap();
        let mut p = PENDING.lock().unwrap();
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
            if let Some(b) = key {
                set_modifier(&mut p.modifiers, b.idCode, b.value != 0.0);
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
        PRESSES.lock().unwrap().pending.extend(presses.into_iter().filter(|(_, scope)| *scope == Scope::Overlay || !stop));
        if let Some(listener) = LISTENER.get() {
            for report in reports {
                listener(report);
            }
        }
        if stop { BSEventNotifyControl::Stop } else { BSEventNotifyControl::Continue }
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

/// Records a shift, control or alt key going down or up; any other key is ignored.
fn set_modifier(m: &mut Modifiers, scan_code: u32, down: bool) {
    match scan_code {
        0x2A | 0x36 => m.shift = down,
        0x1D | 0x9D => {
            m.ctrl = down;
            m.command = down;
        }
        0x38 | 0xB8 => m.alt = down,
        _ => {}
    }
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
