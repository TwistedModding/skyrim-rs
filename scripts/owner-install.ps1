# The owner's permanent install in a game folder, which no suite may run next to. Called under the game lock: -Stash moves
# what is there to a fresh folder and returns the folder, -Restore <folder> moves it back; nothing is ever overwritten.
# The folder holds at least game-dir.txt for the whole run, so one with a file in it means a run died before restoring,
# and every call but -Restore stops on it. With no switch it only makes that check.
param(
    [Parameter(Mandatory)][string]$GameDir,
    [switch]$Stash,
    [string]$Restore = ""
)
$ErrorActionPreference = "Stop"
$Root = "$env:LOCALAPPDATA\skyrim-rs\stash-multiruntime"
$Items = @(
    "Data\SKSE\Plugins\skyrim-runtime-extender", "Data\SKSE\Plugins\skyrim_runtime_extender.dll",
    "Data\SKSE\Plugins\skyrim-rs", "Data\SKSE\Plugins\skyrim_rs.dll", "Data\SKSE\Plugins\skyrim_rs_improved_camera_stub.dll",
    "Data\Scripts\SkyrimRsExample.pex", "dinput8.dll", "steam_appid.txt"
)
if ($Restore) {
    foreach ($item in $Items) {
        if (-not (Test-Path "$Restore\$item")) { continue }
        if (Test-Path "$GameDir\$item") { throw "$GameDir\$item exists; the owner's copy stays in $Restore" }
        Move-Item "$Restore\$item" "$GameDir\$item"
    }
    Remove-Item $Restore -Recurse -Force
    return
}
foreach ($dead in @(if (Test-Path $Root) { Get-ChildItem $Root -Directory | Where-Object { Get-ChildItem $_.FullName -Recurse -File -Force } })) {
    $from = Get-Content "$($dead.FullName)\game-dir.txt" -ErrorAction SilentlyContinue
    throw "a run died before restoring $($dead.FullName); check $from for what it left and run: scripts\owner-install.ps1 -GameDir `"$from`" -Restore `"$($dead.FullName)`""
}
if (-not $Stash) { return }
$folder = "$Root\$(Get-Date -Format yyyyMMdd-HHmmss)-$PID"
New-Item -ItemType Directory $folder | Out-Null
Set-Content "$folder\game-dir.txt" $GameDir -Encoding ascii
foreach ($item in $Items) {
    if (-not (Test-Path "$GameDir\$item")) { continue }
    New-Item -ItemType Directory -Force (Split-Path "$folder\$item") | Out-Null
    Move-Item "$GameDir\$item" "$folder\$item"
    Write-Host "stashed $GameDir\$item in $folder"
}
$folder
