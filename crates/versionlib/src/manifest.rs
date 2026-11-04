//! The `.sremf` section of a DLL built on the library: what its code uses of the game, one record
//! per use, read from the file without running it. `docs/subsystems/runtimes.md` is the contract.

use std::collections::BTreeSet;
use std::fmt;

use crate::VersionDb;
use crate::profile::{Profile, Row};

const SECTION: &[u8; 8] = b".sremf\0\0";

/// One thing a DLL uses of the game, named as its profile row names it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Record {
    Id(u32),
    Member(String),
    Slot { class: String, vtable_offset: u32, index: u32 },
    Site { id: u32, offset: u32 },
    Size(String),
    Base(String),
    /// `Enum.Value`.
    Enum(String),
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "id {id}"),
            Self::Member(member) => write!(f, "member {member}"),
            Self::Slot { class, vtable_offset, index } => write!(f, "slot {class}[{vtable_offset:x}].{index:x}"),
            Self::Site { id, offset } => write!(f, "site {id} {offset:x}"),
            Self::Size(class) => write!(f, "size {class}"),
            Self::Base(class_and_base) => write!(f, "base {class_and_base}"),
            Self::Enum(value) => write!(f, "enum {value}"),
        }
    }
}

/// The records of a section's bytes, deduplicated and sorted. A zero kind byte is linker padding.
pub fn records(section: &[u8]) -> Result<BTreeSet<Record>, String> {
    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(&kind) = section.get(at) {
        at += 1;
        if kind == 0 {
            continue;
        }
        let field = |at: usize| section.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let (Some(a), Some(b)) = (field(at), field(at + 4)) else { return Err(format!("record at {:#x} is cut short", at - 1)) };
        let name = section.get(at + 8..).unwrap_or_default();
        let Some(len) = name.iter().position(|&c| c == 0) else { return Err(format!("record at {:#x} has no end to its name", at - 1)) };
        let name = core::str::from_utf8(&name[..len]).map_err(|_| format!("record at {:#x} has a name that is not text", at - 1))?.to_owned();
        out.insert(match kind {
            1 => Record::Id(a),
            2 => Record::Member(name),
            3 => Record::Slot { class: name, vtable_offset: a, index: b },
            4 => Record::Site { id: a, offset: b },
            5 => Record::Size(name),
            6 => Record::Base(name),
            7 => Record::Enum(name),
            _ => return Err(format!("record at {:#x} has unknown kind {kind}", at - 1)),
        });
        at += 8 + len + 1;
    }
    Ok(out)
}

fn little_endian<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at + N)?.try_into().ok()
}

/// The manifest section of a PE file's bytes, `None` when the file has none.
fn find_section(pe: &[u8]) -> Result<Option<&[u8]>, String> {
    const NOT_PE: &str = "not a PE file";
    let u16_at = |at| little_endian::<2>(pe, at).map(u16::from_le_bytes).ok_or(NOT_PE);
    let u32_at = |at| little_endian::<4>(pe, at).map(u32::from_le_bytes).ok_or(NOT_PE);
    let header = u32_at(0x3C)? as usize;
    if pe.get(..2) != Some(b"MZ") || pe.get(header..header + 4) != Some(b"PE\0\0") {
        return Err(NOT_PE.to_owned());
    }
    let count = usize::from(u16_at(header + 6)?);
    let table = header + 24 + usize::from(u16_at(header + 20)?);
    for entry in (0..count).map(|i| table + i * 40) {
        if pe.get(entry..entry + 8) == Some(SECTION) {
            let (virtual_size, raw_size, raw_at) = (u32_at(entry + 8)? as usize, u32_at(entry + 16)? as usize, u32_at(entry + 20)? as usize);
            return pe.get(raw_at..raw_at + virtual_size.min(raw_size)).map(Some).ok_or_else(|| "the manifest section lies past the end of the file".to_owned());
        }
    }
    Ok(None)
}

/// The manifest section of a PE file's bytes; empty when the file has none.
pub fn section(pe: &[u8]) -> Result<&[u8], String> {
    Ok(find_section(pe)?.unwrap_or_default())
}

/// The records of a PE file's bytes.
pub fn read(pe: &[u8]) -> Result<BTreeSet<Record>, String> {
    records(section(pe)?)
}

/// The records of a PE file's bytes, `None` for a DLL with no section: built before the manifest.
pub fn declared(pe: &[u8]) -> Result<Option<BTreeSet<Record>>, String> {
    find_section(pe)?.map(records).transpose()
}

