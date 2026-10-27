//! The forms suite through the generated `sre` methods and the `skyrim` prelude; the expected
//! values are those of the RE repo's `docs/subsystems/forms-verify.md`.

use core::ffi::CStr;

use skyrim::actors::ActorValueListExt;
use skyrim::forms::form_by_editor_id;
use skyrim::prelude::*;
use sre::raw::globals;
use sre::{ActorValue, BGSKeyword, BGSListForm, FormType, TESDataHandler, TESFile, TESForm, TESGlobal, TESNPC, TESObjectREFR, TESObjectWEAP};

use crate::{IRON_SWORD, PLAYER, Test, check, describe, file_name, form, keyword_field};

const VENDOR_ITEM_WEAPON: u32 = 0x8F958;

pub const TESTS: &[(&str, Test)] = &[
    ("lookup_player", lookup_player),
    ("lookup_weapon", lookup_weapon),
    ("lookup_missing", lookup_missing),
    ("editor_id_keyword", editor_id_keyword),
    ("editor_id_missing", editor_id_missing),
    ("editor_id_global", editor_id_global),
    ("source_files", source_files),
    ("downcast", downcast),
    ("as_reference", as_reference),
    ("keywords", keywords),
    ("data_handler", data_handler),
    ("form_arrays", form_arrays),
    ("files", files),
    ("get_file", get_file),
    ("form_list", form_list),
    ("next_form_id", next_form_id),
    ("generated_api", generated_api),
];

fn address<T>(object: &T) -> usize {
    core::ptr::from_ref(object) as usize
}

fn file_flag(file: &TESFile, flag: sre::TESFile_fileFlags) -> bool {
    sre::TESFile_fileFlags(file.fileFlags).contains(flag)
}

fn handler() -> Result<&'static TESDataHandler, String> {
    TESDataHandler::singleton().ok_or_else(|| "g_TESDataHandler_ptr is null".into())
}

fn lookup_player() -> Result<String, String> {
    let player = form(PLAYER)?;
    let global = skyrim::reloc().address(globals::g_PlayerCharacter_ptr).map_err(|e| e.to_string())?;
    let held = unsafe { *(global as *const *const TESForm) };
    check(core::ptr::eq(held, player), "FindFormByID(0x14) is not the object g_PlayerCharacter_ptr holds")?;
    check(player.formID == PLAYER, "formID is not 0x14")?;
    check(player.formType == FormType::ACHR as u8, "formType is not ACHR")?;
    check(player.source_files().is_none() && player.get_file(0).is_none(), "the player has source files")?;
    Ok(format!("{} at {:#x} = g_PlayerCharacter_ptr, flags {:#x}, no source files", describe(player), address(player), { player.formFlags }))
}

fn lookup_weapon() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    check(sword.formID == IRON_SWORD, "formID is not 0x12eb7")?;
    check(sword.formType == FormType::WEAP as u8, "formType is not WEAP")?;
    check(sword.editor_id().is_none(), "a weapon reported an editor id")?;
    Ok(format!("formID={:08X} type={} ({}) editor_id=none", { sword.formID }, { sword.formType }, FormType::WEAP.signature()))
}

fn lookup_missing() -> Result<String, String> {
    check(sre::find_form_by_id(0x7FFF_FFFF).is_none(), "0x7fffffff resolved to a form")?;
    Ok("FindFormByID(0x7fffffff) = null".into())
}

