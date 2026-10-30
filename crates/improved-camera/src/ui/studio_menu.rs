//! `ui/menu/StudioMenu`: the photo mode's paged menu as the tab's "Photo Mode" section (IC's
//! standalone window; Back returns to Home), with the mouse outside the overlay's windows driving
//! the orbit camera (drag rotates, right-drag pans, the wheel dollies, Ctrl+wheel sets the fly
//! speed, Shift+wheel the FOV) and, on the Bones page, a drag that starts on a bone marker posing
//! that bone (Shift moves it in the view plane). Closing the overlay releases the input to the free
//! camera's flight; the photo mode's input key brings the menu back.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use framework::egui::{self, Color32, Painter, Pos2, Stroke};
use framework::View;
use skyrim::forms::TESFormExt;
use skyrim::render::{self as render, NiAVObjectExt};

use super::widgets::{combo, rows, section_header, slider, toggle};
use crate::camera::studio::adjustments::{axis_angle, degrees_from_rotation, rotation_from_degrees, third_person_3d};
use crate::camera::studio::capture::ASPECT_NAMES;
use crate::camera::studio::environment::{self, ImodInfo};
use crate::camera::studio::idles::{self, PosePack};
use crate::camera::studio::io::{self, CameraBookmark, SamPack, Scene, SceneSun};
use crate::camera::studio::manager::{self, CameraPose, Manager};
use crate::camera::studio::{gear, node_names, shaders};
use crate::util::point;
use crate::util::skyrim_util as util;

pub const SECTION: &str = "Photo Mode";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Home,
    Target,
    Pose,
    Bones,
    Position,
    Effects,
    Gear,
    Camera,
    Environment,
    Scenes,
    Idles,
    Options,
}

#[derive(Default)]
struct Menu {
    page: Page,
    selected_adjustment: usize,
    selected_bone: String,
    last_pose_target: u32,
    bone_search: String,
    show_all_bones: bool,
    bone_group: usize,
    files_dirty: bool,
    adjustment_files: Vec<String>,
    pose_files: Vec<String>,
    sam_poses: Vec<SamPack>,
    pose_source: usize,
    selected_adjustment_file: String,
    selected_pose_file: String,
    pose_save_name: String,
    layer_save_name: String,
    bookmarks: Option<Vec<CameraBookmark>>,
    bookmark_name: String,
    scene_files: Option<Vec<String>>,
    scene_save_name: String,
    selected_scene: String,
    packs: Option<Vec<PosePack>>,
    selected_pack: usize,
    idle_search: String,
    last_event: String,
    imods: Option<Vec<ImodInfo>>,
    imod_search: String,
    weathers: Option<Vec<environment::WeatherInfo>>,
    /// The effect browser's lists: shaders, art objects.
    effects: Option<[Vec<shaders::EffectInfo>; 2]>,
    effect_art: bool,
    effect_search: String,
    weather_search: String,
    imod_strength: Option<f32>,
    /// World units per dragged pixel on the Position page (IC: 1).
    move_strength: Option<f32>,
    object_search: String,
    /// The mouse button driving the orbit (0 rotate, 1 pan) or a bone drag (2).
    drag: Option<u8>,
}

static MENU: Mutex<Option<Menu>> = Mutex::new(None);
static MARKERS: Mutex<Vec<(String, Pos2)>> = Mutex::new(Vec::new());
static FRAME: AtomicU64 = AtomicU64::new(0);
static DRAWN: AtomicU64 = AtomicU64::new(0);

fn menu<R>(f: impl FnOnce(&mut Menu) -> R) -> R {
    f(MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(|| Menu { files_dirty: true, ..Menu::default() }))
}

