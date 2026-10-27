//! The safe layer over `sre`: what the generated methods cannot give a mod without `unsafe`.
//!
//! The game itself is `sre`: `sre::TESForm`, `sre::PlayerCharacter` and every other class with
//! its engine functions as methods. This crate adds the objects that own or guard something (the
//! interned [`BSFixedString`], reference and script object handles that release, the engine's spin
//! lock), the event sinks, Papyrus natives and dispatch, a mod's own menus and console commands, the
//! hook installers, and per class the few reads that would otherwise need `unsafe` (a C string by
//! `&str`, an engine array as a slice, a checked cast), as extension traits collected in [`prelude`].
//! [`init`] checks the running game against the runtime `sre` describes.

use core::sync::atomic::{AtomicU32, Ordering};
use std::fmt;
use std::sync::OnceLock;

use windows_sys::Win32::System::Threading::{GetCurrentThreadId, Sleep};

pub mod actors;
pub mod animation;
pub mod audio;
pub mod camera;
pub mod events;
pub mod forms;
pub mod input;
pub mod papyrus;
pub mod render;
pub mod save;
pub mod string;
pub mod ui;

pub use string::BSFixedString;

/// The extension traits, so one `use skyrim::prelude::*;` puts the safe helpers next to the
/// generated methods in completion.
pub mod prelude {
    pub use crate::actors::{ActorValueListExt, InventoryChangesExt, TESContainerExt, TESFullNameExt, TESObjectREFRExt};
    pub use crate::animation::{BSAnimationGraphManagerExt, BShkbAnimationGraphExt, IAnimationGraphManagerHolderExt};
    pub use crate::camera::{PlayerCameraExt, TESCameraStateExt};
    pub use crate::forms::{FormClass, TESDataHandlerExt, TESFileExt, TESFormExt};
    pub use crate::input::{ControlMapExt, InputEventExt, InputManagerExt};
    pub use crate::papyrus::VirtualMachineExt;
    pub use crate::render::{BSCullingProcessExt, BSShaderMaterialExt, BSShaderPropertyExt, BSShadowLightExt, BSTextureSetExt, NiAVObjectExt, NiNodeExt, NodeClass, ShadowSceneNodeExt};
    pub use crate::save::{BGSSaveLoadGameExt, BGSSaveLoadManagerExt};
    pub use crate::ui::{GFxMovieViewExt, MenuManagerExt, UIMessageQueueExt};
}

#[derive(Debug)]
pub enum Error {
    /// The game is not the runtime the layouts were generated for; holds `[major, minor, build, store]`.
    Runtime([u32; 4]),
    Hook(hook::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime([major, minor, build, store]) => {
                let [a, b, c] = sre::VERSION;
                write!(f, "runtime {major}.{minor}.{build} store {store} is not {a}.{b}.{c}, the only runtime the layouts describe")
            }
            Self::Hook(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<hook::Error> for Error {
    fn from(e: hook::Error) -> Self {
        Self::Hook(e)
    }
}

static RELOC: OnceLock<hook::Reloc> = OnceLock::new();

/// Checks the runtime and opens the Address Library. Call it once from `SKSEPlugin_Load` before
/// any other function of this crate; a second call is a no-op.
pub fn init(skse: &skse::Interface) -> Result<(), Error> {
    if RELOC.get().is_some() {
        return Ok(());
    }
    let version = skse::runtime::unpack(skse.runtime_version);
    if version[..3] != sre::VERSION {
        return Err(Error::Runtime(version));
    }
    let _ = RELOC.set(hook::Reloc::open(skse.runtime_version)?);
    Ok(())
}

/// The relocation table [`init`] opened, for the mod's own hooks.
///
/// # Panics
/// When [`init`] has not succeeded.
#[must_use]
pub fn reloc() -> &'static hook::Reloc {
    RELOC.get().expect("skyrim::init has not run")
}

/// The engine function at an Address Library id as a function pointer of type `F`, for a call the
/// generated method cannot make (a null where it takes a reference, an id without a typed method).
///
/// # Safety
/// `F` must be a function pointer type matching the function at `id`.
pub(crate) unsafe fn engine_fn<F: Copy>(id: u64) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
    let address = reloc().address(id).unwrap_or_else(|e| panic!("id {id}: {e}"));
    unsafe { core::mem::transmute_copy::<usize, F>(&address) }
}

/// The ledger's vtable for a live object from the vtable pointer in its first 8 bytes: the
/// object's dynamic class, `None` when the pointer is not a vtable the ledger knows.
#[must_use]
pub fn class_of<T>(object: &T) -> Option<&'static sre::raw::Vtable> {
    const { assert!(size_of::<T>() >= size_of::<usize>(), "an object with a vtable starts with its pointer") };
    let vtable = unsafe { core::ptr::from_ref(object).cast::<usize>().read_unaligned() };
    let rva = u32::try_from(vtable.checked_sub(reloc().base())?).ok()?;
    sre::raw::vtables::find(rva)
}

/// Writes a field of an engine object, which the engine reads on its own threads. A mod writes
/// only what the field's doc says the engine re-reads, on the thread the doc names.
pub fn store<T: Copy>(field: &T, value: T) {
    unsafe { core::ptr::from_ref(field).cast_mut().write_volatile(value) }
}

/// A held `BSSpinLock`, the engine's recursive lock (owner thread id at +0, recursion count at +4),
/// spun on the way the engine does (`Sleep(0)`, then `Sleep(1)` after 10,000 tries) and released on drop.
pub(crate) struct SpinGuard(*const sre::BSSpinLock);

impl SpinGuard {
    pub(crate) fn lock(lock: &sre::BSSpinLock) -> Self {
        let (owner, count) = Self::words(lock);
        let me = unsafe { GetCurrentThreadId() };
        let mut spins = 0u32;
        loop {
            if owner.load(Ordering::Acquire) == me {
                count.fetch_add(1, Ordering::Relaxed);
                return Self(lock);
            }
            if owner.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                count.store(1, Ordering::Relaxed);
                return Self(lock);
            }
            spins += 1;
            unsafe { Sleep(u32::from(spins > 10_000)) };
        }
    }

    fn words(lock: &sre::BSSpinLock) -> (&AtomicU32, &AtomicU32) {
        unsafe { (AtomicU32::from_ptr((&raw const lock.owningThread).cast_mut()), AtomicU32::from_ptr((&raw const lock.lockCount).cast_mut())) }
    }
}

impl Drop for SpinGuard {
    fn drop(&mut self) {
        let (owner, count) = Self::words(unsafe { &*self.0 });
        if count.fetch_sub(1, Ordering::Relaxed) == 1 {
            owner.store(0, Ordering::Release);
        }
    }
}
