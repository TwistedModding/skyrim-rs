//! `camera/studio/StudioIO`: the studio's files. Its own live in a folder of the port's
//! (`Data\SKSE\Plugins\skyrim-rs\improved-camera\PhotoMode\`, with IC's layout: `Adjustments`,
//! `Poses`, `Expressions`, `Lights`, `Scenes`, `Shots` and `Cameras.json`);
//! Screen Archer Menu's pose, adjustment and face morph packs under `Data\SAM\` are only read. JSON
//! as IC writes it (tab-indented, `version` 1).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use framework::settings::{Json, parse_json};
use skyrim::render::Transform;

use super::adjustments::{Adjustment, BoneDelta, degrees_from_rotation, rotation_from_degrees};
use super::face::{FaceNames, FaceState, parse_sam_expression};
use super::lights::{self, Animation, Attach, Kind, Light, Rig, Role};
use crate::util::point::P3;

const FILE_VERSION: f64 = 1.0;

/// `PhotoMode\` in the port's folder beside its settings file.
#[must_use]
pub fn photo_mode_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(r"Data\SKSE\Plugins\skyrim-rs\improved-camera\PhotoMode")
}

/// `Data\SAM\<kind>` (`Poses` or `Adjustments`).
#[must_use]
pub fn sam_dir(kind: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(r"Data\SAM").join(kind)
}

/// `EnsureDirectory`: `PhotoMode\<sub>`, created on first use.
#[must_use]
pub fn directory(sub: &str) -> PathBuf {
    let path = photo_mode_dir().join(sub);
    if let Err(e) = std::fs::create_dir_all(&path) {
        crate::log(&format!("<Photo Mode> Failed to create {}: {e}", path.display()));
    }
    path
}

/// `SanitizeFileName`: letters, digits, spaces, `-` and `_`.
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect()
}

/// `ListFiles`: the sorted `*.json` stems in `dir`.
#[must_use]
pub fn list_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .collect();
    files.sort();
    files
}

fn num(v: f32) -> Json {
    Json::Number(v.to_string().parse().unwrap_or(0.0))
}

fn triple(v: P3) -> Json {
    Json::Array(v.iter().map(|x| num(*x)).collect())
}

