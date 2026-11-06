// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The RE repository's difference files, `export/versions/<runtime>.json`, and what is made of them:
//! the set of items that are not plain on every runtime, and one profile per runtime.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::PROFILE_DIR;
use crate::inputs::Inputs;
use crate::profile::{Profile, Row as Written, Slot};

pub const CANONICAL: &str = "1.7.104";

/// The runtimes that decide plain versus accessor. 1.7.99 stays out: it has no install and every
/// layout unverified, so it would turn every field into an accessor, and it gets no profile.
pub const SUPPORTED: [&str; 4] = ["1.5.97", "1.6.353", "1.6.1170", CANONICAL];

/// Managed patches' bytes on each runtime, which the difference files do not give.
const SITE_PATCHES: &str = include_str!("../site-patches.txt");

/// `{"columns": [...], "rows": [[...], ...]}`, read by column name so added columns change nothing.
#[derive(Default, Deserialize)]
struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

struct Cells<'a> {
    columns: &'a HashMap<&'a str, usize>,
    cells: &'a [Value],
}

impl Cells<'_> {
    fn cell(&self, column: &str) -> &Value {
        self.columns.get(column).and_then(|&i| self.cells.get(i)).unwrap_or(&Value::Null)
    }

    fn text(&self, column: &str) -> Option<&str> {
        self.cell(column).as_str()
    }

    /// An integer cell, or a hex string such as `0x8f8680`.
    fn number(&self, column: &str) -> Option<u64> {
        let cell = self.cell(column);
        cell.as_u64().or_else(|| u64::from_str_radix(cell.as_str()?.trim_start_matches("0x"), 16).ok())
    }

    fn u32(&self, column: &str) -> Option<u32> {
        u32::try_from(self.number(column)?).ok()
    }

    fn name(&self, column: &str) -> String {
        self.text(column).unwrap_or_default().to_owned()
    }

    /// Hex bytes, spaced or not.
    fn bytes(&self, column: &str) -> Option<Vec<u8>> {
        let hex: String = self.text(column)?.chars().filter(|c| !c.is_whitespace()).collect();
        if hex.is_empty() || !hex.len().is_multiple_of(2) {
            return None;
        }
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
    }
}

impl Table {
    fn for_each(&self, mut f: impl FnMut(Cells)) {
        let columns: HashMap<&str, usize> = self.columns.iter().enumerate().map(|(i, c)| (c.as_str(), i)).collect();
        for cells in &self.rows {
            f(Cells { columns: &columns, cells });
        }
    }
}

/// What a runtime has for one canonical item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diff {
    Same,
    Value(u32),
    Missing,
    Unverified,
}

impl Diff {
    /// `same`, `moved`/`changed` with the runtime's value, `absent`. Anything else, a changed row
    /// without a value included, is unverified: it must never read as identical.
    fn of(status: Option<&str>, value: Option<u64>) -> Self {
        match (status, value) {
            (Some("same"), _) => Self::Same,
            (Some("moved" | "changed"), Some(value)) => u32::try_from(value).map_or(Self::Unverified, Self::Value),
            (Some("absent"), _) => Self::Missing,
            _ => Self::Unverified,
        }
    }

    /// A field, slot, base or class size, where a wrong value is a silently wrong offset the
    /// manifest cannot see. Status comes first: review and confidence never make an `unverified`
    /// row usable. A value at any confidence, any row at `low` and a `same` at `medium` from RE's
    /// `statics.py` (evidence `hand: statics`) are unverified unless a review accepts them, and
    /// `rejected` is never usable. Other `same` at `medium` stays usable, and the suites are its
    /// only check.
    fn checked(row: &Cells, status: &str, value: &str) -> Self {
        let diff = Self::of(row.text(status), row.number(value));
        let confidence = row.text("confidence");
        let scripted = confidence == Some("medium") && diff == Self::Same && row.text("evidence").is_some_and(|e| e.starts_with("hand: statics"));
        let weak = scripted || confidence == Some("low") || matches!(diff, Self::Value(_));
        if row.text("review") == Some("rejected") || (weak && !accepted(row)) { Self::Unverified } else { diff }
    }
}

