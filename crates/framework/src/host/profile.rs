// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The load profile the Debug tab shows: when the game reached each startup step, how long each
//! framework plugin took to load, and how long the engine spent reading each data file. The SKSE
//! plugins' Loads and listeners are timed by the timing plugin (`crates/timing`), which every loader
//! maps before the others; the host reads its records through [`skse::timing`]. Each data file is timed
//! from its open to its close by a hook on the engine's per-file load.

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use skse::timing::{Load, since_start};
use skyrim::forms::{TESDataHandlerExt as _, TESFileExt as _};

/// `0.4 ms`, `12 ms`, `1.24 s`.
pub(super) fn time(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

static STEPS: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());

/// Notes that the game reached `step`, once.
pub(super) fn step(step: &'static str) {
    let at = since_start();
    let mut steps = STEPS.lock().unwrap_or_else(PoisonError::into_inner);
    if !steps.iter().any(|(s, _)| *s == step) {
        steps.push((step, at));
    }
}

pub(super) fn steps() -> Vec<(&'static str, Duration)> {
    STEPS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

static FRAMEWORK_LOADS: Mutex<Vec<Load>> = Mutex::new(Vec::new());

pub(super) fn framework_loads() -> Vec<Load> {
    FRAMEWORK_LOADS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Times `load`, a framework plugin's load of `file` (reading its DLL and running its entry point).
pub(super) fn framework_load(file: &str, load: impl FnOnce() -> bool) -> bool {
    let at = since_start();
    let start = Instant::now();
    let result = load();
    FRAMEWORK_LOADS.lock().unwrap_or_else(PoisonError::into_inner).push(Load { file: file.to_owned(), took: start.elapsed(), result, at });
    result
}

/// A data file as the profile lists it.
#[derive(Clone)]
pub(super) struct DataFile {
    pub name: String,
    /// `00` to `FD`, or `FE:000` to `FE:FFF` for a light file.
    pub index: String,
    pub master: bool,
    pub light: bool,
    pub took: Duration,
}

/// The files the engine loaded, in load order, and when the first was opened and `DATA_LOADED` arrived.
#[derive(Clone, Default)]
pub(super) struct DataFiles {
    pub files: Vec<DataFile>,
    pub first: Option<Duration>,
    pub done: Option<Duration>,
    /// No file went through the timed load (the hook was not installed, or another plugin's patch bypassed it).
    pub unseen: bool,
}

/// `bool (TESDataHandler*, TESFile* file, bool first)`, id 13760: opens one file, reads all its records
/// and closes it; `LoadFiles` calls it once per loaded file, in load order (RE request LP). The fourth
/// register is passed through untouched.
type LoadFileFn = unsafe extern "C" fn(*mut core::ffi::c_void, *mut sre::TESFile, bool, usize) -> bool;

static LOAD_FILE: OnceLock<hook::Hook<LoadFileFn>> = OnceLock::new();
/// `TESFile` addresses with the time spent reading each.
static READ: Mutex<Vec<(usize, Duration)>> = Mutex::new(Vec::new());
static FIRST_FILE: OnceLock<Duration> = OnceLock::new();
static DATA_FILES: OnceLock<DataFiles> = OnceLock::new();

unsafe extern "C" fn timed_file(handler: *mut core::ffi::c_void, file: *mut sre::TESFile, first: bool, passed: usize) -> bool {
    let original = LOAD_FILE.get().expect("stored before the hook was enabled").original();
    FIRST_FILE.get_or_init(since_start);
    let start = Instant::now();
    let read = unsafe { original(handler, file, first, passed) };
    let took = start.elapsed();
    let mut times = READ.lock().unwrap_or_else(PoisonError::into_inner);
    match times.iter_mut().find(|(f, _)| *f == file as usize) {
        Some((_, t)) => *t += took,
        None => times.push((file as usize, took)),
    }
    read
}

/// Hooks the engine's per-file load, before the init thread starts loading the data files.
pub(super) fn time_data_files() -> Result<(), hook::Error> {
    if LOAD_FILE.get().is_none() {
        let target = skyrim::reloc().address(sre::id!(13760))?;
        let _ = LOAD_FILE.set(unsafe { hook::Hook::create(target, timed_file as LoadFileFn) }?);
    }
    LOAD_FILE.get().expect("set above").enable()
}

/// Matches the per-file times to the loaded files at `DATA_LOADED`.
pub(super) fn finish_data_files() {
    let done = since_start();
    let times = READ.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(handler) = sre::TESDataHandler::singleton() else { return };
    let took = |file: &sre::TESFile| times.iter().find(|(f, _)| *f == core::ptr::from_ref(file) as usize).map_or(Duration::ZERO, |(_, t)| *t);
    let mut files: Vec<DataFile> = handler.compiled_files().iter().flatten().enumerate().map(|(i, f)| DataFile { name: f.name().to_owned(), index: format!("{i:02X}"), master: f.is_master(), light: false, took: took(f) }).collect();
    files.extend(handler.light_files().iter().flatten().enumerate().map(|(i, f)| DataFile { name: f.name().to_owned(), index: format!("FE:{i:03X}"), master: f.is_master(), light: true, took: took(f) }));
    let _ = DATA_FILES.set(DataFiles { unseen: times.is_empty(), files, first: FIRST_FILE.get().copied(), done: Some(done) });
}

pub(super) fn data_files() -> Option<&'static DataFiles> {
    DATA_FILES.get()
}

/// The profile at `DATA_LOADED` for the host log: the SKSE plugins' Loads and listeners, and the data files.
pub(super) fn summary() -> String {
    let skse = match skse::timing::table() {
        Some(Ok(timing)) => {
            let mut loads = timing.loads();
            loads.sort_by_key(|l| core::cmp::Reverse(l.took));
            let total: Duration = loads.iter().map(|l| l.took).sum();
            let slowest: Vec<String> = loads.iter().take(3).map(|l| format!("{} {}", l.file, time(l.took))).collect();
            let calls = timing.calls();
            format!("{} SKSE plugin loads timed, {} in all (slowest {}); {} listener calls, {}", loads.len(), time(total), slowest.join(", "), calls.iter().map(|c| c.count).sum::<u32>(), time(calls.iter().map(|c| c.total).sum()))
        }
        _ => "SKSE plugins not timed".to_owned(),
    };
    let files = data_files().map_or_else(
        || "data files not read".to_owned(),
        |d| {
            let light = d.files.iter().filter(|f| f.light).count();
            let seen = d.files.iter().filter(|f| !f.took.is_zero()).count();
            let slowest = d.files.iter().max_by_key(|f| f.took).filter(|f| !f.took.is_zero()).map_or_else(String::new, |f| format!(", slowest {} {}", f.name, time(f.took)));
            let span = d.first.zip(d.done).map_or_else(String::new, |(first, done)| format!(", {} from the first opened to DATA_LOADED", time(done.saturating_sub(first))));
            format!("{} data files ({} full, {light} light), {seen} timed{slowest}{span}", d.files.len(), d.files.len() - light)
        },
    );
    format!("{skse}; {files}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framework_load_is_timed() {
        assert!(framework_load("x.dll", || true));
        assert!(framework_loads().iter().any(|l| l.file == "x.dll" && l.result && l.at > Duration::ZERO));
    }
}
