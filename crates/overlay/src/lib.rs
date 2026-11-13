// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The framework's in-game overlay: `egui` windows drawn over the finished frame and fed by the
//! game's input while open.
//!
//! [`install`] redirects `RenderFrame`'s call to `PresentFrame` (the frame pass: the frame's
//! present on the main thread, after the world, the menus and the screenshot; the loading screen
//! and the Bink loop present through other callers and are left alone; when another plugin has
//! already patched that call, it detours `PresentFrame`'s entry and draws only while `RenderFrame`
//! runs on the calling thread) and, once the input
//! manager exists, puts a sink at the front of the input source. The toggle key (F11) opens and
//! closes the overlay and the close key (Escape) closes it once no widget has the focus; neither
//! key's press nor its release reaches the game while the overlay uses it. While the overlay is
//! open the sink turns the frame's keys, characters, mouse moves, buttons and wheel steps into
//! `egui` events and ends the walk, so neither the game's handlers nor other plugins' sinks see
//! them. A host binds other keys, and keys of its own actions, through [`set_toggle`],
//! [`set_close`] and [`set_overlay_keys`]. Every frame the overlay is open, the windows added with [`add`] run
//! on the main thread and the result is painted into the active frame target with a Direct3D 11 pipeline
//! of this crate's own, in the framework [`theme`]: the look of the game's menus, with the game's
//! own Futura Condensed converted from `Interface\fonts_en.swf` at startup when it can be read and
//! Segoe UI and Consolas from the Windows font folder otherwise and for monospace (`egui`'s bundled
//! fonts are not under a license the workspace allows), and the game's own mouse cursor from
//! `Interface\cursormenu.swf`. The HUD layer set with [`on_hud`] draws every frame, open or not,
//! under the windows, with a [`View`] that places shapes in the world through the frame camera.
//! The HUD layer and the guest set with [`on_guest`] are drawn by other `egui` contexts (a framework
//! plugin's, in its own DLL) and handed over as [`Painted`] meshes: the guest over the windows, in a
//! rect the windows leave for it, with the pointer and keyboard events routed to it.

use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub use egui;
use egui::epaint::{ClippedShape, textures::TexturesDelta};
use egui::{ClippedPrimitive, Context, Event, Modifiers, Pos2, RawInput, Rect, TextureId, ViewportId, ViewportInfo};
use sre::raw::{fns, globals};
use skyrim::render::PresentFn;
use windows::core::Interface;

mod adapter;
mod frame;
mod hud;
mod input;
mod paint;
pub mod theme;
mod vanilla;

pub use adapter::{Adapter, VideoMemory, adapter, video_memory};
pub use frame::{Frame, read_back, save_next_frame};
pub use hud::View;
pub use input::{Capture, DEFAULT_CLOSE, DEFAULT_TOGGLE, GameMouse, GamePad, Hotkey, KeyUse, Scope, capture, capture_next_key, capturing, filter_game_buttons, game_key_held, game_key_released, game_mouse, game_pad, on_key, pressed, set_capture, set_close, set_overlay_keys, set_toggle};

/// How far into `RenderFrame` its one call to `PresentFrame` is looked for (it sits at +0x601).
const RENDER_FRAME_LEN: usize = 0x800;

/// Where [`install`] put the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// On `RenderFrame`'s `call rel32` to `PresentFrame`.
    RenderFrameCall,
    /// On `PresentFrame`'s entry, because another plugin had already patched `RenderFrame`'s call.
    PresentEntry,
}

impl fmt::Display for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RenderFrameCall => "on RenderFrame's call to PresentFrame",
            Self::PresentEntry => "on PresentFrame's entry, RenderFrame's call to it is patched by another plugin",
        })
    }
}

#[derive(Debug)]
pub enum Error {
    Hook(hook::Error),
    /// Neither the game's font nor `segoeui.ttf` or `consola.ttf` from the Windows font folder could be read.
    NoFont,
    AlreadyInstalled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hook(e) => e.fmt(f),
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
    /// The game's cursor as a texture of `ctx`, made at the first frame drawn with it.
    cursor: Option<egui::TextureHandle>,
    router: input::Router,
    /// What the guest drew last frame, which routes this frame's input.
    claim: Option<input::Claim>,
}

/// `RenderFrame` forwarded with all four argument registers: its untiered signature has four.
type RenderFrameFn = unsafe extern "C" fn(usize, usize, usize, usize);