pub fn markers<R>(f: impl FnOnce(&mut Vec<(String, Pos2)>) -> R) -> R {
    f(&mut MARKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// From the frame callback: one more frame.
pub fn tick() {
    FRAME.fetch_add(1, Ordering::Relaxed);
}

/// IC's `IsInputCaptured`: the menu is up (the section drew this frame or the last), so the
/// mouse belongs to the studio; with the overlay closed the free camera flies.
#[must_use]
pub fn input_captured() -> bool {
    DRAWN.load(Ordering::Relaxed) + 1 >= FRAME.load(Ordering::Relaxed)
}

fn contains_no_case(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// The bones the markers show: SAM's posing set when the skeleton has any of it (everything on a
/// creature it does not cover), and only bones a mesh uses, unless "Show All Bones" is on.
fn bone_shown(name: &str, used: bool, show_all: bool, has_known: bool) -> bool {
    show_all || (used && (!has_known || node_names::find(name).is_some()))
}

/// The markers of the Bones page, projected through the frame camera.
pub fn paint_markers(painter: &Painter, view: &View) {
    let (page, selected, search, show_all) = menu(|m| (m.page, m.selected_bone.clone(), m.bone_search.clone(), m.show_all_bones));
    if page != Page::Bones {
        return;
    }
    let bones = manager::with(|m| m.bones.clone());
    let has_known = bones.iter().any(|b| node_names::find(&b.name).is_some());
    let mut placed = Vec::new();
    for bone in bones.iter().filter(|b| bone_shown(&b.name, b.used, show_all, has_known) && contains_no_case(&b.name, &search)) {
        let Some(at) = view.project(bone.world) else { continue };
        let picked = bone.name == selected;
        let (radius, color, width) = if picked { (7.0, Color32::from_rgb(255, 210, 80), 2.5) } else { (3.5, Color32::from_white_alpha(140), 1.5) };
        painter.circle_stroke(at, radius, Stroke::new(width, color));
        placed.push((bone.name.clone(), at));
    }
    markers(|m| *m = placed);
}

/// `TryBeginBoneDrag`: the nearest marker within reach of the pointer becomes the edited bone.
fn try_begin_bone_drag(pointer: Pos2) -> bool {
    let best = markers(|m| m.iter().map(|(n, at)| (n.clone(), at.distance(pointer))).filter(|(_, d)| *d <= 6.0).min_by(|a, b| a.1.total_cmp(&b.1)));
    let Some((name, _)) = best else { return false };
    let index = manager::with(|m| {
        let target = m.target()?;
        let stack = &mut m.pose.get_or_create(&target).stack;
        if stack.is_empty() {
            stack.push(crate::camera::studio::adjustments::Adjustment { name: "Pose Adjustment".to_owned(), ..Default::default() });
        }
        Some(stack.len())
    });
    let Some(len) = index else { return false };
    menu(|m| {
        m.selected_adjustment = m.selected_adjustment.min(len - 1);
        m.selected_bone = name;
    });
    true
}

/// `UpdateBoneDrag`: the drag in camera space mapped into the bone's frames: with Shift the bone
/// moves in the view plane (parent space), else it turns about world Z and the camera's right.
fn update_bone_drag(dx: f32, dy: f32, shift: bool) {
    let (bone, layer) = menu(|m| (m.selected_bone.clone(), m.selected_adjustment));
    let yaw = Manager::read_camera_pose().yaw;
    manager::with(|m| {
        let Some(point) = m.bones.iter().find(|b| b.name == bone).cloned() else { return };
        let Some(target) = m.target() else { return };
        let Some(adjustment) = m.pose.actors.get_mut(&target.formID).and_then(|a| a.stack.get_mut(layer)) else { return };
        let delta = adjustment.bones.entry(bone).or_default();
        let right = [yaw.cos(), -yaw.sin(), 0.0];
        if shift {
            let world = [right[0] * dx * 0.06, right[1] * dx * 0.06, -dy * 0.06];
            let scale = if point.parent_scale > 0.0001 { point.parent_scale } else { 1.0 };
            delta.position = point::add(delta.position, point::scale(point::mul_vec(&point::transpose(&point.parent_rotate), world), 1.0 / scale));
        } else {
            let inverse = point::transpose(&point.rotate);
            let spin_yaw = axis_angle((dx * 0.3).to_radians(), point::mul_vec(&inverse, [0.0, 0.0, 1.0]));
            let spin_pitch = axis_angle((dy * 0.3).to_radians(), point::mul_vec(&inverse, right));
            let rotation = point::mul(&point::mul(&rotation_from_degrees(delta.rotation), &spin_yaw), &spin_pitch);
            delta.rotation = degrees_from_rotation(&rotation);
        }
    });
}

/// IC's `UIManager` mouse routing: what the pointer does outside the overlay's windows.
fn route_mouse(ctx: &egui::Context) {
    let over_window = ctx.is_pointer_over_egui();
    let (pressed, released, delta, scroll, pointer, modifiers) = ctx.input(|i| {
        let pressed = [egui::PointerButton::Primary, egui::PointerButton::Secondary].map(|b| i.pointer.button_pressed(b));
        (pressed, i.pointer.any_released(), i.pointer.delta(), i.events.iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum::<f32>(), i.pointer.interact_pos(), i.modifiers)
    });
    if released {
        menu(|m| m.drag = None);
    }
    if !over_window && menu(|m| m.drag.is_none()) {
        if pressed[0] {
            let bone = menu(|m| m.page == Page::Bones) && pointer.is_some_and(try_begin_bone_drag);
            menu(|m| m.drag = Some(if bone { 2 } else { 0 }));
        } else if pressed[1] {
            menu(|m| m.drag = Some(1));
        }
    }
    match menu(|m| m.drag) {
        Some(0) => manager::with(|m| m.orbit_rotate(delta.x, delta.y)),
        Some(1) => manager::with(|m| m.orbit_pan(delta.x, delta.y)),
        Some(_) => update_bone_drag(delta.x, delta.y, modifiers.shift),
        None => {}
    }
    if !over_window && scroll != 0.0 {
        let steps = scroll.signum();
        if modifiers.shift {
            let mut pose = Manager::read_camera_pose();
            pose.fov = (pose.fov - steps * 5.0).clamp(5.0, 165.0);
            manager::with(|m| m.request_camera_pose(pose, false, false, false, true));
        } else if modifiers.ctrl {
            Manager::adjust_fly_speed(steps);
        } else {
            manager::with(|m| m.orbit_zoom(steps));
        }
    }
}

/// `SectionTitle`.
fn title(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.vertical_centered(|ui| ui.strong(text));
    ui.separator();
}

/// `MenuRow`: a full-width row, its value on the right.
fn row(ui: &mut egui::Ui, label: &str, value: Option<&str>, tooltip: &str) -> bool {
    let response = ui.add_sized([ui.available_width(), 24.0], egui::Button::new(label).right_text(value.unwrap_or_default()));
    let response = if tooltip.is_empty() { response } else { response.on_hover_text(tooltip) };
    response.clicked()
}

/// `PageHeader`: the page's title under a Back row; true when Back is clicked.
fn page_header(ui: &mut egui::Ui, text: &str) -> bool {
    let back = ui.button("< Back").clicked();
    title(ui, text);
    back
}

pub fn draw(ui: &mut egui::Ui) {
    if util::is_menu_open("Loading Menu") {
        return;
    }
    DRAWN.store(FRAME.load(Ordering::Relaxed), Ordering::Relaxed);
    manager::with(Manager::poll_console_selection);
    route_mouse(ui.ctx());
    let (active, target_id, target_name) = manager::with(|m| {
        let t = m.target();
        (m.active, t.as_ref().map_or(0, |t| t.formID), manager::display_name(t.as_deref()))
    });
    menu(|m| {
        if m.last_pose_target != target_id {
            m.last_pose_target = target_id;
            m.selected_bone.clear();
            m.selected_adjustment = 0;
        }
    });
    ui.heading(&target_name);
    ui.weak("Drag outside the menu: rotate / pan (right) / zoom (wheel). Close the overlay to fly.");
    ui.separator();
    if !active {
        if row(ui, "Enter Photo Mode", None, "Opens the free camera with the studio (ic pm)") {
            manager::with(Manager::enter);
        }
        return;
    }
    let page = menu(|m| m.page);
    let back = match page {
        Page::Home => {
            home(ui, &target_name);
            false
        }
        Page::Target => target(ui),
        Page::Pose => pose(ui),
        Page::Bones => bones(ui),
        Page::Position => position(ui),
        Page::Effects => effects_page(ui),
        Page::Gear => gear_page(ui),
        Page::Camera => camera(ui),
        Page::Environment => environment_page(ui),
        Page::Scenes => scenes(ui),
        Page::Idles => idle_page(ui),
        Page::Options => options(ui),
    };
    if back {
        menu(|m| m.page = if page == Page::Bones { Page::Pose } else { Page::Home });
    }
}

fn home(ui: &mut egui::Ui, target_name: &str) {
    title(ui, "Photo Mode");
    let go = |page| menu(|m| m.page = page);
    if row(ui, "Target", Some(target_name), "") {
        go(Page::Target);
    }
    if row(ui, "Pose", None, "") {
        go(Page::Pose);
    }
    if row(ui, "Position", None, "") {
        go(Page::Position);
    }
    if row(ui, "Gear", None, "Hide the target's helmet, hair, shield, quiver or weapons for the shot") {
        go(Page::Gear);
    }
    if row(ui, "Effects", None, "Hold an effect shader or art object on the target - glows, membranes, auras") {
        go(Page::Effects);
    }
    if row(ui, "Camera", None, "") {
        go(Page::Camera);
    }
    if row(ui, "Environment", None, "Sun angle and colour grading") {
        go(Page::Environment);
    }
    if row(ui, "Pose Packs", None, "") {
        go(Page::Idles);
    }
    if row(ui, "Scenes", None, "Save or restore the camera framing and the sun") {
        go(Page::Scenes);
    }
    if row(ui, "Options", None, "") {
        go(Page::Options);
    }
    ui.add_space(10.0);
    if row(ui, "Exit Photo Mode", None, "") {
        manager::with(Manager::exit);
    }
}

fn target(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Target") {
        return true;
    }
    if row(ui, "Target Player", None, "") {
        manager::with(Manager::target_player);
    }
    let console = manager::display_name(manager::console_ref().as_deref());
    if row(ui, "Target Console Selection", Some(&console), "A ref clicked in the game console also becomes the target automatically") {
        manager::with(Manager::target_console_ref);
    }
    let crosshair = manager::display_name(manager::crosshair_ref().as_deref());
    if row(ui, "Target Crosshair Pick", Some(&crosshair), "") {
        manager::with(Manager::target_crosshair_ref);
    }
    let (target, nearby, objects) = manager::with(|s| (s.target().map(|t| t.formID), s.nearby.clone(), s.objects.clone()));
    title(ui, "Nearby");
    super::edit(ui, |ui, p| rows(ui, "NearbyRadius", |ui| slider(ui, "Search radius", &mut p.Studio.fNearbyRadius, 256.0, 8192.0, 0, "")));
    let mut pick = None;
    nearby_list(ui, "NearbyActors", &nearby, target, "", "No actors within radius", 200.0, &mut pick);
    title(ui, "Nearby Objects");
    let search = menu(|m| {
        ui.add(egui::TextEdit::singleline(&mut m.object_search).hint_text("Search props...").desired_width(f32::INFINITY));
        m.object_search.clone()
    });
    nearby_list(ui, "NearbyObjects", &objects, target, &search, "No objects within radius", f32::INFINITY, &mut pick);
    if let Some(r) = pick.and_then(|h: skyrim::actors::RefHandle| h.lookup()) {
        manager::with(|s| s.set_target(Some(&r)));
    }
    false
}

/// A selectable list of nearby references with their distance on the right.
#[allow(clippy::too_many_arguments)]
fn nearby_list(ui: &mut egui::Ui, id: &str, entries: &[manager::Nearby], target: Option<u32>, search: &str, empty: &str, height: f32, pick: &mut Option<skyrim::actors::RefHandle>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(id).max_height(height).show(ui, |ui| {
            if entries.is_empty() {
                ui.weak(empty);
            }
            for entry in entries.iter().filter(|e| contains_no_case(&e.name, search)) {
                let Some(r) = entry.handle.lookup() else { continue };
                ui.horizontal(|ui| {
                    if ui.selectable_label(target == Some(r.formID), &entry.name).clicked() {
                        *pick = Some(entry.handle);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.weak(format!("{:.0}", entry.distance)));
                });
            }
        });
    });
}

