// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Known game runtimes and the Address Library file that goes with each.
//!
//! The packed version is the number the SKSE loader prints after the version in its log
//! (`version = 2.3.1 01070680` for 1.7.104.0, `01050610` for 1.5.97.0): major in the top byte,
//! minor in the next, build in the next twelve bits, the store in the low nibble: 0 on Steam, 1 on
//! GOG (its 2.2.6 loader logs `010649B1` for 1.6.1179.0). The table lists the runtimes an Address Library file or a game install on
//! this machine proved; a runtime missing here is unknown, not unsupported.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};

/// The runtime `sre` describes; every other runtime is its difference from this one.
pub const CANONICAL: u32 = pack(1, 7, 104, 0);

#[must_use]
pub const fn pack(major: u32, minor: u32, build: u32, store: u32) -> u32 {
    (major & 0xFF) << 24 | (minor & 0xFF) << 16 | (build & 0xFFF) << 4 | (store & 0xF)
}

#[must_use]
pub const fn unpack(packed: u32) -> [u32; 4] {
    [packed >> 24, (packed >> 16) & 0xFF, (packed >> 4) & 0xFFF, packed & 0xF]
}

/// Whether a packed version is the canonical runtime, on any store.
#[must_use]
pub const fn is_canonical(packed: u32) -> bool {
    packed >> 4 == CANONICAL >> 4
}

/// The GOG release, whose user folder is `My Games\Skyrim Special Edition GOG`.
#[must_use]
pub const fn is_gog(packed: u32) -> bool {
    packed & 0xF == 1
}

/// 1.5.x is Special Edition; 1.6 and later are Anniversary Edition.
#[must_use]
pub const fn is_se(packed: u32) -> bool {
    packed >> 16 <= 0x0105
}

/// Address Library file name for a runtime: the SE files are `version-M-m-b-0.bin`, the AE files
/// `versionlib-M-m-b-0.bin` (the 1.7.104 SKSE DLL carries the format string
/// `Data\SKSE\Plugins\versionlib-%d-%d-%d-%d.bin`).
#[must_use]
pub fn versionlib_name(packed: u32) -> String {
    let [major, minor, build, _] = unpack(packed);
    let prefix = if is_se(packed) { "version" } else { "versionlib" };
    format!("{prefix}-{major}-{minor}-{build}-0.bin")
}

/// Whether the script extender's DLL for a runtime (`skse64_M_m_b.dll`, as in the game folder) is loaded in this process.
#[must_use]
pub fn script_extender_loaded(packed: u32) -> bool {
    let [major, minor, build, _] = unpack(packed);
    let name: Vec<u16> = format!("skse64_{major}_{minor}_{build}.dll").encode_utf16().chain([0]).collect();
    !unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(name.as_ptr()) }.is_null()
}

/// The running executable's version, packed, with no loader involved. `None` when the executable
/// has no version resource, as a test binary has none. The file version names no store, so a
/// runtime in the table takes its store from there.
#[must_use]
pub fn running() -> Option<u32> {
    let [major, minor, build, store] = file_version(&std::env::current_exe().ok()?)?;
    let packed = pack(major, minor, build, store);
    Some(RUNTIMES.iter().find(|r| r.packed >> 4 == packed >> 4).map_or(packed, |r| r.packed))
}

/// The `FileVersion` string of a file's version resource. The fixed block next to it is no use:
/// 1.5.97's says 1.0.0.0.
fn file_version(path: &Path) -> Option<[u32; 4]> {
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), core::ptr::null_mut()) };
    let mut block = vec![0u32; (size as usize).div_ceil(4)];
    if size == 0 || unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, block.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let (translation, len) = version_value(&block, r"\VarFileInfo\Translation")?;
    if (len as usize) < 2 * size_of::<u16>() {
        return None;
    }
    let (language, code_page) = unsafe { (*translation, *translation.add(1)) };
    let (text, len) = version_value(&block, &format!(r"\StringFileInfo\{language:04x}{code_page:04x}\FileVersion"))?;
    parse_file_version(&String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(text, len as usize) }))
}

/// A value inside a version block: where it starts and the length `VerQueryValueW` gives, which
/// counts bytes for binary values and characters for strings.
fn version_value(block: &[u32], name: &str) -> Option<(*const u16, u32)> {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let (mut value, mut len) = (core::ptr::null_mut(), 0);
    let found = unsafe { VerQueryValueW(block.as_ptr().cast(), name.as_ptr(), &raw mut value, &raw mut len) };
    (found != 0 && !value.is_null()).then_some((value.cast_const().cast(), len))
}

