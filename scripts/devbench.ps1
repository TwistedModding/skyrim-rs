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

# A plugin that refuses the runtime may stop the game with a message box (class #32770) until someone clicks it; close
# every such box of the game, keeping its title and text in dialogs.txt, so an unattended run goes on.
Add-Type -Namespace DevBench -Name Win32 -MemberDefinition @'
public delegate bool EnumProc(System.IntPtr hwnd, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumChildWindows(System.IntPtr parent, EnumProc proc, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hwnd, System.Text.StringBuilder name, int max);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hwnd, System.Text.StringBuilder text, int max);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool PostMessage(System.IntPtr hwnd, uint msg, System.IntPtr w, System.IntPtr l);
'@
function Close-Dialogs {
    $ids = @(Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
    $found = New-Object System.Collections.ArrayList
    [void][DevBench.Win32]::EnumWindows({ param($hwnd, $l)
        $procId = [uint32]0
        [void][DevBench.Win32]::GetWindowThreadProcessId($hwnd, [ref]$procId)
        $class = New-Object System.Text.StringBuilder 64
        [void][DevBench.Win32]::GetClassName($hwnd, $class, 64)
        if ($procId -in $ids -and $class.ToString() -eq "#32770") { [void]$found.Add($hwnd) }
        $true }, [IntPtr]::Zero)
    foreach ($hwnd in $found) {
        $texts = New-Object System.Collections.ArrayList
        $title = New-Object System.Text.StringBuilder 512
        [void][DevBench.Win32]::GetWindowText($hwnd, $title, 512)
        [void][DevBench.Win32]::EnumChildWindows($hwnd, { param($child, $l)
            $t = New-Object System.Text.StringBuilder 2048
            [void][DevBench.Win32]::GetWindowText($child, $t, 2048)
            if ($t.Length) { [void]$texts.Add($t.ToString()) }
            $true }, [IntPtr]::Zero)
        $line = "closed a message box titled '$title': $(($texts -join ' | ') -replace '\s+', ' ')"
        Write-Warning $line
        $line | Out-File -Append -Encoding utf8 "$OutDir\dialogs.txt"
        [void][DevBench.Win32]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    }
}

function Wait-Until([scriptblock]$Cond, [int]$Seconds, [string]$What) {
    $t0 = Get-Date
    while (-not (& $Cond)) {
        Close-Dialogs
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
            Close-Dialogs
            Write-Warning "game paused at frame $lastFrame; re-activating its window"
            [void]$Shell.AppActivate($Game.Id)
        }
        if ($h) { $lastFrame = $h.frame }
        Start-Sleep -Seconds 2
    }
}

if (-not $Replay -and -not $Steps) { throw "give -Replay <recording> or -Steps <steps.json>" }
& "$PSScriptRoot\game-free.ps1"
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
}
if (-not $ok) { exit 1 }
