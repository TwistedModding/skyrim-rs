//! Smallest complete plugin: declares itself and writes one log line.

use std::io::Write;

skse::plugin! {
    name: "skyrim-rs-sample",
    author: "TwistedModding",
    version: 1,
    load: load,
    independence: skse::independence::ADDRESS_LIBRARY_POST_AE,
    independence_ex: skse::independence::EX_ADDRESS_LIBRARY_V5 | skse::independence::EX_NO_STRUCT_USE,
}

fn load(skse: &skse::Interface) -> bool {
    let Ok(mut log) = skse::log::open(skse.runtime_version, "skyrim-rs-sample") else {
        return false;
    };
    let [major, minor, build, store] = skse::runtime::unpack(skse.runtime_version);
    let _ = writeln!(
        log,
        "runtime {major}.{minor}.{build} store {store}, skse {:08x}",
        skse.skse_version
    );
    true
}
