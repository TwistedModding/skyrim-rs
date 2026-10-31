//! The overlay's one window, in the look of the game's journal: a row of tabs across the top, Mods,
//! Plugins (the framework plugins with their version, file and state, buttons that reload, unload
//! and load each, the files the host did not load and a rescan of the folder), Legacy plugins (the SKSE plugins in `Data\SKSE\Plugins`) and Settings (every hotkey,
//! set by capturing the next key pressed, and the capture rule). Mods has a sidebar of every plugin
//! that added a tab ([`crate::Host::add_tab`], in load order), the selected one's sections under
//! it when it has two or more, and the selected section beside it. A plugin's sections and HUD
//! painters draw with the plugin's own `egui` contexts ([`crate::ui`]): the window leaves the
//! selected section's rect empty and the overlay draws the plugin's meshes over it as its guest,
//! with the plugin's textures under overlay ids of their own.

use core::cell::RefCell;
use std::sync::{Mutex, PoisonError};

use overlay::egui::{self, Align, Layout, RichText, Sense, TextStyle, TextWrapMode, WidgetText, vec2};
use overlay::theme;

use super::owned::Owned;
use super::reload::{self, Request, Status};
use super::{HOSTED_BY, NAME, keys, plugins, refused, skse_plugins};
use crate::abi::{Record, Sink, read_sized, size};
use crate::settings::{self, Setting, Value};
use crate::ui::{RawEvent, RawFrameInput, RawFrameOutput, RawMesh, RawPlatformOutput, RawTexture, RawView, UiCallback, corners, raw_modifiers};

/// The host setting holding the capture rule: `focus` (the default) or `all`.
const CAPTURE: &str = "overlay.capture";
/// The window's size until the player drags its corner, and the smallest it gets.
const DEFAULT_SIZE: egui::Vec2 = vec2(1120.0, 680.0);
const MIN_SIZE: egui::Vec2 = vec2(820.0, 500.0);
/// Space between the window's frame and its contents, and above and below the rules.
const MARGIN: egui::Margin = egui::Margin { left: 36, right: 36, top: 22, bottom: 18 };
const RULE_SPACE: f32 = 12.0;

/// One section of a plugin's tab.
struct Section {
    owner: &'static str,
    name: String,
    draw: UiCallback,
}

static SECTIONS: Owned<Section> = Owned::new();

/// A plugin's HUD painter.
struct Hud {
    owner: &'static str,
    draw: UiCallback,
}

static HUDS: Owned<Hud> = Owned::new();
/// The rect this frame's window left for the selected section, and whose section it is.
static RESERVED: Mutex<Option<(String, String, egui::Rect)>> = Mutex::new(None);
static TEXTURES: Mutex<Textures> = Mutex::new(Textures { ids: Vec::new(), next: 0 });

/// The plugins' textures under overlay ids of their own (`TextureId::User`, never reused).
struct Textures {
    ids: Vec<(&'static str, u64, egui::TextureId)>,
    next: u64,
}

impl Textures {
    /// The overlay's id for the plugin `owner`'s texture `id`, made at its first use.
    fn id(&mut self, owner: &'static str, id: u64) -> egui::TextureId {
        if let Some((_, _, texture)) = self.ids.iter().find(|(o, i, _)| *o == owner && *i == id) {
            return *texture;
        }
        let texture = egui::TextureId::User(self.next);
        self.next += 1;
        self.ids.push((owner, id, texture));
        texture
    }

    fn free(&mut self, owner: &str, id: u64) -> Option<egui::TextureId> {
        let index = self.ids.iter().position(|(o, i, _)| *o == owner && *i == id)?;
        Some(self.ids.swap_remove(index).2)
    }

