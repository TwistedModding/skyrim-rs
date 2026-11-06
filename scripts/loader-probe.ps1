# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# Runs the loader-probe plugin and copies of it with patched version data under one script extender, up to the
# data-loaded message, and keeps that loader's log and the probe's. See docs/subsystems/skse-loaders.md. Each copy's
# name is its variant's, so a line of skse64.log says what was refused. The install only gains files in Data\SKSE\Plugins
# for the run; the owner's install there is moved aside and put back.
param(
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition",
    [string]$OutDir = "loader-probe-out",
    [ValidateSet("verify", "release")][string]$Profile = "verify",
    # Further DLLs to install for the run, as they are, comma-separated.
    [string[]]$Extra = @(),
    [int]$TimeoutSec = 240
)
$ErrorActionPreference = "Stop"
. "$PSScriptRoot\game-dialogs.ps1"
$Root = Split-Path $PSScriptRoot -Parent
$Plugins = "$GameDir\Data\SKSE\Plugins"
$SkseLogs = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\SKSE"
$Logs = @("skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "skyrim-rs-loader-probe.log")
$Owner = @("skyrim-runtime-extender", "skyrim_runtime_extender.dll", "skyrim-rs", "skyrim_rs.dll", "skyrim_rs_improved_camera_stub.dll", "improved_camera_stub.dll")
$Listed = @(0x01050610, 0x01061610, 0x01064920, 0x01070680)
$Gated = 1600000000

# name = patched fields; every field left out keeps the plugin! default.
$Variants = [ordered]@{
    "probe-default" = @{}
    "probe-data2" = @{ data = 2 }
    "probe-data0" = @{ data = 0 }
    "probe-noname" = @{ noname = $true }
    "probe-method8" = @{ method = 8 }
    "probe-m5f2" = @{ flags = 2; method = 5 }
    "probe-m1f2" = @{ flags = 2 }
    "probe-m2f1" = @{ flags = 1; method = 2 }
    "probe-m0" = @{ flags = 0; method = 0 }
    "probe-m0unlisted" = @{ flags = 0; method = 0; list = @() }
    "probe-gated" = @{ flags = 1; list = @(); time = $Gated }
    "probe-gatedlisted" = @{ flags = 0; time = $Gated }
    "probe-newer" = @{ minimum = 0x7F000000 }
}

function Install-Variant($bytes, $name, $v) {
    $out = [byte[]]$bytes.Clone()
    $marker = [Text.Encoding]::ASCII.GetBytes("skyrim-rs-loader-probe")
    $text = [Text.Encoding]::GetEncoding(28591).GetString($out)
    $head = [Text.Encoding]::GetEncoding(28591).GetString([byte[]](1, 0, 0, 0, 1, 0, 0, 0) + $marker)
    $at = $text.IndexOf($head, [StringComparison]::Ordinal)
    if ($at -lt 0) { throw "no version data in the probe" }
    $put = { param($offset, [uint32]$value) [BitConverter]::GetBytes($value).CopyTo($out, $at + $offset) }
    $field = [byte[]]::new(256)
    if (-not $v.noname) { [Text.Encoding]::ASCII.GetBytes($name).CopyTo($field, 0) }
    $field.CopyTo($out, $at + 8)
    if ($v.ContainsKey("data")) { & $put 0 $v.data }
    if ($v.ContainsKey("flags")) { & $put 0x304 $v.flags }
    if ($v.ContainsKey("method")) { & $put 0x308 $v.method }
    if ($v.ContainsKey("list")) { for ($i = 0; $i -lt 16; $i++) { & $put (0x30C + 4 * $i) $(if ($i -lt $v.list.Count) { $v.list[$i] } else { 0 }) } }
    if ($v.ContainsKey("minimum")) { & $put 0x34C $v.minimum }
    if ($v.ContainsKey("time")) {
        $nt = [BitConverter]::ToInt32($out, 0x3C)
        [BitConverter]::GetBytes([uint32]$v.time).CopyTo($out, $nt + 8)
    }
    [IO.File]::WriteAllBytes("$Plugins\$name.dll", $out)
}

Push-Location $Root
try { cargo build --profile $Profile -p loader-probe; if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } } finally { Pop-Location }
$probe = [IO.File]::ReadAllBytes("$Root\target\$Profile\skyrim_rs_loader_probe.dll")

while ($true) {
    try { $tookLock = & "$PSScriptRoot\game-free.ps1"; break } catch { "game busy: $($_.Exception.Message); waiting 60 s"; Start-Sleep -Seconds 60 }
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
Remove-Item "$SkseLogs\skyrim-rs-loader-probe.log" -ErrorAction SilentlyContinue

$stash = "$env:LOCALAPPDATA\skyrim-rs\stash-multiruntime"
$present = @($Owner | Where-Object { Test-Path "$Plugins\$_" })
if ($present -and (Test-Path $stash) -and @(Get-ChildItem $stash -Force).Count) { $stash = Join-Path $stash "loader-probe-$PID" }
$retraceOff = Test-Path "$Plugins\retrace.dll"
$installed = @()
$started = Get-Date
try {
    if ($present) {
        New-Item -ItemType Directory -Force $stash | Out-Null
        foreach ($item in $present) { Move-Item "$Plugins\$item" "$stash\$item" }
    }
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
    foreach ($name in $Variants.Keys) {
        Install-Variant $probe $name $Variants[$name]
        $installed += "$Plugins\$name.dll"
    }
    foreach ($dll in @($Extra -split ",")) {
        Copy-Item $dll $Plugins
        $installed += "$Plugins\$(Split-Path $dll -Leaf)"
    }
    Start-Process "$GameDir\skse64_loader.exe" -WorkingDirectory $GameDir
    $deadline = $started.AddSeconds($TimeoutSec)
    $seen = $false
    while (-not $seen -and (Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        Close-Dialogs
        $seen = (Test-Path "$SkseLogs\skyrim-rs-loader-probe.log") -and (Select-String -Path "$SkseLogs\skyrim-rs-loader-probe.log" -Pattern "^probe-default .*kind=8 " -Quiet)
    }
    Start-Sleep -Seconds 5
    "data loaded seen: $seen after $([int]((Get-Date) - $started).TotalSeconds) s"
} finally {
    # Every step runs whatever the one before it did: nothing of the run may stay in the install.
    $ErrorActionPreference = "Continue"
    $game = @(Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq "$GameDir\SkyrimSE.exe" -and $_.StartTime -ge $started.AddSeconds(-1) })
    $game | Stop-Process -Force
    $game | Wait-Process -Timeout 30 -ErrorAction SilentlyContinue
    foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } }
    # A killed game keeps its files a moment after the process has gone.
    for ($try = 0; $try -lt 30 -and @($installed | Where-Object { Test-Path $_ }).Count; $try++) {
        foreach ($item in $installed) { Remove-Item $item -ErrorAction SilentlyContinue }
        Start-Sleep -Seconds 1
    }
    foreach ($item in $installed) { if (Test-Path $item) { Write-Warning "$item could not be removed" } }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force } else { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
    }
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    foreach ($item in $present) { Move-Item "$stash\$item" "$Plugins\$item" }
    if ($tookLock) { & "$PSScriptRoot\game-free.ps1" -Release }
}
