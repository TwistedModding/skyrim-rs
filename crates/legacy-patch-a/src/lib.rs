//! The compat suite's plain SKSE plugin a (`legacy_patch::load_a`).

skse::plugin! {
    name: "skyrim-rs-legacy-patch-a",
    author: "TwistedModding",
    version: 1,
    load: legacy_patch::load_a,
}
