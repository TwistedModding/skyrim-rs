# xtask

`cargo xtask gen` regenerates `crates/sre/src/generated/`, `crates/sre/Cargo.toml`, `crates/sre/generated.lock` and `crates/host/profiles/` from the SkyrimRE-1.7.104 repository, found through `SKYRIMRE_DIR` or as `../SkyrimRE-1.7.104` next to this workspace. It writes a file only when its content changed.

The difference files `export/versions/<runtime>.json` of 1.5.97, 1.6.353 and 1.6.1170 decide what stays plain: a member, slot or base not identical on every file present becomes an accessor reading the runtime's profile, so the split is provisional until all three exist. A class size has no generated code: `sre::class_size!` reads it from the profile for the code that needs it. Generation works with any of them missing; each one present gets a profile, and 1.7.104's is always the header alone. `docs/subsystems/runtimes.md` is the contract.

`xtask/site-patches.txt` holds managed patches' bytes per runtime with how each was established, for what the difference files do not give: the generator writes them into the profiles' `site` rows and fails on a row that contradicts a compared one. A patch's replacement goes there, never in the plugin.

`crates/sre/src/generated/ledger_tables.rs` holds every member, base, class and slot the ledger names. `sre::member_offset!` and its kin look a name up there at compile time, so a name outside the ledger does not compile and the canonical offset is never typed by hand.

`cargo xtask manifest <dll>` prints what a DLL records it uses of the game, read from its `.sremf` section; `--check <profile>` prints what that profile lacks of it and exits non-zero when it lacks anything. An id with no row on an AE profile is left to the Address Library file, and a difference file's `id_map` writes a row for each id that file holds under another number; `--library <bin>` after `--check` checks them too, as the host does at load.

`cargo test -p xtask` regenerates in memory from the commit recorded in `generated.lock` and fails when the files on disk differ; it skips when the RE repository is absent.

`cargo crash-decode <file.dmp> [out.log]` writes the crash log of a minidump, to stdout without `out.log`.