fn object(members: Vec<(&str, Json)>) -> Json {
    Json::Object(members.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

#[allow(clippy::cast_possible_truncation)]
fn as_f32(json: Option<&Json>) -> Option<f32> {
    match json {
        Some(Json::Number(v)) => Some(*v as f32),
        _ => None,
    }
}

fn f32_or(json: &Json, key: &str, default: f32) -> f32 {
    as_f32(json.get(key)).unwrap_or(default)
}

fn bool_or(json: &Json, key: &str, default: bool) -> bool {
    match json.get(key) {
        Some(Json::Bool(v)) => *v,
        _ => default,
    }
}

fn text_or(json: &Json, key: &str, default: &str) -> String {
    match json.get(key) {
        Some(Json::Text(v)) => v.clone(),
        _ => default.to_owned(),
    }
}

/// `ReadFloat3`: elements that are not numbers keep `out`'s; false when the key is not a 3-array.
fn read_triple(json: &Json, key: &str, out: &mut P3) -> bool {
    let Some(Json::Array(items)) = json.get(key).filter(|v| matches!(v, Json::Array(a) if a.len() == 3)) else { return false };
    for (o, item) in out.iter_mut().zip(items) {
        if let Some(v) = as_f32(Some(item)) {
            *o = v;
        }
    }
    true
}

fn members(json: Option<&Json>) -> &[(String, Json)] {
    match json {
        Some(Json::Object(m)) => m,
        _ => &[],
    }
}

fn read(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("Failed to open {}: {e}", path.display()))?;
    parse_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, json: &Json) -> Result<(), String> {
    std::fs::write(path, json.to_string()).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn bone_delta_json(d: &BoneDelta) -> Json {
    object(vec![("pos", triple(d.position)), ("rot", triple(d.rotation)), ("scale", num(d.scale))])
}

fn bone_delta(json: &Json) -> BoneDelta {
    let mut d = BoneDelta::default();
    read_triple(json, "pos", &mut d.position);
    read_triple(json, "rot", &mut d.rotation);
    d.scale = f32_or(json, "scale", 1.0);
    d
}

/// Identity bones are left out: a layer is its edits.
fn bones_json(bones: &BTreeMap<String, BoneDelta>) -> Json {
    Json::Object(bones.iter().filter(|(_, d)| !d.is_identity()).map(|(n, d)| (n.clone(), bone_delta_json(d))).collect())
}

fn bones(json: Option<&Json>) -> BTreeMap<String, BoneDelta> {
    members(json).iter().map(|(n, e)| (n.clone(), bone_delta(e))).collect()
}

/// `LayerToJson`: a whole layer with its flags, as a scene keeps it.
fn layer_json(layer: &Adjustment) -> Json {
    object(vec![
        ("name", Json::Text(layer.name.clone())),
        ("file", Json::Text(layer.file.clone())),
        ("enabled", Json::Bool(layer.enabled)),
        ("isPose", Json::Bool(layer.is_pose)),
        ("persistent", Json::Bool(layer.persistent)),
        ("includeInPose", Json::Bool(layer.include_in_pose)),
        ("bones", bones_json(&layer.bones)),
    ])
}

/// `LayerFromJson`.
fn layer(json: &Json) -> Adjustment {
    Adjustment {
        name: text_or(json, "name", "Adjustment"),
        file: text_or(json, "file", ""),
        enabled: bool_or(json, "enabled", true),
        is_pose: bool_or(json, "isPose", false),
        persistent: bool_or(json, "persistent", false),
        include_in_pose: bool_or(json, "includeInPose", true),
        bones: bones(json.get("bones")),
    }
}

/// `FaceToJson`: the face body expression files and scenes share.
fn face_json(face: &FaceState) -> Vec<(&'static str, Json)> {
    let mut fields = vec![
        ("expression", Json::Number(f64::from(face.expression))),
        ("expressionValue", num(face.expression_value)),
        ("phonemes", Json::Array(face.phonemes.iter().map(|v| num(*v)).collect())),
        ("modifiers", Json::Array(face.modifiers.iter().map(|v| num(*v)).collect())),
        ("eyesOverride", Json::Bool(face.eyes_override)),
        ("eyesHeading", num(face.eyes_heading)),
        ("eyesPitch", num(face.eyes_pitch)),
        ("suppressBlink", Json::Bool(face.suppress_blink)),
        ("persistent", Json::Bool(face.persistent)),
    ];
    if !face.tongue.is_empty() {
        fields.push(("tongue", Json::Object(face.tongue.iter().map(|(n, d)| (n.clone(), bone_delta_json(d))).collect())));
    }
    fields
}

/// `FaceFromJson`.
#[allow(clippy::cast_possible_truncation)]
fn face(json: &Json) -> FaceState {
    let mut out = FaceState::default();
    if let Some(Json::Number(v)) = json.get("expression") {
        out.expression = *v as i32;
    }
    out.expression_value = f32_or(json, "expressionValue", 0.0);
    for (key, values) in [("phonemes", &mut out.phonemes[..]), ("modifiers", &mut out.modifiers[..])] {
        if let Some(Json::Array(items)) = json.get(key) {
            for (value, item) in values.iter_mut().zip(items) {
                if let Some(v) = as_f32(Some(item)) {
                    *value = v;
                }
            }
        }
    }
    out.eyes_override = bool_or(json, "eyesOverride", false);
    out.eyes_heading = f32_or(json, "eyesHeading", 0.0);
    out.eyes_pitch = f32_or(json, "eyesPitch", 0.0);
    out.suppress_blink = bool_or(json, "suppressBlink", true);
    out.persistent = bool_or(json, "persistent", false);
    out.tongue = bones(json.get("tongue"));
    out
}

/// `SaveExpression`: `<dir>\<name>.json`.
pub fn save_expression(dir: &Path, face_state: &FaceState, name: &str) -> Result<PathBuf, String> {
    let mut fields = face_json(face_state);
    fields.push(("version", Json::Number(FILE_VERSION)));
    fields.push(("type", Json::Text("expression".into())));
    let path = dir.join(format!("{name}.json"));
    write(&path, &object(fields)).map(|()| path)
}

/// `LoadExpression`.
pub fn load_expression(dir: &Path, name: &str) -> Result<FaceState, String> {
    read(&dir.join(format!("{name}.json"))).map(|json| face(&json))
}

/// `ListSamExpressions`: the YAML files under `Data\SAM\FaceMorphs`, by relative stem.
#[must_use]
pub fn list_sam_expressions(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
                && let Ok(relative) = path.with_extension("").strip_prefix(base)
            {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();
    files
}

/// `LoadSamExpression`: a `FaceMorphs` file by its relative stem, `.yaml` or `.yml`.
pub fn load_sam_expression(dir: &Path, relative: &str, names: &FaceNames, previous: &FaceState) -> Result<FaceState, String> {
    let text = ["yaml", "yml"].iter().find_map(|ext| std::fs::read_to_string(dir.join(format!("{relative}.{ext}"))).ok()).ok_or_else(|| format!("SAM expression not found: {relative}"))?;
    Ok(parse_sam_expression(&text, names, previous))
}

/// `SaveAdjustment`: one layer's deltas as `<dir>\<name>.json`.
pub fn save_adjustment(dir: &Path, adjustment: &Adjustment, name: &str) -> Result<PathBuf, String> {
    let json = object(vec![("version", Json::Number(FILE_VERSION)), ("type", Json::Text("adjustment".into())), ("name", Json::Text(adjustment.name.clone())), ("bones", bones_json(&adjustment.bones))]);
    let path = dir.join(format!("{name}.json"));
    write(&path, &json).map(|()| path)
}

/// `LoadAdjustment`.
pub fn load_adjustment(dir: &Path, name: &str) -> Result<Adjustment, String> {
    let path = dir.join(format!("{name}.json"));
    let json = read(&path)?;
    if json.get("bones").is_none() {
        return Err(format!("Invalid adjustment file: {}", path.display()));
    }
    Ok(Adjustment { name: text_or(&json, "name", name), bones: bones(json.get("bones")), ..Adjustment::default() })
}

/// `SavePose`: absolute local transforms per bone, the rotation as the exact matrix (`rotmat`, rows
/// first) and readable Euler degrees.
pub fn save_pose(dir: &Path, name: &str, locals: &BTreeMap<String, Transform>) -> Result<PathBuf, String> {
    let bones = locals
        .iter()
        .map(|(bone, t)| {
            let rotmat = Json::Array(t.rotation.iter().flatten().map(|v| num(*v)).collect());
            (bone.clone(), object(vec![("pos", triple(t.translation)), ("rotmat", rotmat), ("rot", triple(degrees_from_rotation(&t.rotation))), ("scale", num(t.scale))]))
        })
        .collect();
    let json = object(vec![("version", Json::Number(FILE_VERSION)), ("type", Json::Text("pose".into())), ("bones", Json::Object(bones))]);
    let path = dir.join(format!("{name}.json"));
    write(&path, &json).map(|()| path)
}

/// `LoadPose`: the matrix when there is one, else the Euler degrees.
pub fn load_pose(dir: &Path, name: &str) -> Result<BTreeMap<String, Transform>, String> {
    let path = dir.join(format!("{name}.json"));
    let json = read(&path)?;
    if json.get("bones").is_none() {
        return Err(format!("Invalid pose file: {}", path.display()));
    }
    let mut out = BTreeMap::new();
    for (bone, entry) in members(json.get("bones")) {
        let mut t = Transform { rotation: crate::util::point::IDENTITY, translation: [0.0; 3], scale: f32_or(entry, "scale", 1.0) };
        read_triple(entry, "pos", &mut t.translation);
        match entry.get("rotmat") {
            Some(Json::Array(m)) if m.len() == 9 => {
                for (i, v) in m.iter().enumerate() {
                    t.rotation[i / 3][i % 3] = as_f32(Some(v)).unwrap_or(0.0);
                }
            }
            _ => {
                let mut rot = [0.0; 3];
                if read_triple(entry, "rot", &mut rot) {
                    t.rotation = rotation_from_degrees(rot);
                }
            }
        }
        out.insert(bone.clone(), t);
    }
    Ok(out)
}

/// A SAM pack: a subfolder of `Data\SAM\<kind>` (`""` for the loose files) and its files,
/// relative, with their extensions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamPack {
    pub name: String,
    pub files: Vec<String>,
}

fn is_sam_file(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| ["yaml", "yml", "json"].iter().any(|s| x.eq_ignore_ascii_case(s)))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    for path in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            walk(&path, base, out);
        } else if is_sam_file(&path)
            && let Ok(relative) = path.strip_prefix(base)
        {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `ListSamPacks`: the loose files first, then each subfolder's files (recursively), sorted.
#[must_use]
pub fn list_sam_packs(dir: &Path) -> Vec<SamPack> {
    let mut loose = SamPack::default();
    let mut packs = Vec::new();
    for path in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()) {
        if path.is_file() && is_sam_file(&path) {
            loose.files.extend(path.file_name().map(|f| f.to_string_lossy().into_owned()));
        } else if path.is_dir() {
            let mut pack = SamPack { name: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(), files: Vec::new() };
            walk(&path, &path, &mut pack.files);
            if !pack.files.is_empty() {
                pack.files.sort();
                packs.push(pack);
            }
        }
    }
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    if !loose.files.is_empty() {
        loose.files.sort();
        packs.insert(0, loose);
    }
    packs
}

/// `SamFloat`: a number, or a number written as a string (older files).
fn sam_float(json: &Json, key: &str, default: f32) -> f32 {
    match json.get(key) {
        Some(Json::Text(s)) => s.trim().parse().unwrap_or(default),
        other => as_f32(other).unwrap_or(default),
    }
}

/// `ParseSamJson`. The rotations are negated: SAM's Euler matrix is `SetEulerAnglesXYZ` with every
/// angle flipped. A rotation order other than `exyz` is read as XYZ and noted.
fn parse_sam_json(text: &str, name: &str, notes: &mut Vec<String>) -> Result<Adjustment, String> {
    let json = parse_json(text).map_err(|e| format!("Invalid SAM file {name}: {e}"))?;
    let Some(Json::Object(transforms)) = json.get("transforms") else { return Err(format!("Invalid SAM file: {name}")) };
    let order = match json.get("rotation") {
        Some(Json::Text(s)) => s.clone(),
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(Json::Number(n)) => String::from_utf8_lossy(&(*n as u32).to_le_bytes()).into_owned(),
        _ => "exyz".to_owned(),
    };
    if order != "exyz" {
        notes.push(format!("SAM file '{name}' uses rotation order '{order}', read as XYZ"));
    }
    let mut adjustment = Adjustment { name: format!("[SAM] {name}"), ..Adjustment::default() };
    for (bone, entry) in transforms.iter().filter(|(_, e)| matches!(e, Json::Object(_))) {
        let delta = BoneDelta {
            position: ["tx", "ty", "tz"].map(|k| sam_float(entry, k, 0.0)),
            rotation: ["rx", "ry", "rz"].map(|k| -sam_float(entry, k, 0.0)),
            scale: sam_float(entry, "s", 1.0),
        };
        if !delta.is_identity() {
            adjustment.bones.insert(bone.clone(), delta);
        }
    }
    Ok(adjustment)
}

/// `ParseSamYaml`: SAM's machine-written YAML, the same schema as its JSON, read line by line.
fn parse_sam_yaml(text: &str, name: &str, notes: &mut Vec<String>) -> Adjustment {
    let mut adjustment = Adjustment { name: format!("[SAM] {name}"), ..Adjustment::default() };
    let (mut bone, mut delta, mut in_transforms) = (String::new(), BoneDelta::default(), false);
    let mut commit = |bone: &mut String, delta: &mut BoneDelta| {
        if !bone.is_empty() && !delta.is_identity() {
            adjustment.bones.insert(core::mem::take(bone), *delta);
        }
        bone.clear();
        *delta = BoneDelta::default();
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim_end();
        let Some((key, value)) = line.split_once(':') else { continue };
        let indented = line.starts_with([' ', '\t']);
        let mut key = key.trim_start();
        let value = value.trim_start();
        if key.len() >= 2 && (key.starts_with('\'') || key.starts_with('"')) && key.ends_with(&key[..1]) {
            key = &key[1..key.len() - 1];
        }
        if !indented {
            commit(&mut bone, &mut delta);
            in_transforms = key == "transforms";
            if key == "rotation" && !value.is_empty() && value != "exyz" {
                notes.push(format!("SAM pose '{name}' uses rotation order '{value}', read as XYZ"));
            }
            continue;
        }
        if !in_transforms {
            continue;
        }
        if value.is_empty() {
            commit(&mut bone, &mut delta);
            key.clone_into(&mut bone);
            continue;
        }
        let Ok(number) = value.parse::<f32>() else { continue };
        match key {
            "tx" => delta.position[0] = number,
            "ty" => delta.position[1] = number,
            "tz" => delta.position[2] = number,
            "rx" => delta.rotation[0] = -number,
            "ry" => delta.rotation[1] = -number,
            "rz" => delta.rotation[2] = -number,
            "s" => delta.scale = number,
            _ => {}
        }
    }
    commit(&mut bone, &mut delta);
    adjustment
}

/// `LoadSamFile`: `file` of `pack` under `dir` (`Data\SAM\<kind>`), JSON by its extension, YAML
/// otherwise; `notes` gets what was read loosely.
pub fn load_sam_file(dir: &Path, pack: &str, file: &str, notes: &mut Vec<String>) -> Result<Adjustment, String> {
    let path = if pack.is_empty() { dir.join(file) } else { dir.join(pack).join(file) };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("SAM file not found: {} ({e})", path.display()))?;
    let file = Path::new(file);
    let name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if file.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) { parse_sam_json(&text, &name, notes) } else { Ok(parse_sam_yaml(&text, &name, notes)) }
}

