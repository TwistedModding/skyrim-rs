# DevBench comparisons for the gui suite (step indices of gui-steps.json, two parts merged: 36 steps, then 20); dot-sourced
# by verify.ps1. The host runs the example mod (first in load order, so its Before on Actor::ModActorValueImpl is
# subscription #1) and the hooks suite's plugins a and b. Part 1: "SkyrimRs window" opens the overlay on the inspector and
# lists the plugins, W is held for 2 s with nothing focused (focus capture: the player moves), F11 closes the overlay,
# "SkyrimRs hooks" lists the sites, the example's native sets the damage scale to 1.5, then three damages of 10 with
# health read back: with #1 on (100 -> 84: 15, a +1, b doubles and halves), after "SkyrimRs toggle 1" (-> 73: the scale
# skipped) and after toggling it on again (-> 57); the save makes b remove itself and a report what its Before saw.
# Between the parts the example's DLL is copied in again and the host reloads it as generation 2. Part 2: the inspector
# lists the new generation's subscription, one damage of 10 (-> 41: the scale kept, b gone), "SkyrimRs capture" writes
# the overlay frame with the inspector open, F11, Escape opens the game's journal for DevBench's capture, Escape, the
# native sets the scale to 0.5 and the save; the example's settings file is read after the run.
$Phases = @()

$hostLog = if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" } else { @() }
function HostLines($pattern) { @($hostLog | Where-Object { $_ -match $pattern }) }
$loaded = HostLines '^loaded skyrim_rs_(example_mod|hooks_a|hooks_b)\.dll: skyrim-rs-(example|hooks-a|hooks-b)$'
$font = HostLines '^overlay font: Futura CondensedLight from .*fonts_en\.swf$'
$command = HostLines '^command SkyrimRs took over console opcode 0x[0-9a-f]+'
$reloading = HostLines '^skyrim-rs-example: skyrim_rs_example_mod\.dll changed, reloading as generation 2$'
$detached = HostLines '^skyrim-rs-example: generation 1: 1 hook subscriptions, 0 sinks, 1 natives stubbed, 0 commands stubbed, 1 windows, 0 message handlers, 1 Papyrus callbacks detached$'
$reloaded = HostLines '^skyrim-rs-example: generation 2 loaded, image at 0x[0-9a-f]+$'
$settingsWritten = HostLines '^settings written to .*skyrim-rs-example\.settings\.json$'
$frameWritten = HostLines '^overlay frame written to .*skyrim-rs-overlay\.bmp$'

function Pos($i) { Captured (Lines $i) '>>\s*(-?[\d.]+)' }
$moved = if ((Pos 7) -and (Pos 13)) { [math]::Sqrt([math]::Pow([double](Pos 13) - [double](Pos 7), 2) + [math]::Pow([double](Pos 14) - [double](Pos 8), 2)) } else { 0 }
$health = @(19, 22, 27, 32, 39 | ForEach-Object { Returned $_ })
$window1 = Lines 5
$window2 = Lines 40
# The calls counted for subscription #1 and for its site in a "SkyrimRs hooks" output.
function EntryCalls($i, $id) { Captured (Lines $i) "#$id skyrim-rs-example Before rank \d+: (\d+) calls" }
function SiteCalls($i) { Captured (Lines $i) 'site Actor::ModActorValueImpl at 0x[0-9a-f]+: (\d+) calls' }
# Plugin a's report at the first save (its own log: the suite's report is the example's): the deltas its Before saw.
$hooksLog = if (Test-Path "$OutDir\skyrim-rs-hooks.log") { Get-Content "$OutDir\skyrim-rs-hooks.log" } else { @() }
$aBefore = Captured (($hooksLog | Where-Object { $_ -match '^(PASS|FAIL) a_before: ' }) -join ' ') '(saw \[[^\]]*\])'
$newId = Captured (Lines 36) '#(\d+) skyrim-rs-example Before'
$settingsFile = "$OutDir\skyrim-rs-example.settings.json"
$scaleOnDisk = if (Test-Path $settingsFile) { Captured (Get-Content $settingsFile -Raw) '"damage_scale":\s*([-\d.]+)' } else { $null }

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
$shot = HalfJpeg "$SkseLogs\skyrim-rs-overlay.bmp" "$OutDir\gui-1.7.104.jpg"
$vanillaPath = (Step 48).path
$vanilla = $vanillaPath -and (HalfJpeg $vanillaPath "$OutDir\gui-vanilla-1.7.104.jpg")

