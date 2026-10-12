# DevBench comparisons for the papyrus suite (step indices of papyrus-steps.json); dot-sourced by verify.ps1.
# The plugin registers its natives in the SKSE PapyrusInterface callback, checks handles and script types after
# the load and the calls DevBench made at the save.
$Phases = "register", "postLoadGame", "saveGame"

$pluginLines = Get-Content "$OutDir\$PluginLog"
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
    @{ name = "player_form";   plugin = (NativeLine "Player" "ACHR"); devbench = "Player() = $((Returned 10).formId) $((Returned 10).name) ($((Step 10).returnedType)), Game.GetForm(20) = $((Returned 14).formId)";
       ok = ((Returned 10).formId -eq "0x00000014" -and (Returned 14).formId -eq "0x00000014") }
    @{ name = "describe";      plugin = (NativeLine "Describe" "swords"); devbench = "Describe(0x14, 3, 'swords', false) = '$(Returned 11)'";
       ok = ((Returned 11) -eq "00000014 x3 swords flag=false") }
    @{ name = "dispatch";      plugin = (Detail "dispatch"); devbench = "two Ping calls answered: $((Step 4).called), $((Step 12).called)";
       ok = ($plugin["dispatch"].ok -and (Step 4).called -eq $true -and (Step 12).called -eq $true) }
    @{ name = "save trigger";  plugin = ($summaries -match 'papyrus saveGame') -join ' '; devbench = "console save: $((Step 16) | ConvertTo-Json -Compress), waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 17 }).satisfied)";
       ok = ((($steps | Where-Object { $_.index -eq 17 }).satisfied -eq $true)) }
)
