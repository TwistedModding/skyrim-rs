//! Where the generated items go: the group module of every class, function and global, the files
//! that hold a group under the size limit, and the features that compile each group.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::api::Tier;
use crate::emit::HEADER;

/// The largest generated file, so an editor indexes every one.
pub(crate) const FILE_LIMIT: usize = 1_000_000;

/// The groups an engine name falls in when no semantic pass places it, checked in order, each
/// with the rule `family` applies and the module doc.
const FAMILIES: &[(&str, &str)] = &[
    ("script", "Papyrus internals: the `BSScript`, `SkyrimScript` and `SkyrimVM` names."),
    ("bnet", "Bethesda.net: the `bnet` names."),
    ("stl", "The C++ standard library types the engine instantiates (`std::`)."),
    ("scaleform", "Scaleform: the `Scaleform`, `GFx` and `GAS` names."),
    ("havok", "Havok: the `hk` and `bhk` names."),
    ("ni", "The scene graph: the `Ni` names."),
    ("bgs", "The `BGS` names."),
    ("tes", "The `TES` names."),
    ("bs", "The `BS` names."),
    ("combat", "Combat AI: the `Combat` names."),
    ("pathing", "Movement and pathfinding: the `Movement`, `Pathing` and `Path` names."),
    ("extra", "Extra data: the `Extra` names."),
    ("settings", "Game and INI settings: the `Setting`, `GameSetting`, `INISetting` and `INIPrefSetting` names."),
    ("factory", "Registration and factories: the `AutoRegister` and `Concrete` names and every `Factory` or `Creator`."),
    ("other", "Every name no other group takes."),
];

/// Whether `name` starts with `prefix` followed by an uppercase letter, a digit, `_` or nothing.
fn word(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|rest| rest.chars().next().is_none_or(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
}

/// The group of an engine name no semantic pass places: by its outermost namespace when it has
/// one (anonymous namespaces skipped), else by the name, as `FAMILIES` lists them.
pub(crate) fn family(name: &str) -> &'static str {
    let mut name = name;
    while let Some((_, rest)) = name.strip_prefix("_anon_").and_then(|r| r.split_once("::")) {
        name = rest;
    }
    let mut depth = 0i32;
    let mut head = name;
    for (i, c) in name.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ':' if depth == 0 && name[i..].starts_with("::") => {
                head = &name[..i];
                break;
            }
            _ => {}
        }
    }
    let bare = head.split('<').next().unwrap_or(head);
    let family = if ["BSScript", "SkyrimScript", "SkyrimVM"].iter().any(|p| word(head, p)) {
        "script"
    } else if head.starts_with("bnet") {
        "bnet"
    } else if head == "std" {
        "stl"
    } else if ["Scaleform", "GFx", "GAS"].iter().any(|p| word(head, p)) {
        "scaleform"
    } else if head.starts_with("hk") || head.starts_with("bhk") {
        "havok"
    } else if word(head, "Ni") {
        "ni"
    } else if word(head, "BGS") {
        "bgs"
    } else if word(head, "TES") {
        "tes"
    } else if word(head, "BS") {
        "bs"
    } else if word(head, "Combat") {
        "combat"
    } else if ["Movement", "Pathing", "Path"].iter().any(|p| word(head, p)) {
        "pathing"
    } else if word(head, "Extra") {
        "extra"
    } else if ["Setting", "GameSetting", "INISetting", "INIPrefSetting"].iter().any(|p| word(head, p)) {
        "settings"
    } else if head.starts_with("AutoRegister") || head.starts_with("Concrete") || bare.ends_with("Factory") || bare.ends_with("Creator") {
        "factory"
    } else {
        "other"
    };
    debug_assert!(FAMILIES.iter().any(|(f, _)| *f == family));
    family
}

/// What a group file holds: the structs and enums, or the callable layer of one tier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Types,
    Api(Tier),
}

impl Kind {
    fn stem(self) -> &'static str {
        match self {
            Self::Types => "types",
            Self::Api(tier) => tier.module(),
        }
    }

    fn head(self) -> &'static str {
        match self {
            Self::Types => "use core::ffi::c_void;\n\n",
            Self::Api(_) => "use core::ffi::{c_char, c_void};\n\n",
        }
    }

    /// The `cfg` that compiles the kind in `group`: the structs always, the callable layer by the
    /// group's feature and its tier's.
    fn cfg(self, group: &str) -> String {
        match self {
            Self::Types => String::new(),
            Self::Api(tier) => match tier.feature() {
                None => format!("#[cfg(feature = \"{group}\")]\n"),
                Some(t) => format!("#[cfg(all(feature = \"{group}\", feature = \"{t}\"))]\n"),
            },
        }
    }
}

/// Every group's items keyed by group and kind, in output order.
pub(crate) type Items = BTreeMap<(String, Kind), Vec<String>>;

/// Splits `items` into files of at most `FILE_LIMIT` bytes, each `HEADER`, `head`, a run of items
/// and `tail`: one module `stem`, or `stem_1`, `stem_2`, ... Returns `(module, text)` pairs.
pub(crate) fn pack(stem: &str, head: &str, tail: &str, items: &[String]) -> Vec<(String, String)> {
    let fixed = HEADER.len() + head.len() + tail.len();
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for item in items {
        assert!(fixed + item.len() <= FILE_LIMIT, "{stem}: one item of {} bytes passes the file limit", item.len());
        if !current.is_empty() && fixed + current.len() + item.len() > FILE_LIMIT {
            parts.push(std::mem::take(&mut current));
        }
        current.push_str(item);
    }
    parts.push(current);
    let single = parts.len() == 1;
    parts
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let module = if single { stem.to_owned() } else { format!("{stem}_{}", i + 1) };
            (module, format!("{HEADER}{head}{body}{tail}"))
        })
        .collect()
}

