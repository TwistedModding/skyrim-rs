# DevBench comparisons for the animation suite (step indices of animation-steps.json); dot-sourced by verify.ps1.
# The plugin registers on the player's graph 0 after the load, samples the head bone at the pose write and at the
# world update, and reports at the save after the walk, the weapon draw, the sent event and the switch to first person.
$Phases = "dataLoaded", "postLoadGame", "save1"

$graphs = RunDetail "animation save1" "graphs"
$variables = RunDetail "animation save1" "variables"
$weapon = RunDetail "animation save1" "weapon_state"
$events = RunDetail "animation save1" "events"
$pose = RunDetail "animation save1" "pose_write"
$world = RunDetail "animation save1" "world_update"

$rows = @(
    @{ name = "graphs";       plugin = $graphs; devbench = "camera get after setPov first: pov $((Step 19).pov) (active graph 1 expected)";
       ok = ($graphs -match '^2 graphs, active 1') }
    @{ name = "variables";    plugin = $variables; devbench = "Actor.GetAnimationVariableFloat(Speed) = $(Returned 20), IsWeaponDrawn = $(Returned 21)";
       ok = ((Near (Captured $variables '^Speed ([-0-9.]+)') (Returned 20) 1.0) -and $variables -match 'highest Speed during the run') }
    @{ name = "weapon state"; plugin = $weapon; devbench = "IsWeaponDrawn = $(Returned 21), DrawWeapon called = $((Step 9).called)";
       ok = ((Returned 21) -eq $true -and $weapon -match 'weapon state 3') }
    @{ name = "events";       plugin = $events; devbench = "4.5 s walk, DrawWeapon, SendAnimationEvent(IdleForceDefaultState) called = $((Step 11).called)";
       ok = ($events -match 'FootLeft x\d+' -and $events -match 'BeginWeaponDraw x\d+' -and (Step 11).called -eq $true) }
    @{ name = "pose write";   plugin = $pose; devbench = "frames of the run";
       ok = ($pose -match '^pose writes: third-person graph \d+, first-person graph \d+') }
    @{ name = "world update"; plugin = $world; devbench = "frames of the run";
       ok = ($world -match '^\d+ world updates') }
    @{ name = "capture";      plugin = "animation-front.png (third person, from the front, weapon drawn)"; devbench = "animation-front.png $((Step 15).bytes) bytes";
       ok = ((Step 15).ok -eq $true) }
)
