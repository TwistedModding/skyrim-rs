//! The Debug tab: a sidebar of four pages. Load times (the startup steps, each SKSE plugin's Load and
//! listeners, each framework plugin's load), Data files (every loaded plugin file with its type and
//! read time, and the counts against the engine's limits), Logs (the files of the SKSE log folder,
//! filtered and followed) and System (Windows, the processor, memory, the graphics adapter and driver,
//! and what the game uses of them).

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use overlay::egui::{self, Align, Layout, RichText, Sense, TextStyle, vec2};
use overlay::theme;

use super::super::profile::time;
use super::super::{LOG_DIR, SKSE, message_name, plugins, profile, skse_plugins, system};
use super::{RULE_SPACE, dim, heading, hosted_by, sidebar_row, table, version_text, warn};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Loads,
    Files,
    Logs,
    System,
}

const PAGES: [(Page, &str); 4] = [(Page::Loads, "Load times"), (Page::Files, "Data files"), (Page::Logs, "Logs"), (Page::System, "System")];

/// The log file shown and what was read of it.
struct LogView {
    file: Option<PathBuf>,
    filter: String,
    follow: bool,
    lines: Vec<String>,
    /// The file's length and modification time when read, and when it was last looked at.
    read: Option<(u64, Option<SystemTime>, Instant)>,
    error: Option<String>,
}

struct Shown {
    page: Page,
    by_time: bool,
    log: LogView,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown { page: Page::Loads, by_time: false, log: LogView { file: None, filter: String::new(), follow: true, lines: Vec::new(), read: None, error: None } });

/// How much of a log's end is read.
const LOG_TAIL: u64 = 1024 * 1024;
const ROW: f32 = 22.0;

pub(super) fn tab(ui: &mut egui::Ui) {
    let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    egui::Panel::left("debug pages").resizable(false).exact_size(200.0).frame(egui::Frame::NONE.inner_margin(egui::Margin { right: 16, ..egui::Margin::ZERO })).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (page, name) in PAGES {
            if sidebar_row(ui, name, shown.page == page, shown.page == page, 16.0, None) {
                shown.page = page;
            }
        }
    });
    egui::CentralPanel::no_frame().frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 28, right: 8, top: 4, bottom: 0 })).show(ui, |ui| match shown.page {
        Page::Loads => {
            egui::ScrollArea::vertical().id_salt("load times").auto_shrink(false).show(ui, loads);
        }
        Page::Files => files(ui, &mut shown.by_time),
        Page::Logs => logs(ui, &mut shown.log),
        Page::System => {
            egui::ScrollArea::vertical().id_salt("system").auto_shrink(false).show(ui, system_page);
        }
    });
}

fn cell(text: impl Into<RichText>) -> (RichText, String) {
    (text.into(), String::new())
}

fn loads(ui: &mut egui::Ui) {
    heading(ui, "Startup");
    let steps = profile::steps();
    let mut rows: Vec<Vec<(RichText, String)>> = Vec::new();
    let mut last = Duration::ZERO;
    let mut add = |name: &str, at: Duration| {
        rows.push(vec![cell(name), cell(dim(format!("{:.2} s", at.as_secs_f64()))), cell(dim(format!("+{}", time(at.saturating_sub(last)))))]);
        last = at;
    };
    let data = profile::data_files();
    let mut timeline: Vec<(&str, Duration)> = steps.clone();
    timeline.extend(data.and_then(|d| d.first).map(|at| ("first data file opened", at)));
    timeline.sort_by_key(|(_, at)| *at);
    for (name, at) in timeline {
        add(name, at);
    }
    table(ui, "steps", &["Step", "Since the game started", "After the step before"], rows);
    ui.add_space(2.0 * RULE_SPACE);

    heading(ui, "SKSE plugins");
    skse_loads(ui);
    ui.add_space(2.0 * RULE_SPACE);

    heading(ui, "Framework plugins");
    let mut framework = profile::framework_loads();
    framework.sort_by_key(|l| core::cmp::Reverse(l.took));
    if framework.is_empty() {
        ui.label(dim("No framework plugin was loaded."));
    } else {
        let rows = framework.into_iter().map(|l| vec![cell(l.file), cell(time(l.took)), cell(if l.result { RichText::new("Loaded") } else { warn(ui, "Load returned false") })]).collect();
        table(ui, "framework loads", &["File", "Load", "Result"], rows);
    }
}

