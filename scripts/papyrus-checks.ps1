# DevBench comparisons for the papyrus suite (step indices of papyrus-steps.json); dot-sourced by verify.ps1.
# The plugin registers its natives in the SKSE PapyrusInterface callback, checks handles and script types after
# the load, binds a SkyrimRsTestRef object to the player, and checks the calls DevBench made and the typed dispatches
# from inside Ping at the save. DevBench also calls the array natives, the two latent natives and EventCount, which
# reads the events Ping sent to the bound object. The scenario gives the player the Rested ability (a scripted effect)
# before the first Ping, which also relays the event to the suite's alias and effect objects. The equip-counter mod
# runs alongside and answers its own natives.
$Phases = "register", "postLoadGame", "saveGame"

$pluginLines = Get-Content "$OutDir\$PluginLog"
$modLog = "$OutDir\skyrim-rs-equip-counter.log"
$modLines = if (Test-Path $modLog) { Get-Content $modLog } else { @() }
function NativeLine($name, $needle) { ($pluginLines | Where-Object { $_ -match "^native $name \(stack \d+, thread \d+\): .*$([regex]::Escape($needle))" } | Select-Object -First 1) }
$health = Captured (NativeLine "PlayerHealth" "ACHR 00000014") '-> ([0-9.]+)$'

$rows = @(
    @{ name = "ping";          plugin = (NativeLine "Ping" "x=41"); devbench = "SkyrimRsTest.Ping(41) = $(Returned 5) ($((Step 5).returnedType)), Ping(1) = $(Returned 13)";
       ok = ((Returned 5) -eq 42 -and (Step 5).returnedType -eq "int" -and (Returned 13) -eq 2 -and $plugin["calls"].ok) }
    @{ name = "form_id";       plugin = "$(NativeLine 'FormId' '-> 20') | $(NativeLine 'FormId' '-> 77495')"; devbench = "FormId(0x14) = $(Returned 6), FormId(0x12eb7) = $(Returned 7)";
       ok = ((Returned 6) -eq 20 -and (Returned 7) -eq 77495) }
    @{ name = "player_health"; plugin = (NativeLine "PlayerHealth" "ACHR 00000014"); devbench = "PlayerHealth(0x14) = $(Returned 8), Actor.GetActorValue(player, Health) = $(Returned 14)";
       ok = ((Near $health (Returned 8) 0.01) -and (Near (Returned 8) (Returned 14) 0.01)) }
    @{ name = "echo";          plugin = (NativeLine "Echo" "hello skyrim"); devbench = "Echo('hello skyrim') = '$(Returned 9)' ($((Step 9).returnedType))";
       ok = ((Returned 9) -eq "hello skyrim") }
    @{ name = "scale";         plugin = (NativeLine "Scale" "x=2.5"); devbench = "Scale(2.5, true) = $(Returned 10) ($((Step 10).returnedType))";
       ok = (Near (Returned 10) 5.0 0.001) }
    @{ name = "player_form";   plugin = (NativeLine "Player" "ACHR"); devbench = "Player() = $((Returned 11).formId) ($((Step 11).returnedType)), Game.GetForm(20) = $((Returned 15).formId)";
       ok = ((Returned 11).formId -eq "0x00000014" -and (Returned 15).formId -eq "0x00000014") }
    @{ name = "describe";      plugin = (NativeLine "Describe" "swords"); devbench = "Describe(0x14, 3, 'swords', false) = '$(Returned 12)'";
       ok = ((Returned 12) -eq "00000014 x3 swords flag=false") }
    @{ name = "dispatch";      plugin = (Detail "dispatch"); devbench = "two Ping calls answered: $((Step 5).called), $((Step 13).called)";
       ok = ($plugin["dispatch"].ok -and (Step 5).called -eq $true -and (Step 13).called -eq $true) }
    @{ name = "equip counter"; plugin = ($modLines -match '^(registered|listening|equip )') -join ' | '; devbench = "GetEquipCount before = $(Returned 16), EquipItem(torch) called=$((Step 17).called), UnequipItem called=$((Step 19).called), GetEquipCount after = $(Returned 21), GetLastEquipped = $((Returned 22).formId)";
       ok = ((Returned 16) -eq 0 -and (Returned 21) -eq 1 -and (Returned 22).formId -eq "0x0001D4EC" -and ($modLines -match '^equip 1: 0001D4EC$').Count -eq 1) }
    @{ name = "bound method";  plugin = "$(Detail "bind_object") | $(NativeLine 'SelfFormId' '-> 20')"; devbench = "SkyrimRsTestRef.SelfFormId() on the player = $(Returned 23) ($((Step 23).returnedType))";
       ok = ($plugin["bind_object"].ok -and (Returned 23) -eq 20) }
    @{ name = "arrays";        plugin = (Detail "arrays"); devbench = "Halves([3, 8, -5]) = $((Returned 24) -join ', ') ($((Step 24).returnedType)), Forms() = $((@(Returned 25) | ForEach-Object { if ($_.none) { 'None' } else { $_.formId } }) -join ', ') ($((Step 25).returnedType))";
       ok = ($plugin["arrays"].ok -and @(Returned 24).Count -eq 3 -and (Near (Returned 24)[0] 1.5 0.001) -and (Near (Returned 24)[1] 4.0 0.001) -and (Near (Returned 24)[2] -2.5 0.001) -and @(Returned 25).Count -eq 3 -and (Returned 25)[0].formId -eq "0x00000014" -and (Returned 25)[1].none -eq $true -and (Returned 25)[2].formId -eq "0x00012EB7") }
    @{ name = "latent";        plugin = (Detail "latent"); devbench = "DoubleNow(21) = $(Returned 26) ($((Step 26).returnedType)), DoubleLater(5) = $(Returned 27) ($((Step 27).returnedType))";
       ok = ($plugin["latent"].ok -and (Returned 26) -eq 42 -and (Returned 27) -eq 10) }
    @{ name = "events";        plugin = (Detail "events"); devbench = "EventCount() after the two Pings = $(Returned 28)";
       ok = ($plugin["events"].ok -and (Returned 28) -eq 2) }
    @{ name = "relay";         plugin = (Detail "relay"); devbench = "player.addspell 000FB981 (Rested): $((Step 3) | ConvertTo-Json -Compress)";
       ok = ($plugin["relay"].ok -and (Step 3) -ne $null) }
    @{ name = "save trigger";  plugin = ($summaries -match 'papyrus saveGame') -join ' '; devbench = "console save: $((Step 30) | ConvertTo-Json -Compress), waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 31 }).satisfied)";
       ok = ((($steps | Where-Object { $_.index -eq 31 }).satisfied -eq $true)) }
)
