# Launches Skyrim through SKSE, waits for DevBench, runs one replay or one scenario (in parts, with -Between run between
# them, when the steps file holds an array of step arrays), collects the result and logs, stops the game.
# In a steps file {rt} is the runtime as save names carry it (1-6-353) and {base} the runtime's base save.
# See docs/DEVBENCH.md.
param(
    [string]$Replay,
    [string]$Steps,
    [scriptblock]$Between,
    [string]$OutDir = "devbench-out",
    [int]$TimeoutSec = 300,
    # skse: through skse64_loader.exe; standalone: SkyrimSE.exe itself, for the dinput8.dll host
    [ValidateSet("skse", "standalone")][string]$Launch = "skse",
    [string]$Runtime = "1.7.104",
    # Keeps the game's window in front instead of giving the foreground back.
    [switch]$Foreground,
    [string]$GameDir = "",
    # Waits for a game or lock held by someone else instead of failing.
    [switch]$WaitForGame
)
$ErrorActionPreference = "Stop"
$Install = & "$PSScriptRoot\runtimes.ps1" -Runtime $Runtime -GameDir $GameDir
$GameDir = $Install.dir
$SkseLogs = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\SKSE"
$RuntimeJson = "$GameDir\Data\SKSE\Plugins\devbench\runtime.json"
$HostLog = "$SkseLogs\skyrim-runtime-extender-standalone.log"
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

# DevBench's input goes through the game's own event queue and bAlwaysActive keeps it running behind other windows, so a run
# does not need the foreground: a game that takes it (it does at launch) goes behind the window the user was in. -Foreground
# keeps it in front, for the overlay suite's typed text, the one thing that goes through SendInput.
Add-Type -Namespace DevBench -Name Focus -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsWindow(System.IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr hwnd, System.IntPtr after, int x, int y, int w, int h, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint pid);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
'@
function Yield-Focus {
    if ($Foreground) { return }
    $front = [DevBench.Fg]::GetForegroundWindow()
    if (-not (Game-InFront)) {
        if ($front -ne [IntPtr]::Zero) { $script:UserWindow = $front }
        return
    }
    # Attached to the game's input queue this process may hand the foreground on.
    [void][DevBench.Focus]::SetWindowPos($front, [IntPtr]1, 0, 0, 0, 0, 0x13)
    if ($UserWindow -and [DevBench.Focus]::IsWindow($UserWindow)) {
        $owner = 0
        $thread = [DevBench.Focus]::GetWindowThreadProcessId($front, [ref]$owner)
        $self = [DevBench.Focus]::GetCurrentThreadId()
        [void][DevBench.Focus]::AttachThreadInput($self, $thread, $true)
        [void][DevBench.Focus]::SetForegroundWindow($UserWindow)
        [void][DevBench.Focus]::AttachThreadInput($self, $thread, $false)
    }
}

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
            Write-Warning "game not advancing at frame $lastFrame"
        }
        Yield-Focus
        if ($h) { $lastFrame = $h.frame }
        Start-Sleep -Seconds 2
    }
}

if (-not $Replay -and -not $Steps) { throw "give -Replay <recording> or -Steps <steps.json>" }
$tookLock = & "$PSScriptRoot\game-free.ps1" -Wait:$WaitForGame
New-Item -ItemType Directory -Force $OutDir | Out-Null
$UserWindow = [DevBench.Fg]::GetForegroundWindow()
$Game = $null
try {
    # The lock's holder may have spent minutes building; a game that started meanwhile is not ours to stop.
    while (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) {
        if (-not $WaitForGame) { throw "SkyrimSE is already running" }
        Write-Warning "SkyrimSE is running; trying again in 60 s"
        Start-Sleep -Seconds 60
    }
    $exe = if ($Launch -eq "standalone") { "SkyrimSE.exe" } else { "skse64_loader.exe" }
    if ($Launch -eq "skse" -and -not (Test-Path "$GameDir\$exe")) { throw "$GameDir has no $exe" }
    $Launched = Get-Date
    Start-Process "$GameDir\$exe" -WorkingDirectory $GameDir
    Wait-Until { Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue } 60 "the SkyrimSE process"
    $Game = Get-Process -Name SkyrimSE | Select-Object -First 1
    Wait-Until {
        if (Test-Path $RuntimeJson) { $script:Base = "http://127.0.0.1:$((Get-Content $RuntimeJson -Raw | ConvertFrom-Json).port)" }
        if ($Launch -eq "skse" -and -not (Get-Process -Id $Game.Id -ErrorAction SilentlyContinue)) { throw "the game ended before the main menu" }
        if ($Launch -eq "standalone" -and (Get-Item $HostLog -ErrorAction SilentlyContinue).LastWriteTime -gt $Launched -and (Select-String -Path $HostLog -SimpleMatch "nothing loaded" -Quiet)) { throw "the standalone loader loaded nothing; see $HostLog" }
        Yield-Focus
        $h = Get-Health
        # Started directly, SkyrimSE.exe may hand itself to Steam and come back as a new process.
        if ($h -and $h.pid -ne $Game.Id) { $script:Game = Get-Process -Id $h.pid -ErrorAction SilentlyContinue }
        $h -and $Game -and $h.pid -eq $Game.Id -and $h.lastLifecycle -eq "dataLoaded"
    } 180 "DevBench at the main menu"
    if ((Split-Path $Game.Path) -ne $GameDir.TrimEnd('\')) { throw "the game started from $($Game.Path), not $GameDir" }
    @{ runtime = $Runtime; exe = $Game.Path; fileVersion = (Get-Item $Game.Path).VersionInfo.FileVersion; pid = $Game.Id } | ConvertTo-Json | Out-File -Encoding utf8 "$OutDir\launch.json"
    if ($Foreground) { [void](New-Object -ComObject WScript.Shell).AppActivate($Game.Id) }
    if ($Replay) {
        if ($Replay -notmatch "[/\\]") { $Replay = "Data/SKSE/Plugins/devbench/recordings/$Replay" }
        $queued = Invoke-Tool record "{`"action`":`"replay`",`"path`":`"$Replay`",`"restoreScene`":true,`"closeMenus`":true,`"async`":true}"
        $status = Wait-Run record ($queued | ConvertFrom-Json).runId
    } else {
        # A steps file holding an array of step arrays runs as parts in the one game session, -Between (with the number
        # of the part just finished) between them; the parts' results are merged with the step indices continuing.
        $parsed = (Get-Content $Steps -Raw).Replace("{rt}", $Install.tag).Replace("{base}", $Install.baseSave) | ConvertFrom-Json
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
    if ($Game) { Stop-Process -Id $Game.Id -Force -ErrorAction SilentlyContinue; Wait-Process -Id $Game.Id -Timeout 15 -ErrorAction SilentlyContinue }
    # The game just stopped may still hold its logs.
    foreach ($f in "skse64.log", "devbench.log") {
        for ($attempt = 1; $attempt -le 10 -and (Test-Path "$SkseLogs\$f"); $attempt++) {
            try { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force; break }
            catch { if ($attempt -eq 10) { Write-Warning "could not keep ${f}: $($_.Exception.Message)" } else { Start-Sleep -Seconds 3 } }
        }
    }
    if ($tookLock) { & "$PSScriptRoot\game-free.ps1" -Release }
}
if (-not $ok) { exit 1 }
