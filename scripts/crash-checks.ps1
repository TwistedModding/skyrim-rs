# Checks for the crash suite, one game session per scenario (the folders $OutDir\<scenario>); dot-sourced by verify.ps1.
# Each session arms one crash through the test plugin's console command and triggers it: null, overflow and write fire in
# the plugin's SAVE_GAME handler on the main thread at a console save, hook fires inside its Before subscriber on
# Actor::ModActorValueImpl at a damageactorvalue on the console's thread. The host's crash logger must write a minidump
# and a log naming the exception, the faulting module and plugin, the thread, the game functions on the stack and, for
# hook, the hook site and subscriber; the thread id in the log must be the one the plugin logged right before crashing.
$Phases = @()

function ScenarioFile($s, $pattern) { Get-ChildItem "$OutDir\$s" -Filter $pattern -ErrorAction SilentlyContinue | Select-Object -First 1 }
function CrashLog($s) { $f = ScenarioFile $s "skyrim-rs-crash-????????-??????.log"; if ($f) { @(Get-Content $f.FullName) } else { @() } }
function PluginLog($s) { if (Test-Path "$OutDir\$s\$($Mod.log)") { @(Get-Content "$OutDir\$s\$($Mod.log)") } else { @() } }
function HostLogOf($s) { if (Test-Path "$OutDir\$s\skyrim-rs.log") { @(Get-Content "$OutDir\$s\skyrim-rs.log") } else { @() } }
function First($lines, $pattern) { $m = @($lines | Where-Object { $_ -match $pattern }); if ($m.Count) { $m[0] } else { "" } }
function Count($lines, $pattern) { @($lines | Where-Object { $_ -match $pattern }).Count }
function Captured2($text, $pattern) { if ($text -match $pattern) { $Matches[1] } else { $null } }

