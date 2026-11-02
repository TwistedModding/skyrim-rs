# Builds release and writes the loader's package dist\skyrim-rs-<version>.zip and the example plugin's
# dist\skyrim-rs-example-<version>.zip, each with its files at their paths under Data and its README.
# The version is <major.minor>.<commits on HEAD>, with -dirty for a tree with uncommitted changes.
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
$Release = "0.4"
$commits = git -C $Root rev-list --count HEAD
if ($LASTEXITCODE -ne 0) { throw "git rev-list failed" }
$version = "$Release.$commits"
if (git -C $Root status --porcelain) { $version += "-dirty" }
$env:SKYRIM_RS_VERSION = $version
cargo build --release --manifest-path "$Root\Cargo.toml" -p host -p standalone -p example-mod
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$pex = & "$PSScriptRoot\compile-papyrus.ps1" -Source "$Root\crates\example-mod\papyrus"
$release = "$Root\target\release"
New-Item -ItemType Directory -Force "$Root\dist" | Out-Null
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem

function Write-Package([string]$Name, $Entries) {
    $out = "$Root\dist\$Name-$version.zip"
    Remove-Item -Force $out -ErrorAction SilentlyContinue
    $zip = [IO.Compression.ZipFile]::Open($out, "Create")
    try {
        foreach ($e in $Entries.GetEnumerator()) { [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $e.Value, $e.Key, "Optimal") }
    } finally { $zip.Dispose() }
    $out
}

Write-Package "skyrim-rs" ([ordered]@{
    "SKSE/Plugins/skyrim_rs.dll" = "$release\skyrim_rs.dll"
    "optional-standalone/dinput8.dll" = "$release\dinput8.dll"
    "optional-standalone/README.md" = "$Root\crates\standalone\README.md"
    "README.md" = "$Root\crates\host\README.md"
})
Write-Package "skyrim-rs-example" ([ordered]@{
    "SKSE/Plugins/skyrim-rs/skyrim_rs_example_mod.dll" = "$release\skyrim_rs_example_mod.dll"
    "Scripts/SkyrimRsExample.pex" = $pex
    "README.md" = "$Root\crates\example-mod\README.md"
})