    fn remove_owner(&mut self, owner: &str) -> Vec<egui::TextureId> {
        let (gone, kept): (Vec<_>, Vec<_>) = core::mem::take(&mut self.ids).into_iter().partition(|(o, _, _)| *o == owner);
        self.ids = kept;
        gone.into_iter().map(|(_, _, texture)| texture).collect()
    }
}

/// Runs a plugin's section or HUD painter over `input` and gathers what it drew, with its texture
/// ids made the overlay's, and its platform output.
fn paint(owner: &'static str, draw: &UiCallback, input: &RawFrameInput) -> (overlay::Painted, Option<RawPlatformOutput>) {
    let drawn = RefCell::new(overlay::Painted::default());
    let mut textures = TEXTURES.lock().unwrap_or_else(PoisonError::into_inner);
    let textures = RefCell::new(&mut *textures);
    let mut platform = None;
    let mut mesh = |m: *const RawMesh| {
        let Some(m) = (unsafe { read_sized(m) }) else { return };
        let texture = textures.borrow_mut().id(owner, m.texture);
        drawn.borrow_mut().primitives.push(unsafe { m.to_primitive(texture) });
    };
    let mut set = |t: *const RawTexture| {
        let Some(t) = (unsafe { read_sized(t) }) else { return };
        let texture = textures.borrow_mut().id(owner, t.id);
        drawn.borrow_mut().textures.push(texture, unsafe { t.to_delta() });
    };
    let mut free = |id: u64| {
        if let Some(texture) = textures.borrow_mut().free(owner, id) {
            drawn.borrow_mut().textures.free(texture);
        }
    };
    let mut keep = |p: *const RawPlatformOutput| platform = unsafe { read_sized(p) };
    let output = RawFrameOutput { size: size::<RawFrameOutput<'_>>(), meshes: Sink::new(&mut mesh), textures: Sink::new(&mut set), freed: Sink::new(&mut free), platform: Sink::new(&mut keep) };
    (draw.call)(draw.ctx, input, &raw const output);
    (drawn.into_inner(), platform)
}
/// Each plugin's tab title.
static TITLES: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mods,
    Plugins,
    Legacy,
    Settings,
}

const TABS: [(Tab, &str); 4] = [(Tab::Mods, "Mods"), (Tab::Plugins, "Plugins"), (Tab::Legacy, "Legacy plugins"), (Tab::Settings, "Settings")];

/// The selected tab, the selected mod (the first while `None`), each mod's selected section, by
/// name, so a reloaded generation finds its own again, and the window's size.
struct Shown {
    tab: Tab,
    plugin: Option<String>,
    sections: Vec<(String, String)>,
    size: egui::Vec2,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown { tab: Tab::Mods, plugin: None, sections: Vec::new(), size: DEFAULT_SIZE });

/// A plugin's tab as the sidebar lists it.
struct Mod {
    owner: String,
    title: String,
    sections: Vec<String>,
}

pub(super) fn set_title(owner: &'static str, title: &str) {
    let mut titles = TITLES.lock().unwrap_or_else(PoisonError::into_inner);
    match titles.iter_mut().find(|(o, _)| *o == owner) {
        Some(entry) => title.clone_into(&mut entry.1),
        None => titles.push((owner, title.to_owned())),
    }
}

pub(super) fn add_section(owner: &'static str, name: &str, draw: UiCallback) {
    SECTIONS.add(owner, Section { owner, name: name.to_owned(), draw });
}

pub(super) fn add_hud(owner: &'static str, draw: UiCallback) {
    HUDS.add(owner, Hud { owner, draw });
}

/// Puts the window, the selected section and every plugin's HUD painters on the overlay.
pub(super) fn install() {
    overlay::add(draw);
    overlay::on_guest(draw_section);
    overlay::on_hud(draw_hud);
}

/// The overlay's HUD callback: every plugin's painter in registration order, on the whole screen.
fn draw_hud(input: &overlay::HudInput) -> overlay::Painted {
    let screen = input.view.screen();
    let view = RawView { camera: input.view.camera(), screen: corners(screen) };
    let raw = RawFrameInput { rect: view.screen, screen: [screen.width(), screen.height()], pixels_per_point: input.pixels_per_point, time: input.time, view, ..RawFrameInput::base() };
    let mut all = overlay::Painted::default();
    HUDS.each_mut(|hud| {
        let (drawn, _) = paint(hud.owner, &hud.draw, &raw);
        all.primitives.extend(drawn.primitives);
        all.textures.append(drawn.textures);
    });
    all
}

/// The overlay's guest: the selected section in the rect the window left for it this frame.
fn draw_section(input: &overlay::GuestInput) -> Option<overlay::GuestOutput> {
    let (owner, name, rect) = RESERVED.lock().unwrap_or_else(PoisonError::into_inner).take()?;
    let events: Vec<RawEvent> = input.events.iter().filter_map(RawEvent::new).collect();
    let raw = RawFrameInput {
        rect: corners(rect),
        screen: [input.screen.width(), input.screen.height()],
        pixels_per_point: input.pixels_per_point,
        time: input.time,
        events: events.as_ptr(),
        len: events.len(),
        modifiers: raw_modifiers(input.modifiers),
        focused: input.focused,
        view: RawView { camera: core::ptr::null(), screen: corners(input.screen) },
        ..RawFrameInput::base()
    };
    let mut drawn = None;
    SECTIONS.each_mut(|s| {
        if drawn.is_none() && s.owner == owner && s.name == name {
            drawn = Some(paint(s.owner, &s.draw, &raw));
        }
    });
    let (painted, platform) = drawn?;
    let (wants_pointer, wants_keyboard) = platform.map_or((false, false), |p| (p.wants_pointer, p.wants_keyboard));
    Some(overlay::GuestOutput { rect, painted, wants_pointer, wants_keyboard })
}

pub(super) fn show_tab(owner: &'static str) {
    select(Tab::Mods, Some(owner.to_owned()), None);
    overlay::set_open(true);
}

pub(super) fn show_section(owner: &'static str, section: &str) {
    select(Tab::Mods, Some(owner.to_owned()), Some(section.to_owned()));
    overlay::set_open(true);
}

/// Drops a plugin's tab and HUD painters, frees its textures after the next frame, and returns
/// how many sections and painters it had.
pub(super) fn remove(owner: &str) -> (usize, usize) {
    TITLES.lock().unwrap_or_else(PoisonError::into_inner).retain(|(o, _)| *o != owner);
    overlay::free_textures(TEXTURES.lock().unwrap_or_else(PoisonError::into_inner).remove_owner(owner));
    (SECTIONS.remove_owner(owner), HUDS.remove_owner(owner))
}

fn title_of(owner: &str) -> String {
    TITLES.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(o, _)| *o == owner).map_or_else(|| owner.to_owned(), |(_, t)| t.clone())
}

