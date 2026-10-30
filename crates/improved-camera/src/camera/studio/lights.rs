//! `camera/studio/StudioLights`: the light rig. Real light references placed at the target (no
//! plugin of the port's own: the base forms are the load order's lights without a model that cannot
//! be carried), following it every tick, spotlights aimed at its chest; colour, intensity and
//! radius go to each reference's point light, so the base form is never changed. Placed lights near
//! the target can be adopted, edited and handed back. Placing and removing references waits for
//! the frame callback (IC's task slot); everything else runs on the game tick.

use std::sync::OnceLock;

use skyrim::actors::{RefHandle, TESObjectREFRExt};
use skyrim::forms::{TESDataHandlerExt, TESFormExt};
use skyrim::physics;
use skyrim::render::{LightValues, NiAVObjectExt, NiPointLightExt, TESObjectLIGHExt, point_light};
use sre::{FormType, NiPointLight, TESObjectLIGH, TESObjectLIGH_flags as Flags, TESObjectREFR};

use super::adjustments::third_person_3d;
use crate::settings;
use crate::util::point::{self, P3};
use crate::util::skyrim_util as util;

/// What the engine light is. Spot and shadow are flags of the base form, so a kind is had only by
/// placing a form that carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Point,
    Spot,
}

/// Animated intensity, also a flag of the base form. IC's slow variants are bits the engine does
/// not slow anything with: 0x40 alone does not flicker, 0x100 pulses as 0x80 does from the same
/// speed field (RE `camera.md` IC9 H), so they name forms carrying those bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Animation {
    #[default]
    None,
    Flicker,
    FlickerSlow,
    Pulse,
    PulseSlow,
}

/// The photographic role: it only seeds the placement and colour, every value stays editable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Custom,
    Key,
    Fill,
    Rim,
    Kicker,
    Background,
}

/// What the light is anchored to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Attach {
    /// Orbits the subject.
    #[default]
    Target,
    /// Rides the camera, like an on-camera flash.
    Camera,
    /// Follows a named node of the subject.
    Bone,
    /// Stays where it was placed.
    World,
}

impl Kind {
    pub const ALL: [Self; 2] = [Self::Point, Self::Spot];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::Spot => "Spot",
        }
    }
}

impl Animation {
    pub const ALL: [Self; 5] = [Self::None, Self::Flicker, Self::FlickerSlow, Self::Pulse, Self::PulseSlow];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "Steady",
            Self::Flicker => "Flicker",
            Self::FlickerSlow => "Flicker (variant bit)",
            Self::Pulse => "Pulse",
            Self::PulseSlow => "Pulse (variant bit)",
        }
    }
}

impl Role {
    pub const ALL: [Self; 6] = [Self::Custom, Self::Key, Self::Fill, Self::Rim, Self::Kicker, Self::Background];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Key => "Key",
            Self::Fill => "Fill",
            Self::Rim => "Rim",
            Self::Kicker => "Kicker",
            Self::Background => "Background",
        }
    }
}

impl Attach {
    pub const ALL: [Self; 4] = [Self::Target, Self::Camera, Self::Bone, Self::World];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Target => "Target",
            Self::Camera => "Camera",
            Self::Bone => "Bone",
            Self::World => "World",
        }
    }
}

/// An enum's place in its `ALL` table, as IC's files store it.
pub fn index_of<T: PartialEq>(all: &[T], value: &T) -> u32 {
    all.iter().position(|v| v == value).and_then(|i| u32::try_from(i).ok()).unwrap_or(0)
}

/// `TESObjectLIGH` flags as the engine reads them (RE `camera.md` IC8 E, IC9 H). The engine's spot
/// is the spot shadow 0x400, a cone of the form's angle (+0xf4); nothing reads 0x200, `sre`'s
/// `SPOT` row.
const SPOT: u32 = Flags::SHADOW_SPOT.bits();
const SHADOW: u32 = Flags::SHADOW_SPOT.bits() | Flags::SHADOW_HEMISPHERE.bits() | Flags::SHADOW_OMNI.bits();
/// What animates a light: flicker 0x8, pulse 0x80 or 0x100 (`UpdateLightFlicker`). The engine's
/// inline 0x1c8 test, which also counts 0x40, is cell bookkeeping.
const ANIMATED: u32 = Flags::FLICKER.bits() | Flags::PULSE.bits() | Flags::PULSE_VARIANT.bits();

/// The form is the single truth for whether a light aims: a negative spot still is one.
#[must_use]
pub const fn is_spot(flags: u32) -> bool {
    flags & SPOT != 0
}

/// Vanilla has no shadowless spotlights, so asking for a plain spot still lands on a shadow caster;
/// the checkbox reports what is installed.
#[must_use]
pub const fn casts_shadows(flags: u32) -> bool {
    flags & SHADOW != 0
}

#[must_use]
pub const fn kind_of(flags: u32) -> Kind {
    if is_spot(flags) { Kind::Spot } else { Kind::Point }
}

