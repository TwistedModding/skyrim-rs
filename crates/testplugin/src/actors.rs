//! The actors suite on the player of the `retrace-base` save, through the generated `sre` methods
//! and the `skyrim` prelude; the expected values are those of the RE repo's
//! `docs/subsystems/actors-verify.md`. `STATE_CHANGED` tells the state test whether `DevBench` has
//! already made the player sneak and draw the weapon.

use std::sync::atomic::{AtomicBool, Ordering};

use skyrim::actors::{Modifier, RefHandle, lookup_actor_value};
use skyrim::forms::form_by_editor_id;
use skyrim::prelude::*;
use sre::raw::vtables;
use sre::{ActorState_flags1, ActorValue, ActorValueList, Actor_boolFlags, BGSKeyword, BGSLocation, Character, FormType, PlayerCharacter, TESActorBaseData_flags, TESClass, TESForm, TESNPC, TESObjectCELL, TESObjectREFR, TESRace, TESWorldSpace};

use crate::{IRON_SWORD, PLAYER, Test, check, describe, form};

pub static STATE_CHANGED: AtomicBool = AtomicBool::new(false);

const PLAYER_NPC: u32 = 0x7;
const GOLD: u32 = 0xF;
const TORCH: u32 = 0x1D4EC;
const IRON_WAR_AXE: u32 = 0x13790;
const RIVERWOOD_CELL: u32 = 0x9732;
const TAMRIEL: u32 = 0x3C;
const RIVERWOOD_LOCATION: u32 = 0x13163;
const NORD_RACE: u32 = 0x13746;
const PLAYER_CLASS: u32 = 0x2F202;
const EXTRA_CONTAINER_CHANGES: u32 = 0x15;
const EXTRA_ALIAS_INSTANCE_ARRAY: u32 = 0x88;

pub const TESTS: &[(&str, Test)] = &[
    ("player", player),
    ("base_object", base_object),
    ("position", position),
    ("parent_cell", parent_cell),
    ("node_3d", node_3d),
    ("scale", scale),
    ("ref_has_keyword", ref_has_keyword),
    ("is_disabled", is_disabled),
    ("handle", handle),
    ("inventory", inventory),
    ("extra_list", extra_list),
    ("process", process),
    ("actor_values", actor_values),
    ("actor_value_write", actor_value_write),
    ("state", state),
    ("actor_queries", actor_queries),
    ("race", race),
    ("npc", npc),
    ("container", container),
    ("actor_value_names", actor_value_names),
    ("actor_downcast", actor_downcast),
];

fn player_ref() -> Result<&'static PlayerCharacter, String> {
    sre::PlayerCharacter::singleton().ok_or_else(|| "g_PlayerCharacter_ptr is null".into())
}

fn base_npc(player: &PlayerCharacter) -> Result<&TESNPC, String> {
    player.base_object().and_then(TESForm::cast::<TESNPC>).ok_or_else(|| "the base object is not an NPC_".into())
}

