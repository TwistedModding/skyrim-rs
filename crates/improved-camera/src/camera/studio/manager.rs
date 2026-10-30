//! `camera/studio/StudioManager`: the photo mode session. It opens only on request (the menu or
//! `ic pm`), through the game's free camera, and closes with it; while it is open the camera pins
//! onto the target as an orbit (rotate, pan and dolly from the mouse outside the menu), manual
//! framing and bookmarks are applied on the game tick, and the target's bone markers are kept for
//! the HUD painter. Pose layers apply every frame in every camera state (outside the studio only the
//! persistent ones, unless edits are kept on exit).

use std::sync::Mutex;
use std::time::Duration;

use skyrim::actors::{PlayerCharacterExt, RefHandle, TESObjectREFRExt};
use skyrim::camera::{PlayerCameraExt, TESCameraStateExt};
use skyrim::forms::TESFormExt;
use skyrim::render::{NiAVObjectExt, NiNodeExt, Transform};
use sre::{Actor, CameraState, FreeCameraState, InputContextEnum, NiAVObject, NiNode, TESObjectREFR};

use super::adjustments::{PoseEngine, third_person_3d};
use super::capture::{self, Capture, Step};
use super::environment;
use super::io::{self, CameraBookmark};
use crate::settings;
use crate::util::point::{self, P3};
use crate::util::skyrim_util as util;

const ORBIT_ROTATE_SENSITIVITY: f32 = 0.008;
const ORBIT_ZOOM_STEP: f32 = 1.15;
/// The free camera's fly speed limits (`FREE_CAMERA_SPEED_MIN`/`MAX`).
const SPEED_MIN: f32 = 1.0;
const SPEED_MAX: f32 = 200.0;

/// `OrbitPanStep`: the world step per pixel, scaled with the pivot distance.
fn orbit_pan_step(distance: f32) -> f32 {
    distance.max(30.0) * 0.0015
}

/// `OrbitRightFromYaw`: the screen-horizontal drag axis for a look yaw.
fn orbit_right(look_yaw: f32) -> P3 {
    [look_yaw.cos(), -look_yaw.sin(), 0.0]
}

/// `DragSpeedFactor`: drags follow the fly speed, 1 at the vanilla 30.
fn drag_speed_factor() -> f32 {
    *util::free_camera_speed() / 30.0
}

/// A camera framing (`CameraPose`): pitch and yaw in radians, roll in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub position: P3,
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
    pub fov: f32,
}

impl Default for CameraPose {
    fn default() -> Self {
        Self { position: [0.0; 3], pitch: 0.0, yaw: 0.0, roll: 0.0, fov: 80.0 }
    }
}

impl From<&CameraBookmark> for CameraPose {
    fn from(b: &CameraBookmark) -> Self {
        Self { position: b.position, pitch: b.pitch, yaw: b.yaw, roll: b.roll, fov: b.fov }
    }
}

