# Builds release and writes the loader's package dist\skyrim-runtime-extender-<version>.zip: its files at their paths under Data and its README.
# The version is <major.minor>.<commits on HEAD>, with -dirty for a tree with uncommitted changes.
$ErrorActionPreference = "Stop"
$Root = Split-Path $PSScriptRoot -Parent
$Release = "0.4"
$commits = git -C $Root rev-list --count HEAD
if ($LASTEXITCODE -ne 0) { throw "git rev-list failed" }
$version = "$Release.$commits"
if (git -C $Root status --porcelain) { $version += "-dirty" }
$env:SKYRIM_RS_VERSION = $version
cargo build --release --manifest-path "$Root\Cargo.toml" -p host -p standalone
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$release = "$Root\target\release"
$entries = [ordered]@{
    "SKSE/Plugins/skyrim_runtime_extender.dll" = "$release\skyrim_runtime_extender.dll"
    "optional-standalone/dinput8.dll" = "$release\dinput8.dll"
    "optional-standalone/README.md" = "$Root\crates\standalone\README.md"
    "README.md" = "$Root\crates\host\README.md"
}
New-Item -ItemType Directory -Force "$Root\dist" | Out-Null
$out = "$Root\dist\skyrim-runtime-extender-$version.zip"
Remove-Item -Force $out -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open($out, "Create")
try {
    foreach ($e in $entries.GetEnumerator()) { [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $e.Value, $e.Key, "Optimal") }
} finally { $zip.Dispose() }
$out