fn class_name(vtable: Option<&'static sre::raw::Vtable>) -> &'static str {
    vtable.map_or("unknown", |v| v.class)
}

fn address<T>(object: &T) -> usize {
    core::ptr::from_ref(object) as usize
}

fn player() -> Result<String, String> {
    let player = player_ref()?;
    check(player.formID == PLAYER && player.formType == FormType::ACHR as u8, "the player is not ACHR 0x14")?;
    let form = form(PLAYER)?;
    let refr = form.cast::<TESObjectREFR>().ok_or("cast to TESObjectREFR failed")?;
    let actor = refr.cast::<sre::Actor>().ok_or("cast to Actor failed")?;
    check(address(actor) == address(player), "FindFormByID(0x14) and g_PlayerCharacter_ptr differ")?;
    check(form.cast::<PlayerCharacter>().is_some() && form.cast::<Character>().is_some(), "the player does not cast to PlayerCharacter and Character")?;
    let class = skyrim::class_of(player).ok_or("the player's vtable is not in the ledger")?;
    check(class.id == sre::id!(vtables::PlayerCharacter_vtable::ID), &format!("the player's class is {}", class.class))?;
    let count = player.refCount & 0x3FF;
    check(count >= 1, "refCount is 0")?;
    Ok(format!("{} at {:#x}, class {}, refCount {count}", describe(player), address(player), class.class))
}

fn base_object() -> Result<String, String> {
    let player = player_ref()?;
    let base = player.base_object().ok_or("baseObject is null")?;
    check(base.formID == PLAYER_NPC && base.formType == FormType::NPC_ as u8, &format!("baseObject is {}", describe(base)))?;
    let npc = base_npc(player)?;
    check(address(npc) == address(base), "the NPC cast moved the pointer")?;
    check(skyrim::class_of(npc).is_some_and(|c| c.id == sre::id!(vtables::TESNPC_vtable::ID)), "the base's vtable is not TESNPC's")?;
    Ok(format!("baseObject = {} ({})", describe(base), class_name(skyrim::class_of(base))))
}

fn position() -> Result<String, String> {
    let player = player_ref()?;
    let [x, y, z] = [player.posX, player.posY, player.posZ];
    let [rx, ry, rz] = [player.rotX, player.rotY, player.rotZ];
    check(x.is_finite() && y.is_finite() && z.is_finite(), "position is not finite")?;
    Ok(format!("pos=({x:.4}, {y:.4}, {z:.4}) rot=({rx:.4}, {ry:.4}, {rz:.4}) headingDeg={:.4}", rz.to_degrees()))
}

fn parent_cell() -> Result<String, String> {
    let player = player_ref()?;
    let cell: &TESObjectCELL = player.parent_cell().ok_or("parentCell is null")?;
    check(cell.formID == RIVERWOOD_CELL && cell.formType == FormType::CELL as u8, &format!("parentCell is {}", describe(cell)))?;
    let world: &TESWorldSpace = player.get_worldspace().ok_or("GetWorldspace returned null")?;
    check(world.formID == TAMRIEL && world.formType == FormType::WRLD as u8, &format!("worldspace is {}", describe(world)))?;
    let location: &BGSLocation = player.get_current_location().ok_or("GetCurrentLocation returned null")?;
    check(location.formID == RIVERWOOD_LOCATION && location.formType == FormType::LCTN as u8, &format!("location is {}", describe(location)))?;
    check(core::ptr::eq(player.currentLocation(), location), "PlayerCharacter.currentLocation differs from GetCurrentLocation")?;
    Ok(format!("parentCell={} worldspace={} location={} = PlayerCharacter.currentLocation", describe(cell), describe(world), describe(location)))
}

fn node_3d() -> Result<String, String> {
    let player = player_ref()?;
    let node = player.get3_d().ok_or("Get3D returned null")?;
    let loaded = player.loaded_data().and_then(|d| d.node3_d()).ok_or("loadedData->node3D is null")?;
    let class = skyrim::class_of(node).ok_or("the node's vtable is not in the ledger")?;
    let loaded_class = skyrim::class_of(loaded).ok_or("the loaded node's vtable is not in the ledger")?;
    check(class.id == sre::id!(vtables::BSFadeNode_vtable::ID) && loaded_class.id == sre::id!(vtables::BSFadeNode_vtable::ID), &format!("the root nodes are {} and {}", class.class, loaded_class.class))?;
    let same = core::ptr::eq(node, loaded);
    Ok(format!("Get3D at {:#x} ({}), loadedData->node3D at {:#x} ({}), same={same}", address(node), class.class, address(loaded), loaded_class.class))
}

fn scale() -> Result<String, String> {
    let player = player_ref()?;
    let scale = player.get_scale();
    let npc = base_npc(player)?;
    let ref_scale = player.refScale();
    check(ref_scale == 100, &format!("refScale is {ref_scale}"))?;
    check((scale - 1.03).abs() < 0.001, &format!("GetScale is {scale}"))?;
    check((npc.height - 1.0).abs() < f32::EPSILON, &format!("base height is {}", npc.height))?;
    Ok(format!("GetScale={scale:.4} refScale={ref_scale} height={:.2} effectiveHeight={:.4}", { npc.height }, npc.get_height()))
}

fn keyword(editor_id: &str) -> Result<&'static BGSKeyword, String> {
    form_by_editor_id(editor_id).and_then(TESForm::cast::<BGSKeyword>).ok_or_else(|| format!("{editor_id} did not resolve"))
}

fn ref_has_keyword() -> Result<String, String> {
    let player = player_ref()?;
    let npc_type = keyword("ActorTypeNPC")?;
    let vendor = keyword("VendorItemWeapon")?;
    check(!player.has_keyword(vendor), "the player has VendorItemWeapon")?;
    Ok(format!("has_keyword(ActorTypeNPC)={} has_keyword(VendorItemWeapon)=false", player.has_keyword(npc_type)))
}

