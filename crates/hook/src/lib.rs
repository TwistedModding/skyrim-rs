//! Detours through `MinHook` and Address Library relocation against the running module.

use core::ffi::c_void;
use std::fmt;
use std::io;

use minhook::{MH_STATUS, MinHook};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

#[derive(Debug)]
pub enum Error {
    MinHook(MH_STATUS),
    VersionLib(versionlib::Error),
    Io(io::Error),
    UnknownId(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MinHook(s) => write!(f, "minhook: {s:?}"),
            Self::VersionLib(e) => e.fmt(f),
            Self::Io(e) => write!(f, "hook: {e}"),
            Self::UnknownId(id) => write!(f, "address library has no id {id}"),
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
    /// Creates and enables a detour on the function at `target`.
    ///
    /// # Safety
    /// `target` must be the entry of a function whose signature is `F`, and `detour`
    /// must be callable in its place. `F` must be a function pointer type.
    pub unsafe fn install(target: usize, detour: F) -> Result<Self, Error> {
        const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
        let detour: *mut c_void = unsafe { core::mem::transmute_copy(&detour) };
        let target_ptr = target as *mut c_void;
        let trampoline = unsafe { MinHook::create_hook(target_ptr, detour) }?;
        if let Err(e) = unsafe { MinHook::enable_hook(target_ptr) } {
            let _ = unsafe { MinHook::remove_hook(target_ptr) };
            return Err(e.into());
        }
        Ok(Self { target, trampoline: unsafe { core::mem::transmute_copy(&trampoline) } })
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

/// Base address of the game executable.
#[must_use]
pub fn module_base() -> usize {
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}

/// Address Library ids resolved against the running game.
pub struct Reloc {
    db: versionlib::VersionDb,
    base: usize,
}

impl Reloc {
    /// Opens `<exe dir>\Data\SKSE\Plugins\<versionlib file>` for the packed runtime version SKSE reports.
    pub fn open(packed_runtime: u32) -> Result<Self, Error> {
        let exe = std::env::current_exe()?;
        let dir = exe.parent().ok_or_else(|| io::Error::other("executable has no parent directory"))?;
        let path = dir.join("Data").join("SKSE").join("Plugins").join(skse::runtime::versionlib_name(packed_runtime));
        Ok(Self { db: versionlib::VersionDb::open(&path)?, base: module_base() })
    }

    #[must_use]
    pub fn base(&self) -> usize {
        self.base
    }

    #[must_use]
    pub fn db(&self) -> &versionlib::VersionDb {
        &self.db
    }

    pub fn address(&self, id: u64) -> Result<usize, Error> {
        let offset = self.db.offset(id).ok_or(Error::UnknownId(id))?;
        Ok(self.base + usize::try_from(offset).map_err(|_| Error::UnknownId(id))?)
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
}
