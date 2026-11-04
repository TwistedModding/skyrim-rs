//! A small hand-written ledger and difference files: no real game data.

use std::collections::BTreeMap;
use std::process::Command;

use super::*;
use crate::emit;
use crate::inputs::Symbol;

fn symbol(id: u64, rva: u64, name: &str, kind: &str) -> Symbol {
    Symbol { id: Some(id), rva, name: name.to_owned(), kind: kind.to_owned(), signature: String::new(), confidence: "high".to_owned(), evidence: "test".to_owned(), sig_confidence: String::new() }
}

fn ledger() -> Inputs {
    let layouts = r#"{
        "ActorValueOwner": {"size": 8, "size_conf": "high", "primary_base": null, "size_lower": 8, "size_upper": 8, "size_kind": "exact", "size_evidence": "test",
            "fields": [{"off": 0, "size": 8, "type": "vtbl:ActorValueOwner:0", "name": "__vftable"}]},
        "TESForm": {"size": 16, "size_conf": "high", "primary_base": null, "size_lower": 16, "size_upper": 16, "size_kind": "exact", "size_evidence": "test",
            "fields": [{"off": 0, "size": 8, "type": "vtbl:TESForm:0", "name": "__vftable"}, {"off": 8, "size": 4, "type": "u32", "name": "formID"}]},
        "Actor": {"size": 64, "size_conf": "high", "primary_base": "TESForm", "size_lower": 64, "size_upper": 64, "size_kind": "exact", "size_evidence": "test",
            "fields": [{"off": 0, "size": 8, "type": "vtbl:Actor:0", "name": "__vftable"}, {"off": 8, "size": 4, "type": "u32", "name": "formID"},
                       {"off": 16, "size": 8, "type": "vtbl:Actor:10", "name": "__vftable_10"}, {"off": 24, "size": 4, "type": "f32", "name": "health"},
                       {"off": 32, "size": 8, "type": "ptr:TESForm", "name": "process"}, {"off": 40, "size": 8, "type": "class:Inner", "name": "inner"},
                       {"off": 48, "size": 4, "type": "u32", "name": "stable"}]}
    }"#;
    let structs = r#"{"labels": [], "structs": {"Inner": {"size": 8, "description": "test", "fields": [{"off": 0, "size": 8, "type": "u64", "name": "value"}]}}}"#;
    let rtti = r#"{"classes": [
        {"name": "ActorValueOwner", "bases": [{"name": "ActorValueOwner", "mdisp": 0, "pdisp": -1}], "cols": [{"offset": 0, "vtable": 4096, "slots": [8192]}]},
        {"name": "TESForm", "bases": [{"name": "TESForm", "mdisp": 0, "pdisp": -1}], "cols": [{"offset": 0, "vtable": 4112, "slots": [8193]}]},
        {"name": "Actor", "bases": [{"name": "Actor", "mdisp": 0, "pdisp": -1}, {"name": "TESForm", "mdisp": 0, "pdisp": -1}, {"name": "ActorValueOwner", "mdisp": 16, "pdisp": -1}],
         "cols": [{"offset": 0, "vtable": 4128, "slots": [8193, 8194]}, {"offset": 16, "vtable": 4144, "slots": [8192]}]}
    ]}"#;
    let semantic = r#"{"subsystem": "actors", "flags": [], "globals": [],
        "enums": [{"name": "Stage", "width": 4, "meaning": "test", "values": [{"value": 0, "name": "First"}, {"value": 1, "name": "Gone"}, {"value": 2, "name": "Last"}]},
                  {"name": "Steady", "width": 4, "meaning": "test", "values": [{"value": 0, "name": "One"}]}],
        "classes": [{"name": "Actor", "meaning": "test", "confidence": "high", "fields": [{"offset": 32, "name": "process", "meaning": "test", "confidence": "high", "retyped": null}]}],
        "functions": [{"id": 300, "name": "Actor::Kill", "signature": "void Actor::Kill(Actor* this)", "meaning": "test", "confidence": "high", "retyped": null},
                      {"id": 302, "name": "Actor::SetStage", "signature": "Stage Actor::SetStage(Actor* this, Stage stage, Steady steady)", "meaning": "test", "confidence": "high", "retyped": null},
                      {"id": 301, "name": "Actor::ModValue", "signature": "void Actor::ModValue(ActorValueOwner* this, float amount)", "meaning": "test", "confidence": "high", "retyped": null}],
        "vtables": [{"class": "Actor", "slots": [{"index": 1, "name": "Update", "signature": "void (Actor* this)", "meaning": "test", "confidence": "high", "retyped": null}]}]
    }"#;
    Inputs {
        symbols: vec![
            symbol(100, 4096, "vtable ActorValueOwner", "vtable"),
            symbol(101, 4112, "vtable TESForm", "vtable"),
            symbol(102, 4128, "vtable Actor", "vtable"),
            symbol(103, 4144, "vtable Actor 10", "vtable"),
            symbol(300, 12288, "Actor::Kill", "function"),
            symbol(301, 12289, "Actor::ModValue", "function"),
            symbol(302, 12290, "Actor::SetStage", "function"),
        ],
        layouts: serde_json::from_str(layouts).unwrap(),
        structs: serde_json::from_str(structs).unwrap(),
        rtti: serde_json::from_str(rtti).unwrap(),
        formtypes: serde_json::from_str(r#"[{"id": 0, "sig": "NONE", "cls": null}]"#).unwrap(),
        semantic: vec![serde_json::from_str(semantic).unwrap()],
        hashes: Vec::new(),
    }
}