/// A camera framing: `Cameras.json`'s bookmarks and a scene's camera.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraBookmark {
    pub name: String,
    pub position: P3,
    /// Radians.
    pub pitch: f32,
    pub yaw: f32,
    /// Degrees, the free camera's roll.
    pub roll: f32,
    pub fov: f32,
}

impl Default for CameraBookmark {
    fn default() -> Self {
        Self { name: String::new(), position: [0.0; 3], pitch: 0.0, yaw: 0.0, roll: 0.0, fov: 80.0 }
    }
}

fn camera_json(b: &CameraBookmark, named: bool) -> Json {
    let mut fields = vec![("position", triple(b.position)), ("pitch", num(b.pitch)), ("yaw", num(b.yaw)), ("roll", num(b.roll)), ("fov", num(b.fov))];
    if named {
        fields.insert(0, ("name", Json::Text(b.name.clone())));
    }
    object(fields)
}

fn camera(json: &Json, name: &str) -> CameraBookmark {
    let mut b = CameraBookmark { name: text_or(json, "name", name), pitch: f32_or(json, "pitch", 0.0), yaw: f32_or(json, "yaw", 0.0), roll: f32_or(json, "roll", 0.0), fov: f32_or(json, "fov", 80.0), ..CameraBookmark::default() };
    read_triple(json, "position", &mut b.position);
    b
}

