//! Tab sections and HUD painters by data (`docs/subsystems/abi.md`, "UI"): a plugin draws with
//! `egui` contexts of its own, in the loader's fonts and the framework theme, and hands the host
//! meshes, texture changes and what it wants of the input. The host never runs plugin code on its
//! own context, so nothing of a plugin's `egui` outlives its DLL.

use core::ffi::c_void;
use core::mem::offset_of;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use overlay::View;
use overlay::egui::epaint::{ImageDelta, Primitive, Vertex};
use overlay::egui::{
    self, AreaState, ClippedPrimitive, Context, CursorIcon, Event, FontData, FontDefinitions, FontFamily, ImageData, Key, LayerId, Modifiers, MouseWheelSource, MouseWheelUnit, Order, OutputCommand, PointerButton, Pos2, RawInput, Rect,
    TextureFilter, TextureId, TextureOptions, TouchPhase, ViewportId, ViewportInfo, pos2, vec2,
};

use crate::abi::{Api, Callback, Record, Sink, Str, call, read_array, read_sized, size, slice};

/// A tab section or a HUD painter: called once a frame it is drawn, with the frame's input and the
/// sinks its drawing goes through, during the call.
pub type UiCallback = Callback<extern "C" fn(*mut c_void, *const RawFrameInput, *const RawFrameOutput<'_>)>;

/// An input event, an element of [`RawFrameInput::events`]: `kind` 0 the pointer moved to `pos`, 1 `button` (0 primary, 1 secondary,
/// 2 middle, 3 and 4 the extra ones) `pressed` or released at `pos`, 2 the wheel turned by `delta`
/// lines, 3 the key named `text` (`egui`'s key names) `pressed` or released, 4 `text` typed, 5 the
/// pointer gone. Positions are in points; `modifiers` as in [`RawFrameInput`]. A side that does
/// not know a kind ignores the event.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawEvent {
    pub kind: u32,
    pub pos: [f32; 2],
    pub delta: [f32; 2],
    pub button: u32,
    pub pressed: bool,
    pub modifiers: u32,
    pub text: Str,
}

impl Record for RawEvent {
    fn base() -> Self {
        Self { kind: Self::GONE, pos: [0.0; 2], delta: [0.0; 2], button: 0, pressed: false, modifiers: 0, text: Str::default() }
    }
}

const BUTTONS: [PointerButton; 5] = [PointerButton::Primary, PointerButton::Secondary, PointerButton::Middle, PointerButton::Extra1, PointerButton::Extra2];

impl RawEvent {
    const MOVED: u32 = 0;
    const BUTTON: u32 = 1;
    const WHEEL: u32 = 2;
    const KEY: u32 = 3;
    const TEXT: u32 = 4;
    const GONE: u32 = 5;

    /// `event` as it crosses, its text borrowed; `None` for an event the overlay never feeds.
    pub(crate) fn new(event: &Event) -> Option<Self> {
        let gone = Self::base();
        Some(match event {
            Event::PointerMoved(p) => Self { kind: Self::MOVED, pos: [p.x, p.y], ..gone },
            Event::PointerButton { pos, button, pressed, modifiers } => {
                let button = u32::try_from(BUTTONS.iter().position(|b| b == button)?).ok()?;
                Self { kind: Self::BUTTON, pos: [pos.x, pos.y], button, pressed: *pressed, modifiers: raw_modifiers(*modifiers), ..gone }
            }
            Event::MouseWheel { delta, modifiers, .. } => Self { kind: Self::WHEEL, delta: [delta.x, delta.y], modifiers: raw_modifiers(*modifiers), ..gone },
            Event::Key { key, pressed, modifiers, .. } => Self { kind: Self::KEY, pressed: *pressed, modifiers: raw_modifiers(*modifiers), text: Str::from(key.name()), ..gone },
            Event::Text(text) => Self { kind: Self::TEXT, text: Str::from(text.as_str()), ..gone },
            Event::PointerGone => gone,
            _ => return None,
        })
    }

