# Runs the skyrim-rs test plugin in the 1.7.104 game through DevBench (scripts/forms-steps.json), then compares
# the plugin's PASS lines with DevBench's own answers from the same session. See docs/subsystems/forms.md.
param(
    [string]$OutDir = "verify-forms-out",
    [string]$GameDir = "C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition"
)
$ErrorActionPreference = "Stop"
$SkseLogs = "$env:USERPROFILE\Documents\My Games\Skyrim Special Edition\SKSE"
$Plugins = "$GameDir\Data\SKSE\Plugins"
$Root = Split-Path $PSScriptRoot -Parent
$Dll = "$Root\target\release\skyrim_rs_test.dll"
$PluginLog = "skyrim-rs-test.log"
$Logs = "skse64.log", "skse64_loader.log", "skse64_steam_loader.log", "devbench.log", "retrace.log", $PluginLog

if (Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue) { throw "SkyrimSE is already running" }
Push-Location $Root
try { cargo build --release -p testplugin; if ($LASTEXITCODE -ne 0) { throw "cargo build failed" } } finally { Pop-Location }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Backup = Join-Path $OutDir "backup"
New-Item -ItemType Directory -Force $Backup | Out-Null
foreach ($f in $Logs) { if (Test-Path "$SkseLogs\$f") { Copy-Item "$SkseLogs\$f" "$Backup\$f" -Force } }
Remove-Item "$SkseLogs\$PluginLog" -ErrorAction SilentlyContinue

Copy-Item $Dll "$Plugins\skyrim_rs_test.dll" -Force
$retraceOff = Test-Path "$Plugins\retrace.dll"
if ($retraceOff) { Rename-Item "$Plugins\retrace.dll" "retrace.dll.off" }
try {
    & "$PSScriptRoot\devbench.ps1" -Steps "$PSScriptRoot\forms-steps.json" -OutDir $OutDir
    $scenarioOk = $LASTEXITCODE -eq 0
} finally {
    Remove-Item "$Plugins\skyrim_rs_test.dll" -ErrorAction SilentlyContinue
    if ($retraceOff) { Rename-Item "$Plugins\retrace.dll.off" "retrace.dll" }
    if (Test-Path "$SkseLogs\$PluginLog") { Copy-Item "$SkseLogs\$PluginLog" "$OutDir\$PluginLog" -Force }
    foreach ($f in $Logs) {
        if (Test-Path "$Backup\$f") { Copy-Item "$Backup\$f" "$SkseLogs\$f" -Force }
        elseif ($f -eq $PluginLog) { Remove-Item "$SkseLogs\$f" -ErrorAction SilentlyContinue }
    }
}

# The plugin's last PASS/FAIL per test (the postLoadGame phase runs after the dataLoaded one).
$plugin = @{}
$summaries = @()
foreach ($line in Get-Content "$OutDir\$PluginLog") {
    if ($line -match '^(PASS|FAIL) (\w+): (.*)$') { $plugin[$Matches[2]] = @{ ok = ($Matches[1] -eq "PASS"); detail = $Matches[3] } }
    elseif ($line -match '^summary ') { $summaries += $line }
}
$steps = ((Get-Content "$OutDir\result.json" -Raw | ConvertFrom-Json).result.results)
function Step($i) { $s = $steps | Where-Object { $_.index -eq $i }; if ($s -and $s.ok) { $s.result } else { $null } }
function Returned($i) { $r = Step $i; if ($r) { $r.returned } else { $null } }
function Lines($i) { $r = Step $i; if ($r) { $r.lines -join " | " } else { "" } }
function Detail($name) { if ($plugin[$name]) { $plugin[$name].detail } else { "" } }

$mods = Step 4
$dbFiles = @()
if ($mods) {
    $dbFiles += $mods.plugins | ForEach-Object { "compiled $($_.index):$($_.name)" }
    $dbFiles += $mods.lightPlugins | ForEach-Object { "light $($_.index):$($_.name)" }
}
$pluginFiles = @()
$filesDetail = Detail "files"
if ($filesDetail -match 'compiled=\[([^\]]*)\] light=\[([^\]]*)\]') {
    $pluginFiles += ($Matches[1] -split ',') | ForEach-Object { if ($_ -match '^(\d+):([^:]+):') { "compiled $($Matches[1]):$($Matches[2])" } }
    $pluginFiles += ($Matches[2] -split ',') | ForEach-Object { if ($_ -match '^(\d+):([^:]+):') { "light $($Matches[1]):$($Matches[2])" } }
}
$hour = $null
if ((Lines 14) -match 'GameHour = ([0-9.]+)') { $hour = [double]$Matches[1] }
$pluginHour = $null
if ((Detail "editor_id_global") -match 'GameHour=([0-9.]+)') { $pluginHour = [double]$Matches[1] }

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
       ok = ($hour -ne $null -and $pluginHour -ne $null -and [math]::Abs($hour - $pluginHour) -lt 0.1) }
    @{ name = "form_arrays";       plugin = (Detail "form_arrays"); devbench = "forms-verify.md recorded KYWD=1290 GLOB=1261 WEAP=3328 NPC_=6462 FLST=1219";
       ok = ((Detail "form_arrays") -match 'KYWD=1290 GLOB=1261 WEAP=3328 NPC_=6462 FLST=1219') }
)

""
"plugin summary: $($summaries -join '; ')"
"scenario ok: $scenarioOk"
""
$allOk = $scenarioOk -and ($summaries -match '^summary postLoadGame: \d+ pass, 0 fail').Count -eq 1
foreach ($r in $rows) {
    $verdict = if ($r.ok) { "OK  " } else { "DIFF"; $allOk = $false }
    "$verdict $($r.name)"
    "     plugin:   $($r.plugin)"
    "     devbench: $($r.devbench)"
}
foreach ($name in ($plugin.Keys | Sort-Object)) { if (-not $plugin[$name].ok) { "FAIL $name`: $($plugin[$name].detail)"; $allOk = $false } }
""
"logs in $OutDir"
if (-not $allOk) { exit 1 }
