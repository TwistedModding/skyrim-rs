# DevBench comparisons for the actors suite (step indices of actors-steps.json); dot-sourced by verify.ps1.
# The plugin runs the suite after the load (standing) and again at the save (sneaking, weapon drawn).
$Phases = "postLoadGame", "saveGame"

$av = Detail "actor_values"
$inv = Detail "inventory"
$pos = Detail "position"
$cell = Detail "parent_cell"
$npc = Detail "npc"
$queries = Detail "actor_queries"
$stateBefore = RunDetail "actors postLoadGame" "state"
$stateAfter = RunDetail "actors saveGame" "state"
$inspect = Step 45

$rows = @(
    @{ name = "actor_values";    plugin = $av; devbench = "GetActorValue Health/Magicka/Stamina/CarryWeight/SpeedMult = $(Returned 4)/$(Returned 5)/$(Returned 6)/$(Returned 7)/$(Returned 8), GetBaseActorValue(Health) = $(Returned 9)";
       ok = ((Near (Captured $av 'Health=([0-9.]+)') (Returned 4) 0.01) -and (Near (Captured $av 'Magicka=([0-9.]+)') (Returned 5) 0.01) -and
             (Near (Captured $av 'Stamina=([0-9.]+)') (Returned 6) 0.01) -and (Near (Captured $av 'CarryWeight=([0-9.]+)') (Returned 7) 0.01) -and
             (Near (Captured $av 'SpeedMult=([0-9.]+)') (Returned 8) 0.01) -and (Near (Captured $av 'base\(Health\)=([0-9.]+)') (Returned 9) 0.01)) }
    @{ name = "inventory";       plugin = $inv; devbench = "GetItemCount gold/IronSword/torch = $(Returned 10)/$(Returned 11)/$(Returned 12)";
       ok = ((Captured $inv 'gold=(\d+)') -eq (Returned 10) -and (Captured $inv 'IronSword=(\d+)') -eq (Returned 11) -and (Captured $inv 'torch=(\d+)') -eq (Returned 12)) }
    @{ name = "position";        plugin = $pos; devbench = "GetPositionX/Y/Z = $(Returned 13)/$(Returned 14)/$(Returned 15), GetAngleZ = $(Returned 16)";
       ok = ((Near (Captured $pos 'pos=\(([-0-9.]+),') (Returned 13) 0.5) -and (Near (Captured $pos 'pos=\([-0-9.]+, ([-0-9.]+),') (Returned 14) 0.5) -and
             (Near (Captured $pos 'pos=\([-0-9.]+, [-0-9.]+, ([-0-9.]+)\)') (Returned 15) 0.5) -and (Near (Captured $pos 'headingDeg=([-0-9.]+)') (Returned 16) 0.5)) }
    @{ name = "parent_cell";     plugin = $cell; devbench = "GetParentCell = $((Returned 17).formId) $((Returned 17).editorId), GetWorldSpace = $((Returned 18).formId), GetCurrentLocation = $((Returned 19).formId) $((Returned 19).name)";
       ok = ($cell -match 'parentCell=CELL 00009732' -and (Returned 17).formId -eq "0x00009732" -and $cell -match 'worldspace=WRLD 0000003C' -and (Returned 18).formId -eq "0x0000003C" -and
             $cell -match 'location=LCTN 00013163' -and (Returned 19).formId -eq "0x00013163") }
    @{ name = "scale";           plugin = (Detail "scale"); devbench = "GetScale = $(Returned 20)";
       ok = (Near (Captured (Detail "scale") 'GetScale=([0-9.]+)') (Returned 20) 0.001) }
    @{ name = "base_object";     plugin = (Detail "base_object"); devbench = "GetBaseObject = $((Returned 21).formId) $((Returned 21).name) type $((Returned 21).formType)";
       ok = ((Detail "base_object") -match 'baseObject = NPC_ 00000007' -and (Returned 21).formId -eq "0x00000007" -and (Returned 21).formType -eq "43") }
    @{ name = "node_3d";         plugin = (Detail "node_3d"); devbench = "Is3DLoaded = $(Returned 22)";
       ok = ($plugin["node_3d"].ok -and (Returned 22) -eq $true) }
    @{ name = "actor_queries";   plugin = $queries; devbench = "GetLevel = $(Returned 23), IsDead = $(Returned 27), IsInCombat = $(Returned 28), IsPlayerTeammate = $(Returned 29)";
       ok = ($queries -match 'level=1' -and (Returned 23) -eq 1 -and (Returned 27) -eq $false -and (Returned 28) -eq $false -and (Returned 29) -eq $false) }
    @{ name = "race";            plugin = (Detail "race"); devbench = "Actor.GetRace = $((Returned 24).formId) $((Returned 24).editorId), ActorBase.GetRace = $((Returned 33).formId)";
       ok = ((Detail "race") -match 'race=RACE 00013746' -and (Returned 24).formId -eq "0x00013746" -and (Returned 33).formId -eq "0x00013746") }
    @{ name = "process";         plugin = (Detail "process"); devbench = "GetEquippedWeapon(false) = $((Returned 25).formId) $((Returned 25).name), (true) = $((Step 26).returnedType)";
       ok = ((Detail "process") -match 'right=WEAP 00013790' -and (Returned 25).formId -eq "0x00013790" -and (Step 26).returnedType -eq "none") }
    @{ name = "state before";    plugin = $stateBefore; devbench = "IsSneaking = $(Returned 30), IsWeaponDrawn = $(Returned 31)";
       ok = ($stateBefore -match 'sneaking=false .*drawn=false' -and (Returned 30) -eq $false -and (Returned 31) -eq $false) }
    @{ name = "state after";     plugin = $stateAfter; devbench = "after StartSneaking + DrawWeapon: IsSneaking = $(Returned 43), IsWeaponDrawn = $(Returned 44)";
       ok = ($stateAfter -match 'sneaking=true .*drawn=true' -and (Returned 43) -eq $true -and (Returned 44) -eq $true) }
    @{ name = "npc";             plugin = $npc; devbench = "GetSex = $(Returned 32), GetClass = $((Returned 34).formId) $((Returned 34).name), IsEssential = $(Returned 35), IsUnique = $(Returned 36), GetBaseObject.name = $((Returned 21).name)";
       ok = ($npc -match 'fullName="Prisoner" sex=Male class=CLAS 0002F202' -and (Returned 32) -eq 0 -and (Returned 34).formId -eq "0x0002F202" -and (Returned 35) -eq $false -and (Returned 36) -eq $true -and (Returned 21).name -eq "Prisoner") }
    @{ name = "ref_has_keyword"; plugin = (Detail "ref_has_keyword"); devbench = "Form.HasKeyword(0x14, ActorTypeNPC) = $(Returned 37), (VendorItemWeapon) = $(Returned 38)";
       ok = ((Captured (Detail "ref_has_keyword") 'ActorTypeNPC\)=(\w+)') -eq "$(Returned 37)".ToLower() -and (Returned 38) -eq $false) }
    @{ name = "is_disabled";     plugin = (Detail "is_disabled"); devbench = "IsDisabled = $(Returned 39)";
       ok = ((Detail "is_disabled") -match 'is_disabled=false' -and (Returned 39) -eq $false) }
    @{ name = "inspect player";  plugin = "$av; $(Detail 'process'); $npc"; devbench = "health $($inspect.actorValues.health.current)/$($inspect.actorValues.health.max), carryWeight $($inspect.actorValues.carryWeight.current), right $($inspect.equipped.right.formId), left $($inspect.equipped.left), name $($inspect.name), level $($inspect.level)";
       ok = ($inspect -and $inspect.actorValues.health.current -eq 100 -and $inspect.actorValues.carryWeight.current -eq 300 -and $inspect.equipped.right.formId -eq "0x00013790" -and $inspect.equipped.left -eq $null -and $inspect.name -eq "Prisoner" -and $inspect.level -eq 1) }
    @{ name = "save trigger";    plugin = ($summaries -match 'actors saveGame') -join ' '; devbench = "console save: $((Step 46) | ConvertTo-Json -Compress), waitFor saveGame satisfied: $(($steps | Where-Object { $_.index -eq 47 }).satisfied)";
       ok = ((($steps | Where-Object { $_.index -eq 47 }).satisfied -eq $true)) }
)