fn mods() -> Vec<Mod> {
    let mut sections: Vec<(&'static str, String)> = Vec::new();
    SECTIONS.each(|s| {
        sections.push((s.owner, s.name.clone()));
        true
    });
    plugins()
        .iter()
        .filter_map(|p| {
            let names: Vec<String> = sections.iter().filter(|(o, _)| *o == p.name).map(|(_, n)| n.clone()).collect();
            (!names.is_empty()).then(|| Mod { owner: p.name.clone(), title: title_of(&p.name), sections: names })
        })
        .collect()
}

/// The selected tab, and the selected mod's index and its selected section's (none while no mod has a tab).
fn selection(mods: &[Mod]) -> (Tab, Option<(usize, usize)>) {
    let shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let index = shown.plugin.as_ref().and_then(|p| mods.iter().position(|m| m.owner == *p)).unwrap_or(0);
    let selected = mods.get(index).map(|m| (index, shown.sections.iter().find(|(o, _)| *o == m.owner).and_then(|(_, name)| m.sections.iter().position(|n| n == name)).unwrap_or(0)));
    (shown.tab, selected)
}

/// Selects `tab` and, when given, the mod `plugin` and its `section` (the one selected before when `None`).
fn select(tab: Tab, plugin: Option<String>, section: Option<String>) {
    let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    if let (Some(owner), Some(section)) = (&plugin, section) {
        shown.sections.retain(|(o, _)| o != owner);
        shown.sections.push((owner.clone(), section));
    }
    if plugin.is_some() {
        shown.plugin = plugin;
    }
    shown.tab = tab;
}

/// The pages `SkyrimRs gui` numbers: each mod's sections (Mods itself while no mod has a tab), then
/// the other tabs; each with whether it is shown, and what selects it.
#[allow(clippy::type_complexity)]
fn page_list() -> Vec<(String, bool, (Tab, Option<String>, Option<String>))> {
    let mods = mods();
    let (tab, selected) = selection(&mods);
    let mut pages = Vec::new();
    if mods.is_empty() {
        pages.push(("Mods".to_owned(), tab == Tab::Mods, (Tab::Mods, None, None)));
    }
    for (i, m) in mods.iter().enumerate() {
        for (j, s) in m.sections.iter().enumerate() {
            pages.push((format!("{} / {s}", m.title), tab == Tab::Mods && selected == Some((i, j)), (Tab::Mods, Some(m.owner.clone()), Some(s.clone()))));
        }
    }
    pages.extend(TABS[1..].iter().map(|(t, title)| ((*title).to_owned(), tab == *t, (*t, None, None))));
    pages
}

/// Every page as `SkyrimRs gui` numbers them, and whether it is shown.
pub(super) fn pages() -> Vec<(String, bool)> {
    page_list().into_iter().map(|(title, shown, _)| (title, shown)).collect()
}

/// Selects page `n` of [`pages`]; false when there is none.
pub(super) fn show_page(n: usize) -> bool {
    match page_list().into_iter().nth(n) {
        Some((_, _, (tab, plugin, section))) => {
            select(tab, plugin, section);
            true
        }
        None => false,
    }
}

pub(super) fn select_settings() {
    select(Tab::Settings, None, None);
}

fn focus_capture() -> bool {
    settings::get(NAME, CAPTURE).and_then(|s| String::from_setting(&s)).is_none_or(|v| v != "all")
}

/// Sets the overlay's capture rule from the host's settings, where it is written when missing.
pub(super) fn apply_capture() {
    let focus = focus_capture();
    if settings::get(NAME, CAPTURE).is_none() {
        settings::set(NAME, CAPTURE, Setting::Text("focus".to_owned()));
    }
    overlay::set_capture(if focus { overlay::Capture::Focus } else { overlay::Capture::All });
}

pub(super) fn hosted_by() -> &'static str {
    HOSTED_BY.get().copied().unwrap_or("an unknown loader")
}

