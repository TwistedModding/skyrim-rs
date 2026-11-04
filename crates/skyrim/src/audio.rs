//! Audio (subsystem pass 11): sound handles built from a form id or an editor id, the UI sound
//! path, a category's volume and the music type playing now; the generated `SoundHandle` plays,
//! places, follows, stops and reads back a built sound.
//!
//! Audio runs on its own thread (`LibAudioUpdate`), outside the frame: every operation on a handle
//! posts a command the thread drains about once every two frames, so a sound starts, and a stopped
//! sound's handle turns invalid, some milliseconds later. Poll; there is no frame guarantee. Any
//! thread may build and post (the manager locks its own queues).

use std::ffi::CString;

use sre::{BGSSoundCategory, SoundHandle, TESForm};


/// Released by the audio thread once the sound has played and stopped (the end of a one-shot, or a
/// stop); the handle then reads invalid. Papyrus `Sound.Play` and the UI path build with it.
pub const RELEASE_WHEN_DONE: u32 = 0x10;

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no interior nul left")
}

const fn invalid() -> SoundHandle {
    SoundHandle { id: u32::MAX, assumeState: 0, unknown_5: [0; 3], requestedState: 0, __engine: sre::Engine::new() }
}

fn built(handle: SoundHandle) -> Option<SoundHandle> {
    (handle.id != u32::MAX).then_some(handle)
}

/// Builds a sound from a sound descriptor (SNDR) or sound marker (SOUN) form id
/// (`BuildSoundHandleByFormId`); `None` when the form is neither. A sound built without
/// [`RELEASE_WHEN_DONE`] stays until `SoundHandle::release`.
#[must_use]
pub fn sound_from_form(form_id: u32, flags: u32) -> Option<SoundHandle> {
    let manager = sre::get_audio_manager()?;
    let mut handle = invalid();
    unsafe { manager.build_sound_handle_by_form_id(&raw mut handle, form_id, flags) };
    built(handle)
}

/// Builds a sound from a descriptor's editor id (`BuildSoundHandleByEditorId`).
#[must_use]
pub fn sound_from_editor_id(editor_id: &str, flags: u32) -> Option<SoundHandle> {
    let manager = sre::get_audio_manager()?;
    let editor_id = c_string(editor_id);
    let mut handle = invalid();
    unsafe { manager.build_sound_handle_by_editor_id(&raw mut handle, editor_id.as_ptr(), flags) };
    built(handle)
}

/// Plays a sound descriptor or music type by editor id through the UI output model, with no
/// position (`PlayUISoundOrMusicByEditorId`, the path behind a HUD notification's sound).
pub fn play_ui_sound(editor_id: &str) {
    let editor_id = c_string(editor_id);
    unsafe { sre::play_ui_sound_or_music_by_editor_id(editor_id.as_ptr()) }
}

/// Sets a category's volume and recomputes every sound (`SetCategoryVolume`, what Papyrus
/// `SoundCategory.SetVolume` calls). The engine takes the address of a variable holding the
/// category's interface, not the interface itself.
pub fn set_category_volume(category: &BGSSoundCategory, volume: f32) {
    let mut interface = (&raw const category.categoryInterface).cast_mut();
    unsafe { sre::set_category_volume(&raw mut interface, volume) }
}

/// The music type playing now (`BGSMusicType`, MUSC), `None` while nothing plays.
#[must_use]
pub fn current_music() -> Option<&'static TESForm> {
    let current = sre::g_bs_music_manager_ptr()?.current as usize;
    if current == 0 || current == core::ptr::from_ref(sre::g_no_music()) as usize {
        return None;
    }
    unsafe { ((current - sre::base_offset!("BGSMusicType:BSIMusicType", 0x20)) as *const TESForm).as_ref() }
}
