// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The framework plugin images the host reports: where each generation was mapped, and which
//! ones a reload has unloaded since, so an address in a freed image is named as such.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::mem::{Live, Source};
use crate::record::{Image, MAX_IMAGES, NAME_LEN, Record, store};

struct Table {
    count: usize,
    images: [Image; MAX_IMAGES],
}

struct Shared(core::cell::UnsafeCell<Table>);

unsafe impl Sync for Shared {}

static LOCK: AtomicBool = AtomicBool::new(false);
static TABLE: Shared = Shared(core::cell::UnsafeCell::new(Table { count: 0, images: [Image { base: 0, size: 0, generation: 0, live: 0, name_len: 0, name: [0; NAME_LEN] }; MAX_IMAGES] }));

fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let r = f(unsafe { &mut *TABLE.0.get() });
    LOCK.store(false, Ordering::Release);
    r
}

/// The image size from the PE header at `base` (`SizeOfImage` in the 64-bit optional header).
fn image_size(base: u64) -> u32 {
    let src = Live;
    src.read_u32(base + 0x3c).and_then(|nt| src.read_u32(base + u64::from(nt) + 0x50)).unwrap_or(0)
}

/// Records a plugin generation mapped at `base` (`live`), or marks it unloaded (`!live`).
pub fn note(name: &str, generation: u32, base: usize, live: bool) {
    let base = base as u64;
    with(|t| {
        let found = t.images[..t.count].iter_mut().find(|i| i.generation == generation && i.name() == name);
        match (found, live) {
            (Some(image), live) => {
                image.live = u8::from(live);
                if live {
                    image.base = base;
                    image.size = image_size(base);
                }
            }
            (None, true) if t.count < MAX_IMAGES => {
                let image = &mut t.images[t.count];
                *image = Image { base, size: image_size(base), generation, live: 1, name_len: 0, name: [0; NAME_LEN] };
                image.name_len = u8::try_from(store(&mut image.name, name)).unwrap_or(0);
                t.count += 1;
            }
            (None, _) => {}
        }
    });
}

/// Copies the table into the record; reads without the lock when a crash holds it.
pub(crate) fn copy_into(rec: &mut Record) {
    let table = unsafe { &*TABLE.0.get() };
    let count = table.count.min(MAX_IMAGES);
    rec.images[..count].copy_from_slice(&table.images[..count]);
    rec.image_count = u32::try_from(count).unwrap_or(0);
}