/// A packed plugin version (`major << 24 | minor << 16 | build << 4 | sub`) as `1.28.0`, with the sub when it is not 0.
pub(super) fn version_text(packed: u32) -> String {
    let [major, minor, build, sub] = skse::runtime::unpack(packed);
    if sub == 0 { format!("{major}.{minor}.{build}") } else { format!("{major}.{minor}.{build}.{sub}") }
}

/// A framework plugin, or a file the host did not load, as the Plugins tab lists it.
pub(super) struct PluginRow {
    pub name: String,
    pub version: String,
    pub file: String,
    pub status: String,
    /// Why it is not running, empty while it is.
    pub reason: String,
    /// The plugin's state; `None` for a file the host did not load.
    pub state: Option<Status>,
}

pub(super) fn plugin_rows() -> Vec<PluginRow> {
    let generations = reload::generations();
    let mut rows: Vec<PluginRow> = plugins()
        .iter()
        .map(|p| {
            let (generation, state) = generations.iter().find(|g| g.0 == p.name).map_or((1, Status::Running), |g| (g.1, g.2));
            let (status, reason) = match (state, generation) {
                (Status::Stopped, _) => ("Stopped".to_owned(), "its load returned false".to_owned()),
                (Status::Unloaded, _) => ("Unloaded".to_owned(), "unloaded from this menu or the console; Load starts it again from its file".to_owned()),
                (Status::Running, 1) => ("Running".to_owned(), String::new()),
                (Status::Running, 2) => ("Running, reloaded once".to_owned(), String::new()),
                (Status::Running, n) => (format!("Running, reloaded {} times", n - 1), String::new()),
            };
            PluginRow { name: p.name.clone(), version: p.version.clone(), file: p.file.clone(), status, reason, state: Some(state) }
        })
        .collect();
    rows.extend(refused().into_iter().map(|(file, reason)| PluginRow { name: String::new(), version: String::new(), file, status: "Not loaded".to_owned(), reason, state: None }));
    rows
}

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::DIM)
}

fn warn(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text).color(ui.visuals().warn_fg_color)
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).text_style(TextStyle::Heading).size(22.0));
    ui.separator();
    ui.add_space(4.0);
}

