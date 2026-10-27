//! One test per `skyrim::actors` API on the player of the `retrace-base` save; the expected values
//! are those of the RE repo's `docs/subsystems/actors-verify.md`. `STATE_CHANGED` tells the state
//! test whether `DevBench` has already made the player sneak and draw the weapon.

use std::sync::atomic::{AtomicBool, Ordering};

use sre::raw::vtables;
use skyrim::actors::{ActorValue, ActorValueList, Modifier, PlayerCharacter, RefHandle, Sex, StateFlags, actor_value_info, lookup_actor_value};
use skyrim::forms::{FormId, FormType, TESForm, find_form_by_editor_id};

use crate::{IRON_SWORD, PLAYER, Test, check, form};

pub static STATE_CHANGED: AtomicBool = AtomicBool::new(false);

const PLAYER_NPC: FormId = FormId(0x7);
const GOLD: FormId = FormId(0xF);
const TORCH: FormId = FormId(0x1D4EC);
const IRON_WAR_AXE: FormId = FormId(0x13790);
const RIVERWOOD_CELL: FormId = FormId(0x9732);
const TAMRIEL: FormId = FormId(0x3C);
const RIVERWOOD_LOCATION: FormId = FormId(0x13163);
const NORD_RACE: FormId = FormId(0x13746);
const PLAYER_CLASS: FormId = FormId(0x2F202);
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
    PlayerCharacter::get().ok_or_else(|| "g_PlayerCharacter_ptr is null".into())
}

