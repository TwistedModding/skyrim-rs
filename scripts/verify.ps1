# Runs the skyrim-rs test plugin in the 1.7.104 game through DevBench (scripts/<suite>-steps.json), then compares the
# plugin's PASS lines with DevBench's own answers from the same session as scripts/<suite>-checks.ps1 lists them.
# See docs/subsystems/<suite>.md.
param(
    [Parameter(Mandatory)][ValidateSet("forms", "actors", "events")][string]$Suite,
    [string]$OutDir = "verify-$Suite-out",
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"
)
$ErrorActionPreference = "Stop"
$MyGames = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition"
$SkseLogs = "$MyGames\SKSE"
$Plugins = "$GameDir\Data\SKSE\Plugins"
$Root = Split-Path $PSScriptRoot -Parent
$Dll = "$Root\target\release\skyrim_rs_test.dll"
$PluginLog = "skyrim-rs-test.log"
$Logs = "skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog
$TestSave = "skyrim-rs-$Suite"

if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
Push-Location $Root
try { cargo build --release -p testplugin; if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } } finally { Pop-Location }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
Remove-Item "$SkseLogs\$PluginLog" -ErrorAction SilentlyContinue

Copy-Item $Dll "$Plugins\skyrim_rs_test.dll" -Force
$retraceOff = Test-Path "$Plugins\retrace.dll"
if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
try {
    & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\$Suite-steps.json" -OutDir $OutDir
    $scenarioOk = $LASTEXITCODE -eq 0
} finally {
    Remove-Item "$Plugins\skyrim_rs_test.dll" -ErrorAction SilentlyContinue
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    Remove-Item "$MyGames\Saves\$TestSave.*" -ErrorAction SilentlyContinue
    if (Test-Path "$SkseLogs\$PluginLog") { Copy-Item "$SkseLogs\$PluginLog" "$OutDir\$PluginLog" -Force }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force }
        elseif ($f -eq $PluginLog) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
    }
}

# The plugin's PASS/FAIL lines per "<suite> <phase>" run (the summary line closes a run) and the last one per test.
$plugin = @{}
$runs = @{}
$pending = @{}
$summaries = @()
foreach ($line in Get-Content "$OutDir\$PluginLog") {
    if ($line -match '^(PASS|FAIL) (\w+): (.*)$') { $pending[$Matches[2]] = @{ ok = ($Matches[1] -eq "PASS"); detail = $Matches[3] } }
    elseif ($line -match '^summary (\w+ \w+): ') {
        $runs[$Matches[1]] = $pending
        foreach ($k in $pending.Keys) { $plugin[$k] = $pending[$k] }
        $pending = @{}
        $summaries += $line
    }
}
$steps = ((Get-Content "$OutDir\result.json" -Raw | ConvertFrom-Json).result.results)
function Step($i) { $s = $steps | Where-Object { $_.index -eq $i }; if ($s -and $s.ok) { $s.result } else { $null } }
function Returned($i) { $r = Step $i; if ($r) { $r.returned } else { $null } }
function Lines($i) { $r = Step $i; if ($r) { $r.lines -join " | " } else { "" } }
function Detail($name) { if ($plugin[$name]) { $plugin[$name].detail } else { "" } }
function RunDetail($run, $name) { if ($runs[$run] -and $runs[$run][$name]) { $runs[$run][$name].detail } else { "" } }
function Captured($text, $pattern) { if ($text -match $pattern) { $Matches[1] } else { $null } }
function Near($a, $b, $tol) { ($a -ne $null) -and ($b -ne $null) -and ([math]::Abs([double]$a - [double]$b) -lt $tol) }

. "$PSScriptRoot\$Suite-checks.ps1"

""
"plugin summary: $($summaries -join '; ')"
"scenario ok: $scenarioOk"
""
$allOk = $scenarioOk
foreach ($phase in $Phases) {
    if (($summaries -match "^summary $Suite $phase`: \d+ pass, 0 fail").Count -ne 1) { "no clean $Suite run in phase $phase"; $allOk = $false }
}
foreach ($r in $rows) {
    $verdict = if ($r.ok) { "OK  " } else { "DIFF"; $allOk = $false }
    "$verdict $($r.name)"
    "     plugin:   $($r.plugin)"
    "     devbench: $($r.devbench)"
}
foreach ($name in ($plugin.Keys | Sort-Object)) { if (-not $plugin[$name].ok) { "FAIL $name`: $($plugin[$name].detail)"; $allOk = $false } }
""
"logs in $OutDir"
if (-not $allOk) { exit 1 }