    unsafe fn to_event(self) -> Option<Event> {
        let modifiers = modifiers(self.modifiers);
        let pos = pos2(self.pos[0], self.pos[1]);
        Some(match self.kind {
            Self::MOVED => Event::PointerMoved(pos),
            Self::BUTTON => Event::PointerButton { pos, button: *BUTTONS.get(usize::try_from(self.button).ok()?)?, pressed: self.pressed, modifiers },
            Self::WHEEL => Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(self.delta[0], self.delta[1]), phase: TouchPhase::Move, source: MouseWheelSource::Wheel, modifiers },
            Self::KEY => {
                let key = Key::from_name(unsafe { self.text.as_str() })?;
                Event::Key { key, physical_key: Some(key), pressed: self.pressed, repeat: false, modifiers }
            }
            Self::TEXT => Event::Text(unsafe { self.text.as_str() }.to_owned()),
            Self::GONE => Event::PointerGone,
            _ => return None,
        })
    }
}

/// Modifier bits: 1 Alt, 2 Ctrl, 4 Shift, 8 the Mac's Cmd, 16 the platform's command key.
pub(crate) fn raw_modifiers(m: Modifiers) -> u32 {
    [m.alt, m.ctrl, m.shift, m.mac_cmd, m.command].into_iter().enumerate().fold(0, |bits, (i, on)| bits | u32::from(on) << i)
}

fn modifiers(bits: u32) -> Modifiers {
    let on = |i: u32| bits & 1 << i != 0;
    Modifiers { alt: on(0), ctrl: on(1), shift: on(2), mac_cmd: on(3), command: on(4) }
}

/// The frame camera for a HUD painter: null for a section.
#[repr(C)]
pub struct RawView {
    pub camera: *const sre::NiCamera,
    /// The screen in points: min x, min y, max x, max y.
    pub screen: [f32; 4],
}

/// One frame of a section or HUD painter: the area to fill (a section's rect, the screen for a
/// HUD painter) and the screen, in points; the routed events, `event_size` bytes apart; the
/// modifiers held; and `focused`, false when a press this frame went to the host's own widgets, so
/// the plugin gives up the keyboard focus and closes its popups as a click elsewhere does.
#[repr(C)]
pub struct RawFrameInput {
    pub size: u32,
    pub rect: [f32; 4],
    pub screen: [f32; 2],
    pub pixels_per_point: f32,
    pub time: f64,
    pub events: *const RawEvent,
    pub len: usize,
    pub event_size: u32,
    pub modifiers: u32,
    pub focused: bool,
    pub view: RawView,
}

impl Record for RawFrameInput {
    fn base() -> Self {
        Self {
            size: size::<Self>(),
            rect: [0.0; 4],
            screen: [0.0; 2],
            pixels_per_point: 1.0,
            time: 0.0,
            events: core::ptr::null(),
            len: 0,
            event_size: size::<RawEvent>(),
            modifiers: 0,
            focused: true,
            view: RawView { camera: core::ptr::null(), screen: [0.0; 4] },
        }
    }
}

/// `egui`'s premultiplied vertex, in points, as the overlay's renderer reads it: frozen, and
/// asserted against the `egui` each side is built with.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

const _: () = assert!(
    size_of::<RawVertex>() == size_of::<Vertex>() && offset_of!(RawVertex, pos) == offset_of!(Vertex, pos) && offset_of!(RawVertex, uv) == offset_of!(Vertex, uv) && offset_of!(RawVertex, color) == offset_of!(Vertex, color)
);

/// A triangle list clipped to `clip` (points) on the plugin's texture `texture`, borrowed for the call.
#[repr(C)]
pub struct RawMesh {
    pub size: u32,
    pub clip: [f32; 4],
    pub texture: u64,
    pub vertices: *const RawVertex,
    pub vertex_len: usize,
    pub indices: *const u32,
    pub index_len: usize,
}

impl Record for RawMesh {
    fn base() -> Self {
        Self { size: size::<Self>(), clip: [0.0; 4], texture: 0, vertices: core::ptr::null(), vertex_len: 0, indices: core::ptr::null(), index_len: 0 }
    }
}

