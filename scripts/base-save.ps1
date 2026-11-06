# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# A scenario starts from its runtime's base save. 1.7.104 has retrace-base; on any other runtime the first run writes
# one from the main menu (coc Riverwood lands the default character where retrace-base stands), before anything is
# installed, so no save ever crosses runtimes.
param(
    [string]$Runtime = "1.7.104",
    [string]$GameDir = "",
    [Parameter(Mandatory)][string]$OutDir,
    [switch]$WaitForGame
)
$ErrorActionPreference = "Stop"
$Install = & "$PSScriptRoot\runtimes.ps1" -Runtime $Runtime -GameDir $GameDir
$save = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\Saves\$($Install.baseSave).ess"
if (Test-Path $save) { return }
if ($Install.canonical) { throw "$save is missing" }
if (-not $Install.canSave) { Write-Warning "$($Install.dir) has no Data\Scripts\Game.pex: no base save can be written"; return }
& "$PSScriptRoot\devbench.ps1" -Runtime $Runtime -GameDir $GameDir -Steps "$PSScriptRoot\base-save-steps.json" -OutDir "$OutDir\base-save" -WaitForGame:$WaitForGame
if (-not (Test-Path $save)) { throw "the base save was not written; see $OutDir\base-save" }
