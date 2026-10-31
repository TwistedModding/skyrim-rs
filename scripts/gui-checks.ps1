# DevBench comparisons for the gui suite (step indices of gui-steps.json, two parts merged: 56 steps, then 70); dot-sourced
# by verify.ps1. The host runs the example mod (first in load order, so its Before on Actor::ModActorValueImpl is
# subscription #1; its tab "Example mod" has the sections Damage and About) and the hooks suite's plugins a and b, which
# add no tab. Part 1: "SkyrimRs gui" opens the overlay on its first page and lists the pages (the example's two sections,
# Plugins, Legacy plugins, Settings), the framework plugins and the SKSE plugins; "SkyrimRs capture 0" writes that frame,
# W is held for 2 s with nothing focused (focus capture: the player moves), then pages 1 to 4 are shown and captured in
# turn, page 1 (About) is selected again and F11 closes the overlay. "SkyrimRs hooks" lists the sites, the example's
# native sets the damage scale to 1.5, then three damages of 10 with health read back: with #1 on (100 -> 84: 15, a +1,
# b doubles and halves), after "SkyrimRs toggle 1" (-> 73: the scale skipped) and after toggling it on again (-> 57); the
# save makes b remove itself and a report what its Before saw. Between the parts the example's DLL is copied in again and
# the host reloads it as generation 2. Part 2: the new generation's subscription, one damage of 10 (-> 41: the scale kept,
# b gone), "SkyrimRs gui" (the reloaded plugin's tab still on About), F11, Escape opens the game's journal for DevBench's
# capture, Escape, the native sets the scale to 0.5 and the save. Then the keys (74 on): F11 opens, Tab focuses a widget,
# Escape releases it (overlay still open, no journal), Escape closes the overlay (no journal), F11 opens and closes it,
# then the example's open-tab action (Scope::Game, Shift+K) with the overlay closed: K and Alt+K do nothing, Shift+K opens
# the overlay on the example's tab; "SkyrimRs bind 2" captures Shift+L for it, F11 closes, Shift+K no longer opens and
# Shift+L does, a second capture of F11 is refused, F11 closes, and the save makes the test plugin list every key its sink
# (after the game's handlers) saw. The host's and the example's settings files are read after the part; then two more
# launches: rebind (the toggle set to scan code 68 by hand: F11 reaches the game and no longer toggles, F10 does) and
# malformed (the file reported, renamed to .bad and replaced by the defaults: F11 toggles again), their files in
# <OutDir>\<session>.
$Phases = @()

$hostLog = if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" } else { @() }
function HostLines($pattern) { @($hostLog | Where-Object { $_ -match $pattern }) }
$loaded = HostLines '^loaded skyrim_rs_(example_mod|hooks_a|hooks_b)\.dll: skyrim-rs-(example|hooks-a|hooks-b)$'
$font = HostLines '^overlay font: Futura CondensedLight from .*fonts_en\.swf$'
$cursorLine = HostLines "^overlay cursor: the game's, from .*cursormenu\.swf$"
$command = HostLines '^command SkyrimRs took over console opcode 0x[0-9a-f]+'
$reloading = HostLines '^skyrim-rs-example: skyrim_rs_example_mod\.dll changed, reloading as generation 2$'
$detached = HostLines '^skyrim-rs-example: generation 1: 1 hook subscriptions, 0 sinks, 1 natives stubbed, 0 commands stubbed, 0 engine commands restored, 2 tab sections, 0 HUD painters, 0 message handlers, 0 interfaces parked, 1 Papyrus callbacks, 1 frame callbacks detached$'
$reloaded = HostLines '^skyrim-rs-example: generation 2 loaded, image at 0x[0-9a-f]+$'
$settingsWritten = HostLines '^settings written to .*skyrim-rs-example\.settings\.json$'
$framesWritten = HostLines '^overlay frame written to .*skyrim-rs-overlay-[0-4]\.bmp$'

