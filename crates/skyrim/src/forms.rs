//! Forms and the data handler (subsystem pass 1): lookup by form id and editor id, the per-type
//! form arrays, plugin files, keywords and form lists.
//!
//! Every wrapper is `#[repr(transparent)]` over the generated layout struct, so a `&TESForm` is a
//! typed view of the live game object. The lookups take the game's own map locks; the data
//! handler's arrays and the form fields are read without locking, which is safe on the game's main
//! thread (SKSE message handlers, hooks) where the engine itself reads them. References returned
//! as `'static` stay valid for as long as the game keeps the form registered: record forms loaded
//! from files are never freed, runtime-created forms (references, 0xFF form ids) can be.
//!
//! Items rated below high confidence in `export/semantic/forms.json` say so in their docs.

use core::ffi::{CStr, c_void};
use std::ffi::CString;
use std::fmt;

use sre::raw::{fns, globals, vtables};

pub use crate::actors::TESObjectREFR;
pub use crate::string::BSFixedString;
pub use sre::{FormType, TESFile_fileFlags as FileFlags, TESForm_formFlags as FormFlags};

/// Light files resolve to `0xFE` in the mod index byte, runtime-created forms to `0xFF`.
pub const LIGHT_INDEX: u8 = 0xFE;
pub const RUNTIME_INDEX: u8 = 0xFF;

pub(crate) struct Fns {
    find_by_id: fns::FindFormByID_fn,
    find_by_editor_id: fns::FindFormByEditorID_fn,
    get_file: fns::TESDataHandler__GetFile_fn,
    resolve_form_id: fns::TESFile__ResolveFormID_fn,
    keyword_form: fns::TESForm__GetKeywordForm_fn,
    list_get_at: fns::BGSListForm__GetAt_fn,
    list_find: fns::BGSListForm__Find_fn,
    list_len: fns::BGSListForm__GetFormsSize_fn,
    data_handler: *const *const sre::TESDataHandler,
}

unsafe impl Send for Fns {}
unsafe impl Sync for Fns {}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                find_by_id: crate::resolve(reloc, fns::FindFormByID)?,
                find_by_editor_id: crate::resolve(reloc, fns::FindFormByEditorID)?,
                get_file: crate::resolve(reloc, fns::TESDataHandler__GetFile)?,
                resolve_form_id: crate::resolve(reloc, fns::TESFile__ResolveFormID)?,
                keyword_form: crate::resolve(reloc, fns::TESForm__GetKeywordForm)?,
                list_get_at: crate::resolve(reloc, fns::BGSListForm__GetAt)?,
                list_find: crate::resolve(reloc, fns::BGSListForm__Find)?,
                list_len: crate::resolve(reloc, fns::BGSListForm__GetFormsSize)?,
                data_handler: reloc.address(globals::g_TESDataHandler_ptr)? as *const *const sre::TESDataHandler,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().forms
}

/// `len` pointers at `data` as optional references.
///
/// # Safety
/// `data` must point to `len` pointers that stay readable and unchanged for `'a`, each null or
/// pointing to a live `T` (a `#[repr(transparent)]` wrapper of the pointee's layout).
pub(crate) unsafe fn ptr_slice<'a, T>(data: *const c_void, len: u32) -> &'a [Option<&'a T>] {
    if data.is_null() || len == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(data.cast::<Option<&'a T>>(), len as usize) }
}

/// # Safety
/// `arr` must hold pointers as [`ptr_slice`] requires.
unsafe fn array<T>(arr: &sre::BSTArray) -> &[Option<&T>] {
    unsafe { ptr_slice(arr.data, arr.size) }
}

/// A form id: mod index in the top byte, then the record's local id (12 bits behind a light index, 24 otherwise).
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormId(pub u32);

impl FormId {
    #[must_use]
    pub const fn mod_index(self) -> u8 {
        (self.0 >> 24) as u8
    }

    #[must_use]
    pub const fn is_light(self) -> bool {
        self.mod_index() == LIGHT_INDEX
    }

    #[must_use]
    pub const fn is_runtime(self) -> bool {
        self.mod_index() == RUNTIME_INDEX
    }

    /// The index among the light files, for ids in the `0xFE` range.
    #[must_use]
    pub const fn light_file_index(self) -> Option<u16> {
        if self.is_light() { Some(((self.0 >> 12) & 0xFFF) as u16) } else { None }
    }

    /// The id as written in the owning file: 12 bits behind a light index, 24 otherwise.
    #[must_use]
    pub const fn local(self) -> u32 {
        if self.is_light() { self.0 & 0xFFF } else { self.0 & 0x00FF_FFFF }
    }

    /// The registered form with this id, through `FindFormByID`.
    #[must_use]
    pub fn lookup(self) -> Option<&'static TESForm> {
        find_form_by_id(self)
    }
}