fn is_disabled() -> Result<String, String> {
    let player = player_ref()?;
    check(!sre::TESForm_formFlags(player.formFlags).contains(sre::TESForm_formFlags::DISABLED), "the player is disabled")?;
    Ok(format!("is_disabled=false flags={:#x}", { player.formFlags }))
}

fn handle() -> Result<String, String> {
    let player = player_ref()?;
    let handle = player.handle();
    check(handle != RefHandle::INVALID, "GetHandle returned the null handle")?;
    let (index, age) = (handle.0 & 0xF_FFFF, (handle.0 >> 20) & 0x3F);
    check(index == player.refCount >> 11, &format!("handle index {index} is not refCount >> 11"))?;
    let refs = || player.refCount & 0x3FF;
    let before = refs();
    let found = handle.lookup().ok_or("ResolveRefHandle failed on the player's handle")?;
    check(address(&*found) == address(player), "the lookup returned another object")?;
    check(refs() == before + 1, "the lookup did not add one reference")?;
    drop(found);
    check(refs() == before, "dropping the RefPtr did not release the reference")?;
    check(RefHandle::INVALID.lookup().is_none(), "the null handle resolved")?;
    let stale = RefHandle(handle.0 ^ (1 << 20));
    check(stale.lookup().is_none(), "a handle with the wrong age resolved")?;
    Ok(format!("handle {handle} (index {index}, age {age}) -> the player, refCount {before} -> {} -> {before}; null and stale handles -> none", before + 1))
}

fn inventory() -> Result<String, String> {
    let player = player_ref()?;
    let changes = player.get_inventory_changes().ok_or("GetInventoryChanges returned null")?;
    check(changes.owner().is_some_and(|o| address(o) == address(player)), "the owner is not the player")?;
    let entries: Vec<String> = changes.items().map(|e| format!("{:?}x{:+}", e.object().map(describe), e.countDelta.cast_signed())).collect();
    let right = player.current_process().and_then(|p| p.equipped_right()).ok_or("no weapon in the right hand")?;
    check(changes.items().any(|e| e.object().is_some_and(|o| core::ptr::eq(o, right))), &format!("no entry for the weapon in hand, {}", describe(right)))?;
    let gold = changes.get_item_count(form(GOLD)?);
    let sword = changes.get_item_count(form(IRON_SWORD)?);
    let torch = changes.get_item_count(form(TORCH)?);
    check(gold == 140 && sword == 1 && torch == 2, &format!("counts gold={gold} sword={sword} torch={torch}"))?;
    Ok(format!("gold={gold} IronSword={sword} torch={torch}; {} entries: {}", entries.len(), entries.join(" ")))
}

fn extra_list() -> Result<String, String> {
    let player = player_ref()?;
    let refr: &TESObjectREFR = player;
    let list = &refr.extraList;
    check(list.has_type(EXTRA_CONTAINER_CHANGES) && list.has_type(EXTRA_ALIAS_INSTANCE_ARRAY), "ContainerChanges or AliasInstanceArray missing")?;
    let node = list.get_by_type(EXTRA_CONTAINER_CHANGES).ok_or("GetByType(0x15) returned null")?;
    let held = unsafe { &*core::ptr::from_ref(node).cast::<sre::ExtraContainerChanges>() }.changes;
    let changes = player.get_inventory_changes().ok_or("GetInventoryChanges returned null")?;
    check(core::ptr::eq(held, changes), "ExtraContainerChanges.changes differs from GetInventoryChanges")?;
    check(address(list).wrapping_sub(address(player)) == 0x70, "the extra list is not at +0x70")?;
    check(!list.has_type(0x1F), "the player has ExtraGhost")?;
    Ok("extraList at +0x70: HasType 0x15 and 0x88 true, 0x1f false; GetByType(0x15)->changes = GetInventoryChanges".into())
}

