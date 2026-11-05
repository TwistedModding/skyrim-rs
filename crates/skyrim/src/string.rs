//! `BSFixedString`, the engine's interned string, owned (interned on creation, released on drop)
//! or viewed inside an engine object.
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

use sre::raw::fns;

/// The reference count of a pinned entry, never counted up or down.
pub const PINNED: u32 = 0x7FFF;

/// An interned string: owned when made by [`new`](Self::new) or [`clone`](Clone::clone) (released on
/// drop), borrowed when viewed inside an engine object through [`view`](Self::view).
#[repr(transparent)]
pub struct BSFixedString(sre::BSFixedString);

impl BSFixedString {
    /// Interns `text` through `BSFixedString::ctor` (id 69161); a text with an interior NUL interns
    /// up to it.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let text = CString::new(text.split('\0').next().unwrap_or("")).unwrap_or_default();
        let mut s = Self::empty();
        let ctor: fns::BSFixedString__ctor_fn = unsafe { crate::engine_fn(sre::id!(fns::BSFixedString__ctor)) };
        unsafe { ctor(&raw mut s.0, text.as_ptr()) };
        s
    }

    /// A null string: reads as "", but is not the engine's pooled empty string (`new("")`), which
    /// pointer comparisons such as a native's state name need.
    #[must_use]
    pub const fn empty() -> Self {
        Self(sre::BSFixedString { data: core::ptr::null_mut(), __engine: sre::Engine::new() })
    }

    /// The string a field of an engine object holds, borrowed without touching its count.
    #[must_use]
    pub fn view(field: &sre::BSFixedString) -> &Self {
        unsafe { &*core::ptr::from_ref(field).cast::<Self>() }
    }

    /// Gives the reference up to an engine object that will hold it from now on.
    #[must_use]
    pub fn into_raw(self) -> sre::BSFixedString {
        let raw = sre::BSFixedString { data: self.0.data, __engine: sre::Engine::new() };
        core::mem::forget(self);
        raw
    }

    #[must_use]
    pub fn raw(&self) -> &sre::BSFixedString {
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
    pub fn entry(&self) -> Option<&sre::BSStringPool__Entry> {
        if self.is_empty() {
            return None;
        }
        unsafe { self.0.data.byte_sub(sre::class_size!("BSStringPool::Entry", sre::BSStringPool__Entry)).cast::<sre::BSStringPool__Entry>().as_ref() }
    }

    /// Byte count up to the NUL.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the entry holds `wchar_t` characters.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.entry().is_some_and(|e| sre::BSStringPool__Entry_flags(entry_flags(e)).contains(sre::BSStringPool__Entry_flags::WIDE))
    }

    /// Bits 0-14 of the entry flags: the reference count, [`PINNED`] for a pinned entry; 0 for the empty string.
    #[must_use]
    pub fn ref_count(&self) -> u32 {
        self.entry().map_or(0, |e| entry_flags(e) & PINNED)
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
        let mut s = Self::empty();
        let copy: fns::BSFixedString__copy_fn = unsafe { crate::engine_fn(sre::id!(fns::BSFixedString__copy)) };
        unsafe { copy(&raw mut s.0, &raw const self.0) };
        s
    }
}

impl Drop for BSFixedString {
    /// Drops the reference through `BSFixedString::release` (id 69164).
    fn drop(&mut self) {
        if !self.is_empty() {
            let release: fns::BSFixedString__release_fn = unsafe { crate::engine_fn(sre::id!(fns::BSFixedString__release)) };
            unsafe { release(&raw mut self.0) };
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

/// Through the profile by hand: this module is always compiled, and the accessor sits in a feature's group.
fn entry_flags(entry: &sre::BSStringPool__Entry) -> u32 {
    unsafe { core::ptr::from_ref(entry).byte_add(sre::member_offset!("BSStringPool::Entry.flags")).cast::<u32>().read() }
}
