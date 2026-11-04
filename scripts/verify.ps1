# Runs the skyrim-rs test plugin in the game of -Runtime through DevBench (scripts/<suite>-steps.json), then compares the
# plugin's PASS lines with DevBench's own answers from the same session as scripts/<suite>-checks.ps1 lists them, and
# writes <OutDir>\results.json: the runtime, suite, plugin build commit and the pass, fail and not-run counts. Exit code
# 0 passed, 1 failed, 2 the loader refused a plugin of the suite (nothing to check, all not run), 3 the suite cannot run
# on this runtime, 4 the install differs from before the run or a cleanup step failed, whatever the suite did. The
# runtime comes from scripts/runtimes.ps1; the SKSE in the install is the one used. Under the game lock the owner's
# permanent install moves aside for the run (scripts/owner-install.ps1), which stops if a dead run left one aside.
# The papyrus suite also builds the equip-counter mod, compiles both crates' papyrus folders and installs the mod DLL and
# the .pex files for the run; the shadows suite installs the first-person-shadows mod next to the test plugin.
# The overlay suite runs the plugin in mode overlay. The framework suite installs the host (skyrim_runtime_extender.dll) and puts its
# test plugin, the four order-* plugins and a decoy DLL without a descriptor into Data\SKSE\Plugins\skyrim-runtime-extender; the test
# plugin writes the suite's report. The hooks suite installs the host with its two hook plugins, which share one report
# log. The reload suite builds its plugin three more times with the build tags 2, 3 and 4 before the game starts, installs
# the host with the tag 1 build, and between the parts of its scenario copies the next tag's DLL over the installed one
# and waits for the host to report the reload. The compat suite runs the hooks scenario with the hooks plugins and two
# plain SKSE plugins of its own that hook the same functions (legacy-patch-a before the host by file name,
# legacy-patch-z after it), with skyrim-runtime-extender.ini setting the host's compatibility mode to -Compat. The legacy suite copies
# the third-party SKSE plugins it lists from -LegacyDir into Data\SKSE\Plugins for the run, next to the host. The gui
# suite installs the host with the example mod and the two hook plugins, captures each page of the host's window, copies
# the example's build in again between its two parts (a reload) and keeps the settings files the host wrote; then it
# launches the game twice more on the same
# install (its sessions: the host's settings file edited by hand to rebind the toggle, then malformed), each with
# scripts/gui-<session>-steps.json and its logs and settings files in <OutDir>\<session>. The camera suite installs the camera-demo plugin,
# drives it through its console command and reads its report log. The crash suite installs the host with its crash-test
# plugin and runs one game session per scenario (null, hook, overflow, write, form, deleted, script), each ending in the crash the plugin's
# console command armed; the host's crash log and minidump of each session go to <OutDir>\<scenario>. Any suite's run
# that leaves a skyrim-runtime-extender-crash-* file in the SKSE folder has it moved into <OutDir>. The sample suite installs the sample
# plugin of docs/MODS.md and compares its log with DevBench. See docs/subsystems/<suite>.md.
param(
    [Parameter(Mandatory)][ValidateSet("forms", "actors", "events", "papyrus", "render", "shadows", "frame", "animation", "bone", "ui", "input", "save", "audio", "overlay", "framework", "hooks", "reload", "compat", "legacy", "gui", "camera", "crash", "sample")][string]$Suite,
    [string]$OutDir = "",
    [string]$Mode = "",
    [ValidateSet("refuse", "stack")][string]$Compat = "refuse",
    # skse: the script extender alone; skse-proxy: the standalone host's dinput8.dll next to the game as well, which must
    # leave the script extender in charge; standalone: dinput8.dll and the game started without the script extender.
    [ValidateSet("skse", "skse-proxy", "standalone")][string]$Launch = "skse",
    [string]$Runtime = "1.7.104",
    [string]$GameDir = "",
    # Waits for a game or lock held by someone else instead of failing.
    [switch]$WaitForGame,
    # The legacy suite's plugins, one folder per plugin holding its SKSE\Plugins files as the mod ships them.
    [string]$LegacyDir = $(if ($env:SKYRIM_RS_LEGACY_DIR) { $env:SKYRIM_RS_LEGACY_DIR } else { "$env:LOCALAPPDATA\skyrim-rs\legacy" }),
    # Runs the legacy suite with these plugins instead of its list, to try a candidate alone.
    [string[]]$Legacy = @(),
    # Compares the logs an earlier run left in -OutDir again, without building, installing or launching.
    [switch]$CheckOnly,
    # Runs only these scenarios of a scenario suite (the crash suite), to retry one.
    [string[]]$Scenarios = @(),
    # Cargo profile for the build: verify (release code, no LTO) for iteration, release for a shipping check.
    [ValidateSet("verify", "release")][string]$Profile = "verify"
)
$ErrorActionPreference = "Stop"
$Install = & "$PSScriptRoot\runtimes.ps1" -Runtime $Runtime -GameDir $GameDir
$GameDir = $Install.dir
if (-not $OutDir) { $OutDir = "verify-$Suite-$Runtime-out" }
$MyGames = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition"
$SkseLogs = "$MyGames\SKSE"
$Plugins = "$GameDir\Data\SKSE\Plugins"
$Scripts = "$GameDir\Data\Scripts"
$Root = Split-Path $PSScriptRoot -Parent
$Dll = "$Root\target\$Profile\skyrim_rs_test.dll"
$Mods = @{
    papyrus = @{ crate = "equip-counter"; dll = "skyrim_rs_equip_counter.dll"; log = "skyrim-rs-equip-counter.log" }
    shadows = @{ crate = "first-person-shadows"; dll = "skyrim_rs_first_person_shadows.dll"; log = "skyrim-rs-first-person-shadows.log" }
    bone = @{ crate = "bone-driver"; dll = "skyrim_rs_bone_driver.dll"; log = "skyrim-rs-bone-driver.log" }
    camera = @{ crate = "camera-demo"; dll = "skyrim_rs_camera_demo.dll"; log = "skyrim-rs-camera-demo.log"; report = $true }
    sample = @{ crate = "sample-plugin"; dll = "skyrim_rs_sample.dll"; log = "skyrim-rs-sample.log"; report = $true }
    framework = @{ crate = "framework-test"; dll = "skyrim_rs_framework_test.dll"; log = "skyrim-rs-framework-test.log"; dir = "skyrim-runtime-extender"; report = $true;
                   crates = @("order-head", "order-tail", "order-cycle-a", "order-cycle-b")
                   dlls = @("skyrim_rs_order_head.dll", "skyrim_rs_order_tail.dll", "skyrim_rs_order_cycle_a.dll", "skyrim_rs_order_cycle_b.dll") }
    hooks = @{ crate = "hooks-test-a"; dll = "skyrim_rs_hooks_a.dll"; log = "skyrim-rs-hooks.log"; dir = "skyrim-runtime-extender"; report = $true;
               crates = @("hooks-test-b"); dlls = @("skyrim_rs_hooks_b.dll") }
    reload = @{ crate = "reload-test"; dll = "skyrim_rs_reload_test.dll"; log = "skyrim-rs-reload-test.log"; dir = "skyrim-runtime-extender"; report = $true; tags = @(2, 3, 4) }
    compat = @{ crate = "hooks-test-a"; dll = "skyrim_rs_hooks_a.dll"; log = "skyrim-rs-hooks.log"; dir = "skyrim-runtime-extender"; report = $true; steps = "hooks";
                crates = @("hooks-test-b", "legacy-patch-a", "legacy-patch-z"); dlls = @("skyrim_rs_hooks_b.dll");
                plugins = @("skyrim_legacy_patch_a.dll", "skyrim_z_legacy_patch.dll"); logs = @("skyrim-rs-legacy-patch-a.log", "skyrim-rs-legacy-patch-z.log") }
    # Third-party plugins built for 1.7.x on the Address Library, each <name>.dll in $LegacyDir\<name>\SKSE\Plugins
    # (DevBench is the other one; it is installed for every suite).
    legacy = @{ dir = "skyrim-runtime-extender"; legacy = @("CrashLogger") }
    gui = @{ crate = "example-mod"; dll = "skyrim_rs_example_mod.dll"; log = "skyrim-rs-example.log"; dir = "skyrim-runtime-extender"; report = $true; tags = @(2);
             crates = @("hooks-test-a", "hooks-test-b"); dlls = @("skyrim_rs_hooks_a.dll", "skyrim_rs_hooks_b.dll")
             # Each session's edit of the host's settings file before its launch.
             sessions = [ordered]@{
                 rebind = { param($file)
                     $text = Get-Content $file -Raw
                     if ($text -notmatch '"key\.skyrim-runtime-extender\.toggle": "F11"') { throw "$file does not bind the toggle to F11" }
                     ($text -replace '"key\.skyrim-runtime-extender\.toggle": "F11"', '"key.skyrim-runtime-extender.toggle": 68') -replace '^\{', "{`n  `"by.hand`": `"kept`"," | Set-Content $file -Encoding ascii -NoNewline }
                 malformed = { param($file) Set-Content $file "{`n  `"key.skyrim-runtime-extender.toggle`": `"F10`"`n  `"overlay.capture`": `"focus`"`n}`n" -Encoding ascii -NoNewline }
             } }
    # The first copy of the crash logger to load reports, and the test plugin sorts before the host, whose logger is under test here.
    crash = @{ crate = "crash-test"; dll = "skyrim_rs_crash_test.dll"; log = "skyrim-rs-crash-test.log"; dir = "skyrim-runtime-extender"; report = $true; noTestPlugin = $true;
               scenarios = @("null", "hook", "overflow", "write", "form", "deleted", "script")
               # The step that triggers each armed crash, as the scenario's JSON.
               triggers = @{ null = '{"tool": "console", "args": {"command": "save skyrim-rs-crash-{rt}"}}'; overflow = '{"tool": "console", "args": {"command": "save skyrim-rs-crash-{rt}"}}'; write = '{"tool": "console", "args": {"command": "save skyrim-rs-crash-{rt}"}}'
                             hook = '{"tool": "console", "args": {"command": "player.damageactorvalue health 1"}}'; form = '{"tool": "console", "args": {"command": "save skyrim-rs-crash-{rt}"}}'; deleted = '{"tool": "console", "args": {"command": "save skyrim-rs-crash-{rt}"}}'
                             script = '{"tool": "papyrus", "args": {"action": "call", "script": "SkyrimRsCrashTest", "function": "CrashFromScript"}}' } }
}
$TagDir = "$Root\target\$Profile\reload-tags"
$Mod = $Mods[$Suite]
if ($Suite -eq "legacy" -and $Legacy) { $Mod.legacy = @($Legacy -split ",") }
if ($Mod.scenarios -and $Scenarios) { $Mod.scenarios = @($Scenarios -split ",") }
$PluginLog = "skyrim-rs-test.log"
$HostDll = "skyrim_runtime_extender.dll"
$ModLogs = @($Mods.Values | ForEach-Object { $_.log; $_.logs } | Where-Object { $_ }) + @("skyrim-runtime-extender.log", "skyrim-runtime-extender-standalone.log", "skyrim-rs-order.log")
$Proxy = "$GameDir\dinput8.dll"
$Logs = @("skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog) + $ModLogs
$StepsName = if ($Mod.steps) { $Mod.steps } else { $Suite }
# Everything the suite saved, its records beside the saves too, on this runtime only.
$TestSaves = "skyrim-rs-$StepsName*$($Install.tag)*"
$Ini = "$Plugins\skyrim-runtime-extender.ini"
$HostFolder = "$Plugins\skyrim-runtime-extender"
$ProfileFile = "$Root\crates\host\profiles\$($Install.profile)"
$Captures = "$Plugins\devbench\captures"
$PexSources = @(
    if ($Suite -eq "papyrus") { "$Root\crates\testplugin\papyrus", "$Root\crates\equip-counter\papyrus" }
    if ($Suite -in @("framework", "reload", "gui", "crash")) { "$Root\crates\$($Mod.crate)\papyrus" }
)
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

$InstallDiff = $null
$CleanupFailures = @()
$RefusedBy = @()
$scenarioOk = $null
$summaries = @()
# A cleanup step that fails is tried again for 30 s (a game just stopped may still hold its files), then reported.
function Retried($what, [scriptblock]$action) {
    for ($attempt = 1; ; $attempt++) {
        try { & $action; return }
        catch {
            if ($attempt -ge 10) { $script:CleanupFailures += "${what}: $($_.Exception.Message)"; return }
            Start-Sleep -Seconds 3
        }
    }
}
function Write-Results($verdict, $reason = "", $checks = @()) {
    $clean = $null
    if ($null -ne $InstallDiff) { $clean = -not ($InstallDiff -or $CleanupFailures) }
    $counts = { param($r) @($checks | Where-Object { $_.result -eq $r }).Count }
    @{
        runtime = $Runtime; skse = $Install.skse; gameDir = $GameDir; suite = $Suite; mode = $Mode; launch = $Launch; profile = $Profile; finished = (Get-Date -Format s)
        commit = (git -C $Root rev-parse HEAD); dirty = [bool](git -C $Root status --porcelain)
        verdict = $verdict; reason = $reason; refusedBy = @($RefusedBy)
        pass = (& $counts "pass"); fail = (& $counts "fail"); notRun = $(if ($verdict -in "refused", "not-run") { "all" } else { & $counts "not-run" }); scenarioOk = $scenarioOk; pluginSummaries = @($summaries)
        installClean = $clean; installDiff = @($InstallDiff | Where-Object { $_ }); cleanupFailures = @($CleanupFailures)
        checks = @($checks)
    } | ConvertTo-Json -Depth 5 | Out-File -Encoding utf8 "$OutDir\results.json"
    if ($InstallDiff -or $CleanupFailures) { "INSTALL NOT CLEAN after a $verdict run; see $OutDir\results.json" }
}
trap { Write-Results "error" $_.Exception.Message; break }

# On an install other than the canonical one nothing goes outside Data\SKSE\Plugins until that is approved.
$NotRunnable = if (-not $Install.canonical) {
    if ($Launch -ne "skse") { "-Launch $Launch copies dinput8.dll next to the game's executable" }
    elseif ($PexSources) { "the suite copies .pex files to Data\Scripts" }
    elseif ($Suite -eq "legacy") { "its third-party plugins are built for 1.7.x; canonical only" }
}
if ($NotRunnable) {
    "NOT RUN $Suite on ${Runtime}: $NotRunnable"
    Write-Results "not-run" $NotRunnable
    exit 3
}

if ($Suite -eq "overlay") { $Mode = "overlay" }
if (-not $CheckOnly) {
$tookLock = & "$PSScriptRoot\game-free.ps1" -Wait:$WaitForGame
& "$PSScriptRoot\owner-install.ps1" -GameDir $GameDir
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
            try { cargo build --profile $Profile -p $Mod.crate @features; if ($LASTEXITCODE -ne 0) { throw "cargo build failed for tag $t" } } finally { Remove-Item env:SKYRIM_RS_RELOAD_TAG }
            Copy-Item "$Root\target\$Profile\$($Mod.dll)" "$TagDir\$t.dll" -Force
        }
    }
    cargo build --profile $Profile @($crates | ForEach-Object { "-p"; $_ }); if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }
$Pex = @(if ($PexSources) { & "$PSScriptRoot\compile-papyrus.ps1" -Source $PexSources })
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

$AppId = "$GameDir\steam_appid.txt"
$StateBefore = @(& "$PSScriptRoot\install-state.ps1" -GameDir $GameDir)
$CapturesBefore = @(if (Test-Path $Captures) { Get-ChildItem $Captures -Recurse -File | ForEach-Object FullName })
$Stash = & "$PSScriptRoot\owner-install.ps1" -GameDir $GameDir -Stash
$PluginsBefore = @(Get-ChildItem $Plugins -Name)
$Installed = $false
$retraceOff = $false
$LegacyItems = @()
$LegacyMissing = @()
$ModDir = "$Plugins\$($Mod.dir)"
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
    while (-not (Select-String -Path "$SkseLogs\skyrim-runtime-extender.log" -Pattern "generation $tag loaded" -Quiet)) {
        if (((Get-Date) - $t0).TotalSeconds -gt 90) { throw "the host did not report generation $tag within 90 s" }
        $game = Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($game) { [void]$shell.AppActivate($game.Id) }
        Start-Sleep -Seconds 2
    }
    Start-Sleep -Seconds 2
}.GetNewClosure()
$DevBenchLaunch = if ($Launch -eq "standalone") { "standalone" } else { "skse" }
$Session = @{ Runtime = $Runtime; GameDir = $GameDir; Launch = $DevBenchLaunch; WaitForGame = $WaitForGame }
try {
    # Nothing is copied or removed until the install is known to hold nothing of this run's names.
    foreach ($p in $Pex) { if (Test-Path "$Scripts\$(Split-Path $p -Leaf)") { throw "$Scripts already holds $(Split-Path $p -Leaf)" } }
    if ($Suite -eq "compat" -and (Test-Path $Ini)) { throw "$Ini already exists" }
    $retraceOff = Test-Path "$Plugins\retrace.dll"
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
    & "$PSScriptRoot\base-save.ps1" -Runtime $Runtime -GameDir $GameDir -OutDir $OutDir -WaitForGame:$WaitForGame

    $Installed = $true
    if ($Launch -ne "skse") { Copy-Item "$Root\target\$Profile\dinput8.dll" $Proxy }
    # Started directly, the game asks Steam to relaunch it through its launcher unless it finds its app id next to it.
    if ($Launch -eq "standalone") { Set-Content $AppId "489830" -Encoding ascii -NoNewline }
    if (-not $Mod.noTestPlugin) { Copy-Item $Dll "$Plugins\skyrim_rs_test.dll" -Force }
    if ($Mode) { Set-Content "$Plugins\skyrim-rs-test.mode" $Mode -Encoding ascii }
    # Every plugin built on the library reads this runtime's profile, with or without the host.
    if (Test-Path $ProfileFile) {
        New-Item -ItemType Directory -Force "$HostFolder\profiles" | Out-Null
        Copy-Item $ProfileFile "$HostFolder\profiles\"
    }
    if ($Mod.dir) {
        New-Item -ItemType Directory -Force $ModDir | Out-Null
        # A plugin built with its PDB (the crash suite's) gets it next to the DLL, for the crash log's names.
        foreach ($d in @($Mod.dll) + @(if ($Mod.dlls) { $Mod.dlls })) { if ($d) { Copy-Item "$Root\target\$Profile\$d" $ModDir; $pdb = "$Root\target\$Profile\$([IO.Path]::ChangeExtension($d, 'pdb'))"; if (Test-Path $pdb) { Copy-Item $pdb $ModDir } } }
        Copy-Item $Dll "$ModDir\decoy.dll"
        Copy-Item "$Root\target\$Profile\$HostDll" "$Plugins\$HostDll" -Force
    } elseif ($Mod) { Copy-Item "$Root\target\$Profile\$($Mod.dll)" "$Plugins\$($Mod.dll)" -Force }
    foreach ($d in @($Mod.plugins)) { if ($d) { Copy-Item "$Root\target\$Profile\$d" "$Plugins\$d" -Force } }
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
    if ($Suite -eq "compat") { Set-Content $Ini "[hooks]`r`ncompat = $Compat" -Encoding ascii }
    foreach ($p in $Pex) { Copy-Item $p $Scripts -Force }

    if ($Mod.scenarios) {
        # One game session per scenario, each ending in the crash it armed: devbench.ps1 reports the dead game as a
        # failure, and what matters is the crash log and dump the host's watchdog left in the SKSE folder.
        $scenarioOk = $true
        foreach ($s in $Mod.scenarios) {
            $runDir = "$OutDir\$s"
            New-Item -ItemType Directory -Force $runDir | Out-Null
            (Get-Content "$PSScriptRoot\$StepsName-steps.json" -Raw).Replace("{scenario}", $s).Replace("{trigger}", $Mod.triggers[$s]) | Set-Content "$runDir\steps.json" -Encoding ascii
            try { & "$PSScriptRoot\devbench.ps1" -Steps "$runDir\steps.json" -OutDir $runDir @Session } catch { "scenario $s`: the game ended: $($_.Exception.Message)" }
            foreach ($f in @("skyrim-runtime-extender.log", $Mod.log)) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$runDir\$f" -Force } }
            Get-ChildItem "$SkseLogs" -Filter "skyrim-runtime-extender-crash-????????-??????.*" | Move-Item -Destination $runDir -Force
            if (-not (Get-ChildItem $runDir -Filter "skyrim-runtime-extender-crash-????????-??????.log")) { "scenario $s`: no crash log"; $scenarioOk = $false }
        }
    } else {
        # Without saves a scenario cannot start; the main menu is enough to see what the loader did with the plugins.
        $StepsFile = if ($Install.canSave) { "$PSScriptRoot\$StepsName-steps.json" } else { "$PSScriptRoot\main-menu-steps.json" }
        & "$PSScriptRoot\devbench.ps1" -Steps $StepsFile -Between $(if ($Mod.tags) { $Between }) -OutDir $OutDir @Session
        $scenarioOk = $LASTEXITCODE -eq 0
        if ($Mod.sessions) {
            # Every session's logs leave the SKSE folder with it, so the next starts fresh and the cleanup below keeps them.
            $keep = { param($to)
                foreach ($f in @($PluginLog) + $ModLogs) { if (Test-Path "$SkseLogs\$f") { Move-Item "$SkseLogs\$f" "$to\$f" -Force } }
                Get-ChildItem "$Plugins\$($Mod.dir)" -Filter "*.settings.json*" | Copy-Item -Destination $to -Force
            }
            & $keep $OutDir
            foreach ($s in $Mod.sessions.Keys) {
                $runDir = "$OutDir\$s"
                New-Item -ItemType Directory -Force $runDir | Out-Null
                & $Mod.sessions[$s] "$Plugins\$($Mod.dir)\skyrim-runtime-extender.settings.json"
                & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\$StepsName-$s-steps.json" -OutDir $runDir @Session
                $scenarioOk = $scenarioOk -and $LASTEXITCODE -eq 0
                & $keep $runDir
            }
        }
    }
} finally {
    Retried "move the crash files" {
        Get-ChildItem "$SkseLogs" -Filter "skyrim-runtime-extender-crash-????????-??????.*" | Move-Item -Destination $OutDir -Force
        if ($Mod.dir -and -not $Mod.sessions) { Get-ChildItem "$Plugins\$($Mod.dir)" -Filter "*.settings.json" -ErrorAction SilentlyContinue | Copy-Item -Destination $OutDir -Force }
    }
    if ($Installed) {
        # A game that crashed may still hold the installed DLLs for a few seconds after Stop-Process.
        $paths = @(@("skyrim_rs_test.dll", "skyrim-rs-test.mode", $HostDll) + @($Mods.Values | ForEach-Object { $_.dll }) + @($Mod.plugins) | Where-Object { $_ } | ForEach-Object { "$Plugins\$_" }) +
            @($HostFolder) + @(if ($Suite -eq "compat") { $Ini }) + @($LegacyItems) + @(if ($Launch -ne "skse") { $Proxy }) + @(if ($Launch -eq "standalone") { $AppId }) + @($Pex | ForEach-Object { "$Scripts\$(Split-Path $_ -Leaf)" })
        foreach ($path in $paths) { Retried "remove $path" { if (Test-Path $path) { Remove-Item $path -Recurse -Force } } }
    }
    if ($retraceOff) { Retried "rename retrace.dll back" { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" } }
    # DevBench's captures leave the install with the run, each with its metadata.
    if (Test-Path $Captures) {
        Retried "move the captures" {
            foreach ($f in Get-ChildItem $Captures -Recurse -File | Where-Object { $_.FullName -notin $CapturesBefore }) {
                $to = "$OutDir\captures$($f.FullName.Substring($Captures.Length))"
                New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
                Move-Item $f.FullName $to -Force
            }
            Get-ChildItem $Captures -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending | Where-Object { -not (Get-ChildItem $_.FullName -Force) } | Remove-Item
            if (-not $CapturesBefore -and -not (Get-ChildItem $Captures -Force)) { Remove-Item $Captures }
        }
    }
    if ($Mod.legacy) {
        Retried "clear the legacy plugins' files" {
            foreach ($item in Get-ChildItem $Plugins -Name | Where-Object { $_ -notin $PluginsBefore -and $_ -notlike "retrace.dll*" }) { "removing $item, left in Data\SKSE\Plugins by the run"; Remove-Item "$Plugins\$item" -Recurse -Force }
            New-Item -ItemType Directory -Force "$OutDir\legacy-logs" | Out-Null
            foreach ($f in Get-ChildItem $SkseLogs -File | Where-Object { $_.Name -notin $Logs -and (-not $OtherLogs.ContainsKey($_.Name) -or $_.LastWriteTime -ne $OtherLogs[$_.Name]) }) {
                Copy-Item $f.FullName "$OutDir\legacy-logs\$($f.Name)" -Force
                if ($OtherLogs.ContainsKey($f.Name)) { Copy-Item "$Backup\other\$($f.Name)" $f.FullName -Force } else { Remove-Item $f.FullName }
            }
        }
    }
    Retried "remove the suite's saves" { Remove-Item "$MyGames\Saves\$TestSaves" }
    foreach ($f in @($PluginLog) + $ModLogs) { if (Test-Path "$SkseLogs\$f") { Retried "keep $f" { Copy-Item "$SkseLogs\$f" "$OutDir\$f" -Force } } }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Retried "restore $f" { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force } }
        elseif ($f -in @($PluginLog) + $ModLogs) { Retried "remove $f" { if (Test-Path "$SkseLogs\$f") { Remove-Item "$SkseLogs\$f" } } }
    }
    Retried "restore the owner's install from $Stash" { & "$PSScriptRoot\owner-install.ps1" -GameDir $GameDir -Restore $Stash }
    $InstallDiff = @(Compare-Object $StateBefore @(& "$PSScriptRoot\install-state.ps1" -GameDir $GameDir) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" })
    foreach ($f in $CleanupFailures) { "CLEANUP FAILED: $f" }
    foreach ($d in $InstallDiff) { "INSTALL NOT CLEAN: $d" }
    if ($tookLock) { & "$PSScriptRoot\game-free.ps1" -Release }
}
}