static PRESENT: OnceLock<PresentFn> = OnceLock::new();
/// Set only when the overlay sits on `PresentFrame`'s entry.
static RENDER_FRAME: OnceLock<RenderFrameFn> = OnceLock::new();
thread_local! {
    static IN_RENDER_FRAME: Cell<bool> = const { Cell::new(false) };
}
static STATE: Mutex<Option<State>> = Mutex::new(None);
static WINDOWS: Mutex<Vec<Window>> = Mutex::new(Vec::new());
static OPEN: AtomicBool = AtomicBool::new(false);
static FAILURE: OnceLock<String> = OnceLock::new();
static FRAME: OnceLock<fn()> = OnceLock::new();
static HUD: OnceLock<HudFn> = OnceLock::new();
static GUEST: OnceLock<GuestFn> = OnceLock::new();
/// Textures of other contexts to free after the next frame is painted.
static FREED: Mutex<Vec<TextureId>> = Mutex::new(Vec::new());
static FONTS: OnceLock<Option<(egui::FontDefinitions, String)>> = OnceLock::new();
static CURSOR: OnceLock<Result<vanilla::Cursor, String>> = OnceLock::new();
static KEYBOARD_FOCUS: AtomicBool = AtomicBool::new(false);

/// Installs the overlay. Call it once from `SKSEPlugin_Load`, after `skyrim::init`, before the
/// first frame is drawn: the call site is patched in place.
pub fn install() -> Result<Placement, Error> {
    let ctx = themed_context(font_definitions().ok_or(Error::NoFont)?.clone(), &theme::Values::default());
    let reloc = skyrim::reloc();
    let render_frame = reloc.address(sre::id!(fns::RenderFrame))?;
    let present = reloc.address(sre::id!(fns::PresentFrame))?;
    let mut state = STATE.lock().unwrap();
    if state.is_some() {
        return Err(Error::AlreadyInstalled);
    }
    *state = Some(State { ctx, painter: None, start: Instant::now(), cursor: None, router: input::Router::default(), claim: None });
    drop(state);
    if let Some(site) = hook::find_call(render_frame, RENDER_FRAME_LEN, present) {
        let site = unsafe { hook::CallSite::create(site, present_detour as PresentFn) }?;
        let _ = PRESENT.set(site.original());
        unsafe { site.enable() }?;
        return Ok(Placement::RenderFrameCall);
    }
    let frame = unsafe { hook::Hook::create(render_frame, render_frame_detour as RenderFrameFn) }?;
    let entry = unsafe { hook::Hook::create(present, present_detour as PresentFn) }?;
    let _ = RENDER_FRAME.set(frame.original());
    let _ = PRESENT.set(entry.original());
    entry.enable()?;
    frame.enable()?;
    Ok(Placement::PresentEntry)
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

/// What another `egui` context drew for a frame: meshes in points whose texture ids are the
/// overlay's (`TextureId::User`, unique to the drawing context), and the changes to those textures.
#[derive(Default)]
pub struct Painted {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures: TexturesDelta,
}

/// The frame as the HUD layer sees it.
pub struct HudInput {
    pub view: View,
    pub time: f64,
    pub pixels_per_point: f32,
}

/// Draws the HUD layer: shapes under the windows, placed through the frame camera.
pub type HudFn = fn(&HudInput) -> Painted;

/// Runs `f` on the main thread at every present the overlay sees, open or not, once the scene
/// graph exists; what it returns is drawn into the frame under the overlay's windows and takes no
/// input. The first call sets it for the life of the game.
pub fn on_hud(f: HudFn) {
    let _ = HUD.set(f);
}

/// A frame's input for the guest: the events routed to it, the modifiers held, the time, the screen
/// in points and its scale. `focused` is false when a press this frame went to the overlay's own
/// widgets: the guest then gives up the keyboard focus and closes its popups, as a click elsewhere does.
pub struct GuestInput {
    pub events: Vec<Event>,
    pub modifiers: Modifiers,
    pub time: f64,
    pub screen: Rect,
    pub pixels_per_point: f32,
    pub focused: bool,
}

/// What the guest drew in `rect` (in points; its popups may leave it) and whether it wants the
/// pointer (over one of its areas, or a drag in progress) and the keyboard (a widget focused).
pub struct GuestOutput {
    pub rect: Rect,
    pub painted: Painted,
    pub wants_pointer: bool,
    pub wants_keyboard: bool,
}

/// Draws the guest after the windows, or returns `None` when the windows left no rect for one.
pub type GuestFn = fn(&GuestInput) -> Option<GuestOutput>;

/// Runs `f` on the main thread every frame the overlay is open, after its windows: UI another
/// context draws over them. Pointer events go to the guest inside its rect, while it wants the
/// pointer, over no window of the overlay, and until the buttons pressed there are released;
/// keyboard events while it wants the keyboard. The overlay's windows get none of those, and the
/// guest sees every pointer move. The first call sets it for the life of the game.
pub fn on_guest(f: GuestFn) {
    let _ = GUEST.set(f);
}

/// Frees textures the HUD layer or the guest uploaded, after the next frame is painted.
pub fn free_textures(ids: Vec<TextureId>) {
    FREED.lock().unwrap().extend(ids);
}

/// Which font the proportional text uses and where it came from, or why the game's font was not used.
#[must_use]
pub fn font_source() -> &'static str {
    fonts().map_or("no font", |(_, source)| source.as_str())
}

