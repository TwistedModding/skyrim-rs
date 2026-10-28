# DevBench comparisons for the gui suite (step indices of gui-steps.json, two parts merged: 36 steps, then 70); dot-sourced
# by verify.ps1. The host runs the example mod (first in load order, so its Before on Actor::ModActorValueImpl is
# subscription #1) and the hooks suite's plugins a and b. Part 1: "SkyrimRs window" opens the overlay on the inspector and
# lists the plugins, W is held for 2 s with nothing focused (focus capture: the player moves), F11 closes the overlay,
# "SkyrimRs hooks" lists the sites, the example's native sets the damage scale to 1.5, then three damages of 10 with
# health read back: with #1 on (100 -> 84: 15, a +1, b doubles and halves), after "SkyrimRs toggle 1" (-> 73: the scale
# skipped) and after toggling it on again (-> 57); the save makes b remove itself and a report what its Before saw.
# Between the parts the example's DLL is copied in again and the host reloads it as generation 2. Part 2: the inspector
# lists the new generation's subscription, one damage of 10 (-> 41: the scale kept, b gone), "SkyrimRs capture" writes
# the overlay frame with the inspector open, F11, Escape opens the game's journal for DevBench's capture, Escape, the
# native sets the scale to 0.5 and the save. Then the keys (56 on): F11 opens, Tab focuses a widget, Escape releases it
# (overlay still open, no journal), Escape closes the overlay (no journal), F11 opens and closes it, F11 opens it for the
# example's action (Shift+K hides its window; K and Alt+K do nothing), "SkyrimRs bind 2" captures Shift+L for that action,
# Shift+K no longer fires and Shift+L does (the window shown again), a second capture of F11 is refused, F11 closes, Shift+L
# with the overlay closed does not fire, and the save makes the test plugin list every key its sink (after the game's
# handlers) saw. The host's and the example's settings files are read after the part; then two more launches: rebind
# (the toggle set to scan code 68 by hand: F11 reaches the game and no longer toggles, F10 does) and malformed (the file
# reported, renamed to .bad and replaced by the defaults: F11 toggles again), their files in <OutDir>\<session>.
$Phases = @()

$hostLog = if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" } else { @() }
function HostLines($pattern) { @($hostLog | Where-Object { $_ -match $pattern }) }
$loaded = HostLines '^loaded skyrim_rs_(example_mod|hooks_a|hooks_b)\.dll: skyrim-rs-(example|hooks-a|hooks-b)$'
$font = HostLines '^overlay font: Futura CondensedLight from .*fonts_en\.swf$'
$command = HostLines '^command SkyrimRs took over console opcode 0x[0-9a-f]+'
$reloading = HostLines '^skyrim-rs-example: skyrim_rs_example_mod\.dll changed, reloading as generation 2$'
$detached = HostLines '^skyrim-rs-example: generation 1: 1 hook subscriptions, 0 sinks, 1 natives stubbed, 0 commands stubbed, 1 windows, 0 message handlers, 1 Papyrus callbacks, 1 frame callbacks detached$'
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
# DevBench writes its capture's metadata beside the picture, inside the game install.
if ($vanillaPath) { Remove-Item ([IO.Path]::ChangeExtension($vanillaPath, "json")) -ErrorAction SilentlyContinue }

