//! The overlay suite, run in mode `overlay`: `install` installs the overlay on `RenderFrame`'s call
//! to `PresentFrame` and a probe on `PresentFrame`'s entry (the frame suite's detour there is not
//! installed in this mode), so every frame the overlay draws first and the probe reads the back
//! buffer before the present, on the thread that ran the overlay's windows. The suite's window has a
//! button `DevBench` clicks through the keyboard (Tab focuses it, Enter and Space click it), a text
//! field the suite focuses after the third click and types "skyrim" into with `SendInput` scan codes
//! (real key presses: `DevBench`'s injected keys never become characters, request 9c), and a magenta
//! square at a fixed place; a witness sink on the input source counts the W presses the overlay lets
//! through. `SAVE_TESTS` check the clicks, the typed text, the witness, the square in the back buffer
//! while open and its absence once closed; the frame showing the typed text is written as a bitmap.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use overlay::egui;
use skyrim::events::{BSEventNotifyControl, BSTEventSource, Event, EventSink, Sink};
use skyrim::input::{InputDevice, InputEventList};
use skyrim::render::PresentFn;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use crate::{Test, check};

pub const TESTS: &[(&str, Test)] = &[("installed", installed)];
pub const SAVE_TESTS: &[(&str, Test)] = &[("clicks", clicks), ("typed", typed), ("capture", capture), ("drawn", drawn), ("closed", closed)];

const KEY_W: u32 = 0x11;
/// "skyrim" as `DirectInput` scan codes.
const TYPED: (&str, [u16; 6]) = ("skyrim", [0x1F, 0x25, 0x15, 0x13, 0x17, 0x32]);
/// The square's corner and side, in points (1 point = 1 pixel at 1080 lines).
const SQUARE: (f32, f32) = (16.0, 32.0);
const MAGENTA: [u8; 3] = [255, 0, 255];

static PROBE: OnceLock<hook::Hook<PresentFn>> = OnceLock::new();
static DRAW_THREAD: AtomicU32 = AtomicU32::new(0);
static CLICKS: AtomicU32 = AtomicU32::new(0);
static OPEN_FRAMES: AtomicU32 = AtomicU32::new(0);
static WAS_OPEN: AtomicBool = AtomicBool::new(false);
static WANT_SHOT: AtomicBool = AtomicBool::new(false);
static WITNESS_W: Mutex<(u32, u32)> = Mutex::new((0, 0));
static SHOT: Mutex<Option<Result<String, String>>> = Mutex::new(None);
static TEXT: Mutex<String> = Mutex::new(String::new());
static TYPING: AtomicBool = AtomicBool::new(false);
static OPEN_PIXEL: Mutex<Option<[u8; 3]>> = Mutex::new(None);
/// What reached the game while typing: presses of the typed keys, characters, and whether the game owned the foreground.
static TYPED_KEYS: AtomicU32 = AtomicU32::new(0);
static TYPED_CHARS: AtomicU32 = AtomicU32::new(0);
static TYPING_CONTEXT: Mutex<String> = Mutex::new(String::new());
static CLOSED_PIXEL: Mutex<Option<[u8; 3]>> = Mutex::new(None);

struct Witness;

impl EventSink<InputEventList> for Witness {
    fn process(&self, list: &InputEventList, _: &BSTEventSource<InputEventList>) -> BSEventNotifyControl {
        let presses = list.iter().filter_map(|e| e.as_button().filter(|b| e.device_kind() == Some(InputDevice::Keyboard) && b.id() == KEY_W && b.is_pressed())).count();
        if presses > 0 {
            let mut w = WITNESS_W.lock().unwrap();
            if overlay::is_open() { w.0 += 1 } else { w.1 += 1 }
        }
        BSEventNotifyControl::Continue
    }
}

static WITNESS: Sink<InputEventList, Witness> = Sink::new(Witness);

/// Ahead of the overlay's sink while typing: counts the typed keys' presses and the characters the keyboard poll made of them.
struct TypingWitness;