/// An AE id on one runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdState {
    /// The id the runtime's Address Library holds it under.
    Mapped(u32),
    /// The function is known here, but that library holds no id at its address.
    Missing,
    Unverified,
    /// The address is good and the signature was not compared, with the id here when it differs.
    Address(Option<u32>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Site {
    /// Compared: where it is here and what sits there, `same` or not.
    Here { same: bool, offset: u32, bytes: Vec<u8>, replacement: Option<Vec<u8>> },
    Missing,
    Unverified,
}

#[derive(Deserialize)]
struct AddressLibrary {
    file: String,
    database: String,
}

#[derive(Deserialize)]
struct File {
    schema: u64,
    runtime: String,
    address_library: AddressLibrary,
    #[serde(default)]
    functions: Table,
    #[serde(default)]
    classes: Table,
    #[serde(default)]
    bases: Table,
    #[serde(default)]
    fields: Table,
    #[serde(default)]
    vtables: Table,
    #[serde(default)]
    slots: Table,
    #[serde(default)]
    globals: Table,
    #[serde(default)]
    sites: Table,
    #[serde(default)]
    id_map: Table,
    #[serde(default)]
    enums: Table,
}

/// One runtime's differences from 1.7.104.
pub struct Runtime {
    pub name: String,
    address_library: String,
    /// `se` or `ae`.
    database: String,
    members: HashMap<(String, String), Diff>,
    sizes: HashMap<String, Diff>,
    bases: HashMap<(String, String), Diff>,
    /// Keyed by class, vtable offset and canonical index.
    slots: HashMap<(String, u32, u32), Diff>,
    /// Keyed by AE id and canonical offset.
    sites: HashMap<(u32, u32), Site>,
    /// Keyed by enum and value, only the values that moved or are absent: `unverified` is no
    /// evidence of a difference and most enums have such rows on 1.7.104 itself.
    enums: BTreeMap<(String, String), Diff>,
    ids: BTreeMap<u32, IdState>,
    /// Ids with a row here, so a ledger id without one can be told apart.
    seen: HashSet<u32>,
}

/// Whether a review accepted this row and says on what basis; RE's older reviews have none and
/// were not re-read, so they count as no review.
fn accepted(row: &Cells) -> bool {
    row.text("review") == Some("accepted") && row.text("review_basis").is_some()
}

/// How a reviewed function row's note says that only its address was read.
const ADDRESS_ONLY: &str = "the verdict covers the address only";

/// Whether an id's own row lets it be used on a runtime.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Usable {
    Yes,
    Address,
    Unverified,
    Rejected,
}

/// A row's status and review. `rejected` is never usable, and `accepted` on a `changed` signature is
/// a real parameter change: the runtime's prototype applies, which the generator cannot express yet,
/// so only the address is good, as it is on a row whose note says the verdict covers nothing more.
/// An identical body reads the same arguments, since the comparison masks only addresses, so it is
/// usable whatever its signature says; any other body needs a compared signature or a review.
/// Tables with no signature column (globals, vtables) need only a row that was compared.
fn usable(row: &Cells, signature: bool) -> Usable {
    let review = row.text("review");
    let status = row.text("status");
    let signature_status = row.text("signature_status");
    if review == Some("rejected") {
        Usable::Rejected
    } else if !signature {
        if status == Some("unverified") { Usable::Unverified } else { Usable::Yes }
    } else if accepted(row) && (signature_status == Some("changed") || row.text("note").is_some_and(|n| n.contains(ADDRESS_ONLY))) {
        Usable::Address
    } else if status == Some("same") || accepted(row) || signature_status == Some("same") {
        Usable::Yes
    } else {
        Usable::Unverified
    }
}