/// `ScoreForm`: 4 for the shadow state, 2 for the animation; getting the shadow flag wrong is the
/// expensive one, since a shadow caster competes for the engine's shadow-light budget. 6 is an
/// exact match.
#[must_use]
pub const fn score(flags: u32, shadows: bool, animation: Animation) -> i32 {
    let shadow = if casts_shadows(flags) == shadows { 4 } else { 0 };
    let bits = match animation {
        Animation::None => return shadow + if flags & ANIMATED == 0 { 2 } else { 0 },
        Animation::Flicker => Flags::FLICKER.bits(),
        Animation::FlickerSlow => Flags::FLICKER.bits() | Flags::FLICKER_VARIANT.bits(),
        Animation::Pulse => Flags::PULSE.bits(),
        Animation::PulseSlow => Flags::PULSE_VARIANT.bits(),
    };
    shadow + if flags & bits == bits { 2 } else { 0 }
}

/// A loaded light form, as the rig and the form browser see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormInfo {
    pub form_id: u32,
    pub flags: u32,
    pub has_model: bool,
    /// The form's plugin, `Dynamic` without one.
    pub plugin: String,
    /// The full name and form id, or the form id alone.
    pub label: String,
}

impl FormInfo {
    /// A pure emitter: no model (a candle would come with it), not carried (torches, lanterns).
    #[must_use]
    pub const fn usable(&self) -> bool {
        !self.has_model && self.flags & Flags::CAN_CARRY.bits() == 0
    }
}

/// Every loaded light form, read once: the load order does not change inside a session and the
/// page asks every frame.
pub fn all_forms() -> &'static [FormInfo] {
    static FORMS: OnceLock<Vec<FormInfo>> = OnceLock::new();
    FORMS.get_or_init(|| {
        let Some(data) = sre::TESDataHandler::singleton() else { return Vec::new() };
        data.forms_of_type(FormType::LIGH)
            .iter()
            .flatten()
            .filter_map(|f| f.cast::<TESObjectLIGH>())
            .map(|light| FormInfo {
                form_id: light.formID,
                flags: light.flags,
                has_model: skyrim::BSFixedString::view(&light.model).to_str().is_some_and(|m| !m.is_empty()),
                plugin: light.get_file(0).map_or_else(|| "Dynamic".to_owned(), |f| skyrim::forms::TESFileExt::name(f).to_owned()),
                label: light.full_name().map_or_else(|| format!("{:08X}", light.formID), |n| format!("{} ({:08X})", n.to_string_lossy(), light.formID)),
            })
            .collect()
    })
}

/// `PickForm`: the usable form of `kind` scoring best for the shadow and animation asked for; the
/// first of equal scores.
#[must_use]
pub fn pick_form(forms: &[FormInfo], kind: Kind, shadows: bool, animation: Animation) -> Option<&FormInfo> {
    let mut best: Option<(&FormInfo, i32)> = None;
    for form in forms.iter().filter(|f| f.usable() && kind_of(f.flags) == kind) {
        let s = score(form.flags, shadows, animation);
        if best.is_none_or(|(_, b)| s > b) {
            best = Some((form, s));
        }
    }
    best.map(|(f, _)| f)
}

/// `HasFormFor`: whether the kind can be offered at all on this load order.
#[must_use]
pub fn has_form_for(forms: &[FormInfo], kind: Kind) -> bool {
    forms.iter().any(|f| f.usable() && kind_of(f.flags) == kind)
}

/// `CanMatch`: a form carrying both the shadow state and the animation asked for exists.
#[must_use]
pub fn can_match(forms: &[FormInfo], kind: Kind, shadows: bool, animation: Animation) -> bool {
    forms.iter().any(|f| f.usable() && kind_of(f.flags) == kind && score(f.flags, shadows, animation) == 6)
}

/// `DefaultForm`: shadowless and steady, since shadow casters can silently not render.
#[must_use]
pub fn default_form(forms: &[FormInfo], spot: bool) -> Option<&FormInfo> {
    pick_form(forms, if spot { Kind::Spot } else { Kind::Point }, false, Animation::None)
}

/// A saved light's form key as the runtime form and its flags; the load order's default form of
/// the kind when the key's plugin is gone (`LightsFromJson`'s fallback).
#[must_use]
pub fn resolve_form(key: &str, spot: bool) -> Option<(u32, u32)> {
    let id = super::environment::resolve_form_key(key);
    let forms = all_forms();
    forms.iter().find(|f| id != 0 && f.form_id == id).or_else(|| default_form(forms, spot)).map(|f| (f.form_id, f.flags))
}