fn editor_id_keyword() -> Result<String, String> {
    let keyword = form_by_editor_id("VendorItemWeapon").ok_or("VendorItemWeapon did not resolve")?;
    check(keyword.formID == VENDOR_ITEM_WEAPON, "formID is not 0x8f958")?;
    check(keyword.formType == FormType::KYWD as u8, "formType is not KYWD")?;
    let via_slot = keyword.editor_id().map(|s| s.to_string_lossy().into_owned());
    check(via_slot.as_deref() == Some("VendorItemWeapon"), "GetFormEditorID did not return VendorItemWeapon")?;
    let via_field = keyword.cast::<BGSKeyword>().and_then(|k| keyword_field(k).to_str());
    check(via_field == Some("VendorItemWeapon"), "BGSKeyword.keyword is not VendorItemWeapon (empty before a game is loaded)")?;
    check(form_by_editor_id("vendoritemweapon").map(|f| f.formID) == Some(VENDOR_ITEM_WEAPON), "lookup is not case-insensitive")?;
    Ok(format!("formID={:08X} type=4 (KYWD) editor_id=VendorItemWeapon keyword=VendorItemWeapon", { keyword.formID }))
}

fn editor_id_missing() -> Result<String, String> {
    for name in ["IronSword", "HousecarlWhiterun", "NoSuchEditorId"] {
        check(form_by_editor_id(name).is_none(), &format!("{name} resolved to a form"))?;
    }
    Ok("IronSword, HousecarlWhiterun, NoSuchEditorId = null".into())
}

fn editor_id_global() -> Result<String, String> {
    let hour = form_by_editor_id("GameHour").ok_or("GameHour did not resolve")?;
    check(hour.formType == FormType::GLOB as u8, "formType is not GLOB")?;
    let value = hour.cast::<TESGlobal>().ok_or("cast to TESGlobal returned None")?.value;
    Ok(format!("GameHour={value:.2} formID={:08X}", { hour.formID }))
}

fn source_files() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let files: Vec<&str> = (0..).map_while(|i| sword.get_file(i)).map(file_name).collect();
    check(files == ["Skyrim.esm"], "IronSword's source files are not [Skyrim.esm]")?;
    check(sword.source_files().is_some_and(|a| a.count == 1), "TESForm.sourceFiles does not hold one file")?;
    Ok(format!("IronSword source files = {files:?}"))
}

fn downcast() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let npc = form(0x7)?;
    let player = form(PLAYER)?;
    check(sword.cast::<TESObjectWEAP>().is_some() && sword.cast::<TESNPC>().is_none() && sword.cast::<BGSKeyword>().is_none(), "weapon casts")?;
    check(npc.formType == FormType::NPC_ as u8 && npc.cast::<TESNPC>().is_some() && npc.cast::<TESObjectWEAP>().is_none(), "NPC casts")?;
    check(player.cast::<TESNPC>().is_none() && player.cast::<TESObjectWEAP>().is_none() && player.cast::<BGSListForm>().is_none(), "player casts")?;
    check(sword.cast::<TESObjectWEAP>().is_some_and(|w| address(w) == address(sword)), "the cast moved the pointer")?;
    Ok(format!("0x12eb7 -> TESObjectWEAP, 0x7 -> TESNPC ({}), 0x14 -> neither", describe(npc)))
}

fn as_reference() -> Result<String, String> {
    let player = form(PLAYER)?;
    let sword = form(IRON_SWORD)?;
    let refr = player.cast::<TESObjectREFR>().ok_or("the player is not a reference")?;
    check(address(refr) == address(player), "AsReference moved the player pointer")?;
    check(refr.formID == PLAYER, "the reference's formID is not 0x14")?;
    check(sword.cast::<TESObjectREFR>().is_none(), "a weapon is a reference")?;
    Ok("player -> TESObjectREFR at the same address, IronSword -> none".into())
}

fn keyword(editor_id: &str) -> Result<&'static BGSKeyword, String> {
    form_by_editor_id(editor_id).and_then(TESForm::cast::<BGSKeyword>).ok_or_else(|| format!("{editor_id} did not resolve"))
}

