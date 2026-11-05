//! Detours through `MinHook`, rel32 call-site patches and Address Library relocation against the running module.

use core::ffi::c_void;
use std::fmt;
use std::io;

use minhook::{MH_STATUS, MinHook};

mod site;

pub use site::{CallSite, Trampoline, alloc_near, call_length, cave, find_call, restore_call, retarget, write_branch, write_call, write_code, write_pointer};

#[derive(Debug)]
pub enum Error {
    MinHook(MH_STATUS),
    VersionLib(versionlib::Error),
    Io(io::Error),
    UnknownId(u64),
    UnknownSite(u64, usize),
    NotRel32(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MinHook(s) => write!(f, "minhook: {s:?}"),
            Self::VersionLib(e) => e.fmt(f),
            Self::Io(e) => write!(f, "hook: {e}"),
            Self::UnknownId(id) => write!(f, "no usable id {id} on this runtime"),
            Self::UnknownSite(id, offset) => write!(f, "no site {id}+{offset:#x} on this runtime"),
            Self::NotRel32(site) => write!(f, "no call rel32, jump rel32 or virtual call on the first argument at {site:#x}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<MH_STATUS> for Error {
    fn from(s: MH_STATUS) -> Self {
        Self::MinHook(s)
    }
}

impl From<versionlib::Error> for Error {
    fn from(e: versionlib::Error) -> Self {
        Self::VersionLib(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// An installed detour. `F` is the `extern "C" fn` type shared by target, detour and trampoline.
pub struct Hook<F: Copy> {
    target: usize,
    trampoline: F,
}

unsafe impl<F: Copy> Send for Hook<F> {}
unsafe impl<F: Copy> Sync for Hook<F> {}

impl<F: Copy> Hook<F> {
    /// Creates and enables a detour on the function at `target`. The detour is live before this
    /// returns, so a detour that needs the trampoline from a shared place must use [`Hook::create`]
    /// and [`Hook::enable`] around storing it.
    ///
    /// # Safety
    /// `target` must be the entry of a function whose signature is `F`, and `detour`
    /// must be callable in its place. `F` must be a function pointer type.
    pub unsafe fn install(target: usize, detour: F) -> Result<Self, Error> {
        let hook = unsafe { Self::create(target, detour) }?;
        hook.enable()?;
        Ok(hook)
    }

    /// Creates the detour without enabling it; the target keeps running the original until
    /// [`Hook::enable`].
    ///
    /// # Safety
    /// Same as [`Hook::install`].
    pub unsafe fn create(target: usize, detour: F) -> Result<Self, Error> {
        const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
        let detour: *mut c_void = unsafe { core::mem::transmute_copy(&detour) };
        let trampoline = unsafe { MinHook::create_hook(target as *mut c_void, detour) }?;
        Ok(Self { target, trampoline: unsafe { core::mem::transmute_copy(&trampoline) } })
    }

    /// Enables a created detour; on failure the hook is removed and cannot be enabled again.
    pub fn enable(&self) -> Result<(), Error> {
        let target = self.target as *mut c_void;
        if let Err(e) = unsafe { MinHook::enable_hook(target) } {
            let _ = unsafe { MinHook::remove_hook(target) };
            return Err(e.into());
        }
        Ok(())
    }

    /// The original function, callable from the detour.
    pub fn original(&self) -> F {
        self.trampoline
    }

    #[must_use]
    pub fn target(&self) -> usize {
        self.target
    }

    /// Disables and removes the detour. Code already executing the trampoline keeps running.
    ///
    /// # Safety
    /// No other thread may be inside the detour afterwards expecting the trampoline to stay valid.
    pub unsafe fn remove(self) -> Result<(), Error> {
        let target = self.target as *mut c_void;
        unsafe { MinHook::disable_hook(target) }?;
        unsafe { MinHook::remove_hook(target) }?;
        Ok(())
    }
}

pub use versionlib::module_base;

/// Address Library ids resolved against the running game, through the process's one
/// [`versionlib::running`] database.
pub struct Reloc {
    running: &'static versionlib::Running,
}

impl Reloc {
    /// Opens the Address Library the running game's profile names. The game is identified by its
    /// executable, not by what the loader reports.
    pub fn open() -> Result<Self, Error> {
        Ok(Self { running: versionlib::running()? })
    }

    #[must_use]
    pub fn base(&self) -> usize {
        self.running.base()
    }

    #[must_use]
    pub fn running(&self) -> &'static versionlib::Running {
        self.running
    }

    /// The live address of an AE id on the running game.
    pub fn address(&self, id: u64) -> Result<usize, Error> {
        self.running.address(id).ok_or(Error::UnknownId(id))
    }

    /// The live address of the site `offset` bytes into the function at an AE id on 1.7.104,
    /// wherever the running game's profile places it.
    pub fn site(&self, id: u64, offset: usize) -> Result<usize, Error> {
        u32::try_from(offset).ok().and_then(|o| self.running.site(id, o)).ok_or(Error::UnknownSite(id, offset))
    }

    /// Installs `detour` on the function the Address Library knows as `id`.
    ///
    /// # Safety
    /// Same as [`Hook::install`].
    pub unsafe fn hook<F: Copy>(&self, id: u64, detour: F) -> Result<Hook<F>, Error> {
        unsafe { Hook::install(self.address(id)?, detour) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn add(a: u32, b: u32) -> u32 {
        a.wrapping_add(b)
    }

    static ORIGINAL: std::sync::OnceLock<Hook<extern "C" fn(u32, u32) -> u32>> = std::sync::OnceLock::new();

    extern "C" fn add_detour(a: u32, b: u32) -> u32 {
        ORIGINAL.get().unwrap().original()(a, b) + 100
    }

    #[test]
    fn detour_round_trips_through_trampoline() {
        let target: extern "C" fn(u32, u32) -> u32 = add;
        let hook = unsafe { Hook::install(target as usize, add_detour as extern "C" fn(u32, u32) -> u32) }.unwrap();
        assert!(ORIGINAL.set(hook).is_ok());
        assert_eq!(core::hint::black_box(target)(2, 3), 105);
        assert_eq!(ORIGINAL.get().unwrap().original()(2, 3), 5);
        assert_ne!(module_base(), 0);
    }

    #[test]
    fn a_game_with_no_profile_is_an_error_not_a_panic() {
        let Err(error) = Reloc::open() else { panic!("a test binary is no game") };
        assert!(error.to_string().contains("file version"), "{error}");
    }
}
