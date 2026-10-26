//! The framework's in-game overlay: `egui` windows drawn over the finished frame and fed by the
//! game's input while open.
//!
//! [`install`] redirects `RenderFrame`'s call to `PresentFrame` (the frame pass: the frame's
//! present on the main thread, after the world, the menus and the screenshot; the loading screen
//! and the Bink loop present through other callers and are left alone) and, once the input
//! manager exists, puts a sink at the front of the input source. The toggle key opens and closes
//! the overlay; while it is open the sink turns the frame's keys, characters, mouse moves, buttons
//! and wheel steps into `egui` events and ends the walk, so neither the game's handlers nor other
//! plugins' sinks see them. Every frame the overlay is open, the windows added with [`add`] run
//! on the main thread and the result is painted into the back buffer with a Direct3D 11 pipeline
//! of this crate's own. Text uses Segoe UI and Consolas from the Windows font folder: `egui`'s
//! bundled fonts are not under a license the workspace allows.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub use egui;
use egui::{Context, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use layout::v1_7_104::{fns, globals};
use skyrim::render::{PresentFn, RenderWindow};

mod input;
mod paint;

pub use input::DEFAULT_TOGGLE;

/// How far into `RenderFrame` its one call to `PresentFrame` is looked for (it sits at +0x601).
const RENDER_FRAME_LEN: usize = 0x800;

#[derive(Debug)]
pub enum Error {
    Hook(hook::Error),
    /// `RenderFrame` holds no `call rel32` to `PresentFrame`.
    NoPresentCall,
    /// Neither `segoeui.ttf` nor `consola.ttf` could be read from the Windows font folder.
    NoFont,
    AlreadyInstalled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hook(e) => e.fmt(f),
            Self::NoPresentCall => f.write_str("no call to PresentFrame found in RenderFrame"),
            Self::NoFont => f.write_str("no system font to draw text with"),
            Self::AlreadyInstalled => f.write_str("the overlay is already installed"),
        }
    }
}

impl std::error::Error for Error {}

impl From<hook::Error> for Error {
    fn from(e: hook::Error) -> Self {
        Self::Hook(e)
    }
}

type Window = Box<dyn FnMut(&Context) + Send>;

struct State {
    ctx: Context,
    painter: Option<paint::Painter>,
    start: Instant,
}

static SITE: OnceLock<hook::CallSite<PresentFn>> = OnceLock::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
static WINDOWS: Mutex<Vec<Window>> = Mutex::new(Vec::new());
static OPEN: AtomicBool = AtomicBool::new(false);
static FAILURE: OnceLock<String> = OnceLock::new();
static FRAME: OnceLock<fn()> = OnceLock::new();

/// Installs the overlay with `toggle` (a keyboard scan code, [`DEFAULT_TOGGLE`] is F11) as the key
/// that opens and closes it. Call it once from `SKSEPlugin_Load`, after `skyrim::init`, before the
/// first frame is drawn: the call site is patched in place.
pub fn install(toggle: u32) -> Result<(), Error> {
    let fonts = system_fonts().ok_or(Error::NoFont)?;
    let reloc = skyrim::reloc();
    let render_frame = reloc.address(fns::RenderFrame)?;
    let present = reloc.address(fns::PresentFrame)?;
    let site = hook::find_call(render_frame, RENDER_FRAME_LEN, present).ok_or(Error::NoPresentCall)?;
    let mut state = STATE.lock().unwrap();
    if state.is_some() {
        return Err(Error::AlreadyInstalled);
    }
    let ctx = Context::default();
    ctx.set_fonts(fonts);
    *state = Some(State { ctx, painter: None, start: Instant::now() });
    input::set_toggle(toggle);
    let _ = SITE.set(unsafe { hook::CallSite::install(site, present_detour as PresentFn) }?);
    Ok(())
}

/// Adds a window: `draw` runs on the main thread every frame the overlay is open, with the
/// overlay's `egui` context (`egui::Window::new(..).show(ctx, ..)`).
pub fn add(draw: impl FnMut(&Context) + Send + 'static) {
    WINDOWS.lock().unwrap().push(Box::new(draw));
}

/// Runs `f` on the main thread at every present the overlay sees, open or not, before its
/// windows; the first call sets it for the life of the game.
pub fn on_frame(f: fn()) {
    let _ = FRAME.set(f);
}

