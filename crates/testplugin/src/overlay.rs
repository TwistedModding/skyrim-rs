//! The overlay suite, run in mode `overlay`: `install` installs the overlay on `RenderFrame`'s call
//! to `PresentFrame` and a probe on `PresentFrame`'s entry (the frame suite's detour there is not
//! installed in this mode), so every frame the overlay draws first and the probe reads the back
//! buffer before the present, on the thread that ran the overlay's windows. The suite's window has a
//! button `DevBench` clicks through the keyboard (Tab focuses it, Enter and Space click it) and a
//! magenta square at a fixed place; a witness sink on the input source counts the W presses the
//! overlay lets through. `SAVE_TESTS` check the clicks, the witness, the square in the back buffer
//! while open and its absence once closed; the frame of the third click is written as a bitmap.

use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use layout::v1_7_104::globals;
use overlay::egui;
use skyrim::events::{BSEventNotifyControl, BSTEventSource, Event, EventSink, Sink};
use skyrim::input::{InputDevice, InputEventList};
use skyrim::render::{PresentFn, RenderWindow};
use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::core::Interface;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use crate::{Test, check};

pub const TESTS: &[(&str, Test)] = &[("installed", installed)];
pub const SAVE_TESTS: &[(&str, Test)] = &[("clicks", clicks), ("capture", capture), ("drawn", drawn), ("closed", closed)];

const KEY_W: u32 = 0x11;
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
static OPEN_PIXEL: Mutex<Option<[u8; 3]>> = Mutex::new(None);
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
        if ui.add_sized([320.0, 160.0], egui::Button::new("Count")).clicked() && CLICKS.fetch_add(1, Ordering::Relaxed) == 2 {
            WANT_SHOT.store(true, Ordering::Relaxed);
        }
        ui.label(format!("clicks {}", CLICKS.load(Ordering::Relaxed)));
    });
    let square = egui::Rect::from_min_size(egui::pos2(SQUARE.0, SQUARE.0), egui::vec2(SQUARE.1, SQUARE.1));
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("probe square"))).rect_filled(square, 0.0, egui::Color32::from_rgb(MAGENTA[0], MAGENTA[1], MAGENTA[2]));
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

/// Copies the back buffer to a staging texture and returns the pixel at the square's centre,
/// writing the whole frame as a bitmap when `save` is set.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn read_back(save: bool) -> Result<Option<[u8; 3]>, String> {
    let e = |e: windows::core::Error| e.to_string();
    let rtv = RenderWindow::current().map(RenderWindow::back_buffer_rtv).filter(|p| !p.is_null()).ok_or("no back buffer")?;
    let reloc = skyrim::reloc();
    let (device, context) = unsafe { (*(reloc.address(globals::g_D3D11Device).map_err(|e| e.to_string())? as *const *mut core::ffi::c_void), *(reloc.address(globals::g_D3D11Context).map_err(|e| e.to_string())? as *const *mut core::ffi::c_void)) };
    let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }.ok_or("no device")?;
    let context = unsafe { ID3D11DeviceContext::from_raw_borrowed(&context) }.ok_or("no context")?;
    let rtv = unsafe { ID3D11RenderTargetView::from_raw_borrowed(&rtv) }.ok_or("no view")?;
    let back = unsafe { rtv.GetResource() }.map_err(e)?.cast::<ID3D11Texture2D>().map_err(e)?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { back.GetDesc(&raw mut desc) };
    let staging_desc = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.cast_unsigned(), MiscFlags: 0, ..desc };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&raw const staging_desc, None, Some(&raw mut staging)) }.map_err(e)?;
    let staging = staging.ok_or("no staging texture")?;
    unsafe { context.CopyResource(&staging, &back) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)) }.map_err(e)?;
    let (w, h, pitch) = (desc.Width as usize, desc.Height as usize, mapped.RowPitch as usize);
    let rows = unsafe { core::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * h) };
    let ppp = (h as f32 / 1080.0).max(1.0);
    let centre = ((SQUARE.0 + SQUARE.1 / 2.0) * ppp) as usize;
    let at = centre * pitch + centre * 4;
    let pixel = rows.get(at..at + 3).map(|p| [p[0], p[1], p[2]]);
    let written = if save { write_bmp(rows, w, h, pitch) } else { Ok(()) };
    unsafe { context.Unmap(&staging, 0) };
    written.map(|()| pixel)
}

fn shot_path() -> std::path::PathBuf {
    crate::LOG_DIR.get().cloned().unwrap_or_default().join("skyrim-rs-test-overlay.bmp")
}

/// A 32-bit top-down bitmap of R8G8B8A8 rows.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn write_bmp(rows: &[u8], w: usize, h: usize, pitch: usize) -> Result<(), String> {
    let path = shot_path();
    let size = w * h * 4;
    let mut out = Vec::with_capacity(54 + size);
    out.extend_from_slice(b"BM");
    for v in [(54 + size) as u32, 0, 54, 40] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    for v in [0u32, size as u32, 2835, 2835, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for y in 0..h {
        for px in rows[y * pitch..y * pitch + w * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    std::fs::File::create(path).and_then(|mut f| f.write_all(&out)).map_err(|e| e.to_string())
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
    let shot = SHOT.lock().unwrap().clone().ok_or("the third click's frame was not captured")??;
    Ok(format!("square pixel {pixel:?} while open; frame written to {shot}"))
}

fn closed() -> Result<String, String> {
    check(!overlay::is_open(), "the overlay is still open")?;
    let pixel = *CLOSED_PIXEL.lock().unwrap();
    check(pixel.is_some_and(|p| p != MAGENTA), &format!("pixel at the square after closing: {pixel:?}"))?;
    Ok(format!("square pixel {pixel:?} after closing"))
}