/// One light of the rig (`Light`).
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Light {
    /// Identity for spawn results landing later.
    pub id: u32,
    pub handle: RefHandle,
    pub base: u32,
    pub name: String,
    /// Derived from the installed form, for the aim.
    pub spot: bool,
    pub enabled: bool,
    pub kind: Kind,
    pub animation: Animation,
    pub role: Role,
    pub attach: Attach,
    pub shadows: bool,
    /// Subtractive: the engine's own negative flag is only a negated diffuse at the light's creation,
    /// so it is done on the instance and works with any form.
    pub negative: bool,
    pub bone: String,
    /// Degrees around the anchor, from the rig's rotation.
    pub angle: f32,
    pub distance: f32,
    pub height: f32,
    /// Spotlight aim offsets from the chest.
    pub aim_x: f32,
    pub aim_y: f32,
    pub color: P3,
    pub fade: f32,
    pub radius: f32,
    /// The engine has no area lights: a softbox is a grid of dimmer emitters on a rectangle facing
    /// the subject; the emitters past the first are `extra`.
    pub softbox: bool,
    pub soft_width: f32,
    pub soft_height: f32,
    pub soft_columns: i32,
    pub soft_rows: i32,
    pub extra: Vec<RefHandle>,
    /// Emitter spawns queued, so a failing spawn is not asked for again every frame.
    pub pending_extras: u32,
    /// The last placement, for the markers.
    pub world_position: P3,
    /// `Attach::World`'s anchor, taken when the mode is picked.
    pub anchor: P3,
    pub anchor_valid: bool,
    pub spawn_requested: bool,
    pub delete_requested: bool,
    pub swap_to: u32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            id: 0,
            handle: RefHandle::INVALID,
            base: 0,
            name: String::new(),
            spot: false,
            enabled: true,
            kind: Kind::Point,
            animation: Animation::None,
            role: Role::Custom,
            attach: Attach::Target,
            shadows: false,
            negative: false,
            bone: String::new(),
            angle: 0.0,
            distance: 120.0,
            height: 120.0,
            aim_x: 0.0,
            aim_y: 0.0,
            color: [1.0; 3],
            fade: 1.0,
            radius: 256.0,
            softbox: false,
            soft_width: 160.0,
            soft_height: 120.0,
            soft_columns: 3,
            soft_rows: 2,
            extra: Vec::new(),
            pending_extras: 0,
            world_position: [0.0; 3],
            anchor: [0.0; 3],
            anchor_valid: false,
            spawn_requested: false,
            delete_requested: false,
            swap_to: 0,
        }
    }
}

/// A role's seed: angle from the rig's facing, distance, height, colour, fade (kept under 1, the
/// engine overdrives above it), radius and kind.
struct RolePreset {
    angle: f32,
    distance: f32,
    height: f32,
    color: P3,
    fade: f32,
    radius: f32,
    kind: Kind,
}

const ROLE_PRESETS: [RolePreset; 6] = [
    RolePreset { angle: 0.0, distance: 120.0, height: 120.0, color: [1.0, 1.0, 1.0], fade: 1.0, radius: 256.0, kind: Kind::Point },
    RolePreset { angle: 45.0, distance: 130.0, height: 150.0, color: [1.0, 0.95, 0.86], fade: 1.0, radius: 420.0, kind: Kind::Spot },
    RolePreset { angle: 300.0, distance: 150.0, height: 90.0, color: [0.84, 0.89, 1.0], fade: 0.45, radius: 520.0, kind: Kind::Point },
    RolePreset { angle: 165.0, distance: 120.0, height: 175.0, color: [1.0, 1.0, 1.0], fade: 0.9, radius: 300.0, kind: Kind::Spot },
    RolePreset { angle: 120.0, distance: 130.0, height: 60.0, color: [1.0, 0.92, 0.8], fade: 0.65, radius: 300.0, kind: Kind::Spot },
    RolePreset { angle: 180.0, distance: 230.0, height: 45.0, color: [0.9, 0.93, 1.0], fade: 0.8, radius: 560.0, kind: Kind::Point },
];

/// `ApplyRolePreset`: re-seeds a light from its role; a spot role falls back to a point when the
/// load order has no usable spotlight. `spot` stays the installed form's.
pub fn apply_role_preset(light: &mut Light, role: Role, forms: &[FormInfo]) {
    light.role = role;
    if role == Role::Custom {
        return;
    }
    let preset = &ROLE_PRESETS[index_of(&Role::ALL, &role) as usize];
    light.angle = preset.angle;
    light.distance = preset.distance;
    light.height = preset.height;
    light.color = preset.color;
    light.fade = preset.fade;
    light.radius = preset.radius;
    light.kind = if has_form_for(forms, preset.kind) { preset.kind } else { Kind::Point };
}

/// `AimAngle`: the reference angles pointing a spotlight at `from` towards `to`. The beam is the
/// light's local +X (world rotation column 0): `M = Rz(a) * Ry(b)` lands column 0 on the direction,
/// and its XYZ Euler angles negated are the reference's (SAM's recipe, as IC has it).
#[must_use]
pub fn aim_angle(from: P3, to: P3) -> Option<P3> {
    let direction = point::sub(to, from);
    let length = point::length(direction);
    if length <= 1.0 {
        return None;
    }
    let a = direction[1].atan2(direction[0]);
    let b = -(direction[2] / length).clamp(-1.0, 1.0).asin();
    Some([-(-b.sin() * a.sin()).atan2(b.cos()), -(b.sin() * a.cos()).asin(), -a.sin().atan2(b.cos() * a.cos())])
}

