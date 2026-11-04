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