fn process() -> Result<String, String> {
    let player = player_ref()?;
    let process = player.current_process().ok_or("currentProcess is null")?;
    let right = process.equipped_right().ok_or("equippedRight is null")?;
    // Each runtime's base save holds its own weapon; the suite compares it with DevBench's answer.
    check((right.formID == IRON_WAR_AXE || !crate::is_canonical()) && right.formType == FormType::WEAP as u8, &format!("equippedRight is {}", describe(right)))?;
    check(process.equipped_left().is_none(), "equippedLeft is not null")?;
    let middle = process.middle_high().ok_or("middleHigh is null")?;
    let entry = middle.right_hand_entry().ok_or("rightHandEntry is null")?;
    check(entry.object().is_some_and(|o| core::ptr::eq(o, right)), "rightHandEntry.object differs from equippedRight")?;
    check(middle.left_hand_entry().is_none(), "leftHandEntry is not null")?;
    check(process.get_equipped_entry(false).is_some_and(|e| core::ptr::eq(e, entry)), "GetEquippedEntry(false) is not rightHandEntry")?;
    check(process.get_equipped_entry(true).is_none(), "GetEquippedEntry(true) is not null")?;
    let object = player.get_equipped_object(false).ok_or("GetEquippedObject(false) is null")?;
    check(core::ptr::eq(object, right), "GetEquippedObject(false) differs from equippedRight")?;
    check(skyrim::class_of(object).is_some_and(|c| c.id == sre::id!(vtables::TESObjectWEAP_vtable::ID)), "the weapon's vtable is not TESObjectWEAP's")?;
    check(player.get_equipped_object(true).is_none(), "GetEquippedObject(true) is not null")?;
    Ok(format!("right={} ({}) countDelta {} left=none; GetEquippedObject, equippedRight and rightHandEntry agree", describe(right), class_name(skyrim::class_of(object)), entry.countDelta.cast_signed()))
}

fn actor_values() -> Result<String, String> {
    let player = player_ref()?;
    let owner = player.as_actor_value_owner();
    check(address(owner).wrapping_sub(address(player)) == sre::base_offset!("Actor:ActorValueOwner"), "ActorValueOwner is not where the profile has it")?;
    let values: Vec<(ActorValue, f32)> = [ActorValue::Health, ActorValue::Magicka, ActorValue::Stamina, ActorValue::CarryWeight, ActorValue::SpeedMult]
        .into_iter()
        .map(|v| (v, owner.get_actor_value(v)))
        .collect();
    let expected = [100.0, 100.0, 100.0, 300.0, 100.0];
    check(values.iter().zip(expected).all(|((_, got), want)| (got - want).abs() < 0.01), &format!("values {values:?}"))?;
    let base = owner.get_base_actor_value(ActorValue::Health);
    let permanent = owner.get_permanent_actor_value(ActorValue::Health);
    check((base - 100.0).abs() < 0.01 && (permanent - 100.0).abs() < 0.01, &format!("base {base} permanent {permanent}"))?;
    let modifiers = [Modifier::Permanent, Modifier::Temporary, Modifier::Damage].map(|m| player.get_actor_value_modifier(m as i32, ActorValue::Health));
    let field = skyrim::render::floats::<3>(unsafe { &*player.healthModifiers_ptr() });
    check(modifiers.iter().chain(&field).all(|m| m.abs() < f32::EPSILON), &format!("health modifiers {modifiers:?}, healthModifiers {field:?}"))?;
    let text: Vec<String> = values.iter().map(|(v, x)| format!("{v:?}={x}")).collect();
    Ok(format!("{} base(Health)={base} permanent(Health)={permanent} healthModifiers={modifiers:?}", text.join(" ")))
}

fn actor_value_write() -> Result<String, String> {
    let owner = player_ref()?.as_actor_value_owner();
    let (speed, health, magicka) = (ActorValue::SpeedMult, ActorValue::Health, ActorValue::Magicka);
    owner.mod_actor_value(Modifier::Temporary as i32, speed, 5.0);
    let boosted = owner.get_actor_value(speed);
    owner.mod_actor_value(Modifier::Temporary as i32, speed, -5.0);
    let restored = owner.get_actor_value(speed);
    check((boosted - 105.0).abs() < 0.01 && (restored - 100.0).abs() < 0.01, &format!("ModActorValue: {boosted} then {restored}"))?;
    owner.store_base_actor_value(health, 110.0);
    let set = owner.get_base_actor_value(health);
    owner.add_base_actor_value(health, -10.0);
    let modded = owner.get_base_actor_value(health);
    check((set - 110.0).abs() < 0.01 && (modded - 100.0).abs() < 0.01, &format!("StoreBaseActorValue: {set}, AddBaseActorValue: {modded}"))?;
    owner.set_actor_value(magicka, 120.0);
    let via_set = owner.get_base_actor_value(magicka);
    owner.store_base_actor_value(magicka, 100.0);
    check((via_set - 120.0).abs() < 0.01 && (owner.get_base_actor_value(magicka) - 100.0).abs() < 0.01, &format!("SetActorValue: {via_set}"))?;
    Ok(format!("SpeedMult +5 temporary -> {boosted} -> {restored}; Health base 110 -> {set}, -10 -> {modded}; Magicka set 120 -> {via_set} -> 100"))
}