/// `LoadCameraBookmarks` from `Cameras.json` in `dir`; none when it is missing or invalid.
#[must_use]
pub fn load_camera_bookmarks(dir: &Path) -> Vec<CameraBookmark> {
    match read(&dir.join("Cameras.json")).ok().and_then(|j| j.get("bookmarks").cloned()) {
        Some(Json::Array(items)) => items.iter().map(|e| camera(e, "Bookmark")).collect(),
        _ => Vec::new(),
    }
}

pub fn save_camera_bookmarks(dir: &Path, bookmarks: &[CameraBookmark]) -> Result<(), String> {
    let json = object(vec![("version", Json::Number(FILE_VERSION)), ("bookmarks", Json::Array(bookmarks.iter().map(|b| camera_json(b, true)).collect()))]);
    write(&dir.join("Cameras.json"), &json)
}

/// `MatrixToRotationVector`: Screen Archer Menu's axis times angle, for files that match its own.
fn rotation_vector(m: &[[f32; 3]; 3]) -> P3 {
    let cos = (f64::from(m[0][0]) + f64::from(m[1][1]) + f64::from(m[2][2]) - 1.0) * 0.5;
    let v = [m[1][2] - m[2][1], m[2][0] - m[0][2], m[0][1] - m[1][0]].map(f64::from);
    #[allow(clippy::cast_possible_truncation)]
    let scaled = |s: f64| v.map(|x| (x * s) as f32);
    if cos > 0.5 {
        let sin2 = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if sin2 == 0.0 {
            return [0.0; 3];
        }
        return scaled((sin2 * 0.5).asin() / sin2);
    }
    let angle = cos.clamp(-1.0, 1.0).acos();
    let sin = angle.sin();
    if sin.abs() < 1e-6 {
        return [0.0; 3];
    }
    scaled(angle / (2.0 * sin))
}