/// Why a profile cannot serve a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lack {
    Missing,
    Unverified,
}

impl fmt::Display for Lack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "missing",
            Self::Unverified => "unverified",
        })
    }
}

fn lack<T>(row: &Row<T>) -> Option<Lack> {
    match row {
        Row::Same | Row::Here(_) => None,
        Row::Missing => Some(Lack::Missing),
        Row::Unverified => Some(Lack::Unverified),
    }
}

fn profile_lack(record: &Record, profile: &Profile) -> Option<Lack> {
    match record {
        Record::Id(id) => lack(&profile.id(*id)),
        Record::Member(member) => lack(&profile.member(member)),
        Record::Slot { class, vtable_offset, index } => lack(&profile.slot(class, *vtable_offset, *index)),
        Record::Site { id, offset } => lack(&profile.site(*id, *offset)),
        Record::Size(class) => lack(&profile.size(class)),
        Record::Base(class_and_base) => lack(&profile.base(class_and_base)),
        Record::Enum(value) => lack(&profile.enum_value(value)),
    }
}

/// What `profile` lacks of `records`. An id with no row on an AE database is not reported: the
/// Address Library file decides whether it exists.
pub fn lacking<'a>(records: impl IntoIterator<Item = &'a Record>, profile: &Profile) -> Vec<(&'a Record, Lack)> {
    records.into_iter().filter_map(|record| Some((record, profile_lack(record, profile)?))).collect()
}

/// What `profile` and its Address Library file lack of `records`: an id the profile leaves usable
/// that the file does not hold is missing. With no file, as [`lacking`].
pub fn lacking_in<'a>(records: impl IntoIterator<Item = &'a Record>, profile: &Profile, db: Option<&VersionDb>) -> Vec<(&'a Record, Lack)> {
    records
        .into_iter()
        .filter_map(|record| {
            let lacks = profile_lack(record, profile).or_else(|| match (record, db) {
                (Record::Id(id), Some(db)) => {
                    let here = if let Row::Here(here) = profile.id(*id) { here } else { *id };
                    db.offset(u64::from(here)).is_none().then_some(Lack::Missing)
                }
                _ => None,
            });
            Some((record, lacks?))
        })
        .collect()
}

/// Whether a DLL fits a runtime.
#[derive(Debug, PartialEq, Eq)]
pub enum Fit {
    Fits,
    /// Everything it uses that the runtime lacks.
    Lacks(Vec<(Record, Lack)>),
    /// No section, on a runtime other than 1.7.104, the one every older build was written for.
    BeforeManifest,
    Unreadable(String),
}

impl Fit {
    /// How many records are missing and how many unverified.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let Self::Lacks(lacks) = self else { return (0, 0) };
        let missing = lacks.iter().filter(|(_, why)| *why == Lack::Missing).count();
        (missing, lacks.len() - missing)
    }
}

/// How a DLL with these bytes fits a runtime.
#[must_use]
pub fn fit(pe: &[u8], profile: &Profile, db: Option<&VersionDb>) -> Fit {
    match declared(pe) {
        Err(e) => Fit::Unreadable(e),
        Ok(None) if skse::runtime::is_canonical(profile.runtime()) => Fit::Fits,
        Ok(None) => Fit::BeforeManifest,
        Ok(Some(records)) => match lacking_in(&records, profile, db).as_slice() {
            [] => Fit::Fits,
            lacks => Fit::Lacks(lacks.iter().map(|(record, why)| ((*record).clone(), *why)).collect()),
        },
    }
}

