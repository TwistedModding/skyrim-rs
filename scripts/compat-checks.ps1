# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# DevBench comparisons for the compat suite (the hooks scenario, step indices of hooks-steps.json); dot-sourced by
# verify.ps1. Next to the hooks suite's plugins, skyrim_legacy_patch_a.dll (loaded before skyrim_runtime_extender.dll by file name)
# detours Actor::ModActorValueImpl with MinHook in its SKSEPlugin_Load and logs the player's health damage it receives;
# skyrim_z_legacy_patch.dll (loaded after it) detours TESObjectREFR::GetScale, where hooks plugin a has an After, at
# PLUGINS_LOADED_2, after the host's check of that moment. skyrim-runtime-extender.ini sets the host's mode to -Compat.
# refuse: the host reports a's patch at startup and refuses both hooks plugins' subscriptions on ModActorValueImpl, so
# the damage reaches the game unedited (100, 90, 85, 75) through a's detour alone; it finds z's overwrite at DATA_LOADED
# and stops running GetScale's subscribers. stack: the host installs on top of a's patch, the hooks plugins' chain runs
# as in the hooks suite (100, 89, 89, 78) and ends in a's detour, which sees the halved delta; z's overwrite is reported
# and GetScale's subscribers keep running behind it.
$Phases = @()

function LogOf($name) { @(if (Test-Path "$OutDir\$name") { Get-Content "$OutDir\$name" }) }
$legacyA = LogOf "skyrim-rs-legacy-patch-a.log"
$legacyZ = LogOf "skyrim-rs-legacy-patch-z.log"
$hooksLog = LogOf "skyrim-rs-hooks.log"
$handles = @("skyrim_legacy_patch_a.dll", "skyrim_runtime_extender.dll", "skyrim_z_legacy_patch.dll" | ForEach-Object { Loaded $_ })
$hooked = @{}
foreach ($l in $legacyA + $legacyZ) { if ($l -match '^hooked (\S+) at (0x[0-9a-f]+)$') { $hooked[$Matches[1]] = $Matches[2] } }
$deltas = @($legacyA | Where-Object { $_ -match '^player health damage (-?[\d.]+)$' } | ForEach-Object { [double]$Matches[1] })
$scaleCalls = Captured ($legacyZ -join ' ') 'GetScale ran (\d+) times'
$health = @(5, 8, 11, 17 | ForEach-Object { Returned $_ })
$mode = @($hostLog | Where-Object { $_ -eq "hooks: compatibility mode $Compat (skyrim-runtime-extender.ini)" })
$unmanaged = @($hostLog | Where-Object { $_ -match "^hooks: startup: Actor::ModActorValueImpl at $($hooked['Actor::ModActorValueImpl']): patched by skyrim_legacy_patch_a\.dll: unmanaged patch, found e9 " })
$overwrite = @($hostLog | Where-Object { $_ -match "^hooks: (\w+): TESObjectREFR::GetScale at $($hooked['TESObjectREFR::GetScale']): managed detour overwritten by skyrim_z_legacy_patch\.dll: " })
$overwriteMoment = Captured ($overwrite -join ' ') '^hooks: (\w+):'
$scaleDiagnostics = @($hooksLog | Where-Object { $_ -match '^diagnostics: TESObjectREFR::GetScale at .*, patched by skyrim_z_legacy_patch\.dll' })
function Result($run, $name) { if ($runs[$run] -and $runs[$run][$name]) { $runs[$run][$name] } else { $null } }
$scaleResult = Result "hooks aSave1" "a_scale_calls"

