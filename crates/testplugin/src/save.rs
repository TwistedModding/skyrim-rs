//! The save suite through `skyrim::save` and the generated save classes, against the RE repo's `docs/subsystems/save-verify.md`.
//! `install` adds a sink on the save/load manager's event at `DataLoaded`: a finished save writes
//! the suite's record (the player's gold and three form ids: a regular plugin's, a light
//! plugin's, a created one) next to the save, a starting load drops the record read last; a
//! `TESLoadGameEvent` sink reads the loaded save's record after every load and maps its ids back.
//! `SAVE_TESTS` check the event sequence, the records and the remapping at the final save.

use std::path::PathBuf;
use std::sync::Mutex;

use skyrim::events::{BSTEventSource, Event, EventSink, Sink};
use skyrim::prelude::*;
use skyrim::save::SaveLoadEvent;
use sre::{BGSSaveLoadGame, BGSSaveLoadManager, BSEventNotifyControl, FormType, SaveLoadEventKind, TESDataHandler, TESLoadGameEvent};

use crate::{Test, check, file_name, form};

const GOLD: u32 = 0xF;
const CREATED: u32 = 0xFF00_0800;
const SAVE: &str = "skyrim-rs-save";
/// The save that ends the scenario; `SAVE_TESTS` run only at it.
pub const FINAL: &str = "skyrim-rs-save-2";

pub const SAVE_TESTS: &[(&str, Test)] = &[("events", events), ("records", records), ("remap", remap), ("saved_files", saved_files)];

/// What the suite keeps next to a save.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    gold: i32,
    ids: [u32; 3],
}

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WRITTEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOADED: Mutex<Option<Record>> = Mutex::new(None);
static LOADS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static REMAPS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn saves_dir() -> Option<PathBuf> {
    Some(skse::log::dir(0).ok()?.parent()?.join("Saves"))
}

fn record_path(name: &str) -> Option<PathBuf> {
    Some(saves_dir()?.join(format!("{name}.skyrimrs")))
}

/// The first keyword of a regular plugin past the base game and the first of a light plugin.
fn sample_ids() -> Option<[u32; 3]> {
    let keywords = TESDataHandler::singleton()?.forms_of_type(FormType::KYWD);
    let id_where = |f: fn(u32) -> bool| keywords.iter().flatten().map(|k| k.formID).find(|id| f(*id));
    Some([id_where(|id| (1..0xFE).contains(&(id >> 24)))?, id_where(|id| id >> 24 == 0xFE)?, CREATED])
}

fn player_gold() -> Result<i32, String> {
    let player = sre::PlayerCharacter::singleton().ok_or("no player")?;
    let gold = form(GOLD)?;
    Ok(player.get_inventory_changes().map_or(0, |c| c.get_item_count(gold)))
}

fn write_record(name: &str) -> Result<String, String> {
    let gold = player_gold()?;
    let ids = sample_ids().ok_or("no regular or light keyword")?;
    let path = record_path(name).ok_or("no saves folder")?;
    let text = format!("{gold} {:08X} {:08X} {:08X}\n", ids[0], ids[1], ids[2]);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("{name}: gold {gold}, ids {:08X} {:08X} {:08X}", ids[0], ids[1], ids[2]))
}

fn read_record(name: &str) -> Option<Record> {
    let text = std::fs::read_to_string(record_path(name)?).ok()?;
    let mut parts = text.split_whitespace();
    let gold = parts.next()?.parse().ok()?;
    let mut ids = [0; 3];
    for id in &mut ids {
        *id = u32::from_str_radix(parts.next()?, 16).ok()?;
    }
    Some(Record { gold, ids })
}

struct Events;

impl EventSink<SaveLoadEvent> for Events {
    fn process(&self, event: &SaveLoadEvent, _: &BSTEventSource<SaveLoadEvent>) -> BSEventNotifyControl {
        let name = current_save_name();
        EVENTS.lock().unwrap().push(format!("{}:{} {name}", event.kind_raw(), u8::from(event.done())));
        match (event.kind(), event.done()) {
            (Some(SaveLoadEventKind::save), true) => {
                let result = write_record(&name).unwrap_or_else(|e| format!("failed: {e}"));
                WRITTEN.lock().unwrap().push(result);
            }
            (Some(SaveLoadEventKind::load), false) => *LOADED.lock().unwrap() = None,
            _ => {}
        }
        BSEventNotifyControl::Continue
    }
}

static SINK: Sink<SaveLoadEvent, Events> = Sink::new(Events);

struct Loaded;

impl EventSink<TESLoadGameEvent> for Loaded {
    fn process(&self, _: &TESLoadGameEvent, _: &BSTEventSource<TESLoadGameEvent>) -> BSEventNotifyControl {
        on_load();
        BSEventNotifyControl::Continue
    }
}

