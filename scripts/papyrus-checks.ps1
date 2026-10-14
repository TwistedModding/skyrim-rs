# DevBench comparisons for the papyrus suite (step indices of papyrus-steps.json); dot-sourced by verify.ps1.
# The plugin registers its natives in the SKSE PapyrusInterface callback, checks handles and script types after
# the load, binds a SkyrimRsTestRef object to the player, and checks the calls DevBench made and the typed dispatches
# from inside Ping at the save. The equip-counter mod runs alongside and answers its own natives.
$Phases = "register", "postLoadGame", "saveGame"

$pluginLines = Get-Content "$OutDir\$PluginLog"
$modLog = "$OutDir\skyrim-rs-equip-counter.log"
$modLines = if (Test-Path $modLog) { Get-Content $modLog } else { @() }
function NativeLine($name, $needle) { ($pluginLines | Where-Object { $_ -match "^native $name \(stack \d+\): .*$([regex]::Escape($needle))" } | Select-Object -First 1) }
$health = Captured (NativeLine "PlayerHealth" "ACHR 00000014") '-> ([0-9.]+)$'

$rows = @(
    @{ name = "ping";          plugin = (NativeLine "Ping" "x=41"); devbench = "SkyrimRsTest.Ping(41) = $(Returned 4) ($((Step 4).returnedType)), Ping(1) = $(Returned 12)";
       ok = ((Returned 4) -eq 42 -and (Step 4).returnedType -eq "int" -and (Returned 12) -eq 2 -and $plugin["calls"].ok) }
    @{ name = "form_id";       plugin = "$(NativeLine 'FormId' '-> 20') | $(NativeLine 'FormId' '-> 77495')"; devbench = "FormId(0x14) = $(Returned 5), FormId(0x12eb7) = $(Returned 6)";
       ok = ((Returned 5) -eq 20 -and (Returned 6) -eq 77495) }
    @{ name = "player_health"; plugin = (NativeLine "PlayerHealth" "ACHR 00000014"); devbench = "PlayerHealth(0x14) = $(Returned 7), Actor.GetActorValue(player, Health) = $(Returned 13)";
       ok = ((Near $health (Returned 7) 0.01) -and (Near (Returned 7) (Returned 13) 0.01)) }
    @{ name = "echo";          plugin = (NativeLine "Echo" "hello skyrim"); devbench = "Echo('hello skyrim') = '$(Returned 8)' ($((Step 8).returnedType))";
       ok = ((Returned 8) -eq "hello skyrim") }
    @{ name = "scale";         plugin = (NativeLine "Scale" "x=2.5"); devbench = "Scale(2.5, true) = $(Returned 9) ($((Step 9).returnedType))";
       ok = (Near (Returned 9) 5.0 0.001) }
    @{ name = "player_form";   plugin = (NativeLine "Player" "ACHR"); devbench = "Player() = $((Returned 10).formId) ($((Step 10).returnedType)), Game.GetForm(20) = $((Returned 14).formId)";
       ok = ((Returned 10).formId -eq "0x00000014" -and (Returned 14).formId -eq "0x00000014") }
    @{ name = "describe";      plugin = (NativeLine "Describe" "swords"); devbench = "Describe(0x14, 3, 'swords', false) = '$(Returned 11)'";
       ok = ((Returned 11) -eq "00000014 x3 swords flag=false") }
    @{ name = "dispatch";      plugin = (Detail "dispatch"); devbench = "two Ping calls answered: $((Step 4).called), $((Step 12).called)";
       ok = ($plugin["dispatch"].ok -and (Step 4).called -eq $true -and (Step 12).called -eq $true) }
    @{ name = "equip counter"; plugin = ($modLines -match '^(registered|listening|equip )') -join ' | '; devbench = "GetEquipCount before = $(Returned 15), EquipItem(torch) called=$((Step 16).called), UnequipItem called=$((Step 18).called), GetEquipCount after = $(Returned 20), GetLastEquipped = $((Returned 21).formId)";
       ok = ((Returned 15) -eq 0 -and (Returned 20) -eq 1 -and (Returned 21).formId -eq "0x0001D4EC" -and ($modLines -match '^equip 1: 0001D4EC$').Count -eq 1) }
    @{ name = "bound object";  plugin = (Detail "bind_object"); devbench = "SkyrimRsTestRef.SelfFormId() on the player called = $((Step 22).called), returned $(Returned 22) ($((Step 22).returnedType)); the native method runs once request 4d is answered";
       ok = ($plugin["bind_object"].ok -and (Step 22).called -eq $true) }
    @{ name = "save trigger";  plugin = ($summaries -match 'papyrus saveGame') -join ' '; devbench = "console save: $((Step 24) | ConvertTo-Json -Compress), waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 25 }).satisfied)";
       ok = ((($steps | Where-Object { $_.index -eq 25 }).satisfied -eq $true)) }
)