/// `SoftboxExtras`: the emitters past the first.
#[must_use]
pub fn softbox_extras(light: &Light) -> usize {
    if !light.softbox {
        return 0;
    }
    let (columns, rows) = (light.soft_columns.clamp(1, 5), light.soft_rows.clamp(1, 5));
    usize::try_from(columns * rows - 1).unwrap_or(0)
}

/// The grid positions of a softbox centred on `centre` facing `aim`, row by row.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn softbox_positions(light: &Light, centre: P3, aim: P3) -> Vec<P3> {
    let (forward, length) = point::unitize(point::sub(aim, centre));
    let forward = if length < 0.001 { [0.0, 1.0, 0.0] } else { forward };
    let (right, length) = point::unitize(point::cross(forward, [0.0, 0.0, 1.0]));
    let right = if length < 0.001 { [1.0, 0.0, 0.0] } else { right };
    let (up, _) = point::unitize(point::cross(right, forward));
    let (columns, rows) = (light.soft_columns.clamp(1, 5), light.soft_rows.clamp(1, 5));
    let spread = |i: i32, n: i32| if n > 1 { i as f32 / (n - 1) as f32 - 0.5 } else { 0.0 };
    (0..rows).flat_map(|row| (0..columns).map(move |column| (row, column))).map(|(row, column)| point::add(point::add(centre, point::scale(right, spread(column, columns) * light.soft_width)), point::scale(up, spread(row, rows) * light.soft_height))).collect()
}

/// A placed light near the target (`CellLight`): display only until its first edit, then managed
/// and its values held every tick; the originals are taken while it is untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct CellLight {
    pub handle: RefHandle,
    pub label: String,
    pub distance: f32,
    pub managed: bool,
    pub enabled: bool,
    pub values: LightValues,
    pub original: LightValues,
    pub restore_requested: bool,
}

/// Engine work for the frame callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Place a reference of `base` at `target` for light `light` (one of its softbox's extra
    /// emitters when `extra`).
    Spawn { light: u32, base: u32, target: RefHandle, extra: bool },
    Delete(RefHandle),
}

/// The rig as a file or a scene keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rig {
    /// Degrees.
    pub rotation: f32,
    pub offset: P3,
    pub lights: Vec<Light>,
}

/// `LightEngine`.
#[derive(Debug, Default)]
pub struct LightEngine {
    pub lights: Vec<Light>,
    pub cell_lights: Vec<CellLight>,
    /// Degrees, the whole rig at once.
    pub rig_rotation: f32,
    pub rig_offset: P3,
    pub markers_visible: bool,
    graveyard: Vec<RefHandle>,
    next_id: u32,
    cell_tick: u32,
    jobs: Vec<Job>,
}

fn light_node(handle: RefHandle) -> Option<&'static NiPointLight> {
    let reference = handle.lookup()?;
    let root = reference.get3_d()?;
    point_light(root)
}

/// `WriteLightNode`: a subtractive light is a negated diffuse, the whole of the engine's own flag.
fn write_light(node: &NiPointLight, color: P3, fade: f32, radius: f32, negative: bool) {
    let sign = if negative { -1.0 } else { 1.0 };
    node.set_values(LightValues { color: point::scale(color, sign), fade, radius });
}

/// `ApplyLightNode`.
fn apply_light(handle: RefHandle, light: &Light, fade_scale: f32) {
    if let Some(node) = light_node(handle) {
        write_light(node, light.color, if light.enabled { light.fade * fade_scale } else { 0.0 }, light.radius, light.negative);
    }
}

/// Moves a light reference, turning it first when it aims.
fn place(reference: &TESObjectREFR, position: P3, angle: Option<P3>) {
    if let Some([x, y, z]) = angle {
        reference.set_angles(&sre::NiPoint3 { x, y, z, __engine: sre::Engine::new() });
    }
    physics::move_reference(reference, position);
}

impl LightEngine {
    /// `AddLight`.
    pub fn add_light(&mut self, kind: Kind, shadows: bool, negative: bool) {
        let Some(form) = pick_form(all_forms(), kind, shadows, Animation::None) else {
            crate::log(&format!("<Photo Mode> No usable {} light form found in the load order", kind.name()));
            return;
        };
        let name = format!("{}{}{} {}", if negative { "Negative " } else { "" }, if shadows { "Shadow " } else { "" }, kind.name(), self.lights.len() + 1);
        #[allow(clippy::cast_precision_loss)]
        let angle = self.lights.len() as f32 * 120.0;
        self.push(Light { base: form.form_id, name, kind, negative, spot: is_spot(form.flags), shadows: casts_shadows(form.flags), angle, ..Light::default() });
    }

    /// `AddRoleLight`: seeded from the role, then placed.
    pub fn add_role_light(&mut self, role: Role) {
        let mut light = Light::default();
        apply_role_preset(&mut light, role, all_forms());
        let Some(form) = pick_form(all_forms(), light.kind, false, Animation::None) else {
            crate::log(&format!("<Photo Mode> No usable {} light form found in the load order", light.kind.name()));
            return;
        };
        self.push(Light { base: form.form_id, spot: is_spot(form.flags), name: role.name().to_owned(), ..light });
    }

