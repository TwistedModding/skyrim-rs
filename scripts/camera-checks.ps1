# DevBench comparisons for the camera suite (step indices of camera-steps.json); dot-sourced by verify.ps1.
# The camera-demo plugin is driven through its console command and reports each mode with PASS/FAIL lines and a
# "summary camera <label>" line; the comparisons set its read-backs against DevBench's camera tool, the console's
# getini and the captured console output of the same session.
$Phases = "orbit", "offset", "fov", "near5", "near15", "shadow", "furniture"

function Cam($i) { $r = Step $i; if ($r) { "({0:F1}, {1:F1}, {2:F1})" -f $r.camX, $r.camY, $r.camZ } else { "" } }
function Root($line) { if ($line -match 'root \(([-0-9.]+), ([-0-9.]+), ([-0-9.]+)\)') { @([double]$Matches[1], [double]$Matches[2], [double]$Matches[3]) } else { $null } }
function CamNear($i, $root) { $r = Step $i; ($null -ne $root) -and ($null -ne $r) -and (Near $r.camX $root[0] 1.0) -and (Near $r.camY $root[1] 1.0) -and (Near $r.camZ $root[2] 1.0) }
function Ini($i) { Captured (Lines $i) '>> ([-0-9.]+)' }
# Whether DevBench's camera position of step $i lies on the orbit the plugin reported: radius from the centre within 1.5 units.
function OnOrbit($i, $line) {
    $r = Step $i
    if (-not $r -or $line -notmatch 'center \(([-0-9.]+), ([-0-9.]+), ([-0-9.]+)\) radius ([-0-9.]+)') { return $false }
    $d = [math]::Sqrt(([double]$r.camX - [double]$Matches[1]) * ([double]$r.camX - [double]$Matches[1]) + ([double]$r.camY - [double]$Matches[2]) * ([double]$r.camY - [double]$Matches[2]) + ([double]$r.camZ - [double]$Matches[3]) * ([double]$r.camZ - [double]$Matches[3]))
    [math]::Abs($d - [double]$Matches[4]) -lt 1.5
}

$orbit = RunDetail "camera orbit" "written"
$offset = RunDetail "camera offset" "written"
$offsetRoot = Root (Lines 16)
$fov = RunDetail "camera fov" "hands_frustum"
$near5 = RunDetail "camera near5" "gpu"
$near15 = RunDetail "camera near15" "gpu"
$shadow = RunDetail "camera shadow" "cascades"
$furniture = RunDetail "camera furniture" "fields"
$furnitureZoom = Captured (Lines 59) 'zoom ([-0-9.]+)'
$furniturePitch = Captured (Lines 59) 'pitch ([-0-9.]+)'

$rows = @(
    @{ name = "hooks";        plugin = ($ReportLines | Where-Object { $_ -match '^state step sites' }) -join ' '; devbench = "SKSE loaded the demo next to the test plugin; CameraDemo took over a console entry";
       ok = (($ReportLines -match '^state step sites').Count -eq 1 -and ($ReportLines -match '^CameraDemo took over').Count -eq 1) }
    @{ name = "orbit";        plugin = "$orbit; console: $(Lines 12)"; devbench = "camera get before $(Cam 8), during the orbit $(Cam 11)";
       ok = ((Step 11).stateId -eq 9 -and (OnOrbit 11 (Lines 12)) -and ([math]::Abs((Step 11).camX - (Step 8).camX) + [math]::Abs((Step 11).camY - (Step 8).camY)) -gt 10) }
    @{ name = "offset";       plugin = "$offset; console: $(Lines 16)"; devbench = "camera get with the offset $(Cam 15)";
       ok = ((Step 15).stateId -eq 9 -and (CamNear 15 $offsetRoot)) }
    @{ name = "fov";          plugin = "$fov; console: $(Lines 25)"; devbench = "camera get: pov $((Step 21).pov) then $((Step 24).pov) stateId $((Step 24).stateId)";
       ok = ((Step 21).pov -eq "third" -and (Step 24).pov -eq "first" -and (Step 24).stateId -eq 0) }
    @{ name = "near 5";       plugin = "$near5; console: $(Lines 32)"; devbench = "getini fNearDistance:Display: $(Lines 31)";
       ok = ((Near (Ini 31) 5 0.01)) }
    @{ name = "near 15";      plugin = "$near15; console: $(Lines 36)"; devbench = "getini fNearDistance:Display: $(Lines 35)";
       ok = ((Near (Ini 35) 15 0.01)) }
    @{ name = "shadow";       plugin = "$shadow; console: $(Lines 46)"; devbench = "camera get: pov $((Step 41).pov) stateId $((Step 41).stateId); camera-shadow.png $((Step 44).bytes) bytes";
       ok = ((Step 41).pov -eq "first" -and (Step 44).ok -eq $true) }
    # A chair of the base save (0x8B825, base 0x6E7A8): the furniture camera is current from the activation until the sit-enter event pops it.
    @{ name = "furniture";    plugin = "$furniture; console: $(Lines 59)"; devbench = "camera get after the activation: stateId $((Step 54).stateId), seated: stateId $((Step 56).stateId); getgs fFurnitureCameraZoom $(Lines 57), fFurnitureCameraAngle $(Lines 58)";
       ok = ((Step 54).stateId -eq 5 -and (Step 56).stateId -ne 5 -and (Near $furnitureZoom (Ini 57) 0.01) -and (Near $furniturePitch (Ini 58) 0.001)) }
)