fn state() -> Result<String, String> {
    let player = player_ref()?;
    let state = player.as_actor_state();
    check(address(state).wrapping_sub(address(player)) == sre::base_offset!("Actor:ActorState"), "ActorState is not where the profile has it")?;
    let changed = STATE_CHANGED.load(Ordering::Relaxed);
    let flags1 = ActorState_flags1(state.flags1);
    let sneaking = flags1.contains(ActorState_flags1::SNEAKING);
    let weapon_state = (state.flags2 >> 5) & 7;
    let drawn = weapon_state >= 3;
    check(sneaking == changed, &format!("sneaking is {sneaking} (flags1 {:#x})", state.flags1))?;
    let mask = u16::try_from(ActorState_flags1::SNEAKING.bits() & 0x3FFF).unwrap_or(0);
    check(state.has_flags(mask) == changed, "HasFlags(SNEAKING) disagrees with the field")?;
    check(drawn == changed, &format!("drawn is {drawn} (weapon state {weapon_state})"))?;
    check(!flags1.contains(ActorState_flags1::SPRINTING) && state.flags1 & (0xF << 21) == 0, "sprinting or not alive")?;
    Ok(format!("flags1={:#x} flags2={:#x} sneaking={sneaking} weaponState={weapon_state} drawn={drawn} sprinting=false lifeState=0 (expected sneaking and drawn: {changed})", state.flags1, state.flags2))
}

fn actor_queries() -> Result<String, String> {
    let player = player_ref()?;
    check(!player.is_dead(true) && !player.is_dead(false), "IsDead is true")?;
    check(!player.is_in_combat(), "IsInCombat is true")?;
    check(!player.is_running(), "IsRunning is true")?;
    check(!player.is_on_mount(), "IsOnMount is true")?;
    check(!Actor_boolFlags(player.boolFlags()).contains(Actor_boolFlags::PLAYER_TEAMMATE), "the player is its own teammate")?;
    check(player.get_level() == 1, &format!("level is {}", player.get_level()))?;
    Ok(format!("dead=false inCombat=false running=false mounted=false teammate=false level=1 boolFlags={:#x}", player.boolFlags()))
}

fn race() -> Result<String, String> {
    let player = player_ref()?;
    let race: &TESRace = player.race().ok_or("race is null")?;
    check(race.formID == NORD_RACE && race.formType == FormType::RACE as u8, &format!("race is {}", describe(race)))?;
    let base_race = base_npc(player)?.race().ok_or("the base's race is null")?;
    check(core::ptr::eq(base_race, race), "Actor.race differs from TESNPC.race")?;
    Ok(format!("race={} = base race, editor id {:?}", describe(race), race.editor_id()))
}

fn npc() -> Result<String, String> {
    let npc = base_npc(player_ref()?)?;
    let name = npc.as_tes_full_name().full_name().ok_or("GetFullName returned null or empty")?;
    check(name.to_bytes() == b"Prisoner", &format!("full name is {name:?}"))?;
    check(npc.get_sex() == 0, &format!("sex is {}", npc.get_sex()))?;
    let class: &TESClass = npc.npc_class().ok_or("npcClass is null")?;
    check(class.formID == PLAYER_CLASS && class.formType == FormType::CLAS as u8, &format!("class is {}", describe(class)))?;
    let level = npc.as_tes_actor_base_data().get_level();
    check(level == 1, &format!("level is {level}"))?;
    let flags = TESActorBaseData_flags(npc.flags);
    let (unique, essential, female) = (flags.contains(TESActorBaseData_flags::UNIQUE), flags.contains(TESActorBaseData_flags::ESSENTIAL), flags.contains(TESActorBaseData_flags::FEMALE));
    check(unique && !essential && !female, &format!("base flags {:#x}", npc.flags))?;
    Ok(format!("fullName={name:?} sex=Male class={} level={level} flags={:#x} unique=true essential=false female=false height={} weight={}", describe(class), npc.flags, npc.height, npc.weight))
}

fn container() -> Result<String, String> {
    let player = player_ref()?;
    let npc = base_npc(player)?;
    let container = npc.as_tes_container();
    check(address(container).wrapping_sub(address(npc)) == 0x88, "TESContainer is not at +0x88")?;
    check(player.get_container().is_some_and(|c| core::ptr::eq(c, container)), "GetContainer differs from the base's component")?;
    let entries = container.entries();
    check(entries.len() == 16 && entries.iter().all(Option::is_some), &format!("{} entries", entries.len()))?;
    let gold = container.count_objects_in_container(form(GOLD)?);
    check(gold == 140, &format!("base gold count is {gold}"))?;
    let first: Vec<String> = entries.iter().take(3).flatten().map(|e| format!("{:?}x{}", e.form().map(describe), e.count)).collect();
    Ok(format!("TESContainer at +0x88 = GetContainer: {} entries, gold {gold}, first {}", entries.len(), first.join(" ")))
}