    fn push(&mut self, light: Light) {
        self.next_id += 1;
        self.lights.push(Light { id: self.next_id, spawn_requested: true, ..light });
    }

    /// `AddThreePointSetup`: key, fill and rim, the standard portrait.
    pub fn add_three_point_setup(&mut self) {
        for role in [Role::Key, Role::Fill, Role::Rim] {
            self.add_role_light(role);
        }
    }

    /// `RefreshForm`: kind, shadow and animation live on the base form, so a change swaps the form
    /// (respawned in place on the tick); what lands is read back over the request.
    pub fn refresh_form(&mut self, index: usize) {
        let Some(light) = self.lights.get_mut(index) else { return };
        let Some(form) = pick_form(all_forms(), light.kind, light.shadows, light.animation) else {
            crate::log(&format!("<Photo Mode> No usable {} light form found in the load order", light.kind.name()));
            return;
        };
        if form.form_id != light.base {
            light.swap_to = form.form_id;
        }
    }

    pub fn request_delete(&mut self, index: usize) {
        if let Some(light) = self.lights.get_mut(index) {
            light.delete_requested = true;
        }
    }

    pub fn request_swap(&mut self, index: usize, form_id: u32) {
        if let Some(light) = self.lights.get_mut(index) {
            light.swap_to = form_id;
        }
    }

    fn bury(&mut self) {
        for light in core::mem::take(&mut self.lights) {
            self.graveyard.extend(Some(light.handle).filter(|h| *h != RefHandle::INVALID));
            self.graveyard.extend(light.extra);
        }
    }

    /// `ClearAll`: the references go to the graveyard (removed on the next tick), the rig's
    /// transform with them, edited cell lights are handed back and the rest of the listing dropped.
    pub fn clear_all(&mut self) {
        self.bury();
        self.rig_rotation = 0.0;
        self.rig_offset = [0.0; 3];
        self.cell_lights.retain_mut(|c| {
            c.restore_requested = c.managed;
            c.managed
        });
    }

    pub fn set_all_cell_lights(&mut self, enabled: bool) {
        for c in &mut self.cell_lights {
            c.managed = true;
            c.enabled = enabled;
        }
    }

    pub fn restore_all_cell_lights(&mut self) {
        for c in self.cell_lights.iter_mut().filter(|c| c.managed) {
            c.restore_requested = true;
        }
    }

    /// The rig as it stands, for a file or a scene.
    #[must_use]
    pub fn rig(&self) -> Rig {
        Rig { rotation: self.rig_rotation, offset: self.rig_offset, lights: self.lights.clone() }
    }

    /// `ReplaceRig`: the incoming rig is data only; every reference is placed fresh.
    pub fn replace_rig(&mut self, rig: Rig) {
        self.bury();
        for light in rig.lights {
            self.next_id += 1;
            self.lights.push(Light { id: self.next_id, handle: RefHandle::INVALID, extra: Vec::new(), pending_extras: 0, ..light });
        }
        self.rig_rotation = rig.rotation;
        self.rig_offset = rig.offset;
    }

    /// `OwnsRef`: the graveyard counts, a released emitter lives until the next tick.
    fn owns(&self, handle: RefHandle) -> bool {
        self.graveyard.contains(&handle) || self.lights.iter().any(|l| l.handle == handle || l.extra.contains(&handle))
    }

