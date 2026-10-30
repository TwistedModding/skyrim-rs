//! `camera/studio/StudioIO`: the studio's files. Its own live in a folder of the port's
//! (`Data\SKSE\Plugins\skyrim-rs\improved-camera\PhotoMode\`, with IC's layout: `Adjustments`,
//! `Poses`, `Scenes`, `Shots` and `Cameras.json`); Screen Archer Menu's pose and adjustment packs
//! under `Data\SAM\` are only read. JSON as IC writes it (tab-indented, `version` 1).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use framework::settings::{Json, parse_json};
use skyrim::render::Transform;

use super::adjustments::{Adjustment, BoneDelta, degrees_from_rotation, rotation_from_degrees};
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

/// The sun override a scene carries: compass azimuth and elevation, degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSun {
    pub azimuth: f32,
    pub elevation: f32,
}

/// What of IC's scene the port restores: the camera and the sun. IC's actors, lights, hour and
/// weather wait on RE request IC8; a file that holds them is still read, those blocks noted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub camera: CameraBookmark,
    pub sun: Option<SceneSun>,
}

pub fn save_scene(dir: &Path, scene: &Scene, name: &str) -> Result<PathBuf, String> {
    let mut fields = vec![("lights", Json::Array(Vec::new())), ("version", Json::Number(FILE_VERSION)), ("type", Json::Text("scene".into())), ("camera", camera_json(&scene.camera, false))];
    if let Some(sun) = scene.sun {
        fields.push(("sun", object(vec![("azimuth", num(sun.azimuth)), ("elevation", num(sun.elevation))])));
    }
    fields.push(("actors", Json::Array(Vec::new())));
    let path = dir.join(format!("{name}.json"));
    write(&path, &object(fields)).map(|()| path)
}

/// `LoadScene`; `notes` names the blocks the port does not restore.
pub fn load_scene(dir: &Path, name: &str, notes: &mut Vec<String>) -> Result<Scene, String> {
    let path = dir.join(format!("{name}.json"));
    let json = read(&path)?;
    if !matches!(json.get("type"), Some(Json::Text(t)) if t == "scene") {
        return Err(format!("Invalid scene file: {}", path.display()));
    }
    let mut scene = Scene { camera: json.get("camera").map(|c| camera(c, "")).unwrap_or_default(), ..Scene::default() };
    if let Some(sun) = json.get("sun").filter(|s| matches!(s, Json::Object(_))) {
        scene.sun = Some(SceneSun { azimuth: f32_or(sun, "azimuth", 0.0), elevation: f32_or(sun, "elevation", 0.0) });
    }
    for key in ["actors", "lights"] {
        if matches!(json.get(key), Some(Json::Array(a)) if !a.is_empty()) {
            notes.push(format!("{name}: the scene's {key} are not restored here"));
        }
    }
    for key in ["hour", "weather"] {
        if json.get(key).is_some() {
            notes.push(format!("{name}: the scene's {key} is not restored here"));
        }
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use skyrim::render::Transform;

    use super::*;
    use crate::camera::studio::adjustments::euler_xyz;

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
        assert!((b.rotation[0][1] + 1.0).abs() < 1e-6 && (b.scale - 2.0).abs() < f32::EPSILON && b.translation.iter().zip([1.0, 2.0, 3.0]).all(|(a, e)| (a - e).abs() < f32::EPSILON));
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
        let scene = Scene { camera: CameraBookmark { name: String::new(), ..shots[0].clone() }, sun: Some(SceneSun { azimuth: 135.0, elevation: 20.0 }) };
        save_scene(&dir, &scene, "Dusk").unwrap();
        let mut notes = Vec::new();
        assert_eq!(load_scene(&dir, "Dusk", &mut notes).unwrap(), scene);
        assert_eq!(notes, Vec::<String>::new());
        std::fs::write(dir.join("Ic.json"), r#"{"type": "scene", "camera": {"fov": 40}, "hour": 6.5, "actors": [{"actor": "Skyrim.esm|00000007"}], "lights": []}"#).unwrap();
        let ic = load_scene(&dir, "Ic", &mut notes).unwrap();
        assert!((ic.camera.fov - 40.0).abs() < f32::EPSILON && ic.sun.is_none());
        assert_eq!(notes, ["Ic: the scene's actors are not restored here", "Ic: the scene's hour is not restored here"]);
        std::fs::write(dir.join("Pose.json"), r#"{"type": "pose"}"#).unwrap();
        assert!(load_scene(&dir, "Pose", &mut notes).is_err());
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