/// `1.5.97.0`, with anything after a space ignored (`10.0.26100.1 (WinBuild.160101.0800)`).
fn parse_file_version(text: &str) -> Option<[u32; 4]> {
    let mut parts = text.trim_end_matches('\0').split_whitespace().next()?.split('.').map(|part| part.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

pub struct Runtime {
    pub file_version: [u16; 4],
    pub packed: u32,
    pub name: &'static str,
    pub versionlib: &'static str,
    /// The Address Library file is format 5 (1.7.99 and later), read from the files themselves.
    pub v5: bool,
}

pub const RUNTIMES: &[Runtime] = &[
    Runtime { file_version: [1, 5, 97, 0], packed: pack(1, 5, 97, 0), name: "SE 1.5.97", versionlib: "version-1-5-97-0.bin", v5: false },
    Runtime { file_version: [1, 6, 353, 0], packed: pack(1, 6, 353, 0), name: "AE 1.6.353", versionlib: "versionlib-1-6-353-0.bin", v5: false },
    Runtime { file_version: [1, 6, 640, 0], packed: pack(1, 6, 640, 0), name: "AE 1.6.640", versionlib: "versionlib-1-6-640-0.bin", v5: false },
    Runtime { file_version: [1, 6, 1170, 0], packed: pack(1, 6, 1170, 0), name: "AE 1.6.1170", versionlib: "versionlib-1-6-1170-0.bin", v5: false },
    Runtime { file_version: [1, 6, 1179, 0], packed: pack(1, 6, 1179, 1), name: "AE 1.6.1179 GOG", versionlib: "versionlib-1-6-1179-0.bin", v5: false },
    Runtime { file_version: [1, 7, 99, 0], packed: pack(1, 7, 99, 0), name: "AE 1.7.99", versionlib: "versionlib-1-7-99-0.bin", v5: true },
    Runtime { file_version: [1, 7, 104, 0], packed: pack(1, 7, 104, 0), name: "AE 1.7.104", versionlib: "versionlib-1-7-104-0.bin", v5: true },
];

#[must_use]
pub fn find(packed: u32) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.packed == packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_matches_the_loader_log() {
        assert_eq!(pack(1, 5, 97, 0), 0x0105_0610);
        assert_eq!(pack(1, 7, 104, 0), 0x0107_0680);
        assert_eq!(unpack(0x0106_4920), [1, 6, 1170, 0]);
        assert_eq!(pack(1, 6, 640, 0), 0x0106_2800);
        assert_eq!(pack(1, 6, 1179, 1), 0x0106_49B1);
        assert!(is_gog(0x0106_49B1) && !is_gog(0x0106_4920));
    }

    #[test]
    fn table_is_consistent() {
        for r in RUNTIMES {
            let [a, b, c, _] = r.file_version;
            assert_eq!(unpack(r.packed)[..3], [u32::from(a), u32::from(b), u32::from(c)]);
            assert_eq!(versionlib_name(r.packed), r.versionlib);
            assert_eq!(r.v5, r.packed >= pack(1, 7, 99, 0));
        }
        assert!(is_se(pack(1, 5, 97, 0)) && !is_se(pack(1, 6, 1170, 0)));
    }

    #[test]
    fn canonical_ignores_the_store() {
        assert!(is_canonical(pack(1, 7, 104, 0)) && is_canonical(pack(1, 7, 104, 8)));
        assert!(!is_canonical(pack(1, 7, 99, 0)) && !is_canonical(pack(1, 6, 1170, 0)));
    }

    #[test]
    fn file_version_string_parses() {
        assert_eq!(parse_file_version("1.5.97.0\0"), Some([1, 5, 97, 0]));
        assert_eq!(parse_file_version("10.0.26100.1 (WinBuild.160101.0800)"), Some([10, 0, 26100, 1]));
        assert_eq!(parse_file_version("1.5.97"), None);
        assert_eq!(parse_file_version("1.5.97.0.1"), None);
        assert_eq!(parse_file_version(""), None);
    }

    #[test]
    fn file_version_reads_a_real_resource() {
        let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap();
        let [major, ..] = file_version(&system.join("System32").join("kernel32.dll")).unwrap();
        assert!(major >= 6);
    }

    #[test]
    fn a_binary_without_a_version_resource_has_no_version() {
        assert_eq!(running(), None);
    }
}
