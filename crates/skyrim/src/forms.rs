//! Forms (subsystem pass 1): what the generated `TESForm`, `TESDataHandler` and `TESFile` methods
//! leave `unsafe`. Editor ids come back as `&CStr`, the engine's pointer arrays as slices, a lookup
//! takes a `&str`, and [`TESFormExt::cast`] checks a form's class before viewing it as one.
//!
//! Read on the game's main thread (SKSE message handlers, hooks), where the engine reads the data
//! handler's arrays and the form fields without a lock. References stay valid while the game keeps
//! the form registered: forms read from files are never freed, runtime-created ones (`0xFF` ids) can be.

use core::ffi::CStr;
use std::ffi::CString;

use sre::{Actor, BGSArtObject, BGSKeyword, BGSListForm, BGSLocation, BGSSoundCategory, Character, EffectSetting, FormType, PlayerCharacter, TESClass, TESDataHandler, TESEffectShader, TESFile, TESForm, TESFurniture, TESGlobal, TESNPC, TESObjectCELL, TESObjectLIGH, TESObjectREFR, TESObjectWEAP, TESQuest, TESRace, TESWeather, TESWorldSpace, VMTypeID};

use crate::ptr_slice;

/// The form registered under an editor id, through `FindFormByEditorID` (id 14620).
///
/// On this runtime only the classes that keep their editor id register one (keywords, globals,
/// cells, quests, races, worldspaces, topics, idles and a few more); weapons, armor, NPCs and
/// references do not, so they resolve to `None` here. Case-insensitive.
#[must_use]
pub fn form_by_editor_id(editor_id: &str) -> Option<&'static TESForm> {
    let name = CString::new(editor_id).ok()?;
    unsafe { sre::find_form_by_editor_id(name.as_ptr()).as_ref() }
}

/// A class a form can be checked and viewed as ([`TESFormExt::cast`]), and the script type a
/// Papyrus native declares for it.
///
/// # Safety
/// `is` must return true only for a form whose object is a `Self`.
pub unsafe trait FormClass: 'static {
    const TYPE_ID: VMTypeID;
    fn is(form: &TESForm) -> bool;

    /// The form this object is.
    fn as_form(&self) -> &TESForm {
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}

unsafe impl FormClass for TESForm {
    const TYPE_ID: VMTypeID = VMTypeID::Form;
    fn is(_: &TESForm) -> bool {
        true
    }
}

/// Through vtable slot 44 `AsReference`, which a reference answers with itself; the slot's meaning
/// is rated medium in `forms.json`, the forms suite proves it on the player and a weapon.
unsafe impl FormClass for TESObjectREFR {
    const TYPE_ID: VMTypeID = VMTypeID::ObjectReference;
    fn is(form: &TESForm) -> bool {
        core::ptr::eq(unsafe { form.as_reference() }.cast::<TESForm>(), form)
    }
}

unsafe impl FormClass for Actor {
    const TYPE_ID: VMTypeID = VMTypeID::Actor;
    fn is(form: &TESForm) -> bool {
        form.formType == FormType::ACHR as u8
    }
}

/// By vtable: an actor whose class is `Character` or `PlayerCharacter`.
unsafe impl FormClass for Character {
    const TYPE_ID: VMTypeID = VMTypeID::Actor;
    fn is(form: &TESForm) -> bool {
        crate::class_of(form).is_some_and(|c| c.id == sre::id!(sre::raw::vtables::Character_vtable::ID) || c.id == sre::id!(sre::raw::vtables::PlayerCharacter_vtable::ID))
    }
}

unsafe impl FormClass for PlayerCharacter {
    const TYPE_ID: VMTypeID = VMTypeID::Actor;
    fn is(form: &TESForm) -> bool {
        crate::class_of(form).is_some_and(|c| c.id == sre::id!(sre::raw::vtables::PlayerCharacter_vtable::ID))
    }
}

macro_rules! by_form_type {
    ($($class:ident = $ty:ident / $id:ident),* $(,)?) => {$(
        unsafe impl FormClass for $class {
            const TYPE_ID: VMTypeID = VMTypeID::$id;
            fn is(form: &TESForm) -> bool {
                form.formType == FormType::$ty as u8
            }
        }
    )*};
}

by_form_type!(
    BGSKeyword = KYWD / Keyword,
    BGSListForm = FLST / FormList,
    TESGlobal = GLOB / GlobalVariable,
    TESObjectWEAP = WEAP / Weapon,
    TESFurniture = FURN / Furniture,
    TESQuest = QUST / Quest,
    EffectSetting = MGEF / MagicEffect,
    TESNPC = NPC_ / ActorBase,
    TESRace = RACE / Race,
    TESClass = CLAS / Class,
    TESObjectCELL = CELL / Cell,
    TESWorldSpace = WRLD / WorldSpace,
    BGSLocation = LCTN / Location,
    BGSSoundCategory = SNCT / SoundCategory,
    TESWeather = WTHR / Weather,
    TESObjectLIGH = LIGH / Light,
    TESEffectShader = EFSH / EffectShader,
    BGSArtObject = ARTO / Art,
);

