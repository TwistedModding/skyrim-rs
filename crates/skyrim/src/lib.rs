//! Game layer: safe Rust over the generated layouts, one module per subsystem pass.
//!
//! Everything here is for the runtime `sre` describes. [`init`] checks the running
//! game against it, resolves every Address Library id the modules need once, and keeps the
//! [`hook::Reloc`] for the mod's own use.

use core::ffi::c_void;
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

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

struct Runtime {
    reloc: hook::Reloc,
    forms: forms::Fns,
    actors: actors::Fns,
    events: events::Fns,
    papyrus: papyrus::Fns,
    render: render::Fns,
    camera: camera::Fns,
    animation: animation::Fns,
    strings: string::Fns,
    ui: ui::Fns,
    input: input::Fns,
    save: save::Fns,
    audio: audio::Fns,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get().expect("skyrim::init has not run")
}

/// The function behind an Address Library id as a function pointer of type `F`.
///
/// # Safety
/// `F` must be a function pointer type matching the signature of the function at `id`.
unsafe fn resolve<F: Copy>(reloc: &hook::Reloc, id: u64) -> Result<F, hook::Error> {
    const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
    let address = reloc.address(id)?;
    Ok(unsafe { core::mem::transmute_copy::<usize, F>(&address) })
}

/// Checks the runtime, opens the Address Library and resolves every id the game layer uses.
/// Call it once from `SKSEPlugin_Load` before any other function of this crate; a second call is a no-op.
pub fn init(skse: &skse::Interface) -> Result<(), Error> {
    if RUNTIME.get().is_some() {
        return Ok(());
    }
    let version = skse::runtime::unpack(skse.runtime_version);
    if version[..3] != sre::VERSION {
        return Err(Error::Runtime(version));
    }
    let reloc = hook::Reloc::open(skse.runtime_version)?;
    let forms = forms::Fns::resolve(&reloc)?;
    let actors = actors::Fns::resolve(&reloc)?;
    let events = events::Fns::resolve(&reloc)?;
    let papyrus = papyrus::Fns::resolve(&reloc)?;
    let render = render::Fns::resolve(&reloc)?;
    let camera = camera::Fns::resolve(&reloc)?;
    let animation = animation::Fns::resolve(&reloc)?;
    let strings = string::Fns::resolve(&reloc)?;
    let ui = ui::Fns::resolve(&reloc)?;
    let input = input::Fns::resolve(&reloc)?;
    let save = save::Fns::resolve(&reloc)?;
    let audio = audio::Fns::resolve(&reloc)?;
    let _ = RUNTIME.set(Runtime { reloc, forms, actors, events, papyrus, render, camera, animation, strings, ui, input, save, audio });
    Ok(())
}

/// The relocation table [`init`] opened, for resolving ids beyond what this crate wraps.
///
/// # Panics
/// When [`init`] has not succeeded.
#[must_use]
pub fn reloc() -> &'static hook::Reloc {
    &runtime().reloc
}

/// The ledger's vtable for a live object, from the vtable pointer in its first 8 bytes.
///
/// # Safety
/// `object` must point to a live object of a class with a vtable at offset 0.
pub(crate) unsafe fn class_of(object: *const c_void) -> Option<&'static sre::raw::Vtable> {
    let vtable = unsafe { *object.cast::<usize>() };
    let rva = u32::try_from(vtable.checked_sub(reloc().base())?).ok()?;
    sre::raw::vtables::find(rva)
}

/// A held `BSSpinLock`, the engine's recursive lock (owner thread id at +0, recursion count at +4),
/// spun on the way the engine does (`Sleep(0)`, then `Sleep(1)` after 10,000 tries) and released on drop.
pub(crate) struct SpinGuard(*const sre::BSSpinLock);

impl SpinGuard {
    /// # Safety
    /// `lock` must point to a `BSSpinLock` that outlives the guard.
    pub(crate) unsafe fn lock(lock: *const sre::BSSpinLock) -> Self {
        let (owner, count) = unsafe { Self::words(lock) };
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

    unsafe fn words<'a>(lock: *const sre::BSSpinLock) -> (&'a AtomicU32, &'a AtomicU32) {
        unsafe { (AtomicU32::from_ptr((&raw const (*lock).owningThread).cast_mut()), AtomicU32::from_ptr((&raw const (*lock).lockCount).cast_mut())) }
    }
}

impl Drop for SpinGuard {
    fn drop(&mut self) {
        let (owner, count) = unsafe { Self::words(self.0) };
        if count.fetch_sub(1, Ordering::Relaxed) == 1 {
            owner.store(0, Ordering::Release);
        }
    }
}