/// Rows for every ledger item at `same`, with `changes` replacing the rows they key.
fn file(runtime: &str, library: &str, database: &str, changes: &str) -> String {
    let same_fields = [("ActorValueOwner", "__vftable"), ("TESForm", "__vftable"), ("TESForm", "formID"), ("Actor", "__vftable"), ("Actor", "formID"), ("Actor", "__vftable_10"), ("Actor", "health"), ("Actor", "process"), ("Actor", "inner"), ("Actor", "stable"), ("Inner", "value")];
    let fields: Vec<String> = same_fields.iter().map(|(c, n)| format!(r#"["{c}","{n}",null,"same"]"#)).collect();
    let classes: Vec<String> = ["ActorValueOwner", "TESForm", "Actor", "Inner"].iter().map(|c| format!(r#"["{c}","same",null,"same"]"#)).collect();
    let bases = [("Actor", "TESForm"), ("Actor", "ActorValueOwner")].map(|(c, b)| format!(r#"["{c}","{b}",null,"same"]"#));
    let slots = [("ActorValueOwner", 0, 0), ("TESForm", 0, 0), ("Actor", 0, 0), ("Actor", 0, 1), ("Actor", 16, 0)].map(|(c, d, i)| format!(r#"["{c}",{d},{i},null,"same"]"#));
    let changes: serde_json::Value = serde_json::from_str(changes).unwrap();
    let table = |name: &str, columns: &str, rows: Vec<String>| {
        let extra = changes[name].as_array().map(|a| a.iter().map(ToString::to_string).collect::<Vec<_>>()).unwrap_or_default();
        let rows: Vec<String> = rows.into_iter().chain(extra).collect();
        format!(r#""{name}": {{"columns": {columns}, "rows": [{}]}}"#, rows.join(","))
    };
    let tables = [
        table("fields", r#"["class","name","offset","status","a_later_column"]"#, fields),
        table("classes", r#"["name","status","size","size_status"]"#, classes),
        table("bases", r#"["class","base","offset","status"]"#, bases.to_vec()),
        table("slots", r#"["class","displacement","index_canonical","index","status"]"#, slots.to_vec()),
        table("functions", r#"["id","status","signature_status","review"]"#, Vec::new()),
        table("globals", r#"["id","status"]"#, Vec::new()),
        table("vtables", r#"["id_canonical","id","status"]"#, Vec::new()),
        table("sites", r#"["id","offset_canonical","offset","bytes","status"]"#, Vec::new()),
        table("id_map", r#"["ae","se","confidence","review"]"#, Vec::new()),
        table("enums", r#"["enum","name","value","status"]"#, Vec::new()),
    ];
    format!(r#"{{"schema": 1, "runtime": "{runtime}", "address_library": {{"file": "{library}", "database": "{database}"}}, {}}}"#, tables.join(", "))
}

const MOVES: &str = r#"{
    "fields": [["Actor","health","0x1c","moved"], ["Actor","process",null,"unverified"], ["Actor","inner",null,"absent"]],
    "classes": [["Actor","changed","0x48","changed"]],
    "bases": [["Actor","ActorValueOwner","0x18","moved"]],
    "slots": [["Actor",0,1,2,"moved"]],
    "functions": [[300,"same","same",null], [301,"changed","same",null], [302,"changed","changed",null], [303,"changed","changed","accepted"], [304,"unverified","same",null], [305,"same","same","rejected"], [306,"absent",null,null], [307,"same","unverified",null], [308,"changed","unverified",null]],
    "vtables": [[100,100,"same"], [101,101,"same"], [102,102,"same"], [103,103,"same"]],
    "sites": [[300,"0x10","0x14","e8 01 02 03 04","differs"], [300,"0x20",null,null,"absent"], [300,"0x30","0x30","e8","same"]],
    "enums": [["Stage","First",0,"same"], ["Stage","Gone",null,"absent"], ["Stage","Last",1,"moved"], ["Steady","One",null,"unverified"]]
}"#;

fn runtime(text: &str) -> Runtime {
    parse(text.as_bytes()).unwrap()
}

fn moved() -> Vec<Runtime> {
    vec![runtime(&file("1.6.1170", "versionlib-1-6-1170-0.bin", "AE", MOVES))]
}

/// Every generated file's text joined, with the fixture's stats.
fn generated(inp: &Inputs, runtimes: &[Runtime]) -> String {
    let variance = Variance::of(inp, runtimes);
    let (files, _) = emit::all(inp, &variance, "test");
    files.values().cloned().collect()
}

#[test]
fn nothing_becomes_an_accessor_without_a_differing_file() {
    let inp = ledger();
    let none = generated(&inp, &[]);
    let same = runtime(&file(CANONICAL, "versionlib-1-7-104-0.bin", "AE", "{}"));
    let canonical = generated(&inp, &[same]);
    let unchanged = generated(&inp, &[runtime(&file("1.6.1170", "versionlib-1-6-1170-0.bin", "AE", "{}"))]);
    assert_eq!(none, canonical, "1.7.104's own file is not compared");
    assert_eq!(none.replace("for 1.5.97, 1.6.353, 1.6.1170", "for 1.5.97, 1.6.353"), unchanged, "an all-same file changes only the provisional line");
    assert!(none.contains("pub health: f32,"));
    assert!(!none.contains("member!(") && !none.contains("slot_at!(") && !none.contains("base_at!("));
    assert!(none.contains("provisional: no difference file yet for 1.5.97, 1.6.353, 1.6.1170"));
}

#[test]
fn a_moved_member_is_opaque_and_read_from_the_profile() {
    let inp = ledger();
    let text = generated(&inp, &moved());
    assert!(!text.contains("pub health: f32,"), "the plain field must be gone");
    assert!(text.contains("#[doc(hidden)]\n    pub __health: [u8; 4],"));
    assert!(text.contains("offset_of!(Actor, __health) == 0x18"), "the struct keeps its canonical layout");
    assert!(text.contains("assert!(size_of::<Actor>() == 0x40)"));
    assert!(text.contains(r#"pub fn __member_health(this: *const Self) -> *mut f32 { unsafe { member!(this, "Actor.health", 0x18, f32) } }"#), "{text}");
    assert_eq!(text.matches(r#"member!(this, "Actor.health""#).count(), 1, "one read of the profile per field");
    assert!(text.contains("pub fn health(&self) -> f32 { unsafe { Self::__member_health(core::ptr::from_ref(self)).read_unaligned() } }"));
    assert!(text.contains("pub fn set_health(&mut self, value: f32) { unsafe { Self::__member_health(core::ptr::from_mut(self).cast_const()).write_unaligned(value) } }"));
    assert!(text.contains("pub fn health_ptr(&self) -> *mut f32 { Self::__member_health(core::ptr::from_ref(self)) }"));
    assert!(text.contains("pub fn inner(&self) -> &crate::Inner { unsafe { &*Self::__member_inner(core::ptr::from_ref(self)) } }"), "an embedded class is a reference");
    assert!(!text.contains("set_inner"));
    assert!(text.contains("pub stable: u32,"), "what stayed identical stays plain");
    assert!(text.contains("pub fn process_field(&self) -> *mut crate::TESForm"), "the `Option` reader keeps its name");
    assert!(text.contains("pub fn process(&self) -> Option<&crate::TESForm> { unsafe { Self::__member_process(core::ptr::from_ref(self)).read_unaligned().as_ref() } }"), "{text}");
}

#[test]
fn a_moved_slot_base_and_size_are_read_from_the_profile() {
    let inp = ledger();
    let text = generated(&inp, &moved());
    assert!(text.contains(r#"virtual_function!(self, "Actor", 0x0, 1, (*const Self))(core::ptr::from_ref(self))"#), "{text}");
    assert!(text.contains(r#"pub fn SLOT_Update() -> usize { slot_at!("Actor", 0x0, 1) }"#));
    assert!(text.contains("pub __slot_1: *const c_void,"));
    assert!(text.contains("pub slot_0: *const c_void,"));
    assert!(text.contains(r#"byte_add(base_at!("Actor:ActorValueOwner", 0x10))"#), "{text}");
}

#[test]
fn an_enum_that_differs_goes_through_the_profile() {
    let inp = ledger();
    let text = generated(&inp, &moved());
    assert!(text.contains(r#"Self::Last => { record!(7, 0, 0, "Stage.Last"); engine_value!("Stage.Last", 2, u32) }"#), "{text}");
    assert!(text.contains(r#"if engine_value!("Stage.Last", 2, u32) == Some(raw) { return Some(Self::Last); }"#));
    assert!(!text.contains("Stage::from_u32") && !text.contains("pub const fn from_u32(value: u32) -> Option<Self> {\n        match value {\n            0 => Some(Self::First)"));
    assert!(text.contains(r#"stage.to_engine().expect("this value does not exist on this runtime")"#), "{text}");
    assert!(text.contains("crate::Stage::from_engine(raw).ok_or(raw)"));
    assert!(text.contains("steady: crate::Steady") && text.contains("pub enum Steady"), "an enum with only unverified rows stays plain");
    assert!(!text.contains("Steady.One"), "and gets no machinery");
}

#[test]
fn profile_rows_follow_the_contract() {
    let inp = ledger();
    let profiles = profiles(&inp, &moved());
    assert_eq!(profiles[&format!("{PROFILE_DIR}/1-7-104.profile")], "skyrim-rs-profile 1\nruntime 1.7.104\naddress-library versionlib-1-7-104-0.bin\ndatabase ae\n");
    assert_eq!(
        profiles[&format!("{PROFILE_DIR}/1-6-1170.profile")],
        "skyrim-rs-profile 1\nruntime 1.6.1170\naddress-library versionlib-1-6-1170-0.bin\ndatabase ae\n\
         id-unverified 302\nid-unverified 305\nid-unverified 308\n\
         member Actor.health 1c\nmember-missing Actor.inner\nmember-unverified Actor.process\n\
         slot Actor[0].1 2\n\
         size Actor 48\n\
         base Actor:ActorValueOwner 18\n\
         site 300 10 14 e801020304\nsite-missing 300 20\nsite 300 30 30 e8\n\
         enum-missing Stage.Gone\nenum Stage.Last 1\n"
    );
    let parsed = versionlib::profile::parse(&profiles[&format!("{PROFILE_DIR}/1-6-1170.profile")]).unwrap();
    assert_eq!(parsed.site(300, 0x30), versionlib::profile::Row::Here(&versionlib::profile::Site { offset: 0x30, expected: vec![0xE8], replacement: None }), "a compared site has a row, same or not");
    assert_eq!(parsed.site(300, 0x40), versionlib::profile::Row::Unverified, "a site with no row was never compared");
}

#[test]
fn a_ledger_item_with_no_row_is_unverified() {
    let inp = ledger();
    let bare = runtime(r#"{"schema": 1, "runtime": "1.6.353", "address_library": {"file": "versionlib-1-6-353-0.bin", "database": "AE"}}"#);
    let variance = Variance::of(&inp, std::slice::from_ref(&bare));
    assert!(variance.member("Actor", "stable") && variance.slot("TESForm", 0, 0) && variance.sizes.contains("Inner") && variance.base("Actor", "TESForm"));
    let text = &profiles(&inp, std::slice::from_ref(&bare))[&format!("{PROFILE_DIR}/1-6-353.profile")];
    for row in ["id-unverified 300\n", "member-unverified Actor.stable\n", "slot-unverified TESForm[0].0\n", "size-unverified Inner\n", "base-unverified Actor:TESForm\n"] {
        assert!(text.contains(row), "{row}");
    }
}

#[test]
fn a_site_only_another_file_lists_is_unverified_here() {
    let inp = ledger();
    let canonical = runtime(&file(CANONICAL, "versionlib-1-7-104-0.bin", "AE", r#"{"sites": [[300,"0x40","0x40",null,"same"]]}"#));
    let other = runtime(&file("1.6.1170", "versionlib-1-6-1170-0.bin", "AE", "{}"));
    let text = &profiles(&inp, &[canonical, other])[&format!("{PROFILE_DIR}/1-6-1170.profile")];
    assert!(text.contains("site-unverified 300 40\n"), "{text}");
}

#[test]
fn se_ids_follow_confidence_and_review() {
    let inp = ledger();
    let changes = r#"{
        "id_map": [[1,11,"high",null], [2,12,"medium",null], [3,13,"low","accepted"], [4,14,"high","rejected"], [5,15,"medium","rejected"], [6,16,"high",null], [7,17,"high",null]],
        "functions": [[6,"changed","changed",null], [7,"same","same","rejected"]],
        "vtables": [[8,18,"same"], [9,null,"same"], [10,20,"absent"]]
    }"#;
    let se = runtime(&file("1.5.97", "version-1-5-97-0.bin", "SE", changes));
    let text = &profiles(&inp, &[se])[&format!("{PROFILE_DIR}/1-5-97.profile")];
    let ids: Vec<&str> = text.lines().filter(|l| l.starts_with("id")).collect();
    assert_eq!(ids, ["id 1 11", "id-unverified 2", "id 3 13", "id-unverified 6", "id 8 18"]);
    assert!(text.contains("database se\n"));
}

#[test]
fn a_file_of_another_schema_is_refused() {
    let error = parse(br#"{"schema": 2, "runtime": "1.6.1170", "address_library": {"file": "a", "database": "AE"}}"#).err().unwrap();
    assert!(error.to_string().contains("schema 2"));
}

/// Writes `files` under `dir`, creating folders.
fn write_tree(dir: &Path, files: &BTreeMap<String, String>) {
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

/// Compiles the fixture into a throwaway `sre` crate and a DLL using it, proves a direct read of an
/// opaque field does not compile, and reads the DLL's records back. `cargo test -p xtask -- --ignored`.
#[test]
#[ignore = "runs cargo on a throwaway crate"]
fn the_fixture_compiles_and_records_what_it_uses() {
    let root = crate::workspace_root();
    let dir = root.join("target").join("fixture-sre");
    let _ = fs::remove_dir_all(dir.join("sre"));
    let inp = ledger();
    let variance = Variance::of(&inp, &moved());
    let (files, _) = emit::all(&inp, &variance, "test");
    write_tree(&dir.join("sre"), &files);
    fs::copy(root.join("crates/sre/src/lib.rs"), dir.join("sre/src/lib.rs")).unwrap();
    let skse = root.join("crates/skse").display().to_string().replace('\\', "/");
    let versionlib = root.join("crates/versionlib").display().to_string().replace('\\', "/");
    let user = r#"
use std::ffi::c_void;

/// # Safety
/// Test only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uses(actor: *mut sre::Actor) -> f32 {
    let actor = unsafe { &mut *actor };
    actor.set_health(1.0);
    actor.update();
    actor.mod_value(1.0);
    let _ = actor.process();
    let _ = sre::class_size!("Actor", sre::Actor);
    let at: *const c_void = core::ptr::from_ref(actor.inner()).cast();
    std::hint::black_box(at);
    actor.kill();
    let _ = actor.set_stage(sre::Stage::Last, sre::Steady::One);
    let _ = sre::Stage::from_engine(1);
    #[cfg(feature = "direct")]
    let _ = actor.health;
    actor.health()
}
"#;
    let tree = BTreeMap::from([
        ("Cargo.toml".to_owned(), format!("[workspace]\nresolver = \"3\"\nmembers = [\"sre\", \"user\"]\n[workspace.package]\nedition = \"2024\"\nversion = \"0.1.0\"\npublish = false\n[workspace.dependencies]\nskse = {{ path = \"{skse}\" }}\nversionlib = {{ path = \"{versionlib}\" }}\nwindows-sys = \"0.61\"\n[workspace.lints.rust]\n")),
        ("user/Cargo.toml".to_owned(), "[package]\nname = \"user\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\ncrate-type = [\"cdylib\"]\n[features]\ndirect = []\n[dependencies]\nsre = { package = \"skyrim-sre\", path = \"../sre\", features = [\"actors\"] }\n".to_owned()),
        ("user/src/lib.rs".to_owned(), user.to_owned()),
    ]);
    write_tree(&dir, &tree);
    let cargo = |args: &[&str]| Command::new(env!("CARGO")).args(args).current_dir(&dir).env_remove("RUSTFLAGS").output().unwrap();
    let built = cargo(&["build", "-p", "user"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let direct = cargo(&["check", "-p", "user", "--features", "direct"]);
    assert!(!direct.status.success(), "a direct read of an opaque field compiled");
    assert!(String::from_utf8_lossy(&direct.stderr).contains("E0615"), "{}", String::from_utf8_lossy(&direct.stderr));
    let dll = fs::read(dir.join("target/debug/user.dll")).unwrap();
    let records: Vec<String> = versionlib::manifest::read(&dll).unwrap().iter().map(ToString::to_string).collect();
    for record in ["id 300", "id 301", "member Actor.health", "member Actor.process", "member Actor.inner", "slot Actor[0].1", "size Actor", "base Actor:ActorValueOwner", "enum Stage.Last", "enum Stage.*"] {
        assert!(records.iter().any(|r| r == record), "{record} not in {records:?}");
    }
}

#[test]
fn a_site_patch_fills_or_answers_the_compared_row_and_refuses_to_contradict_it() {
    let inp = ledger();
    let mut other = runtime(&file("1.6.1170", "versionlib-1-6-1170-0.bin", "AE", MOVES));
    other.apply_site_patches("# a comment\n1.6.1170 300 10 14 e801020304 9090909090 how\n1.6.1170 300 50 60 aa bb read by hand\n1.5.97 300 70 70 aa bb another runtime\n").unwrap();
    let text = &profiles(&inp, std::slice::from_ref(&other))[&format!("{PROFILE_DIR}/1-6-1170.profile")];
    assert!(text.contains("site 300 10 14 e801020304 9090909090\n"), "{text}");
    assert!(text.contains("site 300 50 60 aa bb\n"), "{text}");
    assert!(!text.contains("300 70"), "{text}");
    assert!(other.apply_site_patches("1.6.1170 300 30 31 e8 90 moved").unwrap_err().contains("difference file"));
    assert!(other.apply_site_patches("1.6.1170 300 30 30 e9 90 other bytes").unwrap_err().contains("difference file"));
    assert!(other.apply_site_patches("1.6.1170 300 20 20 e8 90 absent").unwrap_err().contains("absent"));
    assert!(other.apply_site_patches("1.6.1170 300 10 14 e8").unwrap_err().contains("malformed"));
}

#[test]
fn an_ae_profile_maps_the_ids_its_library_holds_under_another_number() {
    let functions = r#""functions": {"columns": ["id","status","signature_status","review"], "rows": [[1,"same",null,null],[2,"same",null,null],[3,"same",null,null],[4,"same",null,null],[5,"same",null,null],[6,"changed","unverified",null]]}"#;
    let id_map = r#""id_map": {"columns": ["ae","se","confidence","review"], "rows": [[1,11,"high",null],[2,12,"low","accepted"],[3,13,"medium",null],[4,14,"high","rejected"],[5,null,"high",null],[6,16,"high",null]]}"#;
    let vtables = r#""vtables": {"columns": ["id_canonical","id","status"], "rows": [[7,7,"same"],[8,18,"same"]]}"#;
    let other = runtime(&format!(r#"{{"schema": 1, "runtime": "1.6.353", "address_library": {{"file": "versionlib-1-6-353-0.bin", "database": "AE"}}, {functions}, {id_map}, {vtables}}}"#));
    let text = &profiles(&ledger(), std::slice::from_ref(&other))[&format!("{PROFILE_DIR}/1-6-353.profile")];
    for row in ["id 1 11\n", "id 2 12\n", "id-unverified 3\n", "id-missing 5\n", "id-unverified 6\n", "id 8 18\n"] {
        assert!(text.contains(row), "{row} in {text}");
    }
    assert!(!text.contains("id 4 ") && !text.contains("id-unverified 4\n") && !text.contains("id 7 "), "{text}");
}

#[test]
fn the_site_patch_table_is_well_formed() {
    for name in ["1.5.97", "1.6.353", "1.6.1170"] {
        let mut bare = runtime(&file(name, "versionlib-1-6-1170-0.bin", "AE", "{}"));
        bare.apply_site_patches(SITE_PATCHES).unwrap();
        assert!(bare.sites.len() >= 3, "{name}");
    }
}