/// Each SKSE plugin's Load and listeners, as the timing plugin recorded them.
fn skse_loads(ui: &mut egui::Ui) {
    let timing = match skse::timing::table() {
        Some(Ok(timing)) => timing,
        Some(Err(version)) => {
            ui.label(warn(ui, format!("{} has table version {version}; this host reads version {}. Install both from the same package.", skse::timing::FILE, skse::timing::VERSION)));
            return;
        }
        None => {
            ui.label(dim(format!("Not timed: {} is not installed in Data\\SKSE\\Plugins. It comes with this package and times every SKSE plugin's Load and listeners.", skse::timing::FILE)));
            return;
        }
    };
    let calls = timing.calls();
    let mut loads = timing.loads();
    loads.sort_by_key(|l| core::cmp::Reverse(l.took));
    let total: Duration = loads.iter().map(|l| l.took).sum();
    let listening: Duration = calls.iter().map(|c| c.total).sum();
    ui.label(dim(format!("{} plugin loads timed, {} in all; message listeners {} in all. A plugin's Load is timed from the call to its return; a listener from the message reaching it to its return.", loads.len(), time(total), time(listening))));
    ui.label(dim(timing.status()).small());
    let untimed: Vec<String> = skse_plugins().into_iter().filter(|p| p.loaded && !p.file.eq_ignore_ascii_case(skse::timing::FILE) && !loads.iter().any(|l| l.file.eq_ignore_ascii_case(&p.file))).map(|p| p.file).collect();
    let mut rows: Vec<Vec<(RichText, String)>> = loads
        .iter()
        .map(|l| {
            let (spent, count, hover) = listener_summary(&calls, &l.file);
            let result = if l.result { RichText::new("Loaded") } else { warn(ui, "Load returned false") };
            vec![cell(l.file.as_str()), cell(time(l.took)), (dim(if count == 0 { String::new() } else { format!("{} in {count} calls", time(spent)) }), hover), cell(dim(format!("{:.2} s", l.at.as_secs_f64()))), cell(result)]
        })
        .collect();
    for file in calls.iter().map(|c| c.module.clone()).filter(|m| !loads.iter().any(|l| l.file.eq_ignore_ascii_case(m))).collect::<std::collections::BTreeSet<_>>() {
        let (spent, count, hover) = listener_summary(&calls, &file);
        rows.push(vec![cell(file), cell(dim("not timed")), (dim(format!("{} in {count} calls", time(spent))), hover), cell(""), cell("")]);
    }
    if rows.is_empty() {
        ui.label(dim("No SKSE plugin load was timed."));
    } else {
        table(ui, "skse loads", &["File", "Load", "Listeners", "Started at", "Result"], rows);
    }
    if !untimed.is_empty() {
        ui.add_space(4.0);
        ui.label(dim(format!("Loaded without timing: {}", untimed.join(", "))));
    }
}

/// A plugin's listeners: the time they took, how many calls, and the hover text per message kind.
fn listener_summary(calls: &[skse::timing::Calls], file: &str) -> (Duration, u32, String) {
    let (mut spent, mut count, mut lines) = (Duration::ZERO, 0, Vec::new());
    for c in calls.iter().filter(|c| c.module.eq_ignore_ascii_case(file)) {
        spent += c.total;
        count += c.count;
        let kind = if c.sender.eq_ignore_ascii_case("SKSE") { message_name(c.kind).to_owned() } else { format!("{} kind {}", c.sender, c.kind) };
        lines.push(format!("{kind}: {} calls, {} (longest {}), listening to {}", c.count, time(c.total), time(c.max), c.asked));
    }
    (spent, count, lines.join("\n"))
}