/// `SaveOutfitStudioPose`: one layer as Outfit Studio's pose XML (negated rotation vectors).
pub fn save_outfit_studio_pose(dir: &Path, layer: &Adjustment, name: &str) -> Result<PathBuf, String> {
    let mut xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<PoseData>\n\t<Pose name=\"{name}\">\n");
    for (bone, d) in layer.bones.iter().filter(|(_, d)| !d.is_identity()) {
        let v = rotation_vector(&rotation_from_degrees(d.rotation));
        let _ = writeln!(xml, "\t\t<Bone name=\"{bone}\" rotX=\"{}\" rotY=\"{}\" rotZ=\"{}\" transX=\"{}\" transY=\"{}\" transZ=\"{}\" scale=\"{}\"/>", -v[0], -v[1], -v[2], d.position[0], d.position[1], d.position[2], d.scale);
    }
    xml.push_str("\t</Pose>\n</PoseData>\n");
    let path = dir.join(format!("{name}.xml"));
    std::fs::write(&path, xml).map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(path)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn u32_or(json: &Json, key: &str, default: u32) -> u32 {
    match json.get(key) {
        Some(Json::Number(v)) if *v >= 0.0 => *v as u32,
        _ => default,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn i32_or(json: &Json, key: &str, default: i32) -> i32 {
    match json.get(key) {
        Some(Json::Number(v)) => *v as i32,
        _ => default,
    }
}

fn light_json(light: &Light, key: &dyn Fn(u32) -> String) -> Json {
    let index = |i: u32| Json::Number(f64::from(i));
    object(vec![
        ("form", Json::Text(key(light.base))),
        ("name", Json::Text(light.name.clone())),
        ("spot", Json::Bool(light.spot)),
        ("kind", index(lights::index_of(&Kind::ALL, &light.kind))),
        ("animation", index(lights::index_of(&Animation::ALL, &light.animation))),
        ("role", index(lights::index_of(&Role::ALL, &light.role))),
        ("attach", index(lights::index_of(&Attach::ALL, &light.attach))),
        ("shadows", Json::Bool(light.shadows)),
        ("negative", Json::Bool(light.negative)),
        ("bone", Json::Text(light.bone.clone())),
        ("softbox", Json::Bool(light.softbox)),
        ("softWidth", num(light.soft_width)),
        ("softHeight", num(light.soft_height)),
        ("softColumns", Json::Number(f64::from(light.soft_columns))),
        ("softRows", Json::Number(f64::from(light.soft_rows))),
        ("enabled", Json::Bool(light.enabled)),
        ("angle", num(light.angle)),
        ("distance", num(light.distance)),
        ("height", num(light.height)),
        ("aimX", num(light.aim_x)),
        ("aimY", num(light.aim_y)),
        ("color", triple(light.color)),
        ("fade", num(light.fade)),
        ("radius", num(light.radius)),
    ])
}

/// `LightsToJson`: the rig body light rig files and scenes share; `key` names a base form by plugin.
fn rig_json(rig: &Rig, key: &dyn Fn(u32) -> String) -> Vec<(&'static str, Json)> {
    vec![("rigRotation", num(rig.rotation)), ("rigOffset", triple(rig.offset)), ("lights", Json::Array(rig.lights.iter().map(|l| light_json(l, key)).collect()))]
}

/// What a saved light's form key comes back as: the form's runtime id and its flags (the form the
/// load order offers instead when the key's plugin is gone), `None` to skip the light.
pub type ResolveLight<'a> = &'a dyn Fn(&str, bool) -> Option<(u32, u32)>;

/// `LightsFromJson`: `None` without a `lights` array. Every light is staged to be placed; the form
/// that resolved decides whether it aims and casts shadows. Rigs from before kinds carry only
/// `spot`; the old third kind (2) was a negative point.
fn rig_from_json(json: &Json, resolve: ResolveLight) -> Option<Rig> {
    let Some(Json::Array(entries)) = json.get("lights") else { return None };
    let mut rig = Rig { rotation: f32_or(json, "rigRotation", 0.0), ..Rig::default() };
    read_triple(json, "rigOffset", &mut rig.offset);
    for entry in entries.iter().filter(|e| matches!(e, Json::Object(_))) {
        let spot = bool_or(entry, "spot", false);
        let kind = u32_or(entry, "kind", u32::from(spot));
        let Some((base, flags)) = resolve(&text_or(entry, "form", ""), spot) else { continue };
        let enumerated = |key: &str, len: usize| Some(u32_or(entry, key, 0) as usize).filter(|i| *i < len).unwrap_or(0);
        let mut light = Light {
            base,
            spot: lights::is_spot(flags),
            shadows: lights::casts_shadows(flags),
            negative: bool_or(entry, "negative", kind == 2),
            kind: Kind::ALL.get(kind as usize).copied().unwrap_or(Kind::Point),
            animation: Animation::ALL[enumerated("animation", Animation::ALL.len())],
            role: Role::ALL[enumerated("role", Role::ALL.len())],
            attach: Attach::ALL[enumerated("attach", Attach::ALL.len())],
            bone: text_or(entry, "bone", ""),
            softbox: bool_or(entry, "softbox", false),
            soft_width: f32_or(entry, "softWidth", 160.0),
            soft_height: f32_or(entry, "softHeight", 120.0),
            soft_columns: i32_or(entry, "softColumns", 3),
            soft_rows: i32_or(entry, "softRows", 2),
            name: text_or(entry, "name", ""),
            enabled: bool_or(entry, "enabled", true),
            angle: f32_or(entry, "angle", 0.0),
            distance: f32_or(entry, "distance", 120.0),
            height: f32_or(entry, "height", 120.0),
            aim_x: f32_or(entry, "aimX", 0.0),
            aim_y: f32_or(entry, "aimY", 0.0),
            fade: f32_or(entry, "fade", 1.0),
            radius: f32_or(entry, "radius", 256.0),
            spawn_requested: true,
            ..Light::default()
        };
        read_triple(entry, "color", &mut light.color);
        if light.name.is_empty() {
            light.name = format!("{} {}", if light.spot { "Spot" } else { "Point" }, rig.lights.len() + 1);
        }
        rig.lights.push(light);
    }
    Some(rig)
}

/// `SaveLightRig`: `<dir>\<name>.json`.
pub fn save_light_rig(dir: &Path, rig: &Rig, name: &str, key: &dyn Fn(u32) -> String) -> Result<PathBuf, String> {
    let mut fields = rig_json(rig, key);
    fields.push(("version", Json::Number(FILE_VERSION)));
    fields.push(("type", Json::Text("lightrig".into())));
    let path = dir.join(format!("{name}.json"));
    write(&path, &object(fields)).map(|()| path)
}

/// `LoadLightRig`.
pub fn load_light_rig(dir: &Path, name: &str, resolve: ResolveLight) -> Result<Rig, String> {
    let path = dir.join(format!("{name}.json"));
    rig_from_json(&read(&path)?, resolve).ok_or_else(|| format!("Invalid light rig: {}", path.display()))
}

/// The sun override a scene carries: compass azimuth and elevation, degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSun {
    pub azimuth: f32,
    pub elevation: f32,
}

/// `ResolveFormKey`'s parse: a `Plugin|LOCALID` key's plugin and local id.
#[must_use]
pub fn parse_form_key(key: &str) -> Option<(&str, u32)> {
    let (plugin, local) = key.split_once('|')?;
    Some((plugin, u32::from_str_radix(local, 16).ok()?))
}

/// One actor of a scene (`Scene::Actor`): keyed by plugin and local id, so the scene loads into
/// another save; its non-persistent layers with their flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneActor {
    pub key: String,
    /// Shown only; the key decides.
    pub name: String,
    pub frozen: bool,
    pub collision_off: bool,
    pub hidden_gear: u32,
    pub face: Option<FaceState>,
    pub layers: Vec<Adjustment>,
}