/// Why a DLL that does not fit `runtime` is refused, with the complete list of what it lacks;
/// `None` when it fits.
#[must_use]
pub fn refusal(fit: &Fit, runtime: &str) -> Option<String> {
    match fit {
        Fit::Fits => None,
        Fit::Unreadable(e) => Some(format!("its manifest cannot be read: {e}")),
        Fit::BeforeManifest => Some(format!("it was built before the manifest, so nothing says it fits {runtime}; it needs a rebuild against this library")),
        Fit::Lacks(lacks) => {
            let group = |word: &str, kind: Lack| {
                let items: Vec<String> = lacks.iter().filter(|(_, why)| *why == kind).map(|(record, _)| record.to_string()).collect();
                (!items.is_empty()).then(|| format!("{} {word} ({})", items.len(), items.join(", ")))
            };
            let groups: Vec<String> = [group("missing", Lack::Missing), group("unverified", Lack::Unverified)].into_iter().flatten().collect();
            Some(format!("{runtime} lacks what it uses: {}", groups.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u8, a: u32, b: u32, name: &str) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend(a.to_le_bytes());
        out.extend(b.to_le_bytes());
        out.extend(name.as_bytes());
        out.push(0);
        out
    }

    #[test]
    fn walks_records_over_padding_and_deduplicates() {
        let mut bytes = record(1, 7, 0, "");
        bytes.extend([0, 0, 0]);
        bytes.extend(record(2, 0, 0, "Actor.process"));
        bytes.extend(record(1, 7, 0, ""));
        bytes.extend(record(3, 0xB0, 5, "Actor"));
        bytes.extend(record(4, 9, 0x1A, ""));
        bytes.extend(record(5, 0, 0, "Actor"));
        bytes.extend(record(6, 0, 0, "Actor:TESForm"));
        bytes.extend(record(7, 0, 0, "InputContext.Favor"));
        bytes.extend([0; 16]);
        let read: Vec<String> = records(&bytes).unwrap().iter().map(ToString::to_string).collect();
        assert_eq!(read, ["id 7", "member Actor.process", "slot Actor[b0].5", "site 9 1a", "size Actor", "base Actor:TESForm", "enum InputContext.Favor"]);
    }

    #[test]
    fn refuses_what_is_not_a_record() {
        assert!(records(&[1, 0, 0]).unwrap_err().contains("cut short"));
        assert!(records(&[1, 0, 0, 0, 0, 0, 0, 0, 0, b'a']).unwrap_err().contains("no end"));
        assert!(records(&record(9, 0, 0, "")).unwrap_err().contains("unknown kind"));
    }

    fn pe_with(name: [u8; 8], data: &[u8]) -> Vec<u8> {
        let mut pe = vec![0; 0x200];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        pe[0x54..0x56].copy_from_slice(&0xF0u16.to_le_bytes());
        let entry = 0x58 + 0xF0;
        pe[entry..entry + 8].copy_from_slice(&name);
        pe[entry + 8..entry + 12].copy_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        pe[entry + 16..entry + 20].copy_from_slice(&0x200u32.to_le_bytes());
        pe[entry + 20..entry + 24].copy_from_slice(&0x200u32.to_le_bytes());
        pe.extend(data);
        pe.resize(0x400, 0);
        pe
    }

    #[test]
    fn finds_the_section_by_name_and_trims_file_padding() {
        let data = record(1, 42, 0, "");
        assert_eq!(read(&pe_with(*SECTION, &data)).unwrap().into_iter().collect::<Vec<_>>(), [Record::Id(42)]);
        assert!(read(&pe_with(*b".text\0\0\0", &data)).unwrap().is_empty());
        assert!(section(b"not a pe").is_err());
    }

    const CANONICAL_HEADER: &str = "skyrim-rs-profile 1\nruntime 1.7.104\naddress-library versionlib-1-7-104-0.bin\ndatabase ae\n";

    /// A format 5 file holding offset `id + 0x1000` for each of `ids`.
    fn db_holding(name: &str, ids: &[u32]) -> VersionDb {
        let count = ids.iter().max().map_or(0, |&m| m + 1);
        let mut file = Vec::new();
        for field in [5u32, 1, 7, 104, 0] {
            file.extend(field.to_le_bytes());
        }
        file.extend([0; 64]);
        for field in [8u32, 0, count] {
            file.extend(field.to_le_bytes());
        }
        for id in 0..count {
            file.extend((if ids.contains(&id) { id + 0x1000 } else { 0 }).to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("versionlib-manifest-{}-{name}.bin", std::process::id()));
        std::fs::write(&path, file).unwrap();
        VersionDb::open(&path).unwrap()
    }

    #[test]
    fn an_id_the_profile_leaves_to_the_file_is_checked_against_it() {
        let db = db_holding("ids", &[7, 9]);
        let profile = crate::profile::parse(&format!("{CANONICAL_HEADER}id-unverified 9\n")).unwrap();
        let wanted = [Record::Id(7), Record::Id(8), Record::Id(9)];
        assert_eq!(lacking_in(&wanted, &profile, None).len(), 1, "with no file only the profile's rows count");
        let found: Vec<String> = lacking_in(&wanted, &profile, Some(&db)).iter().map(|(r, why)| format!("{r} {why}")).collect();
        assert_eq!(found, ["id 8 missing", "id 9 unverified"]);
        let se = crate::profile::parse("skyrim-rs-profile 1\nruntime 1.5.97\naddress-library version-1-5-97-0.bin\ndatabase se\nid 100 7\nid 101 8\n").unwrap();
        let wanted = [Record::Id(100), Record::Id(101), Record::Id(102)];
        let found: Vec<String> = lacking_in(&wanted, &se, Some(&db)).iter().map(|(r, why)| format!("{r} {why}")).collect();
        assert_eq!(found, ["id 101 missing", "id 102 missing"], "an SE id is looked up translated");
    }

    #[test]
    fn a_refusal_names_everything_lacking() {
        let db = db_holding("refusal", &[1]);
        let mut section = record(1, 1, 0, "");
        section.extend(record(1, 2, 0, ""));
        section.extend(record(2, 0, 0, "Actor.a"));
        section.extend(record(4, 1, 0x10, ""));
        section.extend(record(3, 0, 5, "Actor"));
        section.extend(record(5, 0, 0, "Actor"));
        section.extend(record(6, 0, 0, "Actor:TESForm"));
        let pe = pe_with(*SECTION, &section);
        let profile = crate::profile::parse(&format!("{CANONICAL_HEADER}member-missing Actor.a\nsite-unverified 1 10\nslot-unverified Actor[0].5\nsize-missing Actor\nbase-unverified Actor:TESForm\n")).unwrap();
        let found = fit(&pe, &profile, Some(&db));
        assert_eq!(found.counts(), (3, 3));
        assert_eq!(refusal(&found, &profile.name()).unwrap(), "1.7.104 lacks what it uses: 3 missing (id 2, member Actor.a, size Actor), 3 unverified (slot Actor[0].5, site 1 10, base Actor:TESForm)");
        let clean = crate::profile::parse(CANONICAL_HEADER).unwrap();
        assert_eq!(fit(&pe_with(*SECTION, &record(1, 1, 0, "")), &clean, Some(&db)), Fit::Fits);
    }

    #[test]
    fn a_dll_with_no_section_is_safe_only_on_the_canonical_runtime() {
        let db = db_holding("none", &[1]);
        let old = pe_with(*b".text\0\0\0", &[]);
        assert_eq!(declared(&old).unwrap(), None);
        assert_eq!(fit(&old, &crate::profile::parse(CANONICAL_HEADER).unwrap(), Some(&db)), Fit::Fits);
        let other = crate::profile::parse("skyrim-rs-profile 1\nruntime 1.6.1170\naddress-library versionlib-1-6-1170-0.bin\ndatabase ae\n").unwrap();
        let before = fit(&old, &other, Some(&db));
        assert_eq!(before, Fit::BeforeManifest);
        assert!(refusal(&before, "1.6.1170").unwrap().contains("built before the manifest"));
        assert_eq!(declared(&pe_with(*SECTION, &[0])).unwrap(), Some(BTreeSet::new()), "padding alone is a section with nothing in it");
        assert_eq!(fit(&pe_with(*SECTION, &[0]), &other, Some(&db)), Fit::Fits);
    }

    #[test]
    fn a_broken_manifest_is_refused() {
        let db = db_holding("broken", &[1]);
        let found = fit(&pe_with(*SECTION, &[1, 0, 0]), &crate::profile::parse(CANONICAL_HEADER).unwrap(), Some(&db));
        assert!(refusal(&found, "1.7.104").unwrap().contains("cannot be read"));
    }

    #[test]
    fn reports_what_a_profile_lacks() {
        let profile = crate::profile::parse("skyrim-rs-profile 1\nruntime 1.5.97\naddress-library version-1-5-97-0.bin\ndatabase se\nid 1 2\nid-unverified 3\nmember-unverified Actor.a\nslot-missing Actor[0].5\n").unwrap();
        let wanted = [Record::Id(1), Record::Id(2), Record::Id(3), Record::Member("Actor.a".into()), Record::Member("Actor.b".into()), Record::Slot { class: "Actor".into(), vtable_offset: 0, index: 5 }];
        let found: Vec<String> = lacking(&wanted, &profile).iter().map(|(r, why)| format!("{r} {why}")).collect();
        assert_eq!(found, ["id 2 missing", "id 3 unverified", "member Actor.a unverified", "slot Actor[0].5 missing"]);
    }
}
