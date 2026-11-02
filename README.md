# Skyrim Runtime Extender

A Rust library and plugin loader for Skyrim Special Edition 1.7.104 mods. Its crates, DLL, folders and console command are named `skyrim-rs`, `skyrim_rs` and `SkyrimRs`. `skyrim_rs.dll` is an SKSE plugin that loads Rust plugins from `Data\SKSE\Plugins\skyrim-rs\`, with an in-game menu (F11), settings, hotkeys, hook dispatch, hot reload and a crash logger. Plugins talk to the loader through a stable C ABI, so a plugin runs on any loader version that has the functions it uses.

## Build and package

```
cargo build --release
powershell -File scripts\package.ps1
```

The package script writes the loader to `dist\skyrim-runtime-extender-<version>.zip`. The example mod is not packaged. The version is `<major.minor>.<commit count>`, with `-dirty` when the tree has uncommitted changes; the loader reports it in its log and on its Plugins tab. Major.minor is set at the top of the script.

To package every commit and merge on `master` in the background, with the output in `dist\package.log`, enable the hooks once per clone:

```
git config core.hooksPath .githooks
``` Install a package with a mod manager, or extract it into the game's `Data` folder.

## Writing a mod

A mod is its own repository with skyrim-rs as a submodule:

```
git submodule add https://github.com/TwistedModding/skyrim-rs.git extern/skyrim-rs
```

Workspace `Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["my-mod"]
exclude = ["extern"]

[workspace.dependencies]
skse = { path = "extern/skyrim-rs/crates/skse" }
sre = { path = "extern/skyrim-rs/crates/sre", package = "skyrim-sre" }
skyrim = { path = "extern/skyrim-rs/crates/skyrim" }
framework = { path = "extern/skyrim-rs/crates/framework" }
```

The mod crate is a `cdylib` (`crate-type = ["cdylib"]`) that declares itself:

```rust
framework::plugin! {
    name: "my-mod",
    load: load,
}

fn load(host: &'static framework::Host) -> bool {
    skyrim::init(host.skse()).is_ok()
}
```

Copy `.cargo/config.toml` and `rust-toolchain.toml` from this repository. `crates/example-mod` is a complete example: a hook, a setting, a Papyrus native and a menu tab. Its DLL goes in `Data\SKSE\Plugins\skyrim-rs\`.