function Pos($i) { Captured (Lines $i) '>>\s*(-?[\d.]+)' }
$moved = if ((Pos 9) -and (Pos 15)) { [math]::Sqrt([math]::Pow([double](Pos 15) - [double](Pos 9), 2) + [math]::Pow([double](Pos 16) - [double](Pos 10), 2)) } else { 0 }
$health = @(39, 42, 47, 52, 59 | ForEach-Object { Returned $_ })
$gui1 = Lines 5
$gui2 = Lines 60
# The calls counted for subscription #1 and for its site in a "SkyrimRs hooks" output.
function EntryCalls($i, $id) { Captured (Lines $i) "#$id skyrim-rs-example Before rank \d+: (\d+) calls" }
function SiteCalls($i) { Captured (Lines $i) 'site Actor::ModActorValueImpl at 0x[0-9a-f]+: (\d+) calls' }
# Plugin a's report at the first save (its own log: the suite's report is the example's): the deltas its Before saw.
$hooksLog = if (Test-Path "$OutDir\skyrim-rs-hooks.log") { Get-Content "$OutDir\skyrim-rs-hooks.log" } else { @() }
$aBefore = Captured (($hooksLog | Where-Object { $_ -match '^(PASS|FAIL) a_before: ' }) -join ' ') '(saw \[[^\]]*\])'
$newId = Captured (Lines 56) '#(\d+) skyrim-rs-example Before'
$settingsFile = "$OutDir\skyrim-rs-example.settings.json"
$scaleOnDisk = if (Test-Path $settingsFile) { Captured (Get-Content $settingsFile -Raw) '"damage_scale":\s*([-\d.]+)' } else { $null }

# One capture per page, as "SkyrimRs gui <n>" numbers them.
$pageNames = @("example-damage", "example-about", "plugins", "legacy", "settings")
$shots = @(0..4 | ForEach-Object { HalfJpeg "$SkseLogs\skyrim-rs-overlay-$_.bmp" "$OutDir\gui-$($pageNames[$_])-1.7.104.jpg" })
$vanillaPath = (Step 66).path
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
$fired = @($exampleLog | Where-Object { $_ -match '^open-tab fired' })
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
$setLine = HostLines '^keys: skyrim-rs-example\.open-tab set to Shift\+L$'
$refusedLine = HostLines '^keys: F11 for skyrim-rs-example\.open-tab refused: F11 is bound to skyrim-rs\.toggle$'
$hostWritten = HostLines '^settings written to .*\\skyrim-rs\.settings\.json$'
$reported = @($malformedLog | Where-Object { $_ -match "^skyrim-rs: settings not read, .*\\skyrim-rs\.settings\.json: malformed at line 3, column 3: expected ','; renamed to .*\\skyrim-rs\.settings\.json\.bad, the defaults are used$" })
$pages = @($gui1 -split ' \| ' | Where-Object { $_ -match '^page \d+: ' })
$expectedPages = @("page 0: Example mod / Damage, shown", "page 1: Example mod / About", "page 2: Plugins", "page 3: Legacy plugins", "page 4: Settings")
$shownPages = @(17, 21, 25, 29 | ForEach-Object { Captured (Lines $_) '(page \d+: [^|,]*), shown' })

