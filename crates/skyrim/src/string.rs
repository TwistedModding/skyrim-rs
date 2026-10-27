//! `BSFixedString`, the engine's interned string, pinned by subsystem pass 3 because events, menus
//! and forms carry it.
//!
//! The value is one pointer to the NUL-terminated characters of a `BSStringPool::Entry`; the
//! entry header (`next`, `flags`, `length`) lies 0x18 bytes before them. Interning folds case, so
//! two strings that compare equal case-insensitively share one entry and one pointer, which is how
//! the engine itself compares menu names. The text is read up to its NUL; the header fields are
//! rated medium in `events.json` and only [`entry`](BSFixedString::entry), [`ref_count`](BSFixedString::ref_count)
//! and [`is_wide`](BSFixedString::is_wide) read them.

use core::ffi::{CStr, c_char};
use std::ffi::CString;
use std::fmt;

use skre::raw::fns;

type Ctor = unsafe extern "C" fn(this: *mut skre::BSFixedString, text: *const c_char) -> *mut skre::BSFixedString;
type Copy_ = unsafe extern "C" fn(this: *mut skre::BSFixedString, other: *const skre::BSFixedString) -> *mut skre::BSFixedString;
type Release = unsafe extern "C" fn(this: *mut skre::BSFixedString);

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
pub struct BSFixedString(skre::BSFixedString);

impl BSFixedString {
    /// Interns `text` through `BSFixedString::ctor` (id 69161); a text with an interior NUL interns
    /// up to it.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let text = CString::new(text.split('\0').next().unwrap_or("")).unwrap_or_default();
        let mut s = skre::BSFixedString { data: core::ptr::null_mut() };
        unsafe { (fns().ctor)(&raw mut s, text.as_ptr()) };
        Self(s)
    }

    /// A null string: reads as "", but is not the engine's pooled empty string (`new("")`), which
    /// pointer comparisons such as a native's state name need.
    #[must_use]
    pub const fn empty() -> Self {
        Self(skre::BSFixedString { data: core::ptr::null_mut() })
    }

    /// Gives the reference up to an engine object that will hold it from now on.
    #[must_use]
    pub fn into_raw(self) -> skre::BSFixedString {
        let raw = skre::BSFixedString { data: self.0.data };
        core::mem::forget(self);
        raw
    }

    /// A view of a `BSFixedString` the engine owns.
    ///
    /// # Safety
    /// `p` must point to a `BSFixedString` field that stays valid for `'a`.
    #[must_use]
    pub unsafe fn from_ptr<'a>(p: *const skre::BSFixedString) -> &'a Self {
        unsafe { &*p.cast::<Self>() }
    }

    /// For an engine out parameter that stores one reference into the string.
    pub(crate) fn as_mut_ptr(&mut self) -> *mut skre::BSFixedString {
        &raw mut self.0
    }

    #[must_use]
    pub fn raw(&self) -> &skre::BSFixedString {
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
    pub fn entry(&self) -> Option<&skre::BSStringPool__Entry> {
        if self.is_empty() {
            return None;
        }
        unsafe { self.0.data.cast::<skre::BSStringPool__Entry>().sub(1).as_ref() }
    }

    /// Byte count up to the NUL.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the entry holds `wchar_t` characters.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.entry().is_some_and(|e| skre::BSStringPool__Entry_flags(e.flags).contains(skre::BSStringPool__Entry_flags::WIDE))
    }

    /// Bits 0-14 of the entry flags: the reference count, [`PINNED`] for a pinned entry; 0 for the empty string.
    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.entry().map_or(0, |e| e.flags & PINNED)
    }

    /// The bytes up to the NUL; empty for the empty string, one character of a wide entry.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.as_c_str().map_or(&[], CStr::to_bytes)
    }

    /// The text when it is UTF-8.
    #[must_use]
    pub fn to_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// The NUL-terminated characters, `None` for the empty string.
    #[must_use]
    pub fn as_c_str(&self) -> Option<&CStr> {
        if self.is_empty() {
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
        let mut s = skre::BSFixedString { data: core::ptr::null_mut() };
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
        f.write_str(&String::from_utf8_lossy(self.as_bytes()))
    }
}

impl fmt::Debug for BSFixedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}@{:p}", self.to_string(), self.0.data)
    }
}
