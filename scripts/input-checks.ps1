# DevBench comparisons for the input suite (step indices of input-steps.json); dot-sourced by verify.ps1.
# The plugin records the keystrokes DevBench injects through its own sink on the input source and adds the SkyrimRsTest
# console command, which DevBench runs twice to open and close the ui suite's menu; the plugin reports at the save.
$Phases = "dataLoaded", "postLoadGame", "saveGame"

$stream = (Get-Content "$OutDir\events.json" -Raw | ConvertFrom-Json).events
$loadSeq = ($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "postLoadGame" } | Select-Object -Last 1).seq
$dbMenus = @($stream | Where-Object { $_.topic -eq "menu" -and $_.seq -gt $loadSeq } | ForEach-Object { "$($_.data.name):$([int]$_.data.opening)" })
function Open($i) { $r = Step $i; if ($r) { @($r.openMenus) -join "," } else { "" } }

$rows = @(
    @{ name = "key E";        plugin = (Detail "key_e"); devbench = "input tap e ok = $((Step 4) -ne $null)";
       ok = ($plugin["key_e"].ok -and (Step 4) -ne $null) }
    @{ name = "key W";        plugin = (Detail "key_w"); devbench = "input down w, 2 s, up w: ok = $((Step 6) -ne $null), $((Step 8) -ne $null)";
       ok = ($plugin["key_w"].ok -and (Step 6) -ne $null -and (Step 8) -ne $null) }
    @{ name = "key Tab";      plugin = (Detail "key_tab"); devbench = "two Tab taps; menu events after the load: $($dbMenus -join ',')";
       ok = ($plugin["key_tab"].ok -and ($dbMenus -contains "TweenMenu:1") -and ($dbMenus -contains "TweenMenu:0")) }
    @{ name = "contexts";     plugin = (Detail "contexts"); devbench = "TweenMenu opened and closed by the Tab taps";
       ok = ($plugin["contexts"].ok -and ($dbMenus -contains "TweenMenu:1")) }
    @{ name = "console path"; plugin = "(the console tool's own command)"; devbench = "player.getpos x: $(Lines 14)";
       ok = ((Lines 14) -match 'GetPos: X >>') }
    @{ name = "command show"; plugin = (Detail "command"); devbench = "SkyrimRsTest 1 player: $(Lines 15); menu list: $(Open 17)";
       ok = ($plugin["command"].ok -and (Lines 15) -match 'SkyrimRsTest: show 1 target 00000014' -and (Open 17) -match 'SkyrimRsTestMenu') }
    @{ name = "command hide"; plugin = (Detail "command"); devbench = "SkyrimRsTest 0: $(Lines 18); menu list: $(Open 20)";
       ok = ((Lines 18) -match 'SkyrimRsTest: show 0' -and (Open 20) -notmatch 'SkyrimRsTestMenu') }
    @{ name = "save trigger"; plugin = ($summaries -match 'input saveGame') -join ' '; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 22 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 22 }).satisfied -eq $true) }
)