impl Runtime {
    fn of(file: File) -> Self {
        let database = file.address_library.database.to_ascii_lowercase();
        let canonical = file.runtime == CANONICAL;
        let diff = |row: &Cells, status: &str, value: &str| if canonical { Diff::of(row.text(status), row.number(value)) } else { Diff::checked(row, status, value) };
        let mut runtime = Self {
            name: file.runtime,
            address_library: file.address_library.file,
            database,
            members: HashMap::new(),
            sizes: HashMap::new(),
            bases: HashMap::new(),
            slots: HashMap::new(),
            sites: HashMap::new(),
            enums: BTreeMap::new(),
            ids: BTreeMap::new(),
            seen: HashSet::new(),
        };
        file.fields.for_each(|r| {
            runtime.members.insert((r.name("class"), r.name("name")), diff(&r, "status", "offset"));
        });
        file.classes.for_each(|r| {
            let status = if r.text("status") == Some("absent") { "status" } else { "size_status" };
            runtime.sizes.insert(r.name("name"), diff(&r, status, "size"));
        });
        file.bases.for_each(|r| {
            runtime.bases.insert((r.name("class"), r.name("base")), diff(&r, "status", "offset"));
        });
        file.slots.for_each(|r| {
            if let (Some(displacement), Some(index)) = (r.u32("displacement"), r.u32("index_canonical")) {
                runtime.slots.insert((r.name("class"), displacement, index), diff(&r, "status", "index"));
            }
        });
        file.sites.for_each(|r| {
            let (Some(id), Some(canonical)) = (r.u32("id"), r.u32("offset_canonical")) else { return };
            let site = match (r.text("status"), r.u32("offset"), r.bytes("bytes")) {
                (Some(status @ ("same" | "differs")), Some(offset), Some(bytes)) => Site::Here { same: status == "same", offset, bytes, replacement: r.bytes("replacement") },
                (Some("absent"), ..) | (Some("differs"), None, _) => Site::Missing,
                _ => Site::Unverified,
            };
            runtime.sites.insert((id, canonical), site);
        });
        file.enums.for_each(|r| {
            let value = Diff::of(r.text("status"), r.number("value"));
            if matches!(value, Diff::Value(_) | Diff::Missing) {
                runtime.enums.insert((r.name("enum"), r.name("name")), value);
            }
        });
        runtime.read_ids(&file.functions, &file.globals, &file.vtables, &file.id_map);
        runtime
    }

