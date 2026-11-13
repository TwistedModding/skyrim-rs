#!/usr/bin/env bash
# macOS counterpart of package.ps1: cross-builds release and writes dist/skyrim-runtime-extender-<version>.zip.
# Skips notices.ps1 -Check (PowerShell only); run it on Windows before publishing.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

RELEASE="$(sed -n 's/^\$Release = "\(.*\)"$/\1/p' scripts/package.ps1)"
since="$(git log -1 --format=%H -G '^\$Release = ' -- scripts/package.ps1)"
[ -n "$since" ] || { echo "git log failed" >&2; exit 1; }
version="$RELEASE.$(git rev-list --count "$since..HEAD")"
[ -z "$(git status --porcelain)" ] || version="$version-dirty"
export SKYRIM_RS_VERSION="$version"

scripts/build-macos.sh --release -p host -p timing -p standalone
R="target/x86_64-pc-windows-msvc/release"

STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
put() { mkdir -p "$STAGE/$(dirname "$1")"; cp "$2" "$STAGE/$1"; }
put "SKSE/Plugins/skyrim_runtime_extender.dll" "$R/skyrim_runtime_extender.dll"
put "SKSE/Plugins/!!!skyrim-runtime-extender-timing.dll" "$R/skyrim_runtime_extender_timing.dll"
put "optional-standalone/dinput8.dll" "$R/dinput8.dll"
put "optional-standalone/README.md" crates/standalone/README.md
put "README.md" crates/host/README.md
put "LICENSE.txt" LICENSE
put "THIRD-PARTY-NOTICES.txt" THIRD-PARTY-NOTICES.txt
# Keep in sync with $ShippedProfiles in package.ps1.
for p in $(sed -n 's/^\$ShippedProfiles = @(\(.*\))$/\1/p' scripts/package.ps1 | tr ',' ' ' | tr -d '"'); do
  put "SKSE/Plugins/skyrim-runtime-extender/profiles/$p.profile" "crates/host/profiles/$p.profile"
done

mkdir -p dist
out="$ROOT/dist/skyrim-runtime-extender-$version.zip"
rm -f "$out"
(cd "$STAGE" && zip -q -r -X "$out" .)
echo "$out"