fn class_name(vtable: Option<&'static sre::raw::Vtable>) -> &'static str {
    vtable.map_or("unknown", |v| v.class)
}

fn player() -> Result<String, String> {
    let player = player_ref()?;
    check(player.form_id() == PLAYER && player.form_type() == Some(FormType::ACHR), "the player is not ACHR 0x14")?;
    let form = form(PLAYER)?;
    let refr = form.as_reference().ok_or("as_reference failed")?;
    let actor = refr.as_actor().ok_or("as_actor failed")?;
    check(std::ptr::eq(actor.as_ptr().cast(), player.as_ptr()), "FindFormByID(0x14) and g_PlayerCharacter_ptr differ")?;
    check(actor.as_player().is_some() && actor.as_character().is_some(), "the player does not downcast to PlayerCharacter and Character")?;
    let class = player.class().ok_or("the player's vtable is not in the ledger")?;
    check(class.id == vtables::PlayerCharacter_vtable::ID, &format!("the player's class is {}", class.class))?;
    check(player.ref_count() >= 1, "refCount is 0")?;
    Ok(format!("{player:?} at {:p}, class {}, refCount {}", player.as_ptr(), class.class, player.ref_count()))
}

fn base_object() -> Result<String, String> {
    let player = player_ref()?;
    let base = player.base_object().ok_or("baseObject is null")?;
    check(base.form_id() == PLAYER_NPC && base.form_type() == Some(FormType::NPC_), &format!("baseObject is {base:?}"))?;
    let npc = player.base_npc().ok_or("base_npc is None")?;
    check(std::ptr::eq(npc.raw(), base.as_ptr().cast()), "base_npc is not baseObject")?;
    check(npc.class().is_some_and(|c| c.id == vtables::TESNPC_vtable::ID), "the base's vtable is not TESNPC's")?;
    Ok(format!("baseObject = {base:?} ({})", class_name(base.class())))
}

fn position() -> Result<String, String> {
    let player = player_ref()?;
    let [x, y, z] = player.position();
    let [rx, ry, rz] = player.rotation();
    check(x.is_finite() && y.is_finite() && z.is_finite(), "position is not finite")?;
    Ok(format!("pos=({x:.4}, {y:.4}, {z:.4}) rot=({rx:.4}, {ry:.4}, {rz:.4}) headingDeg={:.4}", rz.to_degrees()))
}

fn parent_cell() -> Result<String, String> {
    let player = player_ref()?;
    let cell = player.parent_cell().ok_or("parentCell is null")?;
    check(cell.form_id() == RIVERWOOD_CELL && cell.form_type() == Some(FormType::CELL), &format!("parentCell is {cell:?}"))?;
    let world = player.worldspace().ok_or("GetWorldspace returned null")?;
    check(world.form_id() == TAMRIEL && world.form_type() == Some(FormType::WRLD), &format!("worldspace is {world:?}"))?;
    let location = player.current_location().ok_or("GetCurrentLocation returned null")?;
    check(location.form_id() == RIVERWOOD_LOCATION && location.form_type() == Some(FormType::LCTN), &format!("location is {location:?}"))?;
    let field = player.raw().currentLocation;
    check(std::ptr::eq(field.cast_const(), location.raw()), "PlayerCharacter.currentLocation differs from GetCurrentLocation")?;
    Ok(format!("parentCell={cell:?} worldspace={world:?} location={location:?} = PlayerCharacter.currentLocation"))
}

fn node_3d() -> Result<String, String> {
    let player = player_ref()?;
    let node = player.node_3d().ok_or("Get3D returned null")?;
    let loaded = player.loaded_node_3d().ok_or("loadedData->node3D is null")?;
    let class = node.class().ok_or("the node's vtable is not in the ledger")?;
    let loaded_class = loaded.class().ok_or("the loaded node's vtable is not in the ledger")?;
    check(class.id == vtables::BSFadeNode_vtable::ID && loaded_class.id == vtables::BSFadeNode_vtable::ID, &format!("the root nodes are {} and {}", class.class, loaded_class.class))?;
    let same = std::ptr::eq(node.raw(), loaded.raw());
    Ok(format!("Get3D at {:p} ({}), loadedData->node3D at {:p} ({}), same={same}", node.raw(), class.class, loaded.raw(), loaded_class.class))
}

fn scale() -> Result<String, String> {
    let player = player_ref()?;
    let scale = player.scale();
    let npc = player.base_npc().ok_or("base_npc is None")?;
    check(player.ref_scale() == 100, &format!("refScale is {}", player.ref_scale()))?;
    check((scale - 1.03).abs() < 0.001, &format!("GetScale is {scale}"))?;
    check((npc.height() - 1.0).abs() < f32::EPSILON, &format!("base height is {}", npc.height()))?;
    Ok(format!("GetScale={scale:.4} refScale={} height={:.2} effectiveHeight={:.4}", player.ref_scale(), npc.height(), npc.effective_height()))
}

fn ref_has_keyword() -> Result<String, String> {
    let player = player_ref()?;
    let npc_type = find_form_by_editor_id("ActorTypeNPC").and_then(TESForm::as_keyword).ok_or("ActorTypeNPC did not resolve")?;
    let vendor = find_form_by_editor_id("VendorItemWeapon").and_then(TESForm::as_keyword).ok_or("VendorItemWeapon did not resolve")?;
    check(!player.has_keyword(vendor), "the player has VendorItemWeapon")?;
    Ok(format!("has_keyword(ActorTypeNPC)={} has_keyword(VendorItemWeapon)=false", player.has_keyword(npc_type)))
}

fn is_disabled() -> Result<String, String> {
    let player = player_ref()?;
    check(!player.is_disabled(), "the player is disabled")?;
    Ok(format!("is_disabled=false flags={:#x}", player.flags().bits()))
}

fn handle() -> Result<String, String> {
    let player = player_ref()?;
    let handle = player.handle();
    check(!handle.is_invalid(), "GetHandle returned the null handle")?;
    check(handle.index() == player.raw().refCount >> 11, &format!("handle index {} is not refCount >> 11", handle.index()))?;
    let before = player.ref_count();
    let found = handle.lookup().ok_or("ResolveRefHandle failed on the player's handle")?;
    check(std::ptr::eq(found.raw(), player.as_ptr().cast()), "the lookup returned another object")?;
    check(player.ref_count() == before + 1, "the lookup did not add one reference")?;
    drop(found);
    check(player.ref_count() == before, "dropping the RefPtr did not release the reference")?;
    check(RefHandle::INVALID.lookup().is_none(), "the null handle resolved")?;
    let stale = RefHandle(handle.0 ^ (1 << 20));
    check(stale.lookup().is_none(), "a handle with the wrong age resolved")?;
    Ok(format!("handle {handle} (index {}, age {}) -> the player, refCount {before} -> {} -> {before}; null and stale handles -> none", handle.index(), handle.age(), before + 1))
}

fn inventory() -> Result<String, String> {
    let player = player_ref()?;
    let changes = player.inventory_changes().ok_or("GetInventoryChanges returned null")?;
    check(changes.owner().is_some_and(|o| std::ptr::eq(o.raw(), player.as_ptr().cast())), "the owner is not the player")?;
    let entries: Vec<String> = changes.entries().map(|e| format!("{:?}x{:+}", e.object(), e.count_delta())).collect();
    check(changes.entries().any(|e| e.object().is_some_and(|o| o.form_id() == IRON_WAR_AXE)), "no entry for the Iron War Axe")?;
    let gold = changes.item_count(form(GOLD)?);
    let sword = changes.item_count(form(IRON_SWORD)?);
    let torch = changes.item_count(form(TORCH)?);
    check(gold == 140 && sword == 1 && torch == 2, &format!("counts gold={gold} sword={sword} torch={torch}"))?;
    check(player.item_count(form(GOLD)?) == 140 && player.item_count(form(TORCH)?) == 2, "TESObjectREFR::item_count differs")?;
    Ok(format!("gold={gold} IronSword={sword} torch={torch}; {} entries: {}", entries.len(), entries.join(" ")))
}

fn extra_list() -> Result<String, String> {
    let player = player_ref()?;
    let list = player.extra_list();
    check(list.has_type(EXTRA_CONTAINER_CHANGES) && list.has_type(EXTRA_ALIAS_INSTANCE_ARRAY), "ContainerChanges or AliasInstanceArray missing")?;
    let node = list.get_by_type(EXTRA_CONTAINER_CHANGES).ok_or("GetByType(0x15) returned null")?;
    let held = unsafe { &*std::ptr::from_ref(node).cast::<sre::ExtraContainerChanges>() }.changes;
    let changes = player.inventory_changes().ok_or("GetInventoryChanges returned null")?;
    check(std::ptr::eq(held.cast_const(), changes.raw()), "ExtraContainerChanges.changes differs from GetInventoryChanges")?;
    let offset = (std::ptr::from_ref(list.raw()) as usize).wrapping_sub(player.as_ptr() as usize);
    check(offset == 0x70, "the extra list is not at +0x70")?;
    check(!list.has_type(0x1F), "the player has ExtraGhost")?;
    Ok("extraList at +0x70: HasType 0x15 and 0x88 true, 0x1f false; GetByType(0x15)->changes = GetInventoryChanges".into())
}

fn process() -> Result<String, String> {
    let player = player_ref()?;
    let process = player.process().ok_or("currentProcess is null")?;
    let right = process.equipped_right().ok_or("equippedRight is null")?;
    check(right.form_id() == IRON_WAR_AXE && right.form_type() == Some(FormType::WEAP), &format!("equippedRight is {right:?}"))?;
    check(process.equipped_left().is_none(), "equippedLeft is not null")?;
    let middle = process.middle_high().ok_or("middleHigh is null")?;
    let entry = middle.right_hand_entry().ok_or("rightHandEntry is null")?;
    check(entry.object().is_some_and(|o| std::ptr::eq(o, right)), "rightHandEntry.object differs from equippedRight")?;
    check(middle.left_hand_entry().is_none(), "leftHandEntry is not null")?;
    check(process.equipped_entry(false).is_some_and(|e| std::ptr::eq(e, entry)), "GetEquippedEntry(false) is not rightHandEntry")?;
    check(process.equipped_entry(true).is_none(), "GetEquippedEntry(true) is not null")?;
    let object = player.equipped_object(false).ok_or("GetEquippedObject(false) is null")?;
    check(std::ptr::eq(object, right), "GetEquippedObject(false) differs from equippedRight")?;
    check(object.class().is_some_and(|c| c.id == vtables::TESObjectWEAP_vtable::ID), "the weapon's vtable is not TESObjectWEAP's")?;
    check(player.equipped_object(true).is_none(), "GetEquippedObject(true) is not null")?;
    Ok(format!("right={right:?} ({}) countDelta {} left=none; GetEquippedObject, equippedRight and rightHandEntry agree", class_name(object.class()), entry.count_delta()))
}

fn actor_values() -> Result<String, String> {
    let player = player_ref()?;
    let owner = player.actor_value_owner();
    let offset = (std::ptr::from_ref(owner) as usize).wrapping_sub(player.as_ptr() as usize);
    check(offset == 0xB8, "ActorValueOwner is not at +0xB8")?;
    let values: Vec<(ActorValue, f32)> = [ActorValue::Health, ActorValue::Magicka, ActorValue::Stamina, ActorValue::CarryWeight, ActorValue::SpeedMult]
        .into_iter()
        .map(|v| (v, owner.get(v)))
        .collect();
    let expected = [100.0, 100.0, 100.0, 300.0, 100.0];
    check(values.iter().zip(expected).all(|((_, got), want)| (got - want).abs() < 0.01), &format!("values {values:?}"))?;
    let base = owner.get_base(ActorValue::Health);
    let permanent = owner.get_permanent(ActorValue::Health);
    check((base - 100.0).abs() < 0.01 && (permanent - 100.0).abs() < 0.01, &format!("base {base} permanent {permanent}"))?;
    let modifiers = player.modifiers(ActorValue::Health).ok_or("no health modifiers")?;
    check(modifiers.iter().all(|m| m.abs() < f32::EPSILON) && player.modifiers(ActorValue::SpeedMult).is_none(), &format!("health modifiers {modifiers:?}"))?;
    let text: Vec<String> = values.iter().map(|(v, x)| format!("{v:?}={x}")).collect();
    Ok(format!("{} base(Health)={base} permanent(Health)={permanent} healthModifiers={modifiers:?}", text.join(" ")))
}

fn actor_value_write() -> Result<String, String> {
    let owner = player_ref()?.actor_value_owner();
    owner.mod_(Modifier::Temporary, ActorValue::SpeedMult, 5.0);
    let boosted = owner.get(ActorValue::SpeedMult);
    owner.mod_(Modifier::Temporary, ActorValue::SpeedMult, -5.0);
    let restored = owner.get(ActorValue::SpeedMult);
    check((boosted - 105.0).abs() < 0.01 && (restored - 100.0).abs() < 0.01, &format!("ModActorValue: {boosted} then {restored}"))?;
    owner.store_base(ActorValue::Health, 110.0);
    let set = owner.get_base(ActorValue::Health);
    owner.add_base(ActorValue::Health, -10.0);
    let modded = owner.get_base(ActorValue::Health);
    check((set - 110.0).abs() < 0.01 && (modded - 100.0).abs() < 0.01, &format!("StoreBaseActorValue: {set}, AddBaseActorValue: {modded}"))?;
    owner.set(ActorValue::Magicka, 120.0);
    let via_set = owner.get_base(ActorValue::Magicka);
    owner.store_base(ActorValue::Magicka, 100.0);
    check((via_set - 120.0).abs() < 0.01 && (owner.get_base(ActorValue::Magicka) - 100.0).abs() < 0.01, &format!("SetActorValue: {via_set}"))?;
    Ok(format!("SpeedMult +5 temporary -> {boosted} -> {restored}; Health base 110 -> {set}, -10 -> {modded}; Magicka set 120 -> {via_set} -> 100"))
}

fn state() -> Result<String, String> {
    let player = player_ref()?;
    let state = player.state();
    let offset = (std::ptr::from_ref(state) as usize).wrapping_sub(player.as_ptr() as usize);
    check(offset == 0xC0, "ActorState is not at +0xC0")?;
    let changed = STATE_CHANGED.load(Ordering::Relaxed);
    check(state.is_sneaking() == changed, &format!("is_sneaking is {} (flags1 {:#x})", state.is_sneaking(), state.flags1().bits()))?;
    check(state.has_flags(StateFlags::SNEAKING) == changed, "HasFlags(SNEAKING) disagrees with the field")?;
    check(state.is_weapon_drawn() == changed, &format!("is_weapon_drawn is {} (weapon state {})", state.is_weapon_drawn(), state.weapon_state()))?;
    check(!state.is_sprinting() && state.life_state() == 0, "sprinting or not alive")?;
    Ok(format!(
        "flags1={:#x} flags2={:#x} sneaking={} weaponState={} drawn={} sprinting=false lifeState=0 (expected sneaking and drawn: {changed})",
        state.flags1().bits(),
        state.flags2(),
        state.is_sneaking(),
        state.weapon_state(),
        state.is_weapon_drawn()
    ))
}

fn actor_queries() -> Result<String, String> {
    let player = player_ref()?;
    check(!player.is_dead(true) && !player.is_dead(false), "IsDead is true")?;
    check(!player.is_in_combat(), "IsInCombat is true")?;
    check(!player.is_running(), "IsRunning is true")?;
    check(!player.is_on_mount(), "IsOnMount is true")?;
    check(!player.is_player_teammate(), "the player is its own teammate")?;
    check(player.level() == 1, &format!("level is {}", player.level()))?;
    Ok(format!("dead=false inCombat=false running=false mounted=false teammate=false level=1 boolFlags={:#x}", player.flags().bits()))
}

fn race() -> Result<String, String> {
    let player = player_ref()?;
    let race = player.race().ok_or("race is null")?;
    check(race.form_id() == NORD_RACE && race.form_type() == Some(FormType::RACE), &format!("race is {race:?}"))?;
    let base_race = player.base_npc().and_then(|n| n.race()).ok_or("the base's race is null")?;
    check(std::ptr::eq(base_race, race), "Actor.race differs from TESNPC.race")?;
    Ok(format!("race={race:?} = base race, editor id {:?}", race.editor_id()))
}

fn npc() -> Result<String, String> {
    let npc = player_ref()?.base_npc().ok_or("base_npc is None")?;
    let name = npc.full_name().ok_or("GetFullName returned null or empty")?;
    check(name.to_bytes() == b"Prisoner", &format!("full name is {name:?}"))?;
    check(npc.sex() == Some(Sex::Male), &format!("sex is {:?}", npc.sex()))?;
    let class = npc.npc_class().ok_or("npcClass is null")?;
    check(class.form_id() == PLAYER_CLASS && class.form_type() == Some(FormType::CLAS), &format!("class is {class:?}"))?;
    check(npc.level() == 1, &format!("level is {}", npc.level()))?;
    check(npc.is_unique() && !npc.is_essential() && !npc.is_female(), &format!("base flags {:#x}", npc.base_flags().bits()))?;
    Ok(format!(
        "fullName={name:?} sex=Male class={class:?} level={} flags={:#x} unique=true essential=false female=false height={} weight={}",
        npc.level(),
        npc.base_flags().bits(),
        npc.height(),
        npc.weight()
    ))
}

fn container() -> Result<String, String> {
    let player = player_ref()?;
    let npc = player.base_npc().ok_or("base_npc is None")?;
    let container = npc.container();
    let offset = (std::ptr::from_ref(container) as usize).wrapping_sub(std::ptr::from_ref(npc.raw()) as usize);
    check(offset == 0x88, "TESContainer is not at +0x88")?;
    check(player.container().is_some_and(|c| std::ptr::eq(c, container)), "GetContainer differs from the base's component")?;
    let entries = container.entries();
    check(entries.len() == 16 && entries.iter().all(Option::is_some), &format!("{} entries", entries.len()))?;
    let gold = container.count(form(GOLD)?);
    check(gold == 140, &format!("base gold count is {gold}"))?;
    let first: Vec<String> = entries.iter().take(3).flatten().map(|e| format!("{:?}x{}", e.form(), e.count())).collect();
    Ok(format!("TESContainer at +0x88 = GetContainer: {} entries, gold {gold}, first {}", entries.len(), first.join(" ")))
}

fn actor_value_names() -> Result<String, String> {
    check(lookup_actor_value("Health") == Some(ActorValue::Health) && ActorValue::Health as u32 == 24, "Health is not 24")?;
    check(lookup_actor_value("CarryWeight") == Some(ActorValue::CarryWeight) && lookup_actor_value("SpeedMult") == Some(ActorValue::SpeedMult), "CarryWeight or SpeedMult")?;
    check(lookup_actor_value("health") == Some(ActorValue::Health), "lookup is not case-insensitive")?;
    check(lookup_actor_value("NoSuchValue").is_none(), "NoSuchValue resolved")?;
    let info = actor_value_info(ActorValue::Health).ok_or("GetActorValueInfo(Health) is null")?;
    check(info.form_type() == Some(FormType::AVIF) && info.name().is_some_and(|n| n.to_bytes() == b"Health"), &format!("info is {info:?} {:?}", info.name()))?;
    let list = ActorValueList::get().ok_or("g_ActorValueList_ptr is null")?;
    let infos = list.infos();
    let odd: Vec<String> = infos
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.is_some_and(|i| i.form_type() == Some(FormType::AVIF)))
        .map(|(n, i)| format!("{n}:{}", i.map_or("null".into(), |i| format!("{i:?}"))))
        .collect();
    check(infos.len() == 164 && odd.is_empty(), &format!("{} infos, not AVIF: {}", infos.len(), odd.join(" ")))?;
    check(infos[ActorValue::Health as usize].is_some_and(|i| std::ptr::eq(i, info)), "infos[24] is not GetActorValueInfo(24)")?;
    check(infos[0].is_some_and(|i| actor_value_info(ActorValue::Aggression).is_some_and(|a| std::ptr::eq(i, a))), "infos[0] is not GetActorValueInfo(0)")?;
    let mut round_trips = 0;
    for (i, entry) in infos.iter().enumerate() {
        let value = ActorValue::from_u32(u32::try_from(i).unwrap()).ok_or(format!("no enum value {i}"))?;
        let name = entry.and_then(|e| e.name()).ok_or(format!("value {i} has no name"))?;
        check(lookup_actor_value(&name.to_string_lossy()) == Some(value), &format!("{value:?} ({i}) round trip through {name:?} failed"))?;
        round_trips += 1;
    }
    Ok(format!("Health=24 CarryWeight=32 SpeedMult=30, NoSuchValue=none, info(Health).name=Health, {round_trips} names round trip through the 164 AVIF forms"))
}

fn actor_downcast() -> Result<String, String> {
    let npc = form(PLAYER_NPC)?;
    check(npc.as_reference().is_none() && npc.as_npc().is_some(), "NPC_ 0x7 is a reference or not an NPC")?;
    check(form(NORD_RACE)?.as_race().is_some() && form(NORD_RACE)?.as_cell().is_none(), "0x13746 is not a race")?;
    check(form(RIVERWOOD_CELL)?.as_cell().is_some(), "0x9732 is not a cell")?;
    check(form(TAMRIEL)?.as_worldspace().is_some(), "0x3c is not a worldspace")?;
    check(form(RIVERWOOD_LOCATION)?.as_location().is_some(), "0x13163 is not a location")?;
    check(form(PLAYER_CLASS)?.as_class().is_some(), "0x2f202 is not a class")?;
    check(form(IRON_SWORD)?.as_reference().and_then(|r| r.as_actor()).is_none(), "a weapon is an actor")?;
    Ok("0x7 -> TESNPC not a reference; 0x13746 RACE, 0x9732 CELL, 0x3c WRLD, 0x13163 LCTN, 0x2f202 CLAS; 0x14 -> Actor/Character/PlayerCharacter".into())
}
