#!/usr/bin/env bash
# Cross-compile skyrim-rs for x86_64-pc-windows-msvc on macOS (Apple Silicon).
# Needs: rustup (brew), LLVM (clang-cl, llvm-lib), and an xwin-style sysroot ($WINSYSROOT) with crt/ and sdk/.
#   scripts/build-macos.sh [cargo build args...]     default: --release (whole workspace)
set -euo pipefail

WINSYSROOT="${WINSYSROOT:-/Volumes/S512/Windows/Build-Tools/winsdk}"
LLVM="${LLVM:-/opt/homebrew/opt/llvm/bin}"
RUSTUP_BIN="${RUSTUP_BIN:-/opt/homebrew/opt/rustup/bin}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

[ -d "$WINSYSROOT/crt/include" ] || { echo "WINSYSROOT '$WINSYSROOT' has no crt/include" >&2; exit 1; }
[ -x "$LLVM/clang-cl" ] || { echo "clang-cl not found in $LLVM" >&2; exit 1; }

export PATH="$RUSTUP_BIN:$LLVM:$PATH"
export CARGO_BUILD_TARGET=x86_64-pc-windows-msvc
# cc-rs (minhook builds C): clang-cl with the sysroot headers.
export CC_x86_64_pc_windows_msvc="$LLVM/clang-cl"
export CXX_x86_64_pc_windows_msvc="$LLVM/clang-cl"
export AR_x86_64_pc_windows_msvc="$LLVM/llvm-lib"
export CFLAGS_x86_64_pc_windows_msvc="--target=x86_64-pc-windows-msvc -Wno-unused-command-line-argument -imsvc $WINSYSROOT/crt/include -imsvc $WINSYSROOT/sdk/include/ucrt -imsvc $WINSYSROOT/sdk/include/shared -imsvc $WINSYSROOT/sdk/include/um"
# rust-lld (lld-link flavour) reads LIB for import libraries and the CRT.
export LIB="$WINSYSROOT/crt/lib/x86_64;$WINSYSROOT/sdk/lib/ucrt/x86_64;$WINSYSROOT/sdk/lib/um/x86_64"

cd "$ROOT"
[ $# -gt 0 ] || set -- --release
exec cargo build "$@"
