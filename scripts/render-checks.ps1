# DevBench comparisons for the render suite (step indices of render-steps.json); dot-sourced by verify.ps1.
# The plugin reads the scene after the load and at each save: save 1 outdoors in first person after the third- and
# first-person windows, save 2 in Dragonsreach; the caster counts accumulate over both windows.
$Phases = "dataLoaded", "postLoadGame", "save1", "save2"

$view = RunDetail "render save1" "view"
$scene = RunDetail "render save1" "scene"
$sun = RunDetail "render save1" "sun"
$indoors = RunDetail "render save2" "lights"
$casters = RunDetail "render save2" "casters"
function SceneLights($i) { $r = Step $i; if ($r -and $r.lights) { @($r.lights).Count } else { $null } }
function ShadowLights($i) { $r = Step $i; if ($r -and $r.lights) { @($r.lights | Where-Object { $_.inScene -eq "shadow" }).Count } else { $null } }

$rows = @(
    @{ name = "view";             plugin = $view; devbench = "camera get after setPov third: pov $((Step 8).pov) stateId $((Step 8).stateId); after setPov first: pov $((Step 13).pov) stateId $((Step 13).stateId)";
       ok = ((Step 8).pov -eq "third" -and (Step 8).stateId -eq 9 -and (Step 13).pov -eq "first" -and (Step 13).stateId -eq 0 -and $view -match 'third_person=false state=Some\(FirstPerson\)') }
    @{ name = "sun"; notRun = (PluginNotRun "sun");              plugin = $sun; devbench = "scene at save 1: gameHour $((Step 19).gameHour), weather $((Step 19).weather.formId)";
       ok = ($sun -match 'maps 2 \(viewport \[0, 4096, 4096, 0\]' -and $sun -match 'biasScale 60') }
    @{ name = "outdoor lights";   plugin = $scene; devbench = "inspect lights scope=scene: $(SceneLights 9) lights ($(ShadowLights 9) in the shadow list) in third person, $(SceneLights 14) ($(ShadowLights 14)) in first person";
       ok = ($scene -match '^world at' -and (SceneLights 9) -ne $null) }
    @{ name = "indoor lights"; notRun = (PluginNotRun "lights");    plugin = $indoors; devbench = "inspect lights scope=scene in Dragonsreach: $(SceneLights 29) lights ($(ShadowLights 29) in the shadow list), pov $((Step 30).pov)";
       ok = ($indoors -match ', [1-9]\d* shadow lights' -and (ShadowLights 29) -ge 1) }
    @{ name = "captures";         plugin = "third then first person"; devbench = "render-third.png $((Step 10).bytes) bytes frame $((Step 10).frame), render-first.png $((Step 17).bytes) bytes frame $((Step 17).frame)";
       ok = ((Step 10).ok -eq $true -and (Step 17).ok -eq $true) }
    @{ name = "casters"; notRun = (PluginNotRun "casters");          plugin = $casters; devbench = "two switches through DevBench setPov (third, first) and the coc";
       ok = ($casters -match '^third-person' -and ($runs["render save2"]["casters"].ok) -and ($runs["render save2"]["switches"].ok)) }
)