/// Where the cursor's shape came from, or why the game's was not read and the overlay draws its own arrow.
#[must_use]
pub fn cursor_source() -> String {
    match cursor() {
        Ok(c) => format!("the game's, from {}", c.source),
        Err(e) => format!("the overlay's arrow, the game's cursor was not read: {e}"),
    }
}

fn cursor() -> &'static Result<vanilla::Cursor, String> {
    CURSOR.get_or_init(|| game_data().ok_or_else(|| "no game folder".to_owned()).and_then(|d| vanilla::cursor(&d)))
}

/// `Data` next to the executable.
fn game_data() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("Data")))
}

/// Whether a widget, the overlay's or the guest's, held the keyboard focus at the end of the last frame drawn.
#[must_use]
pub fn keyboard_focus() -> bool {
    KEYBOARD_FOCUS.load(Ordering::Relaxed)
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

unsafe extern "C" fn render_frame_detour(a: usize, b: usize, c: usize, d: usize) {
    IN_RENDER_FRAME.set(true);
    unsafe { RENDER_FRAME.get().expect("stored before the hook was enabled")(a, b, c, d) };
    IN_RENDER_FRAME.set(false);
}

unsafe extern "C" fn present_detour() {
    if RENDER_FRAME.get().is_none() || IN_RENDER_FRAME.get() {
        before_present();
    }
    unsafe { PRESENT.get().expect("stored before the hook was enabled")() }
}

fn before_present() {
    input::ensure_sink();
    input::begin_frame();
    if let Some(frame) = FRAME.get() {
        frame();
    }
    if (is_open() || HUD.get().is_some()) && FAILURE.get().is_none()
        && let Some(fallback) = frame::back_buffer_view()
    {
        match unsafe { paint::Painter::frame_target(fallback) } {
            Ok(target) => draw(target.as_raw()),
            Err(error) => {
                let _ = FAILURE.set(format!("frame target: {error}"));
            }
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn draw(rtv: *mut core::ffi::c_void) {
    let mut state = STATE.lock().unwrap();
    let Some(state) = state.as_mut() else { return };
    if state.painter.is_none() {
        let reloc = skyrim::reloc();
        let (device, context) = match (reloc.address(sre::id!(globals::g_D3D11Device)), reloc.address(sre::id!(globals::g_D3D11Context))) {
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
    let Some(desc) = (unsafe { paint::Painter::target_desc(rtv) }) else {
        return;
    };
    let size = [desc.Width, desc.Height];
    let ppp = (size[1] as f32 / 1080.0).max(1.0);
    let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32) / ppp);
    let open = is_open();
    let time = state.start.elapsed().as_secs_f64();
    let (mut events, mouse, modifiers) = input::take(screen.size(), ppp);
    if !open {
        events.clear();
    }
    let host = &state.ctx;
    let routed = state.router.route(events, state.claim.filter(|_| open), |p| host.layer_id_at(p).is_some());
    if routed.guest_pressed {
        host.memory_mut(egui::Memory::stop_text_input);
    }
    let mut drawn = HUD.get().zip(View::current(screen)).map(|(hud, view)| hud(&HudInput { view, time, pixels_per_point: ppp })).unwrap_or_default();
    if state.cursor.is_none()
        && let Ok(c) = cursor()
    {
        state.cursor = Some(state.ctx.load_texture("game cursor", c.image.clone(), egui::TextureOptions::LINEAR));
    }
    let cursor = state.cursor.as_ref().zip(cursor().as_ref().ok()).map(|(texture, c)| (texture.id(), c.size));
    let mut raw = RawInput { screen_rect: Some(screen), time: Some(time), events: routed.host, focused: true, ..RawInput::default() };
    raw.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(ppp), focused: Some(true), ..ViewportInfo::default() });
    let mut windows = core::mem::take(&mut *WINDOWS.lock().unwrap());
    let output = state.ctx.run_pass(raw, |ctx| {
        if open {
            for window in &mut windows {
                window(ctx);
            }
        }
    });
    let mut added = WINDOWS.lock().unwrap();
    windows.append(&mut added);
    *added = windows;
    drop(added);
    let guest = GUEST.get().filter(|_| open).and_then(|f| f(&GuestInput { events: routed.guest, modifiers, time, screen, pixels_per_point: ppp, focused: !routed.host_pressed }));
    state.claim = guest.as_ref().map(|g| input::Claim { rect: g.rect, pointer: g.wants_pointer, keyboard: g.wants_keyboard });
    KEYBOARD_FOCUS.store(state.ctx.egui_wants_keyboard_input() || state.claim.is_some_and(|c| c.keyboard), Ordering::Relaxed);
    let mut textures = core::mem::take(&mut drawn.textures);
    textures.append(output.textures_delta);
    drawn.primitives.extend(state.ctx.tessellate(output.shapes, output.pixels_per_point));
    if let Some(guest) = guest {
        drawn.primitives.extend(guest.painted.primitives);
        textures.append(guest.painted.textures);
    }
    if open {
        drawn.primitives.extend(state.ctx.tessellate(vec![ClippedShape::new(screen, input::cursor_shape(mouse, cursor))], output.pixels_per_point));
    }
    for id in core::mem::take(&mut *FREED.lock().unwrap()) {
        textures.free(id);
    }
    if let Err(e) = unsafe { painter.paint(rtv, size, output.pixels_per_point, &drawn.primitives, &textures) } {
        let _ = FAILURE.set(format!("paint: {e}"));
    }
    textures.clear();
    frame::after_draw(rtv);
}

/// A context with `fonts` and the [`theme`] `values`: the overlay's own, and a framework plugin's
/// with the fonts and the values its loader handed over.
#[must_use]
pub fn themed_context(fonts: egui::FontDefinitions, values: &theme::Values) -> Context {
    let ctx = Context::default();
    ctx.set_fonts(fonts);
    theme::apply(&ctx, values);
    ctx
}

/// The fonts the overlay draws with ([`font_source`] says which), `None` when no font could be read.
#[must_use]
pub fn font_definitions() -> Option<&'static egui::FontDefinitions> {
    fonts().map(|(fonts, _)| fonts)
}

fn fonts() -> Option<&'static (egui::FontDefinitions, String)> {
    FONTS.get_or_init(load_fonts).as_ref()
}

