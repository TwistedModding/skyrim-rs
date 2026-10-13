# Launches Skyrim through SKSE, waits for DevBench, runs one replay or one scenario, collects the result and logs, stops the game.
# See docs/DEVBENCH.md.
param(
    [string]$Replay,
    [string]$Steps,
    [string]$OutDir = "devbench-out",
    [int]$TimeoutSec = 300,
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"
)
$ErrorActionPreference = "Stop"
$SkseLogs = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\SKSE"
$RuntimeJson = "$GameDir\Data\SKSE\Plugins\devbench\runtime.json"
$Base = $null

# Every request opens its own connection: DevBench drops idle keep-alive connections, and a reused one fails with
# "connection that was expected to be kept alive was closed". A refused connection is retried while the game process
# is alive, so a stalled server does not end the run before the game does.
function Invoke-Tool($Tool, $Body) {
    for ($attempt = 1; ; $attempt++) {
        try { return (Invoke-WebRequest "$Base/api/tool/$Tool" -Method Post -ContentType "application/json" -Body $Body -UseBasicParsing -DisableKeepAlive -TimeoutSec 120).Content }
        catch {
            $r = $_.Exception.Response
            if ($r) { throw "devbench $Tool failed: " + (New-Object IO.StreamReader($r.GetResponseStream())).ReadToEnd() }
            if ($attempt -ge 5 -or -not (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue)) { throw }
            Write-Warning "devbench ${Tool}: $($_.Exception.Message); retrying"
            Start-Sleep -Seconds 3
        }
    }
}

function Get-Health {
    try { return (Invoke-WebRequest "$Base/api/health" -UseBasicParsing -DisableKeepAlive -TimeoutSec 5).Content | ConvertFrom-Json } catch { return $null }
}

function Wait-Until([scriptblock]$Cond, [int]$Seconds, [string]$What) {
    $t0 = Get-Date
    while (-not (& $Cond)) {
        if (((Get-Date) - $t0).TotalSeconds -gt $Seconds) { throw "timeout after $Seconds s waiting for $What" }
        Start-Sleep -Seconds 2
    }
}

# The game pauses whenever its window loses focus, which stops DevBench's main-thread tasks; re-activate it when frames stop.
function Wait-Run($Tool, $RunId) {
    $t0 = Get-Date
    $lastFrame = -1
    while ($true) {
        $status = Invoke-Tool $Tool "{`"action`":`"status`",`"runId`":$RunId}"
        if (($status | ConvertFrom-Json).done) { return $status }
        if (((Get-Date) - $t0).TotalSeconds -gt $TimeoutSec) { throw "run $RunId did not finish within $TimeoutSec s" }
        $h = Get-Health
        if ($h -and $h.frame -eq $lastFrame) {
            Write-Warning "game paused at frame $lastFrame; re-activating its window"
            [void]$Shell.AppActivate($Game.Id)
        }
        if ($h) { $lastFrame = $h.frame }
        Start-Sleep -Seconds 2
    }
}

if (-not $Replay -and -not $Steps) { throw "give -Replay <recording> or -Steps <steps.json>" }
if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$Shell = New-Object -ComObject WScript.Shell
$Game = $null
try {
    Start-Process "$GameDir\skse64_loader.exe" -WorkingDirectory $GameDir
    Wait-Until { Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue } 60 "the SkyrimSE process"
    $Game = Get-Process -Name SkyrimSE
    Wait-Until {
        if (Test-Path $RuntimeJson) { $script:Base = "http://127.0.0.1:$((Get-Content $RuntimeJson -Raw | ConvertFrom-Json).port)" }
        $h = Get-Health
        $h -and $h.pid -eq $Game.Id -and $h.lastLifecycle -eq "dataLoaded"
    } 180 "DevBench at the main menu"
    [void]$Shell.AppActivate($Game.Id)
    if ($Replay) {
        if ($Replay -notmatch "[/\\]") { $Replay = "Data/SKSE/Plugins/devbench/recordings/$Replay" }
        $queued = Invoke-Tool record "{`"action`":`"replay`",`"path`":`"$Replay`",`"restoreScene`":true,`"closeMenus`":true,`"async`":true}"
        $tool = "record"
    } else {
        $queued = Invoke-Tool scenario ('{"async":true,"continueOnError":true,"steps":' + (Get-Content $Steps -Raw) + '}')
        $tool = "scenario"
    }
    $runId = ($queued | ConvertFrom-Json).runId
    $status = Wait-Run $tool $runId
    $status | Out-File -Encoding utf8 "$OutDir\result.json"
    (Invoke-WebRequest "$Base/api/events" -UseBasicParsing -DisableKeepAlive).Content | Out-File -Encoding utf8 "$OutDir\events.json"
    $r = ($status | ConvertFrom-Json).result
    "runId=$runId ok=$($r.ok) aborted=$($r.aborted) stepsRun=$($r.stepsRun) elapsedMs=$($r.elapsedMs)"
    $failed = @($r.results | Where-Object { $_.ok -eq $false -or $_.timedOut })
    foreach ($f in $failed) { "failed step $($f.index): $($f | ConvertTo-Json -Compress)" }
    $ok = [bool]$r.ok
} finally {
    if ($Game) { Stop-Process -Id $Game.Id -Force -ErrorAction SilentlyContinue; $Game.WaitForExit(15000) | Out-Null }
    foreach ($f in "skse64.log", "devbench.log") { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } }
}
if (-not $ok) { exit 1 }
