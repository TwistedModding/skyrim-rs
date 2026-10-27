//! Audio (subsystem pass 11): sound handles built from descriptors and played, placed, followed,
//! stopped and read back; the UI sound path; sound category volumes; the master volume and the
//! music type playing now.
//!
//! Audio runs on its own thread (`LibAudioUpdate`), outside the frame: every operation on a handle
//! posts a command the thread drains about once every two frames, so a sound starts, and a stopped
//! sound's handle turns invalid, some milliseconds later. Poll; there is no frame guarantee. Any
//! thread may build and post (the manager locks its own queues).

use core::ffi::c_char;
use std::ffi::CString;

use skre::raw::{fns, globals};

use crate::forms::{FormId, FormType, TESForm};
use crate::render::NiAVObject;

type BuildByFormIdFn = unsafe extern "C" fn(manager: *mut skre::BSAudioManager, out: *mut skre::SoundHandle, form_id: u32, flags: u32);
type BuildByEditorIdFn = unsafe extern "C" fn(manager: *mut skre::BSAudioManager, out: *mut skre::SoundHandle, editor_id: *const c_char, flags: u32);

/// `BGSMusicType` keeps its `BSIMusicType` interface here.
const MUSIC_TYPE_INTERFACE: usize = 0x20;

pub(crate) struct Fns {
    manager: fns::GetAudioManager_fn,
    music: *const *mut skre::BSMusicManager,
    no_music: usize,
    by_form_id: BuildByFormIdFn,
    by_editor_id: BuildByEditorIdFn,
    play_ui: fns::PlayUISoundOrMusicByEditorId_fn,
    set_category_volume: fns::SetCategoryVolume_fn,
    play: fns::SoundHandle__Play_fn,
    stop: fns::SoundHandle__Stop_fn,
    release: fns::SoundHandle__Release_fn,
    is_valid: fns::SoundHandle__IsValid_fn,
    is_playing: fns::SoundHandle__IsPlaying_fn,
    set_position: fns::SoundHandle__SetPosition_fn,
    follow_node: fns::SoundHandle__FollowNode_fn,
    set_volume: fns::SoundHandle__SetVolume_fn,
    volume: fns::SoundHandle__GetVolume_fn,
}

unsafe impl Sync for Fns {}
unsafe impl Send for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                manager: crate::resolve(reloc, fns::GetAudioManager)?,
                music: reloc.address(globals::g_BSMusicManager_ptr)? as *const *mut skre::BSMusicManager,
                no_music: reloc.address(globals::g_NoMusic)?,
                by_form_id: crate::resolve(reloc, fns::BSAudioManager__BuildSoundHandleByFormId)?,
                by_editor_id: crate::resolve(reloc, fns::BSAudioManager__BuildSoundHandleByEditorId)?,
                play_ui: crate::resolve(reloc, fns::PlayUISoundOrMusicByEditorId)?,
                set_category_volume: crate::resolve(reloc, fns::SetCategoryVolume)?,
                play: crate::resolve(reloc, fns::SoundHandle__Play)?,
                stop: crate::resolve(reloc, fns::SoundHandle__Stop)?,
                release: crate::resolve(reloc, fns::SoundHandle__Release)?,
                is_valid: crate::resolve(reloc, fns::SoundHandle__IsValid)?,
                is_playing: crate::resolve(reloc, fns::SoundHandle__IsPlaying)?,
                set_position: crate::resolve(reloc, fns::SoundHandle__SetPosition)?,
                follow_node: crate::resolve(reloc, fns::SoundHandle__FollowNode)?,
                set_volume: crate::resolve(reloc, fns::SoundHandle__SetVolume)?,
                volume: crate::resolve(reloc, fns::SoundHandle__GetVolume)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().audio
}

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no interior nul left")
}

/// The audio manager (`BSAudioManager`, 0x190).
#[repr(transparent)]
pub struct AudioManager(skre::BSAudioManager);

impl AudioManager {
    /// `None` before the audio backend is created.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (fns().manager)().cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &skre::BSAudioManager {
        &self.0
    }

    fn as_ptr(&self) -> *mut skre::BSAudioManager {
        (&raw const self.0).cast_mut()
    }

    /// The master volume every sound's gain is multiplied by (linear).
    #[must_use]
    pub fn master_volume(&self) -> f32 {
        self.0.masterVolume
    }
}

/// Released by the audio thread once the sound has played and stopped (the end of a one-shot, or a
/// stop); the handle then reads invalid. Papyrus `Sound.Play` and the UI path build with it.
pub const RELEASE_WHEN_DONE: u32 = 0x10;

/// A sound (`SoundHandle`, 12 bytes): the sound's id and the state last requested. It holds no
/// pointer; every call looks the id up or posts a command, so it may be cloned and kept. A sound
/// built without [`RELEASE_WHEN_DONE`] stays until [`SoundHandle::release`].
#[repr(transparent)]
pub struct SoundHandle(skre::SoundHandle);

impl Clone for SoundHandle {
    fn clone(&self) -> Self {
        Self(skre::SoundHandle { id: self.0.id, assumeState: self.0.assumeState, unknown_5: self.0.unknown_5, requestedState: self.0.requestedState })
    }
}

impl SoundHandle {
    const fn invalid() -> Self {
        Self(skre::SoundHandle { id: u32::MAX, assumeState: 0, unknown_5: [0; 3], requestedState: 0 })
    }