/// The game's Futura Condensed Light for proportional text and Futura Condensed for headings, each
/// falling back on Segoe UI; Consolas for monospace, falling back on Segoe UI.
fn load_fonts() -> Option<(egui::FontDefinitions, String)> {
    let mut fonts = egui::FontDefinitions::empty();
    if let Some(dir) = std::env::var_os("WINDIR").map(|w| std::path::PathBuf::from(w).join("Fonts")) {
        for (name, file) in [("segoe", "segoeui.ttf"), ("consolas", "consola.ttf")] {
            if let Ok(bytes) = std::fs::read(dir.join(file)) {
                fonts.font_data.insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
            }
        }
    }
    let source = match game_data().ok_or_else(|| "no game folder".to_owned()).and_then(|d| vanilla::load(&d)) {
        Ok(v) => {
            fonts.font_data.insert(vanilla::BODY.to_owned(), egui::FontData::from_owned(v.body).into());
            fonts.font_data.insert(vanilla::HEADING.to_owned(), egui::FontData::from_owned(v.heading).into());
            format!("{} from {}", vanilla::BODY, v.source)
        }
        Err(e) => format!("Segoe UI, the game's font was not read: {e}"),
    };
    let order = |first: &str| -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for name in [first, "segoe", "consolas"] {
            if fonts.font_data.contains_key(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        names
    };
    let families = [
        (egui::FontFamily::Proportional, order(vanilla::BODY)),
        (egui::FontFamily::Name(theme::HEADING.into()), order(vanilla::HEADING)),
        (egui::FontFamily::Monospace, order("consolas")),
    ];
    if families[0].1.is_empty() {
        return None;
    }
    for (family, names) in families {
        fonts.families.insert(family, names);
    }
    Some((fonts, source))
}
