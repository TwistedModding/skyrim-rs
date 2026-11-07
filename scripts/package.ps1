# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# Builds release and writes the loader's package dist\skyrim-runtime-extender-<version>.zip: its files at their paths under Data and its README.
# The version is <major.minor>.<commits since the commit that set major.minor>, with -dirty for a tree with uncommitted changes.
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
$Release = "1.0"
$since = git -C $Root log -1 --format=%H -G '^\$Release = ' -- scripts/package.ps1
$commits = git -C $Root rev-list --count "$since..HEAD"
if ($LASTEXITCODE -ne 0 -or -not $since) { throw "git log failed" }
$version = "$Release.$commits"
if (git -C $Root status --porcelain) { $version += "-dirty" }
$env:SKYRIM_RS_VERSION = $version
& "$PSScriptRoot\notices.ps1" -Check
cargo build --release --manifest-path "$Root\Cargo.toml" -p host -p timing -p standalone
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$release = "$Root\target\release"
$entries = [ordered]@{
    "SKSE/Plugins/skyrim_runtime_extender.dll" = "$release\skyrim_runtime_extender.dll"
    "SKSE/Plugins/!!!skyrim-runtime-extender-timing.dll" = "$release\skyrim_runtime_extender_timing.dll"
    "optional-standalone/dinput8.dll" = "$release\dinput8.dll"
    "optional-standalone/README.md" = "$Root\crates\standalone\README.md"
    "README.md" = "$Root\crates\host\README.md"
    "LICENSE.txt" = "$Root\LICENSE"
    "THIRD-PARTY-NOTICES.txt" = "$Root\THIRD-PARTY-NOTICES.txt"
}
# The only profiles a release carries. A runtime goes on this list once its suites have passed there; verify.ps1 installs from crates\host\profiles whatever this says.
$ShippedProfiles = @("1-7-104", "1-6-1170", "1-6-353", "1-5-97")
foreach ($name in $ShippedProfiles) { $entries["SKSE/Plugins/skyrim-runtime-extender/profiles/$name.profile"] = "$Root\crates\host\profiles\$name.profile" }
New-Item -ItemType Directory -Force "$Root\dist" | Out-Null
$out = "$Root\dist\skyrim-runtime-extender-$version.zip"
Remove-Item -Force $out -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open($out, "Create")
try {
    foreach ($e in $entries.GetEnumerator()) { [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $e.Value, $e.Key, "Optimal") }
} finally { $zip.Dispose() }
$out
