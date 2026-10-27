# Runs the skyrim-rs test plugin in the 1.7.104 game through DevBench (scripts/<suite>-steps.json), then compares the
# plugin's PASS lines with DevBench's own answers from the same session as scripts/<suite>-checks.ps1 lists them.
# The papyrus suite also builds the equip-counter mod, compiles both crates' papyrus folders and installs the mod DLL and
# the .pex files for the run; the shadows suite installs the first-person-shadows mod next to the test plugin.
# The overlay suite runs the plugin in mode overlay. The framework suite installs the host (skyrim_rs.dll) and puts its
# test plugin, the four order-* plugins and a decoy DLL without a descriptor into Data\SKSE\Plugins\skyrim-rs; the test
# plugin writes the suite's report. The hooks suite installs the host with its two hook plugins, which share one report
# log. The reload suite builds its plugin three more times with the build tags 2, 3 and 4 before the game starts, installs
# the host with the tag 1 build, and between the parts of its scenario copies the next tag's DLL over the installed one
# and waits for the host to report the reload. The compat suite runs the hooks scenario with the hooks plugins and two
# plain SKSE plugins of its own that hook the same functions (legacy-patch-a before the host by file name,
# legacy-patch-z after it), with skyrim-rs.ini setting the host's compatibility mode to -Compat. The legacy suite copies
# the third-party SKSE plugins it lists from -LegacyDir into Data\SKSE\Plugins for the run, next to the host. The gui
# suite installs the host with the example mod and the two hook plugins, copies the example's build in again between its
# two parts (a reload) and keeps the settings files the host wrote. The camera suite installs the camera-demo plugin,
# drives it through its console command and reads its report log. The crash suite installs the host with its crash-test
# plugin and runs one game session per scenario (null, hook, overflow, write, form, deleted, script), each ending in the crash the plugin's
# console command armed; the host's crash log and minidump of each session go to <OutDir>\<scenario>. Any suite's run
# that leaves a skyrim-rs-crash-* file in the SKSE folder has it moved into <OutDir>. The sample suite installs the sample
# plugin of docs/MODS.md and compares its log with DevBench. See docs/subsystems/<suite>.md.
param(
    [Parameter(Mandatory)][ValidateSet("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone", "ui", "input", "save", "audio", "overlay", "framework", "hooks", "reload", "compat", "legacy", "gui", "camera", "crash", "sample")][string]$Suite,
    [string]$OutDir = "verify-$Suite-out",
    [string]$Mode = "",
    [ValidateSet("refuse", "stack")][string]$Compat = "refuse",
    # skse: the script extender alone; skse-proxy: the standalone host's dinput8.dll next to the game as well, which must
    # leave the script extender in charge; standalone: dinput8.dll and the game started without the script extender.
    [ValidateSet("skse", "skse-proxy", "standalone")][string]$Launch = "skse",
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition",
    # The legacy suite's plugins, one folder per plugin holding its SKSE\Plugins files as the mod ships them.
    [string]$LegacyDir = $(if ($env:SKYRIM_RS_LEGACY_DIR) { $env:SKYRIM_RS_LEGACY_DIR } else { "$env:LOCALAPPDATA\skyrim-rs\legacy" }),
    # Runs the legacy suite with these plugins instead of its list, to try a candidate alone.
    [string[]]$Legacy = @(),
    # Compares the logs an earlier run left in -OutDir again, without building, installing or launching.
    [switch]$CheckOnly,
    # Runs only these scenarios of a scenario suite (the crash suite), to retry one.
    [string[]]$Scenarios = @()
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
    camera = @{ crate = "camera-demo"; dll = "skyrim_rs_camera_demo.dll"; log = "skyrim-rs-camera-demo.log"; report = $true }
    sample = @{ crate = "sample-plugin"; dll = "skyrim_rs_sample.dll"; log = "skyrim-rs-sample.log"; report = $true }
    framework = @{ crate = "framework-test"; dll = "skyrim_rs_framework_test.dll"; log = "skyrim-rs-framework-test.log"; dir = "skyrim-rs"; report = $true;
                   crates = @("order-head", "order-tail", "order-cycle-a", "order-cycle-b")
                   dlls = @("skyrim_rs_order_head.dll", "skyrim_rs_order_tail.dll", "skyrim_rs_order_cycle_a.dll", "skyrim_rs_order_cycle_b.dll") }
    hooks = @{ crate = "hooks-test-a"; dll = "skyrim_rs_hooks_a.dll"; log = "skyrim-rs-hooks.log"; dir = "skyrim-rs"; report = $true;
               crates = @("hooks-test-b"); dlls = @("skyrim_rs_hooks_b.dll") }
    reload = @{ crate = "reload-test"; dll = "skyrim_rs_reload_test.dll"; log = "skyrim-rs-reload-test.log"; dir = "skyrim-rs"; report = $true; tags = @(2, 3, 4) }
    compat = @{ crate = "hooks-test-a"; dll = "skyrim_rs_hooks_a.dll"; log = "skyrim-rs-hooks.log"; dir = "skyrim-rs"; report = $true; steps = "hooks";
                crates = @("hooks-test-b", "legacy-patch-a", "legacy-patch-z"); dlls = @("skyrim_rs_hooks_b.dll");
                plugins = @("skyrim_legacy_patch_a.dll", "skyrim_rs_legacy_patch_z.dll"); logs = @("skyrim-rs-legacy-patch-a.log", "skyrim-rs-legacy-patch-z.log") }
    # Third-party plugins built for 1.7.x on the Address Library, each <name>.dll in $LegacyDir\<name>\SKSE\Plugins
    # (DevBench is the other one; it is installed for every suite).
    legacy = @{ dir = "skyrim-rs"; legacy = @("CrashLogger") }
    gui = @{ crate = "example-mod"; dll = "skyrim_rs_example_mod.dll"; log = "skyrim-rs-example.log"; dir = "skyrim-rs"; report = $true; tags = @(2);
             crates = @("hooks-test-a", "hooks-test-b"); dlls = @("skyrim_rs_hooks_a.dll", "skyrim_rs_hooks_b.dll") }
    crash = @{ crate = "crash-test"; dll = "skyrim_rs_crash_test.dll"; log = "skyrim-rs-crash-test.log"; dir = "skyrim-rs"; report = $true;
               scenarios = @("null", "hook", "overflow", "write", "form", "deleted", "script")
               # The step that triggers each armed crash, as the scenario's JSON.
               triggers = @{ null = '{"tool": "console", "args": {"command": "save skyrim-rs-crash"}}'; overflow = '{"tool": "console", "args": {"command": "save skyrim-rs-crash"}}'; write = '{"tool": "console", "args": {"command": "save skyrim-rs-crash"}}'
                             hook = '{"tool": "console", "args": {"command": "player.damageactorvalue health 1"}}'; form = '{"tool": "console", "args": {"command": "save skyrim-rs-crash"}}'; deleted = '{"tool": "console", "args": {"command": "save skyrim-rs-crash"}}'
                             script = '{"tool": "papyrus", "args": {"action": "call", "script": "SkyrimRsCrashTest", "function": "CrashFromScript"}}' } }
}
$TagDir = "$Root\target\release\reload-tags"
$Mod = $Mods[$Suite]
if ($Suite -eq "legacy" -and $Legacy) { $Mod.legacy = @($Legacy -split ",") }
if ($Mod.scenarios -and $Scenarios) { $Mod.scenarios = @($Scenarios -split ",") }
$PluginLog = "skyrim-rs-test.log"
$HostDll = "skyrim_rs.dll"
$ModLogs = @($Mods.Values | ForEach-Object { $_.log; $_.logs } | Where-Object { $_ }) + @("skyrim-rs.log", "skyrim-rs-standalone.log", "skyrim-rs-order.log")
$Proxy = "$GameDir\dinput8.dll"
$Logs = @("skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog) + $ModLogs
$StepsName = if ($Mod.steps) { $Mod.steps } else { $Suite }
$TestSave = "skyrim-rs-$StepsName"
$Ini = "$Plugins\skyrim-rs.ini"

if ($Suite -eq "overlay") { $Mode = "overlay" }
if (-not $CheckOnly) {
& "$PSScriptRoot\game-free.ps1"
Push-Location $Root
$crates = @(@("testplugin") + @(if ($Mod) { $Mod.crate }) + @(if ($Mod.crates) { $Mod.crates }) + @(if ($Mod.dir) { "host" }) + @(if ($Launch -ne "skse") { "standalone" }) | Where-Object { $_ })
try {
    # The tagged builds come first: the untagged build of the main step is the generation installed at launch.
    if ($Mod.tags) {
        New-Item -ItemType Directory -Force $TagDir | Out-Null
        foreach ($t in $Mod.tags) {
            $env:SKYRIM_RS_RELOAD_TAG = "$t"
            # The last tag enables an sre feature the host lacks, so its crates hash differently from the host's.
            $features = if ($t -eq $Mod.tags[-1]) { @("--features", "sre/tier-low") } else { @() }
            try { cargo build --release -p $Mod.crate @features; if ($LASTEXITCODE -ne 0) { throw "cargo build failed for tag $t" } } finally { Remove-Item env:SKYRIM_RS_RELOAD_TAG }
            Copy-Item "$Root\target\release\$($Mod.dll)" "$TagDir\$t.dll" -Force
        }
    }
    cargo build --release @($crates | ForEach-Object { "-p"; $_ }); if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }
$Pex = @()
if ($Suite -eq "papyrus") { $Pex = @(& "$PSScriptRoot\compile-papyrus.ps1" -Source @("$Root\crates\testplugin\papyrus", "$Root\crates\equip-counter\papyrus")) }
if ($Suite -in @("framework", "reload", "gui", "crash")) { $Pex = @(& "$PSScriptRoot\compile-papyrus.ps1" -Source @("$Root\crates\$($Mod.crate)\papyrus")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
foreach ($f in @($PluginLog) + $ModLogs) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
# Other logs the legacy plugins may write next to ours: kept as they were, the run's copies collected afterwards.
$OtherLogs = @{}
if ($Mod.legacy) {
    New-Item -ItemType Directory -Force "$Backup\other" | Out-Null
    foreach ($f in Get-ChildItem $SkseLogs -File | Where-Object { $_.Name -notin $Logs }) { $OtherLogs[$f.Name] = $f.LastWriteTime; Copy-Item $f.FullName "$Backup\other\$($f.Name)" -Force }
}

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
    # A plugin built with its PDB (the crash suite's) gets it next to the DLL, for the crash log's names.
    foreach ($d in @($Mod.dll) + @(if ($Mod.dlls) { $Mod.dlls })) { if ($d) { Copy-Item "$Root\target\release\$d" $ModDir; $pdb = "$Root\target\release\$([IO.Path]::ChangeExtension($d, 'pdb'))"; if (Test-Path $pdb) { Copy-Item $pdb $ModDir } } }
    Copy-Item $Dll "$ModDir\decoy.dll"
    Copy-Item "$Root\target\release\$HostDll" "$Plugins\$HostDll" -Force
} elseif ($Mod) { Copy-Item "$Root\target\release\$($Mod.dll)" "$Plugins\$($Mod.dll)" -Force }
foreach ($d in @($Mod.plugins)) { if ($d) { Copy-Item "$Root\target\release\$d" "$Plugins\$d" -Force } }
$LegacyItems = @()
$LegacyMissing = @()
$PluginsBefore = @(Get-ChildItem $Plugins -Name)
foreach ($name in @($Mod.legacy)) {
    if (-not $name) { continue }
    $tree = "$LegacyDir\$name\SKSE\Plugins"
    if (-not (Test-Path "$tree\$name.dll")) { "legacy plugin $name.dll not found in $LegacyDir\$name\SKSE\Plugins; the suite runs without it"; $LegacyMissing += $name; continue }
    foreach ($item in Get-ChildItem $tree) {
        $to = "$Plugins\$($item.Name)"
        if (Test-Path $to) { throw "$to already exists" }
        Copy-Item $item.FullName $to -Recurse
        $LegacyItems += $to
    }
}
if ($Suite -eq "compat") {
    if (Test-Path $Ini) { throw "$Ini already exists" }
    Set-Content $Ini "[hooks]`r`ncompat = $Compat" -Encoding ascii
}
# After part N of the scenario: the next tag's build replaces the installed DLL, as an author's copy would, and the host
# (polling from the frame, so the game window must stay in front) must report that generation loaded within 90 s.
$Between = {
    param($part)
    $tag = $Mod.tags[$part - 1]
    Copy-Item "$TagDir\$tag.dll" "$ModDir\$($Mod.dll)" -Force
    # A build that did not change the code keeps the file's time; the host watches size and time.
    (Get-Item "$ModDir\$($Mod.dll)").LastWriteTime = Get-Date
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
$DevBenchLaunch = if ($Launch -eq "standalone") { "standalone" } else { "skse" }
try {
    if ($Mod.scenarios) {
        # One game session per scenario, each ending in the crash it armed: devbench.ps1 reports the dead game as a
        # failure, and what matters is the crash log and dump the host's watchdog left in the SKSE folder.
        $scenarioOk = $true
        foreach ($s in $Mod.scenarios) {
            $runDir = "$OutDir\$s"
            New-Item -ItemType Directory -Force $runDir | Out-Null
            (Get-Content "$PSScriptRoot\$StepsName-steps.json" -Raw).Replace("{scenario}", $s).Replace("{trigger}", $Mod.triggers[$s]) | Set-Content "$runDir\steps.json" -Encoding ascii
            try { & "$PSScriptRoot\devbench.ps1" -Steps "$runDir\steps.json" -OutDir $runDir -Launch $DevBenchLaunch } catch { "scenario $s`: the game ended: $($_.Exception.Message)" }
            foreach ($f in @("skyrim-rs.log", $Mod.log)) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$runDir\$f" -Force } }
            Get-ChildItem "$SkseLogs" -Filter "skyrim-rs-crash-????????-??????.*" | Move-Item -Destination $runDir -Force
            if (-not (Get-ChildItem $runDir -Filter "skyrim-rs-crash-????????-??????.log")) { "scenario $s`: no crash log"; $scenarioOk = $false }
        }
    } else {
        & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\$StepsName-steps.json" -Between $(if ($Mod.tags) { $Between }) -OutDir $OutDir -Launch $DevBenchLaunch
        $scenarioOk = $LASTEXITCODE -eq 0
    }
} finally {
    Get-ChildItem "$SkseLogs" -Filter "skyrim-rs-crash-????????-??????.*" | Move-Item -Destination $OutDir -Force
    if ($Mod.dir) { Get-ChildItem "$Plugins\$($Mod.dir)" -Filter "*.settings.json" -ErrorAction SilentlyContinue | Copy-Item -Destination $OutDir -Force }
    # A game that crashed may still hold the installed DLLs for a few seconds after Stop-Process.
    for ($try = 0; $try -lt 10; $try++) {
        foreach ($f in @("skyrim_rs_test.dll", "skyrim-rs-test.mode", $HostDll) + @($Mods.Values | ForEach-Object { $_.dll }) + @($Mod.plugins)) { if ($f) { Remove-Item "$Plugins\$f" -ErrorAction SilentlyContinue } }
        if ($Mod.dir) { Remove-Item "$Plugins\$($Mod.dir)" -Recurse -Force -ErrorAction SilentlyContinue }
        if (-not (Test-Path "$Plugins\$HostDll") -and -not ($Mod.dir -and (Test-Path "$Plugins\$($Mod.dir)"))) { break }
        Start-Sleep -Seconds 3
    }
    if ($Suite -eq "compat") { Remove-Item $Ini -ErrorAction SilentlyContinue }
    foreach ($item in $LegacyItems) { Remove-Item $item -Recurse -Force -ErrorAction SilentlyContinue }
    if ($Launch -ne "skse") { Remove-Item $Proxy -ErrorAction SilentlyContinue }
    if ($Launch -eq "standalone") { Remove-Item $AppId -ErrorAction SilentlyContinue }
    foreach ($p in $Pex) { Remove-Item "$Scripts\$(Split-Path $p -Leaf)" -ErrorAction SilentlyContinue }
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    if ($Mod.legacy) {
        foreach ($item in Get-ChildItem $Plugins -Name | Where-Object { $_ -notin $PluginsBefore -and $_ -notlike "retrace.dll*" }) { "removing $item, left in Data\SKSE\Plugins by the run"; Remove-Item "$Plugins\$item" -Recurse -Force }
        New-Item -ItemType Directory -Force "$OutDir\legacy-logs" | Out-Null
        foreach ($f in Get-ChildItem $SkseLogs -File | Where-Object { $_.Name -notin $Logs -and (-not $OtherLogs.ContainsKey($_.Name) -or $_.LastWriteTime -ne $OtherLogs[$_.Name]) }) {
            Copy-Item $f.FullName "$OutDir\legacy-logs\$($f.Name)" -Force
            if ($OtherLogs.ContainsKey($f.Name)) { Copy-Item "$Backup\other\$($f.Name)" $f.FullName -Force } else { Remove-Item $f.FullName }
        }
    }
    Remove-Item "$MyGames\Saves\$TestSave*" -ErrorAction SilentlyContinue
    foreach ($f in @($PluginLog) + $ModLogs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force }
        elseif ($f -in @($PluginLog) + $ModLogs) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
    }
}
}
$OutDir = (Resolve-Path $OutDir).Path

# The plugin's PASS/FAIL lines per "<suite> <phase>" run (the summary line closes a run) and, for the chosen suite, the last one per test.
$plugin = @{}
$runs = @{}
$pending = @{}
$summaries = @()
$ReportLog = if ($Mod.report) { $Mod.log } else { $PluginLog }
$ReportLines = @(Get-Content "$OutDir\$ReportLog")
foreach ($line in $ReportLines) {
    if ($line -match '^(PASS|FAIL) (\w+): (.*)$') { $pending[$Matches[2]] = @{ ok = ($Matches[1] -eq "PASS"); detail = $Matches[3] } }
    elseif ($line -match '^summary (\w+ \w+): ') {
        $runs[$Matches[1]] = $pending
        if ($Matches[1] -like "$Suite *") { foreach ($k in $pending.Keys) { $plugin[$k] = $pending[$k] } }
        $pending = @{}
        $summaries += $line
    }
}
# A scenario suite's sessions keep their own result.json per scenario folder.
$steps = @(if (Test-Path "$OutDir\result.json") { (Get-Content "$OutDir\result.json" -Raw | ConvertFrom-Json).result.results })
function Step($i) { $s = $steps | Where-Object { $_.index -eq $i }; if ($s -and $s.ok) { $s.result } else { $null } }
function Returned($i) { $r = Step $i; if ($r) { $r.returned } else { $null } }
function Lines($i) { $r = Step $i; if ($r) { $r.lines -join " | " } else { "" } }
function Detail($name) { if ($plugin[$name]) { $plugin[$name].detail } else { "" } }
function RunDetail($run, $name) { if ($runs[$run] -and $runs[$run][$name]) { $runs[$run][$name].detail } else { "" } }
function Captured($text, $pattern) { if ($text -match $pattern) { $Matches[1] } else { $null } }
# The SKSE plugin loader of this launch, its log and the handle it gave a DLL; the host names the same loader in its log.
$HostedBy = if ($Launch -eq "standalone") { "the standalone loader" } else { "the script extender" }
$LoaderLogName = if ($Launch -eq "standalone") { "skyrim-rs-standalone.log" } else { "skse64.log" }
$LoaderLog = @(if (Test-Path "$OutDir\$LoaderLogName") { Get-Content "$OutDir\$LoaderLogName" })
$HostLog = @(if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" })
function Loaded($file) {
    $f = [regex]::Escape($file)
    $pattern = if ($Launch -eq "standalone") { "^$f`: loaded as handle (\d+)" } else { "^plugin $f \(.*\) loaded correctly \(handle (\d+)\)" }
    foreach ($l in $LoaderLog) { if ($l -match $pattern) { return [int]$Matches[1] } }
    $null
}
function Hosted { @($HostLog | Where-Object { $_ -match "^hosted by $HostedBy, " }).Count -eq 1 }
function Near($a, $b, $tol) { ($a -ne $null) -and ($b -ne $null) -and ([math]::Abs([double]$a - [double]$b) -lt $tol) }

. "$PSScriptRoot\$Suite-checks.ps1"

""
"plugin summary: $($summaries -join '; ')"
"scenario ok: $scenarioOk"
""
$allOk = $scenarioOk -ne $false
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
