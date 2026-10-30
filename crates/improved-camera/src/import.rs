//! Imports IC's own files into the profiles: `ImprovedCameraSE.json` (`PluginConfig`, the active
//! profile) and `Profiles\<name>.json` (`PluginProfile`) in `Data\SKSE\Plugins\ImprovedCameraSE\`,
//! where `plugin.cpp` reads them. IC writes them with nlohmann's bindings, every struct an array of
//! one object (`{"General": [{"bEnableBody": true}]}`), which maps onto the profile keys by joining
//! the member names (`General.bEnableBody`). IC's files are only read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use framework::settings::{Json, Setting, parse_json};

use crate::profiles;
use crate::settings::{Memory, Profile, Store};

/// IC's keys that are the host's hotkeys here, with the action that holds them.
const KEYS: [(&str, &str); 7] = [
    ("Effects.Zoom.iKey", "improved-camera.zoom"),
    ("FreeCamera.iKeyRollLeft", "improved-camera.free-camera-roll-left"),
    ("FreeCamera.iKeyRollRight", "improved-camera.free-camera-roll-right"),
    ("FreeCamera.iKeyReset", "improved-camera.free-camera-reset"),
    ("Studio.iToggleInputKey", "improved-camera.photo-mode-input"),
    ("Studio.iCaptureKey", "improved-camera.photo-mode-capture"),
    ("Studio.iHideUIKey", "skyrim-rs.toggle"),
];

/// What an import did.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub imported: Vec<String>,
    /// The profile IC's configuration names as active, when it was imported.
    pub active: Option<String>,
    /// Every key or file left out, and why.
    pub notes: Vec<String>,
}

/// IC's folder beside the game: `plugin.cpp` builds it from the working directory.
#[must_use]
pub fn ic_directory() -> Option<PathBuf> {
    Some(std::env::current_dir().ok()?.join(r"Data\SKSE\Plugins\ImprovedCameraSE"))
}

/// Every scalar of an IC document under its joined key; arrays of one object are IC's structs.
fn flatten(key: &str, json: &Json, out: &mut Vec<(String, Json)>) {
    let join = |name: &str| if key.is_empty() { name.to_owned() } else { format!("{key}.{name}") };
    match json {
        Json::Object(members) => members.iter().for_each(|(name, value)| flatten(&join(name), value, out)),
        Json::Array(items) if items.len() == 1 => flatten(key, &items[0], out),
        other => out.push((key.to_owned(), other.clone())),
    }
}

/// The profile in one IC profile document, IC's defaults where it says nothing; `notes` gets every
/// key that is not taken, named after `file`.
#[must_use]
pub fn profile(json: &Json, file: &str, notes: &mut Vec<String>) -> Profile {
    let known = Memory::default();
    Profile::default().write(&known);
    let staged = Memory::default();
    let mut values = Vec::new();
    flatten("", json, &mut values);
    for (key, value) in values {
        let setting = match &value {
            Json::Bool(v) => Some(Setting::Bool(*v)),
            Json::Number(v) => Some(Setting::Number(*v)),
            _ => None,
        };
        match (known.get(&key), setting) {
            (Some(default), Some(setting)) if core::mem::discriminant(&default) == core::mem::discriminant(&setting) => staged.set(&key, setting),
            (Some(_), _) => notes.push(format!("{file}: {key} is {value:?}, not the type IC writes; the default is kept")),
            (None, _) if key == "version" || key == "name" => {}
            (None, _) => match KEYS.iter().find(|(ic, _)| *ic == key) {
                Some((_, action)) => notes.push(format!("{file}: {key} = {value:?} is a hotkey here: bind {action} in the host's Settings tab")),
                None => notes.push(format!("{file}: unknown option {key} ignored")),
            },
        }
    }
    Profile::read(&staged)
}

/// The active profile's name in IC's `ImprovedCameraSE.json`.
fn config_active(json: &Json) -> Option<String> {
    let mut values = Vec::new();
    flatten("", json, &mut values);
    values.into_iter().find_map(|(key, value)| match value {
        Json::Text(name) if key == "Profile.active" => Some(name),
        _ => None,
    })
}

