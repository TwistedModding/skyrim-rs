//! Generates `crates/sre` from the SkyrimRE-1.7.104 ledgers, and the runtime profiles the host ships.

mod api;
mod emit;
mod groups;
mod inputs;
mod versions;
pub mod profile;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use emit::Stats;

pub const RE_REPO: &str = "SkyrimRE-1.7.104";
/// The generated crate's package name and the library name authors write in paths.
pub const PACKAGE: &str = "skyrim-sre";
pub const LIB: &str = "sre";
pub const CRATE_DIR: &str = "crates/sre";
/// Everything under it is generated; `cargo xtask gen` replaces the folder.
pub const MODULE_DIR: &str = "crates/sre/src/generated";
pub const LOCK: &str = "crates/sre/generated.lock";
/// Everything under it is generated too; the package ships it next to the host.
pub const PROFILE_DIR: &str = "crates/host/profiles";

#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// The RE repository: `SKYRIMRE_DIR`, or the checkout next to this workspace.
#[must_use]
pub fn re_dir() -> PathBuf {
    std::env::var_os("SKYRIMRE_DIR").map_or_else(|| workspace_root().parent().unwrap().join(RE_REPO), PathBuf::from)
}

/// `HEAD` of the RE repository, `unknown` without git.
#[must_use]
pub fn commit_of(re_dir: &Path) -> String {
    Command::new("git")
        .args(["-C", &re_dir.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(|| "unknown".to_owned(), |o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Generated files keyed by path relative to the workspace root; `commit` is recorded as the inputs' provenance.
pub fn generate(re_dir: &Path, commit: &str) -> Result<(BTreeMap<String, String>, Stats), Box<dyn Error>> {
    let inputs = inputs::read(re_dir)?;
    let (runtimes, hashes) = versions::read(re_dir)?;
    let variance = versions::Variance::of(&inputs, &runtimes);
    let (modules, mut stats) = emit::all(&inputs, &variance, commit);
    let mut files: BTreeMap<String, String> =
        modules.into_iter().map(|(name, text)| (format!("{CRATE_DIR}/{name}"), text)).collect();
    files.extend(versions::profiles(&inputs, &runtimes));
    stats.self_check_differences = runtimes.iter().find(|r| r.name == versions::CANONICAL).map(versions::Runtime::differences);
    let mut lock = format!("# Inputs of {MODULE_DIR}, recorded by `cargo xtask gen`.\nrepo {RE_REPO}\ncommit {commit}\n");
    for (path, hash) in inputs.hashes.iter().chain(&hashes) {
        writeln!(lock, "sha256 {hash} {path}").unwrap();
    }
    files.insert(LOCK.to_owned(), lock);
    Ok((files, stats))
}

/// Every file under the generated folders, relative to the workspace root with `/` separators.
#[must_use]
pub fn generated_on_disk(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.join(MODULE_DIR), root.join(PROFILE_DIR)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                found.push(path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found
}

/// The crash log of a minidump the crash logger wrote: the record from its user stream (or what
/// the dump's own streams give) formatted by the logger's own `report`, with DLL symbols from
/// `DbgHelp` where their PDBs are found.
#[cfg(feature = "decode")]
pub fn decode(dump: &Path) -> Result<String, Box<dyn Error>> {
    let dump = crash::dump::Dump::open(dump)?;
    let (rec, note) = match dump.record() {
        Some(rec) => (rec, "decoded offline from the logger's record in the dump"),
        None => (dump.reconstruct(), "decoded offline from the dump's own streams: no logger record, so the frames are a scan of the stack for return addresses and the loader facts are missing"),
    };
    let pdb = crash::pdb::DbgHelp::new(0x4D50 as _, None);
    let mut text = format!("({note})\n");
    crash::report::write(&mut text, &rec, &dump, None, Some(&pdb));
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_output_is_current() {
        let re = re_dir();
        if !re.join("export/symbols.csv").exists() {
            eprintln!("skipped: {} not present (set SKYRIMRE_DIR)", re.display());
            return;
        }
        let root = workspace_root();
        let lock = std::fs::read_to_string(root.join(LOCK)).unwrap_or_default();
        let commit = lock.lines().find_map(|l| l.strip_prefix("commit ")).map_or_else(|| commit_of(&re), str::to_owned);
        let (files, _) = generate(&re, &commit).unwrap();
        for (path, text) in &files {
            let on_disk = std::fs::read_to_string(root.join(path)).unwrap_or_default();
            assert!(on_disk == *text, "{path} is stale; run `cargo xtask gen`");
        }
        for relative in generated_on_disk(&root) {
            assert!(files.contains_key(&relative), "{relative} is not generated any more; run `cargo xtask gen`");
        }
    }
}