fn gear_page(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Gear") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()).filter(IsActor::cast_actor) else {
        ui.weak("Target an actor to hide its equipment.");
        return false;
    };
    let Some(actor) = target.cast::<sre::Actor>() else { return false };
    ui.weak("Hidden for the shot only - equipment is restored when Photo Mode closes.");
    rows(ui, "StudioTableGear", |ui| {
        for slot in gear::Slot::ALL {
            let equipped = gear::is_slot_filled(actor, slot);
            let mut hidden = manager::with(|s| s.gear.is_hidden(actor.formID, slot));
            ui.label(slot.name());
            let response = ui.add_enabled(equipped, egui::Checkbox::without_text(&mut hidden)).on_hover_text(if equipped { "Hide this for the shot" } else { "Nothing equipped in this slot" });
            if response.changed() {
                manager::with(|s| s.gear.set_hidden(actor, slot, hidden));
            }
            ui.end_row();
        }
    });
    if ui.add_sized([ui.available_width(), 22.0], egui::Button::new("Show Everything")).clicked() {
        manager::with(|s| s.gear.set_hidden_mask(actor, 0));
    }
    false
}

fn effects_page(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Effects") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()) else {
        ui.weak("Target something to hold an effect on it.");
        return false;
    };
    ui.weak("Held with no duration and removed when Photo Mode closes.");
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    ui.horizontal(|ui| {
        ui.radio_value(&mut m.effect_art, false, "Effect Shaders");
        ui.radio_value(&mut m.effect_art, true, "Art Objects");
    });
    if ui.add_sized([ui.available_width(), 22.0], egui::Button::new("Remove All")).clicked() {
        manager::with(Manager::remove_all_effects);
    }
    ui.add(egui::TextEdit::singleline(&mut m.effect_search).hint_text("Search effects...").desired_width(f32::INFINITY));
    let art = m.effect_art;
    let lists = m.effects.get_or_insert_with(|| [shaders::effects(false), shaders::effects(true)]);
    let handle = skyrim::actors::TESObjectREFRExt::handle(&*target);
    egui::ScrollArea::vertical().id_salt("EffectList").show(ui, |ui| {
        for effect in lists[usize::from(art)].iter().filter(|e| contains_no_case(&e.label, &m.effect_search)) {
            let entry = shaders::Applied { handle, effect: effect.form_id, art };
            let applied = manager::with(|s| s.shaders.is_applied(entry));
            if ui.selectable_label(applied, &effect.label).clicked() {
                manager::with(|s| if applied { s.remove_effect(entry) } else { s.apply_effect(entry) });
            }
        }
    });
    false
}

fn position(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Position") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()) else {
        ui.weak("No target selected");
        return false;
    };
    let paused = Manager::paused();
    if row(ui, if paused { "Resume Game" } else { "Pause Game" }, None, "Freezes world time - note most posing works better unpaused (freeze actors instead)") {
        Manager::toggle_pause();
    }
    let collision_off = manager::with(|s| s.is_collision_disabled(target.formID));
    if row(ui, if collision_off { "Enable Collision" } else { "Disable Collision" }, collision_off.then_some("off - levitating"), "Turns the target's collision off so it can be placed anywhere (levitate)") {
        manager::with(Manager::toggle_collision);
    }
    if let Some(actor) = target.cast::<sre::Actor>() {
        let ik_off = util::graph_bool(&target, "bHumanoidFootIKDisable");
        if row(ui, if ik_off { "Enable Foot IK" } else { "Disable Foot IK" }, ik_off.then_some("off"), "Foot IK makes legs fight manual posing - disable it per target while posing legs") {
            manager::with(|s| s.set_foot_ik_disabled(actor, !ik_off));
        }
    }
    let depth = manager::with(|s| s.undo_depth());
    let steps = format!("{depth} step(s)");
    if ui.add_enabled_ui(depth > 0, |ui| row(ui, "Undo", (depth > 0).then_some(steps.as_str()), "")).inner {
        manager::with(Manager::undo);
    }
    title(ui, "Transform");
    let strength = menu(|m| *m.move_strength.get_or_insert(1.0));
    let mut new_strength = strength;
    ui.add(egui::Slider::new(&mut new_strength, 0.05..=25.0).logarithmic(true).text("Drag strength")).on_hover_text("How far one pixel of dragging moves the target - low for micro-spacing, high for big moves");
    menu(|m| m.move_strength = Some(new_strength));
    let mut position = util::position(&target);
    let mut angle = [target.rotX, target.rotY, target.rotZ].map(f32::to_degrees);
    let mut scale = f32::from(target.refScale) / 100.0;
    let triple = |ui: &mut egui::Ui, label: &str, values: &mut [f32; 3], speed: f32| {
        ui.horizontal(|ui| {
            let mut started = false;
            let mut changed = false;
            for value in values.iter_mut() {
                let r = ui.add(egui::DragValue::new(value).speed(speed).fixed_decimals(2));
                started |= r.drag_started() || r.gained_focus();
                changed |= r.changed();
            }
            ui.label(label);
            (started, changed)
        })
        .inner
    };
    let (started, changed) = triple(ui, "Position", &mut position, strength);
    let (started_angle, changed_angle) = triple(ui, "Rotation", &mut angle, 0.25);
    let (started_scale, changed_scale) = ui
        .horizontal(|ui| {
            let r = ui.add(egui::DragValue::new(&mut scale).speed(0.002).range(0.01..=10.0).fixed_decimals(3));
            ui.label("Scale");
            (r.drag_started() || r.gained_focus(), r.changed())
        })
        .inner;
    manager::with(|s| {
        if started || started_angle || started_scale {
            s.push_undo();
        }
        if changed {
            s.request_position(position);
        }
        if changed_angle {
            s.request_angle(angle.map(f32::to_radians));
        }
        if changed_scale {
            s.request_scale(scale);
        }
    });
    ui.weak("Drag to adjust - double-click to type an exact value");
    false
}