/// The row of tab names, centred, the selected one white between the journal's arrows. Returns the one clicked.
#[allow(clippy::cast_precision_loss)]
fn tab_row(ui: &mut egui::Ui, selected: Tab) -> Option<Tab> {
    let galleys: Vec<_> = TABS.iter().map(|(_, n)| WidgetText::from(n.to_uppercase()).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, TextStyle::Heading)).collect();
    let (pad, gap) = (vec2(26.0, 4.0), 30.0);
    let total = galleys.iter().map(|g| g.size().x + 2.0 * pad.x).sum::<f32>() + gap * (TABS.len() - 1) as f32;
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for ((tab, _), galley) in TABS.iter().zip(galleys) {
            let (rect, response) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::click());
            let on = *tab == selected;
            let color = if on || response.hovered() { theme::TEXT } else { theme::DIM };
            let painter = ui.painter();
            if on {
                let (y, stroke) = (rect.center().y, egui::Stroke::new(1.5, color));
                let (l, r) = (rect.left() + 4.0, rect.right() - 4.0);
                painter.add(egui::Shape::convex_polygon(vec![egui::pos2(l, y), egui::pos2(l + 10.0, y - 7.0), egui::pos2(l + 10.0, y + 7.0)], egui::Color32::TRANSPARENT, stroke));
                painter.add(egui::Shape::convex_polygon(vec![egui::pos2(r, y), egui::pos2(r - 10.0, y + 7.0), egui::pos2(r - 10.0, y - 7.0)], egui::Color32::TRANSPARENT, stroke));
            }
            painter.galley(rect.min + pad, galley, color);
            if response.clicked() {
                clicked = Some(*tab);
            }
        }
    });
    clicked
}

/// One line of the sidebar across its width, white when selected, on the highlight when `marked`. Returns whether it was clicked.
fn sidebar_row(ui: &mut egui::Ui, text: &str, selected: bool, marked: bool, indent: f32) -> bool {
    let pad = vec2(12.0 + indent, 7.0);
    let galley = WidgetText::from(text).into_galley(ui, Some(TextWrapMode::Truncate), ui.available_width() - pad.x - 10.0, TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), galley.size().y + 2.0 * pad.y), Sense::click());
    if marked {
        ui.painter().rect_filled(rect, 0.0, theme::HIGHLIGHT);
    }
    ui.painter().galley(rect.min + pad, galley, if selected || response.hovered() { theme::TEXT } else { theme::DIM });
    response.clicked()
}

/// The Mods page: the sidebar of mods, the selected one's sections under it, and its selected section.
fn mods_tab(ui: &mut egui::Ui, mods: &[Mod], selected: Option<(usize, usize)>) {
    let Some((index, section)) = selected else {
        ui.label(dim("No mod has added a tab. A framework plugin adds one with Host::add_tab."));
        return;
    };
    egui::Panel::left("mods").resizable(false).exact_size(260.0).frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 16, ..egui::Margin::ZERO })).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("mod list").auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, m) in mods.iter().enumerate() {
                if sidebar_row(ui, &m.title, i == index, i == index && m.sections.len() < 2, 0.0) {
                    select(Tab::Mods, Some(m.owner.clone()), None);
                }
                if i == index && m.sections.len() > 1 {
                    for (j, name) in m.sections.iter().enumerate() {
                        if sidebar_row(ui, name, j == section, j == section, 18.0) {
                            select(Tab::Mods, Some(m.owner.clone()), Some(name.clone()));
                        }
                    }
                }
            }
        });
    });
    let (owner, name) = (&mods[index].owner, &mods[index].sections[section]);
    egui::CentralPanel::no_frame().frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 28, right: 8, top: 4, bottom: 0 })).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        *RESERVED.lock().unwrap_or_else(PoisonError::into_inner) = Some((owner.clone(), name.clone(), rect));
    });
}

/// A striped table under a dim header row; each cell with its hover text, empty for none.
fn table(ui: &mut egui::Ui, id: &str, header: &[&str], rows: Vec<Vec<(RichText, String)>>) {
    egui::Grid::new(id).striped(true).spacing([36.0, 10.0]).show(ui, |ui| {
        for h in header {
            ui.label(dim(*h).small());
        }
        ui.end_row();
        for row in rows {
            for (text, hover) in row {
                let response = ui.label(text);
                if !hover.is_empty() {
                    response.on_hover_text(hover);
                }
            }
            ui.end_row();
        }
    });
}

/// The buttons of a plugin's row, each with its hover text and what it asks for.
fn plugin_buttons(name: &str, state: Status) -> Vec<(&'static str, &'static str, Request)> {
    match state {
        Status::Unloaded => vec![("Load", "Load the plugin from its file again", Request::Load(name.to_owned()))],
        Status::Running | Status::Stopped => vec![
            ("Reload", "Unload the plugin and load its file again, handing over its state", Request::Reload(name.to_owned())),
            ("Unload", "Unload the plugin until Load is clicked; it keeps its state for then", Request::Unload(name.to_owned())),
        ],
    }
}

