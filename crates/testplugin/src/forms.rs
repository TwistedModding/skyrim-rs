//! One test per `skyrim::forms` API; the expected values are those of the RE repo's
//! `docs/subsystems/forms-verify.md`.

use layout::v1_7_104::globals;
use skyrim::forms::{FormId, FormType, TESDataHandler, TESForm, lookup_form_by_editor_id};

use crate::{IRON_SWORD, PLAYER, Test, check, form};

const VENDOR_ITEM_WEAPON: FormId = FormId(0x8F958);

pub const TESTS: &[(&str, Test)] = &[
    ("form_id", form_id),
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
];

fn names(keywords: &[Option<&skyrim::forms::BGSKeyword>]) -> Vec<String> {
    keywords.iter().map(|k| k.and_then(|k| k.name()).map_or("null".into(), |n| n.to_string_lossy().into_owned())).collect()
}

fn form_id() -> Result<String, String> {
    let light = FormId(0xFE00_1800);
    let regular = FormId(0x0500_0800);
    let runtime = FormId(0xFF00_0CFA);
    check(light.is_light() && light.light_index() == Some(1) && light.local() == 0x800 && light.mod_index() == 0xFE, "light id helpers")?;
    check(!regular.is_light() && regular.light_index().is_none() && regular.local() == 0x800 && regular.mod_index() == 5, "regular id helpers")?;
    check(runtime.is_runtime() && !light.is_runtime() && !regular.is_runtime(), "runtime id helpers")?;
    check(format!("{light}") == "FE001800" && format!("{regular:?}") == "FormId(05000800)", "formatting")?;
    Ok(format!("{light} light index 1 local 0x800, {regular} mod index 5, {runtime} runtime"))
}

fn lookup_player() -> Result<String, String> {
    let player = form(PLAYER)?;
    let global = skyrim::reloc().address(globals::g_PlayerCharacter_ptr).map_err(|e| e.to_string())?;
    let held = unsafe { *(global as *const *const layout::v1_7_104::classes::TESForm) };
    check(std::ptr::eq(held, player.as_ptr()), "LookupFormByID(0x14) is not the object g_PlayerCharacter_ptr holds")?;
    check(player.form_id() == PLAYER, "formID is not 0x14")?;
    check(player.form_type() == Some(FormType::ACHR), "formType is not ACHR")?;
    check(player.source_files().is_empty(), "the player has source files")?;
    Ok(format!("{player:?} at {:p} = g_PlayerCharacter_ptr, flags {:#x}, no source files", player.as_ptr(), player.flags().bits()))
}

fn lookup_weapon() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    check(sword.form_id() == IRON_SWORD, "formID is not 0x12eb7")?;
    check(sword.form_type() == Some(FormType::WEAP), "formType is not WEAP")?;
    check(sword.editor_id().is_none(), "a weapon reported an editor id")?;
    Ok(format!("formID={} type={} ({}) editor_id=none", sword.form_id(), sword.form_type_raw(), FormType::WEAP.signature()))
}

fn lookup_missing() -> Result<String, String> {
    check(FormId(0x7FFF_FFFF).lookup().is_none(), "0x7fffffff resolved to a form")?;
    Ok("LookupFormByID(0x7fffffff) = null".into())
}

fn editor_id_keyword() -> Result<String, String> {
    let keyword = lookup_form_by_editor_id("VendorItemWeapon").ok_or("VendorItemWeapon did not resolve")?;
    check(keyword.form_id() == VENDOR_ITEM_WEAPON, "formID is not 0x8f958")?;
    check(keyword.form_type() == Some(FormType::KYWD), "formType is not KYWD")?;
    let via_slot = keyword.editor_id().map(|s| s.to_string_lossy().into_owned());
    check(via_slot.as_deref() == Some("VendorItemWeapon"), "GetFormEditorID did not return VendorItemWeapon")?;
    let via_field = keyword.as_keyword().and_then(|k| k.name()).map(|s| s.to_string_lossy().into_owned());
    check(via_field.as_deref() == Some("VendorItemWeapon"), "BGSKeyword.keyword is not VendorItemWeapon (null before a game is loaded)")?;
    check(lookup_form_by_editor_id("vendoritemweapon").map(TESForm::form_id) == Some(VENDOR_ITEM_WEAPON), "lookup is not case-insensitive")?;
    Ok(format!("formID={} type=4 (KYWD) editor_id=VendorItemWeapon keyword=VendorItemWeapon", keyword.form_id()))
}

