# DevBench comparisons for the hooks suite (step indices of hooks-steps.json); dot-sourced by verify.ps1.
# Plugins a and b both subscribe to Actor::ModActorValueImpl through the host's dispatcher: a's Before (ordered before
# b's) takes one more point, b's Before doubles the damage and b's Around halves it back, or skips the original when it
# is 12; a's After observes the outcome; a replaces the outer ActorValueOwner_ModActorValue and b's attempt to is refused,
# as is b's Before that would close an ordering cycle. With health regeneration off, DevBench damages the player by 10
# (reaching 89), by 5 (skipped, still 89), then by 10 again after b removed its subscriptions at the first save (78).
# a also counts TESObjectREFR::GetScale calls in an After. The host checks every typed operation's entry at startup,
# PLUGINS_LOADED_2 and DATA_LOADED and finds no other module's patch.
$Phases = "aLoad", "bLoad", "aSave1", "bSave1", "aSave2", "bSave2"

$installed = @($hostLog | Where-Object { $_ -match '^hooks: Actor::(ModActorValueImpl|ActorValueOwner_ModActorValue) at 0x[0-9a-f]+: detour installed, entry pristine, equal to the executable file$' })
$cycle = @($hostLog | Where-Object { $_ -match '^hooks: refused: Actor::ModActorValueImpl: ordering cycle between skyrim-rs-hooks-a, skyrim-rs-hooks-b$' })
$exclusive = @($hostLog | Where-Object { $_ -match '^hooks: refused: Actor::ActorValueOwner_ModActorValue: skyrim-rs-hooks-b cannot replace it, skyrim-rs-hooks-a already does$' })
$removed = @($hostLog | Where-Object { $_ -match '^hooks: skyrim-rs-hooks-b: 2 subscriptions removed$' })
$loaded = @($hostLog | Where-Object { $_ -match '^loaded skyrim_rs_hooks_[ab]\.dll: skyrim-rs-hooks-[ab]$' })
$checks = @($hostLog | Where-Object { $_ -match '^hooks: (startup|PLUGINS_LOADED_2|DATA_LOADED): 18 sites checked, 0 patched by another module, compatibility mode refuse$' })
$hostHandle = Loaded "skyrim_rs.dll"
$health = @(5, 8, 11, 17 | ForEach-Object { Returned $_ })
$damage = @(6, 9, 15 | ForEach-Object { Lines $_ })

$rows = @(
    @{ name = "plugins loaded"; plugin = "$(Detail 'a_subscribe') | $(Detail 'b_subscribe')"; devbench = "${LoaderLogName}: skyrim_rs.dll handle $hostHandle; (host log) $(@($hostLog -match '^hosted by ') -join ' | ') | $($loaded -join ' | ')";
       ok = ($plugin["a_subscribe"].ok -and $plugin["b_subscribe"].ok -and $loaded.Count -eq 2 -and $hostHandle -ne $null -and (Hosted)) }
    @{ name = "sites checked";  plugin = "$(Detail 'a_scale') | $(Detail 'a_scale_calls')"; devbench = "(host log) $($checks -join ' | ')";
       ok = ($plugin["a_scale"].ok -and $plugin["a_scale_calls"].ok -and $checks.Count -eq 3) }
    @{ name = "entry bytes";    plugin = (Detail "a_pristine"); devbench = "(host log) $($installed -join ' | ')";
       ok = ($plugin["a_pristine"].ok -and $installed.Count -eq 2) }
    @{ name = "before order";   plugin = "$(Detail 'a_before') | $(Detail 'b_before')"; devbench = "health $($health[0]) -> $($health[1]) after '$($damage[0])'";
       ok = ($plugin["a_before"].ok -and $plugin["b_before"].ok -and (Near $health[0] 100 0.01) -and (Near $health[1] 89 0.01)) }
    @{ name = "around and after"; plugin = "$(Detail 'b_around') | $(Detail 'a_after')"; devbench = "health $($health[1]) -> $($health[2]) after '$($damage[1])' (skipped)";
       ok = ($plugin["b_around"].ok -and $plugin["a_after"].ok -and (Near $health[2] 89 0.01)) }
    @{ name = "replace";        plugin = (Detail "a_outer"); devbench = "(host log) $($exclusive -join ' | ')";
       ok = ($plugin["a_outer"].ok -and $plugin["b_exclusive"].ok -and $exclusive.Count -eq 1) }
    @{ name = "cycle";          plugin = (Detail "b_cycle"); devbench = "(host log) $($cycle -join ' | ')";
       ok = ($plugin["b_cycle"].ok -and $cycle.Count -eq 1) }
    @{ name = "diagnostics";    plugin = "$(Detail 'a_diagnostics') | $(Detail 'a_diagnostics_final')"; devbench = "saveGame seen: $(($steps | Where-Object { $_.index -eq 13 }).satisfied), $(($steps | Where-Object { $_.index -eq 19 }).satisfied)";
       ok = ($plugin["a_diagnostics"].ok -and $plugin["a_diagnostics_final"].ok -and ($steps | Where-Object { $_.index -eq 13 }).satisfied -and ($steps | Where-Object { $_.index -eq 19 }).satisfied) }
    @{ name = "removal";        plugin = "$(Detail 'b_remove') | $(Detail 'b_silent') | $(Detail 'a_after_final')"; devbench = "(host log) $($removed -join ' | '); health $($health[2]) -> $($health[3]) after '$($damage[2])'";
       ok = ($plugin["b_remove"].ok -and $plugin["b_silent"].ok -and $plugin["a_before_final"].ok -and $plugin["a_after_final"].ok -and $removed.Count -eq 1 -and (Near $health[3] 78 0.01)) }
)