fn plugins_tab(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(dim(format!("skyrim-rs {}, loaded by {}. Framework plugins are read from Data\\SKSE\\Plugins\\skyrim-rs.", env!("CARGO_PKG_VERSION"), hosted_by())));
        if ui.button("Rescan folder").on_hover_text("Load the plugins added to the folder since the game started, and try the ones not loaded again").clicked() {
            reload::request(Request::Rescan);
        }
    });
    ui.add_space(RULE_SPACE);
    let rows = plugin_rows();
    if rows.is_empty() {
        ui.label(dim("No framework plugin is installed."));
        return;
    }
    egui::Grid::new("plugins").striped(true).spacing([36.0, 10.0]).show(ui, |ui| {
        for h in ["Name", "Version", "File", "Status", ""] {
            ui.label(dim(h).small());
        }
        ui.end_row();
        for r in rows {
            ui.label(RichText::new(&r.name));
            ui.label(dim(r.version));
            ui.label(dim(r.file));
            let status = ui.label(if r.reason.is_empty() { RichText::new(r.status) } else { warn(ui, r.status) });
            if !r.reason.is_empty() {
                status.on_hover_text(r.reason);
            }
            ui.horizontal(|ui| {
                for (label, hover, request) in r.state.map(|s| plugin_buttons(&r.name, s)).unwrap_or_default() {
                    if ui.button(label).on_hover_text(hover).clicked() {
                        reload::request(request);
                    }
                }
            });
            ui.end_row();
        }
    });
}

fn legacy_tab(ui: &mut egui::Ui) {
    let log = if hosted_by() == "the script extender" { "skse64.log" } else { "skyrim-rs-standalone.log" };
    ui.label(dim(format!("SKSE plugins in Data\\SKSE\\Plugins, loaded by {}. The reason a plugin was not loaded is in {log}.", hosted_by())));
    ui.add_space(RULE_SPACE);
    let rows: Vec<_> = skse_plugins()
        .into_iter()
        .map(|p| {
            let status = if p.loaded { RichText::new("Loaded") } else { warn(ui, "Not loaded") };
            let (name, version, author) = p.data.map_or_else(|| (String::new(), String::new(), String::new()), |d| (d.name, version_text(d.version), d.author));
            vec![(RichText::new(name), String::new()), (dim(version), String::new()), (dim(author), String::new()), (dim(p.file), String::new()), (status, String::new())]
        })
        .collect();
    if rows.is_empty() {
        ui.label(dim("The plugins are read once the loader has loaded them all."));
    } else {
        table(ui, "legacy", &["Name", "Version", "Author", "File", "Status"], rows);
    }
}

/// What a binding does, as the Settings tab names it.
fn binding_label(index: usize, b: &keys::Binding) -> String {
    match index {
        keys::TOGGLE => "Open and close this menu".to_owned(),
        keys::CLOSE => "Close this menu".to_owned(),
        _ => format!("{}: {}", title_of(&b.owner), b.action()),
    }
}

fn settings_tab(ui: &mut egui::Ui) {
    heading(ui, "Keys");
    let capturing = keys::capturing();
    egui::Grid::new("keys").striped(true).spacing([40.0, 10.0]).min_col_width(90.0).show(ui, |ui| {
        for (i, b) in keys::list().iter().enumerate() {
            ui.label(binding_label(i, b)).on_hover_text(format!("Default: {}", keys::show(b.default)));
            let key = ui.label(if b.key.is_some() { RichText::new(keys::show(b.key)) } else { warn(ui, "Unbound") });
            if !b.game.is_empty() {
                key.on_hover_text(format!("The game uses this key too: {}", b.game));
            }
            let armed = capturing == Some(i);
            if ui.button(if armed { "Press a key" } else { "Set" }).clicked() {
                keys::capture((!armed).then_some(i));
            }
            ui.end_row();
        }
    });
    let message = keys::last_message();
    if !message.is_empty() {
        ui.add_space(4.0);
        ui.label(if message.contains("refused") || message.contains("unbound") { warn(ui, message) } else { dim(message) });
    }
    ui.add_space(2.0 * RULE_SPACE);
    heading(ui, "Input");
    let mut focus = focus_capture();
    if ui.checkbox(&mut focus, "Keys reach the game while no field in this menu has the focus").changed() {
        settings::set(NAME, CAPTURE, Setting::Text(if focus { "focus" } else { "all" }.to_owned()));
        apply_capture();
    }
}