# The plugin's PASS/FAIL lines per "<suite> <phase>" run (the summary line closes a run) and, for the chosen suite, the last one per test.
# Their keys are read through PSBase: the input suite's test named "keys" would shadow the .Keys property.
$plugin = @{}
$runs = @{}
$pending = @{}
$summaries = @()
$ReportLog = if ($Mod.report) { $Mod.log } else { $PluginLog }
$ReportLines = @(if (Test-Path "$OutDir\$ReportLog") { Get-Content "$OutDir\$ReportLog" })
foreach ($line in $ReportLines) {
    if ($line -match '^(PASS|FAIL) (\w+): (.*)$') { $pending[$Matches[2]] = @{ ok = ($Matches[1] -eq "PASS"); detail = $Matches[3] } }
    elseif ($line -match '^summary (\w+ \w+): ') {
        $runs[$Matches[1]] = $pending
        if ($Matches[1] -like "$Suite *") { foreach ($k in $pending.PSBase.Keys) { $plugin[$k] = $pending[$k] } }
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
$LoaderLogName = if ($Launch -eq "standalone") { "skyrim-runtime-extender-standalone.log" } else { "skse64.log" }
$LoaderLogDir = if ($Mod.scenarios) { "$OutDir\$($Mod.scenarios[0])" } else { $OutDir }
$LoaderLog = @(if (Test-Path "$LoaderLogDir\$LoaderLogName") { Get-Content "$LoaderLogDir\$LoaderLogName" })
$HostLog = @(if (Test-Path "$OutDir\skyrim-runtime-extender.log") { Get-Content "$OutDir\skyrim-runtime-extender.log" })
function Loaded($file) {
    $f = [regex]::Escape($file)
    # SKSE 2.0.19 names a plugin by its full path, 2.1.5 and later by its file name.
    $pattern = if ($Launch -eq "standalone") { "^$f`: loaded as handle (\d+)" } else { "^plugin (?:.*\\)?$f \(.*\) loaded correctly \(handle (\d+)\)" }
    foreach ($l in $LoaderLog) { if ($l -match $pattern) { return [int]$Matches[1] } }
    $null
}
# Where a run's capture went: the install's captures folder moved into the run's.
function KeptCapture($path) { if ($path) { "$OutDir\captures\" + (($path -replace '^.*/captures/', '') -replace '/', '\') } }
# The exit of a run: a dirty install outranks what the suite did.
function Finish($code) { if ($InstallDiff -or $CleanupFailures) { exit 4 }; exit $code }
function Hosted { @($HostLog | Where-Object { $_ -match "^hosted by $HostedBy, " }).Count -eq 1 }
function Near($a, $b, $tol) { ($a -ne $null) -and ($b -ne $null) -and ([math]::Abs([double]$a - [double]$b) -lt $tol) }
# Converts an overlay capture to a half-size JPEG in the run's folder and removes the bitmap.
Add-Type -AssemblyName System.Drawing
function HalfJpeg($from, $to) {
    if (-not (Test-Path $from)) { return $false }
    $img = [System.Drawing.Image]::FromFile($from)
    $half = New-Object System.Drawing.Bitmap $img, ([int]($img.Width / 2)), ([int]($img.Height / 2))
    $half.Save($to, [System.Drawing.Imaging.ImageFormat]::Jpeg)
    $half.Dispose(); $img.Dispose()
    Remove-Item $from
    $true
}

# A plugin the loader did not load has nothing to compare: each of its lines in the log is the evidence, and no check ran.
$Wanted = @(if (-not $Mod.noTestPlugin) { "skyrim_rs_test.dll" }) + @(if ($Mod.dir) { $HostDll } elseif ($Mod.dll) { $Mod.dll }) + @($Mod.plugins | Where-Object { $_ })
foreach ($f in $Wanted) {
    if ($LoaderLog -and $null -eq (Loaded $f)) { $RefusedBy += @{ plugin = $f; lines = @($LoaderLog | Where-Object { $_ -match [regex]::Escape($f) } | ForEach-Object { "$_" }) } }
}
if ($RefusedBy) {
    foreach ($r in $RefusedBy) { "REFUSED $($r.plugin): $($r.lines -join ' | ')" }
    "scenario ok: $scenarioOk"
    "logs in $OutDir"
    Write-Results "refused" "the loader did not load $(($RefusedBy | ForEach-Object { $_.plugin }) -join ', '); no check ran"
    Finish 2
}
if (-not $Install.canSave) {
    $why = "this install has no Data\Scripts\Game.pex, so DevBench can neither save nor load; only the main menu was reached"
    "NOT RUN $Suite on ${Runtime}: $why"
    Write-Results "not-run" $why
    Finish 3
}

. "$PSScriptRoot\$Suite-checks.ps1"

""
"plugin summary: $($summaries -join '; ')"
"scenario ok: $scenarioOk"
""
$allOk = $scenarioOk -ne $false
$checks = @()
foreach ($phase in $Phases) {
    if (($summaries -match "^summary $Suite $phase`: \d+ pass, 0 fail").Count -ne 1) { "no clean $Suite run in phase $phase"; $allOk = $false; $checks += @{ name = "phase $phase"; result = "fail" } }
}
foreach ($r in $rows) {
    # A row with notRun could not run on this runtime; its reason says why.
    if ($r.notRun) {
        "NOT RUN $($r.name): $($r.notRun)"
        $checks += @{ name = $r.name; result = "not-run"; reason = $r.notRun }
        continue
    }
    $verdict = if ($r.ok) { "OK  " } else { "DIFF"; $allOk = $false }
    "$verdict $($r.name)"
    "     plugin:   $($r.plugin)"
    "     devbench: $($r.devbench)"
    $checks += @{ name = $r.name; result = $(if ($r.ok) { "pass" } else { "fail" }) }
}
foreach ($name in ($plugin.PSBase.Keys | Sort-Object)) { if (-not $plugin[$name].ok) { "FAIL $name`: $($plugin[$name].detail)"; $allOk = $false; $checks += @{ name = "plugin $name"; result = "fail"; reason = $plugin[$name].detail } } }
""
"logs in $OutDir"
Write-Results $(if ($allOk) { "pass" } else { "fail" }) "" $checks
Finish $(if ($allOk) { 0 } else { 1 })
