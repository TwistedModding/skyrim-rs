//! The running runtime's difference from 1.7.104: a text file per runtime, read once per DLL.
//! `docs/subsystems/runtimes.md` is the format's contract.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use skse::runtime;

const READER_VERSION: u32 = 1;

/// What a profile says about one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row<T> {
    /// No row: identical to 1.7.104. An id keeps its value.
    Same,
    Here(T),
    Missing,
    Unverified,
}

/// A patch site on this runtime.
#[derive(Debug, PartialEq, Eq)]
pub struct Site {
    pub offset: u32,
    pub expected: Vec<u8>,
    pub replacement: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct Profile {
    runtime: u32,
    address_library: String,
    se_database: bool,
    ids: BTreeMap<u32, Row<u32>>,
    /// Ids whose address is good and whose signature was not compared, with the number the library
    /// holds them under when it differs. To [`Self::id`] these are unverified.
    addresses: BTreeMap<u32, Option<u32>>,
    members: BTreeMap<String, Row<u32>>,
    slots: BTreeMap<(String, u32, u32), Row<u32>>,
    sizes: BTreeMap<String, Row<u32>>,
    bases: BTreeMap<String, Row<u32>>,
    sites: BTreeMap<(u32, u32), Row<Site>>,
    enums: BTreeMap<String, Row<u32>>,
}

impl Profile {
    fn new(runtime: u32, address_library: String, se_database: bool) -> Self {
        Self { runtime, address_library, se_database, ids: BTreeMap::new(), addresses: BTreeMap::new(), members: BTreeMap::new(), slots: BTreeMap::new(), sizes: BTreeMap::new(), bases: BTreeMap::new(), sites: BTreeMap::new(), enums: BTreeMap::new() }
    }

    /// The runtime the file describes, packed with store 0.
    #[must_use]
    pub fn runtime(&self) -> u32 {
        self.runtime
    }

    /// The runtime as a player reads it, `1.7.104`.
    #[must_use]
    pub fn name(&self) -> String {
        let [major, minor, build, _] = runtime::unpack(self.runtime);
        format!("{major}.{minor}.{build}")
    }

    /// The Address Library file's name in the plugins folder.
    #[must_use]
    pub fn address_library(&self) -> &str {
        &self.address_library
    }

    /// Whether the Address Library holds SE ids, which an AE id must be translated into.
    #[must_use]
    pub fn se_database(&self) -> bool {
        self.se_database
    }

    /// An AE id here. With no row it is missing on an SE database; on an AE database it keeps its
    /// value and the Address Library file decides whether it exists, while a row gives the number
    /// that file holds it under.
    #[must_use]
    pub fn id(&self, ae_id: u32) -> Row<u32> {
        match self.ids.get(&ae_id) {
            Some(&row) => row,
            None if self.se_database => Row::Missing,
            None => Row::Same,
        }
    }

    /// An AE id for code that only locates a call site or compares an address: [`Self::id`], but
    /// usable for an `id-address` row too.
    #[must_use]
    pub fn address_id(&self, ae_id: u32) -> Row<u32> {
        match self.addresses.get(&ae_id) {
            Some(Some(here)) => Row::Here(*here),
            Some(None) => Row::Same,
            None => self.id(ae_id),
        }
    }

    /// The offset of `Class.member`.
    #[must_use]
    pub fn member(&self, member: &str) -> Row<u32> {
        self.members.get(member).copied().unwrap_or(Row::Same)
    }

    /// The index here of slot `index` of the vtable at `vtable_offset` of `class`.
    #[must_use]
    pub fn slot(&self, class: &str, vtable_offset: u32, index: u32) -> Row<u32> {
        self.slots.get(&(class.to_owned(), vtable_offset, index)).copied().unwrap_or(Row::Same)
    }

    #[must_use]
    pub fn size(&self, class: &str) -> Row<u32> {
        self.sizes.get(class).copied().unwrap_or(Row::Same)
    }

    /// The offset of `Class:Base`.
    #[must_use]
    pub fn base(&self, class_and_base: &str) -> Row<u32> {
        self.bases.get(class_and_base).copied().unwrap_or(Row::Same)
    }

    /// The site at `canonical_offset` into the function with an AE id. Sites cannot be enumerated
    /// the way members can, so off 1.7.104 one with no row was never compared.
    #[must_use]
    pub fn site(&self, ae_id: u32, canonical_offset: u32) -> Row<&Site> {
        match self.sites.get(&(ae_id, canonical_offset)) {
            None if runtime::is_canonical(self.runtime) => Row::Same,
            Some(Row::Same) => Row::Same,
            Some(Row::Here(site)) => Row::Here(site),
            Some(Row::Missing) => Row::Missing,
            None | Some(Row::Unverified) => Row::Unverified,
        }
    }

