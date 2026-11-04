# DevBench comparisons for the frame suite (step indices of frame-steps.json); dot-sourced by verify.ps1.
# The plugin samples the player camera after every TESCamera::Update; the scenario goes third person, orbits 180
# degrees, goes first person, toggles the free camera and saves; the save reports the samples and the scene reads.
$Phases = "dataLoaded", "postLoadGame", "save1"

$third = RunDetail "frame save1" "third_person"
$orbit = RunDetail "frame save1" "orbit"
$first = RunDetail "frame save1" "first_person"
$free = RunDetail "frame save1" "free_camera"
$window = RunDetail "frame save1" "window"
$sky = RunDetail "frame save1" "sky"
function Pos($i) { $r = Step $i; if ($r) { "({0:F1}, {1:F1}, {2:F1})" -f $r.camX, $r.camY, $r.camZ } else { "" } }
function Yaw($i) { $r = Step $i; if ($r -and $r.thirdPersonState) { $r.thirdPersonState.currentYaw } else { $null } }

$rows = @(
    @{ name = "third person";  plugin = $third; devbench = "camera get: pov $((Step 7).pov) stateId $((Step 7).stateId) at $(Pos 7), yaw $(Yaw 7), offsets $((Step 7).thirdPersonState.posOffsetActual -join ',')";
       ok = ((Step 7).stateId -eq 9 -and $third -match 'state 9 stack 0' -and (Near (Captured $third 'yaw ([-0-9.]+)') (Yaw 7) 0.05)) }
    @{ name = "orbit";         plugin = $orbit; devbench = "camera get during the orbit: yaw $(Yaw 10), free rotation $((Step 10).thirdPersonState.freeRotation -join ','), orbit on $((Step 10).orbit.on)";
       ok = ($orbit -match '^orbit: free rotation' -and (Step 10).orbit.on -eq $true) }
    @{ name = "first person";  plugin = $first; devbench = "camera get: pov $((Step 15).pov) stateId $((Step 15).stateId) at $(Pos 15)";
       ok = ((Step 15).stateId -eq 0 -and $first -match 'state 0 stack 0') }
    @{ name = "free camera";   plugin = $free; devbench = "camera get with tfc: pov $((Step 18).pov) stateId $((Step 18).stateId) freeCam $((Step 18).freeCam) at $(Pos 18)";
       ok = ((Step 18).stateId -eq 3 -and (Step 18).freeCam -eq $true -and $free -match 'state 3 stack 1') }
    @{ name = "window";        plugin = $window; devbench = "the game window";
       ok = ($window -match 'window \d+x\d+ hwnd 0x') }
    @{ name = "sky"; notRun = (PluginNotRun "sky");           plugin = $sky; devbench = "inspect scene: gameHour $((Step 21).gameHour)";
       ok = ((Near (Captured $sky 'game hour ([0-9.]+)') (Step 21).gameHour 0.2)) }
)
