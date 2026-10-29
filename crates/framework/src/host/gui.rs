//! The overlay's one window, in the look of the game's journal: a row of tabs across the top, Mods,
//! Plugins (the framework plugins with their version, file and state, and the files the host did
//! not load), Legacy plugins (the SKSE plugins in `Data\SKSE\Plugins`) and Settings (every hotkey,
//! set by capturing the next key pressed, and the capture rule). Mods has a sidebar of every plugin
//! that added a tab ([`crate::Host::add_tab`], in load order), the selected one's sections under
//! it when it has two or more, and the selected section beside it.

use std::sync::{Mutex, PoisonError};

use overlay::egui::{self, Align, Layout, RichText, Sense, TextStyle, TextWrapMode, WidgetText, vec2};
use overlay::theme;

use super::owned::Owned;
use super::{HOSTED_BY, NAME, keys, plugins, refused, reload, skse_plugins};
use crate::{DrawFn, HudDrawFn};
use crate::settings::{self, Setting, Value};

/// The host setting holding the capture rule: `focus` (the default) or `all`.
const CAPTURE: &str = "overlay.capture";
/// The window's size until the player drags its corner, and the smallest it gets.
const DEFAULT_SIZE: egui::Vec2 = vec2(1120.0, 680.0);
const MIN_SIZE: egui::Vec2 = vec2(820.0, 500.0);
/// Space between the window's frame and its contents, and above and below the rules.
const MARGIN: egui::Margin = egui::Margin { left: 36, right: 36, top: 22, bottom: 18 };
const RULE_SPACE: f32 = 12.0;

/// One section of a plugin's tab; only the main thread draws it, under the entry's exclusive lock.
struct Section {
    owner: &'static str,
    name: String,
    draw: DrawFn,
}

unsafe impl Sync for Section {}

static SECTIONS: Owned<Section> = Owned::new();

/// A plugin's HUD painter; only the main thread draws it, under the entry's exclusive lock.
struct Hud(HudDrawFn);

unsafe impl Sync for Hud {}

static HUDS: Owned<Hud> = Owned::new();
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

pub(super) fn add_section(owner: &'static str, name: &str, draw: DrawFn) {
    SECTIONS.add(owner, Section { owner, name: name.to_owned(), draw });
}

pub(super) fn add_hud(owner: &'static str, draw: HudDrawFn) {
    HUDS.add(owner, Hud(draw));
}

/// Puts the window and every plugin's HUD painters on the overlay.
pub(super) fn install() {
    overlay::add(draw);
    overlay::on_hud(draw_hud);
}

/// The overlay's HUD callback: every plugin's painter in registration order.
fn draw_hud(painter: &egui::Painter, view: &overlay::View) {
    HUDS.each_mut(|hud| (hud.0)(painter, view));
}

pub(super) fn show_tab(owner: &'static str) {
    select(Tab::Mods, Some(owner.to_owned()), None);
    overlay::set_open(true);
}

/// Drops a plugin's tab and HUD painters and returns how many sections and painters it had.
pub(super) fn remove(owner: &str) -> (usize, usize) {
    TITLES.lock().unwrap_or_else(PoisonError::into_inner).retain(|(o, _)| *o != owner);
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
}

pub(super) fn plugin_rows() -> Vec<PluginRow> {
    let generations = reload::generations();
    let mut rows: Vec<PluginRow> = plugins()
        .iter()
        .map(|p| {
            let (generation, live) = generations.iter().find(|g| g.0 == p.name).map_or((1, true), |g| (g.1, g.2));
            let (status, reason) = match generation {
                _ if !live => ("Stopped".to_owned(), "its load returned false".to_owned()),
                1 => ("Running".to_owned(), String::new()),
                2 => ("Running, reloaded once".to_owned(), String::new()),
                n => (format!("Running, reloaded {} times", n - 1), String::new()),
            };
            PluginRow { name: p.name.clone(), version: p.version.clone(), file: p.file.clone(), status, reason }
        })
        .collect();
    rows.extend(refused().into_iter().map(|(file, reason)| PluginRow { name: String::new(), version: String::new(), file, status: "Not loaded".to_owned(), reason }));
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
        egui::ScrollArea::vertical().id_salt((owner, name)).auto_shrink(false).show(ui, |ui| {
            SECTIONS.each_mut(|s| {
                if s.owner == owner && s.name == *name {
                    (s.draw)(ui);
                }
            });
        });
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

fn plugins_tab(ui: &mut egui::Ui) {
    ui.label(dim(format!("skyrim-rs {}, loaded by {}. Framework plugins are read from Data\\SKSE\\Plugins\\skyrim-rs.", env!("CARGO_PKG_VERSION"), hosted_by())));
    ui.add_space(RULE_SPACE);
    let rows: Vec<_> = plugin_rows()
        .into_iter()
        .map(|r| {
            let status = if r.reason.is_empty() { RichText::new(r.status) } else { warn(ui, r.status) };
            vec![(RichText::new(r.name), String::new()), (dim(r.version), String::new()), (dim(r.file), String::new()), (status, r.reason)]
        })
        .collect();
    if rows.is_empty() {
        ui.label(dim("No framework plugin is installed."));
    } else {
        table(ui, "plugins", &["Name", "Version", "File", "Status"], rows);
    }
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
