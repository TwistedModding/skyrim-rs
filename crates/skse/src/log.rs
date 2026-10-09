//! Log file in the SKSE folder under the user's Documents.

use std::fs::File;
use std::io;
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

use crate::runtime::{STORE_EPIC, STORE_GOG, is_vr, unpack};

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

/// `<Documents>\My Games\<game>\SKSE` for the running game.
pub fn dir(packed_runtime: u32) -> io::Result<PathBuf> {
    let game = if is_vr(packed_runtime) {
        "Skyrim VR"
    } else {
        match unpack(packed_runtime)[3] {
            STORE_GOG => "Skyrim Special Edition GOG",
            STORE_EPIC => "Skyrim Special Edition EPIC",
            _ => "Skyrim Special Edition",
        }
    };
    Ok(documents()?.join("My Games").join(game).join("SKSE"))
}

/// Creates (truncates) `<name>.log` in the SKSE log folder.
pub fn open(packed_runtime: u32, name: &str) -> io::Result<File> {
    let dir = dir(packed_runtime)?;
    std::fs::create_dir_all(&dir)?;
    File::create(dir.join(format!("{name}.log")))
}
