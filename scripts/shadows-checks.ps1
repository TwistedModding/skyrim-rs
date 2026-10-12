# DevBench comparisons for the shadows suite (step indices of shadows-steps.json); dot-sourced by verify.ps1.
# The first-person-shadows mod runs next to the test plugin: the test plugin's render tests still describe the scene,
# the mod's own log at the save is the acceptance evidence, and the two captures are archived for a human check.
$Phases = "dataLoaded", "postLoadGame"

$modLines = Get-Content "$OutDir\skyrim-rs-first-person-shadows.log"
$save = ($modLines | Where-Object { $_ -match '^save: ' } | Select-Object -Last 1)
$shown = ($modLines | Where-Object { $_ -match '^first person: third-person root' } | Select-Object -Last 1)
$bodyShadow = Captured $save 'visited (\d+) times by shadow cameras'
$bodyCamera = Captured $save 'and (\d+) by other cameras'
$headSkipped = Captured $save 'face skipped (\d+) times'
$headShadow = Captured $save 'passed (\d+) times to shadow cameras'

$rows = @(
    @{ name = "hooks";           plugin = ($modLines | Where-Object { $_ -match '^hooked ' }) -join ' '; devbench = "SKSE loaded the mod next to the test plugin";
       ok = (($modLines -match '^hooked NiAVObject::Cull').Count -eq 1) }
    @{ name = "view";            plugin = $shown; devbench = "camera get after setPov first: pov $((Step 13).pov) stateId $((Step 13).stateId); at the save: pov $((Step 20).pov)";
       ok = ((Step 13).pov -eq "first" -and (Step 20).pov -eq "first" -and $shown -ne $null) }
    @{ name = "body shadow";     plugin = $save; devbench = "first-person frames between setPov first and the save";
       ok = ([int]$bodyShadow -gt 0 -and [int]$bodyCamera -gt 0 -and [int]$headSkipped -gt 0 -and [int]$headShadow -gt 0) }
    @{ name = "captures";        plugin = "render-third.png then render-first.png (first person with the body shown)"; devbench = "render-third.png $((Step 10).bytes) bytes, render-first.png $((Step 15).bytes) bytes";
       ok = ((Step 10).ok -eq $true -and (Step 15).ok -eq $true) }
)