$rows = @(
    @{ name = "host loads"; plugin = "(host log) $($loaded -join ' | '); $($command -join ' | ')"; devbench = "SkyrimRs window: $window1";
       ok = ($loaded.Count -eq 3 -and $command.Count -eq 1 -and $window1 -match 'plugin 0: skyrim-rs-example generation 1 \(skyrim_rs_example_mod\.dll; \)' -and $window1 -match 'plugin 1: skyrim-rs-hooks-a generation 1' -and $window1 -match 'plugin 2: skyrim-rs-hooks-b generation 1') }
    @{ name = "font"; plugin = "(host log) $($font -join ' | ')"; devbench = "SkyrimRs window says font $(Captured $window1 'font (.*?), capture')";
       ok = ($font.Count -eq 1 -and $window1 -match 'font Futura CondensedLight from') }
    @{ name = "windows"; plugin = "SkyrimRs window lists the windows"; devbench = $window1;
       ok = ($window1 -match 'window skyrim-rs/inspector "skyrim-rs": open' -and $window1 -match 'window skyrim-rs-example/main "Example mod": open') }
    @{ name = "focus capture"; plugin = "overlay open with nothing focused, capture $(Captured $window1 'capture (\w+)')"; devbench = "player moved $([math]::Round($moved, 1)) units during the 2 s W hold (x $(Pos 7) -> $(Pos 13), y $(Pos 8) -> $(Pos 14))";
       ok = ($window1 -match 'capture Focus' -and $moved -gt 50) }
    @{ name = "hook sites"; plugin = "SkyrimRs hooks: $(Lines 17)"; devbench = "(listed through the console)";
       ok = ((Lines 17) -match 'site Actor::ModActorValueImpl at 0x[0-9a-f]+' -and (Lines 17) -match '#1 skyrim-rs-example Before rank 0' -and (Lines 17) -match 'skyrim-rs-hooks-b Around' -and (Lines 17) -match 'message: refused: Actor::ModActorValueImpl: ordering cycle') }
    @{ name = "native"; plugin = "SetDamageScale(1.5) = $(Returned 18)"; devbench = "health $($health[0]) -> $($health[1]) after '$(Lines 20)'";
       ok = ((Near (Returned 18) 1.5 0.001) -and (Near $health[0] 100 0.01) -and (Near $health[1] 84 0.01)) }
    @{ name = "toggle off"; plugin = "$(Lines 23); #1 ran $(EntryCalls 24 1) then $(EntryCalls 28 1) times while the site ran $(SiteCalls 24) then $(SiteCalls 28); hooks a saw $aBefore"; devbench = "health $($health[1]) -> $($health[2]) after '$(Lines 25)'";
       ok = ((Lines 23) -match 'SkyrimRs: #1 disabled' -and (Lines 28) -match '#1 skyrim-rs-example Before rank 0: \d+ calls, last \d+ ns, mean \d+ ns, disabled' -and (EntryCalls 24 1) -ne $null -and (EntryCalls 24 1) -eq (EntryCalls 28 1) -and [int](SiteCalls 28) -gt [int](SiteCalls 24) -and $aBefore -match 'saw \[-15\.0, -10\.0, -15\.0\]' -and (Near $health[2] 73 0.01)) }
    @{ name = "toggle on"; plugin = (Lines 29); devbench = "health $($health[2]) -> $($health[3]) after '$(Lines 30)'";
       ok = ((Lines 29) -match 'SkyrimRs: #1 enabled' -and (Near $health[3] 57 0.01)) }
    @{ name = "reload"; plugin = "(host log) $($reloading -join ' | '); $($detached -join ' | '); $($reloaded -join ' | ')"; devbench = "SkyrimRs hooks after the reload: example as #$newId; SkyrimRs window: $(Captured $window2 '(plugin 0: [^|]*)')";
       ok = ($reloading.Count -eq 1 -and $detached.Count -eq 1 -and $reloaded.Count -eq 1 -and $newId -and [int]$newId -gt 1 -and (Lines 36) -notmatch '\|\s+#1 skyrim-rs-example Before rank' -and (Lines 36) -notmatch '\|\s+#\d+ skyrim-rs-hooks-b' -and $window2 -match 'plugin 0: skyrim-rs-example generation 2') }
    @{ name = "settings"; plugin = "(host log) $($settingsWritten.Count) writes; damage_scale on disk $scaleOnDisk"; devbench = "health $($health[3]) -> $($health[4]) after the reload (scale 1.5 kept); SetDamageScale(0.5) = $(Returned 51)";
       ok = ($settingsWritten.Count -ge 2 -and (Near $scaleOnDisk 0.5 0.0001) -and (Near (Returned 51) 0.5 0.001) -and (Near $health[4] 41 0.01)) }
    @{ name = "capture"; plugin = "(host log) $($frameWritten -join ' | '); kept as gui-1.7.104.jpg: $shot"; devbench = "journal captured by DevBench, kept as gui-vanilla-1.7.104.jpg: $vanilla";
       ok = ($frameWritten.Count -eq 1 -and $shot -and $vanilla) }
    @{ name = "saves"; plugin = ($summaries -match 'hooks [ab]Save1') -join ' '; devbench = "saveGame seen: $(($steps | Where-Object { $_.index -eq 34 }).satisfied), $(($steps | Where-Object { $_.index -eq 54 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 34 }).satisfied -eq $true -and ($steps | Where-Object { $_.index -eq 54 }).satisfied -eq $true) }
)
