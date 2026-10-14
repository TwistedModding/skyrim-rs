//! One test per `skyrim::audio` API, against the RE repo's `docs/subsystems/audio-verify.md`.
//! After a load `start` runs the suite's sequence on a thread of its own, since audio answers on
//! the audio thread with no frame guarantee: the `UISkillIncrease` marker played on the player
//! and stopped, `UIMenuOKSD` built by editor id and left to end, a handle's volume; then it
//! samples `AudioCategoryUI`'s volume while the scenario's Papyrus calls change it, logging each
//! step as it ends. `TESTS` read the manager and the music after the load; `SAVE_TESTS` report
//! the sequence.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use skyrim::actors::PlayerCharacter;
use skyrim::audio::{AudioManager, RELEASE_WHEN_DONE, SoundCategory, SoundHandle};
use skyrim::forms::FormId;

use crate::{Test, check, form};

const UI_SKILL_INCREASE: FormId = FormId(0x18538);
const AUDIO_CATEGORY_UI: FormId = FormId(0x64451);
const UI_MENU_OK: &str = "UIMenuOKSD";
const POLL: Duration = Duration::from_millis(20);

pub const TESTS: &[(&str, Test)] = &[("manager", manager)];
pub const SAVE_TESTS: &[(&str, Test)] = &[("marker", marker), ("one_shot", one_shot), ("handle_volume", handle_volume), ("category_samples", category_samples)];

#[derive(Clone, Debug, Default)]
struct Run {
    marker: Option<Result<String, String>>,
    one_shot: Option<Result<String, String>>,
    handle_volume: Option<Result<String, String>>,
    ids: Vec<u32>,
    samples: Vec<f32>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

fn update(f: impl FnOnce(&mut Run)) {
    if let Some(run) = RUN.lock().unwrap().as_mut() {
        f(run);
    }
}

/// Polls `test` until it holds or `limit` passes; the time it took.
fn wait_for(limit: Duration, test: impl Fn() -> bool) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if test() {
            return Some(start.elapsed());
        }
        std::thread::sleep(POLL);
    }
    None
}

fn play_marker() -> Result<String, String> {
    let player = PlayerCharacter::get().ok_or("no player")?;
    let mut handle = SoundHandle::from_form(UI_SKILL_INCREASE, RELEASE_WHEN_DONE).ok_or("BuildSoundHandleByFormId gave no handle")?;
    update(|r| r.ids.push(handle.id()));
    check(handle.set_position(player.position()), "SetPosition refused")?;
    if let Some(root) = player.third_person_root() {
        handle.follow(root);
    }
    check(handle.play(), "Play refused")?;
    let playing = wait_for(Duration::from_secs(2), || handle.is_valid() && handle.is_playing()).ok_or("never valid and playing")?;
    check(handle.stop(), "Stop refused")?;
    let probe = handle.clone();
    let gone = wait_for(Duration::from_secs(3), || !probe.is_valid() && !probe.is_playing()).ok_or("still valid 3 s after the stop")?;
    Ok(format!("id {} valid and playing after {} ms, invalid {} ms after the stop", handle.id(), playing.as_millis(), gone.as_millis()))
}

fn play_one_shot() -> Result<String, String> {
    let mut handle = SoundHandle::from_editor_id(UI_MENU_OK, RELEASE_WHEN_DONE).ok_or("BuildSoundHandleByEditorId gave no handle")?;
    update(|r| r.ids.push(handle.id()));
    check(handle.play(), "Play refused")?;
    let started = wait_for(Duration::from_secs(2), || handle.is_valid() && handle.is_playing()).ok_or("never playing")?;
    let ended = wait_for(Duration::from_secs(5), || !handle.is_valid()).ok_or("still valid 5 s after it started")?;
    skyrim::audio::play_ui_sound(UI_MENU_OK);
    Ok(format!("{UI_MENU_OK} id {} playing after {} ms, released by itself {} ms later; the UI path played it again", handle.id(), started.as_millis(), ended.as_millis()))
}

fn set_handle_volume() -> Result<String, String> {
    let mut handle = SoundHandle::from_form(UI_SKILL_INCREASE, RELEASE_WHEN_DONE).ok_or("no handle")?;
    update(|r| r.ids.push(handle.id()));
    check(handle.set_volume(0.5), "SetVolume refused")?;
    check(handle.play(), "Play refused")?;
    let probe = handle.clone();
    let took = wait_for(Duration::from_secs(2), || (probe.volume() - 0.5).abs() < 1e-3).ok_or_else(|| format!("volume reads {}", probe.volume()))?;
    handle.stop();
    Ok(format!("id {} volume 0.5 read back after {} ms", handle.id(), took.as_millis()))
}

/// Starts the suite's sequence on its own thread; after every load.
pub fn start() {
    *RUN.lock().unwrap() = Some(Run::default());
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        let marker = play_marker();
        log!("audio step marker: {marker:?}");
        update(|r| r.marker = Some(marker));
        let one_shot = play_one_shot();
        log!("audio step one_shot: {one_shot:?}");
        update(|r| r.one_shot = Some(one_shot));
        let handle_volume = set_handle_volume();
        log!("audio step handle_volume: {handle_volume:?}");
        update(|r| r.handle_volume = Some(handle_volume));
        let Ok(category) = form(AUDIO_CATEGORY_UI) else { return };
        let Some(category) = SoundCategory::from_form(category) else { return };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(30) {
            let volume = category.volume();
            update(|r| {
                if r.samples.last() != Some(&volume) {
                    r.samples.push(volume);
                }
            });
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

fn manager() -> Result<String, String> {
    let manager = AudioManager::get().ok_or("no audio manager")?;
    let master = manager.master_volume();
    check(master > 0.0 && master <= 1.0, &format!("master volume {master}"))?;
    let music = skyrim::audio::current_music().ok_or("no music type playing")?;
    check(music.form_type() == Some(skyrim::forms::FormType::MUSC), &format!("the current music is a {:?}", music.form_type()))?;
    Ok(format!("master volume {master}, music type {} playing", music.form_id()))
}

fn result(pick: fn(&Run) -> Option<Result<String, String>>, what: &str) -> Result<String, String> {
    let run = RUN.lock().unwrap().clone().ok_or("the sequence never started")?;
    pick(&run).ok_or_else(|| format!("{what} did not finish"))?
}

fn marker() -> Result<String, String> {
    result(|r| r.marker.clone(), "the marker")
}

fn one_shot() -> Result<String, String> {
    result(|r| r.one_shot.clone(), "the one-shot")
}

fn handle_volume() -> Result<String, String> {
    result(|r| r.handle_volume.clone(), "the handle volume")
}

fn category_samples() -> Result<String, String> {
    let run = RUN.lock().unwrap().clone().ok_or("the sequence never started")?;
    check(run.samples == [1.0, 0.5, 1.0], &format!("AudioCategoryUI volumes seen {:?}", run.samples))?;
    check(run.ids.windows(2).all(|w| w[0] < w[1]), &format!("ids {:?} do not grow", run.ids))?;
    Ok(format!("AudioCategoryUI volumes seen {:?} while the scenario set it; own ids {:?}, last {}", run.samples, run.ids, run.ids.last().copied().unwrap_or(0)))
}
