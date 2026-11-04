//! A runtime profile as `cargo xtask gen` writes it; `versionlib::profile` reads the text and
//! `docs/subsystems/runtimes.md` is the format's contract.

use std::fmt::{self, Write};

const FORMAT_VERSION: u32 = 1;

pub struct Profile {
    pub runtime: [u32; 3],
    pub address_library: String,
    pub se_database: bool,
    pub rows: Vec<Row>,
}

pub enum Row {
    Id { ae: u32, se: u32 },
    IdMissing(u32),
    IdUnverified(u32),
    Member { member: String, offset: u32 },
    MemberMissing(String),
    MemberUnverified(String),
    Slot { slot: Slot, here: u32 },
    SlotMissing(Slot),
    SlotUnverified(Slot),
    Size { class: String, size: u32 },
    SizeUnverified(String),
    Base { class_and_base: String, offset: u32 },
    Site { ae: u32, canonical: u32, here: u32, expected: Vec<u8>, replacement: Option<Vec<u8>> },
    SiteMissing { ae: u32, canonical: u32 },
    SiteUnverified { ae: u32, canonical: u32 },
}

pub struct Slot {
    pub class: String,
    pub vtable_offset: u32,
    pub index: u32,
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{:x}].{:x}", self.class, self.vtable_offset, self.index)
    }
}

struct Bytes<'a>(&'a [u8]);

impl fmt::Display for Bytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id { ae, se } => write!(f, "id {ae} {se}"),
            Self::IdMissing(ae) => write!(f, "id-missing {ae}"),
            Self::IdUnverified(ae) => write!(f, "id-unverified {ae}"),
            Self::Member { member, offset } => write!(f, "member {member} {offset:x}"),
            Self::MemberMissing(member) => write!(f, "member-missing {member}"),
            Self::MemberUnverified(member) => write!(f, "member-unverified {member}"),
            Self::Slot { slot, here } => write!(f, "slot {slot} {here:x}"),
            Self::SlotMissing(slot) => write!(f, "slot-missing {slot}"),
            Self::SlotUnverified(slot) => write!(f, "slot-unverified {slot}"),
            Self::Size { class, size } => write!(f, "size {class} {size:x}"),
            Self::SizeUnverified(class) => write!(f, "size-unverified {class}"),
            Self::Base { class_and_base, offset } => write!(f, "base {class_and_base} {offset:x}"),
            Self::Site { ae, canonical, here, expected, replacement } => {
                write!(f, "site {ae} {canonical:x} {here:x} {}", Bytes(expected))?;
                replacement.as_ref().map_or(Ok(()), |bytes| write!(f, " {}", Bytes(bytes)))
            }
            Self::SiteMissing { ae, canonical } => write!(f, "site-missing {ae} {canonical:x}"),
            Self::SiteUnverified { ae, canonical } => write!(f, "site-unverified {ae} {canonical:x}"),
        }
    }
}

impl Profile {
    /// The canonical runtime's profile: the header alone, since 1.7.104 is its own difference.
    #[must_use]
    pub fn canonical() -> Self {
        Self { runtime: [1, 7, 104], address_library: "versionlib-1-7-104-0.bin".to_owned(), se_database: false, rows: Vec::new() }
    }

    /// Path relative to the profile folder.
    #[must_use]
    pub fn file_name(&self) -> String {
        let [major, minor, build] = self.runtime;
        format!("{major}-{minor}-{build}.profile")
    }

    #[must_use]
    pub fn text(&self) -> String {
        let [major, minor, build] = self.runtime;
        let database = if self.se_database { "se" } else { "ae" };
        let mut out = format!("skyrim-rs-profile {FORMAT_VERSION}\nruntime {major}.{minor}.{build}\naddress-library {}\ndatabase {database}\n", self.address_library);
        for row in &self.rows {
            writeln!(out, "{row}").unwrap();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use versionlib::profile::{Row as Read, Site, parse};

    fn slot(index: u32) -> Slot {
        Slot { class: "Actor".to_owned(), vtable_offset: 0xB0, index }
    }

    #[test]
    fn canonical_is_the_header_alone() {
        let profile = Profile::canonical();
        assert_eq!(profile.file_name(), "1-7-104.profile");
        assert_eq!(profile.text(), "skyrim-rs-profile 1\nruntime 1.7.104\naddress-library versionlib-1-7-104-0.bin\ndatabase ae\n");
    }

    #[test]
    fn the_reader_reads_every_row_kind_written() {
        let text = Profile {
            runtime: [1, 5, 97],
            address_library: "version-1-5-97-0.bin".to_owned(),
            se_database: true,
            rows: vec![
                Row::Id { ae: 100, se: 200 },
                Row::IdMissing(101),
                Row::IdUnverified(102),
                Row::Member { member: "Actor.a".to_owned(), offset: 0x1F8 },
                Row::MemberMissing("Actor.b".to_owned()),
                Row::MemberUnverified("Actor.c".to_owned()),
                Row::Slot { slot: slot(0x1F), here: 0x1E },
                Row::SlotMissing(slot(2)),
                Row::SlotUnverified(slot(3)),
                Row::Size { class: "Actor".to_owned(), size: 0x2B0 },
                Row::SizeUnverified("Player".to_owned()),
                Row::Base { class_and_base: "Actor:TESForm".to_owned(), offset: 0x10 },
                Row::Site { ae: 100, canonical: 0x1A, here: 0x2B, expected: vec![0xE8, 0x0F], replacement: None },
                Row::Site { ae: 100, canonical: 0x1B, here: 0x2C, expected: vec![0xE8], replacement: Some(vec![0x90, 0x90]) },
                Row::SiteMissing { ae: 101, canonical: 4 },
                Row::SiteUnverified { ae: 102, canonical: 4 },
            ],
        }
        .text();
        let read = parse(&text).unwrap();
        assert!(read.se_database());
        assert_eq!(read.address_library(), "version-1-5-97-0.bin");
        assert_eq!((read.id(100), read.id(101), read.id(102)), (Read::Here(200), Read::Missing, Read::Unverified));
        assert_eq!((read.member("Actor.a"), read.member("Actor.b"), read.member("Actor.c")), (Read::Here(0x1F8), Read::Missing, Read::Unverified));
        assert_eq!((read.slot("Actor", 0xB0, 0x1F), read.slot("Actor", 0xB0, 2), read.slot("Actor", 0xB0, 3)), (Read::Here(0x1E), Read::Missing, Read::Unverified));
        assert_eq!((read.size("Actor"), read.size("Player")), (Read::Here(0x2B0), Read::Unverified));
        assert_eq!(read.base("Actor:TESForm"), Read::Here(0x10));
        assert_eq!(read.site(100, 0x1A), Read::Here(&Site { offset: 0x2B, expected: vec![0xE8, 0x0F], replacement: None }));
        assert_eq!(read.site(100, 0x1B), Read::Here(&Site { offset: 0x2C, expected: vec![0xE8], replacement: Some(vec![0x90, 0x90]) }));
        assert_eq!((read.site(101, 4), read.site(102, 4)), (Read::Missing, Read::Unverified));
    }
}
