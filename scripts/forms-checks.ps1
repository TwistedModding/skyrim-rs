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

$rows = @(
    @{ name = "files";             plugin = ($pluginFiles -join ", "); devbench = ($dbFiles -join ", ");
       ok = ($pluginFiles.Count -gt 0 -and (($pluginFiles -join ",") -eq ($dbFiles -join ","))) }
    @{ name = "lookup_weapon";     plugin = (Detail "lookup_weapon"); devbench = "Game.GetForm(77495) = $((Returned 5).formId) type $((Returned 5).formType)";
       ok = ((Detail "lookup_weapon") -match 'formID=00012EB7 type=41' -and (Returned 5).formId -eq "0x00012EB7" -and (Returned 5).formType -eq "41") }
    @{ name = "get_file Skyrim";   plugin = (Detail "get_file"); devbench = "GetFormFromFile(77495, Skyrim.esm) = $((Returned 6).formId)";
       ok = ((Detail "get_file") -match 'Skyrim\.esm 0x12eb7 -> 00012EB7' -and (Returned 6).formId -eq "0x00012EB7") }
    @{ name = "get_file Curios";   plugin = (Detail "get_file"); devbench = "GetFormFromFile(2048, Curios) = $((Returned 7).formId)";
       ok = ((Detail "get_file") -match 'Curios 0x800 -> FE001800' -and (Returned 7).formId -eq "0xFE001800") }
    @{ name = "get_file Fish";     plugin = (Detail "get_file"); devbench = "GetFormFromFile(2048, Fish) = $((Returned 8).formId)";
       ok = ((Detail "get_file") -match 'Fish 0x800 -> 05000800' -and (Returned 8).formId -eq "0x05000800") }
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
    @{ name = "form_arrays";       plugin = (Detail "form_arrays"); devbench = "forms-verify.md recorded KYWD=1290 GLOB=1261 WEAP=3328 NPC_=6462 FLST=1219";
       ok = ((Detail "form_arrays") -match 'KYWD=1290 GLOB=1261 WEAP=3328 NPC_=6462 FLST=1219') }
)
