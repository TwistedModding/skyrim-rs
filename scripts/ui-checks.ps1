# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the ui suite (step indices of ui-steps.json); dot-sourced by verify.ps1.
# The plugin shows and hides its own menu and sends a notification on the first frame after the load, samples the
# stack in every draw and reports at the save after TweenMenu, the Papyrus notification and the message box.
$Phases = "dataLoaded", "postLoadGame", "saveGame"

$stream = (Get-Content "$OutDir\events.json" -Raw | ConvertFrom-Json).events
$loadSeq = ($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "postLoadGame" } | Select-Object -Last 1).seq
$dbMenus = @($stream | Where-Object { $_.topic -eq "menu" -and $_.seq -gt $loadSeq } | ForEach-Object { "$($_.data.name):$([int]$_.data.opening)" })
function Open($i) { $r = Step $i; if ($r) { @($r.openMenus) -join "," } else { "" } }

$rows = @(
    @{ name = "stack in play"; plugin = (Detail "stack_play"); devbench = "menu list after the load: $(Open 4)";
       ok = ($plugin["stack_play"].ok -and (Open 4) -match 'HUD Menu' -and (Open 4) -notmatch 'TweenMenu|MessageBoxMenu') }
    @{ name = "stack TweenMenu"; plugin = (Detail "stack_tween"); devbench = "menu list with TweenMenu open: $(Open 7)";
       ok = ($plugin["stack_tween"].ok -and (Open 7) -match 'TweenMenu' -and (Open 7) -match 'Cursor Menu') }
    @{ name = "stack message box"; plugin = (Detail "stack_box"); devbench = "menu list with the box: $(Open 14), messageBoxOpen $((Step 14).messageBoxOpen); after accept: $(Open 17), messageBoxOpen $((Step 17).messageBoxOpen)";
       ok = ($plugin["stack_box"].ok -and (Step 14).messageBoxOpen -eq $true -and (Step 17).messageBoxOpen -eq $false) }
    @{ name = "own menu"; plugin = (Detail "own_menu"); devbench = "menu events after the load: $($dbMenus -join ',')";
       ok = ($plugin["own_menu"].ok -and $plugin["stack_own"].ok -and ($dbMenus -contains "SkyrimRsTestMenu:1") -and ($dbMenus -contains "SkyrimRsTestMenu:0")) }
    @{ name = "messages";        plugin = (Detail "messages"); devbench = "menu events after the load: $($dbMenus -join ',')";
       ok = ($plugin["messages"].ok -and ($dbMenus -contains "TweenMenu:1") -and ($dbMenus -contains "TweenMenu:0") -and ($dbMenus -contains "MessageBoxMenu:1") -and ($dbMenus -contains "MessageBoxMenu:0")) }
    @{ name = "notification";    plugin = (Detail "notification"); devbench = "Debug.Notification(probe-hud) called = $((Step 10).called)";
       ok = ($plugin["notification"].ok -and (Step 10).called -eq $true) }
    @{ name = "scaleform";       plugin = (Detail "scaleform"); devbench = "(plugin only: the HUD movie's own answers)";
       ok = $plugin["scaleform"].ok }
    @{ name = "save trigger";    plugin = ($summaries -match 'ui saveGame') -join ' '; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 19 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 19 }).satisfied -eq $true) }
)
