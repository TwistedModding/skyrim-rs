//! The overlay's one window, in the look of the game's journal: a row of tabs, one per plugin that
//! adds sections ([`crate::Host::add_tab`], in load order; two or more sections are tabs of their
//! own inside it), then the host's Plugins (the framework plugins with their version, file and
//! state, and the files it did not load), Legacy plugins (the SKSE plugins in `Data\SKSE\Plugins`)
//! and Settings (every hotkey, set by capturing the next key pressed, and the capture rule).

use std::sync::{Mutex, PoisonError};

use overlay::egui::{self, Align, Align2, Layout, RichText, Sense, TextStyle, TextWrapMode, WidgetText, vec2};
use overlay::theme;

use super::owned::Owned;
use super::{HOSTED_BY, NAME, keys, plugins, refused, reload, skse_plugins};
use crate::DrawFn;
use crate::settings::{self, Setting, Value};

/// The host setting holding the capture rule: `focus` (the default) or `all`.
const CAPTURE: &str = "overlay.capture";
/// Room kept under the scroll area for the footer.
const FOOTER: f32 = 44.0;

/// One section of a plugin's tab; only the main thread draws it, under the entry's exclusive lock.
struct Section {
    owner: &'static str,
    name: String,
    draw: DrawFn,
}

unsafe impl Sync for Section {}

static SECTIONS: Owned<Section> = Owned::new();
/// Each plugin's tab title.
static TITLES: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

#[derive(Clone, PartialEq, Eq)]
enum Tab {
    Plugin(String),
    Plugins,
    Legacy,
    Settings,
}

/// The selected tab (the first while `None`) and each plugin's selected section, by name, so a
/// reloaded generation finds its own again.
struct Shown {
    tab: Option<Tab>,
    sections: Vec<(String, String)>,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown { tab: None, sections: Vec::new() });

/// A tab as the window lists it, with its sections when it is a plugin's.
struct Entry {
    tab: Tab,
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

pub(super) fn show_tab(owner: &'static str) {
    select(Tab::Plugin(owner.to_owned()), None);
    overlay::set_open(true);
}

/// Drops a plugin's tab and returns how many sections it had.
pub(super) fn remove(owner: &str) -> usize {
    TITLES.lock().unwrap_or_else(PoisonError::into_inner).retain(|(o, _)| *o != owner);
    SECTIONS.remove_owner(owner)
}

fn title_of(owner: &str) -> String {
    TITLES.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(o, _)| *o == owner).map_or_else(|| owner.to_owned(), |(_, t)| t.clone())
}

fn entries() -> Vec<Entry> {
    let mut sections: Vec<(&'static str, String)> = Vec::new();
    SECTIONS.each(|s| {
        sections.push((s.owner, s.name.clone()));
        true
    });
    let mut list: Vec<Entry> = plugins()
        .iter()
        .filter_map(|p| {
            let names: Vec<String> = sections.iter().filter(|(o, _)| *o == p.name).map(|(_, n)| n.clone()).collect();
            (!names.is_empty()).then(|| Entry { tab: Tab::Plugin(p.name.clone()), title: title_of(&p.name), sections: names })
        })
        .collect();
    list.extend([(Tab::Plugins, "Plugins"), (Tab::Legacy, "Legacy plugins"), (Tab::Settings, "Settings")].map(|(tab, title)| Entry { tab, title: title.to_owned(), sections: Vec::new() }));
    list
}

/// The selected tab's index and its selected section's.
fn selection(entries: &[Entry]) -> (usize, usize) {
    let shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let tab = shown.tab.as_ref().and_then(|t| entries.iter().position(|e| e.tab == *t)).unwrap_or(0);
    let section = match &entries[tab].tab {
        Tab::Plugin(owner) => shown.sections.iter().find(|(o, _)| o == owner).and_then(|(_, name)| entries[tab].sections.iter().position(|n| n == name)).unwrap_or(0),
        _ => 0,
    };
    (tab, section)
}

/// Selects `tab` and, for a plugin's tab, `section` (the one selected before when `None`).
fn select(tab: Tab, section: Option<String>) {
    let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    if let (Tab::Plugin(owner), Some(section)) = (&tab, section) {
        shown.sections.retain(|(o, _)| o != owner);
        shown.sections.push((owner.clone(), section));
    }
    shown.tab = Some(tab);
}

/// Every tab, or every section of a plugin's tab, as `SkyrimRs gui` numbers them, and whether it is shown.
pub(super) fn pages() -> Vec<(String, bool)> {
    let entries = entries();
    let (tab, section) = selection(&entries);
    let mut pages = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.sections.is_empty() {
            pages.push((e.title.clone(), i == tab));
        }
        for (j, s) in e.sections.iter().enumerate() {
            pages.push((format!("{} / {s}", e.title), i == tab && j == section));
        }
    }
    pages
}

/// Selects page `n` of [`pages`]; false when there is none.
pub(super) fn show_page(n: usize) -> bool {
    let mut pages = entries().into_iter().flat_map(|e| if e.sections.is_empty() { vec![(e.tab, None)] } else { e.sections.into_iter().map(|s| (e.tab.clone(), Some(s))).collect() });
    match pages.nth(n) {
        Some((tab, section)) => {
            select(tab, section);
            true
        }
        None => false,
    }
}