/// The type of a data file: its extension, and the flags its extension does not imply.
fn kind(f: &profile::DataFile) -> String {
    let ext = f.name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_uppercase());
    let master = f.master && ext != "ESM" && ext != "ESL";
    let light = f.light && ext != "ESL";
    match (master, light) {
        (false, false) => ext,
        (true, false) => format!("{ext}, master"),
        (false, true) => format!("{ext}, light"),
        (true, true) => format!("{ext}, master and light"),
    }
}

/// One row of fixed-width cells, for lists too long for a grid.
fn row(ui: &mut egui::Ui, cells: &[(RichText, f32)]) {
    ui.horizontal(|ui| {
        for (text, width) in cells {
            ui.allocate_ui_with_layout(vec2(*width, ROW), Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(text.clone()).truncate()));
        }
    });
}

fn files(ui: &mut egui::Ui, by_time: &mut bool) {
    let Some(data) = profile::data_files() else {
        ui.label(dim("The data files are listed once the game has loaded them (the script extender's DATA_LOADED message)."));
        return;
    };
    let full: Vec<&profile::DataFile> = data.files.iter().filter(|f| !f.light).collect();
    let light: Vec<&profile::DataFile> = data.files.iter().filter(|f| f.light).collect();
    let masters = full.iter().filter(|f| f.master).count();
    let esl = light.iter().filter(|f| f.name.to_ascii_lowercase().ends_with(".esl")).count();
    ui.label(format!("{} plugin files: {} full of 254 ({masters} masters, {} plugins), {} light of 4096 ({esl} .esl, {} flagged light)", data.files.len(), full.len(), full.len() - masters, light.len(), light.len() - esl));
    let read: Duration = data.files.iter().map(|f| f.took).sum();
    let span = data.first.zip(data.done).map(|(first, done)| format!("{} from the first file opened to DATA_LOADED; ", time(done.saturating_sub(first)))).unwrap_or_default();
    ui.label(dim(format!("{span}{} reading the files, each timed from its open to its close.", time(read))));
    if data.unseen {
        ui.label(warn(ui, "The files were not timed: the per-file load was not hooked, or another plugin's patch went around it."));
    }
    ui.checkbox(by_time, "Slowest first");
    ui.add_space(4.0);
    let mut sorted: Vec<&profile::DataFile> = data.files.iter().collect();
    if *by_time {
        sorted.sort_by_key(|f| core::cmp::Reverse(f.took));
    }
    let widths = [90.0, (ui.available_width() - 420.0).max(200.0), 170.0, 80.0, 60.0];
    row(ui, &[dim("Index").small(), dim("File").small(), dim("Type").small(), dim("Time").small(), dim("Share").small()].into_iter().zip(widths).collect::<Vec<_>>());
    #[allow(clippy::cast_precision_loss)]
    egui::ScrollArea::vertical().id_salt("data files").auto_shrink(false).show_rows(ui, ROW, sorted.len(), |ui, range| {
        for f in &sorted[range] {
            let share = if read.is_zero() { String::new() } else { format!("{:.1}%", f.took.as_secs_f64() * 100.0 / read.as_secs_f64()) };
            row(ui, &[dim(f.index.as_str()), RichText::new(f.name.as_str()), dim(kind(f)), RichText::new(time(f.took)), dim(share)].into_iter().zip(widths).collect::<Vec<_>>());
        }
    });
}

/// The `.log` files of the SKSE log folder, newest first.
fn log_files() -> Vec<(PathBuf, SystemTime)> {
    let Some(dir) = LOG_DIR.get() else { return Vec::new() };
    let mut files: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("log")))
        .map(|e| (e.path(), e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)))
        .collect();
    files.sort_by_key(|(_, at)| core::cmp::Reverse(*at));
    files
}

