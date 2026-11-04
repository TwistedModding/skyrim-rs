# xtask

`cargo xtask gen` regenerates `crates/sre/src/generated/`, `crates/sre/Cargo.toml`, `crates/sre/generated.lock` and `crates/host/profiles/` from the SkyrimRE-1.7.104 repository, found through `SKYRIMRE_DIR` or as `../SkyrimRE-1.7.104` next to this workspace. It writes a file only when its content changed.

The difference files `export/versions/<runtime>.json` of 1.5.97, 1.6.353 and 1.6.1170 decide what stays plain: a member, slot or base not identical on every file present becomes an accessor reading the runtime's profile, so the split is provisional until all three exist. A class size has no generated code: `sre::class_size!` reads it from the profile for the code that needs it. Generation works with any of them missing; each one present gets a profile, and 1.7.104's is always the header alone. `docs/subsystems/runtimes.md` is the contract.

`cargo xtask manifest <dll>` prints what a DLL records it uses of the game, read from its `.sremf` section; `--check <profile>` prints what that profile lacks of it and exits non-zero when it lacks anything. An id with no row on an AE profile is left to the Address Library file.

`cargo test -p xtask` regenerates in memory from the commit recorded in `generated.lock` and fails when the files on disk differ; it skips when the RE repository is absent.

`cargo crash-decode <file.dmp> [out.log]` writes the crash log of a minidump, to stdout without `out.log`.