fn refresh_files(m: &mut Menu) {
    if !m.files_dirty {
        return;
    }
    m.files_dirty = false;
    m.adjustment_files = io::list_files(&io::directory("Adjustments"));
    for pack in io::list_sam_packs(&io::sam_dir("Adjustments")) {
        for file in pack.files {
            m.adjustment_files.push(if pack.name.is_empty() { format!("[SAM] {file}") } else { format!("[SAM] {}/{file}", pack.name) });
        }
    }
    m.pose_files = io::list_files(&io::directory("Poses"));
    m.sam_poses = io::list_sam_packs(&io::sam_dir("Poses"));
    if m.pose_source > m.sam_poses.len() {
        m.pose_source = 0;
        m.selected_pose_file.clear();
    }
}

fn stem(file: &str) -> &str {
    file.rsplit_once('.').map_or(file, |(s, _)| s)
}

fn file_combo(ui: &mut egui::Ui, id: &str, files: &[String], selection: &mut String, empty: &str) {
    egui::ComboBox::from_id_salt(id).width(ui.available_width() * 0.55).selected_text(if selection.is_empty() { empty } else { stem(selection) }).show_ui(ui, |ui| {
        for file in files {
            ui.selectable_value(selection, file.clone(), stem(file));
        }
    });
}

fn log_all(notes: Vec<String>) {
    for note in notes {
        crate::log(&format!("<Photo Mode> {note}"));
    }
}

fn pose(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Pose") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()).filter(IsActor::cast_actor) else {
        ui.weak("Target an actor to pose");
        return false;
    };
    let Some(root) = third_person_3d(&target) else {
        ui.weak("Target has no 3D loaded");
        return false;
    };
    freeze_toggle(ui, &target, false);
    if row(ui, "Edit Bones", None, "") {
        manager::with(|m| {
            let stack = &mut m.pose.get_or_create(&target).stack;
            if stack.is_empty() {
                stack.push(crate::camera::studio::adjustments::Adjustment { name: "Pose Adjustment".to_owned(), ..Default::default() });
            }
        });
        menu(|m| m.page = Page::Bones);
        return false;
    }
    let mut m = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = m.get_or_insert_with(Menu::default);
    refresh_files(m);
    pose_files(ui, m, &target, root);
    title(ui, "Adjustments");
    if row(ui, "New Adjustment", None, "") {
        m.selected_adjustment = manager::with(|s| {
            let a = s.pose.get_or_create(&target);
            a.new_adjustment();
            a.stack.len() - 1
        });
    }
    let mut load_layer = None;
    ui.horizontal(|ui| {
        file_combo(ui, "AdjustmentFiles", &m.adjustment_files.clone(), &mut m.selected_adjustment_file, "Select file...");
        if ui.button("Load Layer").clicked() && !m.selected_adjustment_file.is_empty() {
            load_layer = Some(m.selected_adjustment_file.clone());
        }
    });
    if let Some(file) = load_layer {
        let mut notes = Vec::new();
        let loaded = match file.strip_prefix("[SAM] ") {
            Some(relative) => match relative.split_once('/') {
                Some((pack, f)) => io::load_sam_file(&io::sam_dir("Adjustments"), pack, f, &mut notes),
                None => io::load_sam_file(&io::sam_dir("Adjustments"), "", relative, &mut notes),
            },
            None => io::load_adjustment(&io::directory("Adjustments"), &file).map(|a| crate::camera::studio::adjustments::Adjustment { file: file.clone(), ..a }),
        };
        match loaded {
            Ok(layer) => {
                m.selected_adjustment = manager::with(|s| {
                    let a = s.pose.get_or_create(&target);
                    a.stack.push(layer);
                    a.stack.len() - 1
                });
            }
            Err(e) => notes.push(e),
        }
        log_all(notes);
    }
    manager::with(|s| layer_editor(ui, m, s, &target));
    false
}

fn pose_files(ui: &mut egui::Ui, m: &mut Menu, target: &sre::TESObjectREFR, root: &sre::NiAVObject) {
    title(ui, "Pose Files");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut m.pose_save_name).hint_text("Pose name...").desired_width(ui.available_width() * 0.55));
        if ui.button("Save Pose").on_hover_text("Snapshots the actor's current stance (layers marked 'In Pose Export' only) so it reproduces under any idle").clicked() {
            let name = io::sanitize_file_name(&m.pose_save_name);
            if !name.is_empty() {
                let mut locals = BTreeMap::new();
                root.for_each(&mut |o| {
                    if !core::ptr::eq(o, root)
                        && o.cast::<sre::NiNode>().is_some()
                        && let Some(n) = skyrim::BSFixedString::view(&o.name).to_str().filter(|n| !n.is_empty())
                    {
                        locals.insert(n.to_owned(), o.local_transform());
                    }
                });
                manager::with(|s| {
                    if let Some(a) = s.pose.actors.get(&target.formID) {
                        a.pose_locals(&mut locals);
                    }
                });
                match io::save_pose(&io::directory("Poses"), &name, &locals) {
                    Ok(path) => crate::log(&format!("<Photo Mode> Saved pose: {} ({} bones)", path.display(), locals.len())),
                    Err(e) => crate::log(&format!("<Photo Mode> {e}")),
                }
                m.files_dirty = true;
            }
        }
    });
    let source_label = |i: usize, packs: &[SamPack]| if i == 0 { "Photo Mode".to_owned() } else if packs[i - 1].name.is_empty() { "SAM".to_owned() } else { format!("SAM: {}", packs[i - 1].name) };
    egui::ComboBox::from_id_salt("PoseSource").width(ui.available_width() * 0.55).selected_text(source_label(m.pose_source, &m.sam_poses)).show_ui(ui, |ui| {
        for i in 0..=m.sam_poses.len() {
            if ui.selectable_label(i == m.pose_source, source_label(i, &m.sam_poses)).clicked() && i != m.pose_source {
                m.pose_source = i;
                m.selected_pose_file.clear();
            }
        }
    });
    let files = if m.pose_source == 0 { m.pose_files.clone() } else { m.sam_poses[m.pose_source - 1].files.clone() };
    let mut load = None;
    ui.horizontal(|ui| {
        file_combo(ui, "PoseFiles", &files, &mut m.selected_pose_file, "Select pose...");
        if ui.button("Load Pose").on_hover_text("Applies the pose as the bottom stack layer (SAM packs are authored over the A-Pose)").clicked() && !m.selected_pose_file.is_empty() {
            load = Some(m.selected_pose_file.clone());
        }
        if ui.button("Random").clicked() && !files.is_empty() {
            let pick = files[usize::try_from(ui.ctx().cumulative_frame_nr()).unwrap_or(0) % files.len()].clone();
            m.selected_pose_file.clone_from(&pick);
            load = Some(pick);
        }
    });
    if let Some(file) = load {
        let mut notes = Vec::new();
        let source = m.pose_source;
        let pack = (source > 0).then(|| m.sam_poses[source - 1].name.clone());
        manager::with(|s| {
            let adjustments = s.pose.get_or_create(target);
            match &pack {
                None => match io::load_pose(&io::directory("Poses"), &file) {
                    Ok(pose) => {
                        let count = adjustments.import_pose(&file, &pose).unwrap_or(0);
                        notes.push(format!("Imported pose '{file}' ({count} bones differ)"));
                    }
                    Err(e) => notes.push(e),
                },
                Some(pack) => match io::load_sam_file(&io::sam_dir("Poses"), pack, &file, &mut notes) {
                    Ok(loaded) => adjustments.install_pose(stem(&file), loaded.bones),
                    Err(e) => notes.push(e),
                },
            }
            if let Some(i) = adjustments.stack.iter().position(|a| a.is_pose) {
                m.selected_adjustment = i;
            }
        });
        log_all(notes);
    }
}