$rows = @()
foreach ($s in $Mod.scenarios) {
    $crash = CrashLog $s
    $plugin = PluginLog $s
    $hostLogS = HostLogOf $s
    $dump = ScenarioFile $s "skyrim-rs-crash-????????-??????.dmp"
    $installed = First $hostLogS '^crash handler installed, watchdog thread \d+, game symbols AddressLibrary \((\d+) functions, (\d+) offsets differ'
    $codeLine = First $crash '^code 0x[0-9a-f]{8} '
    $atLine = First $crash '^at '
    $accessLine = First $crash '^(reading|writing|executing) address '
    $threadLine = First $crash '^on thread '
    $crashThread = Captured2 $threadLine '^on thread (\d+)'
    $pluginCrashLine = First $plugin "^crashing on thread \d+ \($s\)"
    $pluginThread = Captured2 $pluginCrashLine '^crashing on thread (\d+)'
    $frames = @($crash | Where-Object { $_ -match '^#\d+ ' })
    $pluginFrames = Count $frames 'skyrim_rs_crash_test\.1\.dll\+0x[0-9a-f]+.*\(framework plugin skyrim-rs-crash-test generation 1\)'
    $pdbFrames = Count $frames 'skyrim_rs_crash_test\.1\.dll\+0x[0-9a-f]+ skyrim_rs_crash_test::\S+\+0x[0-9a-f]+ \(.*lib\.rs:\d+\)'
    $rcx = First $crash '^rcx 0x'
    $explanation = First $crash '^rule '
    $hash = First $crash '^crash signature [0-9a-f]{16} over the top \d+ frames:'
    $gameFrames = Count $frames 'SkyrimSE\.exe\+0x[0-9a-f]+ \S+\+0x[0-9a-f]+ \((high|medium)'
    $hookLine = First $crash '^the crashing thread is '
    $dumpLine = First $crash '^minidump written to '
    $armed = First $plugin "^armed $s`:"

    $rows += @{ name = "$s`: report written"; plugin = "$armed | $pluginCrashLine"; devbench = "crash log lines $($crash.Count); $dumpLine; dump bytes $(if ($dump) { $dump.Length } else { 0 }); (host log) $installed";
                ok = ($crash.Count -gt 20 -and $dumpLine -ne "" -and $dump -and $dump.Length -gt 0 -and $installed -ne "" -and $armed -ne "" -and $pluginCrashLine -ne "") }
    $expectedThread = if ($s -eq "hook") { $threadLine -match '\(a job thread' } else { $threadLine -match '\(the main thread\)$' }
    $rows += @{ name = "$s`: pdb and explanation"; plugin = "frames with PDB names $pdbFrames"; devbench = "$hash | $explanation";
                ok = ($pdbFrames -ge 1 -and $hash -ne "" -and $explanation -ne "") }
    $rows += @{ name = "$s`: thread"; plugin = $pluginCrashLine; devbench = $threadLine;
                ok = ($crashThread -ne $null -and $crashThread -eq $pluginThread -and $expectedThread) }
    switch ($s) {
        "null" {
            $rows += @{ name = "null: exception"; plugin = $atLine; devbench = "$codeLine | $accessLine";
                        ok = ($codeLine -match '^code 0xc0000005 EXCEPTION_ACCESS_VIOLATION' -and $accessLine -eq 'reading address 0x0: null' -and $atLine -match 'skyrim_rs_crash_test\.1\.dll\+0x[0-9a-f]+.*\(framework plugin skyrim-rs-crash-test generation 1\)') }
            $rows += @{ name = "null: stack"; plugin = "plugin frames $pluginFrames, game frames $gameFrames"; devbench = ($frames | Select-Object -First 6) -join ' | ';
                        ok = ($pluginFrames -ge 1 -and $gameFrames -ge 1 -and $frames.Count -ge 8) }
        }
        "hook" {
            $rows += @{ name = "hook: exception"; plugin = $atLine; devbench = "$codeLine | $accessLine";
                        ok = ($codeLine -match '^code 0xc0000005 EXCEPTION_ACCESS_VIOLATION' -and $accessLine -eq 'reading address 0x0: null' -and $atLine -match 'skyrim_rs_crash_test\.1\.dll') }
            $rows += @{ name = "hook: site and subscriber"; plugin = (First $plugin '^PASS hook:'); devbench = $hookLine;
                        ok = ($hookLine -match '^the crashing thread is inside hook site Actor::ModActorValueImpl, subscriber #\d+ skyrim-rs-crash-test Before$') }
            $rows += @{ name = "hook: stack"; plugin = "plugin frames $pluginFrames, game frames $gameFrames"; devbench = ($frames | Select-Object -First 8) -join ' | ';
                        ok = ($pluginFrames -ge 1 -and (Count $frames 'skyrim_rs\.dll\+0x') -ge 1 -and (Count $frames 'Actor::(ActorValueOwner_ModActorValue|ModActorValueImpl)\+0x') -ge 1) }
        }
        "overflow" {
            $rows += @{ name = "overflow: exception"; plugin = $atLine; devbench = $codeLine;
                        ok = ($codeLine -match '^code 0xc00000fd EXCEPTION_STACK_OVERFLOW' -and $atLine -match 'skyrim_rs_crash_test\.1\.dll') }
            $rows += @{ name = "overflow: stack"; plugin = "plugin frames $pluginFrames of $($frames.Count)"; devbench = ($frames | Select-Object -First 3) -join ' | ';
                        ok = ($pluginFrames -ge 10 -and $frames.Count -eq 64) }
        }
        "form" {
            $rows += @{ name = "form: register probe"; plugin = $atLine; devbench = "$accessLine | $rcx";
                        ok = ($accessLine -eq 'reading address 0x0: null' -and $rcx -match '^rcx 0x[0-9a-f]+  TESObjectWEAP form 00012EB7 WEAP, from "Skyrim\.esm"') }
            $decoded = "$OutDir\$s\decoded.log"
            $dmpPath = if ($dump) { $dump.FullName } else { "" }
            if ($dmpPath) { Push-Location $Root; try { & cargo run -q -p xtask -- crash $dmpPath $decoded 2>&1 | Out-Null } finally { Pop-Location } }
            $dec = if (Test-Path $decoded) { @(Get-Content $decoded) } else { @() }
            $decNote = First $dec '^\(decoded offline'
            $decCode = First $dec '^code 0x'
            # The decoder runs after the shadow copy and its PDB are gone, so only the module and offset of its location can agree.
            $decAt = First $dec '^at '
            $decFrames = (Count $dec '^#\d+ ')
            $decRcx = First $dec '^rcx 0x'
            $rows += @{ name = "form: offline decoder"; plugin = "$codeLine | $atLine | $($frames.Count) frames"; devbench = "$decNote | $decCode | $decAt | $decFrames frames | $decRcx";
                        ok = ($dec.Count -gt 20 -and $decNote -match 'from the logger' -and $decCode -eq $codeLine -and ($decAt -split ' ')[1] -eq ($atLine -split ' ')[1] -and $decFrames -eq $frames.Count) }
        }
        "deleted" {
            $rows += @{ name = "deleted: register probe"; plugin = $atLine; devbench = "$accessLine | $rcx";
                        ok = ($accessLine -eq 'reading address 0x0: null' -and $rcx -match '^rcx 0x[0-9a-f]+  PlayerCharacter form 00000014 ACHR DELETED, .*refcount \d+, base TESNPC form 00000007 NPC_, from "[^"]+\.es[lmp]".*, in cell 00009732 .* at \(.*\), name "Prisoner", race 00013746' -and $explanation -match '^rule deleted-record:') }
        }
        "write" {
            $rows += @{ name = "write: exception"; plugin = $atLine; devbench = "$codeLine | $accessLine";
                        ok = ($codeLine -match '^code 0xc0000005 EXCEPTION_ACCESS_VIOLATION' -and $accessLine -match '^writing address 0x[0-9a-f]+: inside SkyrimSE\.exe\+0x1000, .*read-execute$' -and $atLine -match 'skyrim_rs_crash_test\.1\.dll') }
            $rows += @{ name = "write: stack"; plugin = "plugin frames $pluginFrames, game frames $gameFrames"; devbench = ($frames | Select-Object -First 6) -join ' | ';
                        ok = ($pluginFrames -ge 1 -and $gameFrames -ge 1) }
        }
    }
}