$rows = @(
    @{ name = "host loads"; plugin = "(host log) $($loaded -join ' | '); $($command -join ' | ')"; devbench = "SkyrimRs gui: $(@($gui1 -split ' \| ' | Where-Object { $_ -match '^plugin ' }) -join ' | ')";
       ok = ($loaded.Count -eq 3 -and $command.Count -eq 1 -and $gui1 -match 'plugin 0: skyrim-rs-example 0\.1\.0 \(skyrim_rs_example_mod\.dll\): Running( \||$)' -and $gui1 -match 'plugin 1: skyrim-rs-hooks-a 0\.1\.0 \(skyrim_rs_hooks_a\.dll\): Running' -and $gui1 -match 'plugin 2: skyrim-rs-hooks-b 0\.1\.0 \(skyrim_rs_hooks_b\.dll\): Running') }
    @{ name = "font"; plugin = "(host log) $($font -join ' | ')"; devbench = "SkyrimRs gui says font $(Captured $gui1 'font (.*?), capture')";
       ok = ($font.Count -eq 1 -and $gui1 -match 'font Futura CondensedLight from') }
    @{ name = "cursor"; plugin = "(host log) $($cursorLine -join ' | ')"; devbench = "(drawn in the captures)";
       ok = ($cursorLine.Count -eq 1) }
    @{ name = "pages"; plugin = "SkyrimRs gui lists $($pages.Count) pages"; devbench = ($pages -join ' | ');
       ok = (($pages -join '|') -eq ($expectedPages -join '|')) }
    @{ name = "page selection"; plugin = "SkyrimRs gui 1 to 4"; devbench = ($shownPages -join ' | ');
       ok = (($shownPages -join '|') -eq 'page 1: Example mod / About|page 2: Plugins|page 3: Legacy plugins|page 4: Settings') }
    @{ name = "legacy plugins"; plugin = "(from SkyrimRs gui)"; devbench = (@($gui1 -split ' \| ' | Where-Object { $_ -match '^legacy' }) -join ' | ');
       ok = ($gui1 -match 'legacy plugins, loaded by the script extender:' -and $gui1 -match 'legacy devbench\.dll: devbench 1\.28\.0 by alandtse, loaded' -and $gui1 -match 'legacy skyrim_rs\.dll: skyrim-rs [\d.]+ by \w+, loaded' -and $gui1 -match 'legacy skyrim_rs_test\.dll: skyrim-rs-test [\d.]+ by \w+, loaded' -and @($gui1 -split ' \| ' | Where-Object { $_ -match '^legacy .*, not loaded$' }).Count -eq 0) }
    @{ name = "captures"; plugin = "(host log) $($framesWritten.Count) frames written"; devbench = "kept as gui-<page>-1.7.104.jpg: $($shots -join ', '); journal captured by DevBench, kept as gui-vanilla-1.7.104.jpg: $vanilla";
       ok = ($framesWritten.Count -eq 5 -and @($shots | Where-Object { $_ }).Count -eq 5 -and $vanilla) }
    @{ name = "focus capture"; plugin = "overlay open with nothing focused, capture $(Captured $gui1 'capture (\w+)')"; devbench = "player moved $([math]::Round($moved, 1)) units during the 2 s W hold (x $(Pos 9) -> $(Pos 15), y $(Pos 10) -> $(Pos 16))";
       ok = ($gui1 -match 'capture Focus' -and $moved -gt 50) }
    @{ name = "hook sites"; plugin = "SkyrimRs hooks: $(Lines 37)"; devbench = "(listed through the console)";
       ok = ((Lines 37) -match '^hook compatibility mode refuse' -and (Lines 37) -match 'site Actor::ModActorValueImpl at 0x[0-9a-f]+' -and (Lines 37) -match '#1 skyrim-rs-example Before rank 0' -and (Lines 37) -match 'skyrim-rs-hooks-b Around' -and (Lines 37) -match 'message: refused: Actor::ModActorValueImpl: ordering cycle') }
    @{ name = "native"; plugin = "SetDamageScale(1.5) = $(Returned 38)"; devbench = "health $($health[0]) -> $($health[1]) after a damage of 10";
       ok = ((Near (Returned 38) 1.5 0.001) -and (Near $health[0] 100 0.01) -and (Near $health[1] 84 0.01)) }
    @{ name = "toggle off"; plugin = "$(Lines 43); #1 ran $(EntryCalls 44 1) then $(EntryCalls 48 1) times while the site ran $(SiteCalls 44) then $(SiteCalls 48); plugin a's Before $aBefore"; devbench = "health $($health[1]) -> $($health[2]) after a damage of 10";
       ok = ((Lines 43) -match 'SkyrimRs: #1 disabled' -and (Lines 48) -match '#1 skyrim-rs-example Before rank 0: \d+ calls, last \d+ ns, mean \d+ ns, disabled' -and (EntryCalls 44 1) -ne $null -and (EntryCalls 44 1) -eq (EntryCalls 48 1) -and [int](SiteCalls 48) -gt [int](SiteCalls 44) -and $aBefore -match 'saw \[-15\.0, -10\.0, -15\.0\]' -and (Near $health[2] 73 0.01)) }
    @{ name = "toggle on"; plugin = (Lines 49); devbench = "health $($health[2]) -> $($health[3]) after a damage of 10";
       ok = ((Lines 49) -match 'SkyrimRs: #1 enabled' -and (Near $health[3] 57 0.01)) }
    @{ name = "reload"; plugin = "(host log) $($reloading -join ' | '); $($detached -join ' | '); $($reloaded -join ' | ')"; devbench = "SkyrimRs hooks after the reload: example as #$newId; SkyrimRs gui: $(Captured $gui2 '(plugin 0: [^|]*)')";
       ok = ($reloading.Count -eq 1 -and $detached.Count -eq 1 -and $reloaded.Count -eq 1 -and $newId -and [int]$newId -gt 1 -and (Lines 56) -notmatch '\|\s+#1 skyrim-rs-example Before rank' -and (Lines 56) -notmatch '\|\s+#\d+ skyrim-rs-hooks-b' -and $gui2 -match 'plugin 0: skyrim-rs-example 0\.1\.0 \(skyrim_rs_example_mod\.dll\): Running, reloaded once') }
    @{ name = "tab after reload"; plugin = "About selected before the reload"; devbench = "SkyrimRs gui after it: $(@($gui2 -split ' \| ' | Where-Object { $_ -match '^page [01]: ' }) -join ' | ')";
       ok = ($gui2 -match 'page 0: Example mod / Damage( \||$)' -and $gui2 -match 'page 1: Example mod / About, shown') }
    @{ name = "settings"; plugin = "(host log) $($settingsWritten.Count) writes; damage_scale on disk $scaleOnDisk"; devbench = "health $($health[3]) -> $($health[4]) after the reload (scale 1.5 kept); SetDamageScale(0.5) = $(Returned 69)";
       ok = ($settingsWritten.Count -ge 2 -and (Near $scaleOnDisk 0.5 0.0001) -and (Near (Returned 69) 0.5 0.001) -and (Near $health[4] 41 0.01)) }
    @{ name = "saves"; plugin = "(hooks log) $(@($hooksLog | Where-Object { $_ -match '^summary hooks [ab]Save[12]' }) -join ' | ')"; devbench = "saveGame seen: $(@(54, 72, 124 | ForEach-Object { $i = $_; ($steps | Where-Object { $_.index -eq $i }).satisfied }) -join ', '); rebind: $(($rebind | Where-Object { $_.index -eq 16 }).satisfied)";
       ok = (@(54, 72, 124 | Where-Object { $i = $_; ($steps | Where-Object { $_.index -eq $i }).satisfied -eq $true }).Count -eq 3 -and ($rebind | Where-Object { $_.index -eq 16 }).satisfied -eq $true) }
    @{ name = "escape"; plugin = "(host log) $($released -join ' | '); $($closedByEscape -join ' | ')"; devbench = "after Tab: $(LinesOf $steps 80); Escape: $(LinesOf $steps 83), menus $(Menus $steps 84); Escape: $(LinesOf $steps 87), menus $(Menus $steps 88); game's Escape at saves 2 and 3: $(Seen $testLog 1 '0x01') / $(Seen $testLog 2 '0x01')";
       ok = ($released.Count -eq 1 -and $closedByEscape.Count -eq 1 -and (LinesOf $steps 77) -match 'overlay open' -and (LinesOf $steps 80) -match 'overlay open, a widget has the keyboard focus' -and (LinesOf $steps 83) -match 'overlay open, no widget has the keyboard focus' -and (LinesOf $steps 87) -match 'overlay closed' -and (Menus $steps 84) -notmatch 'Journal Menu|no answer' -and (Menus $steps 88) -notmatch 'Journal Menu|no answer' -and (Seen $testLog 1 '0x01') -match '^press [1-9]' -and (Seen $testLog 1 '0x01') -eq (Seen $testLog 2 '0x01')) }
    @{ name = "F11 both ways"; plugin = "(host log) $((HostLines '^overlay (opened|closed) with F11$').Count) toggles"; devbench = "$(LinesOf $steps 91); $(LinesOf $steps 94); game's F11 at the last save: $(Seen $testLog 2 '0x57')";
       ok = ((LinesOf $steps 91) -match 'overlay open' -and (LinesOf $steps 94) -match 'overlay closed' -and (Seen $testLog 0 '0x57') -eq 'not seen' -and (Seen $testLog 1 '0x57') -eq 'not seen' -and (Seen $testLog 2 '0x57') -eq 'not seen') }
    @{ name = "F11 in the game"; plugin = "(host log) $($controlMap -join ' | ')"; devbench = "rebind session, F11 with the overlay closed: menus $(Menus $rebind 4) -> $(Menus $rebind 8); the game's sink saw F11: $(Seen $rebindSeen 0 '0x57')";
       ok = ($controlMap.Count -eq 2 -and (Seen $rebindSeen 0 '0x57') -match '^press 1 hold \d+ release 1' -and (Menus $rebind 8) -notmatch 'no answer') }
    @{ name = "action"; plugin = "(example log) $($fired -join ' | ')"; devbench = "after K and Alt+K: $(Captured (LinesOf $steps 99) '(overlay \w+)'); after Shift+K: $(Captured (LinesOf $steps 102) '(overlay \w+)'), $(Captured (LinesOf $steps 103) '(page \d+: [^|]*, shown)'); the game's K at the last save: $(Seen $testLog 2 '0x25')";
       ok = ($fired.Count -eq 2 -and $fired[0] -eq 'open-tab fired on Shift+K: tab shown' -and $fired[1] -eq 'open-tab fired on Shift+L: tab shown' -and (LinesOf $steps 99) -match 'overlay closed' -and (LinesOf $steps 102) -match 'overlay open' -and (LinesOf $steps 103) -match 'page [01]: Example mod / \w+, shown' -and (Seen $testLog 2 '0x25') -match '^press ([3-9]|\d\d) ') }
    @{ name = "capture"; plugin = "(host log) $($setLine -join ' | '); on disk: $(Captured $hostFile '("key\.skyrim-rs-example\.open-tab": "[^"]*")')"; devbench = "$(LinesOf $steps 104); $(Captured (LinesOf $steps 107) '(key 2 [^|]*)'); Shift+K after it: $(Captured (LinesOf $steps 112) '(overlay \w+)'); Shift+L: $(Captured (LinesOf $steps 115) '(overlay \w+)')";
       ok = ($setLine.Count -eq 1 -and (LinesOf $steps 104) -match 'press the key for skyrim-rs-example\.open-tab' -and (LinesOf $steps 107) -match 'key 2 skyrim-rs-example\.open-tab: Shift\+L \(default Shift\+K, Game' -and (LinesOf $steps 112) -match 'overlay closed' -and (LinesOf $steps 115) -match 'overlay open' -and $hostFile -match '"key\.skyrim-rs-example\.open-tab": "Shift\+L"') }
    @{ name = "conflict"; plugin = "(host log) $($refusedLine -join ' | ')"; devbench = "SkyrimRs keys: $(LinesOf $steps 119)";
       ok = ($refusedLine.Count -eq 1 -and (LinesOf $steps 119) -match 'overlay open' -and (LinesOf $steps 119) -match 'keys: F11 for skyrim-rs-example\.open-tab refused: F11 is bound to skyrim-rs\.toggle' -and (LinesOf $steps 122) -match 'overlay closed') }
    @{ name = "host file"; plugin = "(host log) $($hostWritten.Count) writes of skyrim-rs.settings.json"; devbench = ($hostFile -replace '\s+', ' ');
       ok = ($hostWritten.Count -ge 1 -and $hostFile -match '"key\.skyrim-rs\.toggle": "F11"' -and $hostFile -match '"key\.skyrim-rs\.close": "Escape"' -and $hostFile -match '"overlay\.capture": "focus"' -and $hostFile -notmatch '"window\.') }
    @{ name = "rebind by file"; plugin = "(rebind host log) $(@($rebindLog | Where-Object { $_ -match '^overlay installed' }) -join ' | '); on disk: $($rebindFile -replace '\s+', ' ')"; devbench = "F11: $(LinesOf $rebind 7); F10: $(LinesOf $rebind 11); F10: $(LinesOf $rebind 14); the game's sink saw F10: $(Seen $rebindSeen 0 '0x44')";
       ok = (@($rebindLog | Where-Object { $_ -match '^overlay installed, toggled with F10, closed with Escape$' }).Count -eq 1 -and (LinesOf $rebind 7) -match 'overlay closed' -and (LinesOf $rebind 7) -match 'key 0 skyrim-rs\.toggle: F10 ' -and (LinesOf $rebind 11) -match 'overlay open' -and (LinesOf $rebind 14) -match 'overlay closed' -and (Seen $rebindSeen 0 '0x44') -eq 'not seen' -and $rebindFile -match '"key\.skyrim-rs\.toggle": "F10"' -and $rebindFile -match '"by\.hand": "kept"') }
    @{ name = "malformed"; plugin = "(malformed host log) $($reported -join ' | ')"; devbench = ".bad holds what was written: $($badFile -match '^\{\n  "key\.skyrim-rs\.toggle": "F10"\n  "overlay'); new file: $($malformedFile -replace '\s+', ' '); $(LinesOf $malformed 4); F11: $(LinesOf $malformed 7); F11: $(LinesOf $malformed 10)";
       ok = ($reported.Count -eq 1 -and $badFile -match '^\{\n  "key\.skyrim-rs\.toggle": "F10"\n  "overlay' -and $malformedFile -match '"key\.skyrim-rs\.toggle": "F11"' -and $malformedFile -notmatch 'F10|by\.hand' -and (LinesOf $malformed 4) -match 'key 0 skyrim-rs\.toggle: F11 ' -and (LinesOf $malformed 7) -match 'overlay open' -and (LinesOf $malformed 10) -match 'overlay closed') }
)
