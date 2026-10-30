//! `ui/Widgets`: IC's row helpers in the overlay's theme. A section is a heading over a two-column
//! grid, the label on the left and the control (with IC's tooltip) on the right.

use framework::{Action, egui};

use crate::settings::{CamOffset, Offset};

/// `SectionHeader`.
pub fn section_header(ui: &mut egui::Ui, label: &str) {
    ui.add_space(6.0);
    ui.heading(label);
    ui.separator();
}

/// A table of rows, IC's two-column `BeginTable`.
pub fn rows(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([24.0, 6.0]).show(ui, add);
}

fn hint(response: egui::Response, tooltip: &str) -> egui::Response {
    if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) }
}

/// `ToggleRow`: true on the frame it is clicked.
pub fn toggle(ui: &mut egui::Ui, label: &str, value: &mut bool, tooltip: &str) -> bool {
    ui.label(label);
    let changed = hint(ui.checkbox(value, ""), tooltip).changed();
    ui.end_row();
    changed
}

/// `SliderRow`; IC gives some ranges high end first, which is the same range here.
pub fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, a: f32, b: f32, decimals: usize, tooltip: &str) {
    ui.label(label);
    hint(ui.add(egui::Slider::new(value, a.min(b)..=a.max(b)).fixed_decimals(decimals)), tooltip);
    ui.end_row();
}

/// `DragRow`: a drag field for ranges a slider cannot cover.
pub fn drag(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f32, min: f32, max: f32, decimals: usize) {
    ui.label(label);
    ui.add(egui::DragValue::new(value).speed(speed).range(min..=max).fixed_decimals(decimals));
    ui.end_row();
}

/// `DragRow3` with IC's "FUR" axes, for a body offset.
pub fn offset(ui: &mut egui::Ui, label: &str, o: &mut Offset) {
    const LIMIT: f32 = 256.0;
    ui.label(label);
    ui.horizontal(|ui| {
        for (value, axis) in [(&mut o.fForward, "F "), (&mut o.fUp, "U "), (&mut o.fRight, "R ")] {
            ui.add(egui::DragValue::new(value).speed(0.1).range(-LIMIT..=LIMIT).fixed_decimals(1).prefix(axis));
        }
    });
    ui.end_row();
}

/// IC's `CameraSection`: a heading and Forward, Up and Right drags for one camera.
pub fn camera_offset(ui: &mut egui::Ui, label: &str, o: &mut CamOffset) {
    section_header(ui, label);
    rows(ui, label, |ui| {
        for (value, axis) in [(&mut o.forward, "Forward"), (&mut o.up, "Up"), (&mut o.right, "Right")] {
            drag(ui, axis, value, 1.0, -8192.0, 8192.0, 0);
        }
    });
}

/// `ComboRow` over `items`.
pub fn combo(ui: &mut egui::Ui, label: &str, items: &[&str], index: &mut i32, tooltip: &str) {
    ui.label(label);
    let selected = usize::try_from(*index).ok().and_then(|i| items.get(i)).copied().unwrap_or(items[0]);
    hint(
        egui::ComboBox::from_id_salt(label).selected_text(selected).show_ui(ui, |ui| {
            for (i, item) in (0..).zip(items) {
                ui.selectable_value(index, i, *item);
            }
        }).response,
        tooltip,
    );
    ui.end_row();
}

/// `KeyBindRow`: IC's key options are the host's hotkeys here, bound in its Settings tab.
pub fn key(ui: &mut egui::Ui, label: &str, action: Option<&Action>) {
    ui.label(label);
    let bound = action.and_then(Action::key).map_or_else(|| "Unbound".to_owned(), |k| k.to_string());
    ui.label(bound).on_hover_text("Bound in the Settings tab");
    ui.end_row();
}

/// IC's tab bar: one selectable label per tab, the selection kept for the session under `id`.
pub fn tabs(ui: &mut egui::Ui, id: &str, names: &[&str]) -> usize {
    let id = egui::Id::unique(id);
    let mut selected = ui.data(|d| d.get_temp::<usize>(id)).unwrap_or(0);
    ui.horizontal(|ui| {
        for (i, name) in names.iter().enumerate() {
            ui.selectable_value(&mut selected, i, *name);
        }
    });
    ui.separator();
    ui.data_mut(|d| d.insert_temp(id, selected));
    selected
}