impl RawMesh {
    /// The mesh on the host's texture `texture`.
    pub(crate) unsafe fn to_primitive(&self, texture: TextureId) -> ClippedPrimitive {
        let vertices = unsafe { slice(self.vertices.cast::<Vertex>(), self.vertex_len) }.to_vec();
        let indices = unsafe { slice(self.indices, self.index_len) }.to_vec();
        ClippedPrimitive { clip_rect: rect(self.clip), primitive: Primitive::Mesh(egui::Mesh { indices, vertices, texture_id: texture }) }
    }
}

/// A texture of the plugin's, whole or (`partial`) the region at `pos`: `extent` premultiplied
/// RGBA pixels, borrowed for the call; `linear` filtering, else nearest.
#[repr(C)]
pub struct RawTexture {
    pub size: u32,
    pub id: u64,
    pub partial: bool,
    pub pos: [u32; 2],
    pub extent: [u32; 2],
    pub rgba: *const u8,
    pub linear: bool,
}

impl Record for RawTexture {
    fn base() -> Self {
        Self { size: size::<Self>(), id: 0, partial: false, pos: [0; 2], extent: [0; 2], rgba: core::ptr::null(), linear: true }
    }
}

impl RawTexture {
    pub(crate) unsafe fn to_delta(&self) -> ImageDelta {
        let size = self.extent.map(|n| n as usize);
        let image = egui::ColorImage::from_rgba_premultiplied(size, unsafe { slice(self.rgba, size[0] * size[1] * 4) });
        let options = if self.linear { TextureOptions::LINEAR } else { TextureOptions::NEAREST };
        if self.partial { ImageDelta::partial(self.pos.map(|n| n as usize), image, options) } else { ImageDelta::full(image, options) }
    }
}

/// What the plugin wants after a frame: the cursor (an index of `egui::CursorIcon::ALL`), text it
/// copied (empty for none), and whether it wants the keyboard (a widget focused) and the pointer
/// (over one of its areas, or a drag in progress).
#[repr(C)]
pub struct RawPlatformOutput {
    pub size: u32,
    pub cursor: u32,
    pub copied: Str,
    pub wants_keyboard: bool,
    pub wants_pointer: bool,
}

impl Record for RawPlatformOutput {
    fn base() -> Self {
        Self { size: size::<Self>(), cursor: 0, copied: Str::default(), wants_keyboard: false, wants_pointer: false }
    }
}

/// Where a frame's drawing goes: the meshes in paint order, the textures set before they are
/// painted, the textures freed after, and the platform output.
#[repr(C)]
pub struct RawFrameOutput<'a> {
    pub size: u32,
    pub meshes: Sink<'a, *const RawMesh>,
    pub textures: Sink<'a, *const RawTexture>,
    pub freed: Sink<'a, u64>,
    pub platform: Sink<'a, *const RawPlatformOutput>,
}

impl Record for RawFrameOutput<'_> {
    fn base() -> Self {
        Self { size: size::<Self>(), meshes: Sink::ignore(), textures: Sink::ignore(), freed: Sink::ignore(), platform: Sink::ignore() }
    }
}

/// A font file the loader draws with, under the name the families use.
#[repr(C)]
pub struct RawFont {
    pub size: u32,
    pub name: Str,
    pub bytes: Str,
}

impl Record for RawFont {
    fn base() -> Self {
        Self { size: size::<Self>(), name: Str::default(), bytes: Str::default() }
    }
}

/// A font family: `kind` 0 proportional, 1 monospace, 2 the one `name` names; its fonts in fallback order.
#[repr(C)]
pub struct RawFamily {
    pub size: u32,
    pub kind: u32,
    pub name: Str,
    pub fonts: *const Str,
    pub len: usize,
}

impl Record for RawFamily {
    fn base() -> Self {
        Self { size: size::<Self>(), kind: 0, name: Str::default(), fonts: core::ptr::null(), len: 0 }
    }
}

impl RawFamily {
    pub(crate) fn new(family: &FontFamily, fonts: &[Str]) -> Self {
        let (kind, name) = match family {
            FontFamily::Proportional => (0, ""),
            FontFamily::Monospace => (1, ""),
            FontFamily::Name(name) => (2, &**name),
        };
        Self { size: size::<Self>(), kind, name: Str::from(name), fonts: fonts.as_ptr(), len: fonts.len() }
    }
}

