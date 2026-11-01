# Builds release and writes the loader's package dist\skyrim-rs-<version>.zip and the example plugin's
# dist\skyrim-rs-example-<version>.zip, each with its files at their install paths and its README.
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
cargo build --release --manifest-path "$Root\Cargo.toml" -p host -p standalone -p example-mod
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$pex = & "$PSScriptRoot\compile-papyrus.ps1" -Source "$Root\crates\example-mod\papyrus"
$packages = (cargo metadata --no-deps --format-version 1 --manifest-path "$Root\Cargo.toml" | ConvertFrom-Json).packages
$release = "$Root\target\release"
New-Item -ItemType Directory -Force "$Root\dist" | Out-Null
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem

function Write-Package([string]$Name, [string]$Package, $Entries) {
    $version = ($packages | Where-Object name -eq $Package).version
    $out = "$Root\dist\$Name-$version.zip"
    Remove-Item -Force $out -ErrorAction SilentlyContinue
    $zip = [IO.Compression.ZipFile]::Open($out, "Create")
    try {
        foreach ($e in $Entries.GetEnumerator()) { [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $e.Value, $e.Key, "Optimal") }
    } finally { $zip.Dispose() }
    $out
}

Write-Package "skyrim-rs" "host" ([ordered]@{
    "Data/SKSE/Plugins/skyrim_rs.dll" = "$release\skyrim_rs.dll"
    "optional-standalone/dinput8.dll" = "$release\dinput8.dll"
    "optional-standalone/README.md" = "$Root\crates\standalone\README.md"
    "README.md" = "$Root\crates\host\README.md"
})
Write-Package "skyrim-rs-example" "example-mod" ([ordered]@{
    "Data/SKSE/Plugins/skyrim-rs/skyrim_rs_example_mod.dll" = "$release\skyrim_rs_example_mod.dll"
    "Data/Scripts/SkyrimRsExample.pex" = $pex
    "README.md" = "$Root\crates\example-mod\README.md"
})
