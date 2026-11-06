// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! A `MinHook` detour on an Address Library id that a detour reads lock-free and `unload` removes.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

pub struct Detour<F: Copy> {
    original: AtomicUsize,
    hook: Mutex<Option<hook::Hook<F>>>,
}

impl<F: Copy> Detour<F> {
    pub const fn new() -> Self {
        const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
        Self { original: AtomicUsize::new(0), hook: Mutex::new(None) }
    }

    /// Creates and enables the detour on the function at `id`.
    ///
    /// # Safety
    /// Same as [`hook::Hook::create`]: `F` must be the function's signature.
    pub unsafe fn install(&self, id: u64, detour: F, what: &str) -> Result<(), String> {
        let mut slot = self.hook.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err(format!("{what}: already installed"));
        }
        let address = skyrim::reloc().address(id).map_err(|e| format!("{what}: {e}"))?;
        let hook = unsafe { hook::Hook::create(address, detour) }.map_err(|e| format!("{what}: {e}"))?;
        self.original.store(unsafe { core::mem::transmute_copy::<F, usize>(&hook.original()) }, Ordering::Release);
        hook.enable().map_err(|e| format!("{what}: enable: {e}"))?;
        *slot = Some(hook);
        Ok(())
    }

    /// The function as it was before the detour.
    ///
    /// # Panics
    /// Before [`install`](Self::install).
    pub fn original(&self) -> F {
        let bits = self.original.load(Ordering::Acquire);
        assert!(bits != 0, "detour called before its trampoline was stored");
        unsafe { core::mem::transmute_copy::<usize, F>(&bits) }
    }

    /// Removes the detour; the trampoline stays readable for a detour still returning through it.
    pub fn remove(&self) {
        if let Some(hook) = self.hook.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = unsafe { hook.remove() };
        }
    }
}