impl CameraPose {
    #[must_use]
    pub fn bookmark(&self, name: String) -> CameraBookmark {
        CameraBookmark { name, position: self.position, pitch: self.pitch, yaw: self.yaw, roll: self.roll, fov: self.fov }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Orbit {
    valid: bool,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// World height locked at engage: a falling target drops out of frame, not the camera.
    anchor_z: f32,
    pan: P3,
    pivot_target: u32,
}

/// A degenerate engage (inside the head) frames from the front at portrait distance; otherwise the
/// camera's placement around the anchor becomes the orbit's yaw, pitch and distance.
fn engage(offset: P3, target_heading: f32) -> (f32, f32, f32) {
    let horizontal = offset[0].hypot(offset[1]);
    let distance = point::length(offset);
    if distance < 100.0 || horizontal < 25.0 { (target_heading, 0.05, 160.0) } else { (offset[0].atan2(offset[1]), (offset[2] / distance).clamp(-1.0, 1.0).asin(), distance) }
}

/// Where the orbit puts the camera and its pitch and yaw looking back at the pivot.
fn orbit_place(pivot: P3, yaw: f32, pitch: f32, distance: f32) -> (P3, f32, f32) {
    let direction = [yaw.sin() * pitch.cos(), yaw.cos() * pitch.cos(), pitch.sin()];
    (point::add(pivot, point::scale(direction, distance)), pitch, (yaw + core::f32::consts::PI).rem_euclid(core::f32::consts::TAU))
}

/// The look direction of a free camera pitch and yaw (positive pitch looks down).
fn look(pitch: f32, yaw: f32) -> P3 {
    [yaw.sin() * pitch.cos(), yaw.cos() * pitch.cos(), -pitch.sin()]
}

#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct CameraIntent {
    position: bool,
    rotation: bool,
    roll: bool,
    fov: bool,
    /// Wheel notches along the look direction.
    dolly: f32,
    pose: CameraPose,
}

/// A bone of the target for the markers and mouse posing (`BonePoint`).
#[derive(Clone, Debug)]
pub struct BonePoint {
    pub name: String,
    pub world: P3,
    pub rotate: [[f32; 3]; 3],
    pub parent_rotate: [[f32; 3]; 3],
    pub parent_scale: f32,
}

/// What waits for the frame callback (IC's game-thread tasks).
#[derive(Clone, Debug)]
enum Task {
    Enter,
    Exit,
    Event(u32, &'static str),
    OwnedEvent(u32, String),
    Imod { previous: u32, next: u32, strength: f32 },
}

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Manager {
    pub active: bool,
    hid_hud: bool,
    explicit_open: bool,
    /// The target's reference handle.
    target: u32,
    last_console_selection: u32,
    pub pose: PoseEngine,
    pub capture: Capture,
    pub imod: u32,
    pub imod_strength: f32,
    /// The camera pins onto the target (the Camera page's switch).
    pub orbit_enabled: bool,
    orbit: Orbit,
    intent: CameraIntent,
    pub bones: Vec<BonePoint>,
    tasks: Vec<Task>,
}

static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Manager) -> R) -> R {
    f(MANAGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(|| Manager { orbit_enabled: true, imod_strength: 1.0, ..Manager::default() }))
}

fn free_state() -> Option<&'static FreeCameraState> {
    util::camera()?.state_object(CameraState::Free).and_then(TESCameraStateExt::as_free)
}

/// `PeekConsoleRef`: the console's selected reference.
#[must_use]
pub fn console_ref() -> Option<skyrim::actors::RefPtr> {
    RefHandle(unsafe { *sre::g_console_selected_ref().cast::<u32>() }).lookup()
}

/// `PeekCrosshairRef`: the crosshair's picked reference, else the actor the pick hit (it often
/// survives in the free camera when the target goes stale).
#[must_use]
pub fn crosshair_ref() -> Option<skyrim::actors::RefPtr> {
    let pick = sre::CrosshairPickData::singleton()?;
    RefHandle(pick.targetHandle).lookup().or_else(|| RefHandle(pick.actorHandle).lookup())
}

/// A reference's name for the menu: the base form's full name, its editor id, or its form id.
#[must_use]
pub fn display_name(reference: Option<&TESObjectREFR>) -> String {
    let Some(reference) = reference else { return "<none>".to_owned() };
    if reference.cast::<sre::PlayerCharacter>().is_some() {
        return "Player".to_owned();
    }
    let base = reference.base_object();
    base.and_then(|b| b.cast::<sre::TESNPC>())
        .and_then(|npc| skyrim::actors::TESFullNameExt::full_name(npc.as_tes_full_name()))
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| base.and_then(TESFormExt::editor_id).map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("{:08X}", reference.formID))
}

/// `CollectBonePoints`: every named node under the root, with the frames mouse posing maps through.
fn collect_bones(object: &NiAVObject, depth: u32, out: &mut Vec<BonePoint>) {
    let Some(node) = object.cast::<NiNode>() else { return };
    if depth > 0
        && let Some(name) = skyrim::BSFixedString::view(&object.name).to_str().filter(|n| !n.is_empty())
    {
        let world: Transform = object.world_transform();
        let parent = unsafe { object.parent.cast::<NiAVObject>().as_ref() }.map(NiAVObjectExt::world_transform);
        out.push(BonePoint { name: name.to_owned(), world: world.translation, rotate: world.rotation, parent_rotate: parent.map_or(point::IDENTITY, |p| p.rotation), parent_scale: parent.map_or(1.0, |p| p.scale) });
    }
    for child in node.children().iter().flatten() {
        collect_bones(child, depth + 1, out);
    }
}

impl Manager {
    #[must_use]
    pub fn target(&self) -> Option<skyrim::actors::RefPtr> {
        RefHandle(self.target).lookup()
    }

    pub fn set_target(&mut self, reference: Option<&TESObjectREFR>) {
        self.target = reference.map_or(0, |r| r.handle().0);
        crate::log(&format!("<Photo Mode> SetTarget: {} ({:08X})", display_name(reference), reference.map_or(0, |r| r.formID)));
    }

