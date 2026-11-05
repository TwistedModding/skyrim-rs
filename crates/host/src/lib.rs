//! `skyrim_runtime_extender.dll`, the framework's host: an SKSE plugin that installs the overlay and loads the
//! framework plugins in `Data\SKSE\Plugins\skyrim-runtime-extender\` (see the `framework` crate).

skse::plugin! {
    name: "skyrim-runtime-extender",
    author: "TwistedModding",
    version: 1,
    load: framework::host::load,
}

/// Called by SKSE 2.3.1 before any plugin's Load; the other loaders never call it.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn SKSEPlugin_Preload(skse: &skse::Interface) -> bool {
    framework::host::preload(skse)
}