    /// The engine work queued since the last call.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        core::mem::take(&mut self.jobs)
    }

    /// A spawn's result on the frame callback: kept when its light still wants it, else handed
    /// back to be removed. A light deleted or swapped to another form while the spawn was in
    /// flight does not take it.
    #[must_use]
    pub fn land(&mut self, id: u32, base: u32, extra: bool, placed: Option<RefHandle>) -> Option<RefHandle> {
        let mut entry = self.lights.iter_mut().find(|l| l.id == id);
        if extra && let Some(l) = entry.as_mut() {
            l.pending_extras = l.pending_extras.saturating_sub(1);
        }
        let placed = placed?;
        let kept = match entry {
            Some(l) if extra && l.base == base && l.extra.len() < softbox_extras(l) => {
                l.extra.push(placed);
                true
            }
            Some(l) if !extra && l.handle == RefHandle::INVALID && l.base == base => {
                l.handle = placed;
                true
            }
            _ => false,
        };
        (!kept).then_some(placed)
    }

    /// `ResolvePosition`: where the light sits this tick for its attachment.
    fn resolve_position(&self, light: &mut Light, target: &TESObjectREFR, target_position: P3) -> P3 {
        let mut yaw = self.rig_rotation;
        let mut origin = point::add(target_position, self.rig_offset);
        if light.attach != Attach::World {
            light.anchor_valid = false;
        }
        match light.attach {
            Attach::Camera => {
                if let Some(root) = util::camera().and_then(|c| c.camera_root()) {
                    let world = root.world_transform();
                    origin = world.translation;
                    yaw += world.rotation[0][1].atan2(world.rotation[1][1]).to_degrees();
                }
            }
            Attach::Bone => {
                if let Some(node) = third_person_3d(target).filter(|_| !light.bone.is_empty()).and_then(|root| util::object_by_name(root, &light.bone)) {
                    origin = point::add(node.world_transform().translation, self.rig_offset);
                }
            }
            Attach::World => {
                if !light.anchor_valid {
                    light.anchor = target_position;
                    light.anchor_valid = true;
                }
                origin = point::add(light.anchor, self.rig_offset);
            }
            Attach::Target => {}
        }
        let angle = (light.angle + yaw).to_radians();
        [origin[0] + angle.sin() * light.distance, origin[1] + angle.cos() * light.distance, origin[2] + light.height]
    }

    /// `SyncSoftboxRefs`: surplus emitters released, missing ones asked for once each.
    fn sync_softbox(light: &mut Light, target: RefHandle, graveyard: &mut Vec<RefHandle>, jobs: &mut Vec<Job>) {
        let wanted = softbox_extras(light);
        while light.extra.len() > wanted {
            graveyard.extend(light.extra.pop());
        }
        let have = light.extra.len() + light.pending_extras as usize;
        if have >= wanted {
            return;
        }
        for _ in have..wanted {
            jobs.push(Job::Spawn { light: light.id, base: light.base, target, extra: true });
        }
        light.pending_extras += u32::try_from(wanted - have).unwrap_or(0);
    }

    /// `PlaceSoftbox`: the emitters across the panel, each aiming from where it sits, the panel's
    /// intensity split between them.
    #[allow(clippy::cast_precision_loss)]
    fn place_softbox(light: &mut Light, position: P3, aim: P3) {
        let positions = softbox_positions(light, position, aim);
        let fade_scale = 1.0 / positions.len() as f32;
        light.world_position = position;
        for (slot, at) in positions.into_iter().enumerate() {
            let handle = if slot == 0 { light.handle } else { light.extra.get(slot - 1).copied().unwrap_or(RefHandle::INVALID) };
            let Some(reference) = handle.lookup() else { continue };
            place(&reference, at, if light.spot { aim_angle(at, aim) } else { None });
            apply_light(handle, light, fade_scale);
        }
    }

    /// `OnGameUpdate`: the graveyard and cell light restores first (honoured without a target, the
    /// way the studio hands everything back at exit), deletions, then with a target the cell
    /// listing, the held cell light values and every light's swap, spawn and placement.
    pub fn on_game_update(&mut self, target: Option<&TESObjectREFR>, active: bool) {
        self.jobs.extend(self.graveyard.drain(..).map(Job::Delete));
        self.cell_lights.retain(|c| {
            if !c.restore_requested {
                return true;
            }
            if let Some(node) = light_node(c.handle) {
                node.set_values(c.original);
            }
            false
        });
        let jobs = &mut self.jobs;
        self.lights.retain(|l| {
            if l.delete_requested {
                jobs.extend(Some(l.handle).filter(|h| *h != RefHandle::INVALID).map(Job::Delete));
                jobs.extend(l.extra.iter().copied().map(Job::Delete));
            }
            !l.delete_requested
        });
        let Some(target) = target.filter(|t| active && third_person_3d(t).is_some()) else { return };
        if self.cell_tick.is_multiple_of(15) {
            self.refresh_cell_lights(target);
        }
        self.cell_tick = self.cell_tick.wrapping_add(1);
        for c in self.cell_lights.iter().filter(|c| c.managed) {
            if let Some(node) = light_node(c.handle) {
                node.set_values(LightValues { fade: if c.enabled { c.values.fade } else { 0.0 }, ..c.values });
            }
        }
        let target_position = util::position(target);
        let chest = third_person_3d(target).map_or(target_position, |root| super::manager::aim_point(target, root));
        let target_handle = target.handle();
        let mut lights = core::mem::take(&mut self.lights);
        for light in &mut lights {
            if light.swap_to != 0 {
                self.jobs.extend(Some(light.handle).filter(|h| *h != RefHandle::INVALID).map(Job::Delete));
                self.jobs.extend(light.extra.drain(..).map(Job::Delete));
                light.handle = RefHandle::INVALID;
                light.base = core::mem::take(&mut light.swap_to);
                if let Some(form) = all_forms().iter().find(|f| f.form_id == light.base) {
                    light.spot = is_spot(form.flags);
                    light.kind = kind_of(form.flags);
                    light.shadows = casts_shadows(form.flags);
                }
                light.spawn_requested = true;
            }
            if light.spawn_requested {
                light.spawn_requested = false;
                self.jobs.push(Job::Spawn { light: light.id, base: light.base, target: target_handle, extra: false });
                continue;
            }
            let Some(reference) = light.handle.lookup() else { continue };
            let position = self.resolve_position(light, target, target_position);
            let aim = [chest[0] + light.aim_x, chest[1] + light.aim_y, chest[2]];
            Self::sync_softbox(light, target_handle, &mut self.graveyard, &mut self.jobs);
            if light.softbox {
                Self::place_softbox(light, position, aim);
            } else {
                light.world_position = position;
                place(&reference, position, if light.spot { aim_angle(position, aim) } else { None });
                apply_light(light.handle, light, 1.0);
            }
        }
        self.lights = lights;
    }

    /// `RefreshCellLights`: untouched rows rebuilt, managed rows kept until their 3D goes (a cell
    /// reload rebuilds the light from its form, edits and all), at most 64, nearest first.
    fn refresh_cell_lights(&mut self, target: &TESObjectREFR) {
        self.cell_lights.retain(|c| c.managed && c.handle.lookup().is_some_and(|r| r.get3_d().is_some()));
        let origin = util::position(target);
        for c in &mut self.cell_lights {
            if let Some(r) = c.handle.lookup() {
                c.distance = point::length(point::sub(util::position(&r), origin));
            }
        }
        let radius = settings::profile().Studio.fNearbyRadius;
        for handle in skyrim::actors::references_around(target, radius) {
            if self.cell_lights.len() >= 64 {
                break;
            }
            if self.owns(handle) || self.cell_lights.iter().any(|c| c.handle == handle) {
                continue;
            }
            let Some(reference) = handle.lookup() else { continue };
            let Some(base) = reference.base_object().and_then(|b| b.cast::<TESObjectLIGH>()) else { continue };
            let Some(node) = light_node(handle) else { continue };
            let values = node.values();
            let label = base.full_name().map_or_else(|| format!("Light {:08X}", reference.formID), |n| format!("{} ({:08X})", n.to_string_lossy(), reference.formID));
            self.cell_lights.push(CellLight { handle, label, distance: point::length(point::sub(util::position(&reference), origin)), managed: false, enabled: true, values, original: values, restore_requested: false });
        }
        self.cell_lights.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    }

    /// For an unload: every placed reference removed and every edited cell light handed back now.
    pub fn release(&mut self) {
        self.bury();
        for job in core::mem::take(&mut self.jobs) {
            if let Job::Delete(handle) = job {
                delete(handle);
            }
        }
        for handle in core::mem::take(&mut self.graveyard) {
            delete(handle);
        }
        for c in self.cell_lights.iter().filter(|c| c.managed) {
            if let Some(node) = light_node(c.handle) {
                node.set_values(c.original);
            }
        }
        *self = Self::default();
    }
}

