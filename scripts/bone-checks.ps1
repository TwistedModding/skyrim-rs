# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the bone suite (step indices of bone-steps.json); dot-sourced by verify.ps1.
# The bone-driver mod runs next to the test plugin and turns the player's head after every pose write; the mod's log
# at the save is the acceptance evidence, the test plugin's world-update sampling shows the edit reaching the world
# transforms, and the front capture in third person is archived for a human check.
$Phases = @()

$modLines = Get-Content "$OutDir\skyrim-rs-bone-driver.log"
$save = ($modLines | Where-Object { $_ -match '^save: ' } | Select-Object -Last 1)
$bound = ($modLines | Where-Object { $_ -match '^head bone ' } | Select-Object -Last 1)
$turned = Captured $save 'head turned in (\d+) pose writes'
$world = RunDetail "animation save1" "world_update"

$rows = @(
    @{ name = "hook";          plugin = ($modLines | Where-Object { $_ -match '^hooked ' }) -join ' '; devbench = "SKSE loaded the mod next to the test plugin";
       ok = (($modLines -match '^hooked ApplyPoseToBoneNodes').Count -eq 1 -and $bound -ne $null) }
    @{ name = "head turned";   plugin = $save; devbench = "frames between the load and the save";
       ok = ([int]$turned -gt 100 -and $save -match 'rows before \[.*\] after \[') }
    @{ name = "world update";  plugin = $world; devbench = "the test plugin's UpdateNodesAfterRender samples";
       ok = ($world -match '^\d+ world updates') }
    @{ name = "capture";       plugin = "bone-front.png (third person, from the front, head turned)"; devbench = "bone-front.png $((Step 15).bytes) bytes";
       ok = ((Step 15).ok -eq $true) }
)