static LOADED_SINK: Sink<TESLoadGameEvent, Loaded> = Sink::new(Loaded);

/// Adds the sinks on the manager's event and on `TESLoadGameEvent`; once at `DataLoaded`.
pub fn install() -> Result<String, String> {
    SaveLoadEvent::source().ok_or("no save/load manager")?.add_sink(&SINK).leak();
    TESLoadGameEvent::source().ok_or("no TESLoadGameEvent source")?.add_sink(&LOADED_SINK).leak();
    Ok(format!("sink added; records go to {}", saves_dir().map_or("?".to_owned(), |d| d.display().to_string())))
}

fn current_save_name() -> String {
    BGSSaveLoadManager::singleton().and_then(BGSSaveLoadManager::current_save_name).unwrap_or("").to_owned()
}

/// Reads the loaded save's record and maps its ids back.
fn on_load() {
    let name = current_save_name();
    let dropped = LOADED.lock().unwrap().is_none();
    let Some(record) = read_record(&name) else {
        LOADS.lock().unwrap().push(format!("{name}: no record (state dropped {dropped})"));
        return;
    };
    let mapped: Vec<String> = record.ids.iter().map(|id| format!("{id:08X}->{}", skyrim::save::remap_form_id(*id).map_or("gone".to_owned(), |m| format!("{m:08X}")))).collect();
    let same = record.ids.iter().all(|id| skyrim::save::remap_form_id(*id) == Some(*id) && (id >> 24 == 0xFF || sre::find_form_by_id(*id).is_some()));
    REMAPS.lock().unwrap().push(format!("{name}: {} identical {same}", mapped.join(" ")));
    LOADS.lock().unwrap().push(format!("{name}: record gold {} (state dropped {dropped})", record.gold));
    *LOADED.lock().unwrap() = Some(record);
}

fn events() -> Result<String, String> {
    let seen = EVENTS.lock().unwrap().clone();
    let kinds: Vec<&str> = seen.iter().map(|e| e.split(' ').next().unwrap_or("")).collect();
    let expected = ["2:0", "2:1", "0:0", "0:1", "1:0", "1:1", "2:0", "2:1", "2:0", "2:1", "1:0"];
    check(kinds == expected, &format!("events {seen:?}"))?;
    check(seen[5] == format!("1:1 {SAVE}") && seen[10] == format!("1:0 {SAVE}"), &format!("save names {seen:?}"))?;
    Ok(format!("{seen:?}"))
}

fn records() -> Result<String, String> {
    let written = WRITTEN.lock().unwrap().clone();
    let loads = LOADS.lock().unwrap().clone();
    check(written.len() == 1 && written[0].starts_with(&format!("{SAVE}: gold 145")), &format!("written {written:?}"))?;
    check(loads.len() == 3, &format!("loads {loads:?}"))?;
    check(loads[0] == "retrace-base: no record (state dropped true)" && loads[1] == "retrace-base: no record (state dropped true)", &format!("loads {loads:?}"))?;
    check(loads[2] == format!("{SAVE}: record gold 145 (state dropped true)"), &format!("loads {loads:?}"))?;
    let gold = player_gold()?;
    check(gold == 145, &format!("the player holds {gold} gold"))?;
    Ok(format!("written {written:?}; loads {loads:?}; the player holds {gold} gold"))
}

fn remap() -> Result<String, String> {
    let remaps = REMAPS.lock().unwrap().clone();
    check(remaps.len() == 1 && remaps[0].ends_with("identical true"), &format!("remaps {remaps:?}"))?;
    let gone = skyrim::save::remap_form_id(0xFD00_0800);
    check(gone.is_none(), &format!("a saved index past the plugin list mapped to {gone:?}"))?;
    let handle = skyrim::save::remap_handle(0xFFFF_0000_0000_0014);
    check(handle == Some(0xFFFF_0000_0000_0014), &format!("the player's handle mapped to {handle:x?}"))?;
    Ok(format!("{remaps:?}; saved index 0xFD maps to nothing; the player's handle maps to itself"))
}

fn saved_files() -> Result<String, String> {
    let game = BGSSaveLoadGame::singleton().ok_or("no BGSSaveLoadGame")?;
    let files = game.saved_files();
    let light = game.saved_light_files();
    let first = files.first().copied().flatten().map(|f| file_name(f).to_owned());
    check(first.as_deref() == Some("Skyrim.esm") && files.iter().all(Option::is_some), &format!("saved files start with {first:?}, {} of {} present", files.iter().flatten().count(), files.len()))?;
    let handler = TESDataHandler::singleton().ok_or("no data handler")?;
    check(light.len() == handler.light_files().len(), &format!("{} saved light files, {} loaded", light.len(), handler.light_files().len()))?;
    Ok(format!("{} regular and {} light plugins in the save, all present, Skyrim.esm first", files.len(), light.len()))
}
