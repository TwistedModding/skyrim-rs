//! `ui/menu/ConsoleMenu` ("Console"): the plugin's log lines since the game started, with IC's
//! filter, clear and error and warning colours.

use std::collections::VecDeque;
use std::sync::Mutex;

use framework::egui;

const MAX_ITEMS: usize = 4096;
const PRUNE_COUNT: usize = 512;

static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

fn lines() -> std::sync::MutexGuard<'static, VecDeque<String>> {
    LINES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps one log line; past `MAX_ITEMS` the oldest `PRUNE_COUNT` go.
pub fn push(line: &str) {
    let mut lines = lines();
    if lines.len() >= MAX_ITEMS {
        lines.drain(..PRUNE_COUNT);
    }
    lines.push_back(line.to_owned());
}

pub fn draw(ui: &mut egui::Ui) {
    let id = egui::Id::unique("ConsoleFilter");
    let mut filter = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.add(egui::TextEdit::singleline(&mut filter).desired_width(180.0));
        if ui.button("Clear").clicked() {
            lines().clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, filter.clone()));
    ui.separator();
    for line in lines().iter().filter(|l| l.contains(filter.as_str())) {
        let text = egui::RichText::new(line.as_str()).monospace();
        let text = if line.contains("[ERROR]") {
            text.color(egui::Color32::from_rgb(255, 102, 102))
        } else if line.contains("[WARNING]") {
            text.color(egui::Color32::from_rgb(255, 204, 153))
        } else {
            text
        };
        ui.label(text);
    }
}