/// Replaces the `egui` context with a fresh one. What a context accumulates (widget state, the
/// fonts' shaping tables, caches) holds objects and function pointers of the copy of `egui` that
/// first used it, which is the DLL that drew the windows, so a host resets the context before
/// unloading such a DLL. Every window starts afresh; the font atlas is sent again.
pub fn reset_context() {
    let Some(fonts) = system_fonts() else { return };
    if let Some(state) = STATE.lock().unwrap().as_mut() {
        let ctx = Context::default();
        ctx.set_fonts(fonts);
        state.ctx = ctx;
    }
}

#[must_use]
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Opens or closes the overlay, as the toggle key does.
pub fn set_open(open: bool) {
    OPEN.store(open, Ordering::Relaxed);
}

/// Why the overlay stopped drawing (its Direct3D objects could not be created), `None` while it works.
#[must_use]
pub fn failure() -> Option<&'static str> {
    FAILURE.get().map(String::as_str)
}

/// Whether the input sink is in place: it is added at the first frame after the input manager exists.
#[must_use]
pub fn captures_input() -> bool {
    input::installed()
}

unsafe extern "C" fn present_detour() {
    input::ensure_sink();
    if let Some(frame) = FRAME.get() {
        frame();
    }
    if is_open() && FAILURE.get().is_none() {
        draw();
    }
    unsafe { SITE.get().expect("stored before the call site was patched").original()() }
}

#[allow(clippy::cast_precision_loss)]
fn draw() {
    let Some(rtv) = RenderWindow::current().map(RenderWindow::back_buffer_rtv).filter(|p| !p.is_null()) else {
        return;
    };
    let mut state = STATE.lock().unwrap();
    let Some(state) = state.as_mut() else { return };
    if state.painter.is_none() {
        let reloc = skyrim::reloc();
        let (device, context) = match (reloc.address(globals::g_D3D11Device), reloc.address(globals::g_D3D11Context)) {
            (Ok(d), Ok(c)) => unsafe { (*(d as *const *mut core::ffi::c_void), *(c as *const *mut core::ffi::c_void)) },
            _ => return,
        };
        match unsafe { paint::Painter::new(device, context) } {
            Ok(p) => state.painter = Some(p),
            Err(e) => {
                let _ = FAILURE.set(format!("Direct3D setup: {e}"));
                return;
            }
        }
    }
    let painter = state.painter.as_mut().expect("just created");
    let Some(size) = (unsafe { paint::Painter::target_size(rtv) }) else {
        return;
    };
    let ppp = (size[1] as f32 / 1080.0).max(1.0);
    let screen = egui::vec2(size[0] as f32, size[1] as f32) / ppp;
    let (events, cursor) = input::take(screen, ppp);
    let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)), time: Some(state.start.elapsed().as_secs_f64()), events, focused: true, ..RawInput::default() };
    raw.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(ppp), focused: Some(true), ..ViewportInfo::default() });
    let mut windows = core::mem::take(&mut *WINDOWS.lock().unwrap());
    let output = state.ctx.run_pass(raw, |ctx| {
        for window in &mut windows {
            window(ctx);
        }
        input::draw_cursor(ctx, cursor);
    });
    let mut added = WINDOWS.lock().unwrap();
    windows.append(&mut added);
    *added = windows;
    drop(added);
    let primitives = state.ctx.tessellate(output.shapes, output.pixels_per_point);
    if let Err(e) = unsafe { painter.paint(rtv, size, output.pixels_per_point, &primitives, &output.textures_delta) } {
        let _ = FAILURE.set(format!("paint: {e}"));
    }
}

/// Segoe UI for proportional text and Consolas for monospace, each falling back on the other.
fn system_fonts() -> Option<egui::FontDefinitions> {
    let dir = std::path::PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
    let mut fonts = egui::FontDefinitions::empty();
    for (name, file) in [("segoe", "segoeui.ttf"), ("consolas", "consola.ttf")] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
        }
    }
    let loaded: Vec<String> = fonts.font_data.keys().cloned().collect();
    if loaded.is_empty() {
        return None;
    }
    for (family, first) in [(egui::FontFamily::Proportional, "segoe"), (egui::FontFamily::Monospace, "consolas")] {
        let mut order = loaded.clone();
        order.sort_by_key(|n| n != first);
        fonts.families.insert(family, order);
    }
    Some(fonts)
}
