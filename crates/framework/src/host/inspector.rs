//! The host's own window, "skyrim-rs": the plugins in load order with their generation, the
//! windows with their open state, the overlay's capture rule, every hook site with its subscribers
//! (each one switchable) and the dispatcher's last messages. The console command `SkyrimRs` prints
//! the same and drives it: `window` opens the overlay on it, `hooks` lists the sites, `toggle <id>`
//! switches a subscriber, `capture` writes the next overlay frame to `skyrim-rs-overlay.bmp` in the
//! SKSE log folder.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use overlay::egui::{self, RichText};
use overlay::theme;
use skyrim::input::{Arg, CommandContext, Param, ScriptParamType};

use super::{HOOKS, NAME, WINDOW_LIST, flush_settings, plugins, reload, set_window_open, window_open};
use crate::hooks::{SiteReport, SubscriptionId};
use crate::order::Constraints;
use crate::settings::{self, Setting, Value};

/// The inspector's id among the host's windows.
pub(super) const ID: &str = "inspector";
/// The host setting holding the capture rule: `focus` (the default) or `all`.
const CAPTURE: &str = "overlay.capture";
const REFRESH: Duration = Duration::from_millis(500);
const SHOT: &str = "skyrim-rs-overlay.bmp";

struct Snapshot {
    taken: Instant,
    sites: Vec<SiteReport>,
    messages: Vec<String>,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// The dispatcher's state, read at most twice a second (each report re-reads entry bytes from the executable).
fn snapshot<R>(f: impl FnOnce(&Snapshot) -> R) -> R {
    let mut cached = SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner);
    if cached.as_ref().is_none_or(|s| s.taken.elapsed() > REFRESH) {
        *cached = Some(Snapshot { taken: Instant::now(), sites: HOOKS.diagnostics(), messages: HOOKS.messages() });
    }
    f(cached.as_ref().expect("filled above"))
}

fn invalidate() {
    *SNAPSHOT.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

fn focus_capture() -> bool {
    settings::get(NAME, CAPTURE).and_then(|s| String::from_setting(&s)).is_none_or(|v| v != "all")
}

/// Sets the overlay's capture rule from the host's settings.
pub(super) fn apply_capture() {
    overlay::set_capture(if focus_capture() { overlay::Capture::Focus } else { overlay::Capture::All });
}

fn constraints(c: &Constraints) -> String {
    if *c == Constraints::default() { String::new() } else { c.to_string() }
}

/// How a site's entry looked at install and looks now.
fn site_state(site: &SiteReport) -> String {
    if site.validation == site.current { format!("entry {}", site.current) } else { format!("entry {} at install, {} now", site.validation, site.current) }
}

fn plugin_lines() -> Vec<String> {
    let generations = reload::generations();
    plugins()
        .iter()
        .enumerate()
        .map(|(rank, p)| {
            let (generation, live) = generations.iter().find(|g| g.0 == p.name).map_or((0, false), |g| (g.1, g.2));
            let state = if live { format!("generation {generation}") } else { format!("generation {generation}, not running") };
            format!("plugin {rank}: {} {state} ({}; {})", p.name, p.file, constraints(&p.constraints))
        })
        .collect()
}

fn window_lines() -> Vec<String> {
    WINDOW_LIST.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(owner, id, title)| format!("window {owner}/{id} \"{title}\": {}", if window_open(owner, id) { "open" } else { "closed" })).collect()
}

fn hook_lines() -> Vec<String> {
    invalidate();
    snapshot(|s| {
        let mut lines = Vec::new();
        for site in &s.sites {
            lines.push(format!("site {} at {:#x}: {} calls, last {} ns, mean {} ns, last outcome {:?}, {}", site.name, site.address, site.timing.calls, site.timing.last_ns, site.timing.mean_ns, site.last_outcome, site_state(site)));
            lines.extend(site.entries.iter().map(|e| format!("  {e}")));
        }
        lines.extend(s.messages.iter().map(|m| format!("message: {m}")));
        lines
    })
}

fn toggle(id: u64) -> String {
    let id = SubscriptionId::new(id);
    let enabled = snapshot(|s| s.sites.iter().flat_map(|site| &site.entries).find(|e| e.id == id).map(|e| e.enabled));
    invalidate();
    match enabled {
        Some(enabled) if HOOKS.set_enabled(id, !enabled) => format!("#{} {}", id.get(), if enabled { "disabled" } else { "enabled" }),
        _ => format!("no subscription #{}", id.get()),
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text.to_uppercase()).text_style(egui::TextStyle::Heading).size(22.0));
    ui.separator();
}

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::DIM)
}

