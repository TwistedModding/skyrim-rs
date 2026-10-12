# Compiles every .psc under the given source folders with the Creation Kit's Papyrus compiler against the game's
# Data\Scripts\Source (SKSE's extended declarations) and the base sources shipped in Scripts.zip, into $OutDir.
# Prints the .pex paths. The flags file and base sources are extracted from Scripts.zip into $OutDir\base on first use.
param(
    [string[]]$Source = @("$(Split-Path $PSScriptRoot -Parent)\crates\testplugin\papyrus"),
    [string]$OutDir = "$(Split-Path $PSScriptRoot -Parent)\target\papyrus",
    [string]$Compiler = "E:\SteamLibrary\steamapps\common\Skyrim Special Edition\Papyrus Compiler\PapyrusCompiler.exe",
    [string]$ScriptsZip = "E:\SteamLibrary\steamapps\common\Skyrim Special Edition\Data\Scripts.zip",
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"
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
