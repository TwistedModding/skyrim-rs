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

struct Table {
    /// `(rva, index into FUNCTIONS)` sorted by rva.
    by_rva: Box<[(u32, u32)]>,
    mismatches: u32,
    /// The ledger's rvas describe this game, so its vtables and globals can be named by them.
    canonical: bool,
}

/// `None` or unset: the function names come from the ledger's rvas.
static TABLE: OnceLock<Option<Table>> = OnceLock::new();
static MODE: OnceLock<Mode> = OnceLock::new();

/// Builds the table for the running game: each ledger function at the offset the Address Library
/// gives its id through the profile. Where the file lacks the id, 1.7.104 falls back to the
/// ledger's rva and any other runtime leaves the function out. Returns the mode and how many
/// offsets disagreed with the ledger.
pub fn init() -> (Mode, u32) {
    let mode = *MODE.get_or_init(|| {
        let canonical = skse::runtime::running().is_some_and(skse::runtime::is_canonical);
        let Ok(running) = versionlib::running() else {
            if canonical {
                let _ = TABLE.set(None);
                return Mode::Ledger;
            }
            forget_ledger_addresses();
            return Mode::None;
        };
        let mut mismatches = 0;
        let mut by_rva: Vec<(u32, u32)> = ledger::FUNCTIONS
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let here = running.offset(u64::from(f.id)).and_then(|o| u32::try_from(o).ok());
                let rva = here.or(canonical.then_some(f.rva))?;
                if rva != f.rva {
                    mismatches += 1;
                }
                Some((rva, u32::try_from(i).unwrap_or(u32::MAX)))
            })
            .collect();
        by_rva.sort_unstable();
        let _ = TABLE.set(Some(Table { by_rva: by_rva.into_boxed_slice(), mismatches, canonical }));
        Mode::AddressLibrary
    });
    (mode, TABLE.get().and_then(Option::as_ref).map_or(0, |t| t.mismatches))
}

/// Names nothing from here on unless the table is already built. For a game the ledger's rvas do
/// not describe, with no Address Library to place them: a live one, or a dump decoded offline.
pub(crate) fn forget_ledger_addresses() {
    let _ = TABLE.set(Some(Table { by_rva: Box::default(), mismatches: 0, canonical: false }));
}

/// The function holding a module-relative address.
#[must_use]
pub fn code(rva: u32) -> Option<Code> {
    let (function, offset) = match TABLE.get().and_then(Option::as_ref) {
        Some(table) => {
            let i = table.by_rva.partition_point(|e| e.0 <= rva).checked_sub(1)?;
            let (start, index) = table.by_rva[i];
            (ledger::FUNCTIONS.get(index as usize)?, rva - start)
        }
        None => ledger::function_at(rva)?,
    };
    Some(Code { function, offset, meaning: ledger::meaning_of(function.rva) })
}

fn ledger_data_holds() -> bool {
    TABLE.get().and_then(Option::as_ref).is_none_or(|t| t.canonical)
}

/// The vtable starting at a module-relative address.
#[must_use]
pub fn vtable(rva: u32) -> Option<&'static Vtable> {
    ledger_data_holds().then(|| vtables::find(rva)).flatten()
}

/// The vtable or global holding a module-relative data address.
#[must_use]
pub fn data(rva: u32) -> Option<Data> {
    if !ledger_data_holds() {
        return None;
    }
    let all = &vtables::ALL;
    if let Some(i) = all.floor(rva, |v| v.rva) {
        let vtable = all.get(i)?;
        let end = all.get(i + 1).map_or(vtable.rva + 0x400, |next| next.rva);
        if rva < end {
            return Some(Data::Vtable { vtable, offset: rva - vtable.rva });
        }
    }
    let (global, offset) = ledger::global_at(rva)?;
    (offset < 0x1000).then_some(Data::Global { global, offset })
}

/// The module-relative address of the function with an Address Library id, as the table places it.
#[must_use]
pub fn function_rva(id: u64) -> Option<u32> {
    let id = u32::try_from(id).ok()?;
    match TABLE.get().and_then(Option::as_ref) {
        Some(table) => table.by_rva.iter().find(|e| ledger::FUNCTIONS.get(e.1 as usize).is_some_and(|f| f.id == id)).map(|e| e.0),
        None => ledger::FUNCTIONS.iter().find(|f| f.id == id).map(|f| f.rva),
    }
}

/// How many functions the table names.
#[must_use]
pub fn count() -> usize {
    TABLE.get().and_then(Option::as_ref).map_or(ledger::FUNCTIONS.len(), |t| t.by_rva.len())
}