impl fmt::Debug for FormId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FormId({:08X})", self.0)
    }
}

impl fmt::Display for FormId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08X}", self.0)
    }
}

/// The registered form with this id, through `FindFormByID` (id 14617, read-locks the form id map).
#[must_use]
pub fn find_form_by_id(id: FormId) -> Option<&'static TESForm> {
    unsafe { TESForm::from_ptr((fns().find_by_id)(id.0)) }
}

/// The form registered under an editor id, through `FindFormByEditorID` (id 14620).
///
/// On this runtime only the classes that keep their editor id register one (keywords, globals,
/// cells, quests, races, worldspaces, topics, idles and a few more); weapons, armor, NPCs and
/// references do not, so they resolve to `None` here. Case-insensitive.
#[must_use]
pub fn find_form_by_editor_id(editor_id: &str) -> Option<&'static TESForm> {
    let name = CString::new(editor_id).ok()?;
    unsafe { TESForm::from_ptr((fns().find_by_editor_id)(name.as_ptr())) }
}

/// Base of every record form: `sre::TESForm` with typed accessors.
#[repr(transparent)]
pub struct TESForm(sre::TESForm);

type AsReferenceSlot = unsafe extern "C" fn(this: *mut sre::TESForm) -> *mut sre::TESObjectREFR;

impl TESForm {
    /// # Safety
    /// `ptr` must be null or point to a live form of any class derived from `TESForm` that stays
    /// alive for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(ptr: *const sre::TESForm) -> Option<&'a Self> {
        unsafe { ptr.cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &sre::TESForm {
        &self.0
    }

    #[must_use]
    pub fn as_ptr(&self) -> *const sre::TESForm {
        &raw const self.0
    }

    /// The form's dynamic class from its vtable pointer, `None` when the vtable is not in the ledger.
    #[must_use]
    pub fn class(&self) -> Option<&'static sre::raw::Vtable> {
        unsafe { crate::class_of(self.as_ptr().cast()) }
    }

    fn vtable(&self) -> &vtables::TESForm_vtable {
        unsafe { &*self.0.__vftable }
    }

    #[must_use]
    pub fn form_id(&self) -> FormId {
        FormId(self.0.formID)
    }

    /// The record type, `None` for a value outside the 138-entry table.
    #[must_use]
    pub fn form_type(&self) -> Option<FormType> {
        FormType::from_u8(self.0.formType)
    }

    #[must_use]
    pub fn form_type_raw(&self) -> u8 {
        self.0.formType
    }

    #[must_use]
    pub fn flags(&self) -> FormFlags {
        FormFlags(self.0.formFlags)
    }

    /// The files the form was read from, last one winning; empty for runtime-created forms.
    #[must_use]
    pub fn source_files(&self) -> &[Option<&TESFile>] {
        let Some(files) = (unsafe { self.0.sourceFiles.as_ref() }) else { return &[] };
        unsafe { ptr_slice(files.files, files.count) }
    }

    /// The editor id through vtable slot 50 (`GetFormEditorID`); `None` when the class keeps none.
    #[must_use]
    pub fn editor_id(&self) -> Option<&CStr> {
        let get = self.vtable().GetFormEditorID()?;
        let text = unsafe { get(&raw const self.0) };
        if text.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(text) };
        (!text.is_empty()).then_some(text)
    }

    /// The form's keyword component through `TESForm::GetKeywordForm` (id 14258), `None` for classes without one.
    #[must_use]
    pub fn keyword_form(&self) -> Option<&BGSKeywordForm> {
        let p = unsafe { (fns().keyword_form)(self.as_ptr().cast_mut()) };
        unsafe { p.cast::<BGSKeywordForm>().as_ref() }
    }

    /// The form's keywords; empty for classes without a keyword component.
    #[must_use]
    pub fn keywords(&self) -> &[Option<&BGSKeyword>] {
        self.keyword_form().map_or(&[], BGSKeywordForm::keywords)
    }

    #[must_use]
    pub fn has_keyword(&self, keyword: &BGSKeyword) -> bool {
        self.keyword_form().is_some_and(|k| k.has_keyword(keyword))
    }

    /// The form as a reference through vtable slot 44 (`AsReference`), `None` for non-references.
    /// The slot's meaning is rated medium in `forms.json` (static reading); the test plugin
    /// exercises it on the player and on a weapon.
    #[must_use]
    pub fn as_reference(&self) -> Option<&TESObjectREFR> {
        let slot = self.vtable().slot_44;
        let as_reference = unsafe { core::mem::transmute::<*const c_void, Option<AsReferenceSlot>>(slot) }?;
        let p = unsafe { as_reference(self.as_ptr().cast_mut()) };
        unsafe { p.cast::<TESObjectREFR>().as_ref() }
    }

    fn downcast<T>(&self, ty: FormType) -> Option<&T> {
        (self.form_type() == Some(ty)).then(|| unsafe { &*self.as_ptr().cast::<T>() })
    }
}