/// Reads the last [`LOG_TAIL`] bytes of the shown file when it changed, at most once a second.
fn refresh(view: &mut LogView) {
    let Some(path) = view.file.clone() else { return };
    if view.read.is_some_and(|(_, _, at)| at.elapsed() < Duration::from_secs(1)) {
        return;
    }
    let meta = std::fs::metadata(&path).map(|m| (m.len(), m.modified().ok()));
    let stamp = meta.as_ref().ok().copied();
    if let (Some((len, modified, _)), Some(now)) = (view.read, stamp)
        && (len, modified) == now
    {
        view.read = Some((len, modified, Instant::now()));
        return;
    }
    match read_tail(&path) {
        Ok(text) => {
            view.lines = text.lines().map(str::to_owned).collect();
            view.error = None;
        }
        Err(e) => view.error = Some(e.to_string()),
    }
    let (len, modified) = stamp.unwrap_or((0, None));
    view.read = Some((len, modified, Instant::now()));
}

fn read_tail(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let start = if len > LOG_TAIL { bytes.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1) } else { 0 };
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

fn logs(ui: &mut egui::Ui, view: &mut LogView) {
    let files = log_files();
    if files.is_empty() {
        ui.label(dim("No log file in the SKSE log folder."));
        return;
    }
    if view.file.as_ref().is_none_or(|f| !files.iter().any(|(p, _)| p == f)) {
        let own = format!("{}.log", super::NAME);
        view.file = Some(files.iter().find(|(p, _)| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(own.as_str()))).unwrap_or(&files[0]).0.clone());
        view.read = None;
    }
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ui.horizontal(|ui| {
        let selected = view.file.as_ref().map(name).unwrap_or_default();
        egui::ComboBox::from_id_salt("log file").width(320.0).selected_text(selected).show_ui(ui, |ui| {
            for (path, _) in &files {
                if ui.selectable_label(view.file.as_ref() == Some(path), name(path)).clicked() {
                    view.file = Some(path.clone());
                    view.read = None;
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Filter").desired_width(220.0));
        ui.checkbox(&mut view.follow, "Follow");
        if ui.button("Reload").clicked() {
            view.read = None;
        }
    });
    refresh(view);
    if let Some(e) = &view.error {
        ui.label(warn(ui, format!("Not read: {e}")));
    }
    let filter = view.filter.to_lowercase();
    let lines: Vec<&String> = view.lines.iter().filter(|l| filter.is_empty() || l.to_lowercase().contains(&filter)).collect();
    ui.label(dim(format!("{} of {} lines{}", lines.len(), view.lines.len(), if view.lines.is_empty() { String::new() } else { format!(", the last {} KB at most", LOG_TAIL / 1024) })).small());
    ui.add_space(4.0);
    let height = ui.text_style_height(&TextStyle::Monospace);
    let warn_color = ui.visuals().warn_fg_color;
    egui::ScrollArea::both().id_salt("log lines").auto_shrink(false).stick_to_bottom(view.follow).show_rows(ui, height, lines.len(), |ui, range| {
        for line in &lines[range] {
            let lower = line.to_lowercase();
            let color = if ["error", "fail", "crash", "exception", "warn"].iter().any(|w| lower.contains(w)) { warn_color } else { theme::TEXT };
            ui.add(egui::Label::new(RichText::new(line.as_str()).monospace().color(color)).wrap_mode(egui::TextWrapMode::Extend).sense(Sense::hover()));
        }
    });
}

fn system_page(ui: &mut egui::Ui) {
    let machine = system::machine();
    let usage = system::usage();
    heading(ui, "Machine");
    let mut rows = vec![
        vec![cell("Windows"), cell(machine.windows.as_str())],
        vec![cell("Processor"), cell(format!("{}, {} cores, {} threads", machine.cpu, machine.cores, machine.threads))],
        vec![cell("Memory"), cell(format!("{}, {} free", system::bytes(machine.memory), system::bytes(usage.memory_free)))],
    ];
    match &machine.adapter {
        Some(a) => {
            rows.push(vec![cell("Graphics"), cell(format!("{} ({}, device {:04X})", a.name, a.vendor_name(), a.device))]);
            rows.push(vec![cell("Driver"), cell(a.driver_text().unwrap_or_else(|| "not reported".to_owned()))]);
            rows.push(vec![cell("Video memory"), cell(format!("{} dedicated, {} shared", system::bytes(a.dedicated_memory), system::bytes(a.shared_memory)))]);
        }
        None => rows.push(vec![cell("Graphics"), cell(dim("the game's device was not read"))]),
    }
    table(ui, "machine", &["", ""], rows);
    ui.add_space(2.0 * RULE_SPACE);
    heading(ui, "Game");
    let skse = SKSE.get();
    let runtime = skse.map_or_else(String::new, |s| version_text(s.runtime_version));
    let loader = skse.filter(|_| hosted_by() == "the script extender").map_or_else(|| hosted_by().to_owned(), |s| format!("SKSE {}", version_text(s.skse_version)));
    let legacy = skse_plugins();
    let data = profile::data_files().map_or_else(|| "not loaded yet".to_owned(), |d| {
        let light = d.files.iter().filter(|f| f.light).count();
        format!("{} ({} full, {light} light)", d.files.len(), d.files.len() - light)
    });
    let mut rows = vec![
        vec![cell("Game version"), cell(runtime)],
        vec![cell("Loaded by"), cell(loader)],
        vec![cell("Running for"), cell(format!("{:.0} s", skse::timing::since_start().as_secs_f64()))],
        vec![cell("Process memory"), cell(format!("{} in use, {} committed", system::bytes(usage.working_set), system::bytes(usage.private)))],
        vec![cell("DLLs loaded"), cell(format!("{} in the process", usage.modules))],
        vec![cell("SKSE plugins"), cell(format!("{} loaded, {} in Data\\SKSE\\Plugins", legacy.iter().filter(|p| p.loaded).count(), legacy.len()))],
        vec![cell("Framework plugins"), cell(plugins().len().to_string())],
        vec![cell("Plugin files"), cell(data)],
    ];
    if let Some((used, budget)) = usage.video {
        rows.insert(4, vec![cell("Video memory"), cell(format!("{} in use of a {} budget", system::bytes(used), system::bytes(budget)))]);
    }
    table(ui, "game", &["", ""], rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, master: bool, light: bool) -> profile::DataFile {
        profile::DataFile { name: name.to_owned(), index: String::new(), master, light, took: Duration::ZERO }
    }

    #[test]
    fn names_a_file_by_its_extension_and_the_flag_that_differs() {
        assert_eq!(kind(&file("Skyrim.esm", true, false)), "ESM");
        assert_eq!(kind(&file("Mod.esp", false, false)), "ESP");
        assert_eq!(kind(&file("ccBGSSSE001-Fish.esm", true, true)), "ESM, light");
        assert_eq!(kind(&file("Small.esp", false, true)), "ESP, light");
        assert_eq!(kind(&file("Patch.esl", true, true)), "ESL");
        assert_eq!(kind(&file("Flagged.esp", true, false)), "ESP, master");
        assert_eq!(kind(&file("Both.esp", true, true)), "ESP, master and light");
    }

    #[test]
    fn times_read_as_a_person_writes_them() {
        assert_eq!(time(Duration::from_micros(400)), "0.4 ms");
        assert_eq!(time(Duration::from_millis(12)), "12 ms");
        assert_eq!(time(Duration::from_millis(1240)), "1.24 s");
    }

    #[test]
    fn reads_the_end_of_a_long_log_from_a_line_start() {
        let path = std::env::temp_dir().join(format!("skyrim-rs-tail-{}.log", std::process::id()));
        let line = "x".repeat(99) + "\n";
        std::fs::write(&path, line.repeat(20_000)).unwrap();
        let text = read_tail(&path).unwrap();
        assert!(text.len() as u64 <= LOG_TAIL && text.lines().all(|l| l.len() == 99));
        std::fs::write(&path, "a\nb\n").unwrap();
        assert_eq!(read_tail(&path).unwrap(), "a\nb\n");
        let _ = std::fs::remove_file(&path);
    }
}