    pub fn target_player(&mut self) {
        let player: Option<&TESObjectREFR> = util::player().map(|p| -> &TESObjectREFR { p });
        self.set_target(player);
    }

    /// `TargetConsoleRef`.
    pub fn target_console_ref(&mut self) -> bool {
        let Some(r) = console_ref() else { return false };
        self.set_target(Some(&r));
        true
    }

    /// `TargetCrosshairRef`.
    pub fn target_crosshair_ref(&mut self) -> bool {
        let Some(r) = crosshair_ref() else { return false };
        self.set_target(Some(&r));
        true
    }

    /// `PollConsoleSelection`: a new console selection becomes the target the moment it appears.
    pub fn poll_console_selection(&mut self) {
        let selected = console_ref();
        let id = selected.as_ref().map_or(0, |r| r.formID);
        if id != self.last_console_selection {
            self.last_console_selection = id;
            if let Some(r) = selected {
                self.set_target(Some(&r));
            }
        }
    }

    /// `SelectDefaultTarget`: the console's selection, else the previous target, else the player.
    fn select_default_target(&mut self) {
        if !self.target_console_ref() && self.target().is_none() {
            self.target_player();
        }
    }

    /// `EnterStudio`: arms the entrance and toggles the free camera on the next frame.
    pub fn enter(&mut self) {
        self.explicit_open = true;
        self.tasks.push(Task::Enter);
    }

    /// `ExitStudio`.
    pub fn exit(&mut self) {
        self.tasks.push(Task::Exit);
    }

    /// `OnFreeCameraEnter`: only an armed entrance opens the studio; a bare `tfc` stays vanilla.
    fn on_free_camera_enter(&mut self) {
        if self.active || !core::mem::take(&mut self.explicit_open) || util::is_menu_open("Loading Menu") {
            return;
        }
        self.active = true;
        self.last_console_selection = console_ref().map_or(0, |r| r.formID);
        self.select_default_target();
        if let Some(player) = util::player()
            && let Some(third) = player.third_person_root()
        {
            third.set_hidden(false);
            crate::body::manager::with(|m| m.parts.restore(player));
            third.update_transforms();
        }
        self.apply_hud_preference();
        if let Some(host) = crate::HOST.get() {
            host.show_section(crate::ui::studio_menu::SECTION);
        }
    }

    /// `OnFreeCameraExit`: world state comes down whatever the preference; edits go back unless
    /// the profile keeps them.
    fn on_free_camera_exit(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let steps = self.capture.cancel();
        self.run_capture_steps(steps);
        self.apply_hud_preference();
        environment::release_sun();
        self.clear_imod();
        if settings::profile().Studio.bRevertOnExit {
            for adjustments in self.pose.actors.values_mut() {
                adjustments.revert_all();
            }
        }
    }

    /// `OnSessionReset`: a new game or a loaded save drops everything keyed to the old world.
    pub fn on_session_reset(&mut self) {
        let steps = self.capture.cancel();
        self.run_capture_steps(steps);
        environment::release_sun();
        *self = Manager { orbit_enabled: self.orbit_enabled, imod_strength: 1.0, ..Manager::default() };
        crate::log("<Photo Mode> Session state cleared (game load)");
    }

    /// `ApplyHUDPreference`: the HUD hidden for the session when the profile says so; only a HUD
    /// the studio took down is put back.
    pub fn apply_hud_preference(&mut self) {
        let hide = self.active && settings::profile().Studio.bHideHUD;
        if hide && util::menus_visible() {
            util::show_menus(false);
            self.hid_hud = true;
        } else if !hide && self.hid_hud {
            util::show_menus(true);
            self.hid_hud = false;
        }
    }

    /// `TogglePause`: the main loop's world freeze (`Main`+0x16, as `tfc 1` sets it).
    pub fn toggle_pause() {
        if let Some(main) = unsafe { (*sre::g_main()).cast::<sre::Main>().as_ref() } {
            skyrim::store(&main.field_16, u8::from(main.field_16 == 0));
        }
    }

    #[must_use]
    pub fn paused() -> bool {
        unsafe { (*sre::g_main()).cast::<sre::Main>().as_ref() }.is_some_and(|m| m.field_16 != 0)
    }

