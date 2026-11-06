// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Phase 1: the per-frame memo across the sun's cascade views. `GetDepthRenderPasses` answers the
//! same for every cascade within a frame (RE `graphics.md` DC.2: same mode, same accumulator flags,
//! fades not advanced by the map culls), and a geometry it drops in cascade 0 is dropped again in
//! cascade 1 after the whole registration chain has run for nothing. The memo keeps the dropped
//! geometries of the frame's first cascade; the later cascades' `RegisterDepthGeometry` returns
//! without the call for them. A geometry that casts gets a pass object per accumulator and is not
//! cached. Every cascade registration runs on the frame thread, one cascade after the other.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use sre::BSShaderAccumulator;

const SLOTS: usize = 1 << 16;
const PROBES: usize = 16;
const CASCADE_MODE: u32 = 0xe;

struct Entry {
    geometry: AtomicUsize,
    frame: AtomicU32,
}

static TABLE: [Entry; SLOTS] = [const { Entry { geometry: AtomicUsize::new(0), frame: AtomicU32::new(0) } }; SLOTS];
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// The frame tag stored with an entry: never 0, so a zero slot is one never used.
fn tag() -> u32 {
    skyrim::render::frame_counter().wrapping_add(1).max(1)
}

fn slot(geometry: usize) -> usize {
    (geometry >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (usize::BITS - 16)
}

/// The cascade of a sun map accumulator, `None` for every other accumulator.
fn cascade(accumulator: *mut BSShaderAccumulator) -> Option<u32> {
    let a = unsafe { accumulator.as_ref()? };
    (a.renderMode == CASCADE_MODE && a.isCascadeAccumulator != 0).then_some(a.cascadeIndex)
}

/// Remembers that `accumulator`'s slot 43 call for `geometry` returned no pass; only the first
/// cascade's drops are kept, and only those count as recorded.
pub fn record_drop(accumulator: *mut BSShaderAccumulator, geometry: usize) -> bool {
    if cascade(accumulator) != Some(0) {
        return false;
    }
    let (tag, start) = (tag(), slot(geometry));
    for i in 0..PROBES {
        let entry = &TABLE[(start + i) % SLOTS];
        let key = entry.geometry.load(Ordering::Relaxed);
        if key == geometry || entry.frame.load(Ordering::Relaxed) != tag {
            entry.geometry.store(geometry, Ordering::Relaxed);
            entry.frame.store(tag, Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// Whether a later cascade's registration of `geometry` can be skipped: the memo is on and the
/// first cascade dropped the geometry this frame.
pub fn skip(accumulator: *mut BSShaderAccumulator, geometry: usize) -> bool {
    if !enabled() || !matches!(cascade(accumulator), Some(c) if c > 0) {
        return false;
    }
    let (tag, start) = (tag(), slot(geometry));
    for i in 0..PROBES {
        let entry = &TABLE[(start + i) % SLOTS];
        let key = entry.geometry.load(Ordering::Relaxed);
        if key == 0 {
            return false;
        }
        if key == geometry {
            return entry.frame.load(Ordering::Relaxed) == tag;
        }
    }
    false
}