fn keywords() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let vendor = form(VENDOR_ITEM_WEAPON)?.cast::<BGSKeyword>().ok_or("0x8f958 is not a keyword")?;
    let component = sword.get_keyword_form().ok_or("GetKeywordForm returned null")?;
    check(address(component).wrapping_sub(address(sword)) == 0x140, "the keyword form is not at +0x140")?;
    let found: Vec<String> = sword.keywords().iter().map(|k| k.map_or("null".into(), |k| keyword_field(k).to_string())).collect();
    check(found == ["WeapMaterialIron", "WeapTypeSword", "VendorItemWeapon"], &format!("keywords are {found:?}"))?;
    check(component.has_keyword(vendor), "HasKeyword(VendorItemWeapon) is false")?;
    check(!component.has_keyword(keyword("VendorItemArmor")?), "HasKeyword(VendorItemArmor) is true")?;
    check(form(PLAYER)?.get_keyword_form().is_none() && form(PLAYER)?.keywords().is_empty(), "the player has a keyword form")?;
    Ok(format!("IronSword keyword form at +0x140: {} has_keyword(VendorItemWeapon)=true has_keyword(VendorItemArmor)=false", found.join(",")))
}

fn data_handler() -> Result<String, String> {
    let handler = handler()?;
    let object = skyrim::reloc().address(globals::g_TESDataHandler).map_err(|e| e.to_string())?;
    check(object == address(handler), "the pointer does not hold the static object")?;
    Ok(format!("TESDataHandler at {:#x} = g_TESDataHandler (base + {:#x})", object, object - skyrim::reloc().base()))
}

fn form_arrays() -> Result<String, String> {
    let handler = handler()?;
    let mut total = 0usize;
    let mut nulls = 0usize;
    let mut mismatched = 0usize;
    for ty in (0..=u8::MAX).filter_map(FormType::from_u8) {
        for entry in handler.forms_of_type(ty) {
            total += 1;
            match entry {
                None => nulls += 1,
                Some(f) if f.formType != ty as u8 => mismatched += 1,
                Some(_) => {}
            }
        }
    }
    check(nulls == 0 && mismatched == 0, &format!("{nulls} null entries, {mismatched} entries of another type"))?;
    let counts: Vec<String> = [FormType::KYWD, FormType::GLOB, FormType::WEAP, FormType::NPC_, FormType::FLST]
        .iter()
        .map(|&t| format!("{}={}", t.signature(), handler.forms_of_type(t).len()))
        .collect();
    Ok(format!("{total} forms, every entry of its array's type; {}", counts.join(" ")))
}

fn masters(file: &TESFile) -> Vec<&str> {
    (0..file.get_master_count()).map(|i| file.get_master(i).map_or("null", file_name)).collect()
}

fn files() -> Result<String, String> {
    let handler = handler()?;
    let first = handler.files().next().ok_or("the file list is empty")?;
    check(file_name(first) == "Skyrim.esm" && first.is_master() && file_flag(first, sre::TESFile_fileFlags::ACTIVE) && !first.is_light(), "the first file is not an active master Skyrim.esm")?;
    check(first.compileIndex == 0 && first.get_master_count() == 0, "Skyrim.esm is not compile index 0 without masters")?;
    let listed = handler.files().count();
    let compiled: Vec<String> = handler.compiled_files().iter().enumerate().map(|(i, f)| describe_file(i, *f, false)).collect();
    let light: Vec<String> = handler.light_files().iter().enumerate().map(|(i, f)| describe_file(i, *f, true)).collect();
    let update = handler.files().nth(1).ok_or("no second file")?;
    let update_masters = masters(update);
    check(update_masters == ["Skyrim.esm"], &format!("Update.esm's masters are {update_masters:?}"))?;
    Ok(format!("{listed} files listed; compiled=[{}] light=[{}]; Update.esm masters={update_masters:?}", compiled.join(","), light.join(",")))
}

fn describe_file(index: usize, file: Option<&TESFile>, light: bool) -> String {
    let Some(f) = file else { return format!("{index}:null") };
    let own = if light { usize::from(f.lightFileIndex) } else { usize::from(f.compileIndex) };
    let (master, active, is_light) = (file_flag(f, sre::TESFile_fileFlags::MASTER), file_flag(f, sre::TESFile_fileFlags::ACTIVE), file_flag(f, sre::TESFile_fileFlags::LIGHT));
    let flags = format!("{}{}{}", if master { "M" } else { "" }, if active { "A" } else { "" }, if is_light { "L" } else { "" });
    let index_ok = own == index && (light == f.is_light()) && is_light == f.is_light() && master == f.is_master() && (!light || f.compileIndex == 0xFE);
    format!("{index}:{}:{flags}{}", file_name(f), if index_ok { "" } else { ":BADINDEX" })
}

