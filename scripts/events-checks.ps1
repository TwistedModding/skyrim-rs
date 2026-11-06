# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the events suite (step indices of events-steps.json); dot-sourced by verify.ps1.
# The plugin registers its sinks at DataLoaded and checks what they received at the save that ends the scenario.
# Spell.Cast fires along the caster's heading whatever target it is given, so step 19 turns the player onto the pine
# REFR 00034ECE, 667 units ahead on every base save; off its trunk the flames hit it only about half the time.
$Phases = "dataLoaded", "saveGame"

$stream = (Get-Content "$OutDir\events.json" -Raw | ConvertFrom-Json).events
$dataLoadedSeq = ($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "dataLoaded" } | Select-Object -First 1).seq
$dbMenus = @($stream | Where-Object { $_.topic -eq "menu" -and $_.seq -gt $dataLoadedSeq } | ForEach-Object { "$($_.data.name):$([int]$_.data.opening)" })
$pluginMenus = @((Captured (Detail "menu_open_close") 'sequence=\[([^\]]*)\]') -split ',')
$menuPrefix = @($dbMenus | Select-Object -First $pluginMenus.Count)
$dbLoads = @($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "postLoadGame" }).Count
$dbCells = @($stream | Where-Object { $_.topic -eq "scene.cellLoaded" })
$pluginLoads = Captured (Detail "load_game") '^(\d+) TESLoadGameEvent'
function Hex8($n) { "0x" + ("{0:X8}" -f [int]$n) }

$rows = @(
    @{ name = "menu sequence";   plugin = ($pluginMenus -join ","); devbench = "menu events after dataLoaded (seq $dataLoadedSeq), the plugin's snapshot ends at the save: $($dbMenus -join ',')";
       ok = ($pluginMenus.Count -gt 0 -and (($pluginMenus -join ",") -eq ($menuPrefix -join ","))) }
    @{ name = "menu open/close"; plugin = (Detail "menu_mode_change"); devbench = "menu open: $((Step 4) | ConvertTo-Json -Compress), close: $((Step 6) | ConvertTo-Json -Compress)";
       ok = ($plugin["menu_open_close"].ok -and $plugin["menu_mode_change"].ok -and $null -ne (Step 4) -and $null -ne (Step 6)) }
    @{ name = "equip";           plugin = (Detail "equip"); devbench = "EquipItem called=$((Step 8).called), UnequipItem called=$((Step 10).called)";
       ok = ($plugin["equip"].ok -and (Step 8).called -eq $true -and (Step 10).called -eq $true) }
    @{ name = "container";       plugin = (Detail "container_changed"); devbench = "AddItem called=$((Step 12).called), RemoveItem called=$((Step 14).called), GetItemCount(gold) = $(Returned 16)";
       ok = ($plugin["container_changed"].ok -and (Returned 16) -eq 145) }
    @{ name = "activate";        plugin = (Detail "activate"); devbench = "ObjectReference.Activate(0xECE8D, player) = $(Returned 17)";
       ok = ($plugin["activate"].ok -and (Returned 17) -eq $true) }
    @{ name = "hit";             plugin = (Detail "hit"); devbench = "Spell.Cast(Flames, player) called=$((Step 21).called)";
       ok = ($plugin["hit"].ok -and (Step 21).called -eq $true) }
    @{ name = "load game";       plugin = (Detail "load_game"); devbench = "postLoadGame lifecycle events: $dbLoads, cells loaded: $(($dbCells | ForEach-Object { "$($_.data.cell) $(Hex8 $_.data.formID)" }) -join ', ')";
       ok = ($pluginLoads -eq 2 -and $dbLoads -eq 2 -and $dbCells.Count -ge 1) }
    @{ name = "actor cell";      plugin = (Detail "actor_cell"); devbench = "scene.cellLoaded formID $(if ($dbCells.Count) { Hex8 $dbCells[0].data.formID })";
       ok = ($plugin["actor_cell"].ok -and $dbCells.Count -gt 0 -and $dbCells[0].data.formID -eq 0x9732) }
    @{ name = "save trigger";    plugin = ($summaries -match 'events saveGame') -join ' '; devbench = "console save: $((Step 30) | ConvertTo-Json -Compress), waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 31 }).satisfied)";
       ok = ((($steps | Where-Object { $_.index -eq 31 }).satisfied -eq $true)) }
)
