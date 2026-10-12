//! `BSFixedString`, the engine's interned string, pinned by subsystem pass 3 because events, menus
//! and forms carry it.
//!
//! The value is one pointer to the characters of a `BSStringPool::Entry`; the entry header
//! (`next`, `flags`, `length`) lies 0x18 bytes before them. Interning folds case, so two strings
//! that compare equal case-insensitively share one entry and one pointer, which is how the engine
//! itself compares menu names. The header fields are rated medium in `events.json`: the test
//! plugin reads them on `VendorItemWeapon` and `TweenMenu`.

use core::ffi::{CStr, c_char};
use std::ffi::CString;
use std::fmt;

use layout::v1_7_104::{classes, enums, fns};

type Ctor = unsafe extern "C" fn(this: *mut classes::BSFixedString, text: *const c_char) -> *mut classes::BSFixedString;
type Copy_ = unsafe extern "C" fn(this: *mut classes::BSFixedString, other: *const classes::BSFixedString) -> *mut classes::BSFixedString;
type Release = unsafe extern "C" fn(this: *mut classes::BSFixedString);

pub(crate) struct Fns {
    ctor: Ctor,
    copy: Copy_,
    release: Release,
}

impl Fns {
    pub(crate) fn resolve(reloc: &hook::Reloc) -> Result<Self, hook::Error> {
        unsafe {
            Ok(Self {
                ctor: crate::resolve(reloc, fns::BSFixedString__ctor)?,
                copy: crate::resolve(reloc, fns::BSFixedString__copy)?,
                release: crate::resolve(reloc, fns::BSFixedString__release)?,
            })
        }
    }
}

fn fns() -> &'static Fns {
    &crate::runtime().strings
}

/// The reference count of a pinned entry, never counted up or down.
pub const PINNED: u32 = 0x7FFF;

/// An interned string: owned when made by [`new`](Self::new) or [`clone`](Clone::clone) (released on
/// drop), borrowed when viewed inside an engine object through [`from_ptr`](Self::from_ptr).
#[repr(transparent)]
pub struct BSFixedString(classes::BSFixedString);

impl BSFixedString {
    /// Interns `text` through `BSFixedString::ctor` (id 69161); a text with an interior NUL interns
    /// up to it.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let text = CString::new(text.split('\0').next().unwrap_or("")).unwrap_or_default();
        let mut s = classes::BSFixedString { data: core::ptr::null_mut() };
        unsafe { (fns().ctor)(&raw mut s, text.as_ptr()) };
        Self(s)
    }

    /// A view of a `BSFixedString` the engine owns.
    ///
    /// # Safety
    /// `p` must point to a `BSFixedString` field that stays valid for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const classes::BSFixedString) -> &'a Self {
        unsafe { &*p.cast::<Self>() }
    }

    #[must_use]
    pub fn raw(&self) -> &classes::BSFixedString {
        &self.0
    }

    /// The characters, null for the empty string.
    #[must_use]
    pub fn as_ptr(&self) -> *const c_char {
        self.0.data.cast_const().cast()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.data.is_null()
    }

    /// The pool entry header, `None` for the empty string.
    #[must_use]
    pub fn entry(&self) -> Option<&classes::BSStringPool__Entry> {
        if self.is_empty() {
            return None;
        }
        unsafe { self.0.data.cast::<classes::BSStringPool__Entry>().sub(1).as_ref() }
    }

    /// Character count without the terminator.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entry().map_or(0, |e| e.length as usize)
    }

    /// Whether the entry holds `wchar_t` characters.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.entry().is_some_and(|e| enums::BSStringPool__Entry_flags(e.flags).contains(enums::BSStringPool__Entry_flags::WIDE))
    }

    /// Bits 0-14 of the entry flags: the reference count, [`PINNED`] for a pinned entry; 0 for the empty string.
    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.entry().map_or(0, |e| e.flags & PINNED)
    }

    /// The bytes of a narrow string; empty for the empty string and for wide entries.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        if self.is_empty() || self.is_wide() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.0.data.cast::<u8>(), self.len()) }
    }

    /// The text when it is narrow and UTF-8.
    #[must_use]
    pub fn to_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// The NUL-terminated characters of a narrow string.
    #[must_use]
    pub fn as_c_str(&self) -> Option<&CStr> {
        if self.is_empty() || self.is_wide() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(self.as_ptr()) })
    }

    /// Whether both share one pool entry, which interning makes the engine's own equality.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0.data, other.0.data)
    }
}

impl Clone for BSFixedString {
    /// Adds a reference through `BSFixedString::copy` (id 69162).
    fn clone(&self) -> Self {
        let mut s = classes::BSFixedString { data: core::ptr::null_mut() };
        unsafe { (fns().copy)(&raw mut s, &raw const self.0) };
        Self(s)
    }
}

impl Drop for BSFixedString {
    /// Drops the reference through `BSFixedString::release` (id 69164).
    fn drop(&mut self) {
        if !self.is_empty() {
            unsafe { (fns().release)(&raw mut self.0) };
        }
    }
}

impl PartialEq for BSFixedString {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
    }
}

impl Eq for BSFixedString {}

impl PartialEq<str> for BSFixedString {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes().eq_ignore_ascii_case(other.as_bytes())
    }
}

impl fmt::Display for BSFixedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_wide() {
            return write!(f, "<wide string of {} characters>", self.len());
        }
        f.write_str(&String::from_utf8_lossy(self.as_bytes()))
    }
}

impl fmt::Debug for BSFixedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}@{:p}", self.to_string(), self.0.data)
    }
}
