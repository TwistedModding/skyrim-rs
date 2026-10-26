# DevBench comparisons for the framework suite (step indices of framework-steps.json); dot-sourced by verify.ps1.
# The host (skyrim_rs.dll) loads the suite's plugins from Data\SKSE\Plugins\skyrim-rs in the order their descriptors
# resolve to (order-head by priority, then the test plugin, order-tail after all; the order-cycle pair refused, the decoy
# without a descriptor skipped); each loaded plugin appends its name to skyrim-rs-order.log. DevBench calls the test
# plugin's native and opens and closes the host's overlay; the plugin reports at the save.
$Phases = "load", "dataLoaded", "postLoadGame", "saveGame"

$orderLog = if (Test-Path "$OutDir\skyrim-rs-order.log") { @(Get-Content "$OutDir\skyrim-rs-order.log") } else { @() }
$ExpectedOrder = "skyrim-rs-order-head", "skyrim-rs-framework-test", "skyrim-rs-order-tail"
$loaded = @($hostLog | Where-Object { $_ -match '^loaded \S+: (.*)$' } | ForEach-Object { $Matches[1] })
$decoy = @($hostLog | Where-Object { $_ -match '^skipped decoy\.dll: no SkyrimRs_Plugin export$' })
$orderLine = @($hostLog | Where-Object { $_ -match '^load order: ' })
$missing = @($hostLog | Where-Object { $_ -match '^no plugin named skyrim-rs-nowhere \(asked by skyrim-rs-order-head\)$' })
$cycle = @($hostLog | Where-Object { $_ -match '^skipped skyrim_rs_order_cycle_[ab]\.dll: skyrim-rs-order-cycle-[ab] is in a load order cycle with skyrim-rs-order-cycle-[ab]$' })
$hostHandle = Loaded "skyrim_rs.dll"
$subLoader = @($LoaderLog | Where-Object { $_ -match 'skyrim_rs_framework_test|skyrim_rs_order|decoy' })

$rows = @(
    @{ name = "host loads";   plugin = "$(Detail 'interface') | (host log) $(@($hostLog -match '^hosted by ') -join ' | ')"; devbench = "${LoaderLogName}: skyrim_rs.dll handle $hostHandle; subfolder DLLs named by the loader: $($subLoader.Count)";
       ok = ($plugin["interface"].ok -and $hostHandle -ne $null -and $subLoader.Count -eq 0 -and (Hosted)) }
    @{ name = "plugins loaded"; plugin = ($hostLog -join ' | '); devbench = "(host log) loaded: $($loaded -join ', '); decoy refused: $($decoy.Count)";
       ok = ((Compare-Object $loaded $ExpectedOrder -SyncWindow 0).Count -eq 0 -and $decoy.Count -eq 1) }
    @{ name = "load order";   plugin = (Detail "order"); devbench = "(host log) $($orderLine -join ' | '); skyrim-rs-order.log: $($orderLog -join ', ')";
       ok = ($plugin["order"].ok -and $orderLine -eq "load order: skyrim-rs-order-head (after [skyrim-rs-nowhere], priority 1), skyrim-rs-framework-test, skyrim-rs-order-tail (after [*], priority 9)" -and (Compare-Object $orderLog $ExpectedOrder -SyncWindow 0).Count -eq 0) }
    @{ name = "order reports"; plugin = "cycle plugins in skyrim-rs-order.log: $(@($orderLog -match 'cycle').Count)"; devbench = "(host log) $($missing -join ' | ') | $($cycle -join ' | ')";
       ok = ($missing.Count -eq 1 -and $cycle.Count -eq 2 -and @($orderLog -match 'cycle').Count -eq 0) }
    @{ name = "messages";     plugin = "$(Detail 'startup_messages') | $(Detail 'load_messages')"; devbench = "load retrace-base, postLoadGame seen: $(($steps | Where-Object { $_.index -eq 1 }).satisfied)";
       ok = ($plugin["startup_messages"].ok -and $plugin["load_messages"].ok) }
    @{ name = "native";       plugin = "$(Detail 'papyrus') | $(Detail 'native')"; devbench = "SkyrimRsFrameworkTest.Twice(21) = $(Returned 4) ($((Step 4).returnedType))";
       ok = ($plugin["native"].ok -and (Returned 4) -eq 42) }
    @{ name = "window";       plugin = (Detail "window"); devbench = "F11 taps ok = $((Step 5) -ne $null), $((Step 7) -ne $null)";
       ok = ($plugin["window"].ok -and (Step 5) -ne $null -and (Step 7) -ne $null) }
    @{ name = "save trigger"; plugin = ($summaries -match 'framework saveGame') -join ' '; devbench = "waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 10 }).satisfied)";
       ok = (($steps | Where-Object { $_.index -eq 10 }).satisfied -eq $true) }
)
