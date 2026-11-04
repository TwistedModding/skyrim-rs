# Exercises DevBench's REST tools on one runtime with only what its install already holds (devbench-probe-steps.json, from
# the runtime's base save), for the per-runtime table of docs/DEVBENCH.md: result.json has every step's answer, health.json
# and tools.json the server's, install-diff.txt what changed in the install. The owner's install and retrace.dll stay
# out for the run, and the probe's saves and captures are removed afterwards.
param(
    [string]$Runtime = "1.7.104",
    [string]$OutDir = "devbench-probe-$Runtime-out",
    [switch]$WaitForGame
)
$ErrorActionPreference = "Stop"
$Install = & "$PSScriptRoot\runtimes.ps1" -Runtime $Runtime
$GameDir = $Install.dir
$Plugins = "$GameDir\Data\SKSE\Plugins"
$Saves = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\Saves"
$Captures = "$Plugins\devbench\captures"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$tookLock = & "$PSScriptRoot\game-free.ps1" -Wait:$WaitForGame
$stash = $null
$retraceOff = $false
try {
    $before = @(& "$PSScriptRoot\install-state.ps1" -GameDir $GameDir)
    # Under the lock, so one run never takes another's stashed install for a clean one.
    $stash = & "$PSScriptRoot\owner-install.ps1" -GameDir $GameDir -Stash
    $retraceOff = Test-Path "$Plugins\retrace.dll"
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
    & "$PSScriptRoot\base-save.ps1" -Runtime $Runtime -OutDir $OutDir
    $capturesBefore = @(if (Test-Path $Captures) { Get-ChildItem $Captures -Recurse -File | ForEach-Object FullName })
    $catalog = {
        param($part)
        (Invoke-WebRequest "$Base/api/health" -UseBasicParsing -DisableKeepAlive).Content | Out-File -Encoding utf8 "$OutDir\health.json"
        (Invoke-WebRequest "$Base/api/tools" -UseBasicParsing -DisableKeepAlive).Content | Out-File -Encoding utf8 "$OutDir\tools.json"
    }
    & "$PSScriptRoot\devbench.ps1" -Runtime $Runtime -Steps "$PSScriptRoot\devbench-probe-steps.json" -Between $catalog -OutDir $OutDir -TimeoutSec 900
} finally {
    Remove-Item "$Saves\skyrim-rs-probe*$($Install.tag)*" -ErrorAction SilentlyContinue
    if (Test-Path $Captures) {
        Get-ChildItem $Captures -Recurse -File | Where-Object { $_.FullName -notin $capturesBefore } | Remove-Item
        Get-ChildItem $Captures -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending | Where-Object { -not (Get-ChildItem $_.FullName -Force) } | Remove-Item
        if (-not $capturesBefore -and -not (Get-ChildItem $Captures -Force)) { Remove-Item $Captures }
    }
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    if ($stash) { & "$PSScriptRoot\owner-install.ps1" -GameDir $GameDir -Restore $stash }
    if ($before) {
        $after = @(& "$PSScriptRoot\install-state.ps1" -GameDir $GameDir)
        @(Compare-Object $before $after | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }) | Out-File -Encoding utf8 "$OutDir\install-diff.txt"
    }
    if ($tookLock) { & "$PSScriptRoot\game-free.ps1" -Release }
}
