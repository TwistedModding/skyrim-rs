# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the save suite (step indices of save-steps.json); dot-sourced by verify.ps1.
# The plugin writes a record next to the suite's first save when it is written, drops its state when a load starts, reads
# the record back after each load and reports at the second.
$Phases = "dataLoaded", "saveGame"

$stream = (Get-Content "$OutDir\events.json" -Raw | ConvertFrom-Json).events
$dbLoads = @($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "postLoadGame" }).Count
$dbSaves = @($stream | Where-Object { $_.topic -eq "lifecycle" -and $_.data.event -eq "saveGame" }).Count

$rows = @(
    @{ name = "events";      plugin = (Detail "events"); devbench = "postLoadGame events $dbLoads, saveGame events $dbSaves";
       ok = ($plugin["events"].ok -and $dbLoads -eq 3 -and $dbSaves -eq 2) }
    @{ name = "gold";        plugin = (Detail "records"); devbench = "additem: $(Lines 4); after loading $($Install.baseSave): $(Lines 12); after loading skyrim-rs-save-$($Install.tag): $(Lines 17)";
       ok = ($plugin["records"].ok -and (Lines 12) -match '140' -and (Lines 17) -match '145') }
    @{ name = "remap";       plugin = (Detail "remap"); devbench = "(plugin only: the load order is unchanged)";
       ok = $plugin["remap"].ok }
    @{ name = "saved files"; plugin = (Detail "saved_files"); devbench = "(plugin only)";
       ok = $plugin["saved_files"].ok }
)