/// The journal's key hint at the bottom right: the close key and "Exit", which closes the overlay when clicked.
fn footer(ui: &mut egui::Ui) {
    ui.add_space(RULE_SPACE);
    ui.separator();
    ui.add_space(4.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add(egui::Label::new(dim("Exit")).sense(Sense::click()).selectable(false)).clicked() {
            overlay::set_open(false);
        }
        if let Some(key) = keys::key(keys::CLOSE).or_else(|| keys::key(keys::TOGGLE)) {
            egui::Frame::new().stroke(egui::Stroke::new(1.0, theme::DIM)).inner_margin(egui::Margin::symmetric(5, 0)).show(ui, |ui| ui.label(dim(key.to_string()).small()));
        }
    });
}

/// The grip at the window's bottom right corner: dragging it resizes the window, down to [`MIN_SIZE`] and up to the screen.
fn resize_grip(ctx: &egui::Context, window: egui::Rect) {
    let size = vec2(18.0, 18.0);
    egui::Area::new(egui::Id::unique((NAME, "resize"))).order(egui::Order::Foreground).fixed_pos(window.max - size).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
        let color = if response.hovered() || response.dragged() { theme::TEXT } else { theme::DIM };
        for inset in [4.0, 9.0, 14.0] {
            ui.painter().line_segment([egui::pos2(rect.right() - 3.0, rect.top() + inset), egui::pos2(rect.left() + inset, rect.bottom() - 3.0)], egui::Stroke::new(1.0, color));
        }
        if response.dragged() {
            let max = ctx.content_rect().size() - vec2(16.0, 16.0);
            let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
            shown.size = (shown.size + response.drag_delta()).clamp(MIN_SIZE, max.max(MIN_SIZE));
        }
    });
}

