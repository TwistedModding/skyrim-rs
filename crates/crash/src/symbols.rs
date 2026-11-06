// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Game addresses to the ledger's names: the functions and globals `sre` names and the vtables
//! the RTTI records, each found by the module-relative address.

use std::sync::OnceLock;

use sre::raw::{symbols as ledger, vtables};
use sre::raw::{Function, Global, Vtable};

/// A code address inside the game: the function holding it and the offset into it.
pub struct Code {
    pub function: &'static Function,
    pub offset: u32,
    pub meaning: Option<&'static str>,
}

/// A data address inside the game.
pub enum Data {
    Vtable { vtable: &'static Vtable, offset: u32 },
    Global { global: &'static Global, offset: u32 },
}

/// Where the function table's addresses come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    /// Not the runtime the ledger describes, and its profile or Address Library is unusable: no names.
    None = 0,
    /// The ledger's own rvas, on the runtime they describe (the offline decoder, or no Address Library file).
    Ledger = 1,
    /// The Address Library's offset for every function id it holds, read at startup.
    AddressLibrary = 2,
}

impl Mode {
    #[must_use]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Ledger,
            2 => Self::AddressLibrary,
            _ => Self::None,
        }
    }
}

/// Each ledger row at its module-relative address on this game, as `(rva, row index)` sorted by rva.
struct Table {
    functions: Box<[(u32, u32)]>,
    vtables: Box<[(u32, u32)]>,
    globals: Box<[(u32, u32)]>,
    mismatches: u32,
}

/// Unset until [`init`]: a dump decoded offline starts from the ledger's rvas.
static TABLE: OnceLock<Table> = OnceLock::new();
static MODE: OnceLock<Mode> = OnceLock::new();

fn table() -> &'static Table {
    TABLE.get_or_init(|| Table { functions: ledger::FUNCTIONS.placed(|f| Some(f.rva)), vtables: vtables::ALL.placed(|v| Some(v.rva)), globals: ledger::GLOBALS.placed(|g| Some(g.rva)), mismatches: 0 })
}

/// Builds the table for the running game: each ledger row at the offset the Address Library gives
/// its id through the profile. Where the file lacks the id, 1.7.104 falls back to the ledger's rva
/// and any other runtime leaves the row out. Returns the mode and how many function offsets
/// disagreed with the ledger.
pub fn init() -> (Mode, u32) {
    let mode = *MODE.get_or_init(|| {
        let canonical = skse::runtime::running().is_some_and(skse::runtime::is_canonical);
        let Ok(running) = versionlib::running() else {
            if canonical {
                table();
                return Mode::Ledger;
            }
            forget_ledger_addresses();
            return Mode::None;
        };
        let here = |id: u64, rva: u32| running.offset(id).and_then(|o| u32::try_from(o).ok()).or(canonical.then_some(rva));
        let mut mismatches = 0;
        let functions = ledger::FUNCTIONS.placed(|f| {
            let rva = here(u64::from(f.id), f.rva)?;
            mismatches += u32::from(rva != f.rva);
            Some(rva)
        });
        let _ = TABLE.set(Table { functions, vtables: vtables::ALL.placed(|v| here(v.id, v.rva)), globals: ledger::GLOBALS.placed(|g| here(u64::from(g.id), g.rva)), mismatches });
        Mode::AddressLibrary
    });
    (mode, table().mismatches)
}

/// Names nothing from here on unless the table is already built. For a game the ledger's rvas do
/// not describe, with no Address Library to place them: a live one, or a dump decoded offline.
pub(crate) fn forget_ledger_addresses() {
    let _ = TABLE.set(Table { functions: Box::default(), vtables: Box::default(), globals: Box::default(), mismatches: 0 });
}

/// The position of the entry at or before `rva`.
fn floor(placed: &[(u32, u32)], rva: u32) -> Option<usize> {
    placed.partition_point(|e| e.0 <= rva).checked_sub(1)
}

/// The function holding a module-relative address.
#[must_use]
pub fn code(rva: u32) -> Option<Code> {
    let functions = &table().functions;
    let (start, index) = functions[floor(functions, rva)?];
    let function = ledger::FUNCTIONS.get(index as usize)?;
    Some(Code { function, offset: rva - start, meaning: ledger::meaning_of(function.rva) })
}

/// The vtable starting at a module-relative address.
#[must_use]
pub fn vtable(rva: u32) -> Option<&'static Vtable> {
    let placed = &table().vtables;
    vtables::ALL.get(placed[placed.binary_search_by_key(&rva, |e| e.0).ok()?].1 as usize)
}

/// The vtable or global holding a module-relative data address.
#[must_use]
pub fn data(rva: u32) -> Option<Data> {
    let table = table();
    if let Some(at) = floor(&table.vtables, rva) {
        let (start, index) = table.vtables[at];
        let end = table.vtables.get(at + 1).map_or(start + 0x400, |next| next.0);
        if rva < end {
            return Some(Data::Vtable { vtable: vtables::ALL.get(index as usize)?, offset: rva - start });
        }
    }
    let (start, index) = table.globals[floor(&table.globals, rva)?];
    (rva - start < 0x1000).then_some(Data::Global { global: ledger::GLOBALS.get(index as usize)?, offset: rva - start })
}

/// The module-relative address of the function with an Address Library id, as the table places it.
#[must_use]
pub fn function_rva(id: u64) -> Option<u32> {
    let id = u32::try_from(id).ok()?;
    table().functions.iter().find(|e| ledger::FUNCTIONS.get(e.1 as usize).is_some_and(|f| f.id == id)).map(|e| e.0)
}

/// How many functions the table names.
#[must_use]
pub fn count() -> usize {
    table().functions.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_finds_the_entry_at_or_before() {
        let placed = [(0x10, 0), (0x20, 1)];
        assert_eq!(floor(&placed, 0xF), None);
        assert_eq!(floor(&placed, 0x10), Some(0));
        assert_eq!(floor(&placed, 0x1F), Some(0));
        assert_eq!(floor(&placed, 0x20), Some(1));
    }

    /// The ids resolve vtables and globals the way their ledger rvas did on 1.7.104.
    #[test]
    fn ids_place_the_ledgers_rows_on_the_canonical_file() {
        let Some(dir) = std::env::var_os("VERSIONLIB_DIR") else {
            eprintln!("VERSIONLIB_DIR not set: skipped");
            return;
        };
        let db = versionlib::VersionDb::open(&std::path::Path::new(&dir).join("versionlib-1-7-104-0.bin")).unwrap();
        let differs = |id: u64, rva: u32| db.offset(id) != Some(u64::from(rva));
        let vtables = vtables::ALL.iter().filter(|v| differs(v.id, v.rva)).count();
        let globals = ledger::GLOBALS.iter().filter(|g| differs(u64::from(g.id), g.rva)).count();
        eprintln!("{vtables} of {} vtables and {globals} of {} globals differ", vtables::ALL.len(), ledger::GLOBALS.len());
        assert_eq!((vtables, globals), (0, 0));
    }
}