    /// The handle of an id a Papyrus `Sound.Play` returned or a handle reported.
    #[must_use]
    pub const fn from_id(id: u32) -> Self {
        Self(skre::SoundHandle { id, assumeState: 0, unknown_5: [0; 3], requestedState: 0 })
    }

    fn built(handle: Self) -> Option<Self> {
        (handle.0.id != u32::MAX).then_some(handle)
    }

    /// Builds a sound from a sound descriptor (SNDR) or sound marker (SOUN) form id
    /// (`BuildSoundHandleByFormId`); `None` when the form is neither.
    #[must_use]
    pub fn from_form(form: FormId, flags: u32) -> Option<Self> {
        let manager = AudioManager::get()?;
        let mut handle = Self::invalid();
        unsafe { (fns().by_form_id)(manager.as_ptr(), &raw mut handle.0, form.0, flags) };
        Self::built(handle)
    }

    /// Builds a sound from a descriptor's editor id (`BuildSoundHandleByEditorId`).
    #[must_use]
    pub fn from_editor_id(editor_id: &str, flags: u32) -> Option<Self> {
        let manager = AudioManager::get()?;
        let editor_id = c_string(editor_id);
        let mut handle = Self::invalid();
        unsafe { (fns().by_editor_id)(manager.as_ptr(), &raw mut handle.0, editor_id.as_ptr(), flags) };
        Self::built(handle)
    }

    #[must_use]
    pub fn raw(&self) -> &skre::SoundHandle {
        &self.0
    }

    /// The sound's id, the value Papyrus `Sound.Play` returns; ids only grow.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.id
    }

    pub fn play(&mut self) -> bool {
        unsafe { (fns().play)(&raw mut self.0) }
    }

    pub fn stop(&mut self) -> bool {
        unsafe { (fns().stop)(&raw mut self.0) }
    }

    /// Lets the audio thread free the sound.
    pub fn release(&mut self) -> bool {
        unsafe { (fns().release)(&raw mut self.0) }
    }

    /// Places the sound at a world point.
    pub fn set_position(&mut self, position: [f32; 3]) -> bool {
        unsafe { (fns().set_position)(&raw mut self.0, position[0], position[1], position[2]) }
    }

    /// Moves the sound with `node`; it fades out and is released if the node goes away.
    pub fn follow(&mut self, node: &NiAVObject) {
        unsafe { (fns().follow_node)(&raw mut self.0, node.as_ptr().cast_mut()) }
    }

    /// The handle's own volume, multiplied with its category's and the master volume.
    pub fn set_volume(&mut self, volume: f32) -> bool {
        unsafe { (fns().set_volume)(&raw mut self.0, volume) }
    }

    #[must_use]
    pub fn volume(&self) -> f32 {
        unsafe { (fns().volume)(&raw const self.0) }
    }

    /// The audio thread still knows the sound.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        unsafe { (fns().is_valid)(&raw const self.0) }
    }

    /// The sound's voice runs and is not paused, as of the audio thread's last update.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        unsafe { (fns().is_playing)(&raw const self.0) }
    }
}

impl core::fmt::Debug for SoundHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SoundHandle({})", self.0.id)
    }
}

/// Plays a sound descriptor or music type by editor id through the UI output model, with no
/// position (`PlayUISoundOrMusicByEditorId`, the path behind a HUD notification's sound).
pub fn play_ui_sound(editor_id: &str) {
    let editor_id = c_string(editor_id);
    unsafe { (fns().play_ui)(editor_id.as_ptr()) }
}

/// A sound category (`BGSSoundCategory`, SNCT, 0x58).
#[repr(transparent)]
pub struct SoundCategory(skre::BGSSoundCategory);

impl SoundCategory {
    /// The form as a category, `None` for any other form type.
    #[must_use]
    pub fn from_form(form: &TESForm) -> Option<&Self> {
        (form.form_type() == Some(FormType::SNCT)).then(|| unsafe { &*core::ptr::from_ref(form).cast::<Self>() })
    }

    #[must_use]
    pub fn raw(&self) -> &skre::BGSSoundCategory {
        &self.0
    }

    /// The category's own volume (+0x50), what [`SoundCategory::set_volume`] sets.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.0.volume
    }

    /// The volume the plugin data gives it, 0 to 65535.
    #[must_use]
    pub fn static_volume(&self) -> u16 {
        self.0.staticVolume
    }

    /// Sets the volume and recomputes every sound (`SetCategoryVolume`, what Papyrus
    /// `SoundCategory.SetVolume` calls). The engine takes the address of a variable holding the
    /// category's interface, not the interface itself.
    pub fn set_volume(&self, volume: f32) {
        let mut interface = (&raw const self.0.categoryInterface).cast_mut();
        unsafe { (fns().set_category_volume)(&raw mut interface, volume) }
    }
}

/// The music type playing now (`BGSMusicType`, MUSC), `None` while nothing plays.
#[must_use]
pub fn current_music() -> Option<&'static TESForm> {
    let manager = unsafe { (*fns().music).as_ref() }?;
    let current = manager.current as usize;
    if current == 0 || current == fns().no_music {
        return None;
    }
    unsafe { TESForm::from_ptr((current - MUSIC_TYPE_INTERFACE) as *const skre::TESForm) }
}