pub(crate) fn corners(r: Rect) -> [f32; 4] {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

fn rect(c: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(c[0], c[1]), pos2(c[2], c[3]))
}

/// The fonts the loader hands over, `None` when it hands over none.
fn fonts(api: Api) -> Option<FontDefinitions> {
    let text = |s: Str| unsafe { s.as_str() }.to_owned();
    let mut files = Vec::new();
    let mut families = Vec::new();
    let mut font = |f: *const RawFont| files.extend(unsafe { read_sized(f) }.map(|f| (text(f.name), unsafe { f.bytes.bytes() }.to_vec())));
    let mut family = |f: *const RawFamily| {
        let Some(f) = (unsafe { read_sized(f) }) else { return };
        let family = match f.kind {
            0 => FontFamily::Proportional,
            1 => FontFamily::Monospace,
            2 => FontFamily::Name(text(f.name).into()),
            _ => return,
        };
        families.push((family, unsafe { slice(f.fonts, f.len) }.iter().map(|&n| text(n)).collect::<Vec<_>>()));
    };
    call!(api, fonts(Sink::new(&mut font), Sink::new(&mut family)))?;
    let mut fonts = FontDefinitions::empty();
    fonts.font_data = files.into_iter().map(|(name, bytes)| (name, Arc::new(FontData::from_owned(bytes)))).collect();
    fonts.families = families.into_iter().collect();
    fonts.families.contains_key(&FontFamily::Proportional).then_some(fonts)
}

/// One of a plugin's two contexts, alive while a callback holds it: the sections' (one runs a
/// frame) and the HUD painters' (each runs a pass a frame). `layer` keeps their texture ids apart.
struct Surface {
    context: Mutex<Weak<Context>>,
    layer: u64,
}

static SECTIONS: Surface = Surface { context: Mutex::new(Weak::new()), layer: 0 };
static HUDS: Surface = Surface { context: Mutex::new(Weak::new()), layer: 1 };

impl Surface {
    /// The context in the loader's fonts and the framework theme; `None` when the loader hands over no fonts.
    fn context(&self, api: Api) -> Option<Arc<Context>> {
        let mut slot = self.context.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(ctx) = slot.upgrade() {
            return Some(ctx);
        }
        let ctx = Arc::new(overlay::themed_context(fonts(api)?));
        *slot = Arc::downgrade(&ctx);
        Some(ctx)
    }

    fn texture(&self, id: TextureId) -> u64 {
        match id {
            TextureId::Managed(n) => n << 2 | self.layer,
            TextureId::User(n) => n << 2 | 2 | self.layer,
        }
    }

    /// Runs `ctx` over a frame's input and puts what it drew through `output`.
    fn run(&self, ctx: &Context, input: &RawFrameInput, output: &RawFrameOutput<'_>, ui: impl FnMut(&Context)) {
        if !input.focused {
            ctx.memory_mut(egui::Memory::stop_text_input);
            egui::Popup::close_all(ctx);
        }
        let mut events = vec![Event::ModifiersChanged(modifiers(input.modifiers))];
        events.extend(unsafe { read_array(input.events, input.len, input.event_size as usize) }.into_iter().filter_map(|e| unsafe { e.to_event() }));
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(input.screen[0], input.screen[1]));
        let mut raw = RawInput { screen_rect: Some(screen), time: Some(input.time), events, focused: true, ..RawInput::default() };
        raw.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(input.pixels_per_point), focused: Some(true), ..ViewportInfo::default() });
        let mut full = ctx.run_pass(raw, ui);
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                let ImageData::Color(image) = &delta.image;
                let [pos, extent] = [delta.pos.unwrap_or_default(), image.size].map(|p| p.map(|n| u32::try_from(n).unwrap_or(u32::MAX)));
                output.textures.put(&RawTexture { size: size::<RawTexture>(), id: self.texture(*id), partial: delta.pos.is_some(), pos, extent, rgba: image.pixels.as_ptr().cast(), linear: delta.options.magnification == TextureFilter::Linear });
            }
        }
        for p in ctx.tessellate(full.shapes, full.pixels_per_point) {
            if let Primitive::Mesh(mesh) = &p.primitive {
                let vertices = mesh.vertices.as_ptr().cast::<RawVertex>();
                output.meshes.put(&RawMesh { size: size::<RawMesh>(), clip: corners(p.clip_rect), texture: self.texture(mesh.texture_id), vertices, vertex_len: mesh.vertices.len(), indices: mesh.indices.as_ptr(), index_len: mesh.indices.len() });
            }
        }
        for id in &full.textures_delta.free {
            output.freed.put(self.texture(*id));
        }
        full.textures_delta.clear();
        let platform = &full.platform_output;
        let copied = platform.commands.iter().find_map(|c| if let OutputCommand::CopyText(text) = c { Some(text.as_str()) } else { None }).unwrap_or_default();
        let cursor = CursorIcon::ALL.iter().position(|c| *c == platform.cursor_icon).and_then(|i| u32::try_from(i).ok()).unwrap_or_default();
        output.platform.put(&RawPlatformOutput { size: size::<RawPlatformOutput>(), cursor, copied: Str::from(copied), wants_keyboard: ctx.egui_wants_keyboard_input(), wants_pointer: ctx.egui_is_using_pointer() || ctx.is_pointer_over_egui() });
    }
}

