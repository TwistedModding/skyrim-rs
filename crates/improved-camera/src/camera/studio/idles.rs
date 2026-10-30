//! `camera/studio/StudioIdles`: installed pose packs, read from `Data` (`PoserHotkeys` descriptors in
//! `SKSE\Plugins\poserhotkeys\poserdata\*.json`, then the FNIS lists
//! `meshes\actors\character\animations\<pack>\FNIS_<pack>_List.txt` of packs those leave out),
//! played by sending the pose's animation event into the target's graph.

use std::path::Path;

use framework::settings::{Json, parse_json};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoseSet {
    pub name: String,
    pub events: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PosePack {
    pub name: String,
    pub sets: Vec<PoseSet>,
    pub pose_count: usize,
}

/// The graph event that puts an actor in the static reference pose.
pub const A_POSE: &str = "IdleStaticPoseAStart";
/// The graph event that sends an actor back to its default state.
pub const RESET_IDLE: &str = "IdleForceDefaultState";

/// `Scan`: every pack under `data`, sorted by name.
#[must_use]
pub fn scan(data: &Path) -> Vec<PosePack> {
    let mut packs = Vec::new();
    scan_poser_data(&data.join(r"SKSE\Plugins\poserhotkeys\poserdata"), &mut packs);
    scan_fnis_lists(&data.join(r"meshes\actors\character\animations"), &mut packs);
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

/// One `PoserHotkeys` descriptor: `string.name` names the pack, every array of `stringList` but
/// `packnames` is a set of events, in name order as IC's JSON library keeps them.
fn poser_pack(json: &Json, stem: &str) -> Option<PosePack> {
    let Some(Json::Object(lists)) = json.get("stringList") else { return None };
    let name = match json.get("string").and_then(|s| s.get("name")) {
        Some(Json::Text(n)) => n.clone(),
        _ => stem.to_owned(),
    };
    let mut pack = PosePack { name, ..PosePack::default() };
    for (set, events) in lists {
        let Json::Array(events) = events else { continue };
        if events.is_empty() || set.eq_ignore_ascii_case("packnames") {
            continue;
        }
        let events: Vec<String> = events.iter().filter_map(|e| if let Json::Text(t) = e { Some(t.clone()) } else { None }).collect();
        pack.pose_count += events.len();
        pack.sets.push(PoseSet { name: set.clone(), events });
    }
    pack.sets.sort_by(|a, b| a.name.cmp(&b.name));
    (pack.pose_count > 0).then_some(pack)
}

fn scan_poser_data(dir: &Path, packs: &mut Vec<PosePack>) {
    for path in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()) {
        if !path.is_file() || path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(pack) = std::fs::read_to_string(&path).ok().and_then(|t| parse_json(&t).ok()).and_then(|j| poser_pack(&j, &stem)) {
            packs.push(pack);
        }
    }
}

/// An FNIS list's pose events: lines `<type> [-options] <AnimEvent> <file.hkx> ...` of type `b`
/// (basic) or `o` (with objects).
fn fnis_events(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            if !matches!(tokens.next(), Some("b" | "o")) {
                return None;
            }
            let token = tokens.next()?;
            if token.starts_with('-') { tokens.next() } else { Some(token) }.map(str::to_owned)
        })
        .collect()
}

fn scan_fnis_lists(dir: &Path, packs: &mut Vec<PosePack>) {
    for path in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()) {
        let Some(name) = path.is_dir().then(|| path.file_name().map(|f| f.to_string_lossy().into_owned())).flatten() else { continue };
        if packs.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path.join(format!("FNIS_{name}_List.txt"))) else { continue };
        let events = fnis_events(&text);
        if !events.is_empty() {
            packs.push(PosePack { name, pose_count: events.len(), sets: vec![PoseSet { name: "All".to_owned(), events }] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PoseSet, fnis_events, scan};

    #[test]
    fn poser_descriptors_and_fnis_lists_become_packs() {
        let data = std::env::temp_dir().join(format!("ic-idles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let poser = data.join(r"SKSE\Plugins\poserhotkeys\poserdata");
        std::fs::create_dir_all(&poser).unwrap();
        std::fs::write(poser.join("zz.json"), r#"{"string": {"name": "Halo Poser"}, "stringList": {"packnames": ["x"], "Standing": ["HaloPose1", 7, "HaloPose2"], "Empty": []}}"#).unwrap();
        std::fs::write(poser.join("broken.json"), "{").unwrap();
        let anims = data.join(r"meshes\actors\character\animations");
        for pack in ["Halo Poser", "Kneel"] {
            std::fs::create_dir_all(anims.join(pack)).unwrap();
            std::fs::write(anims.join(pack).join(format!("FNIS_{pack}_List.txt")), "' comment\nb KneelPose1 kneel1.hkx\no -a,Tn KneelPose2 kneel2.hkx Prop\ns Skip skip.hkx\n").unwrap();
        }
        let packs = scan(&data);
        let _ = std::fs::remove_dir_all(&data);
        assert_eq!(packs.len(), 2, "the FNIS list of a pack the descriptors cover is skipped");
        assert_eq!(packs[0].name, "Halo Poser");
        assert_eq!(packs[0].sets, [PoseSet { name: "Standing".into(), events: vec!["HaloPose1".into(), "HaloPose2".into()] }]);
        assert_eq!((packs[1].name.as_str(), packs[1].pose_count), ("Kneel", 2));
        assert_eq!(fnis_events("b A a.hkx\no -o B b.hkx\nb\n"), ["A", "B"]);
    }
}