pub trait TESFormExt {
    /// The form as a `T` when its class is one, so `form.cast::<sre::TESObjectWEAP>()` is a
    /// weapon or `None`.
    fn cast<T: FormClass>(&self) -> Option<&T>;
    /// The editor id through vtable slot 50 (`GetFormEditorID`); `None` when the class keeps none.
    fn editor_id(&self) -> Option<&CStr>;
    /// The keywords of the form's keyword component (`TESForm::GetKeywordForm`); empty for a class
    /// without one.
    fn keywords(&self) -> &[Option<&BGSKeyword>];
}

impl TESFormExt for TESForm {
    fn cast<T: FormClass>(&self) -> Option<&T> {
        T::is(self).then(|| unsafe { &*core::ptr::from_ref(self).cast::<T>() })
    }

    fn editor_id(&self) -> Option<&CStr> {
        let text = self.get_form_editor_id();
        if text.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(text) }).filter(|s| !s.is_empty())
    }

    fn keywords(&self) -> &[Option<&BGSKeyword>] {
        self.get_keyword_form().map_or(&[], |k| unsafe { ptr_slice(k.keywords, k.numKeywords) })
    }
}

pub trait TESDataHandlerExt {
    /// Every form of one type, in load order.
    fn forms_of_type(&self, ty: FormType) -> &[Option<&TESForm>];
    /// The files found in `Data`, masters first, loaded or not.
    fn files(&self) -> impl Iterator<Item = &TESFile>;
    /// The regular loaded files; the index is each file's compile index.
    fn compiled_files(&self) -> &[Option<&TESFile>];
    /// The loaded light files; the index is each file's light index.
    fn light_files(&self) -> &[Option<&TESFile>];
    /// A file by name through `TESDataHandler::GetFile` (id 13731), case-insensitive.
    fn file(&self, name: &str) -> Option<&TESFile>;
}

impl TESDataHandlerExt for TESDataHandler {
    fn forms_of_type(&self, ty: FormType) -> &[Option<&TESForm>] {
        let arrays = core::ptr::from_ref(self).cast::<sre::BSTArray>().wrapping_byte_add(sre::member_offset!("TESDataHandler.formArrays"));
        sre::fixed_size!("BSTArray", sre::BSTArray);
        let array = unsafe { &*arrays.add(ty as usize) };
        unsafe { ptr_slice(array.data, array.size) }
    }

    fn files(&self) -> impl Iterator<Item = &TESFile> {
        let mut node = Some(&self.files);
        core::iter::from_fn(move || {
            let n = node?;
            node = unsafe { n.next.cast::<sre::BSSimpleListNode>().as_ref() };
            unsafe { n.item.cast::<TESFile>().as_ref() }
        })
    }

    fn compiled_files(&self) -> &[Option<&TESFile>] {
        unsafe { ptr_slice(self.compiledFiles.data, self.compiledFiles.size) }
    }

    fn light_files(&self) -> &[Option<&TESFile>] {
        unsafe { ptr_slice(self.lightFiles.data, self.lightFiles.size) }
    }

    fn file(&self, name: &str) -> Option<&TESFile> {
        let name = CString::new(name).ok()?;
        unsafe { self.get_file(name.as_ptr()).as_ref() }
    }
}

pub trait TESFileExt {
    /// A file-relative id as the runtime form id through `TESFile::ResolveFormID` (id 14003);
    /// `None` when the file is not loaded.
    fn runtime_form_id(&self, local: u32) -> Option<u32>;
    /// The file's name (`fileName`, +0x58).
    fn name(&self) -> &str;
}

/// A runtime form id without its load-order prefix: the low 12 bits for a light plugin's (`FE`)
/// form, the low 24 otherwise. With the form's file (`TESForm::GetFile(0)`) it names the form
/// across load orders, as the engine's own `GetFormFromFile` takes it (RE `camera.md` IC8 K).
#[must_use]
pub const fn local_form_id(form_id: u32) -> u32 {
    if form_id >> 24 == 0xFE { form_id & 0xFFF } else { form_id & 0x00FF_FFFF }
}

impl TESFileExt for TESFile {
    fn name(&self) -> &str {
        let bytes = &self.fileName;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        core::str::from_utf8(&bytes[..end]).unwrap_or_default()
    }

    fn runtime_form_id(&self, local: u32) -> Option<u32> {
        if self.compileIndex == 0xFF {
            return None;
        }
        let mut id = local;
        unsafe { self.resolve_form_id(&raw mut id) };
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::local_form_id;

    #[test]
    fn a_local_id_drops_the_load_order_prefix() {
        assert_eq!(local_form_id(0x0000_081A), 0x81A);
        assert_eq!(local_form_id(0x0A01_2345), 0x01_2345);
        assert_eq!(local_form_id(0xFE01_2805), 0x805);
    }
}