/// The selected actor's stack: the layer list, rename, delete, clear, order, group reset, save and
/// export, and the export flag.
fn layer_editor(ui: &mut egui::Ui, m: &mut Menu, s: &mut Manager, target: &sre::TESObjectREFR) {
    let Some(adjustments) = s.pose.actors.get_mut(&target.formID).filter(|a| !a.stack.is_empty()) else {
        ui.weak("No adjustments on this actor yet");
        return;
    };
    m.selected_adjustment = m.selected_adjustment.min(adjustments.stack.len() - 1);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("AdjustmentStack").max_height(160.0).show(ui, |ui| {
            for (i, layer) in adjustments.stack.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut layer.enabled, "").on_hover_text("Enable/Disable this adjustment layer");
                    if ui.selectable_label(i == m.selected_adjustment, format!("{} ({} bones)", layer.name, layer.bones.len())).clicked() {
                        m.selected_adjustment = i;
                    }
                });
            }
        });
    });
    let selected = m.selected_adjustment;
    let count = adjustments.stack.len();
    ui.add(egui::TextEdit::singleline(&mut adjustments.stack[selected].name).id_salt(("AdjustmentName", selected)).desired_width(f32::INFINITY));
    let (mut delete, mut clear, mut up, mut down) = (false, false, false, false);
    ui.horizontal(|ui| {
        delete = ui.button("Delete").clicked();
        clear = ui.button("Clear Bones").on_hover_text("Removes every bone edit from the selected adjustment").clicked();
        up = ui.add_enabled(selected > 0, egui::Button::new("Up")).clicked();
        down = ui.add_enabled(selected + 1 < count, egui::Button::new("Down")).clicked();
    });
    if clear {
        adjustments.stack[selected].bones.clear();
    }
    if up {
        adjustments.move_adjustment(selected, true);
        m.selected_adjustment -= 1;
    }
    if down {
        adjustments.move_adjustment(selected, false);
        m.selected_adjustment += 1;
    }
    if delete {
        adjustments.delete_adjustment(selected);
        return;
    }
    let layer = &mut adjustments.stack[m.selected_adjustment];
    let categories = node_names::categories();
    m.bone_group = m.bone_group.min(categories.len() - 1);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("BoneGroup").width(ui.available_width() * 0.5).selected_text(categories[m.bone_group]).show_ui(ui, |ui| {
            for (i, c) in categories.iter().enumerate() {
                ui.selectable_value(&mut m.bone_group, i, *c);
            }
        });
        if ui.button("Reset Group").on_hover_text("Drops this layer's edits for that body region - play the A-Pose first to send it back to the default stance").clicked() {
            let group = categories[m.bone_group];
            layer.bones.retain(|name, _| node_names::find(name).is_none_or(|n| n.category != group));
        }
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut m.layer_save_name).hint_text("Save as... (default: layer name)").desired_width(ui.available_width() * 0.55));
        let name = io::sanitize_file_name(if m.layer_save_name.is_empty() { &layer.name } else { &m.layer_save_name });
        if ui.button("Save Layer").on_hover_text("Saves the selected adjustment's deltas as a reusable file").clicked() && !name.is_empty() {
            match io::save_adjustment(&io::directory("Adjustments"), layer, &name) {
                Ok(path) => {
                    layer.file.clone_from(&name);
                    m.files_dirty = true;
                    crate::log(&format!("<Photo Mode> Saved adjustment: {}", path.display()));
                }
                Err(e) => crate::log(&format!("<Photo Mode> {e}")),
            }
        }
        if ui.button("Export OS").on_hover_text("Exports the selected layer as an Outfit Studio pose .xml (Poses folder)").clicked() && !name.is_empty() {
            match io::save_outfit_studio_pose(&io::directory("Poses"), layer, &name) {
                Ok(path) => crate::log(&format!("<Photo Mode> Exported Outfit Studio pose: {}", path.display())),
                Err(e) => crate::log(&format!("<Photo Mode> {e}")),
            }
        }
    });
    ui.checkbox(&mut layer.include_in_pose, "In Pose Export").on_hover_text("Bake this layer into saved pose files (turn off for skeleton fixes)");
}

/// The Freeze / Unfreeze switch of the Pose page (a row) and the Bones page (a small button).
fn freeze_toggle(ui: &mut egui::Ui, target: &sre::TESObjectREFR, small: bool) {
    let Some(actor) = target.cast::<sre::Actor>() else { return };
    let frozen = manager::with(|s| s.pose.is_actor_frozen(target.formID));
    let label = if frozen { "Unfreeze" } else { "Freeze" };
    let clicked = if small { ui.small_button(label).clicked() } else { row(ui, label, None, "Pauses the actor's animation so the pose holds still while editing") };
    if clicked {
        manager::with(|s| s.pose.set_actor_frozen(actor, !frozen));
    }
}

/// Playing an event unfreezes first: a frozen graph swallows events.
fn unfreeze(target: &sre::TESObjectREFR) {
    if let Some(actor) = target.cast::<sre::Actor>() {
        manager::with(|s| s.pose.set_actor_frozen(actor, false));
    }
}

