// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Log file in the SKSE folder under the user's Documents.

use std::fs::File;
use std::io;
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
/// their logs (the only folder seen; other stores and VR are unverified and take the same path).
pub fn dir(_packed_runtime: u32) -> io::Result<PathBuf> {
    Ok(documents()?.join("My Games").join("Skyrim Special Edition").join("SKSE"))
}

/// Creates (truncates) `<name>.log` in the SKSE log folder.
pub fn open(packed_runtime: u32, name: &str) -> io::Result<File> {
    let dir = dir(packed_runtime)?;
    std::fs::create_dir_all(&dir)?;
    File::create(dir.join(format!("{name}.log")))
}

/// Opens `<name>.log` in the SKSE log folder for appending, creating it when absent.
pub fn append(packed_runtime: u32, name: &str) -> io::Result<File> {
    let dir = dir(packed_runtime)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::OpenOptions::new().append(true).create(true).open(dir.join(format!("{name}.log")))
}