/// Draws the window; the overlay's one window callback. The window's size is the host's (the grip
/// sets it), so its contents fill it and never grow it.
fn draw(ctx: &egui::Context) {
    let mods = mods();
    let (tab, selected) = selection(&mods);
    let size = SHOWN.lock().unwrap_or_else(PoisonError::into_inner).size;
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(MARGIN);
    let window = egui::Window::new(NAME).id(egui::Id::unique((NAME, "gui"))).title_bar(false).frame(frame).default_pos(ctx.content_rect().center() - DEFAULT_SIZE / 2.0).fixed_size(size);
    let shown = window.show(ctx, |ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 9.0);
        egui::Panel::top("tabs").frame(egui::Frame::NONE).show_separator_line(false).show(ui, |ui| {
            if let Some(clicked) = tab_row(ui, tab) {
                select(clicked, None, None);
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(RULE_SPACE);
        });
        egui::Panel::bottom("footer").frame(egui::Frame::NONE).show_separator_line(false).show(ui, footer);
        egui::CentralPanel::no_frame().show(ui, |ui| match tab {
            Tab::Mods => mods_tab(ui, &mods, selected),
            _ => {
                egui::ScrollArea::vertical().id_salt(TABS.iter().position(|(t, _)| *t == tab)).auto_shrink(false).show(ui, |ui| match tab {
                    Tab::Plugins => plugins_tab(ui),
                    Tab::Legacy => legacy_tab(ui),
                    _ => settings_tab(ui),
                });
            }
        });
    });
    if let Some(shown) = shown {
        resize_grip(ctx, shown.response.rect);
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::Primitive;

    use super::*;
    use crate::abi::Api;
    use crate::ui::{self, RawFrameInput};

    /// A frame of the section in `rect` on a 1920 by 1080 screen, the events crossing as the host
    /// sends them: the primitives, the textures uploaded whole, and the platform output.
    fn frame(owner: &str, rect: egui::Rect, events: &[egui::Event], focused: bool, time: f64) -> (Vec<egui::ClippedPrimitive>, Vec<egui::TextureId>, Option<RawPlatformOutput>) {
        let events: Vec<RawEvent> = events.iter().filter_map(RawEvent::new).collect();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1920.0, 1080.0));
        let raw = RawFrameInput { rect: corners(rect), screen: [1920.0, 1080.0], time, events: events.as_ptr(), len: events.len(), focused, view: RawView { camera: core::ptr::null(), screen: corners(screen) }, ..RawFrameInput::base() };
        let mut drawn = None;
        SECTIONS.each_mut(|s| {
            if s.owner == owner {
                drawn = Some(paint(s.owner, &s.draw, &raw));
            }
        });
        let (mut painted, platform) = drawn.expect("the section was added");
        let whole = painted.textures.set.iter().filter(|(_, deltas)| deltas.iter().any(|d| d.pos.is_none())).map(|(id, _)| *id).collect();
        painted.textures.clear();
        (painted.primitives, whole, platform)
    }

    #[test]
    fn a_section_draws_through_the_c_path_with_its_own_textures() {
        let rect = egui::Rect::from_min_size(egui::pos2(400.0, 200.0), vec2(600.0, 400.0));
        for owner in ["gui-test-a", "gui-test-b"] {
            let mut text = String::new();
            assert!(ui::add_section(Api(super::super::api::build(owner, core::ptr::null())), "Main", move |ui| {
                ui.label("Damage scale");
                ui.text_edit_singleline(&mut text);
            }));
        }
        let (a, font, platform) = frame("gui-test-a", rect, &[], true, 0.0);
        let meshes: Vec<_> = a.iter().filter_map(|p| if let Primitive::Mesh(m) = &p.primitive { Some(m) } else { None }).collect();
        assert!(!meshes.is_empty() && meshes.iter().all(|m| !m.vertices.is_empty()), "the label and the field are meshes");
        assert_eq!(font.len(), 1, "the font atlas is uploaded whole in the first frame");
        assert!(meshes.iter().any(|m| m.texture_id == font[0]));
        assert!(meshes.iter().all(|m| m.vertices.iter().all(|v| rect.expand(1.0).contains(v.pos))), "the section stays in its rect");
        assert!(platform.is_some_and(|p| !p.wants_keyboard && !p.wants_pointer));

        let (_, b, _) = frame("gui-test-b", rect, &[], true, 0.0);
        assert!(matches!(font[0], egui::TextureId::User(_)));
        assert_ne!(b, font, "each plugin's atlas has an id of its own");
        let (_, again, _) = frame("gui-test-a", rect, &[], true, 0.1);
        assert!(again.is_empty(), "the atlas is not sent again");

        let field = egui::pos2(420.0, 245.0);
        let click = |pressed| egui::Event::PointerButton { pos: field, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (_, _, hovered) = frame("gui-test-a", rect, &[egui::Event::PointerMoved(field)], true, 0.2);
        assert!(hovered.is_some_and(|p| p.wants_pointer), "the pointer over the section");
        let (_, _, clicked) = frame("gui-test-a", rect, &[click(true), click(false)], true, 0.3);
        assert!(clicked.is_some_and(|p| p.wants_keyboard), "a click on the field focuses it");
        let (typed, _, _) = frame("gui-test-a", rect, &[egui::Event::Text("x".to_owned())], true, 0.4);
        assert!(!typed.is_empty());
        let (_, _, released) = frame("gui-test-a", rect, &[], false, 0.5);
        assert!(released.is_some_and(|p| !p.wants_keyboard), "a press on the host's widgets takes the focus away");

        let owned = TEXTURES.lock().unwrap().ids.iter().filter(|(o, _, _)| *o == "gui-test-a").count();
        assert_eq!(remove("gui-test-a"), (1, 0));
        assert!(owned > 0 && !TEXTURES.lock().unwrap().ids.iter().any(|(o, _, _)| *o == "gui-test-a"), "the plugin's textures go with it");
        assert_eq!(remove("gui-test-b"), (1, 0));
    }

    #[test]
    fn texture_ids_are_per_plugin_and_never_reused() {
        let mut t = Textures { ids: Vec::new(), next: 0 };
        let (a, b) = (t.id("a", 1), t.id("b", 1));
        assert_ne!(a, b);
        assert_eq!(t.id("a", 1), a);
        assert_eq!(t.free("a", 1), Some(a));
        assert_eq!(t.free("a", 1), None);
        assert_ne!(t.id("a", 1), a, "an id freed is not given out again");
        t.id("a", 2);
        assert_eq!(t.remove_owner("a").len(), 2);
        assert_eq!(t.ids, [("b", 1, b)]);
    }
}
