# The installs the suites run on, one row per runtime (docs/subsystems/runtimes.md). Returns the row of -Runtime with what
# follows from it; -GameDir replaces the folder, and the executable found there must be that runtime.
param(
    [string]$Runtime = "1.7.104",
    [string]$GameDir = ""
)
$Installs = @{
    "1.7.104"  = @{ dir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"; skse = "2.3.1" }
    "1.6.1170" = @{ dir = "C:\Skyrim 1.6.1170"; skse = "2.2.6" }
    "1.6.353"  = @{ dir = "C:\Skyrim 1.6.353"; skse = "2.1.5" }
    "1.5.97"   = @{ dir = "C:\Skyrim 1.5.97"; skse = "2.0.19" }
}
$row = $Installs[$Runtime]
if (-not $row) { throw "unknown runtime $Runtime; known: $(($Installs.Keys | Sort-Object) -join ', ')" }
$dir = if ($GameDir) { $GameDir } else { $row.dir }
$found = (Get-Item "$dir\SkyrimSE.exe").VersionInfo.FileVersion
if ($found -ne "$Runtime.0") { throw "$dir\SkyrimSE.exe is $found, not $Runtime" }
$tag = $Runtime -replace '\.', '-'
$canonical = $Runtime -eq "1.7.104"
@{
    runtime = $Runtime
    tag = $tag
    dir = $dir
    skse = $row.skse
    canonical = $canonical
    # The save a scenario starts from. It is never loaded on another runtime; base-save-steps.json writes the others.
    baseSave = if ($canonical) { "retrace-base" } else { "retrace-base-$tag" }
    profile = "$tag.profile"
    # DevBench saves and loads through SKSE's Game.SaveGame and Game.LoadGame, whose declarations come with SKSE's scripts.
    canSave = Test-Path "$dir\Data\Scripts\Game.pex"
}