    /// `ReadCameraPose`.
    #[must_use]
    pub fn read_camera_pose() -> CameraPose {
        let mut pose = CameraPose::default();
        if let Some(free) = free_state() {
            pose.position = [free.position.x, free.position.y, free.position.z];
            pose.pitch = free.pitch;
            pose.yaw = free.yaw;
        }
        if let Some(camera) = util::camera() {
            pose.fov = camera.worldFOV;
        }
        pose.roll = crate::camera::manager::with(|m| m.state.free_mut().map_or(0.0, |f| f.roll()));
        pose
    }

    /// `RequestCameraPose`: applied on the game tick; moving or turning releases the pin.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn request_camera_pose(&mut self, pose: CameraPose, position: bool, rotation: bool, roll: bool, fov: bool) {
        let i = &mut self.intent;
        i.pose = pose;
        i.position |= position;
        i.rotation |= rotation;
        i.roll |= roll;
        i.fov |= fov;
        if position || rotation {
            self.orbit_enabled = false;
        }
    }

    /// `OrbitRotate`, mouse pixels.
    pub fn orbit_rotate(&mut self, dx: f32, dy: f32) {
        let sensitivity = ORBIT_ROTATE_SENSITIVITY * drag_speed_factor();
        self.orbit.yaw = (self.orbit.yaw + dx * sensitivity + core::f32::consts::PI).rem_euclid(core::f32::consts::TAU) - core::f32::consts::PI;
        self.orbit.pitch = (self.orbit.pitch + dy * sensitivity).clamp(-1.45, 1.45);
    }

    /// `OrbitPan`, mouse pixels.
    pub fn orbit_pan(&mut self, dx: f32, dy: f32) {
        let step = orbit_pan_step(self.orbit.distance) * drag_speed_factor();
        let right = orbit_right(self.orbit.yaw + core::f32::consts::PI);
        self.orbit.pan = point::add(self.orbit.pan, point::scale(right, -dx * step));
        self.orbit.pan[2] += dy * step;
    }

    /// `OrbitZoom`: a multiplicative dolly while pinned, a step along the look direction when free.
    pub fn orbit_zoom(&mut self, steps: f32) {
        if self.orbit_enabled && self.orbit.valid {
            let factor = if steps > 0.0 { 1.0 / ORBIT_ZOOM_STEP } else { ORBIT_ZOOM_STEP };
            self.orbit.distance = (self.orbit.distance * factor).clamp(20.0, 4096.0);
        } else {
            self.intent.dolly += steps;
        }
    }

    /// `AdjustFlySpeed`.
    pub fn adjust_fly_speed(steps: f32) {
        let speed = util::free_camera_speed();
        let factor = if steps > 0.0 { ORBIT_ZOOM_STEP } else { 1.0 / ORBIT_ZOOM_STEP };
        skyrim::store(speed, (*speed * factor).clamp(SPEED_MIN, SPEED_MAX));
    }

    /// `SetImod`: the modifier held at `strength`, restarted on the next frame.
    pub fn set_imod(&mut self, form_id: u32, strength: f32) {
        let previous = self.imod;
        self.imod = form_id;
        self.imod_strength = strength.clamp(0.0, 1.0);
        self.tasks.push(Task::Imod { previous, next: form_id, strength: self.imod_strength });
    }

    /// `ClearImod`.
    pub fn clear_imod(&mut self) {
        let previous = core::mem::take(&mut self.imod);
        if previous != 0 {
            self.tasks.push(Task::Imod { previous, next: 0, strength: 0.0 });
        }
    }

    /// `IdleBrowser::PlayEvent`: the event sent into the target's graph on the next frame.
    pub fn play_event(&mut self, event: &str) {
        self.tasks.push(Task::OwnedEvent(self.target, event.to_owned()));
    }

    /// `PlayAPose` and `ResetIdle`.
    pub fn play_static_event(&mut self, event: &'static str) {
        self.tasks.push(Task::Event(self.target, event));
    }

    /// A shot of the current framing.
    pub fn request_shot(&mut self) {
        let steps = self.capture.request_shot();
        self.run_capture_steps(steps);
    }

    /// One shot per bookmark.
    pub fn request_bracket(&mut self, shots: Vec<CameraBookmark>) {
        let steps = self.capture.request_bracket(shots);
        self.run_capture_steps(steps);
    }

    fn run_capture_steps(&mut self, steps: Vec<Step>) {
        for step in steps {
            match step {
                Step::HideHud => {
                    if util::menus_visible() {
                        util::show_menus(false);
                        self.capture.hid_hud = true;
                    }
                }
                Step::Fly(shot) => self.request_camera_pose(CameraPose::from(&shot), true, true, true, true),
                Step::Grab(path) => self.grab(&path),
                Step::Finish => {
                    if core::mem::take(&mut self.capture.hid_hud) {
                        util::show_menus(true);
                        self.apply_hud_preference();
                    }
                }
            }
        }
    }

    /// `WriteFrame`: the host's frame, cropped to the guide's aspect when the profile says so.
    fn grab(&mut self, path: &std::path::Path) {
        let Some(host) = crate::HOST.get() else { return };
        let studio = settings::profile().Studio;
        let written = host.read_frame().and_then(|frame| {
            let frame = if studio.bCaptureCropToGuide {
                let (left, top, width, height) = capture::crop_rect(frame.width, frame.height, capture::aspect_ratio(studio.iGuideAspect));
                frame.crop(left, top, width, height)
            } else {
                frame
            };
            frame.write_png(path).map_err(|e| e.to_string())
        });
        match written {
            Ok(()) => {
                self.capture.last_shot = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                crate::log(&format!("<Photo Mode> Wrote {}", path.display()));
            }
            Err(e) => crate::log(&format!("<Photo Mode> Capture failed: {e}")),
        }
    }

    /// `UpdateOrbitCamera`: the manual framing first, then the pin.
    fn update_orbit_camera(&mut self) {
        let Some(free) = free_state() else { return };
        let intent = core::mem::take(&mut self.intent);
        if intent.position {
            let p = intent.pose.position;
            for (field, value) in [(&free.position.x, p[0]), (&free.position.y, p[1]), (&free.position.z, p[2])] {
                skyrim::store(field, value);
            }
        }
        if intent.rotation {
            skyrim::store(&free.pitch, intent.pose.pitch);
            skyrim::store(&free.yaw, intent.pose.yaw);
        }
        if intent.roll {
            crate::camera::manager::with(|m| m.state.free_mut().map(|f| f.set_roll(intent.pose.roll)));
        }
        if intent.fov
            && let Some(camera) = util::camera()
        {
            skyrim::store(&camera.worldFOV, intent.pose.fov);
        }
        if intent.dolly != 0.0 {
            let step = util::free_camera_speed().max(2.0) * 2.0;
            let p = point::add([free.position.x, free.position.y, free.position.z], point::scale(look(free.pitch, free.yaw), intent.dolly * step));
            for (field, value) in [(&free.position.x, p[0]), (&free.position.y, p[1]), (&free.position.z, p[2])] {
                skyrim::store(field, value);
            }
        }
        let overlay_open = crate::ui::studio_menu::input_captured();
        if !overlay_open || !self.orbit_enabled {
            self.orbit.valid = false;
            return;
        }
        if util::game_is_paused() {
            return;
        }
        let Some(target) = self.target() else {
            self.orbit.valid = false;
            return;
        };
        let Some(root) = third_person_3d(&target) else {
            self.orbit.valid = false;
            return;
        };
        let mut anchor = util::position(&target);
        if target.cast::<Actor>().is_some() {
            anchor[2] += root.worldBound.radius * 2.0 * 0.65;
        }
        let camera = [free.position.x, free.position.y, free.position.z];
        if !self.orbit.valid || self.orbit.pivot_target != target.formID {
            let (yaw, pitch, distance) = engage(point::sub(camera, anchor), target.rotZ);
            self.orbit = Orbit { valid: true, yaw, pitch, distance, anchor_z: anchor[2], pan: [0.0; 3], pivot_target: target.formID };
        }
        anchor[2] = self.orbit.anchor_z;
        let (position, pitch, yaw) = orbit_place(point::add(anchor, self.orbit.pan), self.orbit.yaw, self.orbit.pitch, self.orbit.distance);
        for (field, value) in [(&free.position.x, position[0]), (&free.position.y, position[1]), (&free.position.z, position[2])] {
            skyrim::store(field, value);
        }
        skyrim::store(&free.pitch, pitch);
        skyrim::store(&free.yaw, yaw);
    }

    /// `RefreshSnapshots`: the target's bones every tick, for the markers and the bone list.
    fn refresh_bones(&mut self) {
        self.bones.clear();
        if let Some(root) = self.target().filter(|t| t.cast::<Actor>().is_some()).and_then(|t| third_person_3d(&t)) {
            collect_bones(root, 0, &mut self.bones);
        }
    }
}

