# DevBench comparisons for the forms suite (step indices of forms-steps.json); dot-sourced by verify.ps1.
$Phases = "postLoadGame"

$mods = Step 4
$dbFiles = @()
if ($mods) {
    $dbFiles += $mods.plugins | ForEach-Object { "compiled $($_.index):$($_.name)" }
    $dbFiles += $mods.lightPlugins | ForEach-Object { "light $($_.index):$($_.name)" }
}
$pluginFiles = @()
$filesDetail = Detail "files"
if ($filesDetail -match 'compiled=\[([^\]]*)\] light=\[([^\]]*)\]') {
    $compiled, $light = $Matches[1], $Matches[2]
    $pluginFiles += ($compiled -split ',') | ForEach-Object { if ($_ -match '^(\d+):([^:]+):') { "compiled $($Matches[1]):$($Matches[2])" } }
    $pluginFiles += ($light -split ',') | ForEach-Object { if ($_ -match '^(\d+):([^:]+):') { "light $($Matches[1]):$($Matches[2])" } }
}
$hour = Captured (Lines 14) 'GameHour = ([0-9.]+)'
$pluginHour = Captured (Detail "editor_id_global") 'GameHour=([0-9.]+)'
# 1.5.97's install has no Creations: nothing to look up, and the form counts below were read with two of them loaded.
$NoCreations = if (-not (Test-Path "$GameDir\Data\ccBGSSSE037-Curios.esl")) { "no Creations in this install" }

$rows = @(
    @{ name = "files";             plugin = ($pluginFiles -join ", "); devbench = ($dbFiles -join ", ");
       ok = ($pluginFiles.Count -gt 0 -and (($pluginFiles -join ",") -eq ($dbFiles -join ","))) }
    @{ name = "lookup_weapon";     plugin = (Detail "lookup_weapon"); devbench = "Game.GetForm(77495) = $((Returned 5).formId) type $((Returned 5).formType)";
       ok = ((Detail "lookup_weapon") -match 'formID=00012EB7 type=41' -and (Returned 5).formId -eq "0x00012EB7" -and (Returned 5).formType -eq "41") }
    @{ name = "get_file Skyrim";   plugin = (Detail "get_file"); devbench = "GetFormFromFile(77495, Skyrim.esm) = $((Returned 6).formId)";
       ok = ((Detail "get_file") -match 'Skyrim\.esm 0x12eb7 -> 00012EB7' -and (Returned 6).formId -eq "0x00012EB7") }
    @{ name = "get_file Dawnguard"; plugin = (Detail "get_file"); devbench = "GetFormFromFile(10298, Dawnguard.esm) = $((Returned 15).formId)";
       ok = ((Captured (Detail "get_file") 'Dawnguard\.esm 0x283a -> ([0-9A-F]{8})') -eq (((Returned 15).formId) -replace '^0x', '')) }
    @{ name = "get_file Curios";   notRun = $NoCreations; plugin = (Detail "get_file"); devbench = "GetFormFromFile(2048, Curios) = $((Returned 7).formId)";
       ok = ((Captured (Detail "get_file") 'Curios 0x800 -> ([0-9A-F]{8})') -eq (((Returned 7).formId) -replace '^0x', '')) }
    @{ name = "get_file Fish";     notRun = $NoCreations; plugin = (Detail "get_file"); devbench = "GetFormFromFile(2048, Fish) = $((Returned 8).formId)";
       ok = ((Captured (Detail "get_file") 'Fish 0x800 -> ([0-9A-F]{8})') -eq (((Returned 8).formId) -replace '^0x', '')) }
    @{ name = "get_file missing";  plugin = (Detail "get_file"); devbench = "GetFormFromFile(2048, NoSuchFile.esp) = $((Step 9).returnedType)";
       ok = ((Detail "get_file") -match 'NoSuchFile\.esp -> none' -and (Step 9).returnedType -eq "none") }
    @{ name = "editor_id_keyword"; plugin = (Detail "editor_id_keyword"); devbench = "Form.GetFormID(VendorItemWeapon) = $(Returned 10); help: $(Lines 12)";
       ok = ((Detail "editor_id_keyword") -match 'formID=0008F958' -and (Returned 10) -eq 588120 -and (Lines 12) -match 'KYWD: VendorItemWeapon \(0008F958\)') }
    @{ name = "keywords";          plugin = (Detail "keywords"); devbench = "Form.HasKeyword(0x12eb7, VendorItemWeapon) = $(Returned 11)";
       ok = ((Detail "keywords") -match 'has_keyword\(VendorItemWeapon\)=true' -and (Returned 11) -eq $true) }
    @{ name = "editor_id_missing"; plugin = (Detail "editor_id_missing"); devbench = "help IronSword 4: $(Lines 13)";
       ok = ($plugin["editor_id_missing"].ok -and (Lines 13) -notmatch 'WEAP:' -and (Lines 13) -match 'ANIO: AnimObjectIronSword') }
    @{ name = "editor_id_global";  plugin = (Detail "editor_id_global"); devbench = "help GameHour 3: $(Lines 14)";
       ok = (Near $hour $pluginHour 0.1) }
    @{ name = "form_arrays";       notRun = $NoCreations; plugin = (Detail "form_arrays"); devbench = "the plugin files of the smallest install with Creations (1.6.353, whose Update.esm is older) hold KYWD=1287 GLOB=1258 WEAP=3328 NPC_=6462 FLST=1217 and the game makes one GLOB more; a larger load order adds forms";
       ok = ($plugin["form_arrays"].ok -and @(@("KYWD", 1287), @("GLOB", 1259), @("WEAP", 3328), @("NPC_", 6462), @("FLST", 1217) | ForEach-Object { [int](Captured (Detail "form_arrays") "$($_[0])=(\d+)") -ge $_[1] }) -notcontains $false) }
    @{ name = "generated_api";     plugin = (Detail "generated_api"); devbench = "the hand-written skyrim wrappers' results in the same run";
       ok = ($plugin["generated_api"].ok -and (Detail "generated_api") -match '12 comparisons equal') }
)