    /// The number of `Enum.Value` here.
    #[must_use]
    pub fn enum_value(&self, value: &str) -> Row<u32> {
        self.enums.get(value).copied().unwrap_or(Row::Same)
    }

    fn add_address(&mut self, ae: u32, here: Option<u32>) -> Option<()> {
        put(&mut self.ids, ae, Row::Unverified)?;
        put(&mut self.addresses, ae, here)
    }

    /// Adds a body row; `None` when the row is malformed or repeats a key.
    fn add_row(&mut self, fields: &[&str]) -> Option<()> {
        match *fields {
            ["id", ae, se] => put(&mut self.ids, decimal(ae)?, Row::Here(decimal(se)?)),
            ["id-missing", ae] => put(&mut self.ids, decimal(ae)?, Row::Missing),
            ["id-unverified", ae] => put(&mut self.ids, decimal(ae)?, Row::Unverified),
            ["id-address", ae] => self.add_address(decimal(ae)?, None),
            ["id-address", ae, here] => self.add_address(decimal(ae)?, Some(decimal(here)?)),
            ["member", key, offset] => put(&mut self.members, member_key(key)?, Row::Here(hex(offset)?)),
            ["member-missing", key] => put(&mut self.members, member_key(key)?, Row::Missing),
            ["member-unverified", key] => put(&mut self.members, member_key(key)?, Row::Unverified),
            ["slot", key, here] => put(&mut self.slots, slot_key(key)?, Row::Here(hex(here)?)),
            ["slot-missing", key] => put(&mut self.slots, slot_key(key)?, Row::Missing),
            ["slot-unverified", key] => put(&mut self.slots, slot_key(key)?, Row::Unverified),
            ["size", class, size] => put(&mut self.sizes, nonempty(class)?, Row::Here(hex(size)?)),
            ["size-missing", class] => put(&mut self.sizes, nonempty(class)?, Row::Missing),
            ["size-unverified", class] => put(&mut self.sizes, nonempty(class)?, Row::Unverified),
            ["base", key, offset] => put(&mut self.bases, base_key(key)?, Row::Here(hex(offset)?)),
            ["base-missing", key] => put(&mut self.bases, base_key(key)?, Row::Missing),
            ["base-unverified", key] => put(&mut self.bases, base_key(key)?, Row::Unverified),
            ["site", id, canonical, here, expected] => put(&mut self.sites, (decimal(id)?, hex(canonical)?), Row::Here(Site { offset: hex(here)?, expected: bytes(expected)?, replacement: None })),
            ["site", id, canonical, here, expected, replacement] => put(&mut self.sites, (decimal(id)?, hex(canonical)?), Row::Here(Site { offset: hex(here)?, expected: bytes(expected)?, replacement: Some(bytes(replacement)?) })),
            ["site-missing", id, canonical] => put(&mut self.sites, (decimal(id)?, hex(canonical)?), Row::Missing),
            ["site-unverified", id, canonical] => put(&mut self.sites, (decimal(id)?, hex(canonical)?), Row::Unverified),
            ["enum", key, value] => put(&mut self.enums, member_key(key)?, Row::Here(hex(value)?)),
            ["enum-missing", key] => put(&mut self.enums, member_key(key)?, Row::Missing),
            _ => None,
        }
    }
}

fn put<K: Ord, T>(rows: &mut BTreeMap<K, T>, key: K, row: T) -> Option<()> {
    rows.insert(key, row).is_none().then_some(())
}

fn decimal(field: &str) -> Option<u32> {
    field.parse().ok()
}

fn hex(field: &str) -> Option<u32> {
    u32::from_str_radix(field, 16).ok()
}

fn nonempty(field: &str) -> Option<String> {
    (!field.is_empty()).then(|| field.to_owned())
}

fn member_key(field: &str) -> Option<String> {
    let (class, member) = field.split_once('.')?;
    (!class.is_empty() && !member.is_empty()).then(|| field.to_owned())
}

fn base_key(field: &str) -> Option<String> {
    let (class, base) = field.split_once(':')?;
    (!class.is_empty() && !base.is_empty()).then(|| field.to_owned())
}

/// `Class[vtable offset].index`, both numbers in hex.
fn slot_key(field: &str) -> Option<(String, u32, u32)> {
    let (class, rest) = field.split_once('[')?;
    let (vtable, index) = rest.split_once("].")?;
    Some((nonempty(class)?, hex(vtable)?, hex(index)?))
}

fn bytes(field: &str) -> Option<Vec<u8>> {
    if field.is_empty() || !field.len().is_multiple_of(2) {
        return None;
    }
    (0..field.len()).step_by(2).map(|i| u8::from_str_radix(field.get(i..i + 2)?, 16).ok()).collect()
}

fn dotted_runtime(field: &str) -> Option<u32> {
    let mut parts = field.split('.').map(decimal);
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then(|| runtime::pack(version[0], version[1], version[2], 0))
}

/// Reads a profile's text. The error names the line.
pub fn parse(text: &str) -> Result<Profile, String> {
    let mut lines = text.lines();
    let mut header = |key: &str| lines.next().and_then(|line| line.strip_prefix(key)?.strip_prefix(' ')).ok_or_else(|| format!("no {key} row"));
    let version = header("skyrim-rs-profile")?;
    let version = decimal(version).filter(|&v| v > 0).ok_or_else(|| format!("bad profile version {version:?}"))?;
    if version > READER_VERSION {
        return Err(format!("profile version {version} is newer than this build reads ({READER_VERSION})"));
    }
    let named = header("runtime")?;
    let runtime = dotted_runtime(named).ok_or_else(|| format!("bad runtime {named:?}"))?;
    let address_library = header("address-library")?.to_owned();
    let se_database = match header("database")? {
        "se" => true,
        "ae" => false,
        other => return Err(format!("bad database {other:?}")),
    };
    let mut profile = Profile::new(runtime, address_library, se_database);
    let mut fields = Vec::new();
    for (index, line) in lines.enumerate() {
        fields.clear();
        fields.extend(line.split(' '));
        profile.add_row(&fields).ok_or_else(|| format!("line {}: bad row {line:?}", index + 5))?;
    }
    Ok(profile)
}

fn folder(plugins: &Path) -> PathBuf {
    plugins.join("skyrim-runtime-extender").join("profiles")
}

/// Every `*.profile` in a plugins folder's profile folder, each read or the reason it could not be.
#[must_use]
pub fn all(plugins: &Path) -> Vec<Result<Profile, String>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(folder(plugins)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("profile"))).collect();
    paths.sort();
    paths.iter().map(|path| std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| parse(&text)).map_err(|e| format!("{}: {e}", path.display()))).collect()
}

