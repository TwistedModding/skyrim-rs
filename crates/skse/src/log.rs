// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Log file in the SKSE folder under the user's Documents.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::Debug::RaiseException;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};


fn documents() -> io::Result<PathBuf> {
    let mut path = core::ptr::null_mut::<u16>();
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, core::ptr::null_mut(), &raw mut path) };
    if hr < 0 || path.is_null() {
        return Err(io::Error::other("SHGetKnownFolderPath failed"));
    }
    let mut len = 0;
    while unsafe { *path.add(len) } != 0 {
        len += 1;
    }
    let wide = unsafe { core::slice::from_raw_parts(path, len) };
    let out = PathBuf::from(String::from_utf16_lossy(wide));
    unsafe { CoTaskMemFree(path.cast()) };
    Ok(out)
}

/// `<Documents>\My Games\Skyrim Special Edition\SKSE`, where the Steam SE and AE loaders write
/// their logs, or `Skyrim Special Edition GOG` on GOG, where its loader writes them.
pub fn dir(packed_runtime: u32) -> io::Result<PathBuf> {
    let game = if crate::runtime::is_gog(packed_runtime) { "Skyrim Special Edition GOG" } else { "Skyrim Special Edition" };
    Ok(documents()?.join("My Games").join(game).join("SKSE"))
}

/// Creates (truncates) `<name>.log` in the SKSE log folder.
pub fn open(packed_runtime: u32, name: &str) -> io::Result<File> {
    let dir = dir(packed_runtime)?;
    std::fs::create_dir_all(&dir)?;
    File::create(dir.join(format!("{name}.log")))
}

/// The exception [`log_panics`] raises for a panic, continuable, with the address and length of the
/// panic's UTF-8 line as its two parameters.
pub const PANIC_EXCEPTION: u32 = 0xE052_5354;

/// Makes every panic in the calling DLL append where it happened and its message, under `who`, to
/// `<file>.log`, then raise [`PANIC_EXCEPTION`] so a crash logger reports it with its stack before
/// the abort that follows; release builds abort on a panic, which a crash logger never sees. The
/// hook belongs to the DLL that calls this, as each has its own std.
pub fn log_panics(packed_runtime: u32, file: &'static str, who: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        let line = format!("{who}: panic on {:?}: {info}", std::thread::current().id());
        if let Ok(mut f) = append(packed_runtime, file) {
            let _ = writeln!(f, "{line}");
        }
        let parameters = [line.as_ptr() as usize, line.len()];
        unsafe { RaiseException(PANIC_EXCEPTION, 0, 2, parameters.as_ptr()) };
    }));
}

/// Opens `<name>.log` in the SKSE log folder for appending, creating it when absent.
pub fn append(packed_runtime: u32, name: &str) -> io::Result<File> {
    let dir = dir(packed_runtime)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::OpenOptions::new().append(true).create(true).open(dir.join(format!("{name}.log")))
}

#[cfg(test)]
mod tests {
    use super::*;

    use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS, RemoveVectoredExceptionHandler};

    static RAISED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

    unsafe extern "system" fn catch(info: *mut EXCEPTION_POINTERS) -> i32 {
        let record = unsafe { &*(*info).ExceptionRecord };
        if record.ExceptionCode.cast_unsigned() != PANIC_EXCEPTION || record.NumberParameters != 2 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let [at, len] = [record.ExceptionInformation[0], record.ExceptionInformation[1]];
        let bytes = unsafe { core::slice::from_raw_parts(at as *const u8, len) };
        *RAISED.lock().unwrap() = String::from_utf8_lossy(bytes).into_owned();
        EXCEPTION_CONTINUE_EXECUTION
    }

    #[test]
    fn a_panic_is_appended_to_the_log_before_it_ends_the_thread() {
        let handler = unsafe { AddVectoredExceptionHandler(1, Some(catch)) };
        let file = "skse-panic-hook-test";
        let path = dir(0).unwrap().join(format!("{file}.log"));
        let _ = std::fs::remove_file(&path);
        log_panics(0, file, "test");
        let joined = std::thread::spawn(|| panic!("forced")).join();
        let _ = std::panic::take_hook();
        unsafe { RemoveVectoredExceptionHandler(handler) };
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(joined.is_err());
        assert!(text.starts_with("test: panic on ThreadId(") && text.contains("log.rs") && text.contains("forced"), "{text}");
        assert_eq!(*RAISED.lock().unwrap(), text.trim_end(), "the exception carries the logged line");
    }
}
