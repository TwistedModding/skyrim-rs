# xtask

`cargo xtask gen` regenerates `crates/sre/src/generated/`, `crates/sre/Cargo.toml` and `crates/sre/generated.lock` from the SkyrimRE-1.7.104 repository, found through `SKYRIMRE_DIR` or as `../SkyrimRE-1.7.104` next to this workspace. It writes a file only when its content changed.

`cargo test -p xtask` regenerates in memory from the commit recorded in `generated.lock` and fails when the files on disk differ; it skips when the RE repository is absent.

`cargo crash-decode <file.dmp> [out.log]` writes the crash log of a minidump, to stdout without `out.log`.