/// `OnGlobalUpdate`, once a frame after the camera update, in every camera state.
pub fn on_global_update() {
    if util::is_menu_open("Loading Menu") {
        return;
    }
    with(|m| {
        if m.active {
            m.update_orbit_camera();
            m.refresh_bones();
        }
        let apply_all = m.active || !settings::profile().Studio.bRevertOnExit;
        m.pose.on_game_update(apply_all);
    });
}

/// For an unload: the bones handed back to the animation, the sun, the colour grading and the HUD
/// put back; a reloaded build starts a fresh session.
pub fn release() {
    with(|m| {
        for adjustments in m.pose.actors.values_mut() {
            adjustments.revert_all();
        }
        environment::release_sun();
        environment::restart_imod(core::mem::take(&mut m.imod), 0, 0.0);
        if m.hid_hud || m.capture.hid_hud {
            util::show_menus(true);
        }
        *m = Manager { orbit_enabled: true, imod_strength: 1.0, ..Manager::default() };
    });
}

/// From `FreeState::OnExit`.
pub fn on_free_camera_exit() {
    with(Manager::on_free_camera_exit);
}

fn notify(handle: u32, event: &str) {
    if let Some(r) = RefHandle(handle).lookup()
        && let Some(actor) = r.cast::<Actor>()
    {
        let sent = actor.notify_animation_graph(skyrim::BSFixedString::new(event).raw());
        crate::log(&format!("<Photo Mode> Anim event '{event}' on {:08X}: {}", r.formID, if sent { "ok" } else { "rejected" }));
    }
}