    /// Writes the rows of `table` for this runtime over its sites. A row that says otherwise than
    /// what the difference file compared means one of them is out of date.
    fn apply_site_patches(&mut self, table: &str) -> Result<(), String> {
        let hex_bytes = |text: &str| (0..text.len()).step_by(2).map(|i| text.get(i..i + 2).and_then(|byte| u8::from_str_radix(byte, 16).ok())).collect::<Option<Vec<u8>>>();
        for line in table.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let bad = || format!("malformed row: {line}");
            let fields: Vec<&str> = line.splitn(7, ' ').collect();
            let &[runtime, ae, canonical, here, expected, replacement, _how] = fields.as_slice() else { return Err(bad()) };
            if runtime != self.name {
                continue;
            }
            let key = (ae.parse().map_err(|_| bad())?, u32::from_str_radix(canonical, 16).map_err(|_| bad())?);
            let offset = u32::from_str_radix(here, 16).map_err(|_| bad())?;
            let bytes = hex_bytes(expected).ok_or_else(bad)?;
            let replacement = hex_bytes(replacement).ok_or_else(bad)?;
            match self.sites.get(&key) {
                Some(Site::Missing) => return Err(format!("{line}: the difference file says the site is absent")),
                Some(Site::Here { offset: compared, bytes: found, .. }) if *compared != offset || *found != bytes => return Err(format!("{line}: the difference file has +{compared:#x} and {found:02x?}")),
                _ => {}
            }
            self.sites.insert(key, Site::Here { same: false, offset, bytes, replacement: Some(replacement) });
        }
        Ok(())
    }

    fn read_ids(&mut self, functions: &Table, globals: &Table, vtables: &Table, id_map: &Table) {
        let se_database = self.database == "se";
        let mut rows: HashMap<u32, Usable> = HashMap::new();
        let mut read = |table: &Table, id: &str, signature: bool| {
            table.for_each(|r| {
                if r.text("status") != Some("absent")
                    && let Some(id) = r.u32(id)
                {
                    rows.insert(id, usable(&r, signature));
                }
            });
        };
        read(functions, "id", true);
        read(globals, "id", false);
        read(vtables, "id_canonical", false);
        self.seen = rows.keys().copied().collect();
        if !se_database {
            self.ids = rows.iter().filter(|&(_, &u)| u != Usable::Yes).map(|(&id, &u)| (id, if u == Usable::Address { IdState::Address(None) } else { IdState::Unverified })).collect();
        }
        vtables.for_each(|r| {
            if let (Some(ae), Some(here)) = (r.u32("id_canonical"), r.u32("id"))
                && rows.get(&ae) == Some(&Usable::Yes)
                && (se_database || ae != here)
            {
                self.ids.insert(ae, IdState::Mapped(here));
            }
        });
        id_map.for_each(|r| {
            let Some(ae) = r.u32("ae") else { return };
            let row = rows.get(&ae).copied().unwrap_or(Usable::Yes);
            if row == Usable::Rejected || r.text("review") == Some("rejected") {
                if !se_database {
                    self.ids.insert(ae, IdState::Unverified);
                }
                return;
            }
            let pair = accepted(&r) || r.text("confidence") == Some("high");
            let state = match r.u32("se") {
                None => IdState::Missing,
                Some(here) if row == Usable::Yes && pair => IdState::Mapped(here),
                Some(here) if row == Usable::Address && pair => IdState::Address(Some(here)),
                Some(_) => IdState::Unverified,
            };
            self.ids.insert(ae, state);
        });
    }

    /// Rows that are not `same`; on the canonical runtime's self-check this must be zero.
    #[must_use]
    pub fn differences(&self) -> usize {
        self.members.values().chain(self.sizes.values()).chain(self.bases.values()).chain(self.slots.values()).filter(|d| **d != Diff::Same).count()
            + self.sites.values().filter(|s| !matches!(s, Site::Here { same: true, .. })).count()
    }
}

pub type Hashes = Vec<(String, String)>;

/// Every difference file of a supported runtime that exists, in `SUPPORTED` order, and the
/// `(relative path, sha256)` of each. Generation works with any of them missing.
pub fn read(re_dir: &Path) -> Result<(Vec<Runtime>, Hashes), Box<dyn Error>> {
    let mut runtimes = Vec::new();
    let mut hashes = Vec::new();
    for name in SUPPORTED {
        let rel = format!("export/versions/{name}.json");
        let Ok(bytes) = fs::read(re_dir.join(&rel)) else { continue };
        hashes.push((rel.clone(), format!("{:x}", Sha256::digest(&bytes))));
        let mut runtime = parse(&bytes).map_err(|e| format!("{rel}: {e}"))?;
        runtime.apply_site_patches(SITE_PATCHES).map_err(|e| format!("xtask/site-patches.txt: {e}"))?;
        runtimes.push(runtime);
    }
    Ok((runtimes, hashes))
}

fn parse(bytes: &[u8]) -> Result<Runtime, Box<dyn Error>> {
    let file: File = serde_json::from_slice(bytes)?;
    if file.schema != 1 {
        return Err(format!("schema {} is not 1", file.schema).into());
    }
    Ok(Runtime::of(file))
}

/// The items that are not identical on every compared runtime: each is an accessor, and everything
/// else stays plain. 1.7.104 is the canonical layout by construction and is never compared.
#[derive(Default)]
pub struct Variance {
    members: HashSet<(String, String)>,
    sizes: HashSet<String>,
    bases: HashSet<(String, String)>,
    slots: HashSet<(String, u32, u32)>,
    /// Enums with a value that moved or is absent on a runtime.
    enums: HashSet<String>,
    /// The runtimes compared, in `SUPPORTED` order.
    pub compared: Vec<String>,
}

