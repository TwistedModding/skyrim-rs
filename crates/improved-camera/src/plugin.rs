//! `plugin.cpp`'s mod detection (`Info().MOD`): the forms of other mods IC reacts to, looked up
//! once the data is loaded. A mod that is not installed leaves its entry empty.

use std::sync::Mutex;

use skyrim::prelude::*;
use sre::{EventSourceHolder_User, InputDevice, TESDataHandler, TESForm, TESGlobal, TESQuest};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModInfo {
    /// Runtime form ids of the mod races IC tags (`Undeath.esp`, `halfdragonrace.esp`,
    /// `Triumvirate - Mage Archetypes.esp`).
    pub lich_race: Option<u32>,
    pub half_dragon_race: Option<u32>,
    pub half_dragon_vampire_race: Option<u32>,
    pub hornedlord_race: Option<u32>,
    pub deer_race: Option<u32>,
    /// First Person Interactions' `FP_AnimTrigger` global.
    pub interaction: Option<u32>,
    /// Simple Fishing Overhaul's `AnimatedFishing_Global`.
    pub fishing: Option<u32>,
    /// Dragonborn's boat navmesh cutter reference, enabled while the boat is out.
    pub boat_navmesh_cutter: Option<u32>,
    /// The intro quest (`Skyrim.esm` 0x3372B, the cart ride); none under Enderal.
    pub quest_intro: Option<u32>,
    /// The magic effects of the transitions IC follows: Dawnguard's vampire lord, Skyrim's
    /// werewolf, `Undeath.esp`'s lich, and `Bathing in Skyrim.esp`'s bathing.
    pub vampirelord_transition: Option<u32>,
    pub werewolf_transition: Option<u32>,
    pub lich_transition: Option<u32>,
    pub bathe: Option<u32>,
    /// The Attack Behavior Framework's `bfcoTG_BaseStyle` global; that mod lets weapons be drawn in water.
    pub bfco: Option<u32>,
    /// The keyboard key the game's control map gives "Toggle POV" (`KEYBINDS.keyboardPOV`).
    pub keyboard_pov: Option<u32>,
}

static INFO: Mutex<Option<ModInfo>> = Mutex::new(None);

#[must_use]
pub fn info() -> ModInfo {
    INFO.lock().unwrap_or_else(std::sync::PoisonError::into_inner).unwrap_or_default()
}

fn lookup(handler: &TESDataHandler, local: u32, file: &str) -> Option<u32> {
    handler.file(file)?.runtime_form_id(local)
}

fn global(editor_id: &str) -> Option<u32> {
    skyrim::forms::form_by_editor_id(editor_id)?.cast::<TESGlobal>().map(|g| g.formID)
}

/// Looks every form up again; run at `DATA_LOADED`.
pub fn detect_mods() {
    let Some(handler) = TESDataHandler::singleton() else { return };
    *INFO.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ModInfo {
        lich_race: lookup(handler, 0x0001_772A, "Undeath.esp"),
        half_dragon_race: lookup(handler, 0x0001_25AA, "halfdragonrace.esp"),
        half_dragon_vampire_race: lookup(handler, 0x0001_3B4C, "halfdragonrace.esp"),
        hornedlord_race: lookup(handler, 0x837, "Triumvirate - Mage Archetypes.esp"),
        deer_race: lookup(handler, 0x0029_DA06, "Triumvirate - Mage Archetypes.esp"),
        interaction: global("FP_AnimTrigger"),
        fishing: global("AnimatedFishing_Global"),
        boat_navmesh_cutter: lookup(handler, 0x0003_CA82, "Dragonborn.esm"),
        quest_intro: lookup(handler, 0x0003_372B, "Skyrim.esm").filter(|_| global("_00E_VideoSkip").is_none()),
        vampirelord_transition: lookup(handler, 0x283C, "Dawnguard.esm"),
        werewolf_transition: lookup(handler, 0x0009_2C45, "Skyrim.esm"),
        lich_transition: lookup(handler, 0xA443, "Undeath.esp"),
        bathe: lookup(handler, 0x2C, "Bathing in Skyrim.esp"),
        bfco: global("bfcoTG_BaseStyle"),
        keyboard_pov: EventSourceHolder_User::singleton().and_then(|map| map.mapped_key("Toggle POV", InputDevice::Keyboard)),
    });
}

/// The stage of a quest found by [`detect_mods`] (`TESQuest`+0x228, the highest stage set).
#[must_use]
pub fn quest_stage(id: Option<u32>) -> Option<u16> {
    sre::find_form_by_id(id?)?.cast::<TESQuest>().map(TESQuest::get_current_stage)
}

/// The value of a global found by [`detect_mods`].
#[must_use]
pub fn global_value(id: Option<u32>) -> Option<f32> {
    sre::find_form_by_id(id?)?.cast::<TESGlobal>().map(|g| g.value)
}

/// Whether a reference found by [`detect_mods`] exists and is disabled.
#[must_use]
pub fn is_disabled(id: Option<u32>) -> Option<bool> {
    let form: &TESForm = sre::find_form_by_id(id?)?;
    Some(sre::TESForm_formFlags(form.formFlags).contains(sre::TESForm_formFlags::DISABLED))
}
