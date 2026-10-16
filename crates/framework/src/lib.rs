//! The framework's own plugin ABI: plugins the host (`skyrim_rs.dll`, an SKSE plugin) loads itself.
//!
//! A framework plugin is a `cdylib` in `Data\SKSE\Plugins\skyrim-rs\` (SKSE loads only the folder
//! above, so it never sees them) that declares itself with [`plugin!`]. The host loads every DLL
//! there from its own `SKSEPlugin_Load`, in file name order, reads the exported `SkyrimRs_Plugin`
//! [`Descriptor`] as plain C data and calls the plugin's `load` only when the descriptor's
//! [`ABI`] and [`BUILD`] equal its own: that call and everything passed through [`Host`] use the
//! Rust ABI, which holds only for one compiler and one build of this crate and its dependencies,
//! so a plugin is built against the same framework release (its `Cargo.lock`) with the same
//! `rustc`. A refused DLL is unloaded and named in `skyrim-rs.log`.
//!
//! What a plugin gets from the [`Host`]: the SKSE interface the host was loaded with, forwarded
//! (`QueryInterface` answers as for the host, and the plugin handle is the host's, valid only while
//! `load` runs), the loader's messages and the Papyrus registration callback, both fanned out by
//! the host to every plugin that asks, and windows on the host's overlay. A plugin calls
//! `skyrim::init` with [`Host::skse`] before using the game layer and never installs the overlay
//! itself.

use core::ffi::{CStr, c_char};

pub use overlay::egui;
use skyrim::papyrus::VirtualMachine;

pub mod host;

/// The descriptor's layout version.
pub const ABI: u32 = 1;

/// This crate's version and the compiler it was built with; a plugin is loaded only when its copy
/// equals the host's.
pub const BUILD: &CStr = match CStr::from_bytes_with_nul(concat!("framework ", env!("CARGO_PKG_VERSION"), ", ", env!("SKYRIM_RS_RUSTC"), "\0").as_bytes()) {
    Ok(build) => build,
    Err(_) => panic!("BUILD holds one nul, at the end"),
};

pub type LoadFn = fn(&'static Host) -> bool;
/// Receives every message the loader sends as sender "SKSE" (`skse::MessagingInterface` kinds).
pub type MessageFn = fn(&skse::Message);
/// Receives the VM at the loader's Papyrus registration pass.
pub type PapyrusFn = fn(&'static VirtualMachine);
pub type Window = Box<dyn FnMut(&egui::Context) + Send>;

/// What [`plugin!`] exports as `SkyrimRs_Plugin`. The host reads `abi` and `build` before anything
/// else, so they stay first and C-typed in every future ABI.
#[repr(C)]
pub struct Descriptor {
    pub abi: u32,
    pub build: *const c_char,
    pub name: *const c_char,
    pub load: LoadFn,
}

unsafe impl Sync for Descriptor {}

/// The host as a plugin's `load` receives it, for the life of the game.
pub struct Host {
    skse: &'static skse::Interface,
    on_message: fn(MessageFn),
    on_papyrus: fn(PapyrusFn),
    add_window: fn(Window),
}

impl Host {
    /// The SKSE interface the host was loaded with.
    #[must_use]
    pub fn skse(&self) -> &'static skse::Interface {
        self.skse
    }

    /// Calls `f` with every loader message from now on, after the host's own handling.
    pub fn on_message(&self, f: MessageFn) {
        (self.on_message)(f);
    }

    /// Calls `f` with the VM at the Papyrus registration pass; register from `load`, which runs before it.
    pub fn on_papyrus(&self, f: PapyrusFn) {
        (self.on_papyrus)(f);
    }

    /// Adds a window to the host's overlay; `draw` runs on the main thread every frame the overlay is open.
    pub fn add_window(&self, draw: impl FnMut(&egui::Context) + Send + 'static) {
        (self.add_window)(Box::new(draw));
    }
}

/// Declares a framework plugin: exports its [`Descriptor`].
///
/// ```ignore
/// framework::plugin! {
///     name: "my-mod",
///     load: load,
/// }
/// fn load(host: &'static framework::Host) -> bool { skyrim::init(host.skse()).is_ok() }
/// ```
#[macro_export]
macro_rules! plugin {
    (name: $name:literal, load: $load:path $(,)?) => {
        #[unsafe(no_mangle)]
        #[allow(non_upper_case_globals)]
        pub static SkyrimRs_Plugin: $crate::Descriptor =
            $crate::Descriptor { abi: $crate::ABI, build: $crate::BUILD.as_ptr(), name: concat!($name, "\0").as_ptr().cast(), load: $load };
    };
}