/// The class names of the ledger with their member names.
fn ledger_members(inp: &Inputs) -> impl Iterator<Item = (&str, &str)> {
    let layouts = inp.layouts.iter().flat_map(|(c, l)| l.fields.iter().map(move |f| (c.as_str(), f.name.as_str())));
    let structs = inp.structs.structs.iter().flat_map(|(c, s)| s.fields.iter().map(move |f| (c.as_str(), f.name.as_str())));
    layouts.chain(structs)
}

fn ledger_classes(inp: &Inputs) -> impl Iterator<Item = &str> {
    inp.layouts.keys().chain(inp.structs.structs.keys()).map(String::as_str)
}

fn ledger_bases(inp: &Inputs) -> impl Iterator<Item = (&str, &str)> {
    inp.rtti.classes.iter().flat_map(|c| c.bases.iter().skip(1).map(move |b| (c.name.as_str(), b.name.as_str())))
}

fn ledger_slots(inp: &Inputs) -> impl Iterator<Item = (&str, u32, u32)> {
    inp.rtti.classes.iter().flat_map(|c| c.cols.iter().flat_map(move |col| (0..col.slots.len()).map(move |i| (c.name.as_str(), u32::try_from(col.offset).unwrap(), u32::try_from(i).unwrap()))))
}

impl Variance {
    /// `runtimes` are the files read; a ledger item with no row in a file is not identical on it.
    #[must_use]
    pub fn of(inp: &Inputs, runtimes: &[Runtime]) -> Self {
        let compared: Vec<&Runtime> = runtimes.iter().filter(|r| r.name != CANONICAL).collect();
        let mut out = Self { compared: compared.iter().map(|r| r.name.clone()).collect(), ..Self::default() };
        if compared.is_empty() {
            return out;
        }
        for (class, member) in ledger_members(inp) {
            let key = (class.to_owned(), member.to_owned());
            if compared.iter().any(|r| r.members.get(&key) != Some(&Diff::Same)) {
                out.members.insert(key);
            }
        }
        for class in ledger_classes(inp) {
            if compared.iter().any(|r| r.sizes.get(class) != Some(&Diff::Same)) {
                out.sizes.insert(class.to_owned());
            }
        }
        for (class, base) in ledger_bases(inp) {
            let key = (class.to_owned(), base.to_owned());
            if compared.iter().any(|r| r.bases.get(&key) != Some(&Diff::Same)) {
                out.bases.insert(key);
            }
        }
        for (class, offset, index) in ledger_slots(inp) {
            let key = (class.to_owned(), offset, index);
            if compared.iter().any(|r| r.slots.get(&key) != Some(&Diff::Same)) {
                out.slots.insert(key);
            }
        }
        out.enums = compared.iter().flat_map(|r| r.enums.keys().map(|(name, _)| name.clone())).collect();
        out
    }

    pub(crate) fn member(&self, class: &str, member: &str) -> bool {
        !self.members.is_empty() && self.members.contains(&(class.to_owned(), member.to_owned()))
    }

    pub(crate) fn base(&self, class: &str, base: &str) -> bool {
        !self.bases.is_empty() && self.bases.contains(&(class.to_owned(), base.to_owned()))
    }

    pub(crate) fn enum_(&self, name: &str) -> bool {
        self.enums.contains(name)
    }

    pub(crate) fn slot(&self, class: &str, offset: u64, index: u64) -> bool {
        !self.slots.is_empty() && self.slots.contains(&(class.to_owned(), u32::try_from(offset).unwrap(), u32::try_from(index).unwrap()))
    }

    /// Accessor counts by the group `group` gives each item's class.
    pub(crate) fn counts(&self, group: impl Fn(&str) -> String) -> BTreeMap<String, AccessorCount> {
        let mut out: BTreeMap<String, AccessorCount> = BTreeMap::new();
        for (class, _) in &self.members {
            out.entry(group(class)).or_default().fields += 1;
        }
        for class in &self.sizes {
            out.entry(group(class)).or_default().sizes += 1;
        }
        for (class, _) in &self.bases {
            out.entry(group(class)).or_default().bases += 1;
        }
        for (class, ..) in &self.slots {
            out.entry(group(class)).or_default().slots += 1;
        }
        out
    }
}