fn editor_id_missing() -> Result<String, String> {
    for name in ["IronSword", "HousecarlWhiterun", "NoSuchEditorId"] {
        check(lookup_form_by_editor_id(name).is_none(), &format!("{name} resolved to a form"))?;
    }
    Ok("IronSword, HousecarlWhiterun, NoSuchEditorId = null".into())
}

fn editor_id_global() -> Result<String, String> {
    let hour = lookup_form_by_editor_id("GameHour").ok_or("GameHour did not resolve")?;
    check(hour.form_type() == Some(FormType::GLOB), "formType is not GLOB")?;
    let value = hour.as_global().ok_or("as_global returned None")?.value();
    Ok(format!("GameHour={value:.2} formID={}", hour.form_id()))
}

fn source_files() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let files: Vec<&str> = sword.source_files().iter().map(|f| f.map_or("null", |f| f.name())).collect();
    check(files == ["Skyrim.esm"], "IronSword's source files are not [Skyrim.esm]")?;
    Ok(format!("IronSword source files = {files:?}"))
}

fn downcast() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let npc = form(FormId(0x7))?;
    let player = form(PLAYER)?;
    check(sword.as_weapon().is_some() && sword.as_npc().is_none() && sword.as_keyword().is_none(), "weapon downcasts")?;
    check(npc.form_type() == Some(FormType::NPC_) && npc.as_npc().is_some() && npc.as_weapon().is_none(), "NPC downcasts")?;
    check(player.as_npc().is_none() && player.as_weapon().is_none() && player.as_form_list().is_none(), "player downcasts")?;
    check(std::ptr::eq(sword.as_weapon().unwrap().raw(), sword.as_ptr().cast()), "the wrapper moved the pointer")?;
    Ok(format!("0x12eb7 -> TESObjectWEAP, 0x7 -> TESNPC ({npc:?}), 0x14 -> neither"))
}

fn as_reference() -> Result<String, String> {
    let player = form(PLAYER)?;
    let sword = form(IRON_SWORD)?;
    let refr = player.as_reference().ok_or("the player is not a reference")?;
    check(std::ptr::eq(refr.raw(), player.as_ptr().cast()), "AsReference moved the player pointer")?;
    check(refr.form_id() == PLAYER, "the reference's formID is not 0x14")?;
    check(sword.as_reference().is_none(), "a weapon is a reference")?;
    Ok("player -> TESObjectREFR at the same address, IronSword -> none".into())
}

fn keywords() -> Result<String, String> {
    let sword = form(IRON_SWORD)?;
    let vendor = form(VENDOR_ITEM_WEAPON)?.as_keyword().ok_or("0x8f958 is not a keyword")?;
    let component = sword.keyword_form().ok_or("GetKeywordForm returned null")?;
    let offset = (std::ptr::from_ref(component) as usize).wrapping_sub(sword.as_ptr() as usize);
    check(offset == 0x140, "the keyword form is not at +0x140")?;
    let found = names(sword.keywords());
    check(found == ["WeapMaterialIron", "WeapTypeSword", "VendorItemWeapon"], &format!("keywords are {found:?}"))?;
    check(sword.has_keyword(vendor), "HasKeyword(VendorItemWeapon) is false")?;
    let armor = lookup_form_by_editor_id("VendorItemArmor").and_then(TESForm::as_keyword).ok_or("VendorItemArmor did not resolve")?;
    check(!sword.has_keyword(armor), "HasKeyword(VendorItemArmor) is true")?;
    check(form(PLAYER)?.keyword_form().is_none() && form(PLAYER)?.keywords().is_empty(), "the player has a keyword form")?;
    Ok(format!("IronSword keyword form at +0x140: {} has_keyword(VendorItemWeapon)=true has_keyword(VendorItemArmor)=false", found.join(",")))
}

fn data_handler() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let object = skyrim::reloc().address(globals::g_TESDataHandler).map_err(|e| e.to_string())?;
    check(object == std::ptr::from_ref(handler.raw()) as usize, "the pointer does not hold the static object")?;
    Ok(format!("TESDataHandler at {:#x} = g_TESDataHandler (base + {:#x})", object, object - skyrim::reloc().base()))
}