/// A spawn job: a reference of the light form `base` placed at `target`. Main thread.
#[must_use]
pub fn spawn(base: u32, target: RefHandle) -> Option<RefHandle> {
    let target = target.lookup()?;
    let form = sre::find_form_by_id(base)?.cast::<TESObjectLIGH>()?;
    let placed = skyrim::actors::place_at_me(&target, form, false)?;
    crate::log(&format!("<Photo Mode> Spawned {} light {placed} (base {base:08X}, flags {:08X}, fov {:.0}, radius {})", if is_spot(form.flags) { "spot" } else { "point" }, form.flags, form.fov, form.radius));
    Some(placed)
}

/// `DeleteRef`: disabled and marked deleted. Main thread.
pub fn delete(handle: RefHandle) {
    if let Some(reference) = handle.lookup() {
        skyrim::actors::delete_reference(&reference);
    }
}

#[cfg(test)]
mod tests {
    use skyrim::render::from_euler_xyz;

    use super::*;

    fn form(id: u32, flags: u32) -> FormInfo {
        FormInfo { form_id: id, flags, has_model: false, plugin: "Skyrim.esm".into(), label: format!("{id:08X}") }
    }

    #[test]
    fn flags_decode_as_the_engine_reads_them() {
        assert!(is_spot(0x400) && !is_spot(0x200) && !is_spot(0x800));
        assert!(casts_shadows(0x400) && casts_shadows(0x800) && casts_shadows(0x1000) && !casts_shadows(0x200));
        assert_eq!(kind_of(0x1400), Kind::Spot);
        assert_eq!(kind_of(0x200), Kind::Point, "nothing reads 0x200");
        assert_eq!(score(0, false, Animation::None), 6);
        assert_eq!(score(0x40, false, Animation::None), 6, "0x40 alone does not flicker");
        for animated in [0x8, 0x80, 0x100] {
            assert_eq!(score(animated, false, Animation::None), 4, "{animated:#x} animates");
        }
        assert_eq!(score(0x0408, true, Animation::Flicker), 6);
        assert_eq!(score(0x0048, false, Animation::FlickerSlow), 6);
        assert_eq!(score(0x0040, false, Animation::FlickerSlow), 4, "the variant bit without flicker");
        assert_eq!(score(0x0080, true, Animation::Pulse), 2);
        assert_eq!(score(0x1100, true, Animation::PulseSlow), 6);
        let carried = FormInfo { flags: 0x2, ..form(1, 0x2) };
        let meshed = FormInfo { has_model: true, ..form(2, 0) };
        assert!(!carried.usable() && !meshed.usable() && form(3, 0x4).usable());
    }