impl EventSink<InputEventList> for TypingWitness {
    fn process(&self, list: &InputEventList, _: &BSTEventSource<InputEventList>) -> BSEventNotifyControl {
        for e in list.iter() {
            if e.as_char().is_some() {
                TYPED_CHARS.fetch_add(1, Ordering::Relaxed);
            } else if e.as_button().is_some_and(|b| e.device_kind() == Some(InputDevice::Keyboard) && b.is_pressed() && TYPED.1.iter().any(|&c| u32::from(c) == b.id())) {
                TYPED_KEYS.fetch_add(1, Ordering::Relaxed);
            }
        }
        BSEventNotifyControl::Continue
    }
}

static TYPING_WITNESS: Sink<InputEventList, TypingWitness> = Sink::new(TypingWitness);

/// Installs the probe and the overlay with the suite's window; from `SKSEPlugin_Load`, before any frame.
pub fn install() -> Result<String, String> {
    overlay::install(overlay::DEFAULT_TOGGLE).map_err(|e| format!("overlay: {e}"))?;
    overlay::add(window);
    let probe = unsafe { skyrim::render::hook_present(probe) }.map_err(|e| format!("probe: {e}"))?;
    let target = probe.target();
    let _ = PROBE.set(probe);
    PROBE.get().expect("just set").enable().map_err(|e| format!("enable probe: {e}"))?;
    Ok(format!("overlay on RenderFrame's call, probe on PresentFrame at {target:#x}"))
}

/// Adds the witness behind the game's own sinks, after the overlay's.
pub fn register() -> Result<String, String> {
    let source = InputEventList::source().ok_or("no input source")?;
    source.add_sink(&WITNESS).leak();
    Ok(format!("{} sinks", source.sink_count()))
}

fn window(ctx: &egui::Context) {
    DRAW_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    egui::Window::new("skyrim-rs test").anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).collapsible(false).show(ctx, |ui| {
        if ui.add_sized([320.0, 160.0], egui::Button::new("Count")).clicked() {
            CLICKS.fetch_add(1, Ordering::Relaxed);
        }
        ui.label(format!("clicks {}", CLICKS.load(Ordering::Relaxed)));
        let mut text = TEXT.lock().unwrap();
        let field = ui.text_edit_singleline(&mut *text);
        if CLICKS.load(Ordering::Relaxed) == 3 && !TYPING.swap(true, Ordering::Relaxed) {
            field.request_focus();
            if let Some(source) = InputEventList::source() {
                source.add_sink_first(&TYPING_WITNESS).leak();
            }
            std::thread::spawn(type_keys);
        }
        if *text == TYPED.0 && SHOT.lock().unwrap().is_none() {
            WANT_SHOT.store(true, Ordering::Relaxed);
        }
    });
    let square = egui::Rect::from_min_size(egui::pos2(SQUARE.0, SQUARE.0), egui::vec2(SQUARE.1, SQUARE.1));
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("probe square"))).rect_filled(square, 0.0, egui::Color32::from_rgb(MAGENTA[0], MAGENTA[1], MAGENTA[2]));
}

/// Presses and releases each key of [`TYPED`] through `SendInput` with scan codes, the way a keyboard
/// reaches `DirectInput`.
fn type_keys() {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, SendInput};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &raw mut owner) };
    let layout = unsafe { GetKeyboardLayout(DRAW_THREAD.load(Ordering::Relaxed)) } as usize;
    *TYPING_CONTEXT.lock().unwrap() = format!("game in the foreground when typing: {}, main thread keyboard layout {layout:#x}", owner == unsafe { GetCurrentProcessId() });
    for code in TYPED.1 {
        for flags in [KEYEVENTF_SCANCODE, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP] {
            let input = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0, wScan: code, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            unsafe { SendInput(1, &raw const input, i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX)) };
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}

