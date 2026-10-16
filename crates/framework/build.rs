//! Records the compiler's version for `framework::BUILD`.

fn main() {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = std::process::Command::new(rustc).arg("--version").output().expect("rustc --version runs");
    println!("cargo:rustc-env=SKYRIM_RS_RUSTC={}", String::from_utf8_lossy(&version.stdout).trim());
    println!("cargo:rerun-if-env-changed=RUSTC");
}
