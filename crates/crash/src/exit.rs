// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! One line in the host's log saying how the game ended: `ExitProcess` and `TerminateProcess` on
//! the game both reach `NtTerminateProcess`, so a quit to the desktop that raises no exception
//! still names the thread and the modules that called it.

use core::fmt::Write as _;
use std::io::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{HANDLE, HMODULE};
use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, GetProcessId};

type TerminateFn = unsafe extern "system" fn(HANDLE, i32) -> i32;

static HOOK: OnceLock<hook::Hook<TerminateFn>> = OnceLock::new();
static LOG: OnceLock<(u32, &'static str)> = OnceLock::new();
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static WATCHDOG: AtomicU32 = AtomicU32::new(0);
static WRITTEN: AtomicBool = AtomicBool::new(false);

/// Hooks `NtTerminateProcess` to append the line to `<name>.log`; called on the game's main thread.
pub(crate) fn install(runtime: u32, name: &'static str, watchdog: u32) -> Result<(), String> {
    let ntdll = unsafe { GetModuleHandleW(windows_sys::w!("ntdll.dll")) };
    let target = unsafe { GetProcAddress(ntdll, c"NtTerminateProcess".as_ptr().cast()) }.ok_or("ntdll.dll has no NtTerminateProcess")?;
    let _ = LOG.set((runtime, name));
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    WATCHDOG.store(watchdog, Ordering::Release);
    let hook = unsafe { hook::Hook::<TerminateFn>::create(target as usize, terminate) }.map_err(|e| format!("NtTerminateProcess: {e}"))?;
    HOOK.get_or_init(|| hook).enable().map_err(|e| format!("NtTerminateProcess: {e}"))
}

unsafe extern "system" fn terminate(process: HANDLE, status: i32) -> i32 {
    let own = process.is_null() || process as isize == -1 || unsafe { GetProcessId(process) } == unsafe { GetCurrentProcessId() };
    if own && !WRITTEN.swap(true, Ordering::AcqRel) {
        note(status);
    }
    HOOK.get().map_or(0, |hook| unsafe { hook.original()(process, status) })
}

fn note(status: i32) {
    let Some(&(runtime, name)) = LOG.get() else { return };
    let thread = unsafe { GetCurrentThreadId() };
    let kind = if thread == MAIN_THREAD.load(Ordering::Acquire) {
        "the main thread"
    } else if thread == WATCHDOG.load(Ordering::Acquire) {
        "the crash handler's watchdog"
    } else {
        "thread"
    };
    let mut frames = [core::ptr::null_mut(); 16];
    let count = usize::from(unsafe { RtlCaptureStackBackTrace(1, 16, frames.as_mut_ptr(), core::ptr::null_mut()) });
    let mut line = format!("the game ended: exit code {:#x} on {kind} {thread}, called from", status.cast_unsigned());
    for (i, &frame) in frames[..count].iter().enumerate() {
        let _ = write!(line, "{}{}", if i == 0 { " " } else { " < " }, Located(frame as usize));
    }
    if let Ok(mut f) = skse::log::append(runtime, name) {
        let _ = writeln!(f, "{line}");
    }
}

/// `module+0xoffset` for a code address.
struct Located(usize);

impl core::fmt::Display for Located {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut module: HMODULE = core::ptr::null_mut();
        let found = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, self.0 as *const u16, &raw mut module) } != 0;
        if !found {
            return write!(f, "{:#x}", self.0);
        }
        let mut path = [0u16; 260];
        let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), 260) } as usize;
        let path = String::from_utf16_lossy(&path[..len]);
        write!(f, "{}+{:#x}", path.rsplit('\\').next().unwrap_or(&path), self.0 - module as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[inline(never)]
    fn caller() {
        note(0x2a);
    }

    #[test]
    fn the_end_line_names_the_calling_module() {
        let name = "crash-exit-test";
        let _ = LOG.set((0, name));
        let path = skse::log::dir(0).unwrap().join(format!("{name}.log"));
        let _ = std::fs::remove_file(&path);
        caller();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let exe = std::env::current_exe().unwrap();
        let exe = exe.file_name().unwrap().to_string_lossy();
        assert!(text.starts_with("the game ended: exit code 0x2a on thread "), "{text}");
        assert!(text.matches(&format!("{exe}+0x")).count() >= 2, "{text}");
    }
}
