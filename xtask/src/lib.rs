//! Generates `crates/layout` from the SkyrimRE-1.7.104 ledgers.

mod emit;
mod inputs;

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use emit::Stats;

pub const RE_REPO: &str = "SkyrimRE-1.7.104";
pub const MODULE_DIR: &str = "crates/layout/src/v1_7_104";
pub const LOCK: &str = "crates/layout/generated.lock";

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
    let (modules, stats) = emit::all(&inputs, commit);
    let mut files: BTreeMap<String, String> =
        modules.into_iter().map(|(name, text)| (format!("{MODULE_DIR}/{name}"), text)).collect();
    let mut lock = format!("# Inputs of {MODULE_DIR}, recorded by `cargo xtask gen`.\nrepo {RE_REPO}\ncommit {commit}\n");
    for (path, hash) in &inputs.hashes {
        writeln!(lock, "sha256 {hash} {path}").unwrap();
    }
    files.insert(LOCK.to_owned(), lock);
    Ok((files, stats))
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
        for (path, text) in files {
            let on_disk = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
            assert!(on_disk == text, "{path} is stale; run `cargo xtask gen`");
        }
    }
}
