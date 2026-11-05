//! The script extender builds this library runs under and what each asks of a plugin, read from the
//! DLLs in the installs and checked by crafted-plugin runs (`docs/subsystems/skse-loaders.md`).

use core::ffi::{CStr, c_char};
use core::ops::RangeInclusive;

use crate::runtime::pack;
use crate::{PluginInfo, PluginVersionData, compat};

/// SKSE 2.3.1's link-timestamp gate: an Address Library plugin linked in this window needs
/// [`compat::FLAG_NO_TIMESTAMP_GATE`] or the runtime in its compatible list.
pub const TIMESTAMP_GATE: RangeInclusive<u32> = 520_128_000..=1_748_217_599;

const BAD_VERSION_DATA: &str = "disabled, bad version data";
const NO_NAME: &str = "disabled, no name specified";
const BAD_METHOD: &str = "disabled, unsupported version independence method";
const NO_ADDRESS_LIBRARY: &str = "disabled, address library needs to be updated";
const RECOMPILE: &str = "must be recompiled for new address library";
const OLD_STRUCTS: &str = "disabled, only compatible with versions earlier than 1.6.629";
const NEWER_LOADER: &str = "disabled, requires newer script extender";

pub struct Loader {
    /// The one runtime the build accepts.
    pub runtime: u32,
    /// Packed, `Interface` +0.
    pub version: u32,
    /// `GetReleaseIndex`.
    pub release_index: u32,
}

const V2_1: u32 = pack(2, 1, 0, 0);
const V2_2: u32 = pack(2, 2, 0, 0);
const V2_3: u32 = pack(2, 3, 0, 0);

pub const LOADERS: [Loader; 4] = [
    Loader { runtime: pack(1, 5, 97, 0), version: pack(2, 0, 19, 0), release_index: 65 },
    Loader { runtime: pack(1, 6, 353, 0), version: pack(2, 1, 5, 0), release_index: 66 },
    Loader { runtime: pack(1, 6, 1170, 0), version: pack(2, 2, 6, 0), release_index: 72 },
    Loader { runtime: pack(1, 7, 104, 0), version: pack(2, 3, 1, 0), release_index: 75 },
];

#[must_use]
pub fn for_runtime(runtime: u32) -> Option<&'static Loader> {
    LOADERS.iter().find(|l| l.runtime == runtime)
}

/// Every loader's runtime as a zero-terminated compatible list.
#[must_use]
pub const fn compatible_runtimes() -> [u32; 16] {
    let mut list = [0; 16];
    let mut i = 0;
    while i < LOADERS.len() {
        list[i] = LOADERS[i].runtime;
        i += 1;
    }
    list
}

impl Loader {
    /// Loads every DLL and calls `SKSEPlugin_Query` then `SKSEPlugin_Load`, both required, and reads
    /// no version data. Otherwise the version data decides and `SKSEPlugin_Load` runs in a second pass.
    #[must_use]
    pub const fn calls_query(&self) -> bool {
        self.version < V2_1
    }

    /// Calls an optional `SKSEPlugin_Preload` before any `SKSEPlugin_Load`.
    #[must_use]
    pub const fn calls_preload(&self) -> bool {
        self.version >= V2_3
    }

    /// Refuses a DLL with no `SKSEPlugin_Load`, which 2.3.1 keeps.
    #[must_use]
    pub const fn requires_load(&self) -> bool {
        self.version < V2_3
    }