fn form_arrays() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let mut total = 0usize;
    let mut nulls = 0usize;
    let mut mismatched = 0usize;
    for ty in (0..=u8::MAX).filter_map(FormType::from_u8) {
        for entry in handler.forms_of_type(ty) {
            total += 1;
            match entry {
                None => nulls += 1,
                Some(f) if f.form_type() != Some(ty) => mismatched += 1,
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

fn files() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let first = handler.files().next().ok_or("the file list is empty")?;
    check(first.name() == "Skyrim.esm" && first.is_master() && first.is_active() && !first.is_light(), "the first file is not an active master Skyrim.esm")?;
    check(first.compile_index() == 0 && first.masters().is_empty(), "Skyrim.esm is not compile index 0 without masters")?;
    let listed = handler.files().count();
    let compiled: Vec<String> = handler.compiled_files().iter().enumerate().map(|(i, f)| describe(i, *f, false)).collect();
    let light: Vec<String> = handler.light_files().iter().enumerate().map(|(i, f)| describe(i, *f, true)).collect();
    let update = handler.files().nth(1).ok_or("no second file")?;
    let update_masters: Vec<&str> = update.masters().iter().map(|f| f.map_or("null", |f| f.name())).collect();
    check(update_masters == ["Skyrim.esm"], &format!("Update.esm's masters are {update_masters:?}"))?;
    Ok(format!("{listed} files listed; compiled=[{}] light=[{}]; Update.esm masters={update_masters:?}", compiled.join(","), light.join(",")))
}

fn describe(index: usize, file: Option<&skyrim::forms::TESFile>, light: bool) -> String {
    let Some(f) = file else { return format!("{index}:null") };
    let own = if light { usize::from(f.light_index()) } else { usize::from(f.compile_index()) };
    let flags = format!("{}{}{}", if f.is_master() { "M" } else { "" }, if f.is_active() { "A" } else { "" }, if f.is_light() { "L" } else { "" });
    let index_ok = own == index && (light == f.is_light()) && (!light || f.compile_index() == 0xFE);
    format!("{index}:{}:{flags}{}", f.name(), if index_ok { "" } else { ":BADINDEX" })
}

fn get_file() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let curios = handler.file("ccBGSSSE037-Curios.esl").ok_or("Curios not found")?;
    check(curios.is_light() && curios.compile_index() == 0xFE && curios.light_index() == 1, "Curios is not light index 1")?;
    let curios_id = curios.resolve_form_id(0x800).ok_or("Curios is not loaded")?;
    check(curios_id == FormId(0xFE00_1800), &format!("Curios 0x800 resolved to {curios_id}"))?;
    let fish = handler.file("ccBGSSSE001-Fish.esm").ok_or("Fish not found")?;
    let fish_id = fish.resolve_form_id(0x800).ok_or("Fish is not loaded")?;
    check(fish_id == FormId(0x0500_0800) && fish.compile_index() == 5, &format!("Fish 0x800 resolved to {fish_id}"))?;
    check(handler.file("NoSuchFile.esp").is_none(), "NoSuchFile.esp was found")?;
    check(handler.file("skyrim.esm").is_some_and(|f| f.name() == "Skyrim.esm"), "GetFile is not case-insensitive")?;
    check(curios_id.lookup().is_some_and(|f| f.form_id() == curios_id), "0xFE001800 does not resolve to a form")?;
    let sword_id = handler.file("Skyrim.esm").and_then(|f| f.resolve_form_id(IRON_SWORD.local())).ok_or("Skyrim.esm is not loaded")?;
    check(sword_id == IRON_SWORD, &format!("Skyrim.esm 0x12eb7 resolved to {sword_id}"))?;
    Ok(format!("Skyrim.esm 0x12eb7 -> {sword_id}, Curios 0x800 -> {curios_id}, Fish 0x800 -> {fish_id}, NoSuchFile.esp -> none, skyrim.esm -> Skyrim.esm"))
}

fn form_list() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let list = handler
        .forms_of_type(FormType::FLST)
        .iter()
        .flatten()
        .filter_map(|f| f.as_form_list())
        .find(|l| !l.forms().is_empty())
        .ok_or("no form list has entries")?;
    let entries = list.forms();
    check(usize::try_from(list.len()).ok() == Some(entries.len()), "GetFormsSize differs from the forms array")?;
    let first = entries[0].ok_or("the first entry is null")?;
    check(list.get(0).is_some_and(|f| std::ptr::eq(f, first)), "GetAt(0) is not the first entry")?;
    check(list.find(first) == Some(0), "Find(first) is not 0")?;
    check(list.find(form(IRON_SWORD)?).is_none() || entries.iter().flatten().any(|f| f.form_id() == IRON_SWORD), "Find found a form that is not in the list")?;
    check(list.get(list.len()).is_none(), "GetAt(len) returned a form")?;
    Ok(format!("{list:?} has {} entries, first {first:?}: GetAt(0) and Find agree", list.len()))
}

fn next_form_id() -> Result<String, String> {
    let handler = TESDataHandler::get().ok_or("g_TESDataHandler_ptr is null")?;
    let next = handler.next_form_id();
    check(next.is_runtime(), &format!("nextFormID {next} is not in the 0xFF range"))?;
    Ok(format!("nextFormID={next}"))
}