#[derive(Default, Clone, Copy)]
pub struct AccessorCount {
    pub fields: usize,
    pub slots: usize,
    pub bases: usize,
    pub sizes: usize,
}

/// The profile row of one item; nothing for `Same`, and `Missing` is `missing` where the format has it.
fn diff_row<K>(key: K, diff: Diff, here: fn(K, u32) -> Written, missing: fn(K) -> Written, unverified: fn(K) -> Written) -> Option<Written> {
    match diff {
        Diff::Same => None,
        Diff::Value(value) => Some(here(key, value)),
        Diff::Missing => Some(missing(key)),
        Diff::Unverified => Some(unverified(key)),
    }
}

/// One runtime's profile. A ledger item with no row in the file is unverified, never identical.
fn profile(runtime: &Runtime, inp: &Inputs, sites: &BTreeSet<(u32, u32)>) -> Profile {
    let numbers: Vec<u32> = runtime.name.split('.').map(|n| n.parse().unwrap()).collect();
    let mut rows = Vec::new();
    {
        for (&ae, state) in &runtime.ids {
            rows.push(match state {
                IdState::Mapped(se) => Written::Id { ae, se: *se },
                IdState::Missing => Written::IdMissing(ae),
                IdState::Unverified => Written::IdUnverified(ae),
                IdState::Address(here) => Written::IdAddress { ae, here: *here },
            });
        }
        if runtime.database != "se" {
            let ids: BTreeSet<u32> = inp.symbols.iter().filter(|s| matches!(s.kind.as_str(), "function" | "global" | "vtable")).filter_map(|s| u32::try_from(s.id?).ok()).filter(|id| !runtime.seen.contains(id) && !runtime.ids.contains_key(id)).collect();
            rows.extend(ids.into_iter().map(Written::IdUnverified));
        }
        let members: BTreeSet<(&str, &str)> = ledger_members(inp).collect();
        rows.extend(members.into_iter().filter_map(|(class, member)| {
            let diff = runtime.members.get(&(class.to_owned(), member.to_owned())).copied().unwrap_or(Diff::Unverified);
            diff_row(format!("{class}.{member}"), diff, |member, offset| Written::Member { member, offset }, Written::MemberMissing, Written::MemberUnverified)
        }));
        let slots: BTreeSet<(&str, u32, u32)> = ledger_slots(inp).chain(runtime.slots.keys().map(|(class, offset, index)| (class.as_str(), *offset, *index))).collect();
        rows.extend(slots.into_iter().filter_map(|(class, vtable_offset, index)| {
            let diff = runtime.slots.get(&(class.to_owned(), vtable_offset, index)).copied().unwrap_or(Diff::Unverified);
            diff_row(Slot { class: class.to_owned(), vtable_offset, index }, diff, |slot, here| Written::Slot { slot, here }, Written::SlotMissing, Written::SlotUnverified)
        }));
        let classes: BTreeSet<&str> = ledger_classes(inp).collect();
        rows.extend(classes.into_iter().filter_map(|class| {
            let diff = runtime.sizes.get(class).copied().unwrap_or(Diff::Unverified);
            diff_row(class.to_owned(), diff, |class, size| Written::Size { class, size }, Written::SizeMissing, Written::SizeUnverified)
        }));
        let bases: BTreeSet<(&str, &str)> = ledger_bases(inp).collect();
        rows.extend(bases.into_iter().filter_map(|(class, base)| {
            let diff = runtime.bases.get(&(class.to_owned(), base.to_owned())).copied().unwrap_or(Diff::Unverified);
            diff_row(format!("{class}:{base}"), diff, |class_and_base, offset| Written::Base { class_and_base, offset }, Written::BaseMissing, Written::BaseUnverified)
        }));
        rows.extend(sites.iter().map(|&(ae, canonical)| match runtime.sites.get(&(ae, canonical)).unwrap_or(&Site::Unverified) {
            Site::Here { offset, bytes, replacement, .. } => Written::Site { ae, canonical, here: *offset, expected: bytes.clone(), replacement: replacement.clone() },
            Site::Missing => Written::SiteMissing { ae, canonical },
            Site::Unverified => Written::SiteUnverified { ae, canonical },
        }));
        rows.extend(runtime.enums.iter().map(|((name, value), diff)| match diff {
            Diff::Value(here) => Written::Enum { value: format!("{name}.{value}"), here: *here },
            _ => Written::EnumMissing(format!("{name}.{value}")),
        }));
    }
    Profile { runtime: [numbers[0], numbers[1], numbers[2]], address_library: runtime.address_library.clone(), se_database: runtime.database == "se", rows }
}