/// Reads IC's configuration and every profile in `dir` into `store`'s saved profiles, replacing
/// those of the same name. A file that cannot be read or parsed, or whose name cannot be a
/// profile's, is noted and skipped; `Err` only when `dir` cannot be listed.
pub fn import(dir: &Path, store: &impl Store) -> Result<Report, String> {
    let read = |path: &Path| std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| parse_json(&text));
    let mut report = Report::default();
    let config = dir.join("ImprovedCameraSE.json");
    let active = match read(&config) {
        Ok(json) => config_active(&json),
        Err(e) => {
            report.notes.push(format!("{}: {e}", config.display()));
            None
        }
    };
    let profiles_dir = dir.join("Profiles");
    let entries = std::fs::read_dir(&profiles_dir).map_err(|e| format!("{}: {e}", profiles_dir.display()))?;
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            files.insert(stem.to_owned(), path);
        }
    }
    for (name, path) in files {
        let file = path.file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        if name.is_empty() || name.contains('.') {
            report.notes.push(format!("{file}: a profile name cannot hold '.'; skipped"));
            continue;
        }
        match read(&path) {
            Ok(json) => {
                let profile = profile(&json, &file, &mut report.notes);
                profiles::save(store, &name, &profile);
                report.imported.push(name);
            }
            Err(e) => report.notes.push(format!("{file}: {e}; skipped")),
        }
    }
    report.active = active.filter(|a| report.imported.contains(a));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Offset;

    /// A trimmed `Profiles\Default.json` as IC writes it, with an edited value in each kind of
    /// nesting, an option this port does not know, hotkeys and the studio.
    const PROFILE: &str = r#"{
    "Advanced": [{ "bHeadbob": true, "fNearDistanceMin": 6.0, "fForwardBumperOffset": 40.0 }],
    "Body": [{
        "Action": [{ "Sneak": [{ "fForward": 1.5, "fRight": 0.0, "fUp": -2.0 }], "fLerp": 10.0 }],
        "Combat": [{ "fForward": 3.0, "fRight": 0.0, "fUp": 0.0 }],
        "Race": [{ "HalfDragon": [{ "fForward": 0.0, "fRight": 0.5, "fUp": 1.0 }] }],
        "bFreeLook": false,
        "fAnchorForward": 12.0
    }],
    "Camera": [{
        "Firstperson": [{ "Combat": [{ "forward": 2.0, "right": 0.0, "up": 1.0 }], "forward": 4.0, "right": 0.0, "up": 0.0 }],
        "Spectator": [{ "forward": 7.0, "right": 0.0, "up": 5.0 }]
    }],
    "Dialogue": [{ "bEnable": true, "fFadeSpeed": 6.0 }],
    "Effects": [{ "Letterbox": [{ "bEnable": true, "bOverlay": false }], "Zoom": [{ "fFOV": -20.0, "iKey": 90 }] }],
    "FOV": [{ "fFirstPerson": 100.0, "fFirstPersonHands": "wide" }],
    "FreeCamera": [{ "bEnable": true, "iKeyReset": 82, "iKeyRollLeft": 81, "iKeyRollRight": 69 }],
    "General": [{ "bEnableBody": true, "bEnableHead": true, "bShowCape": true }],
    "Stabilization": [{ "Horse": [{ "bEnable": false, "fHeadFollowStrength": 0.5, "fSmoothingMaxOffset": 6.0, "fSmoothingSpeed": 14.0 }] }],
    "Studio": [{ "bHideHUD": true, "fUIScale": 1.35, "iCaptureKey": 121 }],
    "Vanity": [{ "iCollisionPullInMethod": 4, "fFocusChance": 50.0 }],
    "name": "Default",
    "version": 1
}"#;

    const CONFIG: &str = r#"{ "Menu": [{ "enabled": true, "font": "Jost/Jost-Regular.ttf", "fontsize": 18.0 }], "Profile": [{ "active": "Combat" }], "version": 1 }"#;

    #[test]
    fn maps_every_option_by_ic_s_name() {
        let mut notes = Vec::new();
        let p = profile(&parse_json(PROFILE).unwrap(), "Default.json", &mut notes);
        assert!(p.Advanced.bHeadbob && !p.Advanced.bHeadbobCombat);
        assert!((p.Advanced.fNearDistanceMin - 6.0).abs() < f32::EPSILON && (p.Advanced.fForwardBumperOffset - 40.0).abs() < f32::EPSILON);
        assert_eq!(p.Action.Sneak, Offset { fForward: 1.5, fUp: -2.0, fRight: 0.0 });
        assert!((p.Action.fLerp - 10.0).abs() < f32::EPSILON);
        assert_eq!(p.Body.Combat, Offset { fForward: 3.0, fUp: 0.0, fRight: 0.0 });
        assert_eq!(p.Race.HalfDragon, Offset { fForward: 0.0, fUp: 1.0, fRight: 0.5 });
        assert!(!p.Body.bFreeLook && (p.Body.fAnchorForward - 12.0).abs() < f32::EPSILON);
        assert!((p.Firstperson.forward - 4.0).abs() < f32::EPSILON && (p.Firstperson.Combat.up - 1.0).abs() < f32::EPSILON);
        assert!((p.Camera.Spectator.forward - 7.0).abs() < f32::EPSILON);
        assert!(p.Dialogue.bEnable && (p.Dialogue.fFadeSpeed - 6.0).abs() < f32::EPSILON);
        assert!(p.Letterbox.bEnable && (p.Zoom.fFOV + 20.0).abs() < f32::EPSILON);
        assert!((p.FOV.fFirstPerson - 100.0).abs() < f32::EPSILON && (p.FOV.fFirstPersonHands - 80.0).abs() < f32::EPSILON);
        assert!(p.General.bEnableHead && p.General.bEnableBody);
        assert!(!p.Stabilization.Horse.bEnable && (p.Stabilization.Horse.fHeadFollowStrength - 0.5).abs() < f32::EPSILON);
        assert_eq!(p.Vanity.iCollisionPullInMethod, 4);
        assert!(p.Studio.bHideHUD && p.Studio.bRevertOnExit);
        assert!((p.Vanity.fFocusChance - 50.0).abs() < f32::EPSILON && (p.Vanity.fDistance - 200.0).abs() < f32::EPSILON);
        assert_eq!(
            notes,
            [
                "Default.json: Effects.Zoom.iKey = Number(90.0) is a hotkey here: bind improved-camera.zoom in the host's Settings tab",
                "Default.json: FOV.fFirstPersonHands is Text(\"wide\"), not the type IC writes; the default is kept",
                "Default.json: FreeCamera.iKeyReset = Number(82.0) is a hotkey here: bind improved-camera.free-camera-reset in the host's Settings tab",
                "Default.json: FreeCamera.iKeyRollLeft = Number(81.0) is a hotkey here: bind improved-camera.free-camera-roll-left in the host's Settings tab",
                "Default.json: FreeCamera.iKeyRollRight = Number(69.0) is a hotkey here: bind improved-camera.free-camera-roll-right in the host's Settings tab",
                "Default.json: unknown option General.bShowCape ignored",
                "Default.json: Studio.iCaptureKey = Number(121.0) is a hotkey here: bind improved-camera.photo-mode-capture in the host's Settings tab",
            ]
        );
    }

    /// IC's shipped `Profiles\Default.json` (disco's `assets`): every option it holds is known
    /// here and its values are this port's defaults.
    #[test]
    fn ic_s_shipped_default_profile_is_this_port_s_default() {
        let mut notes = Vec::new();
        let p = profile(&parse_json(include_str!("../fixtures/Default.json")).unwrap(), "Default.json", &mut notes);
        assert_eq!(p, Profile::default());
        assert_eq!(notes.len(), 7, "{notes:?}");
        assert!(notes.iter().all(|n| n.contains("is a hotkey here")), "{notes:?}");
    }

    #[test]
    fn imports_ic_s_folder_without_writing_to_it() {
        let dir = std::env::temp_dir().join(format!("ic-import-{}", std::process::id()));
        let profiles_dir = dir.join("Profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(dir.join("ImprovedCameraSE.json"), CONFIG).unwrap();
        std::fs::write(profiles_dir.join("Default.json"), PROFILE).unwrap();
        std::fs::write(profiles_dir.join("Combat.json"), "\u{feff}{\"FOV\": [{\"fFirstPersonCombat\": 70.0}]}").unwrap();
        std::fs::write(profiles_dir.join("Broken.json"), "{\"FOV\": [").unwrap();
        std::fs::write(profiles_dir.join("v1.2.json"), "{}").unwrap();
        std::fs::write(profiles_dir.join("notes.txt"), "x").unwrap();
        let before: Vec<_> = std::fs::read_dir(&profiles_dir).unwrap().map(|e| e.unwrap().metadata().unwrap().modified().unwrap()).collect();

        let store = Memory::default();
        let report = import(&dir, &store).unwrap();
        assert_eq!(report.imported, ["Combat", "Default"]);
        assert_eq!(report.active.as_deref(), Some("Combat"));
        assert!(report.notes.iter().any(|n| n == "Broken.json: line 1, column 10: expected a value; skipped"), "{:?}", report.notes);
        assert!(report.notes.iter().any(|n| n == "v1.2.json: a profile name cannot hold '.'; skipped"));
        assert_eq!(profiles::names(&store), ["Combat", "Default"]);
        let combat = profiles::load(&store, "Combat").unwrap();
        assert!((combat.FOV.fFirstPersonCombat - 70.0).abs() < f32::EPSILON && (combat.FOV.fFirstPerson - 80.0).abs() < f32::EPSILON);
        assert!(profiles::load(&store, "Default").unwrap().General.bEnableHead);

        let after: Vec<_> = std::fs::read_dir(&profiles_dir).unwrap().map(|e| e.unwrap().metadata().unwrap().modified().unwrap()).collect();
        assert_eq!(before, after);
        assert_eq!(std::fs::read_to_string(dir.join("ImprovedCameraSE.json")).unwrap(), CONFIG);

        std::fs::remove_file(dir.join("ImprovedCameraSE.json")).unwrap();
        let report = import(&dir, &store).unwrap();
        assert_eq!(report.active, None);
        assert!(report.notes[0].contains("ImprovedCameraSE.json"));
        assert!(import(&dir.join("missing"), &store).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