fn bones(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Bones") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()).filter(IsActor::cast_actor) else {
        ui.weak("Target an actor to pose");
        return false;
    };
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    freeze_toggle(ui, &target, true);
    manager::with(|s| {
        let points = s.bones.clone();
        let Some(adjustments) = s.pose.actors.get_mut(&target.formID).filter(|a| !a.stack.is_empty()) else {
            ui.weak("No adjustment selected");
            return;
        };
        m.selected_adjustment = m.selected_adjustment.min(adjustments.stack.len() - 1);
        let layer = &mut adjustments.stack[m.selected_adjustment];
        ui.weak(format!("Editing layer: {}", layer.name));
        ui.checkbox(&mut m.show_all_bones, "Show All Bones").on_hover_text("Include helper nodes (CME/MOV/physics) in the list and markers; normally only SAM's posing set is shown");
        ui.add(egui::TextEdit::singleline(&mut m.bone_search).hint_text("Search bones...").desired_width(f32::INFINITY));
        let mut buckets: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        let mut other = Vec::new();
        for b in points.iter().filter(|b| m.show_all_bones || b.used) {
            match node_names::find(&b.name) {
                Some(e) => buckets.entry(e.category).or_default().push((b.name.as_str(), e.display_name)),
                None => other.push((b.name.as_str(), b.name.as_str())),
            }
        }
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("BoneList").max_height(260.0).show(ui, |ui| {
                let mut list = |category: &str, rows: &[(&str, &str)]| {
                    let mut header = false;
                    for (nif, display) in rows {
                        if !contains_no_case(nif, &m.bone_search) && !contains_no_case(display, &m.bone_search) {
                            continue;
                        }
                        if !header {
                            header = true;
                            ui.label(egui::RichText::new(category).strong());
                        }
                        let edited = if layer.bones.contains_key(*nif) { "* " } else { "" };
                        if ui.selectable_label(*nif == m.selected_bone, format!("{edited}{display}")).on_hover_text(*nif).clicked() {
                            (*nif).clone_into(&mut m.selected_bone);
                        }
                    }
                };
                for category in node_names::CATEGORY_ORDER {
                    if let Some(rows) = buckets.get(category) {
                        list(category, rows);
                    }
                }
                if m.show_all_bones || buckets.is_empty() {
                    list("Other", &other);
                }
            });
        });
        if m.selected_bone.is_empty() {
            ui.weak("Select a bone to edit");
            return;
        }
        let name = node_names::find(&m.selected_bone).map_or(m.selected_bone.as_str(), |e| e.display_name).to_owned();
        title(ui, &name);
        let mut delta = layer.bones.get(&m.selected_bone).copied().unwrap_or_default();
        let mut changed = false;
        rows(ui, "BoneEdit", |ui| {
            for (label, values, speed) in [("Position", &mut delta.position, 0.05), ("Rotation", &mut delta.rotation, 0.25)] {
                ui.label(label);
                ui.horizontal(|ui| {
                    for v in values.iter_mut() {
                        changed |= ui.add(egui::DragValue::new(v).speed(speed).max_decimals(3)).changed();
                    }
                });
                ui.end_row();
            }
            ui.label("Scale");
            changed |= ui.add(egui::DragValue::new(&mut delta.scale).speed(0.005).range(0.01..=20.0).max_decimals(3)).changed();
            ui.end_row();
        });
        if changed {
            layer.bones.insert(m.selected_bone.clone(), delta);
        }
        if ui.button("Reset Bone").clicked() {
            layer.bones.remove(&m.selected_bone);
        }
        ui.weak("Drag to adjust; double-click to type an exact value. On screen, drag a marker to turn the bone, Shift-drag to move it.");
        if let Some(node) = third_person_3d(&target).and_then(|r| util::object_by_name(r, &m.selected_bone)) {
            let local = node.local_transform();
            let e = degrees_from_rotation(&local.rotation);
            let t = local.translation;
            ui.weak(format!("Local  P {:.3} {:.3} {:.3}  R {:.2} {:.2} {:.2}  S {:.3}", t[0], t[1], t[2], e[0], e[1], e[2], local.scale));
        }
    });
    false
}

fn camera(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Camera") {
        return true;
    }
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    let bookmarks = m.bookmarks.get_or_insert_with(|| io::load_camera_bookmarks(&io::photo_mode_dir()));
    let mut pinned = manager::with(|s| s.orbit_enabled);
    if ui.checkbox(&mut pinned, "Pin to Target (orbit)").on_hover_text("Off = free framing; editing position/rotation below releases the pin automatically").changed() {
        manager::with(|s| s.orbit_enabled = pinned);
    }
    let speed = util::free_camera_speed();
    let mut fly = *speed;
    if ui.add(egui::Slider::new(&mut fly, 1.0..=60.0).text("Fly speed").fixed_decimals(0)).changed() {
        skyrim::store(speed, fly);
    }
    title(ui, "Framing");
    let mut pose = Manager::read_camera_pose();
    let (mut pitch, mut yaw) = (pose.pitch.to_degrees(), pose.yaw.to_degrees());
    let mut request = (false, false, false, false);
    rows(ui, "Framing", |ui| {
        ui.label("Position");
        ui.horizontal(|ui| {
            for v in &mut pose.position {
                request.0 |= ui.add(egui::DragValue::new(v).speed(1.0).fixed_decimals(1)).changed();
            }
        });
        ui.end_row();
        ui.label("Pitch");
        request.1 |= ui.add(egui::DragValue::new(&mut pitch).speed(0.25).range(-89.9..=89.9).fixed_decimals(2)).changed();
        ui.end_row();
        ui.label("Yaw");
        request.1 |= ui.add(egui::DragValue::new(&mut yaw).speed(0.25).fixed_decimals(2)).changed();
        ui.end_row();
        ui.label("Roll");
        request.2 |= ui.add(egui::DragValue::new(&mut pose.roll).speed(0.25).range(-180.0..=180.0).fixed_decimals(2)).changed();
        ui.end_row();
        ui.label("FOV");
        request.3 |= ui.add(egui::DragValue::new(&mut pose.fov).speed(0.25).range(5.0..=165.0).fixed_decimals(1)).changed();
        ui.end_row();
    });
    if request != (false, false, false, false) {
        pose.pitch = pitch.to_radians();
        pose.yaw = yaw.to_radians();
        manager::with(|s| s.request_camera_pose(pose, request.0, request.1, request.2, request.3));
    }
    title(ui, "Capture");
    let (busy, remaining, last) = manager::with(|s| (s.capture.is_busy(), s.capture.remaining(), s.capture.last_shot.clone()));
    if ui.add_enabled(!busy, egui::Button::new("Take Screenshot")).on_hover_text("Writes a PNG to PhotoMode\\Shots with no menu and no HUD in it").clicked() {
        manager::with(Manager::request_shot);
    }
    if ui.add_enabled(!busy && !bookmarks.is_empty(), egui::Button::new("Capture All Bookmarks")).on_hover_text("Flies to each saved shot in turn and captures it").clicked() {
        let shots = bookmarks.clone();
        manager::with(|s| s.request_bracket(shots));
    }
    if busy {
        ui.colored_label(Color32::from_rgb(255, 210, 80), format!("Capturing... {remaining} left"));
    } else if !last.is_empty() {
        ui.weak(format!("Last: {last}"));
    }
    super::edit(ui, |ui, p| {
        ui.checkbox(&mut p.Studio.bCaptureCropToGuide, "Crop to Aspect Guide").on_hover_text("Write the file cropped to the aspect the guide masks, instead of the full frame");
    });
    title(ui, "Controls");
    for (input, action) in [("Left-drag", "Orbit the target (outside the menu)"), ("Right-drag", "Pan"), ("Scroll", "Dolly closer / further"), ("Ctrl+Scroll", "Camera / drag speed"), ("Shift+Scroll", "FOV zoom"), ("Close the overlay", "Fly (WASD + mouse); the photo mode input key returns")] {
        ui.horizontal(|ui| {
            ui.weak(input);
            ui.add_space(12.0);
            ui.weak(action);
        });
    }
    bookmark_list(ui, m);
    false
}