/// The profile of `runtime` from a plugins folder. 1.7.104 with no file is an empty profile; any
/// other runtime with no file is an error naming the runtime and the path.
fn load(runtime: u32, plugins: &Path) -> Result<Profile, String> {
    let [major, minor, build, _] = runtime::unpack(runtime);
    let path = folder(plugins).join(format!("{major}-{minor}-{build}.profile"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound && runtime::is_canonical(runtime) => return Ok(Profile::new(runtime::CANONICAL, runtime::versionlib_name(runtime::CANONICAL), false)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(format!("runtime {major}.{minor}.{build}: no profile at {}", path.display())),
        Err(e) => return Err(format!("runtime {major}.{minor}.{build}: {}: {e}", path.display())),
    };
    let profile = parse(&text).map_err(|e| format!("runtime {major}.{minor}.{build}: {}: {e}", path.display()))?;
    if profile.runtime >> 4 != runtime >> 4 {
        let [a, b, c, _] = runtime::unpack(profile.runtime);
        return Err(format!("runtime {major}.{minor}.{build}: {} describes {a}.{b}.{c}", path.display()));
    }
    Ok(profile)
}

static CURRENT: OnceLock<Result<Profile, String>> = OnceLock::new();

/// For a unit test, whose process is no game and has no profile: every lookup answers as on 1.7.104.
#[doc(hidden)]
pub fn assume_canonical() {
    let _ = CURRENT.set(Ok(Profile::new(runtime::pack(1, 7, 104, 0), "versionlib-1-7-104-0.bin".to_owned(), false)));
}

/// The running runtime's profile, read by the first call in this DLL. A plain SKSE plugin reads it
/// for itself, without the host.
pub fn current() -> Result<&'static Profile, &'static str> {
    CURRENT
        .get_or_init(|| {
            let running = runtime::running().ok_or("the running executable has no file version")?;
            load(running, &crate::plugins_dir().map_err(|e| e.to_string())?)
        })
        .as_ref()
        .map_err(String::as_str)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    const HEADER: &str = "skyrim-rs-profile 1\nruntime 1.5.97\naddress-library version-1-5-97-0.bin\ndatabase se\n";
    const AE_HEADER: &str = "skyrim-rs-profile 1\nruntime 1.6.1170\naddress-library versionlib-1-6-1170-0.bin\ndatabase ae\n";

    fn with(header: &str, rows: &str) -> Result<Profile, String> {
        parse(&format!("{header}{rows}"))
    }

    fn temp_plugins(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("versionlib-profile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skyrim-runtime-extender").join("profiles")).unwrap();
        dir
    }

    #[test]
    fn header_rows() {
        let p = parse(HEADER).unwrap();
        assert_eq!(p.runtime(), runtime::pack(1, 5, 97, 0));
        assert_eq!(p.address_library(), "version-1-5-97-0.bin");
        assert!(p.se_database());
        assert!(!parse(AE_HEADER).unwrap().se_database());
    }

    #[test]
    fn crlf_line_ends_read_the_same() {
        let p = parse(&format!("{HEADER}id 7 9\n").replace('\n', "\r\n")).unwrap();
        assert_eq!(p.id(7), Row::Here(9));
    }

    #[test]
    fn ids_on_an_se_database() {
        let p = with(HEADER, "id 100 200\nid-missing 101\nid-unverified 102\n").unwrap();
        assert_eq!(p.id(100), Row::Here(200));
        assert_eq!(p.id(101), Row::Missing);
        assert_eq!(p.id(102), Row::Unverified);
        assert_eq!(p.id(103), Row::Missing);
    }

    #[test]
    fn ids_on_an_ae_database() {
        let p = with(AE_HEADER, "id 100 7\nid-missing 101\nid-unverified 102\n").unwrap();
        assert_eq!(p.id(100), Row::Here(7));
        assert_eq!(p.id(101), Row::Missing);
        assert_eq!(p.id(102), Row::Unverified);
        assert_eq!(p.id(103), Row::Same);
    }

    #[test]
    fn members() {
        let p = with(HEADER, "member Actor.process 1f8\nmember-missing Actor.gone\nmember-unverified Actor.unsure\n").unwrap();
        assert_eq!(p.member("Actor.process"), Row::Here(0x1F8));
        assert_eq!(p.member("Actor.gone"), Row::Missing);
        assert_eq!(p.member("Actor.unsure"), Row::Unverified);
        assert_eq!(p.member("Actor.other"), Row::Same);
    }

    #[test]
    fn slots() {
        let p = with(HEADER, "slot Actor[0].1f 1e\nslot-missing Actor[b0].2\nslot-unverified Actor[0].3\n").unwrap();
        assert_eq!(p.slot("Actor", 0, 0x1F), Row::Here(0x1E));
        assert_eq!(p.slot("Actor", 0xB0, 2), Row::Missing);
        assert_eq!(p.slot("Actor", 0, 3), Row::Unverified);
        assert_eq!(p.slot("Actor", 0, 2), Row::Same);
    }

    #[test]
    fn sizes() {
        let p = with(HEADER, "size Actor 2b0\nsize-missing Gone\nsize-unverified Player\n").unwrap();
        assert_eq!(p.size("Gone"), Row::Missing);
        assert_eq!(p.size("Actor"), Row::Here(0x2B0));
        assert_eq!(p.size("Player"), Row::Unverified);
        assert_eq!(p.size("TESForm"), Row::Same);
    }

    #[test]
    fn bases() {
        let p = with(HEADER, "base Actor:TESForm 10\nbase-missing Actor:Gone\nbase-unverified Actor:Unsure\n").unwrap();
        assert_eq!(p.base("Actor:TESForm"), Row::Here(0x10));
        assert_eq!(p.base("Actor:Gone"), Row::Missing);
        assert_eq!(p.base("Actor:Unsure"), Row::Unverified);
        assert_eq!(p.base("Actor:Other"), Row::Same);
    }

    #[test]
    fn sites() {
        let p = with(HEADER, "site 100 1a 2b e8ff\nsite 101 1a 2b e8 90\nsite-missing 102 4\nsite-unverified 103 4\n").unwrap();
        assert_eq!(p.site(100, 0x1A), Row::Here(&Site { offset: 0x2B, expected: vec![0xE8, 0xFF], replacement: None }));
        assert_eq!(p.site(101, 0x1A), Row::Here(&Site { offset: 0x2B, expected: vec![0xE8], replacement: Some(vec![0x90]) }));
        assert_eq!(p.site(102, 4), Row::Missing);
        assert_eq!(p.site(103, 4), Row::Unverified);
        assert_eq!(p.site(100, 4), Row::Unverified, "no row off 1.7.104 was never compared");
        assert_eq!(parse("skyrim-rs-profile 1\nruntime 1.7.104\naddress-library versionlib-1-7-104-0.bin\ndatabase ae\n").unwrap().site(100, 4), Row::Same);
    }

    #[test]
    fn enums() {
        let p = with(HEADER, "enum InputContext.Favor 10\nenum-missing InputContext.Marketplace\n").unwrap();
        assert_eq!(p.enum_value("InputContext.Favor"), Row::Here(0x10));
        assert_eq!(p.enum_value("InputContext.Marketplace"), Row::Missing);
        assert_eq!(p.enum_value("InputContext.Console"), Row::Same);
    }

    #[test]
    fn malformed_rows_are_refused_with_their_line() {
        for (row, line) in [("id 1\n", 5), ("id x 2\n", 5), ("member Actor 10\n", 5), ("member Actor.x zz\n", 5), ("slot Actor.1 2\n", 5), ("site 1 2 3 e\n", 5), ("site 1 2 3 e8 9\n", 5), ("base Actor 10\n", 5), ("unknown 1\n", 5), ("\n", 5), ("id  1 2\n", 5), ("id 1 2\nid 1 3\n", 6), ("id 1 2\nruntime 1.5.97\n", 6)] {
            let error = with(HEADER, row).unwrap_err();
            assert!(error.starts_with(&format!("line {line}: bad row")), "{row:?}: {error}");
        }
    }

    #[test]
    fn bad_headers_are_refused() {
        for text in ["", "something else\n", "skyrim-rs-profile x\n", "skyrim-rs-profile 0\n", "skyrim-rs-profile 1\n", "skyrim-rs-profile 1\nruntime 1.5\n", "skyrim-rs-profile 1\nruntime 1.5.97\naddress-library a.bin\ndatabase vr\n"] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_newer_header_is_refused_whatever_follows() {
        let error = parse("skyrim-rs-profile 2\nnew things\n").unwrap_err();
        assert!(error.contains("version 2") && error.contains("newer"), "{error}");
    }

    #[test]
    fn canonical_with_no_file_is_empty() {
        let dir = temp_plugins("canonical");
        let p = load(runtime::CANONICAL, &dir).unwrap();
        assert_eq!(p.address_library(), "versionlib-1-7-104-0.bin");
        assert_eq!(p.id(12345), Row::Same);
        assert_eq!(p.member("Actor.process"), Row::Same);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_runtime_with_no_file_names_the_runtime_and_the_path() {
        let dir = temp_plugins("missing");
        let error = load(runtime::pack(1, 5, 97, 0), &dir).unwrap_err();
        assert!(error.contains("1.5.97"), "{error}");
        assert!(error.contains(&dir.join("skyrim-runtime-extender").join("profiles").join("1-5-97.profile").display().to_string()), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_is_read_and_must_describe_its_runtime() {
        let dir = temp_plugins("present");
        let profiles = dir.join("skyrim-runtime-extender").join("profiles");
        std::fs::write(profiles.join("1-5-97.profile"), format!("{HEADER}id 1 2\n")).unwrap();
        assert_eq!(load(runtime::pack(1, 5, 97, 0), &dir).unwrap().id(1), Row::Here(2));
        std::fs::write(profiles.join("1-6-353.profile"), HEADER).unwrap();
        assert!(load(runtime::pack(1, 6, 353, 0), &dir).unwrap_err().contains("describes 1.5.97"));
        std::fs::write(profiles.join("1-7-104.profile"), "skyrim-rs-profile 9\n").unwrap();
        assert!(load(runtime::CANONICAL, &dir).unwrap_err().contains("newer"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_sixty_thousand_ids() {
        let mut text = HEADER.to_owned();
        for id in 0..60_000u32 {
            writeln!(text, "id {} {}", id * 3 + 10, id * 2 + 5).unwrap();
        }
        let start = std::time::Instant::now();
        let p = parse(&text).unwrap();
        eprintln!("60000 id rows, {} bytes: {:?}", text.len(), start.elapsed());
        assert_eq!(p.id(10), Row::Here(5));
        assert_eq!(p.id(59_999 * 3 + 10), Row::Here(59_999 * 2 + 5));
        assert_eq!(p.id(11), Row::Missing);
    }
}