/// A module whose files are private submodules; `reexport` glob re-exports each into it.
pub(crate) struct Module {
    doc: String,
    body: String,
    pub(crate) files: Vec<(String, String)>,
}

impl Module {
    pub(crate) fn new(doc: &str) -> Self {
        Self { doc: doc.to_owned(), body: String::new(), files: Vec::new() }
    }

    /// Adds `items` packed into files under `cfg`, each re-exported when `reexport` and it has a
    /// public item of its own (not only `impl` blocks); returns the modules.
    pub(crate) fn add(&mut self, cfg: &str, stem: &str, head: &str, tail: &str, items: &[String], reexport: bool) -> Vec<String> {
        let mut modules = Vec::new();
        for (module, text) in pack(stem, head, tail, items) {
            writeln!(self.body, "{cfg}mod {module};").unwrap();
            if reexport && text.lines().any(|l| l.starts_with("pub ")) {
                writeln!(self.body, "{cfg}pub use {module}::*;").unwrap();
            }
            self.files.push((format!("{module}.rs"), text));
            modules.push(module);
        }
        modules
    }

    /// The module's files with `mod.rs`, under `dir`; `tail` ends `mod.rs`.
    pub(crate) fn finish(self, dir: &str, tail: &str) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = self.files.into_iter().map(|(f, t)| (format!("{dir}/{f}"), t)).collect();
        let doc = self.doc.lines().fold(String::new(), |doc, l| doc + "//! " + l + "\n");
        files.push((format!("{dir}/mod.rs"), format!("{doc}//! Generated by `cargo xtask gen`; do not edit.\n\n{}{tail}", self.body)));
        files
    }
}

/// The doc of a group module.
pub(crate) fn group_doc(group: &str, passes: &[&str]) -> String {
    if passes.contains(&group) {
        return format!("What the {group} semantic pass describes: its classes and their methods, its enums, flag sets, functions and globals.");
    }
    FAMILIES.iter().find(|(f, _)| *f == group).map_or_else(|| panic!("group {group} has no doc"), |(_, d)| (*d).to_owned())
}

/// One group's module: its kinds in order, each behind its feature.
pub(crate) fn group_files(group: &str, doc: &str, kinds: &BTreeMap<Kind, &Vec<String>>) -> Vec<(String, String)> {
    let mut module = Module::new(doc);
    for (kind, items) in kinds {
        module.add(&kind.cfg(group), kind.stem(), kind.head(), "", items, true);
    }
    module.finish(group, "")
}

/// The crate's manifest: one feature per group, the tier features, and the default set.
pub(crate) fn cargo_toml(groups: &[&str]) -> String {
    let mut default: Vec<String> = groups.iter().map(|g| format!("\"{g}\"")).collect();
    default.push("\"tier-high\"".into());
    let mut out = format!(
        "# Generated by `cargo xtask gen`, which also names the features after the groups; do not edit.\n\
         [package]\nname = \"{}\"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n\
         [dependencies]\nskse.workspace = true\nversionlib.workspace = true\n\n\
         # A group's feature compiles its methods, free functions and global accessors; a tier feature adds,\n\
         # in every enabled group, the functions typed only by a decompiler prototype of that rating.\n\
         # Structs, enums and `raw` are always compiled.\n\
         [features]\ndefault = [{}]\n",
        crate::PACKAGE,
        default.join(", ")
    );
    for g in groups {
        writeln!(out, "{g} = []").unwrap();
    }
    for tier in Tier::ALL {
        if let Some(f) = tier.feature() {
            writeln!(out, "{f} = []").unwrap();
        }
    }
    out.push_str("\n[lints]\nworkspace = true\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_follow_the_names() {
        assert_eq!(family("BSScript::Internal::VirtualMachine"), "script");
        assert_eq!(family("_anon_76CA921F::BSFoo"), "bs");
        assert_eq!(family("BSResource::_anon_30629CF1::AsyncArchiveStream"), "bs");
        assert_eq!(family("hkAabbTreeQueryUtils<hkAabbTreeData::hkAabbTreeAabb,unsigned_int>::Foo"), "havok");
        assert_eq!(family("bhkWorld"), "havok");
        assert_eq!(family("NiNode"), "ni");
        assert_eq!(family("Night"), "other");
        assert_eq!(family("BSTArray<int>"), "bs");
        assert_eq!(family("std::_Ref_count_obj<X>"), "stl");
        assert_eq!(family("ConcreteFormFactory<TESNPC,43>"), "factory");
        assert_eq!(family("AutoRegisterCreator<A,B>"), "factory");
        assert_eq!(family("INISetting_bUseArchives_Archive"), "settings");
        assert_eq!(family("CraftingSubMenus::AlchemyMenu"), "other");
    }

    #[test]
    fn packing_keeps_files_under_the_limit() {
        let items: Vec<String> = (0..5).map(|i| format!("{i}").repeat(FILE_LIMIT / 3)).collect();
        let parts = pack("types", "", "", &items);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].0, "types_1");
        assert!(parts.iter().all(|(_, t)| t.len() <= FILE_LIMIT));
        assert_eq!(pack("types", "", "", &items[..1])[0].0, "types");
    }
}
