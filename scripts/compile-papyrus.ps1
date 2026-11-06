# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# Compiles every .psc under the given source folders with the Creation Kit's Papyrus compiler against the game's
# Data\Scripts\Source (SKSE's extended declarations) and the base sources shipped in Scripts.zip, into $OutDir.
# Prints the .pex paths. The flags file and base sources are extracted from Scripts.zip into $OutDir\base on first use.
param(
    [string[]]$Source = @("$(Split-Path $PSScriptRoot -Parent)\crates\testplugin\papyrus"),
    [string]$OutDir = "$(Split-Path $PSScriptRoot -Parent)\target\papyrus",
    [string]$GameDir = (& "$PSScriptRoot\runtimes.ps1").dir,
    [string]$Compiler = "$GameDir\Papyrus Compiler\PapyrusCompiler.exe",
    [string]$ScriptsZip = "$GameDir\Data\Scripts.zip"
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Base = Join-Path $OutDir "base"
if (-not (Test-Path "$Base\TESV_Papyrus_Flags.flg")) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    New-Item -ItemType Directory -Force $Base | Out-Null
    $zip = [IO.Compression.ZipFile]::OpenRead($ScriptsZip)
    try {
        foreach ($e in $zip.Entries) {
            if ($e.FullName -like "Source/Scripts/*" -and $e.Name) { [IO.Compression.ZipFileExtensions]::ExtractToFile($e, (Join-Path $Base $e.Name), $true) }
        }
    } finally { $zip.Dispose() }
}
$imports = (@($Source | ForEach-Object { (Resolve-Path $_).Path }) + @("$GameDir\Data\Scripts\Source", $Base)) -join ";"
$pex = @()
foreach ($dir in $Source) {
    foreach ($psc in Get-ChildItem (Resolve-Path $dir).Path -Filter *.psc) {
        $out = & $Compiler $psc.FullName "-f=TESV_Papyrus_Flags.flg" "-i=$imports" "-o=$OutDir"
        if ($LASTEXITCODE -ne 0) { throw "PapyrusCompiler failed on $($psc.Name):`n$($out -join "`n")" }
        $pex += Join-Path $OutDir ($psc.BaseName + ".pex")
    }
}
$pex
