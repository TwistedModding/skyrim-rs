# DevBench comparisons for the reload suite (step indices of reload-steps.json, four parts merged: 22 steps, then 17
# per reload); dot-sourced by verify.ps1. The host runs the tag 1 build; between the parts the next tag's DLL replaces
# the installed file and the host swaps it in at a frame boundary. In every part DevBench calls Version() (the tag),
# damages the player's health by 10 (the generation's Before sees it), runs SkyrimRsReload with capture (the tag and the
# restored reload counter), equips and unequips the torch (the sink), opens and closes the overlay and saves; each
# generation reports at its save.
$Phases = "tag1", "tag2", "tag3", "tag4"
$Tags = 1, 2, 3, 4

$hostLog = if (Test-Path "$OutDir\skyrim-rs.log") { Get-Content "$OutDir\skyrim-rs.log" } else { @() }
$loaded = @($hostLog | Where-Object { $_ -match '^loaded skyrim_rs_reload_test\.dll: skyrim-rs-reload-test$' })
$changed = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: skyrim_rs_reload_test\.dll changed, reloading as generation [234]$' })
$saved = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: generation [123]: 4 bytes of state saved$' })
$detached = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: generation [123]: 1 hook subscriptions, 1 sinks, 1 natives stubbed, 1 commands stubbed, 1 windows, 1 message handlers, 1 Papyrus callbacks detached$' })
$unloaded = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: generation [123] unloaded$' })
$reloaded = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: generation [234] loaded$' })
$restored = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: generation [234]: 4 bytes of state restored$' })
$nativeRepointed = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: SkyrimRsReloadTest\.Version re-pointed$' })
$commandRepointed = @($hostLog | Where-Object { $_ -match '^skyrim-rs-reload-test: command SkyrimRsReload re-pointed$' })
$hookRemoved = @($hostLog | Where-Object { $_ -match '^hooks: skyrim-rs-reload-test: 1 subscriptions removed$' })
$hookSubscribed = @($hostLog | Where-Object { $_ -match '^hooks: Actor::ModActorValueImpl: skyrim-rs-reload-test Before subscribed as #\d+$' })

# Step indices per part, from where each part's Version() call sits (part 1 starts with the load): Version, health
# before, damage, health after, the command, the two F11 taps, the saveGame wait.
$Parts = @(5, 22, 39, 56 | ForEach-Object { @{ version = $_; before = $_ + 1; damage = $_ + 2; after = $_ + 4; command = $_ + 5; f11 = ($_ + 10), ($_ + 12); save = $_ + 15 } })
function PhaseOk($name) { @($Phases | ForEach-Object { $r = $runs["reload $_"]; $r -and $r[$name] -and $r[$name].ok }) -notcontains $false }
function PhaseDetails($name) { ($Phases | ForEach-Object { RunDetail "reload $_" $name }) -join " | " }
$versions = @($Parts | ForEach-Object { Returned $_.version })
$health = @($Parts | ForEach-Object { @((Returned $_.before), (Returned $_.after)) })
$commandLines = @($Parts | ForEach-Object { Lines $_.command })
$f11 = @($Parts | ForEach-Object { (Step $_.f11[0]) -ne $null -and (Step $_.f11[1]) -ne $null })
$saves = @($Parts | ForEach-Object { $i = $_.save; ($steps | Where-Object { $_.index -eq $i }).satisfied })
$healthOk = (0..3 | ForEach-Object { (Near $health[2 * $_] (100 - 10 * $_) 0.01) -and (Near $health[2 * $_ + 1] (90 - 10 * $_) 0.01) }) -notcontains $false
$commandOk = (0..3 | ForEach-Object { $commandLines[$_] -match "SkyrimRsReload: tag $($Tags[$_]), reloads $($Tags[$_] - 1)" }) -notcontains $false

$rows = @(
    @{ name = "host loads";    plugin = (RunDetail "reload tag1" "generation"); devbench = "(host log) $($loaded -join ' | ')";
       ok = ($runs["reload tag1"]["generation"].ok -and $loaded.Count -eq 1) }
    @{ name = "reloads";       plugin = (PhaseDetails "generation"); devbench = "(host log) $($changed -join ' | '); $($unloaded -join ' | '); $($reloaded -join ' | ')";
       ok = ((PhaseOk "generation") -and $changed.Count -eq 3 -and $unloaded.Count -eq 3 -and $reloaded.Count -eq 3) }
    @{ name = "state handoff"; plugin = (PhaseDetails "generation"); devbench = "(host log) $($saved -join ' | '); $($restored -join ' | ')";
       ok = ((PhaseOk "generation") -and $saved.Count -eq 3 -and $restored.Count -eq 3) }
    @{ name = "detach";        plugin = (PhaseDetails "hook"); devbench = "(host log) $($detached -join ' | '); $($hookRemoved -join ' | ')";
       ok = ($detached.Count -eq 3 -and $hookRemoved.Count -eq 3) }
    @{ name = "native";        plugin = (PhaseDetails "native"); devbench = "Version() = $($versions -join ', '); (host log) re-pointed $($nativeRepointed.Count) times";
       ok = ((PhaseOk "native") -and (Compare-Object $versions $Tags -SyncWindow 0).Count -eq 0 -and $nativeRepointed.Count -eq 3) }
    @{ name = "hook";          plugin = (PhaseDetails "hook"); devbench = "health $($health -join ', ') around the four damages; (host log) subscribed $($hookSubscribed.Count) times";
       ok = ((PhaseOk "hook") -and $healthOk -and $hookSubscribed.Count -eq 4) }
    @{ name = "sink";          plugin = (PhaseDetails "sink"); devbench = "EquipItem and UnequipItem called in every part";
       ok = (PhaseOk "sink") }
    @{ name = "command";       plugin = (PhaseDetails "command"); devbench = "$($commandLines -join ' | '); (host log) re-pointed $($commandRepointed.Count) times";
       ok = ((PhaseOk "command") -and $commandOk -and $commandRepointed.Count -eq 3) }
    @{ name = "window";        plugin = (PhaseDetails "window"); devbench = "F11 taps ok per part: $($f11 -join ', ')";
       ok = ((PhaseOk "window") -and $f11 -notcontains $false) }
    @{ name = "messages";      plugin = (PhaseDetails "messages"); devbench = "waitFor saveGame satisfied per part: $($saves -join ', ')";
       ok = ((PhaseOk "messages") -and $saves -notcontains $false -and $saves.Count -eq 4) }
)
