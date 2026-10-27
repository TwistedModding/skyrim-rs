//! The input source's first sink: the toggle key, and while the overlay is open every event of the
//! frame as `egui` events, hidden from the sinks after it unless [`Capture::Focus`] lets the frame
//! through to the game.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use egui::{Context, Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2};
use skyrim::events::{BSTEventSource, Event as _, EventSink, InputEventList, Sink};
use skyrim::prelude::*;
use sre::{BSEventNotifyControl, InputDevice, InputEvent};

/// F11's `DirectInput` scan code.
pub const DEFAULT_TOGGLE: u32 = 0x57;

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
    /// saw pressed always passes, so no key stays held in the game; its mouse events then reach
    /// the game too.
    Focus,
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

struct Pending {
    events: Vec<Event>,
    modifiers: Modifiers,
    pointer: Pos2,
    screen: Vec2,
    pixels_per_point: f32,
    /// Keys whose press reached the game and whose release has not yet.
    game_held: [bool; 256],
}

static PENDING: Mutex<Pending> = Mutex::new(Pending { events: Vec::new(), modifiers: Modifiers::NONE, pointer: Pos2::ZERO, screen: Vec2::ZERO, pixels_per_point: 1.0, game_held: [false; 256] });
static TOGGLE: AtomicU32 = AtomicU32::new(DEFAULT_TOGGLE);
static INSTALLED: AtomicBool = AtomicBool::new(false);
static SINK: Sink<InputEventList, Gate> = Sink::new(Gate);

pub(crate) fn set_toggle(scan_code: u32) {
    TOGGLE.store(scan_code, Ordering::Relaxed);
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
        let toggle = TOGGLE.load(Ordering::Relaxed);
        let mut stop = crate::is_open();
        let mut p = PENDING.lock().unwrap();
        let (mut keyboard, mut overlay_only, mut game_release) = (false, false, false);
        for event in list.iter() {
            let key = event.as_button().filter(|_| InputDevice::from_u32(event.device) == Some(InputDevice::Keyboard));
            if let Some(b) = key
                && b.idCode == toggle
            {
                if b.is_pressed() {
                    crate::set_open(!crate::is_open());
                }
                stop = true;
                continue;
            }
            match InputDevice::from_u32(event.device) {
                Some(InputDevice::Keyboard) => {
                    keyboard = true;
                    overlay_only |= key.is_some_and(|b| b.idCode == TAB);
                    game_release |= key.is_some_and(|b| b.value == 0.0 && p.game_held.get(b.idCode as usize).copied().unwrap_or(false));
                }
                Some(InputDevice::Mouse) => overlay_only = true,
                _ => {}
            }
            if crate::is_open() {
                p.translate(event);
            }
        }
        if stop && crate::is_open() && capture() == Capture::Focus && !crate::keyboard_focus() && keyboard && (game_release || !overlay_only) {
            stop = false;
        }
        if !stop {
            for b in list.iter().filter(|e| InputDevice::from_u32(e.device) == Some(InputDevice::Keyboard)).filter_map(|e| e.as_button()) {
                if let Some(held) = p.game_held.get_mut(b.idCode as usize) {
                    if b.is_pressed() {
                        *held = true;
                    } else if b.value == 0.0 {
                        *held = false;
                    }
                }
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
                    if !set_modifier(&mut self.modifiers, b.idCode, pressed)
                        && let Some(key) = key(b.idCode)
                    {
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

/// Records a shift, control or alt key; false for any other key.
fn set_modifier(m: &mut Modifiers, scan_code: u32, down: bool) -> bool {
    match scan_code {
        0x2A | 0x36 => m.shift = down,
        0x1D | 0x9D => {
            m.ctrl = down;
            m.command = down;
        }
        0x38 | 0xB8 => m.alt = down,
        _ => return false,
    }
    true
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