    /// The build's checks on the version data in its order, each refusal in its log's wording.
    /// `versionlib` says whether the runtime's Address Library file opens; `timestamp` is the DLL's
    /// PE `TimeDateStamp`.
    pub fn accepts(&self, data: &PluginVersionData, versionlib: impl FnOnce() -> bool, timestamp: u32) -> Result<(), &'static str> {
        if data.data_version == 0 || (self.version < V2_2 && data.data_version != 1) {
            return Err(BAD_VERSION_DATA);
        }
        if data.name[0] == 0 {
            return Err(NO_NAME);
        }
        let known_methods = if self.version < V2_2 { 3 } else { 7 };
        if data.method & !known_methods != 0 {
            return Err(BAD_METHOD);
        }
        let listed = data.compatible_versions.iter().take_while(|&&v| v != 0).any(|&v| v == self.runtime);
        if data.method & compat::METHOD_ADDRESS_LIBRARY != 0 && !versionlib() {
            return Err(NO_ADDRESS_LIBRARY);
        }
        let gated = self.version >= V2_3 && data.method & compat::METHOD_ADDRESS_LIBRARY != 0 && data.flags & compat::FLAG_NO_TIMESTAMP_GATE == 0 && TIMESTAMP_GATE.contains(&timestamp);
        if gated {
            // A listed plugin caught by the gate skips the 1.6.629 test.
            if !listed {
                return Err(RECOMPILE);
            }
        } else if data.method & (compat::METHOD_ADDRESS_LIBRARY | compat::METHOD_NO_FILE_CHECK) != 0 {
            if self.version >= V2_2 && data.method & compat::METHOD_STRUCTS_1_6_629 == 0 && data.flags & compat::FLAG_STRUCTS_1_6_629 == 0 {
                return Err(OLD_STRUCTS);
            }
        } else if !listed {
            return Err(if self.version < V2_2 { "disabled, incompatible with current runtime version" } else { "disabled, incompatible with current version of the game" });
        }
        if data.minimum_skse > self.version {
            return Err(NEWER_LOADER);
        }
        Ok(())
    }

    /// 2.0.19's check on what `SKSEPlugin_Query` filled in, after it returned true.
    pub fn accepts_queried(info: &PluginInfo) -> Result<(), &'static str> {
        if info.name.is_null() {
            return Err(NO_NAME);
        }
        if unsafe { CStr::from_ptr(info.name.cast::<c_char>()) } == c"papyrusutil plugin" && info.version < 2 {
            return Err("broken version check, will crash the game");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::*;

    const GATED: u32 = 1_600_000_000;
    const FRESH: u32 = 1_800_000_000;

    fn data(flags: u32, method: u32) -> PluginVersionData {
        PluginVersionData::new("p", "a", 1, flags, method)
    }

    fn unlisted(flags: u32, method: u32) -> PluginVersionData {
        PluginVersionData { compatible_versions: [0; 16], ..data(flags, method) }
    }

    fn loader(version: u32) -> &'static Loader {
        LOADERS.iter().find(|l| l.version == version).expect("a known loader")
    }

    fn v2_1() -> &'static Loader {
        loader(pack(2, 1, 5, 0))
    }

    fn v2_2() -> &'static Loader {
        loader(pack(2, 2, 6, 0))
    }

    fn v2_3() -> &'static Loader {
        loader(pack(2, 3, 1, 0))
    }

    #[test]
    fn versions_match_the_interface_constants_in_the_dlls() {
        assert_eq!(LOADERS.map(|l| l.version), [0x0200_0130, 0x0201_0050, 0x0202_0060, 0x0203_0010]);
        assert_eq!(LOADERS.map(|l| l.runtime), [0x0105_0610, 0x0106_1610, 0x0106_4920, 0x0107_0680]);
    }

    #[test]
    fn the_defaults_pass_every_version_data_loader_at_any_timestamp() {
        let data = data(FLAGS_EVERY_LOADER, METHOD_EVERY_LOADER);
        for l in LOADERS.iter().filter(|l| !l.calls_query()) {
            for timestamp in [0, GATED, FRESH] {
                assert_eq!(l.accepts(&data, || true, timestamp), Ok(()), "loader {:08x} at {timestamp}", l.version);
            }
        }
    }

    #[test]
    fn method_bit_two_is_unknown_to_2_1_5() {
        let old_default = data(FLAG_NO_TIMESTAMP_GATE, METHOD_ADDRESS_LIBRARY | METHOD_STRUCTS_1_6_629);
        assert_eq!(v2_1().accepts(&old_default, || true, FRESH), Err(BAD_METHOD));
        assert_eq!(v2_2().accepts(&old_default, || true, FRESH), Ok(()));
        assert_eq!(v2_3().accepts(&old_default, || true, FRESH), Ok(()));
    }

    #[test]
    fn data_version_two_is_refused_only_by_2_1_5() {
        let two = PluginVersionData { data_version: 2, ..data(FLAGS_EVERY_LOADER, METHOD_EVERY_LOADER) };
        assert_eq!(v2_1().accepts(&two, || true, FRESH), Err(BAD_VERSION_DATA));
        assert_eq!(v2_2().accepts(&two, || true, FRESH), Ok(()));
        assert_eq!(v2_3().accepts(&two, || true, FRESH), Ok(()));
    }

    #[test]
    fn the_1_6_629_test_starts_with_2_2_6() {
        let plain = data(FLAG_NO_TIMESTAMP_GATE, METHOD_ADDRESS_LIBRARY);
        assert_eq!(v2_1().accepts(&plain, || true, FRESH), Ok(()));
        assert_eq!(v2_2().accepts(&plain, || true, FRESH), Err(OLD_STRUCTS));
        assert_eq!(v2_3().accepts(&plain, || true, FRESH), Err(OLD_STRUCTS));
    }

    #[test]
    fn the_timestamp_gate_is_2_3_1_only() {
        let bare = unlisted(FLAG_STRUCTS_1_6_629, METHOD_ADDRESS_LIBRARY);
        assert_eq!(v2_1().accepts(&bare, || true, GATED), Ok(()));
        assert_eq!(v2_2().accepts(&bare, || true, GATED), Ok(()));
        assert_eq!(v2_3().accepts(&bare, || true, GATED), Err(RECOMPILE));
        assert_eq!(v2_3().accepts(&bare, || true, FRESH), Ok(()));
        assert_eq!(v2_3().accepts(&bare, || true, 0), Ok(()));
        let flagged = unlisted(FLAGS_EVERY_LOADER, METHOD_ADDRESS_LIBRARY);
        assert_eq!(v2_3().accepts(&flagged, || true, GATED), Ok(()));
    }

    #[test]
    fn a_listed_plugin_caught_by_the_gate_skips_the_1_6_629_test() {
        let listed = data(0, METHOD_ADDRESS_LIBRARY);
        assert_eq!(v2_3().accepts(&listed, || true, GATED), Ok(()));
        assert_eq!(v2_3().accepts(&listed, || true, FRESH), Err(OLD_STRUCTS));
    }

    #[test]
    fn neither_method_bit_reads_the_list_in_each_build_s_wording() {
        assert_eq!(v2_1().accepts(&data(0, 0), || true, FRESH), Ok(()));
        assert_eq!(v2_2().accepts(&data(0, 0), || true, FRESH), Ok(()));
        assert_eq!(v2_3().accepts(&data(0, 0), || true, FRESH), Ok(()));
        assert_eq!(v2_1().accepts(&unlisted(0, 0), || true, FRESH), Err("disabled, incompatible with current runtime version"));
        assert_eq!(v2_2().accepts(&unlisted(0, 0), || true, FRESH), Err("disabled, incompatible with current version of the game"));
        assert_eq!(v2_3().accepts(&unlisted(0, 0), || true, FRESH), Err("disabled, incompatible with current version of the game"));
    }

    #[test]
    fn the_address_library_file_is_checked_only_for_method_bit_zero() {
        for l in [v2_1(), v2_2(), v2_3()] {
            assert_eq!(l.accepts(&data(FLAGS_EVERY_LOADER, METHOD_ADDRESS_LIBRARY), || false, FRESH), Err(NO_ADDRESS_LIBRARY));
            assert_eq!(l.accepts(&data(FLAGS_EVERY_LOADER, METHOD_NO_FILE_CHECK), || panic!("no file check"), FRESH), Ok(()));
        }
    }

    #[test]
    fn a_minimum_above_the_loader_is_refused_last() {
        for l in [v2_1(), v2_2(), v2_3()] {
            let at = PluginVersionData { minimum_skse: l.version, ..data(FLAGS_EVERY_LOADER, METHOD_EVERY_LOADER) };
            assert_eq!(l.accepts(&at, || true, FRESH), Ok(()));
            let above = PluginVersionData { minimum_skse: l.version + 1, ..data(FLAGS_EVERY_LOADER, METHOD_EVERY_LOADER) };
            assert_eq!(l.accepts(&above, || true, FRESH), Err(NEWER_LOADER));
        }
    }

    #[test]
    fn query_rules() {
        let info = |name: *const c_char, version| PluginInfo { info_version: 1, name, version };
        assert_eq!(Loader::accepts_queried(&info(core::ptr::null(), 1)), Err(NO_NAME));
        assert_eq!(Loader::accepts_queried(&info(c"p".as_ptr(), 1)), Ok(()));
        assert!(Loader::accepts_queried(&info(c"papyrusutil plugin".as_ptr(), 1)).is_err());
        assert_eq!(Loader::accepts_queried(&info(c"papyrusutil plugin".as_ptr(), 2)), Ok(()));
    }

    #[test]
    fn the_compatible_list_is_zero_terminated_and_names_every_runtime() {
        let list = compatible_runtimes();
        assert_eq!(list[..4], LOADERS.map(|l| l.runtime));
        assert!(list[4..].iter().all(|&v| v == 0));
    }
}