unsafe extern "C" fn probe() {
    if DRAW_THREAD.load(Ordering::Relaxed) != unsafe { GetCurrentThreadId() } {
        return unsafe { PROBE.get().expect("stored before enabling").original()() };
    }
    let open = overlay::is_open();
    let was_open = WAS_OPEN.swap(open, Ordering::Relaxed);
    if open && OPEN_FRAMES.fetch_add(1, Ordering::Relaxed) == 30 {
        *OPEN_PIXEL.lock().unwrap() = read_back(false).ok().flatten();
    }
    if open && WANT_SHOT.swap(false, Ordering::Relaxed) {
        *SHOT.lock().unwrap() = Some(read_back(true).map(|_| shot_path().display().to_string()));
    }
    if was_open && !open {
        *CLOSED_PIXEL.lock().unwrap() = read_back(false).ok().flatten();
    }
    unsafe { PROBE.get().expect("stored before enabling").original()() }
}

/// The pixel at the square's centre in the back buffer, writing the whole frame as a bitmap when `save` is set.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn read_back(save: bool) -> Result<Option<[u8; 3]>, String> {
    let frame = overlay::read_back()?;
    let ppp = (frame.height as f32 / 1080.0).max(1.0);
    let centre = ((SQUARE.0 + SQUARE.1 / 2.0) * ppp) as usize;
    if save {
        frame.write_bmp(&shot_path()).map_err(|e| e.to_string())?;
    }
    Ok(frame.pixel(centre, centre))
}

fn shot_path() -> std::path::PathBuf {
    crate::LOG_DIR.get().cloned().unwrap_or_default().join("skyrim-rs-test-overlay.bmp")
}

fn installed() -> Result<String, String> {
    check(overlay::captures_input(), "the overlay's input sink is not in place")?;
    let source = InputEventList::source().ok_or("no input source")?;
    let first = source.sinks().first().copied().unwrap_or_default();
    let witness = source.sinks().iter().position(|s| core::ptr::eq(*s, (&raw const WITNESS).cast())).ok_or("witness not registered")?;
    Ok(format!("{} sinks, overlay sink first at {first:p}, witness at {witness}, overlay {}", source.sink_count(), if overlay::is_open() { "open" } else { "closed" }))
}

fn clicks() -> Result<String, String> {
    let clicks = CLICKS.load(Ordering::Relaxed);
    check(clicks == 3, &format!("{clicks} clicks, expected 3 (Tab and Enter, Space, the left button)"))?;
    Ok(format!("{clicks} clicks through Tab and Enter, Space and the left button, {} frames drawn open", OPEN_FRAMES.load(Ordering::Relaxed)))
}

fn typed() -> Result<String, String> {
    let text = TEXT.lock().unwrap().clone();
    let seen = format!("{}, typed key presses seen: {}, characters seen: {}", TYPING_CONTEXT.lock().unwrap(), TYPED_KEYS.load(Ordering::Relaxed), TYPED_CHARS.load(Ordering::Relaxed));
    check(text == TYPED.0, &format!("the field holds {text:?} after typing {:?}; {seen}", TYPED.0))?;
    Ok(format!("the focused field received {text:?} from scan-code key presses; {seen}"))
}

fn capture() -> Result<String, String> {
    let (hidden, seen) = *WITNESS_W.lock().unwrap();
    check(hidden == 0, &format!("the witness saw {hidden} W presses while the overlay was open"))?;
    check(seen == 1, &format!("the witness saw {seen} W presses with the overlay closed, expected 1"))?;
    Ok(format!("W presses seen by the witness: {hidden} while open, {seen} after closing"))
}

fn drawn() -> Result<String, String> {
    if let Some(f) = overlay::failure() {
        return Err(f.to_owned());
    }
    let pixel = *OPEN_PIXEL.lock().unwrap();
    check(pixel == Some(MAGENTA), &format!("pixel at the square while open: {pixel:?}"))?;
    let shot = SHOT.lock().unwrap().clone().ok_or("the frame with the typed text was not captured")??;
    Ok(format!("square pixel {pixel:?} while open; frame written to {shot}"))
}

fn closed() -> Result<String, String> {
    check(!overlay::is_open(), "the overlay is still open")?;
    let pixel = *CLOSED_PIXEL.lock().unwrap();
    check(pixel.is_some_and(|p| p != MAGENTA), &format!("pixel at the square after closing: {pixel:?}"))?;
    Ok(format!("square pixel {pixel:?} after closing"))
}
