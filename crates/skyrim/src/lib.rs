//! The safe layer over `sre`: what the generated methods cannot give a mod without `unsafe`.
//!
//! The game itself is `sre`: `sre::TESForm`, `sre::PlayerCharacter` and every other class with
//! its engine functions as methods. This crate adds the objects that own or guard something (the
//! interned [`BSFixedString`], reference and script object handles that release, the engine's spin
//! lock), the event sinks, Papyrus natives and dispatch, a mod's own menus and console commands, the
//! hook installers, and per class the few reads that would otherwise need `unsafe` (a C string by
//! `&str`, an engine array as a slice, a checked cast), as extension traits collected in [`prelude`].
//! [`init`] checks the running game against the runtime `sre` describes.

use std::fmt;
use std::sync::OnceLock;

#[cfg(feature = "actors")]
pub mod actors;
#[cfg(feature = "animation")]
pub mod animation;
#[cfg(feature = "audio")]
pub mod audio;
#[cfg(feature = "camera")]
pub mod camera;
#[cfg(feature = "events")]
pub mod events;
#[cfg(feature = "forms")]
pub mod forms;
#[cfg(feature = "input")]
pub mod input;
#[cfg(feature = "papyrus")]
pub mod papyrus;
#[cfg(feature = "physics")]
pub mod physics;
#[cfg(feature = "render")]
pub mod render;
#[cfg(feature = "save")]
pub mod save;
pub mod string;
#[cfg(feature = "ui")]
pub mod ui;

pub use string::BSFixedString;

/// The extension traits of the enabled modules, so one `use skyrim::prelude::*;` puts the safe
/// helpers next to the generated methods in completion.
pub mod prelude {
    #[cfg(feature = "actors")]
    pub use crate::actors::{ActorExt, ActorValueListExt, InventoryChangesExt, PlayerCharacterExt, TESContainerExt, TESFullNameExt, TESObjectREFRExt};
    #[cfg(feature = "animation")]
    pub use crate::animation::{BSAnimationGraphManagerExt, BShkbAnimationGraphExt, IAnimationGraphManagerHolderExt};
    #[cfg(feature = "camera")]
    pub use crate::camera::{PlayerCameraExt, TESCameraStateExt};
    #[cfg(feature = "forms")]
    pub use crate::forms::{FormClass, TESDataHandlerExt, TESFileExt, TESFormExt};
    #[cfg(feature = "input")]
    pub use crate::input::{ButtonEventExt, ControlMapExt, InputEventExt, InputManagerExt};
    #[cfg(feature = "papyrus")]
    pub use crate::papyrus::VirtualMachineExt;
    #[cfg(feature = "render")]
    pub use crate::render::{BSCullingProcessExt, BSFlattenedBoneTreeExt, BSShaderMaterialExt, BSShaderPropertyExt, BSShadowLightExt, BSTextureSetExt, NiAVObjectExt, NiNodeExt, NiPointLightExt, NiSkinInstanceExt, NodeClass, ShadowSceneNodeExt, TESObjectLIGHExt};
    #[cfg(feature = "save")]
    pub use crate::save::{BGSSaveLoadGameExt, BGSSaveLoadManagerExt};
    #[cfg(feature = "ui")]
    pub use crate::ui::{GFxMovieViewExt, MenuManagerExt, RaceSexMenuExt, UIMessageQueueExt};
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

/// `len` pointers at `data` as optional references.
///
/// # Safety
/// `data` must point to `len` pointers that stay readable and unchanged for `'a`, each null or
/// pointing to a live `T`.
#[cfg(any(feature = "actors", feature = "animation", feature = "forms", feature = "render", feature = "save"))]
pub(crate) unsafe fn ptr_slice<'a, T>(data: *const core::ffi::c_void, len: u32) -> &'a [Option<&'a T>] {
    if data.is_null() || len == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(data.cast::<Option<&'a T>>(), len as usize) }
}

/// A held `BSReadWriteLock` (the engine's recursive reader/writer lock), taken and released through
/// the engine's own functions so a writer on this thread may take it again.
#[cfg(feature = "actors")]
pub(crate) struct RwGuard {
    lock: *const sre::BSReadWriteLock,
    write: bool,
}

#[cfg(feature = "actors")]
impl RwGuard {
    pub(crate) fn read(lock: &sre::BSReadWriteLock) -> Self {
        Self::take(lock, false)
    }

    #[cfg(feature = "physics")]
    pub(crate) fn write(lock: &sre::BSReadWriteLock) -> Self {
        Self::take(lock, true)
    }

    fn take(lock: &sre::BSReadWriteLock, write: bool) -> Self {
        let id = if write { sre::raw::fns::BSReadWriteLock__LockForWrite } else { sre::raw::fns::BSReadWriteLock__LockForRead };
        let f: unsafe extern "C" fn(*const sre::BSReadWriteLock) = unsafe { engine_fn(id) };
        unsafe { f(lock) };
        Self { lock, write }
    }
}

#[cfg(feature = "actors")]
impl Drop for RwGuard {
    fn drop(&mut self) {
        let id = if self.write { sre::raw::fns::BSReadWriteLock__UnlockForWrite } else { sre::raw::fns::BSReadWriteLock__UnlockForRead };
        let f: unsafe extern "C" fn(*const sre::BSReadWriteLock) = unsafe { engine_fn(id) };
        unsafe { f(self.lock) };
    }
}

/// The calling thread's id, as the engine's locks record their owner.
#[must_use]
pub fn thread_id() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(any(feature = "animation", feature = "events", feature = "render", feature = "ui"))]
mod spin {
    use core::sync::atomic::{AtomicU32, Ordering};

    use windows_sys::Win32::System::Threading::Sleep;

    /// A held `BSSpinLock`, the engine's recursive lock (owner thread id at +0, recursion count at +4),
    /// taken and released as the engine does (UI `0x1169590`): the count is the lock word, moved
    /// 0 -> 1 by compare-exchange, and the owner is written after it; `Sleep(0)`, then `Sleep(1)`
    /// after 10,000 tries. Released on drop.
    pub(crate) struct SpinGuard(*const sre::BSSpinLock);

    impl SpinGuard {
        pub(crate) fn lock(lock: &sre::BSSpinLock) -> Self {
            let (owner, count) = Self::words(lock);
            let me = super::thread_id();
            if owner.load(Ordering::Acquire) == me {
                count.fetch_add(1, Ordering::AcqRel);
                return Self(lock);
            }
            let mut spins = 0u32;
            while count.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
                spins += 1;
                unsafe { Sleep(u32::from(spins > 10_000)) };
            }
            owner.store(me, Ordering::Release);
            Self(lock)
        }

        fn words(lock: &sre::BSSpinLock) -> (&AtomicU32, &AtomicU32) {
            unsafe { (AtomicU32::from_ptr((&raw const lock.owningThread).cast_mut()), AtomicU32::from_ptr((&raw const lock.lockCount).cast_mut())) }
        }
    }

    impl Drop for SpinGuard {
        fn drop(&mut self) {
            let (owner, count) = Self::words(unsafe { &*self.0 });
            if count.load(Ordering::Acquire) == 1 {
                owner.store(0, Ordering::SeqCst);
                count.store(0, Ordering::SeqCst);
            } else {
                count.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}
