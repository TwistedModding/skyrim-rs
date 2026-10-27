//! Game addresses to the ledger's names: the functions and globals `skre` names and the vtables
//! the RTTI records, each found by the module-relative address.

use std::sync::OnceLock;

use skre::raw::{symbols as ledger, vtables};
use skre::raw::{Function, Global, Vtable};

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
    /// The game is not the runtime the ledger describes: no names.
    None = 0,
    /// The ledger's own rvas (the offline decoder, or no Address Library file).
    Ledger = 1,
    /// The Address Library's offset for every function id, read at startup.
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
}

static TABLE: OnceLock<Option<Table>> = OnceLock::new();
static MODE: OnceLock<Mode> = OnceLock::new();

/// Builds the table for the running game: each ledger function at the offset the Address Library
/// gives its id (the ledger's rva where the file lacks the id). Returns the mode and how many
/// offsets disagreed with the ledger.
pub fn init(runtime: u32) -> (Mode, u32) {
    let mode = *MODE.get_or_init(|| {
        if skse::runtime::unpack(runtime)[..3] != skre::VERSION {
            return Mode::None;
        }
        let Ok(db) = versionlib::running(&skse::runtime::versionlib_name(runtime)).map(versionlib::Running::db) else {
            let _ = TABLE.set(None);
            return Mode::Ledger;
        };
        let mut mismatches = 0;
        let mut by_rva: Vec<(u32, u32)> = ledger::FUNCTIONS
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let rva = db.offset(u64::from(f.id)).and_then(|o| u32::try_from(o).ok()).unwrap_or(f.rva);
                if rva != f.rva {
                    mismatches += 1;
                }
                (rva, u32::try_from(i).unwrap_or(u32::MAX))
            })
            .collect();
        by_rva.sort_unstable();
        let _ = TABLE.set(Some(Table { by_rva: by_rva.into_boxed_slice(), mismatches }));
        Mode::AddressLibrary
    });
    (mode, TABLE.get().and_then(Option::as_ref).map_or(0, |t| t.mismatches))
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

/// The vtable or global holding a module-relative data address.
#[must_use]
pub fn data(rva: u32) -> Option<Data> {
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
    ledger::FUNCTIONS.len()
}
