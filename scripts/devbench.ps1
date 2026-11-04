# Launches Skyrim through SKSE, waits for DevBench, runs one replay or one scenario (in parts, with -Between run between
# them, when the steps file holds an array of step arrays), collects the result and logs, stops the game.
# See docs/DEVBENCH.md.
param(
    [string]$Replay,
    [string]$Steps,
    [scriptblock]$Between,
    [string]$OutDir = "devbench-out",
    [int]$TimeoutSec = 300,
    # skse: through skse64_loader.exe; standalone: SkyrimSE.exe itself, for the dinput8.dll host
    [ValidateSet("skse", "standalone")][string]$Launch = "skse",
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

. "$PSScriptRoot\game-dialogs.ps1"

function Wait-Until([scriptblock]$Cond, [int]$Seconds, [string]$What) {
    $t0 = Get-Date
    while (-not (& $Cond)) {
        Close-Dialogs
        if (((Get-Date) - $t0).TotalSeconds -gt $Seconds) { throw "timeout after $Seconds s waiting for $What" }
        Start-Sleep -Seconds 2
    }
}

Add-Type -Namespace DevBench -Name Fg -MemberDefinition '[DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);'

# Whether the game's window is the foreground window; injected keys and typed characters only reach it there.
function Game-InFront {
    $owner = 0
    [void][DevBench.Fg]::GetWindowThreadProcessId([DevBench.Fg]::GetForegroundWindow(), [ref]$owner)
    $Game -and $owner -eq $Game.Id
}

# Without bAlwaysActive the game pauses whenever its window loses focus, which stops DevBench's main-thread tasks; with it
# the game runs on behind other windows but typed keys never reach it. Either way, keep its window in front.
function Wait-Run($Tool, $RunId) {
    $t0 = Get-Date
    $lastFrame = -1
    while ($true) {
        $status = Invoke-Tool $Tool "{`"action`":`"status`",`"runId`":$RunId}"
        if (($status | ConvertFrom-Json).done) { return $status }
        if (((Get-Date) - $t0).TotalSeconds -gt $TimeoutSec) { throw "run $RunId did not finish within $TimeoutSec s" }
        $h = Get-Health
        if ($h -and $h.frame -eq $lastFrame) {
            Close-Dialogs
            Write-Warning "game paused at frame $lastFrame; re-activating its window"
            [void]$Shell.AppActivate($Game.Id)
        } elseif (-not (Game-InFront)) {
            Write-Warning "game window not in front; re-activating it"
            [void]$Shell.AppActivate($Game.Id)
        }
        if ($h) { $lastFrame = $h.frame }
        Start-Sleep -Seconds 2
    }
}

if (-not $Replay -and -not $Steps) { throw "give -Replay <recording> or -Steps <steps.json>" }
$tookLock = & "$PSScriptRoot\game-free.ps1"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$Shell = New-Object -ComObject WScript.Shell
$Game = $null
try {
    $exe = if ($Launch -eq "standalone") { "SkyrimSE.exe" } else { "skse64_loader.exe" }
    Start-Process "$GameDir\$exe" -WorkingDirectory $GameDir
    Wait-Until { Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue } 60 "the SkyrimSE process"
    $Game = Get-Process -Name SkyrimSE | Select-Object -First 1
    Wait-Until {
        if (Test-Path $RuntimeJson) { $script:Base = "http://127.0.0.1:$((Get-Content $RuntimeJson -Raw | ConvertFrom-Json).port)" }
        $h = Get-Health
        # Started directly, SkyrimSE.exe may hand itself to Steam and come back as a new process.
        if ($h -and $h.pid -ne $Game.Id) { $script:Game = Get-Process -Id $h.pid -ErrorAction SilentlyContinue }
        $h -and $Game -and $h.pid -eq $Game.Id -and $h.lastLifecycle -eq "dataLoaded"
    } 180 "DevBench at the main menu"
    [void]$Shell.AppActivate($Game.Id)
    if ($Replay) {
        if ($Replay -notmatch "[/\\]") { $Replay = "Data/SKSE/Plugins/devbench/recordings/$Replay" }
        $queued = Invoke-Tool record "{`"action`":`"replay`",`"path`":`"$Replay`",`"restoreScene`":true,`"closeMenus`":true,`"async`":true}"
        $status = Wait-Run record ($queued | ConvertFrom-Json).runId
    } else {
        # A steps file holding an array of step arrays runs as parts in the one game session, -Between (with the number
        # of the part just finished) between them; the parts' results are merged with the step indices continuing.
        $parsed = Get-Content $Steps -Raw | ConvertFrom-Json
        $parts = @(, $parsed)
        if ($parsed.Count -gt 0 -and $parsed[0] -is [array]) { $parts = @($parsed) }
        $merged = @{ ok = $true; aborted = $false; stepsRun = 0; elapsedMs = 0; results = @() }
        $offset = 0
        for ($i = 0; $i -lt $parts.Count; $i++) {
            $queued = Invoke-Tool scenario ('{"async":true,"continueOnError":true,"steps":' + (ConvertTo-Json -InputObject @($parts[$i]) -Depth 20 -Compress) + '}')
            $status = Wait-Run scenario ($queued | ConvertFrom-Json).runId
            $r = ($status | ConvertFrom-Json).result
            foreach ($s in $r.results) { $s.index += $offset }
            $merged.results += @($r.results)
            $merged.ok = $merged.ok -and [bool]$r.ok
            $merged.aborted = $merged.aborted -or [bool]$r.aborted
            $merged.stepsRun += $r.stepsRun
            $merged.elapsedMs += $r.elapsedMs
            $offset += @($parts[$i]).Count
            if ($Between -and $i -lt $parts.Count - 1) { & $Between ($i + 1) }
        }
        if ($parts.Count -gt 1) { $status = @{ done = $true; ok = $merged.ok; result = $merged } | ConvertTo-Json -Depth 20 }
    }
    $status | Out-File -Encoding utf8 "$OutDir\result.json"
    (Invoke-WebRequest "$Base/api/events" -UseBasicParsing -DisableKeepAlive).Content | Out-File -Encoding utf8 "$OutDir\events.json"
    $r = ($status | ConvertFrom-Json).result
    "ok=$($r.ok) aborted=$($r.aborted) stepsRun=$($r.stepsRun) elapsedMs=$($r.elapsedMs)"
    $failed = @($r.results | Where-Object { $_.ok -eq $false -or $_.timedOut })
    foreach ($f in $failed) { "failed step $($f.index): $($f | ConvertTo-Json -Compress)" }
    $ok = [bool]$r.ok
} finally {
    if ($Game) { Stop-Process -Id $Game.Id -Force -ErrorAction SilentlyContinue; $Game.WaitForExit(15000) | Out-Null }
    foreach ($f in "skse64.log", "devbench.log") { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } }
    if ($tookLock) { & "$PSScriptRoot\game-free.ps1" -Release }
}
if (-not $ok) { exit 1 }