/// The profile files keyed by path relative to the workspace root: one per file read, and the
/// canonical one always, which is the header alone by construction.
#[must_use]
pub fn profiles(inp: &Inputs, runtimes: &[Runtime]) -> BTreeMap<String, String> {
    let sites: BTreeSet<(u32, u32)> = runtimes.iter().flat_map(|r| r.sites.keys().copied()).collect();
    let mut made: Vec<Profile> = runtimes.iter().filter(|r| r.name != CANONICAL).map(|r| profile(r, inp, &sites)).collect();
    made.push(Profile::canonical());
    made.iter().map(|p| (format!("{PROFILE_DIR}/{}", p.file_name()), p.text())).collect()
}

/// Every member, base, size and slot the ledger names, for the name checks of `sre::member_offset!`
/// and its kin: one `key<tab>value` line per item, sorted by key. A virtual base has no fixed
/// displacement to offer and is left out.
#[must_use]
pub fn ledger_tables(inp: &Inputs, runtimes: &[Runtime]) -> String {
    let mut members: BTreeMap<String, u64> = BTreeMap::new();
    let layouts = inp.layouts.iter().flat_map(|(c, l)| l.fields.iter().map(move |f| (c, f)));
    let structs = inp.structs.structs.iter().flat_map(|(c, s)| s.fields.iter().map(move |f| (c, f)));
    for (class, field) in layouts.chain(structs) {
        members.entry(format!("{class}.{}", field.name)).or_insert(field.off);
    }
    let mut bases: BTreeMap<String, u64> = BTreeMap::new();
    for class in &inp.rtti.classes {
        for base in class.bases.iter().skip(1).filter(|b| b.pdisp == -1) {
            if let Ok(offset) = u64::try_from(base.mdisp) {
                bases.entry(format!("{}:{}", class.name, base.name)).or_insert(offset);
            }
        }
    }
    let sizes: BTreeSet<&str> = ledger_classes(inp).collect();
    let mut slots: BTreeMap<&str, BTreeMap<u32, u32>> = BTreeMap::new();
    let compared = runtimes.iter().flat_map(|r| r.slots.keys().map(|(class, offset, index)| (class.as_str(), *offset, *index)));
    for (class, offset, index) in ledger_slots(inp).chain(compared) {
        let count = slots.entry(class).or_default().entry(offset).or_default();
        *count = (*count).max(index + 1);
    }
    let table = |name: &str, lines: Vec<String>| format!("pub const {name}: &str = r\"{}\n\";\n", lines.join("\n"));
    let mut out = String::from("//! What the ledger names, for `sre::ledger`. Generated by `cargo xtask gen`; do not edit.\n\n");
    out += &table("MEMBERS", members.iter().map(|(key, offset)| format!("{key}\t{offset:x}")).collect());
    out += &table("BASES", bases.iter().map(|(key, offset)| format!("{key}\t{offset:x}")).collect());
    out += &table("SIZES", sizes.iter().map(|key| format!("{key}\t")).collect());
    out += &table(
        "SLOTS",
        slots.iter().map(|(class, tables)| format!("{class}\t{}", tables.iter().map(|(offset, count)| format!("{offset:x}:{count:x}")).collect::<Vec<_>>().join(","))).collect(),
    );
    out
}

#[cfg(test)]
mod tests;
