# skyrim-rs

A Rust library and plugin loader for Skyrim Special Edition 1.7.104 mods. `skyrim_rs.dll` is an SKSE plugin that loads Rust plugins from `Data\SKSE\Plugins\skyrim-rs\`, with an in-game menu (F11), settings, hotkeys, hook dispatch, hot reload and a crash logger. Plugins talk to the loader through a stable C ABI, so a plugin runs on any loader version that has the functions it uses.

## Build and package

```
cargo build --release
powershell -File scripts\package.ps1
```

The package script writes `dist\skyrim-rs-<version>.zip` (the loader) and `dist\skyrim-rs-example-<version>.zip` (the example mod). Install by copying a package's `Data` folder over the game's.

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
