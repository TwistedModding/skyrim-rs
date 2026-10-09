//! Known game runtimes with the SKSE build and Address Library file that go with each.

pub const STORE_STEAM: u32 = 0;
pub const STORE_GOG: u32 = 1;
pub const STORE_EPIC: u32 = 2;

#[must_use]
pub const fn pack(major: u32, minor: u32, build: u32, store: u32) -> u32 {
    (major & 0xFF) << 24 | (minor & 0xFF) << 16 | (build & 0xFFF) << 4 | (store & 0xF)
}

#[must_use]
pub const fn unpack(packed: u32) -> [u32; 4] {
    [packed >> 24, (packed >> 16) & 0xFF, (packed >> 4) & 0xFFF, packed & 0xF]
}

#[must_use]
pub const fn is_se(packed: u32) -> bool {
    packed >> 16 <= 0x0105
}

#[must_use]
pub const fn is_vr(packed: u32) -> bool {
    packed >> 16 == 0x0104
}

/// Address Library file name for a runtime; SE and VR use the old prefix, AE the new one.
#[must_use]
pub fn versionlib_name(packed: u32) -> String {
    let [major, minor, build, _] = unpack(packed);
    let prefix = if is_se(packed) { "version" } else { "versionlib" };
    format!("{prefix}-{major}-{minor}-{build}-0.bin")
}

pub struct Runtime {
    pub file_version: [u16; 4],
    pub packed: u32,
    pub name: &'static str,
    /// Last SKSE release built for this runtime. The 1.6.659 GOG entry is inferred, not verified.
    pub skse: &'static str,
    pub versionlib: &'static str,
    pub v5: bool,
}

pub const RUNTIMES: &[Runtime] = &[
    Runtime {
        file_version: [1, 4, 15, 0],
        packed: pack(1, 4, 15, STORE_STEAM),
        name: "VR 1.4.15",
        skse: "SKSEVR 2.0.12",
        versionlib: "version-1-4-15-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 5, 97, 0],
        packed: pack(1, 5, 97, STORE_STEAM),
        name: "SE 1.5.97",
        skse: "2.0.20",
        versionlib: "version-1-5-97-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 6, 640, 0],
        packed: pack(1, 6, 640, STORE_STEAM),
        name: "AE 1.6.640",
        skse: "2.2.3",
        versionlib: "versionlib-1-6-640-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 6, 659, 0],
        packed: pack(1, 6, 659, STORE_GOG),
        name: "AE 1.6.659 GOG",
        skse: "2.2.3",
        versionlib: "versionlib-1-6-659-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 6, 1130, 0],
        packed: pack(1, 6, 1130, STORE_STEAM),
        name: "AE 1.6.1130",
        skse: "2.2.5",
        versionlib: "versionlib-1-6-1130-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 6, 1170, 0],
        packed: pack(1, 6, 1170, STORE_STEAM),
        name: "AE 1.6.1170",
        skse: "2.2.8",
        versionlib: "versionlib-1-6-1170-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 6, 1179, 0],
        packed: pack(1, 6, 1179, STORE_GOG),
        name: "AE 1.6.1179 GOG",
        skse: "2.2.6 GOG",
        versionlib: "versionlib-1-6-1179-0.bin",
        v5: false,
    },
    Runtime {
        file_version: [1, 7, 99, 0],
        packed: pack(1, 7, 99, STORE_STEAM),
        name: "AE 1.7.99",
        skse: "2.3.0",
        versionlib: "versionlib-1-7-99-0.bin",
        v5: true,
    },
    Runtime {
        file_version: [1, 7, 104, 0],
        packed: pack(1, 7, 104, STORE_STEAM),
        name: "AE 1.7.104",
        skse: "2.3.1",
        versionlib: "versionlib-1-7-104-0.bin",
        v5: true,
    },
];

#[must_use]
pub fn find(packed: u32) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.packed == packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_matches_skse() {
        assert_eq!(pack(1, 5, 97, 0), 0x0105_0610);
        assert_eq!(pack(1, 6, 1170, 0), 0x0106_4920);
        assert_eq!(pack(1, 6, 1179, STORE_GOG), 0x0106_49B1);
        assert_eq!(pack(1, 7, 104, 0), 0x0107_0680);
        assert_eq!(unpack(0x0106_49B1), [1, 6, 1179, 1]);
    }

    #[test]
    fn table_is_consistent() {
        for r in RUNTIMES {
            let [a, b, c, _] = r.file_version;
            assert_eq!(unpack(r.packed)[..3], [u32::from(a), u32::from(b), u32::from(c)]);
            assert_eq!(versionlib_name(r.packed), r.versionlib);
            assert_eq!(r.v5, r.packed >= pack(1, 7, 99, 0));
        }
        assert!(is_se(pack(1, 5, 97, 0)) && is_vr(pack(1, 4, 15, 0)) && !is_se(pack(1, 6, 640, 0)));
    }
}
