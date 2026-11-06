# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the sample suite (step indices of sample-steps.json); dot-sourced by verify.ps1.
# The sample plugin logs the player and a keyword at DataLoaded, every menu event from then on, and the player again
# when its console command runs after the load.
$Phases = @()

$stream = (Get-Content "$OutDir\events.json" -Raw | ConvertFrom-Json).events
$dataLoadedSeq = ($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "dataLoaded" } | Select-Object -First 1).seq
$dbMenus = @($stream | Where-Object { $_.topic -eq "menu" -and $_.seq -gt $dataLoadedSeq } | ForEach-Object { "$($_.data.name):$([int]$_.data.opening)" })
$pluginMenus = @($ReportLines | Where-Object { $_ -match '^menu (.+) (opened|closed)$' } | ForEach-Object { "$($Matches[1]):$(if ($Matches[2] -eq 'opened') { 1 } else { 0 })" })
$command = @($ReportLines | Where-Object { $_ -match '^command: ' }) -join ' '
$player = Step 5
$keywords = Captured $command 'race keywords \[(.*)\]$'
$keywordCount = if ($keywords) { @($keywords -split ', ').Count } else { 0 }
$form = @($ReportLines | Where-Object { $_ -match '^form 0008F958 ' }) -join ' '

$rows = @(
    @{ name = "command";  plugin = $command; devbench = "console: $(Lines 4)";
       ok = ($command -ne "" -and (Lines 4) -eq ($command -replace '^command: ', '')) }
    @{ name = "player";   plugin = $command; devbench = "inspect player: name $($player.name), race $($player.race.formId) $($player.race.editorId), health $($player.actorValues.health.current)";
       ok = ($player -and $command -match "player `"$([regex]::Escape($player.name))`"" -and $command -match 'race "NordRace" 00013746' -and $player.race.formId -eq "0x00013746" -and (Near (Captured $command 'health ([-0-9.]+)') $player.actorValues.health.current 0.01)) }
    @{ name = "keywords"; notRun = $(if ($Launch -eq "standalone") { "DevBench asks through Form.GetNumKeywords, a script extender native the standalone loader does not provide" }); plugin = "$keywordCount`: $keywords"; devbench = "NordRace GetNumKeywords = $(Returned 6)";
       ok = ($keywordCount -gt 0 -and $keywordCount -eq (Returned 6)) }
    @{ name = "form";     plugin = $form; devbench = "help VendorItemWeapon 4: $(Lines 7)";
       ok = ($form -match '"VendorItemWeapon"' -and $form -match 'Ok\(KYWD\)' -and (Lines 7) -match 'KYWD: VendorItemWeapon \(0008F958\)') }
    @{ name = "menus";    plugin = ($pluginMenus -join ','); devbench = ($dbMenus -join ',');
       ok = ($pluginMenus.Count -gt 0 -and ($pluginMenus -contains "TweenMenu:1") -and ($pluginMenus -contains "TweenMenu:0") -and (($pluginMenus -join ',') -eq (@($dbMenus | Select-Object -First $pluginMenus.Count) -join ','))) }
)
