// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Log file in the SKSE folder under the user's Documents.

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
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

/// Makes every panic in the calling DLL append where it happened and its message, under `who`, to
/// `<file>.log` before the abort that follows it; release builds abort on a panic, which leaves no
/// trace for a crash logger. The hook belongs to the DLL that calls this, as each has its own std.
pub fn log_panics(packed_runtime: u32, file: &'static str, who: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = append(packed_runtime, file) {
            let _ = writeln!(f, "{who}: panic on {:?}: {info}", std::thread::current().id());
        }
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

    #[test]
    fn a_panic_is_appended_to_the_log_before_it_ends_the_thread() {
        let file = "skse-panic-hook-test";
        let path = dir(0).unwrap().join(format!("{file}.log"));
        let _ = std::fs::remove_file(&path);
        log_panics(0, file, "test");
        let joined = std::thread::spawn(|| panic!("forced")).join();
        let _ = std::panic::take_hook();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(joined.is_err());
        assert!(text.starts_with("test: panic on ThreadId(") && text.contains("log.rs") && text.contains("forced"), "{text}");
    }
}
