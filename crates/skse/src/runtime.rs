//! Known game runtimes and the Address Library file that goes with each.
//!
//! The packed version is the number the SKSE loader prints after the version in its log
//! (`version = 2.3.1 01070680` for 1.7.104.0, `01050610` for 1.5.97.0): major in the top byte,
//! minor in the next, build in the next twelve bits, the store in the low nibble (0 on Steam, the
//! only store seen). The table lists the runtimes an Address Library file or a game install on
//! this machine proved; a runtime missing here is unknown, not unsupported.

#[must_use]
pub const fn pack(major: u32, minor: u32, build: u32, store: u32) -> u32 {
    (major & 0xFF) << 24 | (minor & 0xFF) << 16 | (build & 0xFFF) << 4 | (store & 0xF)
}

#[must_use]
pub const fn unpack(packed: u32) -> [u32; 4] {
    [packed >> 24, (packed >> 16) & 0xFF, (packed >> 4) & 0xFFF, packed & 0xF]
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
    Runtime { file_version: [1, 6, 1170, 0], packed: pack(1, 6, 1170, 0), name: "AE 1.6.1170", versionlib: "versionlib-1-6-1170-0.bin", v5: false },
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
}