fn actor_value_names() -> Result<String, String> {
    check(lookup_actor_value("Health") == Some(ActorValue::Health) && ActorValue::Health as u32 == 24, "Health is not 24")?;
    check(lookup_actor_value("CarryWeight") == Some(ActorValue::CarryWeight) && lookup_actor_value("SpeedMult") == Some(ActorValue::SpeedMult), "CarryWeight or SpeedMult")?;
    check(lookup_actor_value("health") == Some(ActorValue::Health), "lookup is not case-insensitive")?;
    check(lookup_actor_value("NoSuchValue").is_none(), "NoSuchValue resolved")?;
    let name = |info: &sre::ActorValueInfo| (!info.name.is_null()).then(|| unsafe { core::ffi::CStr::from_ptr(info.name.cast()) });
    let info = ActorValueList::get_actor_value_info(ActorValue::Health).ok_or("GetActorValueInfo(Health) is null")?;
    check(info.formType == FormType::AVIF as u8 && name(info).is_some_and(|n| n.to_bytes() == b"Health"), &format!("info is {} {:?}", describe(info), name(info)))?;
    let list = ActorValueList::singleton().ok_or("g_ActorValueList_ptr is null")?;
    let infos = list.infos();
    let odd: Vec<String> = infos
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.is_some_and(|i| i.formType == FormType::AVIF as u8))
        .map(|(n, i)| format!("{n}:{}", i.map_or("null".into(), |i| describe(i))))
        .collect();
    check(infos.len() == 164 && odd.is_empty(), &format!("{} infos, not AVIF: {}", infos.len(), odd.join(" ")))?;
    check(infos[ActorValue::Health as usize].is_some_and(|i| core::ptr::eq(i, info)), "infos[24] is not GetActorValueInfo(24)")?;
    check(infos[0].is_some_and(|i| ActorValueList::get_actor_value_info(ActorValue::Aggression).is_some_and(|a| core::ptr::eq(i, a))), "infos[0] is not GetActorValueInfo(0)")?;
    let mut round_trips = 0;
    for (i, entry) in infos.iter().enumerate() {
        let value = ActorValue::from_u32(u32::try_from(i).unwrap()).ok_or(format!("no enum value {i}"))?;
        let text = entry.and_then(name).ok_or(format!("value {i} has no name"))?;
        check(lookup_actor_value(&text.to_string_lossy()) == Some(value), &format!("{value:?} ({i}) round trip through {text:?} failed"))?;
        round_trips += 1;
    }
    Ok(format!("Health=24 CarryWeight=32 SpeedMult=30, NoSuchValue=none, info(Health).name=Health, {round_trips} names round trip through the 164 AVIF forms"))
}

fn actor_downcast() -> Result<String, String> {
    let npc = form(PLAYER_NPC)?;
    check(npc.cast::<TESObjectREFR>().is_none() && npc.cast::<TESNPC>().is_some(), "NPC_ 0x7 is a reference or not an NPC")?;
    check(form(NORD_RACE)?.cast::<TESRace>().is_some() && form(NORD_RACE)?.cast::<TESObjectCELL>().is_none(), "0x13746 is not a race")?;
    check(form(RIVERWOOD_CELL)?.cast::<TESObjectCELL>().is_some(), "0x9732 is not a cell")?;
    check(form(TAMRIEL)?.cast::<TESWorldSpace>().is_some(), "0x3c is not a worldspace")?;
    check(form(RIVERWOOD_LOCATION)?.cast::<BGSLocation>().is_some(), "0x13163 is not a location")?;
    check(form(PLAYER_CLASS)?.cast::<TESClass>().is_some(), "0x2f202 is not a class")?;
    check(form(IRON_SWORD)?.cast::<sre::Actor>().is_none(), "a weapon is an actor")?;
    Ok("0x7 -> TESNPC not a reference; 0x13746 RACE, 0x9732 CELL, 0x3c WRLD, 0x13163 LCTN, 0x2f202 CLAS; 0x14 -> Actor/Character/PlayerCharacter".into())
}