# The keys: a session's steps, logs and files are in <OutDir>\<session>, the first session's in <OutDir>.
function SessionSteps($name) { $f = "$OutDir\$name\result.json"; @(if (Test-Path $f) { (Get-Content $f -Raw | ConvertFrom-Json).result.results }) }
function StepOf($all, $i) { $s = @($all | Where-Object { $_.index -eq $i }); if ($s.Count -and $s[0].ok) { $s[0].result } else { $null } }
function LinesOf($all, $i) { $r = StepOf $all $i; if ($r) { $r.lines -join " | " } else { "" } }
function Menus($all, $i) { $r = StepOf $all $i; if ($r) { @($r.openMenus) -join ", " } else { "(no answer)" } }
function LogOf($dir, $name) { $f = "$dir\$name"; @(if (Test-Path $f) { Get-Content $f }) }
function FileOf($dir, $name) { $f = "$dir\$name"; if (Test-Path $f) { Get-Content $f -Raw } else { "" } }
# The test plugin's sink runs after the game's handlers; at every save it lists each key it saw since the load.
function Seen($lines, $save, $code) { $l = @($lines | Where-Object { $_ -match '^PASS keys: ' }); if ($l.Count -gt $save -and $l[$save] -match "$code`: (press \d+ hold \d+ release \d+[^;]*)") { $Matches[1] } elseif ($l.Count -gt $save) { "not seen" } else { "no list" } }
$testLog = LogOf $OutDir "skyrim-rs-test.log"
$exampleLog = LogOf $OutDir "skyrim-rs-example.log"
$fired = @($exampleLog | Where-Object { $_ -match '^toggle-window fired' })
$hostFile = FileOf $OutDir "skyrim-rs.settings.json"
$rebind = SessionSteps "rebind"
$rebindLog = LogOf "$OutDir\rebind" "skyrim-rs.log"
$rebindFile = FileOf "$OutDir\rebind" "skyrim-rs.settings.json"
$rebindSeen = LogOf "$OutDir\rebind" "skyrim-rs-test.log"
$malformed = SessionSteps "malformed"
$malformedLog = LogOf "$OutDir\malformed" "skyrim-rs.log"
$malformedFile = FileOf "$OutDir\malformed" "skyrim-rs.settings.json"
$badFile = FileOf "$OutDir\malformed" "skyrim-rs.settings.json.bad"
$released = HostLines '^Escape released the focused widget; the overlay stays open$'
$closedByEscape = HostLines '^overlay closed with Escape$'
$controlMap = HostLines "^keys: skyrim-rs\.(toggle F11|close Escape) \(Overlay\): in the game's control map"
$setLine = HostLines '^keys: skyrim-rs-example\.toggle-window set to Shift\+L$'
$refusedLine = HostLines '^keys: F11 for skyrim-rs-example\.toggle-window refused: F11 is bound to skyrim-rs\.toggle$'
$hostWritten = HostLines '^settings written to .*\\skyrim-rs\.settings\.json$'
$reported = @($malformedLog | Where-Object { $_ -match "^skyrim-rs: settings not read, .*\\skyrim-rs\.settings\.json: malformed at line 3, column 3: expected ','; renamed to .*\\skyrim-rs\.settings\.json\.bad, the defaults are used$" })

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
       ok = ((Lines 17) -match '^hook compatibility mode refuse' -and (Lines 17) -match 'site Actor::ModActorValueImpl at 0x[0-9a-f]+' -and (Lines 17) -match '#1 skyrim-rs-example Before rank 0' -and (Lines 17) -match 'skyrim-rs-hooks-b Around' -and (Lines 17) -match 'message: refused: Actor::ModActorValueImpl: ordering cycle') }
    @{ name = "native"; plugin = "SetDamageScale(1.5) = $(Returned 18)"; devbench = "health $($health[0]) -> $($health[1]) after a damage of 10";
       ok = ((Near (Returned 18) 1.5 0.001) -and (Near $health[0] 100 0.01) -and (Near $health[1] 84 0.01)) }
    @{ name = "toggle off"; plugin = "$(Lines 23); #1 ran $(EntryCalls 24 1) then $(EntryCalls 28 1) times while the site ran $(SiteCalls 24) then $(SiteCalls 28); plugin a's Before $aBefore"; devbench = "health $($health[1]) -> $($health[2]) after a damage of 10";
       ok = ((Lines 23) -match 'SkyrimRs: #1 disabled' -and (Lines 28) -match '#1 skyrim-rs-example Before rank 0: \d+ calls, last \d+ ns, mean \d+ ns, disabled' -and (EntryCalls 24 1) -ne $null -and (EntryCalls 24 1) -eq (EntryCalls 28 1) -and [int](SiteCalls 28) -gt [int](SiteCalls 24) -and $aBefore -match 'saw \[-15\.0, -10\.0, -15\.0\]' -and (Near $health[2] 73 0.01)) }
    @{ name = "toggle on"; plugin = (Lines 29); devbench = "health $($health[2]) -> $($health[3]) after a damage of 10";
       ok = ((Lines 29) -match 'SkyrimRs: #1 enabled' -and (Near $health[3] 57 0.01)) }
    @{ name = "reload"; plugin = "(host log) $($reloading -join ' | '); $($detached -join ' | '); $($reloaded -join ' | ')"; devbench = "SkyrimRs hooks after the reload: example as #$newId; SkyrimRs window: $(Captured $window2 '(plugin 0: [^|]*)')";
       ok = ($reloading.Count -eq 1 -and $detached.Count -eq 1 -and $reloaded.Count -eq 1 -and $newId -and [int]$newId -gt 1 -and (Lines 36) -notmatch '\|\s+#1 skyrim-rs-example Before rank' -and (Lines 36) -notmatch '\|\s+#\d+ skyrim-rs-hooks-b' -and $window2 -match 'plugin 0: skyrim-rs-example generation 2') }
    @{ name = "settings"; plugin = "(host log) $($settingsWritten.Count) writes; damage_scale on disk $scaleOnDisk"; devbench = "health $($health[3]) -> $($health[4]) after the reload (scale 1.5 kept); SetDamageScale(0.5) = $(Returned 51)";
       ok = ($settingsWritten.Count -ge 2 -and (Near $scaleOnDisk 0.5 0.0001) -and (Near (Returned 51) 0.5 0.001) -and (Near $health[4] 41 0.01)) }
    @{ name = "capture"; plugin = "(host log) $($frameWritten -join ' | '); kept as gui-1.7.104.jpg: $shot"; devbench = "journal captured by DevBench, kept as gui-vanilla-1.7.104.jpg: $vanilla";
       ok = ($frameWritten.Count -eq 1 -and $shot -and $vanilla) }
    @{ name = "saves"; plugin = "(hooks log) $(@($hooksLog | Where-Object { $_ -match '^summary hooks [ab]Save[12]' }) -join ' | ')"; devbench = "saveGame seen: $(@(34, 54, 104 | ForEach-Object { $i = $_; ($steps | Where-Object { $_.index -eq $i }).satisfied }) -join ', '); rebind: $(($rebind | Where-Object { $_.index -eq 16 }).satisfied)";
       ok = (@(34, 54, 104 | Where-Object { $i = $_; ($steps | Where-Object { $_.index -eq $i }).satisfied -eq $true }).Count -eq 3 -and ($rebind | Where-Object { $_.index -eq 16 }).satisfied -eq $true) }
    @{ name = "escape"; plugin = "(host log) $($released -join ' | '); $($closedByEscape -join ' | ')"; devbench = "after Tab: $(LinesOf $steps 62); Escape: $(LinesOf $steps 65), menus $(Menus $steps 66); Escape: $(LinesOf $steps 69), menus $(Menus $steps 70); game's Escape at saves 2 and 3: $(Seen $testLog 1 '0x01') / $(Seen $testLog 2 '0x01')";
       ok = ($released.Count -eq 1 -and $closedByEscape.Count -eq 1 -and (LinesOf $steps 59) -match 'overlay open' -and (LinesOf $steps 62) -match 'overlay open, a widget has the keyboard focus' -and (LinesOf $steps 65) -match 'overlay open, no widget has the keyboard focus' -and (LinesOf $steps 69) -match 'overlay closed' -and (Menus $steps 66) -notmatch 'Journal Menu|no answer' -and (Menus $steps 70) -notmatch 'Journal Menu|no answer' -and (Seen $testLog 1 '0x01') -match '^press [1-9]' -and (Seen $testLog 1 '0x01') -eq (Seen $testLog 2 '0x01')) }
    @{ name = "F11 both ways"; plugin = "(host log) $((HostLines '^overlay (opened|closed) with F11$').Count) toggles"; devbench = "$(LinesOf $steps 73); $(LinesOf $steps 76); game's F11 at the last save: $(Seen $testLog 2 '0x57')";
       ok = ((LinesOf $steps 73) -match 'overlay open' -and (LinesOf $steps 76) -match 'overlay closed' -and (Seen $testLog 0 '0x57') -eq 'not seen' -and (Seen $testLog 1 '0x57') -eq 'not seen' -and (Seen $testLog 2 '0x57') -eq 'not seen') }
    @{ name = "F11 in the game"; plugin = "(host log) $($controlMap -join ' | ')"; devbench = "rebind session, F11 with the overlay closed: menus $(Menus $rebind 4) -> $(Menus $rebind 8); the game's sink saw F11: $(Seen $rebindSeen 0 '0x57')";
       ok = ($controlMap.Count -eq 2 -and (Seen $rebindSeen 0 '0x57') -match '^press 1 hold \d+ release 1' -and (Menus $rebind 8) -notmatch 'no answer') }
    @{ name = "action"; plugin = "(example log) $($fired -join ' | ')"; devbench = "SkyrimRs window after Shift+K: $(Captured (LinesOf $steps 85) '(window skyrim-rs-example/main [^|]*)'); K and Alt+K before it, Shift+K after the rebind and Shift+L with the overlay closed fired nothing";
       ok = ($fired.Count -eq 2 -and $fired[0] -eq 'toggle-window fired on Shift+K: window hidden' -and $fired[1] -eq 'toggle-window fired on Shift+L: window shown' -and (LinesOf $steps 85) -match 'window skyrim-rs-example/main "Example mod": closed') }
    @{ name = "capture"; plugin = "(host log) $($setLine -join ' | '); on disk: $(Captured $hostFile '("key\.skyrim-rs-example\.toggle-window": "[^"]*")')"; devbench = "$(LinesOf $steps 86); $(Captured (LinesOf $steps 89) '(key 2 [^|]*)')";
       ok = ($setLine.Count -eq 1 -and (LinesOf $steps 86) -match 'press the key for skyrim-rs-example\.toggle-window' -and (LinesOf $steps 89) -match 'key 2 skyrim-rs-example\.toggle-window: Shift\+L \(default Shift\+K, Overlay' -and $hostFile -match '"key\.skyrim-rs-example\.toggle-window": "Shift\+L"') }
    @{ name = "conflict"; plugin = "(host log) $($refusedLine -join ' | ')"; devbench = "SkyrimRs keys: $(LinesOf $steps 97)";
       ok = ($refusedLine.Count -eq 1 -and (LinesOf $steps 97) -match 'overlay open' -and (LinesOf $steps 97) -match 'keys: F11 for skyrim-rs-example\.toggle-window refused: F11 is bound to skyrim-rs\.toggle' -and (LinesOf $steps 102) -match 'overlay closed') }
    @{ name = "host file"; plugin = "(host log) $($hostWritten.Count) writes of skyrim-rs.settings.json"; devbench = ($hostFile -replace '\s+', ' ');
       ok = ($hostWritten.Count -ge 1 -and $hostFile -match '"key\.skyrim-rs\.toggle": "F11"' -and $hostFile -match '"key\.skyrim-rs\.close": "Escape"' -and $hostFile -match '"overlay\.capture": "focus"' -and $hostFile -match '"window\.skyrim-rs-example\.main": true') }
    @{ name = "rebind by file"; plugin = "(rebind host log) $(@($rebindLog | Where-Object { $_ -match '^overlay installed' }) -join ' | '); on disk: $($rebindFile -replace '\s+', ' ')"; devbench = "F11: $(LinesOf $rebind 7); F10: $(LinesOf $rebind 11); F10: $(LinesOf $rebind 14); the game's sink saw F10: $(Seen $rebindSeen 0 '0x44')";
       ok = (@($rebindLog | Where-Object { $_ -match '^overlay installed, toggled with F10, closed with Escape$' }).Count -eq 1 -and (LinesOf $rebind 7) -match 'overlay closed' -and (LinesOf $rebind 7) -match 'key 0 skyrim-rs\.toggle: F10 ' -and (LinesOf $rebind 11) -match 'overlay open' -and (LinesOf $rebind 14) -match 'overlay closed' -and (Seen $rebindSeen 0 '0x44') -eq 'not seen' -and $rebindFile -match '"key\.skyrim-rs\.toggle": "F10"' -and $rebindFile -match '"by\.hand": "kept"') }
    @{ name = "malformed"; plugin = "(malformed host log) $($reported -join ' | ')"; devbench = ".bad holds what was written: $($badFile -match '^\{\n  "key\.skyrim-rs\.toggle": "F10"\n  "window'); new file: $($malformedFile -replace '\s+', ' '); $(LinesOf $malformed 4); F11: $(LinesOf $malformed 7); F11: $(LinesOf $malformed 10)";
       ok = ($reported.Count -eq 1 -and $badFile -match '^\{\n  "key\.skyrim-rs\.toggle": "F10"\n  "window' -and $malformedFile -match '"key\.skyrim-rs\.toggle": "F11"' -and $malformedFile -notmatch 'F10|by\.hand' -and (LinesOf $malformed 4) -match 'key 0 skyrim-rs\.toggle: F11 ' -and (LinesOf $malformed 7) -match 'overlay open' -and (LinesOf $malformed 10) -match 'overlay closed') }
)