/// Draws the inspector; registered as the host's window.
pub(super) fn draw(window: egui::Window<'_>, ctx: &egui::Context) {
    window.default_pos([1150.0, 60.0]).default_size([720.0, 860.0]).show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            heading(ui, "Plugins");
            let generations = reload::generations();
            egui::Grid::new("plugins").striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                for (rank, p) in plugins().iter().enumerate() {
                    let (generation, live) = generations.iter().find(|g| g.0 == p.name).map_or((0, false), |g| (g.1, g.2));
                    ui.label(dim(format!("{}", rank + 1)));
                    ui.label(&p.name);
                    ui.label(if live { dim(format!("gen {generation}")) } else { RichText::new(format!("gen {generation}, stopped")).color(ui.visuals().warn_fg_color) });
                    ui.label(dim(&p.file));
                    ui.label(dim(constraints(&p.constraints)));
                    ui.end_row();
                }
            });

            heading(ui, "Windows");
            let list = WINDOW_LIST.lock().unwrap_or_else(PoisonError::into_inner).clone();
            for (owner, id, title) in list.iter().filter(|(owner, _, _)| *owner != NAME) {
                let mut open = window_open(owner, id);
                if ui.checkbox(&mut open, format!("{title}  ")).on_hover_text(format!("{owner}/{id}")).changed() {
                    set_window_open(owner, id, open);
                }
            }
            let mut focus = focus_capture();
            if ui.checkbox(&mut focus, "Keys reach the game while no field has the focus").changed() {
                settings::set(NAME, CAPTURE, Setting::Text(if focus { "focus" } else { "all" }.to_owned()));
                apply_capture();
            }
            ui.label(dim(format!("Font: {}", overlay::font_source())).small());

            heading(ui, "Hooks");
            let (sites, messages) = snapshot(|s| (s.sites.clone(), s.messages.clone()));
            if sites.is_empty() {
                ui.label(dim("No hooked function."));
            }
            for site in &sites {
                egui::CollapsingHeader::new(RichText::new(&site.name).strong()).id_salt(site.address).default_open(true).show(ui, |ui| {
                    ui.label(dim(format!("{:#x}, {}", site.address, site_state(site))));
                    ui.label(dim(format!("{} calls, last {} ns, mean {} ns, last outcome {:?}", site.timing.calls, site.timing.last_ns, site.timing.mean_ns, site.last_outcome)));
                    egui::Grid::new(("entries", site.address)).striped(true).spacing([14.0, 2.0]).show(ui, |ui| {
                        for e in &site.entries {
                            let mut enabled = e.enabled;
                            if ui.checkbox(&mut enabled, format!("#{}", e.id.get())).changed() && HOOKS.set_enabled(e.id, enabled) {
                                invalidate();
                            }
                            let text = |s: String| if e.enabled { RichText::new(s) } else { RichText::new(s).color(theme::DISABLED) };
                            ui.label(text(e.owner.clone()));
                            ui.label(text(format!("{:?}", e.phase)));
                            ui.label(dim(format!("rank {}", e.rank)));
                            ui.label(dim(constraints(&e.constraints)));
                            ui.label(dim(format!("{} calls, mean {} ns", e.timing.calls, e.timing.mean_ns)));
                            ui.end_row();
                        }
                    });
                });
            }

            heading(ui, "Messages");
            for m in messages.iter().rev() {
                let refused = m.contains("refused");
                ui.label(if refused { RichText::new(m).color(ui.visuals().warn_fg_color) } else { dim(m) }.small());
            }
        });
    });
}

static PARAMS: [Param; 2] = [Param::new(c"what", ScriptParamType::String, false), Param::new(c"id", ScriptParamType::Integer, true)];

/// `SkyrimRs window | hooks | toggle <id> | capture`.
pub(super) struct Command;

impl skyrim::input::Command for Command {
    const NAME: &'static core::ffi::CStr = c"SkyrimRs";
    const HELP: &'static core::ffi::CStr = c"skyrim-rs: window, hooks, toggle <subscription id>, capture";
    const PARAMS: &'static [Param] = &PARAMS;

    fn execute(context: &mut CommandContext<'_>) -> bool {
        let lines = match context.args() {
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("window") => {
                overlay::set_open(true);
                set_window_open(NAME, ID, true);
                let mut lines = vec![format!("SkyrimRs: overlay open, font {}, capture {:?}", overlay::font_source(), overlay::capture())];
                lines.extend(plugin_lines());
                lines.extend(window_lines());
                lines
            }
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("hooks") => hook_lines(),
            [Arg::Str(what), Arg::Int(id)] if what.eq_ignore_ascii_case("toggle") => vec![format!("SkyrimRs: {}", u64::try_from(*id).map_or_else(|_| format!("no subscription #{id}"), toggle))],
            [Arg::Str(what), ..] if what.eq_ignore_ascii_case("capture") => {
                let path = super::LOG_DIR.get().cloned().unwrap_or_default().join(SHOT);
                overlay::save_next_frame(path.clone(), |done| match done {
                    Ok(path) => log!("overlay frame written to {}", path.display()),
                    Err(e) => log!("overlay frame not written: {e}"),
                });
                vec![format!("SkyrimRs: the next overlay frame goes to {}", path.display())]
            }
            _ => vec!["SkyrimRs: window, hooks, toggle <subscription id> or capture".to_owned()],
        };
        flush_settings(Some(NAME));
        for line in lines {
            skyrim::input::print(&line);
        }
        true
    }
}
