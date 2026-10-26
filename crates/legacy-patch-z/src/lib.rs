//! The compat suite's plain SKSE plugin z (`legacy_patch::load_z`).

skse::plugin! {
    name: "skyrim-rs-legacy-patch-z",
    author: "TwistedModding",
    version: 1,
    load: legacy_patch::load_z,
}
