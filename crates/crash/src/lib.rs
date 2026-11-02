//! The framework's crash logger: a vectored exception handler and the unhandled-exception
//! filter, a watchdog thread with its own stack and buffers set aside at install, a minidump
//! written first and then a text log in the SKSE log folder (`skyrim-runtime-extender-crash-<time>.log`), with
//! the game's functions named from the ledger. One copy per process reports: the host installs
//! it, and a plain SKSE mod that calls [`install`] next to the host is told the host already did.
//!
//! The report is formatted by `report::write` from a `record::Record` the watchdog fills and the
//! live process's memory; the same function decodes a minidump offline, where the record comes
//! back as the dump's user stream.

use core::fmt;

pub mod dump;
pub mod mem;
pub mod papyrus;
pub mod pdb;
pub mod probe;
pub mod record;
pub mod report;
pub mod rules;
pub mod symbols;
pub mod text;

mod handler;
mod images;
mod live;

pub use text::Writer;

/// Which part of the log a facts callback is filling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The "Loader" block of the header: who hosts the game, the plugins, the last message.
    Context,
    /// The "Framework state" block at the end: what the crashing thread was inside, broken sites, the last log lines.
    State,
}

/// What the watchdog knows when it asks the host for facts.
pub struct Crash {
    pub thread: u32,
    pub main_thread: u32,
    pub exception: u32,
}

/// Fills a section with the host's own facts, on the watchdog thread at crash time: no heap, no
/// blocking locks (`try_lock` and give up), no engine calls.
pub type FactsFn = fn(&mut Writer<'_>, &Crash, Section);

pub struct Options {
    /// Who installs: the host's or the mod's name, shown in the log's header.
    pub name: &'static str,
    /// The packed runtime version the loader reported, for the Address Library and the ledger gate.
    pub runtime_version: u32,
    /// A build string for the header.
    pub build: &'static str,
    pub facts: Option<FactsFn>,
    /// Folders searched for the PDBs of DLLs, `;`-separated, besides each DLL's own folder.
    pub symbol_path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installed {
    /// This copy owns the handlers.
    Here { watchdog: u32, symbols: symbols::Mode, symbol_mismatches: u32 },
    /// Another copy in the process (the host's) already reports; nothing was installed.
    Elsewhere,
}

#[derive(Debug)]
pub enum Error {
    Event(u32),
    Thread(u32),
    Handler,
    LogDir(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Event(e) => write!(f, "CreateEvent failed with error {e}"),
            Self::Thread(e) => write!(f, "the watchdog thread could not be created, error {e}"),
            Self::Handler => f.write_str("AddVectoredExceptionHandler failed"),
            Self::LogDir(e) => write!(f, "no log folder: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Installs the crash logger for the process from the calling thread, which is taken to be the
/// game's main thread (an SKSE `Load` runs there). Call it once, early in `Load`.
pub fn install(options: &Options) -> Result<Installed, Error> {
    let dir = skse::log::dir(options.runtime_version).map_err(Error::LogDir)?;
    let _ = std::fs::create_dir_all(&dir);
    let game = game_addresses(options.runtime_version);
    handler::install(options, &dir.to_string_lossy(), game)
}

/// The globals the game-state probes read, through the Address Library when it opens.
fn game_addresses(runtime: u32) -> live::GameAddresses {
    use sre::raw::globals;
    let mut out = live::GameAddresses::default();
    if skse::runtime::unpack(runtime)[..3] != sre::VERSION {
        return out;
    }
    let Ok(db) = versionlib::running(&skse::runtime::versionlib_name(runtime)) else { return out };
    let resolve = |id: u64| db.address(id).map_or(0, |a| a as u64);
    out.frame_counter = resolve(globals::g_RenderFrameCounter);
    out.player = resolve(globals::g_PlayerCharacter_ptr);
    out.save_manager = resolve(globals::g_BGSSaveLoadManager_ptr);
    out
}

/// Tells the logger where a framework plugin's generation is mapped (`live`), or that a reload
/// unloaded it (`!live`), so a crash inside the old image is named as such.
pub fn note_image(name: &str, generation: u32, base: usize, live: bool) {
    images::note(name, generation, base, live);
}