fn bookmark_list(ui: &mut egui::Ui, m: &mut Menu) {
    title(ui, "Bookmarks");
    let mut save = false;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut m.bookmark_name).hint_text("Bookmark name...").desired_width(ui.available_width() * 0.55));
        save = ui.button("Save Shot").clicked();
    });
    let bookmarks = m.bookmarks.get_or_insert_with(Vec::new);
    let mut changed = false;
    if save {
        let name = if m.bookmark_name.is_empty() { format!("Shot {}", bookmarks.len() + 1) } else { m.bookmark_name.clone() };
        bookmarks.push(Manager::read_camera_pose().bookmark(name));
        changed = true;
    }
    let mut delete = None;
    for (i, b) in bookmarks.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.small_button("x").clicked() {
                delete = Some(i);
            }
            if ui.selectable_label(false, &b.name).clicked() {
                let recalled = CameraPose::from(b);
                manager::with(|s| s.request_camera_pose(recalled, true, true, true, true));
            }
        });
    }
    if let Some(i) = delete {
        bookmarks.remove(i);
        changed = true;
    }
    if changed && let Err(e) = io::save_camera_bookmarks(&io::directory(""), bookmarks) {
        crate::log(&format!("<Photo Mode> {e}"));
    }
}

fn environment_page(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Environment") {
        return true;
    }
    title(ui, "Time of Day");
    let mut hour = environment::hour();
    if ui.add(egui::Slider::new(&mut hour, 0.0..=23.99).text("Hour").fixed_decimals(2)).changed() {
        manager::with(|m| m.set_hour(hour));
    }
    ui.columns(3, |columns| {
        for (i, (label, preset)) in [("Dawn", 5.5), ("Morning", 9.0), ("Noon", 13.0), ("Golden", 18.5), ("Dusk", 20.5), ("Night", 1.0)].into_iter().enumerate() {
            let column = &mut columns[i % 3];
            if column.add_sized([column.available_width(), 20.0], egui::Button::new(label)).clicked() {
                manager::with(|m| m.set_hour(preset));
            }
        }
    });
    title(ui, "Sun");
    let mut sun = environment::get_sun();
    let mut changed = ui.checkbox(&mut sun.enabled, "Override Sun Angle").on_hover_text("Sets the shadow direction directly instead of deriving it from the time of day").changed();
    if sun.enabled {
        if environment::sun_contested() {
            ui.colored_label(Color32::from_rgb(255, 210, 80), "Yielded - another mod is driving the sun.");
            ui.weak("Community Shaders' Sky Sync does this. Turn it off in the\nCommunity Shaders menu, then tick this again.");
        }
        changed |= ui.add(egui::Slider::new(&mut sun.azimuth, 0.0..=360.0).text("Azimuth").suffix(" deg").fixed_decimals(0)).on_hover_text("Compass bearing: 0 is north, increasing clockwise").changed();
        changed |= ui.add(egui::Slider::new(&mut sun.elevation, -20.0..=90.0).text("Elevation").suffix(" deg").fixed_decimals(0)).on_hover_text("Height above the horizon. Low angles give long shadows; below zero puts the sun under the horizon").changed();
        ui.weak("Volumetric fog and screen-space effects take a moment to\nsettle after a big change - they blend over several frames.");
    }
    if changed {
        environment::set_sun(sun);
    }
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    title(ui, "Weather");
    if render::is_interior() {
        ui.weak("Interior cell - weather has no effect here.");
    } else {
        let (current, forced) = (environment::current_weather(), manager::with(|s| s.forced_weather));
        if ui.add_enabled(forced != 0, egui::Button::new("Release Override")).clicked() {
            manager::with(Manager::release_weather);
        }
        ui.add(egui::TextEdit::singleline(&mut m.weather_search).hint_text("Search weathers...").desired_width(f32::INFINITY));
        let list = m.weathers.get_or_insert_with(environment::weathers);
        egui::ScrollArea::vertical().id_salt("WeatherList").max_height(180.0).show(ui, |ui| {
            for weather in list.iter().filter(|w| contains_no_case(&w.label, &m.weather_search)) {
                ui.horizontal(|ui| {
                    if ui.selectable_label(weather.form_id == current, &weather.label).clicked() {
                        manager::with(|s| s.force_weather(weather.form_id, false));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.weak(weather.kind.name()));
                });
            }
        });
    }
    title(ui, "Colour Grading");
    ui.weak("The game's own imagespace modifiers.");
    let (active, held) = manager::with(|s| (s.imod, s.imod_strength));
    let strength = m.imod_strength.get_or_insert(held);
    let response = ui.add_enabled(active != 0, egui::Slider::new(strength, 0.0..=1.0).text("Strength").fixed_decimals(2));
    if response.drag_stopped() || (response.changed() && !response.dragged()) {
        let s = *strength;
        manager::with(|m| m.set_imod(active, s));
    }
    if ui.add_enabled(active != 0, egui::Button::new("Clear Grading")).clicked() {
        manager::with(Manager::clear_imod);
    }
    ui.add(egui::TextEdit::singleline(&mut m.imod_search).hint_text("Search imagespace modifiers...").desired_width(f32::INFINITY));
    let strength = *strength;
    let list = m.imods.get_or_insert_with(environment::imods);
    egui::ScrollArea::vertical().id_salt("ImodList").max_height(300.0).show(ui, |ui| {
        for imod in list.iter().filter(|i| contains_no_case(&i.label, &m.imod_search)) {
            if ui.selectable_label(imod.form_id == active, &imod.label).clicked() {
                manager::with(|s| s.set_imod(imod.form_id, strength));
            }
        }
    });
    false
}

fn scenes(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Scenes") {
        return true;
    }
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    let dir = io::directory("Scenes");
    let files = m.scene_files.get_or_insert_with(|| io::list_files(&dir));
    ui.weak("A scene here is the camera framing, the hour, the forced weather and the sun override.");
    title(ui, "Save");
    let mut saved = false;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut m.scene_save_name).hint_text("Scene name...").desired_width(ui.available_width() * 0.55));
        if ui.button("Save Scene").clicked() {
            let name = io::sanitize_file_name(&if m.scene_save_name.is_empty() { format!("Scene {}", files.len() + 1) } else { m.scene_save_name.clone() });
            let sun = environment::get_sun();
            let forced = manager::with(|s| s.forced_weather);
            let scene = Scene {
                camera: Manager::read_camera_pose().bookmark(String::new()),
                hour: Some(environment::hour()),
                weather: Some(environment::form_key(forced)).filter(|k| forced != 0 && !k.is_empty()),
                sun: sun.enabled.then_some(SceneSun { azimuth: sun.azimuth, elevation: sun.elevation }),
            };
            match io::save_scene(&dir, &scene, &name) {
                Ok(path) => {
                    crate::log(&format!("<Photo Mode> Saved scene: {}", path.display()));
                    m.selected_scene = name;
                    saved = true;
                }
                Err(e) => crate::log(&format!("<Photo Mode> {e}")),
            }
        }
    });
    title(ui, "Load");
    if files.is_empty() {
        ui.weak("No saved scenes yet.");
    }
    let mut delete = None;
    for file in files.iter() {
        ui.horizontal(|ui| {
            if ui.small_button("x").clicked() {
                delete = Some(file.clone());
            }
            if ui.selectable_label(*file == m.selected_scene, file).clicked() {
                m.selected_scene.clone_from(file);
                let mut notes = Vec::new();
                match io::load_scene(&dir, file, &mut notes) {
                    Ok(scene) => apply_scene(&scene),
                    Err(e) => notes.push(e),
                }
                log_all(notes);
            }
        });
    }
    if let Some(file) = delete {
        let _ = std::fs::remove_file(dir.join(format!("{file}.json")));
        saved = true;
    }
    if saved {
        m.scene_files = None;
    }
    false
}