$rows = @(
    @{ name = "load order"; plugin = "(host log) $(@($hostLog -match '^hosted by ') -join ' | ')"; devbench = "${LoaderLogName}: handles legacy a $($handles[0]), host $($handles[1]), legacy z $($handles[2])";
       ok = ((Hosted) -and $handles[0] -ne $null -and $handles[0] -lt $handles[1] -and $handles[1] -lt $handles[2]) }
    @{ name = "mode"; plugin = "(host log) $($mode -join ' | ')"; devbench = "skyrim-runtime-extender.ini: compat = $Compat"; ok = ($mode.Count -eq 1) }
    @{ name = "unmanaged patch"; plugin = "(host log) $($unmanaged -join ' | ')"; devbench = "(legacy a) hooked Actor::ModActorValueImpl at $($hooked['Actor::ModActorValueImpl'])";
       ok = ($unmanaged.Count -eq 1 -and $hooked['Actor::ModActorValueImpl']) }
)
if ($Compat -eq "refuse") {
    $refusedA = RunDetail "hooks aLoad" "a_subscribe"
    $refusedB = RunDetail "hooks bLoad" "b_subscribe"
    $site = 'Actor::ModActorValueImpl: refused for skyrim-rs-hooks-{0}, unmanaged patch, found e9 .*, patched by skyrim_legacy_patch_a\.dll$'
    $rows += @(
        @{ name = "refused"; plugin = "$refusedA | $refusedB"; devbench = "(host log) $(@($hostLog -match '^hooks: refused: Actor::ModActorValueImpl: ') -join ' | ')";
           ok = ($refusedA -match ($site -f 'a') -and $refusedB -match ($site -f 'b') -and @($hostLog -match '^hooks: refused: Actor::ModActorValueImpl: ').Count -eq 3) }
        @{ name = "legacy alone"; plugin = "(legacy a) deltas $($deltas -join ', ')"; devbench = "health $($health -join ' -> ')";
           ok = ((Compare-Object $deltas @(-10, -5, -10) -SyncWindow 0).Count -eq 0 -and (Near $health[0] 100 0.01) -and (Near $health[1] 90 0.01) -and (Near $health[2] 85 0.01) -and (Near $health[3] 75 0.01)) }
        @{ name = "overwritten"; plugin = "(host log) $($overwrite -join ' | ')"; devbench = "(legacy z) hooked TESObjectREFR::GetScale at $($hooked['TESObjectREFR::GetScale']), ran $scaleCalls times";
           ok = ($overwrite.Count -eq 1 -and $overwriteMoment -eq "DATA_LOADED" -and $overwrite[0] -match '; broken, its subscribers no longer run$' -and [int]$scaleCalls -gt 0) }
        @{ name = "broken"; plugin = "a_scale_calls: $(if ($scaleResult) { $scaleResult.detail }) | $($scaleDiagnostics -join ' | ')"; devbench = "(legacy z) GetScale ran $scaleCalls times";
           ok = ($scaleResult -and -not $scaleResult.ok -and $scaleResult.detail -match 'none since' -and @($scaleDiagnostics -match ', broken').Count -ge 1) }
    )
} else {
    $stacked = @($hostLog | Where-Object { $_ -match "^hooks: Actor::ModActorValueImpl at $($hooked['Actor::ModActorValueImpl']): stacking on the patch by skyrim_legacy_patch_a\.dll: " })
    $chain = "a_subscribe", "b_subscribe", "a_before", "b_before", "b_around", "a_after", "a_outer", "b_remove", "b_silent", "a_before_final", "a_after_final"
    $chainResults = foreach ($n in $chain) { $r = foreach ($run in $runs.Keys) { if ($run -like "hooks *" -and $runs[$run][$n]) { $runs[$run][$n] } }; @{ name = $n; ok = [bool]($r -and $r.ok) } }
    $rows += @(
        @{ name = "stacked"; plugin = "(host log) $($stacked -join ' | ')"; devbench = "(hooks plugins) $(@($chainResults | Where-Object { $_.ok }).Count) of $($chain.Count) chain checks pass";
           ok = ($stacked.Count -eq 1 -and @($chainResults | Where-Object { -not $_.ok }).Count -eq 0) }
        @{ name = "chain reaches legacy"; plugin = "(legacy a) deltas $($deltas -join ', ')"; devbench = "health $($health -join ' -> ')";
           ok = ((Compare-Object $deltas @(-11, -11) -SyncWindow 0).Count -eq 0 -and (Near $health[0] 100 0.01) -and (Near $health[1] 89 0.01) -and (Near $health[2] 89 0.01) -and (Near $health[3] 78 0.01)) }
        @{ name = "overwritten"; plugin = "(host log) $($overwrite -join ' | ')"; devbench = "(legacy z) hooked TESObjectREFR::GetScale at $($hooked['TESObjectREFR::GetScale']), ran $scaleCalls times";
           ok = ($overwrite.Count -eq 1 -and $overwriteMoment -eq "DATA_LOADED" -and $overwrite[0] -match '; stack mode, its subscribers still run behind it$' -and [int]$scaleCalls -gt 0) }
        @{ name = "still running"; plugin = "a_scale_calls: $(if ($scaleResult) { $scaleResult.detail }) | $($scaleDiagnostics -join ' | ')"; devbench = "(legacy z) GetScale ran $scaleCalls times";
           ok = ($scaleResult -and $scaleResult.ok -and $scaleDiagnostics.Count -ge 1 -and @($scaleDiagnostics -match ', broken').Count -eq 0) }
    )
}
