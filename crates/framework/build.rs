//! Records the compiler's version and a hash of the sources behind the plugin ABI for `framework::BUILD`.

use std::path::Path;

/// The workspace crates whose types cross the host-plugin boundary, plus the lock that pins the rest.
const ABI_SOURCES: [&str; 8] = ["framework/src", "overlay/src", "skyrim/src", "hook/src", "skse/src", "skre/src", "versionlib/src", "../Cargo.lock"];

fn main() {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = std::process::Command::new(rustc).arg("--version").output().expect("rustc --version runs");
    println!("cargo:rustc-env=SKYRIM_RS_RUSTC={}", String::from_utf8_lossy(&version.stdout).trim());
    println!("cargo:rerun-if-env-changed=RUSTC");

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates folder");
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for source in ABI_SOURCES {
        let root = crates.join(source);
        println!("cargo:rerun-if-changed={}", root.display());
        let mut files = Vec::new();
        collect(&root, &mut files);
        files.sort();
        for file in files {
            let relative = file.strip_prefix(crates).expect("under the crates folder").to_string_lossy().replace('\\', "/");
            for byte in relative.bytes().chain(std::fs::read(&file).expect("source file readable")) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    println!("cargo:rustc-env=SKYRIM_RS_API_HASH={hash:016x}");
}

fn collect(path: &Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("source folder readable").flatten() {
            collect(&entry.path(), files);
        }
    } else if path.is_file() {
        files.push(path.to_path_buf());
    }
}