/// From the frame callback: the queued camera toggles, graph events and modifiers, then the capture.
pub fn on_frame() {
    let tasks = with(|m| core::mem::take(&mut m.tasks));
    for task in tasks {
        match task {
            Task::Enter => enter_free_camera(),
            Task::Exit => {
                if util::is_free_camera()
                    && let Some(camera) = util::camera()
                {
                    camera.toggle_free_camera(false);
                }
            }
            Task::Event(handle, event) => notify(handle, event),
            Task::OwnedEvent(handle, event) => notify(handle, &event),
            Task::Imod { previous, next, strength } => environment::restart_imod(previous, next, strength),
        }
    }
    let delay = Duration::from_secs_f32(settings::profile().Studio.fBracketDelay.clamp(0.0, 5.0));
    with(|m| {
        if m.capture.is_busy() {
            let steps = m.capture.on_frame(delay, &io::directory("Shots"));
            m.run_capture_steps(steps);
        }
    });
}

/// `EnterStudio`'s task: in the free camera already, the studio opens at once; otherwise the free
/// camera is toggled with the console `tfc`'s input context, and a refusal disarms the entrance.
fn enter_free_camera() {
    let Some(camera) = util::camera() else { return };
    if !util::is_free_camera() {
        camera.toggle_free_camera(false);
        if !util::is_free_camera() {
            with(|m| m.explicit_open = false);
            return;
        }
        if let Some(map) = sre::EventSourceHolder_User::singleton() {
            unsafe { sre::control_map__push_input_context(core::ptr::from_ref(map).cast_mut().cast(), InputContextEnum::FreeCamera) };
        }
    }
    with(Manager::on_free_camera_enter);
}

#[cfg(test)]
mod tests {
    use super::{engage, look, orbit_place, orbit_right};

    #[test]
    fn the_orbit_engages_where_the_camera_is_and_looks_back_at_the_pivot() {
        let (yaw, pitch, distance) = engage([0.0, 200.0, 0.0], 1.0);
        assert!(yaw.abs() < 1e-6 && pitch.abs() < 1e-6 && (distance - 200.0).abs() < 1e-4);
        assert_eq!(engage([0.0, 10.0, 50.0], 1.0), (1.0, 0.05, 160.0), "inside the head");
        let (position, pitch, yaw) = orbit_place([10.0, 20.0, 30.0], 0.3, 0.2, 150.0);
        let back = point_from(position, look(pitch, yaw), 150.0);
        assert!((0..3).all(|i| (back[i] - [10.0, 20.0, 30.0][i]).abs() < 1e-3), "{back:?}");
        let (y, p, d) = engage(crate::util::point::sub(position, [10.0, 20.0, 30.0]), 0.0);
        assert!((y - 0.3).abs() < 1e-4 && (p - 0.2).abs() < 1e-4 && (d - 150.0).abs() < 1e-3);
        let right = orbit_right(0.0);
        assert_eq!(right, [1.0, -0.0, 0.0]);
    }

    fn point_from(p: [f32; 3], dir: [f32; 3], d: f32) -> [f32; 3] {
        [p[0] + dir[0] * d, p[1] + dir[1] * d, p[2] + dir[2] * d]
    }
}