impl fmt::Debug for TESForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.form_type().map_or("????", FormType::signature), self.form_id())
    }
}

macro_rules! form_class {
    ($(#[$doc:meta] $name:ident($layout:ident), $ty:ident, $as:ident) => {
        $(#[$doc]
        #[repr(transparent)]
        pub struct $name(sre::$layout);

        impl $name {
            #[must_use]
            pub fn raw(&self) -> &sre::$layout {
                &self.0
            }
        }

        impl core::ops::Deref for $name {
            type Target = TESForm;

            // A packed layout struct has alignment 1; the live object is 8-aligned.
            #[allow(clippy::cast_ptr_alignment)]
            fn deref(&self) -> &TESForm {
                unsafe { &*(&raw const self.0).cast::<TESForm>() }
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                TESForm::fmt(self, f)
            }
        }

        impl TESForm {
            #[doc = concat!("The form as a `", stringify!($name), "` when its type is `", stringify!($ty), "`.")]
            #[must_use]
            pub fn $as(&self) -> Option<&$name> {
                self.downcast(FormType::$ty)
            }
        }
    };
}

form_class!(
    /// A `KYWD` form.
    BGSKeyword(BGSKeyword), KYWD, as_keyword
);
form_class!(
    /// A `FLST` form.
    BGSListForm(BGSListForm), FLST, as_form_list
);
form_class!(
    /// A `GLOB` form.
    TESGlobal(TESGlobal), GLOB, as_global
);
form_class!(
    /// A `WEAP` form; only the `TESForm` base and the keyword component are described by pass 1.
    TESObjectWEAP(TESObjectWEAP), WEAP, as_weapon
);
form_class!(
    /// An `NPC_` form; the keyword component from pass 1, the actor base fields from pass 2 (`skyrim::actors`).
    TESNPC(TESNPC), NPC_, as_npc
);
form_class!(
    /// A `RACE` form; only the `TESForm` base is described.
    TESRace(TESRace), RACE, as_race
);
form_class!(
    /// A `CLAS` form; only the `TESForm` base is described.
    TESClass(TESClass), CLAS, as_class
);
form_class!(
    /// A `CELL` form; only the `TESForm` base is described.
    TESObjectCELL(TESObjectCELL), CELL, as_cell
);
form_class!(
    /// A `WRLD` form; only the `TESForm` base is described.
    TESWorldSpace(TESWorldSpace), WRLD, as_worldspace
);
form_class!(
    /// An `LCTN` form; only the `TESForm` base is described.
    BGSLocation(BGSLocation), LCTN, as_location
);

impl BGSKeyword {
    /// The keyword's editor id from its `keyword` string; empty until a game is loaded.
    #[must_use]
    pub fn name(&self) -> &BSFixedString {
        unsafe { BSFixedString::from_ptr((&raw const self.0.keyword).cast()) }
    }
}

impl TESGlobal {
    /// The value, stored as a float whatever the declared type.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.0.value
    }
}

/// The keyword component embedded in forms that carry keywords (`TESObjectWEAP` at +0x140, `TESNPC` at +0x110, ...).
#[repr(transparent)]
pub struct BGSKeywordForm(sre::BGSKeywordForm);

impl BGSKeywordForm {
    #[must_use]
    pub fn keywords(&self) -> &[Option<&BGSKeyword>] {
        unsafe { ptr_slice(self.0.keywords, self.0.numKeywords) }
    }

    /// Through vtable slot 4 (`HasKeyword`); `false` when the slot is null.
    #[must_use]
    pub fn has_keyword(&self, keyword: &BGSKeyword) -> bool {
        let vt = unsafe { &*self.0.__vftable };
        vt.HasKeyword().is_some_and(|has| unsafe { has(&raw const self.0, &raw const keyword.0) })
    }
}

impl BGSListForm {
    /// The entries of the record; script-added forms are not in here.
    #[must_use]
    pub fn forms(&self) -> &[Option<&TESForm>] {
        unsafe { array(&self.0.forms) }
    }

    /// Number of record entries through `BGSListForm::GetFormsSize` (id 20911).
    #[must_use]
    pub fn len(&self) -> u32 {
        unsafe { (fns().list_len)((&raw const self.0).cast_mut()) }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entry `index` through `BGSListForm::GetAt` (id 20907), counting script-added forms first.
    #[must_use]
    pub fn get(&self, index: u32) -> Option<&TESForm> {
        unsafe { TESForm::from_ptr((fns().list_get_at)((&raw const self.0).cast_mut(), index)) }
    }

    /// Index of `form` through `BGSListForm::Find` (id 20906).
    #[must_use]
    pub fn find(&self, form: &TESForm) -> Option<u32> {
        let i = unsafe { (fns().list_find)((&raw const self.0).cast_mut(), form.as_ptr().cast_mut()) };
        u32::try_from(i).ok()
    }
}

/// One plugin file the data handler found in `Data`.
#[repr(transparent)]
pub struct TESFile(sre::TESFile);

impl TESFile {
    #[must_use]
    pub fn raw(&self) -> &sre::TESFile {
        &self.0
    }

    /// The file name; empty when it is not UTF-8.
    #[must_use]
    pub fn name(&self) -> &str {
        CStr::from_bytes_until_nul(&self.0.fileName).ok().and_then(|s| s.to_str().ok()).unwrap_or("")
    }

    #[must_use]
    pub fn flags(&self) -> FileFlags {
        FileFlags(self.0.fileFlags)
    }

    #[must_use]
    pub fn is_master(&self) -> bool {
        self.flags().contains(FileFlags::MASTER)
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.flags().contains(FileFlags::ACTIVE)
    }

    #[must_use]
    pub fn is_light(&self) -> bool {
        self.flags().contains(FileFlags::LIGHT)
    }

    /// The load index: the mod index of the file's forms, `0xFE` for a light file, `0xFF` when not loaded.
    #[must_use]
    pub fn compile_index(&self) -> u8 {
        self.0.compileIndex
    }

    /// The index among the light files, meaningful when [`compile_index`](Self::compile_index) is `0xFE`.
    #[must_use]
    pub fn light_file_index(&self) -> u16 {
        self.0.lightFileIndex
    }

    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.compile_index() != RUNTIME_INDEX
    }

    /// The master files, resolved to loaded files.
    #[must_use]
    pub fn masters(&self) -> &[Option<&TESFile>] {
        unsafe { ptr_slice(self.0.masters, self.0.masterCount) }
    }

    /// A file-relative id as a runtime form id through `TESFile::ResolveFormID` (id 14003); `None` when the file is not loaded.
    #[must_use]
    pub fn resolve_form_id(&self, local: u32) -> Option<FormId> {
        if !self.is_loaded() {
            return None;
        }
        let mut id = local;
        unsafe { (fns().resolve_form_id)(&raw const self.0, &raw mut id) };
        Some(FormId(id))
    }
}

/// The data handler singleton: every loaded file and the per-type form arrays.
#[repr(transparent)]
pub struct TESDataHandler(sre::TESDataHandler);

impl TESDataHandler {
    /// The singleton behind `g_TESDataHandler_ptr` (id 400269); `None` before `TES::ctor` has run.
    #[must_use]
    pub fn get() -> Option<&'static Self> {
        unsafe { (*fns().data_handler).cast::<Self>().as_ref() }
    }

    #[must_use]
    pub fn raw(&self) -> &sre::TESDataHandler {
        &self.0
    }

    /// Every form of one type, in load order.
    #[must_use]
    pub fn forms_of_type(&self, ty: FormType) -> &[Option<&TESForm>] {
        let arrays = (&raw const self.0).cast::<sre::BSTArray>().wrapping_byte_add(core::mem::offset_of!(sre::TESDataHandler, formArrays));
        unsafe { array(&*arrays.add(ty as usize)) }
    }

    /// The files found in `Data`, masters first, loaded or not.
    pub fn files(&self) -> impl Iterator<Item = &TESFile> {
        let mut node = Some(&self.0.files);
        core::iter::from_fn(move || {
            let n = node?;
            node = unsafe { n.next.cast::<sre::BSSimpleListNode>().as_ref() };
            unsafe { n.item.cast::<TESFile>().as_ref() }
        })
    }

    /// The regular loaded files; the index is each file's compile index.
    #[must_use]
    pub fn compiled_files(&self) -> &[Option<&TESFile>] {
        unsafe { array(&self.0.compiledFiles) }
    }

    /// The loaded light files; the index is each file's light index.
    #[must_use]
    pub fn light_files(&self) -> &[Option<&TESFile>] {
        unsafe { array(&self.0.lightFiles) }
    }

    /// A file by name through `TESDataHandler::GetFile` (id 13731), case-insensitive.
    #[must_use]
    pub fn file(&self, name: &str) -> Option<&TESFile> {
        let name = CString::new(name).ok()?;
        let p = unsafe { (fns().get_file)((&raw const self.0).cast_mut(), name.as_ptr()) };
        unsafe { p.cast::<TESFile>().as_ref() }
    }

    /// The id the next runtime-created form gets, in the `0xFF` range.
    #[must_use]
    pub fn next_form_id(&self) -> FormId {
        FormId(self.0.nextFormID)
    }
}
