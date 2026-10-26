# Runs the skyrim-rs test plugin in the 1.7.104 game through DevBench (scripts/<suite>-steps.json), then compares the
# plugin's PASS lines with DevBench's own answers from the same session as scripts/<suite>-checks.ps1 lists them.
# The papyrus suite also builds the equip-counter mod, compiles both crates' papyrus folders and installs the mod DLL and
# the .pex files for the run; the shadows suite installs the first-person-shadows mod next to the test plugin.
# The overlay suite runs the plugin in mode overlay. The framework suite installs the host (skyrim_rs.dll) and puts its
# test plugin, the four order-* plugins and a decoy DLL without a descriptor into Data\SKSE\Plugins\skyrim-rs; the test
# plugin writes the suite's report. The hooks suite installs the host with its two hook plugins, which share one report
# log. The reload suite builds its plugin three more times with the build tags 2, 3 and 4 before the game starts, installs
# the host with the tag 1 build, and between the parts of its scenario copies the next tag's DLL over the installed one
# and waits for the host to report the reload. See docs/subsystems/<suite>.md.
param(
    [Parameter(Mandatory)][ValidateSet("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone", "ui", "input", "save", "audio", "overlay", "framework", "hooks", "reload")][string]$Suite,
    [string]$OutDir = "verify-$Suite-out",
    [string]$Mode = "",
    # skse: the script extender alone; skse-proxy: the standalone host's dinput8.dll next to the game as well, which must
    # leave the script extender in charge; standalone: dinput8.dll and the game started without the script extender.
    [ValidateSet("skse", "skse-proxy", "standalone")][string]$Launch = "skse",
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
    framework = @{ crate = "framework-test"; dll = "skyrim_rs_framework_test.dll"; log = "skyrim-rs-framework-test.log"; dir = "skyrim-rs"; report = $true;
                   crates = @("order-head", "order-tail", "order-cycle-a", "order-cycle-b")
                   dlls = @("skyrim_rs_order_head.dll", "skyrim_rs_order_tail.dll", "skyrim_rs_order_cycle_a.dll", "skyrim_rs_order_cycle_b.dll") }
    hooks = @{ crate = "hooks-test-a"; dll = "skyrim_rs_hooks_a.dll"; log = "skyrim-rs-hooks.log"; dir = "skyrim-rs"; report = $true;
               crates = @("hooks-test-b"); dlls = @("skyrim_rs_hooks_b.dll") }
    reload = @{ crate = "reload-test"; dll = "skyrim_rs_reload_test.dll"; log = "skyrim-rs-reload-test.log"; dir = "skyrim-rs"; report = $true; tags = @(2, 3, 4) }
}
$TagDir = "$Root\target\release\reload-tags"
$Mod = $Mods[$Suite]
$PluginLog = "skyrim-rs-test.log"
$CrashLog = "skyrim-rs-test-crash.log"
$HostDll = "skyrim_rs.dll"
$ModLogs = @($Mods.Values | ForEach-Object { $_.log }) + @($CrashLog, "skyrim-rs.log", "skyrim-rs-standalone.log", "skyrim-rs-order.log")
$Proxy = "$GameDir\dinput8.dll"
$Logs = @("skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog) + $ModLogs
$TestSave = "skyrim-rs-$Suite"

if ($Suite -eq "overlay") { $Mode = "overlay" }
& "$PSScriptRoot\game-free.ps1"
Push-Location $Root
$crates = @("testplugin") + @(if ($Mod) { $Mod.crate }) + @(if ($Mod.crates) { $Mod.crates }) + @(if ($Mod.dir) { "host" }) + @(if ($Launch -ne "skse") { "standalone" })
try {
    # The tagged builds come first: the untagged build of the main step is the generation installed at launch.
    if ($Mod.tags) {
        New-Item -ItemType Directory -Force $TagDir | Out-Null
        foreach ($t in $Mod.tags) {
            $env:SKYRIM_RS_RELOAD_TAG = "$t"
            try { cargo build --release -p $Mod.crate; if ($LASTEXITCODE -ne 0) { throw "cargo build failed for tag $t" } } finally { Remove-Item env:SKYRIM_RS_RELOAD_TAG }
            Copy-Item "$Root\target\release\$($Mod.dll)" "$TagDir\$t.dll" -Force
        }
    }
    cargo build --release @($crates | ForEach-Object { "-p"; $_ }); if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }
$Pex = @()
if ($Suite -eq "papyrus") { $Pex = @(& "$PSScriptRoot\compile-papyrus.ps1" -Source @("$Root\crates\testplugin\papyrus", "$Root\crates\equip-counter\papyrus")) }
if ($Suite -in @("framework", "reload")) { $Pex = @(& "$PSScriptRoot\compile-papyrus.ps1" -Source @("$Root\crates\$($Mod.crate)\papyrus")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
foreach ($f in @($PluginLog) + $ModLogs) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }

if ($Launch -ne "skse") {
    if (Test-Path $Proxy) { throw "$Proxy already exists" }
    Copy-Item "$Root\target\release\dinput8.dll" $Proxy
}
# Started directly, the game asks Steam to relaunch it through its launcher unless it finds its app id next to it.
$AppId = "$GameDir\steam_appid.txt"
if ($Launch -eq "standalone") {
    if (Test-Path $AppId) { throw "$AppId already exists" }
    Set-Content $AppId "489830" -Encoding ascii -NoNewline
}
# Another run's install (its game not started yet) must not be touched, and nothing is copied before that is known.
if ($Mod.dir -and (Test-Path "$Plugins\$($Mod.dir)")) { throw "$Plugins\$($Mod.dir) already exists" }
Copy-Item $Dll "$Plugins\skyrim_rs_test.dll" -Force
if ($Mode) { Set-Content "$Plugins\skyrim-rs-test.mode" $Mode -Encoding ascii }
if ($Mod.dir) {
    $ModDir = "$Plugins\$($Mod.dir)"
    New-Item -ItemType Directory $ModDir | Out-Null
    foreach ($d in @($Mod.dll) + @(if ($Mod.dlls) { $Mod.dlls })) { Copy-Item "$Root\target\release\$d" $ModDir }
    Copy-Item $Dll "$ModDir\decoy.dll"
    Copy-Item "$Root\target\release\$HostDll" "$Plugins\$HostDll" -Force
} elseif ($Mod) { Copy-Item "$Root\target\release\$($Mod.dll)" "$Plugins\$($Mod.dll)" -Force }
# After part N of the scenario: the next tag's build replaces the installed DLL, as an author's copy would, and the host
# (polling from the frame, so the game window must stay in front) must report that generation loaded within 90 s.
$Between = {
    param($part)
    $tag = $Mod.tags[$part - 1]
    Copy-Item "$TagDir\$tag.dll" "$ModDir\$($Mod.dll)" -Force
    $shell = New-Object -ComObject WScript.Shell
    $t0 = Get-Date
    while (-not (Select-String -Path "$SkseLogs\skyrim-rs.log" -Pattern "generation $tag loaded" -Quiet)) {
        if (((Get-Date) - $t0).TotalSeconds -gt 90) { throw "the host did not report generation $tag within 90 s" }
        $game = Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($game) { [void]$shell.AppActivate($game.Id) }
        Start-Sleep -Seconds 2
    }
    Start-Sleep -Seconds 2
}.GetNewClosure()
foreach ($p in $Pex) { if (Test-Path "$Scripts\$(Split-Path $p -Leaf)") { throw "$Scripts already holds $(Split-Path $p -Leaf)" }; Copy-Item $p $Scripts -Force }
$retraceOff = Test-Path "$Plugins\retrace.dll"
if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
try {
    & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\$Suite-steps.json" -Between $(if ($Mod.tags) { $Between }) -OutDir $OutDir -Launch $(if ($Launch -eq "standalone") { "standalone" } else { "skse" })
    $scenarioOk = $LASTEXITCODE -eq 0
} finally {
    # A game that crashed may still hold the installed DLLs for a few seconds after Stop-Process.
    for ($try = 0; $try -lt 10; $try++) {
        foreach ($f in @("skyrim_rs_test.dll", "skyrim-rs-test.mode", $HostDll) + @($Mods.Values | ForEach-Object { $_.dll })) { Remove-Item "$Plugins\$f" -ErrorAction SilentlyContinue }
        if ($Mod.dir) { Remove-Item "$Plugins\$($Mod.dir)" -Recurse -Force -ErrorAction SilentlyContinue }
        if (-not (Test-Path "$Plugins\$HostDll") -and -not ($Mod.dir -and (Test-Path "$Plugins\$($Mod.dir)"))) { break }
        Start-Sleep -Seconds 3
    }
    if ($Launch -ne "skse") { Remove-Item $Proxy -ErrorAction SilentlyContinue }
    if ($Launch -eq "standalone") { Remove-Item $AppId -ErrorAction SilentlyContinue }
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
$ReportLog = if ($Mod.report) { $Mod.log } else { $PluginLog }
foreach ($line in Get-Content "$OutDir\$ReportLog") {
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