/// `f` boxed behind a [`UiCallback`]; the host calls it from one thread at a time.
fn callback<C: FnMut(&RawFrameInput, &RawFrameOutput<'_>) + Send + 'static>(f: C) -> UiCallback {
    extern "C" fn call<C: FnMut(&RawFrameInput, &RawFrameOutput<'_>)>(ctx: *mut c_void, input: *const RawFrameInput, output: *const RawFrameOutput<'_>) {
        if let (Some(input), Some(output)) = unsafe { (read_sized(input), read_sized(output)) } {
            (unsafe { &*ctx.cast::<Mutex<C>>() }.lock().unwrap_or_else(PoisonError::into_inner))(&input, &output);
        }
    }
    UiCallback::boxed(Mutex::new(f), call::<C>)
}

/// Adds the section `name` of the plugin's tab: `draw` fills the rect the host leaves for it, in a
/// scroll area of its own. False when the loader lacks the entries.
pub(crate) fn add_section(api: Api, name: &str, mut draw: impl FnMut(&mut egui::Ui) + Send + 'static) -> bool {
    let Some(ctx) = SECTIONS.context(api) else { return false };
    let id = egui::Id::unique(("section", name));
    let f = callback(move |input, output| {
        let rect = rect(input.rect);
        SECTIONS.run(&ctx, input, output, |ctx| {
            // A fixed area of a known size: no sizing pass, which would hide the first frame.
            if AreaState::load(ctx, id).is_none() {
                ctx.memory_mut(|m| m.areas_mut().set_state(LayerId::new(Order::Middle, id), AreaState { pivot_pos: Some(rect.min), size: Some(rect.size()), ..AreaState::default() }));
            }
            egui::Area::new(id).fixed_pos(rect.min).constrain(false).fade_in(false).show(ctx, |ui| {
                ui.set_max_size(rect.size());
                ui.set_min_size(rect.size());
                egui::ScrollArea::vertical().id_salt(id).auto_shrink(false).show(ui, &mut draw);
            });
        });
    });
    call!(api, add_section(Str::from(name), f)).is_some()
}

/// Adds a HUD painter: `draw` paints on the background layer of the whole screen. False when the
/// loader lacks the entries.
pub(crate) fn add_hud(api: Api, mut draw: impl FnMut(&egui::Painter, &View) + Send + 'static) -> bool {
    let Some(ctx) = HUDS.context(api) else { return false };
    let f = callback(move |input, output| {
        // The engine's camera, alive for the life of the game.
        let Some(camera) = (unsafe { input.view.camera.as_ref() }) else { return };
        let view = View::new(camera, rect(input.view.screen));
        HUDS.run(&ctx, input, output, |ctx| draw(&ctx.layer_painter(LayerId::background()), &view));
    });
    call!(api, add_hud(f)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_cross_and_come_back() {
        let shift = Modifiers { shift: true, ..Modifiers::NONE };
        let all = [
            Event::PointerMoved(pos2(3.0, 4.5)),
            Event::PointerButton { pos: pos2(1.0, 2.0), button: PointerButton::Secondary, pressed: true, modifiers: shift },
            Event::MouseWheel { unit: MouseWheelUnit::Line, delta: vec2(0.0, -1.0), phase: TouchPhase::Move, source: MouseWheelSource::Wheel, modifiers: Modifiers::CTRL },
            Event::Key { key: Key::Enter, physical_key: Some(Key::Enter), pressed: false, repeat: false, modifiers: Modifiers::ALT },
            Event::Text("é".to_owned()),
            Event::PointerGone,
        ];
        for event in all.clone() {
            assert_eq!(RawEvent::new(&event).and_then(|raw| unsafe { raw.to_event() }), Some(event));
        }
        assert!(RawEvent::new(&Event::Copy).is_none());
        let later = RawEvent { kind: 9, ..RawEvent::new(&Event::PointerGone).unwrap() };
        assert!(unsafe { later.to_event() }.is_none(), "a kind from a later side");
        let unknown = RawEvent { text: Str::from("NoSuchKey"), ..RawEvent::new(&all[3]).unwrap() };
        assert!(unsafe { unknown.to_event() }.is_none(), "a key name this side's egui does not know");
        let all_bits = Modifiers { alt: true, ctrl: true, shift: true, mac_cmd: true, command: true };
        assert_eq!((raw_modifiers(all_bits), modifiers(raw_modifiers(all_bits))), (31, all_bits));
    }

    fn events(input: &RawFrameInput) -> Vec<Option<Event>> {
        unsafe { read_array(input.events, input.len, input.event_size as usize) }.into_iter().map(|e| unsafe { e.to_event() }).collect()
    }

    #[test]
    fn a_frame_reads_across_record_and_event_sizes() {
        #[repr(C)]
        struct LaterEvent {
            event: RawEvent,
            added: [u64; 3],
        }
        #[repr(C)]
        struct LaterInput {
            input: RawFrameInput,
            added: f64,
        }
        let (moved, text) = (Event::PointerMoved(pos2(1.0, 2.0)), Event::Text("é".to_owned()));
        let later_events = [&moved, &text].map(|e| LaterEvent { event: RawEvent::new(e).unwrap(), added: [u64::MAX; 3] });
        let input = RawFrameInput { size: size::<LaterInput>(), time: 3.0, events: later_events.as_ptr().cast(), len: 2, event_size: size::<LaterEvent>(), ..RawFrameInput::base() };
        let later = LaterInput { input, added: 1.0 };
        let read = unsafe { read_sized((&raw const later).cast::<RawFrameInput>()) }.unwrap();
        assert_eq!((read.time, events(&read)), (3.0, vec![Some(moved.clone()), Some(text)]), "a newer host's larger frame and events");

        let older_events = [RawEvent::new(&Event::Text("dropped".to_owned())).unwrap(), RawEvent::new(&moved).unwrap()];
        let camera = core::ptr::dangling::<sre::NiCamera>();
        let input = RawFrameInput {
            size: u32::try_from(offset_of!(RawFrameInput, view)).unwrap(),
            events: older_events.as_ptr(),
            len: 2,
            event_size: size::<RawEvent>(),
            focused: false,
            view: RawView { camera, screen: [1.0; 4] },
            ..RawFrameInput::base()
        };
        let read = unsafe { read_sized(&raw const input) }.unwrap();
        assert!(read.view.camera.is_null() && !read.focused && events(&read).len() == 2, "the view an older host lacks reads as none");
        let without_text = unsafe { slice((&raw const older_events[0]).cast::<u8>(), offset_of!(RawEvent, text)) }.to_vec();
        let short = RawFrameInput { events: without_text.as_ptr().cast(), len: 1, event_size: u32::try_from(without_text.len()).unwrap(), ..read };
        assert_eq!(events(&short), [Some(Event::Text(String::new()))], "an older event without its text");
    }
}