/// `ApplyScene`, for what a scene carries here: the hour and the weather when it sets them, the sun
/// only when it sets one, then the camera.
fn apply_scene(scene: &Scene) {
    manager::with(|m| {
        if let Some(hour) = scene.hour {
            m.set_hour(hour);
        }
        if let Some(key) = &scene.weather {
            m.force_weather(environment::resolve_form_key(key), false);
        }
    });
    if let Some(sun) = scene.sun {
        environment::set_sun(environment::SunOverride { enabled: true, azimuth: sun.azimuth, elevation: sun.elevation });
    }
    let pose = CameraPose::from(&scene.camera);
    manager::with(|m| m.request_camera_pose(pose, true, true, true, true));
}

fn idle_page(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Pose Packs") {
        return true;
    }
    let Some(target) = manager::with(|m| m.target()).filter(IsActor::cast_actor) else {
        ui.weak("Target an actor to play poses");
        return false;
    };
    if row(ui, "A-Pose", Some("static reference pose"), "") {
        unfreeze(&target);
        manager::with(|m| m.play_static_event(idles::A_POSE));
    }
    let mut guard = MENU.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = guard.get_or_insert_with(Menu::default);
    if row(ui, "Reset Idle", Some("back to normal behavior"), "") {
        unfreeze(&target);
        manager::with(|s| s.play_static_event(idles::RESET_IDLE));
        m.last_event.clear();
    }
    let packs = m.packs.get_or_insert_with(|| {
        let packs = idles::scan(&std::env::current_dir().unwrap_or_default().join("Data"));
        crate::log(&format!("<Photo Mode> Idle scan: {} packs, {} poses", packs.len(), packs.iter().map(|p| p.pose_count).sum::<usize>()));
        packs
    });
    if packs.is_empty() {
        ui.weak("No pose packs found (PoserHotkeys data or FNIS lists)");
        return false;
    }
    m.selected_pack = m.selected_pack.min(packs.len() - 1);
    title(ui, "Browse");
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("PosePacks").selected_text(format!("{} ({})", packs[m.selected_pack].name, packs[m.selected_pack].pose_count)).show_ui(ui, |ui| {
            for (i, p) in packs.iter().enumerate() {
                ui.selectable_value(&mut m.selected_pack, i, format!("{} ({})", p.name, p.pose_count));
            }
        });
        ui.add(egui::TextEdit::singleline(&mut m.idle_search).hint_text("Search poses...").desired_width(f32::INFINITY));
    });
    let pack = &packs[m.selected_pack];
    egui::ScrollArea::vertical().id_salt("PoseList").show(ui, |ui| {
        for set in &pack.sets {
            if pack.sets.len() > 1 {
                ui.label(egui::RichText::new(&set.name).strong());
            }
            for event in set.events.iter().filter(|e| contains_no_case(e, &m.idle_search)) {
                if ui.selectable_label(*event == m.last_event, event).clicked() {
                    unfreeze(&target);
                    manager::with(|s| s.play_event(event));
                    m.last_event.clone_from(event);
                }
            }
        }
    });
    false
}

fn options(ui: &mut egui::Ui) -> bool {
    if page_header(ui, "Options") {
        return true;
    }
    let before = crate::settings::profile().Studio.bHideHUD;
    super::edit(ui, |ui, p| {
        let s = &mut p.Studio;
        rows(ui, "StudioTableOptions", |ui| {
            toggle(ui, "Revert Edits on Exit", &mut s.bRevertOnExit, "Hand pose edits back to the animation when leaving the free camera. The sun and the colour grading are always restored.");
            slider(ui, "UI Scale", &mut s.fUIScale, 0.8, 1.8, 2, "Text/row size of the Photo Mode menu (stored; the overlay keeps its own size)");
            toggle(ui, "Hide HUD", &mut s.bHideHUD, "Hides the game HUD for the whole session. Off by default because it also hides the console, which is how targets are picked - a capture hides it for the shot either way.");
            slider(ui, "Bracket Delay", &mut s.fBracketDelay, 0.0, 3.0, 2, "Settling time between shots when capturing every bookmark");
        });
        let keys = crate::PHOTO_MODE.get();
        rows(ui, "StudioKeys", |ui| {
            super::widgets::key(ui, "UI/Fly Toggle Key", keys.map(|k| &k[0]));
            super::widgets::key(ui, "Screenshot Key", keys.map(|k| &k[1]));
        });
        section_header(ui, "Composition Guides");
        rows(ui, "StudioTableGuides", |ui| {
            combo(ui, "Grid", &super::studio_guides::GRID_NAMES, &mut s.iGuideGrid, "Divides the framed area");
            combo(ui, "Aspect", &ASPECT_NAMES, &mut s.iGuideAspect, "Dims everything outside the target aspect so you frame for the crop you will actually post");
            toggle(ui, "Centre Mark", &mut s.bGuideCenter, "");
            toggle(ui, "Title Safe Area", &mut s.bGuideSafeArea, "90% box - the margin a crop or a border is most likely to eat");
            toggle(ui, "Level Indicator", &mut s.bGuideHorizon, "Artificial horizon showing the camera roll; turns gold when level");
        });
    });
    if crate::settings::profile().Studio.bHideHUD != before {
        manager::with(Manager::apply_hud_preference);
    }
    false
}

trait IsActor {
    fn cast_actor(&self) -> bool;
}

impl IsActor for skyrim::actors::RefPtr {
    fn cast_actor(&self) -> bool {
        skyrim::forms::TESFormExt::cast::<sre::Actor>(&***self).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{bone_shown, contains_no_case, stem};

    #[test]
    fn bones_and_searches_filter_as_ic_s_menu() {
        assert!(bone_shown("NPC Head [Head]", true, false, true));
        assert!(!bone_shown("NPC Head [Head]", false, false, true), "a bone nothing skins to");
        assert!(!bone_shown("CME Body [Body]", true, false, true));
        assert!(bone_shown("CME Body [Body]", false, true, true) && bone_shown("Tail1", true, false, false));
        assert!(contains_no_case("NPC Head [Head]", "head") && contains_no_case("x", "") && !contains_no_case("Spine", "neck"));
        assert_eq!(stem("Sub/kneel.yaml"), "Sub/kneel");
        assert_eq!(stem("plain"), "plain");
    }
}