    #[test]
    fn forms_are_picked_by_kind_then_score() {
        let forms = [form(1, 0x2), form(2, 0x8), form(3, 0x400), form(4, 0x200), form(5, 0), form(6, 0x1000)];
        assert_eq!(pick_form(&forms, Kind::Point, false, Animation::None).map(|f| f.form_id), Some(4), "0x200 is a plain point");
        assert_eq!(pick_form(&forms, Kind::Point, false, Animation::Flicker).map(|f| f.form_id), Some(2));
        assert_eq!(pick_form(&forms, Kind::Point, true, Animation::None).map(|f| f.form_id), Some(6));
        assert_eq!(pick_form(&forms, Kind::Spot, false, Animation::None).map(|f| f.form_id), Some(3), "every spot casts shadows");
        assert_eq!(pick_form(&forms, Kind::Spot, true, Animation::None).map(|f| f.form_id), Some(3));
        assert!(can_match(&forms, Kind::Spot, true, Animation::None) && !can_match(&forms, Kind::Spot, true, Animation::Pulse));
        assert!(!has_form_for(&forms[..1], Kind::Point));
        let mut light = Light::default();
        apply_role_preset(&mut light, Role::Key, &forms[..2]);
        assert_eq!((light.kind, light.angle, light.radius), (Kind::Point, 45.0, 420.0), "no spot: the key falls back to a point");
        apply_role_preset(&mut light, Role::Rim, &forms);
        assert_eq!((light.kind, light.role, light.height), (Kind::Spot, Role::Rim, 175.0));
    }

    #[test]
    fn a_spot_aims_its_local_x_at_the_chest() {
        for (from, to) in [([0.0, 0.0, 0.0], [100.0, 0.0, 0.0]), ([10.0, -20.0, 150.0], [-50.0, 80.0, 60.0]), ([0.0, 0.0, 0.0], [0.0, -30.0, -40.0])] {
            let [x, y, z] = aim_angle(from, to).unwrap();
            let m = from_euler_xyz(x, y, z);
            let (dir, _) = point::unitize(point::sub(to, from));
            let column = [m[0][0], m[1][0], m[2][0]];
            assert!((0..3).all(|i| (column[i] - dir[i]).abs() < 1e-4), "{from:?} -> {to:?}: {column:?} vs {dir:?}");
        }
        assert_eq!(aim_angle([0.0; 3], [0.5, 0.0, 0.0]), None);
    }

    #[test]
    fn a_softbox_spreads_its_grid_facing_the_aim() {
        let light = Light { softbox: true, soft_columns: 3, soft_rows: 2, soft_width: 100.0, soft_height: 50.0, ..Light::default() };
        assert_eq!(softbox_extras(&light), 5);
        assert_eq!(softbox_extras(&Light { soft_columns: 9, soft_rows: 0, ..light.clone() }), 4);
        assert_eq!(softbox_extras(&Light::default()), 0);
        let grid = softbox_positions(&light, [0.0, 0.0, 0.0], [0.0, 100.0, 0.0]);
        assert_eq!(grid.len(), 6);
        assert!(grid.iter().all(|p| p[1].abs() < 1e-5), "the panel is perpendicular to the aim");
        assert!((grid[0][0] + 50.0).abs() < 1e-4 && (grid[2][0] - 50.0).abs() < 1e-4 && (grid[0][2] + 25.0).abs() < 1e-4 && (grid[5][2] - 25.0).abs() < 1e-4, "{grid:?}");
    }

    #[test]
    fn spawns_land_only_on_the_light_that_still_wants_them() {
        let mut engine = LightEngine::default();
        engine.push(Light { base: 7, softbox: true, soft_columns: 2, soft_rows: 1, pending_extras: 1, ..Light::default() });
        let id = engine.lights[0].id;
        assert_eq!(engine.land(id, 7, false, Some(RefHandle(0x10))), None);
        assert_eq!(engine.land(id, 7, false, Some(RefHandle(0x11))), Some(RefHandle(0x11)), "already holds one");
        assert_eq!(engine.land(id, 7, true, Some(RefHandle(0x12))), None);
        assert_eq!(engine.lights[0].pending_extras, 0);
        assert_eq!(engine.land(id, 7, true, Some(RefHandle(0x13))), Some(RefHandle(0x13)), "the panel is full");
        assert_eq!(engine.land(id, 8, true, None), None);
        assert_eq!(engine.land(id + 1, 7, false, Some(RefHandle(0x14))), Some(RefHandle(0x14)), "deleted in flight");
        assert!(engine.owns(RefHandle(0x12)));
        engine.clear_all();
        assert!(engine.lights.is_empty() && engine.owns(RefHandle(0x10)) && engine.owns(RefHandle(0x12)));
        engine.replace_rig(Rig { rotation: 30.0, offset: [1.0, 2.0, 3.0], lights: vec![Light { handle: RefHandle(0x99), extra: vec![RefHandle(0x98)], spawn_requested: true, ..Light::default() }] });
        assert_eq!((engine.lights[0].handle, engine.lights[0].extra.len(), engine.rig_rotation), (RefHandle::INVALID, 0, 30.0));
        assert!(engine.lights[0].id > id);
    }
}
