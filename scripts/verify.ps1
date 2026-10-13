# Runs the skyrim-rs test plugin in the 1.7.104 game through DevBench (scripts/<suite>-steps.json), then compares the
# plugin's PASS lines with DevBench's own answers from the same session as scripts/<suite>-checks.ps1 lists them.
# The papyrus suite also builds the equip-counter mod, compiles both crates' papyrus folders and installs the mod DLL and
# the .pex files for the run; the shadows suite installs the first-person-shadows mod next to the test plugin.
# See docs/subsystems/<suite>.md.
param(
    [Parameter(Mandatory)][ValidateSet("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone")][string]$Suite,
    [string]$OutDir = "verify-$Suite-out",
    [string]$Mode = "",
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"
)
$ErrorActionPreference = "Stop"
$MyGames = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition"
$SkseLogs = "$MyGames\SKSE"
$Plugins = "$GameDir\Data\SKSE\Plugins"
$Scripts = "$GameDir\Data\Scripts"
$Root = Split-Path $PSScriptRoot -Parent
$Dll = "$Root\target\release\skyrim_rs_test.dll"
$Mods = @{
    papyrus = @{ crate = "equip-counter"; dll = "skyrim_rs_equip_counter.dll"; log = "skyrim-rs-equip-counter.log" }
    shadows = @{ crate = "first-person-shadows"; dll = "skyrim_rs_first_person_shadows.dll"; log = "skyrim-rs-first-person-shadows.log" }
    bone = @{ crate = "bone-driver"; dll = "skyrim_rs_bone_driver.dll"; log = "skyrim-rs-bone-driver.log" }
}
$Mod = $Mods[$Suite]
$PluginLog = "skyrim-rs-test.log"
$CrashLog = "skyrim-rs-test-crash.log"
$ModLogs = @($Mods.Values | ForEach-Object { $_.log }) + @($CrashLog)
$Logs = @("skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog) + $ModLogs
$TestSave = "skyrim-rs-$Suite"

if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
Push-Location $Root
$crates = @("testplugin") + @(if ($Mod) { $Mod.crate })
try { cargo build --release @($crates | ForEach-Object { "-p"; $_ }); if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } } finally { Pop-Location }
$Pex = @()
if ($Suite -eq "papyrus") { $Pex = @(& "$PSScriptRoot\compile-papyrus.ps1" -Source @("$Root\crates\testplugin\papyrus", "$Root\crates\equip-counter\papyrus")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
foreach ($f in @($PluginLog) + $ModLogs) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }

Copy-Item $Dll "$Plugins\skyrim_rs_test.dll" -Force
if ($Mode) { Set-Content "$Plugins\skyrim-rs-test.mode" $Mode -Encoding ascii }
if ($Mod) { Copy-Item "$Root\target\release\$($Mod.dll)" "$Plugins\$($Mod.dll)" -Force }
foreach ($p in $Pex) { if (Test-Path "$Scripts\$(Split-Path $p -Leaf)") { throw "$Scripts already holds $(Split-Path $p -Leaf)" }; Copy-Item $p $Scripts -Force }
$retraceOff = Test-Path "$Plugins\retrace.dll"
if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
try {
    & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\$Suite-steps.json" -OutDir $OutDir
    $scenarioOk = $LASTEXITCODE -eq 0
} finally {
    foreach ($f in @("skyrim_rs_test.dll", "skyrim-rs-test.mode") + @($Mods.Values | ForEach-Object { $_.dll })) { Remove-Item "$Plugins\$f" -ErrorAction SilentlyContinue }
    foreach ($p in $Pex) { Remove-Item "$Scripts\$(Split-Path $p -Leaf)" -ErrorAction SilentlyContinue }
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    Remove-Item "$MyGames\Saves\$TestSave*" -ErrorAction SilentlyContinue
    foreach ($f in @($PluginLog) + $ModLogs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force }
        elseif ($f -in @($PluginLog) + $ModLogs) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
    }
}

# The plugin's PASS/FAIL lines per "<suite> <phase>" run (the summary line closes a run) and, for the chosen suite, the last one per test.
$plugin = @{}
$runs = @{}
$pending = @{}
$summaries = @()
foreach ($line in Get-Content "$OutDir\$PluginLog") {
    if ($line -match '^(PASS|FAIL) (\w+): (.*)$') { $pending[$Matches[2]] = @{ ok = ($Matches[1] -eq "PASS"); detail = $Matches[3] } }
    elseif ($line -match '^summary (\w+ \w+): ') {
        $runs[$Matches[1]] = $pending
        if ($Matches[1] -like "$Suite *") { foreach ($k in $pending.Keys) { $plugin[$k] = $pending[$k] } }
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