pub(super) fn select_settings() {
    select(Tab::Settings, None);
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
    ui.add_space(6.0);
    ui.label(RichText::new(text.to_uppercase()).text_style(TextStyle::Heading).size(22.0));
    ui.separator();
}

/// A row of tab names, centred, the selected one white: between the journal's arrows on the top
/// row, on the highlight below it. Returns the one clicked.
#[allow(clippy::cast_precision_loss)]
fn tab_row(ui: &mut egui::Ui, names: &[String], selected: usize, top: bool) -> Option<usize> {
    let style = if top { TextStyle::Heading } else { TextStyle::Body };
    let galleys: Vec<_> = names.iter().map(|n| WidgetText::from(n.as_str()).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, style.clone())).collect();
    let (pad, gap) = if top { (vec2(26.0, 4.0), 30.0) } else { (vec2(14.0, 2.0), 6.0) };
    let total = galleys.iter().map(|g| g.size().x + 2.0 * pad.x).sum::<f32>() + gap * names.len().saturating_sub(1) as f32;
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
        for (i, galley) in galleys.into_iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::click());
            let on = i == selected;
            let color = if on || response.hovered() { theme::TEXT } else { theme::DIM };
            let painter = ui.painter();
            if on && top {
                let (y, stroke) = (rect.center().y, egui::Stroke::new(1.5, color));
                let (l, r) = (rect.left() + 4.0, rect.right() - 4.0);
                painter.add(egui::Shape::convex_polygon(vec![egui::pos2(l, y), egui::pos2(l + 10.0, y - 7.0), egui::pos2(l + 10.0, y + 7.0)], egui::Color32::TRANSPARENT, stroke));
                painter.add(egui::Shape::convex_polygon(vec![egui::pos2(r, y), egui::pos2(r - 10.0, y + 7.0), egui::pos2(r - 10.0, y - 7.0)], egui::Color32::TRANSPARENT, stroke));
            } else if on {
                painter.rect_filled(rect, 0.0, theme::HIGHLIGHT);
            }
            painter.galley(rect.min + pad, galley, color);
            if response.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// A striped table under a dim header row; each cell with its hover text, empty for none.
fn table(ui: &mut egui::Ui, id: &str, header: &[&str], rows: Vec<Vec<(RichText, String)>>) {
    egui::Grid::new(id).striped(true).spacing([28.0, 6.0]).show(ui, |ui| {
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
    ui.add_space(8.0);
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
    ui.add_space(8.0);
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
    egui::Grid::new("keys").striped(true).spacing([28.0, 6.0]).show(ui, |ui| {
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
        ui.label(if message.contains("refused") || message.contains("unbound") { warn(ui, message) } else { dim(message) });
    }
    heading(ui, "Input");
    let mut focus = focus_capture();
    if ui.checkbox(&mut focus, "Keys reach the game while no field in this menu has the focus").changed() {
        settings::set(NAME, CAPTURE, Setting::Text(if focus { "focus" } else { "all" }.to_owned()));
        apply_capture();
    }
}

/// The journal's key hint at the bottom right: the close key and "Exit", which closes the overlay when clicked.
fn footer(ui: &mut egui::Ui) {
    ui.separator();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add(egui::Label::new(dim("Exit")).sense(Sense::click()).selectable(false)).clicked() {
            overlay::set_open(false);
        }
        if let Some(key) = keys::key(keys::CLOSE).or_else(|| keys::key(keys::TOGGLE)) {
            egui::Frame::new().stroke(egui::Stroke::new(1.0, theme::DIM)).inner_margin(egui::Margin::symmetric(5, 0)).show(ui, |ui| ui.label(dim(key.to_string()).small()));
        }
    });
}

/// Draws the window; the overlay's one window callback.
pub(super) fn draw(ctx: &egui::Context) {
    let entries = entries();
    let (tab, section) = selection(&entries);
    let entry = &entries[tab];
    let window = egui::Window::new(NAME).id(egui::Id::unique((NAME, "gui"))).title_bar(false).pivot(Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).default_size([1120.0, 680.0]);
    window.show(ctx, |ui| {
        let titles: Vec<String> = entries.iter().map(|e| e.title.to_uppercase()).collect();
        if let Some(i) = tab_row(ui, &titles, tab, true) {
            select(entries[i].tab.clone(), None);
        }
        ui.separator();
        if entry.sections.len() > 1
            && let Some(j) = tab_row(ui, &entry.sections, section, false)
        {
            select(entry.tab.clone(), Some(entry.sections[j].clone()));
        }
        ui.add_space(4.0);
        let height = (ui.available_height() - FOOTER).max(80.0);
        egui::ScrollArea::vertical().id_salt((tab, section)).auto_shrink(false).max_height(height).show(ui, |ui| match &entry.tab {
            Tab::Plugin(owner) => {
                let name = &entry.sections[section];
                SECTIONS.each_mut(|s| {
                    if s.owner == owner && s.name == *name {
                        (s.draw)(ui);
                    }
                });
            }
            Tab::Plugins => plugins_tab(ui),
            Tab::Legacy => legacy_tab(ui),
            Tab::Settings => settings_tab(ui),
        });
        footer(ui);
    });
}