fn file(handler: &'static TESDataHandler, name: &str) -> Result<&'static TESFile, String> {
    handler.file(name).ok_or_else(|| format!("{name} not found"))
}

fn get_file() -> Result<String, String> {
    let handler = handler()?;
    let curios = file(handler, "ccBGSSSE037-Curios.esl")?;
    check(curios.is_light() && curios.compileIndex == 0xFE, &format!("Curios has compile index {:#x}", { curios.compileIndex }))?;
    let curios_id = curios.runtime_form_id(0x800).ok_or("Curios is not loaded")?;
    let curios_expected = 0xFE00_0800 | u32::from(curios.lightFileIndex) << 12;
    check(curios_id == curios_expected, &format!("Curios 0x800 resolved to {curios_id:08X}, light index {}", { curios.lightFileIndex }))?;
    let fish = file(handler, "ccBGSSSE001-Fish.esm")?;
    let fish_id = fish.runtime_form_id(0x800).ok_or("Fish is not loaded")?;
    let fish_expected = u32::from(fish.compileIndex) << 24 | 0x800;
    check(!fish.is_light() && fish_id == fish_expected, &format!("Fish 0x800 resolved to {fish_id:08X}, compile index {}", { fish.compileIndex }))?;
    check(handler.file("NoSuchFile.esp").is_none(), "NoSuchFile.esp was found")?;
    check(handler.file("skyrim.esm").is_some_and(|f| file_name(f) == "Skyrim.esm"), "GetFile is not case-insensitive")?;
    check(sre::find_form_by_id(curios_id).is_some_and(|f| f.formID == curios_id), "0xFE001800 does not resolve to a form")?;
    let sword_id = handler.file("Skyrim.esm").and_then(|f| f.runtime_form_id(IRON_SWORD & 0x00FF_FFFF)).ok_or("Skyrim.esm is not loaded")?;
    check(sword_id == IRON_SWORD, &format!("Skyrim.esm 0x12eb7 resolved to {sword_id:08X}"))?;
    Ok(format!("Skyrim.esm 0x12eb7 -> {sword_id:08X}, Curios 0x800 -> {curios_id:08X}, Fish 0x800 -> {fish_id:08X}, NoSuchFile.esp -> none, skyrim.esm -> Skyrim.esm"))
}

fn form_list() -> Result<String, String> {
    let handler = handler()?;
    let list = handler
        .forms_of_type(FormType::FLST)
        .iter()
        .flatten()
        .filter_map(|f| f.cast::<BGSListForm>())
        .find(|l| l.forms.size != 0)
        .ok_or("no form list has entries")?;
    let len = list.get_forms_size();
    check(len == list.forms.size, "GetFormsSize differs from the forms array")?;
    let first = unsafe { (*list.forms.data.cast::<*const TESForm>()).as_ref() }.ok_or("the first entry is null")?;
    check(list.get_at(0).is_some_and(|f| core::ptr::eq(f, first)), "GetAt(0) is not the first entry")?;
    check(list.find(first) == 0, "Find(first) is not 0")?;
    check(list.find(form(IRON_SWORD)?) < 0 || (0..len).any(|i| list.get_at(i).is_some_and(|f| f.formID == IRON_SWORD)), "Find found a form that is not in the list")?;
    check(list.get_at(len).is_none(), "GetAt(len) returned a form")?;
    Ok(format!("{} has {len} entries, first {}: GetAt(0) and Find agree", describe(list), describe(first)))
}

/// Generated methods, each against a second route to the same answer: a free function and a
/// global, an associated function and the actor value table, `&self` methods and the layout,
/// vtable slots and the fields they read, a base accessor and a method reached through `Deref`
/// against the non-virtual function behind the slot, and a singleton against the static object.
fn generated_api() -> Result<String, String> {
    let player = form(PLAYER)?;
    let sword = form(IRON_SWORD)?;
    let global = skyrim::reloc().address(globals::g_PlayerCharacter_ptr).map_err(|e| e.to_string())?;
    check(core::ptr::eq(player, unsafe { *(global as *const *const TESForm) }), "sre::find_form_by_id(0x14) is not the object g_PlayerCharacter_ptr holds")?;
    check(sre::find_form_by_id(0x7FFF_FFFF).is_none(), "sre::find_form_by_id(0x7fffffff) is Some")?;

    let info = sre::ActorValueList::get_actor_value_info(ActorValue::Health as u32).ok_or("ActorValueList::get_actor_value_info(Health) is None")?;
    let table = sre::ActorValueList::singleton().and_then(|l| l.infos()[ActorValue::Health as usize]);
    check(table.is_some_and(|t| core::ptr::eq(t, info)), "ActorValueList::get_actor_value_info(Health) differs from the table entry")?;

    let component = sword.get_keyword_form().ok_or("TESForm::get_keyword_form is None")?;
    check(address(component) == address(sword) + 0x140, "TESForm::get_keyword_form differs from the weapon's component at +0x140")?;

    let keyword = form(VENDOR_ITEM_WEAPON)?;
    let name = sre_editor_id(keyword).ok_or("slot 50 get_form_editor_id is null")?;
    check(keyword.cast::<BGSKeyword>().is_some_and(|k| keyword_field(k).as_c_str() == Some(name)), "slot 50 get_form_editor_id differs from BGSKeyword.keyword")?;
    check(sword.get_form_type() == sword.formType, "slot 21 get_form_type differs from formType")?;

    let pc = sre::g_player_character_ptr().ok_or("sre::g_player_character_ptr is None")?;
    check(address(pc) == address(player), "sre::g_player_character_ptr is not the player")?;
    check(pc.get_form_type() == player.formType, "PlayerCharacter -> TESForm slot 21 differs from formType")?;
    let owner = pc.as_actor_value_owner();
    check(address(owner) == address(pc) + 0xB8, "Actor::as_actor_value_owner is not at +0xB8")?;
    let health = owner.get_actor_value(ActorValue::Health as i32);
    let direct = owner.actor_value_owner_get_actor_value(ActorValue::Health as i32);
    check(health > 0.0 && health.to_bits() == direct.to_bits(), "ActorValueOwner slot 1 differs from Actor::ActorValueOwner_GetActorValue")?;

    let handler = TESDataHandler::singleton().ok_or("TESDataHandler::singleton is None")?;
    check(core::ptr::eq(handler, sre::g_tes_data_handler()), "TESDataHandler::singleton differs from the static g_TESDataHandler")?;
    let file = unsafe { handler.get_file(c"skyrim.esm".as_ptr()) };
    check(handler.files().next().is_some_and(|first| core::ptr::eq(first, file)), "TESDataHandler::get_file differs from the head of the file list")?;
    Ok(format!(
        "find_form_by_id(0x14)={:#x}, ActorValueList::get_actor_value_info(Health)={:#x}, get_keyword_form, slot 50 \"{}\", slot 21 type {}, player health {health} through Deref and as_actor_value_owner, TESDataHandler::singleton().get_file(skyrim.esm)={file:p}: 12 comparisons equal",
        address(player),
        address(info),
        name.to_string_lossy(),
        sword.formType
    ))
}

fn sre_editor_id(form: &TESForm) -> Option<&CStr> {
    let text = form.get_form_editor_id();
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) })
}

fn next_form_id() -> Result<String, String> {
    let next = handler()?.nextFormID;
    check(next >> 24 == 0xFF, &format!("nextFormID {next:08X} is not in the 0xFF range"))?;
    Ok(format!("nextFormID={next:08X}"))
}
