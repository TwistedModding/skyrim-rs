# DevBench comparisons for the framework suite (step indices of framework-steps.json); dot-sourced by verify.ps1.
# The host (skyrim_rs.dll) loads the suite's plugin from Data\SKSE\Plugins\skyrim-rs and refuses the decoy next to it;
# DevBench calls the plugin's native and opens and closes the host's overlay; the plugin reports at the save.
$Phases = "load", "dataLoaded", "postLoadGame", "saveGame"

$hostLog = if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" } else { @() }
$skseLog = if (Test-Path "$OutDir\skse64.log") { Get-Content "$OutDir\skse64.log" } else { @() }
$loaded = @($hostLog | Where-Object { $_ -match '^loaded skyrim_rs_framework_test\.dll: skyrim-rs-framework-test$' })
$decoy = @($hostLog | Where-Object { $_ -match '^skipped decoy\.dll: no SkyrimRs_Plugin export$' })
$hostSkse = @($skseLog | Where-Object { $_ -match 'skyrim_rs\.dll.*loaded correctly' })
$subSkse = @($skseLog | Where-Object { $_ -match 'skyrim_rs_framework_test|decoy' })

$rows = @(
    @{ name = "host loads";   plugin = (Detail "interface"); devbench = "skse64.log: $($hostSkse -join ' | '); subfolder DLLs named by SKSE: $($subSkse.Count)";
       ok = ($plugin["interface"].ok -and $hostSkse.Count -eq 1 -and $subSkse.Count -eq 0) }
    @{ name = "plugin loaded"; plugin = ($hostLog -join ' | '); devbench = "(host log) loaded: $($loaded.Count), decoy refused: $($decoy.Count)";
       ok = ($loaded.Count -eq 1 -and $decoy.Count -eq 1) }
    @{ name = "messages";     plugin = "$(Detail 'startup_messages') | $(Detail 'load_messages')"; devbench = "load retrace-base, postLoadGame seen: $(($steps | Where-Object { $_.index -eq 1 }).satisfied)";
       ok = ($plugin["startup_messages"].ok -and $plugin["load_messages"].ok) }
    @{ name = "native";       plugin = "$(Detail 'papyrus') | $(Detail 'native')"; devbench = "SkyrimRsFrameworkTest.Twice(21) = $(Returned 4) ($((Step 4).returnedType))";
       ok = ($plugin["native"].ok -and (Returned 4) -eq 42) }
    @{ name = "window";       plugin = (Detail "window"); devbench = "F11 taps ok = $((Step 5) -ne $null), $((Step 7) -ne $null)";
       ok = ($plugin["window"].ok -and (Step 5) -ne $null -and (Step 7) -ne $null) }
    @{ name = "save trigger"; plugin = ($summaries -match 'framework saveGame') -join ' '; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 10 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 10 }).satisfied -eq $true) }
)
