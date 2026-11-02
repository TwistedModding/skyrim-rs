//! `skyrim_runtime_extender.dll`, the framework's host: an SKSE plugin that installs the overlay and loads the
//! framework plugins in `Data\SKSE\Plugins\skyrim-runtime-extender\` (see the `framework` crate).

skse::plugin! {
    name: "skyrim-runtime-extender",
    author: "TwistedModding",
    version: 1,
    load: framework::host::load,
}
