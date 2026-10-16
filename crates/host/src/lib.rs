//! `skyrim_rs.dll`, the framework's host: an SKSE plugin that installs the overlay and loads the
//! framework plugins in `Data\SKSE\Plugins\skyrim-rs\` (see the `framework` crate).

skse::plugin! {
    name: "skyrim-rs",
    author: "TwistedModding",
    version: 1,
    load: framework::host::load,
}
