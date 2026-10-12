# DevBench comparisons for the render suite (step indices of render-steps.json); dot-sourced by verify.ps1.
# The plugin reads the scene after the load; the scenario then goes third person, first person and into the inn,
# and the caster counts are reported at the save.
$Phases = "dataLoaded", "postLoadGame", "saveGame"

$view = Detail "view"
$scene = Detail "scene"
$indoors = Detail "indoors"
$casters = Detail "casters"
function SceneLights($i) { $r = Step $i; if ($r -and $r.lights) { @($r.lights).Count } else { $null } }
function ShadowLights($i) { $r = Step $i; if ($r -and $r.lights) { @($r.lights | Where-Object { $_.inScene -eq "shadow" }).Count } else { $null } }

$rows = @(
    @{ name = "view at load";     plugin = $view; devbench = "camera get after setPov third: pov $((Step 8).pov) stateId $((Step 8).stateId); after setPov first: pov $((Step 13).pov) stateId $((Step 13).stateId)";
       ok = ((Step 8).pov -eq "third" -and (Step 8).stateId -eq 9 -and (Step 13).pov -eq "first" -and (Step 13).stateId -eq 0 -and $plugin["view"].ok) }
    @{ name = "outdoor lights";   plugin = $scene; devbench = "inspect lights scope=scene: $(SceneLights 9) lights ($(ShadowLights 9) in the shadow list) in third person, $(SceneLights 14) ($(ShadowLights 14)) in first person";
       ok = ($plugin["scene"].ok -and (SceneLights 9) -ne $null) }
    @{ name = "indoor lights";    plugin = $indoors; devbench = "inspect lights scope=scene in the inn: $(SceneLights 19) lights ($(ShadowLights 19) in the shadow list), pov $((Step 20).pov)";
       ok = ($plugin["indoors"].ok -and (ShadowLights 19) -ge 1) }
    @{ name = "captures";         plugin = "third then first person"; devbench = "render-third.png $((Step 10).bytes) bytes frame $((Step 10).frame), render-first.png $((Step 15).bytes) bytes frame $((Step 15).frame)";
       ok = ((Step 10).ok -eq $true -and (Step 15).ok -eq $true) }
    @{ name = "casters";          plugin = $casters; devbench = "two switches through DevBench setPov (third, first) and the coc";
       ok = ($plugin["casters"].ok -and $plugin["switches"].ok) }
)