/// A whole shot: the camera, the hour, the forced weather (a form key), the sun, the actors and
/// the light rig.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub camera: CameraBookmark,
    pub hour: Option<f32>,
    pub weather: Option<String>,
    pub sun: Option<SceneSun>,
    pub actors: Vec<SceneActor>,
    pub rig: Rig,
}

fn scene_actor_json(actor: &SceneActor) -> Json {
    let mut fields = vec![
        ("actor", Json::Text(actor.key.clone())),
        ("name", Json::Text(actor.name.clone())),
        ("frozen", Json::Bool(actor.frozen)),
        ("collisionOff", Json::Bool(actor.collision_off)),
        ("hiddenGear", Json::Number(f64::from(actor.hidden_gear))),
    ];
    if let Some(f) = &actor.face {
        fields.push(("face", object(face_json(f))));
    }
    fields.push(("layers", Json::Array(actor.layers.iter().map(layer_json).collect())));
    object(fields)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scene_actor(json: &Json) -> Option<SceneActor> {
    let key = text_or(json, "actor", "");
    if key.is_empty() {
        return None;
    }
    let layers = match json.get("layers") {
        Some(Json::Array(items)) => items.iter().filter(|l| matches!(l, Json::Object(_))).map(layer).collect(),
        _ => Vec::new(),
    };
    let hidden_gear = match json.get("hiddenGear") {
        Some(Json::Number(v)) => *v as u32,
        _ => 0,
    };
    let face = json.get("face").filter(|f| matches!(f, Json::Object(_))).map(face);
    Some(SceneActor { key, name: text_or(json, "name", ""), frozen: bool_or(json, "frozen", false), collision_off: bool_or(json, "collisionOff", false), hidden_gear, face, layers })
}

/// `SaveScene`; `key` names the lights' base forms by plugin.
pub fn save_scene(dir: &Path, scene: &Scene, name: &str, key: &dyn Fn(u32) -> String) -> Result<PathBuf, String> {
    let mut fields = rig_json(&scene.rig, key);
    fields.extend([("version", Json::Number(FILE_VERSION)), ("type", Json::Text("scene".into())), ("camera", camera_json(&scene.camera, false))]);
    if let Some(hour) = scene.hour {
        fields.push(("hour", num(hour)));
    }
    if let Some(weather) = &scene.weather {
        fields.push(("weather", Json::Text(weather.clone())));
    }
    if let Some(sun) = scene.sun {
        fields.push(("sun", object(vec![("azimuth", num(sun.azimuth)), ("elevation", num(sun.elevation))])));
    }
    fields.push(("actors", Json::Array(scene.actors.iter().map(scene_actor_json).collect())));
    let path = dir.join(format!("{name}.json"));
    write(&path, &object(fields)).map(|()| path)
}

/// `LoadScene`: a scene without lights is legitimate, only the block is optional.
pub fn load_scene(dir: &Path, name: &str, resolve: ResolveLight) -> Result<Scene, String> {
    let path = dir.join(format!("{name}.json"));
    let json = read(&path)?;
    if !matches!(json.get("type"), Some(Json::Text(t)) if t == "scene") {
        return Err(format!("Invalid scene file: {}", path.display()));
    }
    let mut scene = Scene {
        camera: json.get("camera").map(|c| camera(c, "")).unwrap_or_default(),
        hour: as_f32(json.get("hour")).filter(|h| *h >= 0.0),
        weather: Some(text_or(&json, "weather", "")).filter(|w| !w.is_empty()),
        actors: match json.get("actors") {
            Some(Json::Array(items)) => items.iter().filter_map(scene_actor).collect(),
            _ => Vec::new(),
        },
        rig: rig_from_json(&json, resolve).unwrap_or_default(),
        ..Scene::default()
    };
    if let Some(sun) = json.get("sun").filter(|s| matches!(s, Json::Object(_))) {
        scene.sun = Some(SceneSun { azimuth: f32_or(sun, "azimuth", 0.0), elevation: f32_or(sun, "elevation", 0.0) });
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use skyrim::render::Transform;

    use super::*;
    use skyrim::render::from_euler_xyz as euler_xyz;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ic-studio-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn adjustments_round_trip_without_identity_bones() {
        let dir = temp("adjustment");
        let mut a = Adjustment { name: "Hands up".into(), ..Adjustment::default() };
        a.bones.insert("NPC R Hand [RHnd]".into(), BoneDelta { position: [0.1, 0.0, -2.5], rotation: [10.0, 0.0, 45.5], scale: 1.25 });
        a.bones.insert("NPC Head [Head]".into(), BoneDelta::default());
        save_adjustment(&dir, &a, "Hands up").unwrap();
        let back = load_adjustment(&dir, "Hands up").unwrap();
        assert_eq!(back.name, "Hands up");
        assert_eq!(back.bones.len(), 1);
        assert_eq!(back.bones["NPC R Hand [RHnd]"], a.bones["NPC R Hand [RHnd]"]);
        assert_eq!(list_files(&dir), ["Hands up"]);
        assert!(load_adjustment(&dir, "missing").is_err());
        std::fs::write(dir.join("bad.json"), "{\"name\": 1}").unwrap();
        assert!(load_adjustment(&dir, "bad").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn poses_keep_the_exact_matrix_and_fall_back_on_euler() {
        let dir = temp("pose");
        let mut locals = BTreeMap::new();
        let t = Transform { rotation: euler_xyz(0.25, -0.5, 1.0), translation: [1.0, 2.0, 3.0], scale: 0.9 };
        locals.insert("NPC Spine [Spn0]".to_owned(), t);
        save_pose(&dir, "Stand", &locals).unwrap();
        assert_eq!(load_pose(&dir, "Stand").unwrap()["NPC Spine [Spn0]"], t);
        std::fs::write(dir.join("Euler.json"), r#"{"bones": {"B": {"pos": [1, 2, 3], "rot": [0, 0, 90], "scale": 2}}}"#).unwrap();
        let b = load_pose(&dir, "Euler").unwrap()["B"];
        assert!((b.rotation[0][1] - 1.0).abs() < 1e-6 && (b.scale - 2.0).abs() < f32::EPSILON && b.translation.iter().zip([1.0, 2.0, 3.0]).all(|(a, e)| (a - e).abs() < f32::EPSILON));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sam_files_negate_rotations_in_json_and_yaml() {
        let dir = temp("sam");
        std::fs::create_dir_all(dir.join("Pack/Sub")).unwrap();
        std::fs::write(dir.join("loose.json"), r#"{"rotation": 2054781029, "transforms": {"NPC Head [Head]": {"rx": 10, "ry": "-5.5", "rz": 0, "tx": 1, "s": 1}, "Empty": {}}}"#).unwrap();
        std::fs::write(dir.join("Pack/Sub/kneel.yaml"), "type: pose\nrotation: exyz\ntransforms:\n  'NPC L Foot [Lft ]':\n    rx: 30 # comment\n    s: 1.1\n    o: 1\n  NPC Pelvis [Pelv]:\n    tz: -12\n").unwrap();
        std::fs::write(dir.join("Pack/readme.txt"), "").unwrap();
        let packs = list_sam_packs(&dir);
        assert_eq!(packs, [SamPack { name: String::new(), files: vec!["loose.json".into()] }, SamPack { name: "Pack".into(), files: vec!["Sub/kneel.yaml".into()] }]);
        let mut notes = Vec::new();
        let json = load_sam_file(&dir, "", "loose.json", &mut notes).unwrap();
        assert_eq!(json.name, "[SAM] loose");
        assert_eq!(json.bones.len(), 1, "identity bones are dropped");
        assert_eq!(json.bones["NPC Head [Head]"], BoneDelta { position: [1.0, 0.0, 0.0], rotation: [-10.0, 5.5, -0.0], scale: 1.0 });
        assert!(notes.is_empty(), "0x7A797865 is exyz");
        let yaml = load_sam_file(&dir, "Pack", "Sub/kneel.yaml", &mut notes).unwrap();
        assert_eq!(yaml.bones["NPC L Foot [Lft ]"], BoneDelta { rotation: [-30.0, 0.0, 0.0], scale: 1.1, ..BoneDelta::default() });
        assert_eq!(yaml.bones["NPC Pelvis [Pelv]"].position, [0.0, 0.0, -12.0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bookmarks_and_scenes_round_trip() {
        let dir = temp("camera");
        assert_eq!(load_camera_bookmarks(&dir), []);
        let shots = vec![CameraBookmark { name: "Shot 1".into(), position: [10.0, -20.5, 300.0], pitch: 0.1, yaw: 3.0, roll: -5.0, fov: 55.0 }, CameraBookmark { name: "Wide".into(), ..CameraBookmark::default() }];
        save_camera_bookmarks(&dir, &shots).unwrap();
        assert_eq!(load_camera_bookmarks(&dir), shots);
        let layer = Adjustment { name: "Lean".into(), file: "lean".into(), enabled: false, is_pose: true, persistent: false, include_in_pose: false, bones: BTreeMap::from([("NPC Head [Head]".to_owned(), BoneDelta { rotation: [5.0, 0.0, -10.0], ..BoneDelta::default() })]) };
        let smile = FaceState { expression: 10, expression_value: 0.75, eyes_override: true, eyes_heading: -0.25, persistent: true, tongue: [("PA Tongue".to_owned(), BoneDelta { position: [0.0, 0.5, 0.0], ..BoneDelta::default() })].into(), ..FaceState::default() };
        let lydia = SceneActor { key: "Skyrim.esm|000A2C94".into(), name: "Lydia".into(), frozen: true, collision_off: true, hidden_gear: 0b10_0001, face: Some(smile), layers: vec![layer] };
        let rig = Rig { rotation: 15.0, offset: [0.0, 10.0, -5.0], lights: vec![key_light()] };
        let scene = Scene { camera: CameraBookmark { name: String::new(), ..shots[0].clone() }, hour: Some(18.5), weather: Some("Skyrim.esm|0000081A".into()), sun: Some(SceneSun { azimuth: 135.0, elevation: 20.0 }), actors: vec![lydia], rig };
        save_scene(&dir, &scene, "Dusk", &form_key).unwrap();
        assert_eq!(load_scene(&dir, "Dusk", &resolve).unwrap(), scene);
        std::fs::write(dir.join("Ic.json"), r#"{"type": "scene", "camera": {"fov": 40}, "hour": 6.5, "actors": [{"actor": "Skyrim.esm|00000007", "face": {}}, {"name": "no key"}]}"#).unwrap();
        let ic = load_scene(&dir, "Ic", &resolve).unwrap();
        assert!((ic.camera.fov - 40.0).abs() < f32::EPSILON && ic.sun.is_none() && ic.hour == Some(6.5) && ic.weather.is_none() && ic.rig == Rig::default());
        assert_eq!(ic.actors, [SceneActor { key: "Skyrim.esm|00000007".into(), face: Some(FaceState::default()), ..SceneActor::default() }], "an actor without a key is skipped");
        assert_eq!(parse_form_key("Skyrim.esm|0000081A"), Some(("Skyrim.esm", 0x81a)));
        assert_eq!(parse_form_key("Skyrim.esm"), None);
        std::fs::write(dir.join("Pose.json"), r#"{"type": "pose"}"#).unwrap();
        assert!(load_scene(&dir, "Pose", &resolve).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Form keys as `Plugin|LOCALID` of a test load order holding `Skyrim.esm` only: spot forms
    /// have bit 12 set in their id, every other plugin falls back to a point form.
    fn form_key(id: u32) -> String {
        format!("Skyrim.esm|{id:08X}")
    }

    fn resolve(key: &str, spot: bool) -> Option<(u32, u32)> {
        match parse_form_key(key) {
            Some(("Skyrim.esm", id)) => Some((id, if id & 0x1000 != 0 { 0x400 } else { 0 })),
            _ if spot => None,
            _ => Some((0x99, 0)),
        }
    }

    fn key_light() -> Light {
        Light { base: 0x1001, name: "Key".into(), spot: true, shadows: true, kind: Kind::Spot, animation: Animation::Pulse, role: Role::Key, attach: Attach::Bone, bone: "NPC Head [Head]".into(), angle: 45.0, distance: 130.0, height: 150.0, aim_x: 2.5, aim_y: -1.0, color: [1.0, 0.95, 0.86], fade: 0.75, radius: 420.0, softbox: true, soft_width: 200.0, soft_height: 90.0, soft_columns: 4, soft_rows: 1, enabled: false, negative: true, spawn_requested: true, ..Light::default() }
    }

    #[test]
    fn light_rigs_round_trip_and_read_older_files() {
        let dir = temp("lights");
        let rig = Rig { rotation: -30.0, offset: [1.0, 2.0, 3.0], lights: vec![key_light(), Light { base: 0x22, name: "Fill".into(), spawn_requested: true, ..Light::default() }] };
        save_light_rig(&dir, &rig, "Portrait", &form_key).unwrap();
        assert_eq!(load_light_rig(&dir, "Portrait", &resolve).unwrap(), rig);
        let live = Rig { lights: vec![Light { id: 4, handle: skyrim::actors::RefHandle(0x0010_0001), extra: vec![skyrim::actors::RefHandle(0x0010_0002)], world_position: [5.0; 3], ..key_light() }], ..rig.clone() };
        save_light_rig(&dir, &live, "Live", &form_key).unwrap();
        assert_eq!(load_light_rig(&dir, "Live", &resolve).unwrap().lights[0], key_light(), "only the rig's data is kept");
        std::fs::write(dir.join("Old.json"), r#"{"lights": [{"form": "Gone.esp|00000001", "spot": true}, {"form": "Gone.esp|00000002", "kind": 2, "name": ""}, {"form": "Skyrim.esm|00001003", "kind": 7, "animation": 9, "attach": 3, "color": [0.5, "x", 0.25]}, 3]}"#).unwrap();
        let old = load_light_rig(&dir, "Old", &resolve).unwrap();
        assert_eq!(old.lights.len(), 2, "a spot whose form is gone and has no fallback is skipped");
        assert_eq!((old.lights[0].base, old.lights[0].negative, old.lights[0].kind, old.lights[0].name.as_str()), (0x99, true, Kind::Point, "Point 1"));
        let third = &old.lights[1];
        assert_eq!((third.spot, third.shadows, third.kind, third.animation, third.attach, third.color), (true, true, Kind::Point, Animation::None, Attach::World, [0.5, 1.0, 0.25]));
        std::fs::write(dir.join("Empty.json"), r#"{"type": "lightrig"}"#).unwrap();
        assert!(load_light_rig(&dir, "Empty", &resolve).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expressions_round_trip_and_sam_face_morphs_are_found() {
        let dir = temp("face");
        let face = FaceState { expression: 2, expression_value: 0.5, modifiers: [0.25; 14], suppress_blink: false, ..FaceState::default() };
        save_expression(&dir, &face, "Grin").unwrap();
        assert_eq!(load_expression(&dir, "Grin").unwrap(), face);
        std::fs::create_dir_all(dir.join("Pack")).unwrap();
        std::fs::write(dir.join("Pack/smile.YML"), "phoneme:\n  Aah: 50\n").unwrap();
        std::fs::write(dir.join("frown.yaml"), "").unwrap();
        assert_eq!(list_sam_expressions(&dir), ["Pack/smile", "frown"]);
        let loaded = load_sam_expression(&dir, "Pack/smile", &crate::camera::studio::face::tests::names(), &face).unwrap();
        assert!((loaded.phonemes[0] - 0.5).abs() < f32::EPSILON && loaded.expression == 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outfit_studio_poses_write_rotation_vectors_and_names_are_sanitized() {
        let dir = temp("os");
        let mut layer = Adjustment::default();
        layer.bones.insert("NPC Head [Head]".into(), BoneDelta { rotation: [0.0, 0.0, 90.0], ..BoneDelta::default() });
        let path = save_outfit_studio_pose(&dir, &layer, "Tilt").unwrap();
        let xml = std::fs::read_to_string(path).unwrap();
        assert!(xml.contains("<Bone name=\"NPC Head [Head]\" rotX=\"-0\" rotY=\"-0\" rotZ=\""), "{xml}");
        let v = rotation_vector(&euler_xyz(0.0, 0.0, 0.5));
        assert!(v[0].abs() < 1e-6 && (v[2].abs() - 0.5).abs() < 1e-5, "{v:?}");
        assert_eq!(sanitize_file_name("a/b:c d-e_f.json"), "abc d-e_fjson");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
