// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! What the timing plugin (`crates/timing`, shipped as [`FILE`]) measures of the SKSE plugins, and how
//! the host reads it: one export, [`TABLE`], returns a [`Table`] of C functions over records that stay
//! valid for the life of the process. Times are nanoseconds; a start time counts from the process's
//! creation ([`since_start`]).

use core::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// The file name in `Data\SKSE\Plugins`: the leading `!` sorts it before every other plugin, so each loader maps it first.
pub const FILE: &str = "!!!skyrim-runtime-extender-timing.dll";
pub const TABLE: &CStr = c"SkyrimRs_Timing";
pub const VERSION: u32 = 1;

/// One plugin's `SKSEPlugin_Load` call.
#[repr(C)]
pub struct RawLoad {
    pub file: *const c_char,
    pub at: u64,
    pub took: u64,
    pub result: bool,
}

/// One message kind from one sender, as one listener of a plugin saw it.
#[repr(C)]
pub struct RawCalls {
    /// The DLL the listener is in.
    pub module: *const c_char,
    /// The sender it registered for, `every plugin` for a null sender.
    pub asked: *const c_char,
    pub sender: *const c_char,
    pub kind: u32,
    pub count: u32,
    pub total: u64,
    pub max: u64,
}

pub type LoadSink = unsafe extern "C" fn(ctx: *mut c_void, load: *const RawLoad);
pub type CallsSink = unsafe extern "C" fn(ctx: *mut c_void, calls: *const RawCalls);

#[repr(C)]
pub struct Table {
    pub version: u32,
    /// Calls `sink` with every Load timed so far, in call order.
    pub loads: unsafe extern "C" fn(sink: LoadSink, ctx: *mut c_void),
    /// Calls `sink` with every listener's calls so far, in registration order.
    pub calls: unsafe extern "C" fn(sink: CallsSink, ctx: *mut c_void),
    /// What the plugin set up, nul-terminated, for the host's log.
    pub status: unsafe extern "C" fn() -> *const c_char,
}

#[derive(Clone)]
pub struct Load {
    pub file: String,
    pub at: Duration,
    pub took: Duration,
    pub result: bool,
}

#[derive(Clone)]
pub struct Calls {
    pub module: String,
    pub asked: String,
    pub sender: String,
    pub kind: u32,
    pub count: u32,
    pub total: Duration,
    pub max: Duration,
}

fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// The timing plugin's table, `None` when it is not loaded; `Err` with its version when this reader does not know it.
#[must_use]
pub fn table() -> Option<Result<&'static Table, u32>> {
    let name: Vec<u16> = FILE.encode_utf16().chain([0]).collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    if module.is_null() {
        return None;
    }
    let export = unsafe { GetProcAddress(module, TABLE.as_ptr().cast()) }?;
    Some(unsafe { checked(export as *const Table) })
}

/// The table an export points at: the export is the static itself, not a function returning it.
///
/// # Safety
/// `table` must point to a `Table`, or at least its version, that lives for the process.
unsafe fn checked(table: *const Table) -> Result<&'static Table, u32> {
    let version = unsafe { table.cast::<u32>().read() };
    if version == VERSION { Ok(unsafe { &*table }) } else { Err(version) }
}

impl Table {
    #[must_use]
    pub fn loads(&self) -> Vec<Load> {
        unsafe extern "C" fn push(ctx: *mut c_void, raw: *const RawLoad) {
            let (out, raw) = unsafe { (&mut *ctx.cast::<Vec<Load>>(), &*raw) };
            out.push(Load { file: text(raw.file), at: Duration::from_nanos(raw.at), took: Duration::from_nanos(raw.took), result: raw.result });
        }
        let mut out: Vec<Load> = Vec::new();
        unsafe { (self.loads)(push, (&raw mut out).cast()) };
        out
    }

    #[must_use]
    pub fn calls(&self) -> Vec<Calls> {
        unsafe extern "C" fn push(ctx: *mut c_void, raw: *const RawCalls) {
            let (out, raw) = unsafe { (&mut *ctx.cast::<Vec<Calls>>(), &*raw) };
            out.push(Calls { module: text(raw.module), asked: text(raw.asked), sender: text(raw.sender), kind: raw.kind, count: raw.count, total: Duration::from_nanos(raw.total), max: Duration::from_nanos(raw.max) });
        }
        let mut out: Vec<Calls> = Vec::new();
        unsafe { (self.calls)(push, (&raw mut out).cast()) };
        out
    }

    #[must_use]
    pub fn status(&self) -> String {
        text(unsafe { (self.status)() })
    }
}

/// The time since the process was created, from the first call's reading of the creation time.
pub fn since_start() -> Duration {
    static ANCHOR: OnceLock<(Instant, Duration)> = OnceLock::new();
    let (at, then) = *ANCHOR.get_or_init(|| {
        let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut now, mut exit, mut kernel, mut user) = (zero, zero, zero, zero, zero);
        unsafe { GetProcessTimes(GetCurrentProcess(), &raw mut created, &raw mut exit, &raw mut kernel, &raw mut user) };
        unsafe { GetSystemTimePreciseAsFileTime(&raw mut now) };
        (Instant::now(), Duration::from_nanos(ticks(now).saturating_sub(ticks(created)) * 100))
    });
    then + at.elapsed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_clock_is_past_its_start() {
        let (a, b) = (since_start(), since_start());
        assert!(a > Duration::ZERO && b >= a);
        assert!(table().is_none(), "the timing plugin is not loaded in a test");
    }

    unsafe extern "C" fn one_load(sink: LoadSink, ctx: *mut c_void) {
        let raw = RawLoad { file: c"a.dll".as_ptr(), at: 5, took: 2_000_000, result: true };
        unsafe { sink(ctx, &raw const raw) };
    }

    unsafe extern "C" fn no_calls(_: CallsSink, _: *mut c_void) {}

    unsafe extern "C" fn ready() -> *const c_char {
        c"ready".as_ptr()
    }

    static EXPORTED: Table = Table { version: VERSION, loads: one_load, calls: no_calls, status: ready };
    static LATER: Table = Table { version: VERSION + 1, loads: one_load, calls: no_calls, status: ready };

    #[test]
    fn an_exported_table_is_read_in_place() {
        let table = unsafe { checked(&raw const EXPORTED) }.unwrap();
        let loads = table.loads();
        assert_eq!((loads.len(), loads[0].file.as_str(), loads[0].took, loads[0].result), (1, "a.dll", Duration::from_millis(2), true));
        assert!(table.calls().is_empty());
        assert_eq!(table.status(), "ready");
        assert_eq!(unsafe { checked(&raw const LATER) }.err(), Some(VERSION + 1));
    }
}
