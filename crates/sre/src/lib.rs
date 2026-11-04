//! SkyrimSE.exe 1.7.104 as the `SkyrimRE` ledgers describe it: every class as a struct with its
//! engine functions as methods, the enums and flag sets, and the globals as accessors.
#![no_std]

extern crate std;

use core::sync::atomic::{AtomicUsize, Ordering};
use std::boxed::Box;
use std::sync::OnceLock;

use versionlib::profile::{Profile, Row};

/// The last field of every class: the engine writes its objects through any pointer it is handed,
/// so a reference to one must not promise the compiler that the memory behind it stays unchanged.
/// This zero-sized cell makes every class interior-mutable, which drops that promise.
#[derive(Default)]
pub struct Engine(core::cell::UnsafeCell<()>);

impl Engine {
    #[must_use]
    pub const fn new() -> Self {
        Self(core::cell::UnsafeCell::new(()))
    }
}

// SAFETY: the marker holds no data; a class's own fields decide whether it is Send or Sync.
unsafe impl Sync for Engine {}

const _: () = assert!(skse::runtime::CANONICAL == skse::runtime::pack(VERSION[0], VERSION[1], VERSION[2], 0));

/// An AE Address Library id, resolved on the running game through its profile on first use and cached.
#[doc(hidden)]
pub struct Relocation {
    id: u32,
    address: AtomicUsize,
}

impl Relocation {
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self { id, address: AtomicUsize::new(0) }
    }

    /// # Panics
    /// When the runtime's profile or Address Library file cannot be read, or the id has no usable
    /// row in the profile or is not in the file.
    #[inline]
    pub fn address(&self) -> usize {
        match self.address.load(Ordering::Relaxed) {
            0 => self.resolve(),
            address => address,
        }
    }

    #[cold]
    #[inline(never)]
    fn resolve(&self) -> usize {
        let running = versionlib::running().unwrap_or_else(|e| panic!("id {}: {e}", self.id));
        let address = running
            .address(u64::from(self.id))
            .unwrap_or_else(|| panic!("id {} has no usable profile row or is not in {}", self.id, running.path().display()));
        self.address.store(address, Ordering::Relaxed);
        address
    }
}

/// A number the running game's profile gives, cached in a static of the call site.
#[doc(hidden)]
#[derive(Default)]
pub struct Cached(AtomicUsize);

impl Cached {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    #[inline]
    pub fn get(&self, read: impl FnOnce() -> usize) -> usize {
        match self.0.load(Ordering::Relaxed) {
            0 => {
                let value = read();
                self.0.store(value + 1, Ordering::Relaxed);
                value
            }
            stored => stored - 1,
        }
    }
}

fn profile() -> &'static Profile {
    versionlib::profile::current().unwrap_or_else(|e| panic!("{e}"))
}

/// The value of a profile row, panicking on a row with none: the manifest check keeps this from
/// happening in a plugin that loaded.
#[cold]
#[inline(never)]
fn usable(what: core::fmt::Arguments, row: Row<u32>, canonical: usize) -> usize {
    match row {
        Row::Same => canonical,
        Row::Here(value) => value as usize,
        Row::Missing => panic!("{what} does not exist on this runtime"),
        Row::Unverified => panic!("{what} is unverified on this runtime"),
    }
}

#[doc(hidden)]
#[cold]
#[inline(never)]
#[must_use]
pub fn member_offset(member: &'static str, canonical: usize) -> usize {
    usable(format_args!("member {member}"), profile().member(member), canonical)
}

#[doc(hidden)]
#[cold]
#[inline(never)]
#[must_use]
pub fn base_offset(class_and_base: &'static str, canonical: usize) -> usize {
    usable(format_args!("base {class_and_base}"), profile().base(class_and_base), canonical)
}

#[doc(hidden)]
#[cold]
#[inline(never)]
#[must_use]
pub fn class_size(class: &'static str, canonical: usize) -> usize {
    usable(format_args!("the size of {class}"), profile().size(class), canonical)
}

#[doc(hidden)]
#[cold]
#[inline(never)]
#[must_use]
pub fn slot_index(class: &'static str, vtable_offset: u32, index: u32) -> usize {
    usable(format_args!("slot {class}[{vtable_offset:x}].{index:x}"), profile().slot(class, vtable_offset, index), index as usize)
}

/// The number of `Enum.Value` on the running game, `usize::MAX` where it does not exist.
#[doc(hidden)]
#[cold]
#[inline(never)]
#[must_use]
pub fn enum_value(value: &'static str, canonical: usize) -> usize {
    match profile().enum_value(value) {
        Row::Same => canonical,
        Row::Here(here) => here as usize,
        Row::Missing | Row::Unverified => usize::MAX,
    }
}

/// A patch site: an offset into the function at an AE id, resolved on first use and cached.
#[doc(hidden)]
pub struct Site {
    id: u32,
    offset: u32,
    address: AtomicUsize,
}

impl Site {
    #[must_use]
    pub const fn new(id: u32, offset: u32) -> Self {
        Self { id, offset, address: AtomicUsize::new(0) }
    }

    /// # Panics
    /// When the function is not usable on this runtime or the site does not exist on it.
    #[inline]
    pub fn address(&self) -> usize {
        match self.address.load(Ordering::Relaxed) {
            0 => self.resolve(),
            address => address,
        }
    }

    #[cold]
    #[inline(never)]
    fn resolve(&self) -> usize {
        let (id, offset) = (self.id, self.offset);
        let running = versionlib::running().unwrap_or_else(|e| panic!("site {id}+{offset:#x}: {e}"));
        let address = running.site(u64::from(id), offset).unwrap_or_else(|| panic!("site {id}+{offset:#x} has no usable profile row or its function is not in {}", running.path().display()));
        self.address.store(address, Ordering::Relaxed);
        address
    }
}

/// Entry `index` of the vtable `object` starts with.
///
/// # Safety
/// `object` must start with a pointer to a vtable of more than `index` entries.
#[doc(hidden)]
#[inline]
pub unsafe fn vtable_entry<T>(object: &T, index: usize) -> usize {
    unsafe { *(*core::ptr::from_ref(object).cast::<*const usize>()).add(index) }
}

/// A manifest record, emitted with the code that needs it, so an unused item leaves none.
#[doc(hidden)]
#[macro_export]
macro_rules! record {
    ($kind:literal, $a:literal, $b:literal, $name:literal) => {
        #[allow(unused_unsafe)]
        unsafe {
            core::arch::asm!(concat!(".pushsection .sremf,\"dr\"\n.byte ", $kind, "\n.long ", $a, ", ", $b, "\n.asciz \"", $name, "\"\n.popsection"), options(nomem, nostack, preserves_flags))
        };
    };
}

/// The live address of an Address Library id, cached in a static of the call site.
#[allow(unused_macros)]
macro_rules! address {
    ($id:literal) => {{
        record!(1, $id, 0, "");
        static RELOCATION: $crate::Relocation = $crate::Relocation::new($id);
        RELOCATION.address()
    }};
}

/// The function at an Address Library id as an `unsafe extern "C" fn` of the given parameters.
#[allow(unused_macros)]
macro_rules! function {
    ($id:literal, ($($param:ty),*) $(-> $ret:ty)?) => {
        core::mem::transmute::<usize, unsafe extern "C" fn($($param),*) $(-> $ret)?>(address!($id))
    };
}

/// The running game's index of a slot, given the class, the canonical vtable offset and index,
/// recorded in the DLL's manifest. Hand-written code takes every numbered slot through this.
#[macro_export]
macro_rules! slot_index {
    ($class:literal, $vtable_offset:literal, $index:literal) => {{
        $crate::record!(3, $vtable_offset, $index, $class);
        static INDEX: $crate::Cached = $crate::Cached::new();
        INDEX.get(|| $crate::slot_index($class, $vtable_offset, $index))
    }};
}

/// Asserts that a table built by hand for the engine to read still has these slots where it put
/// them, and records them in the DLL's manifest.
#[macro_export]
macro_rules! fixed_slots {
    ($class:literal, $vtable_offset:literal, $($index:literal),+) => {
        $(assert!($crate::slot_index!($class, $vtable_offset, $index) == $index, concat!($class, " slot ", $index, " moved"));)+
    };
}

/// Vtable slot `index` of an object as an `unsafe extern "C" fn` of the given parameters; with a
/// class and vtable offset the index comes from the running game's profile.
#[allow(unused_macros)]
macro_rules! virtual_function {
    ($object:expr, $index:literal, ($($param:ty),*) $(-> $ret:ty)?) => {
        core::mem::transmute::<usize, unsafe extern "C" fn($($param),*) $(-> $ret)?>($crate::vtable_entry($object, $index))
    };
    ($object:expr, $class:literal, $vtable_offset:literal, $index:literal, ($($param:ty),*) $(-> $ret:ty)?) => {
        core::mem::transmute::<usize, unsafe extern "C" fn($($param),*) $(-> $ret)?>($crate::vtable_entry($object, slot_index!($class, $vtable_offset, $index)))
    };
}

/// `*mut T` of a member of the object `this` points to, at the running game's offset.
#[allow(unused_macros)]
macro_rules! member {
    ($this:expr, $name:literal, $canonical:literal, $ty:ty) => {
        ($this as *const u8).add(member_offset!($name, $canonical)).cast_mut().cast::<$ty>()
    };
}

/// `Some` number of `Enum.Value` on the running game as `$repr`, cached in a static of the call site.
#[allow(unused_macros)]
macro_rules! engine_value {
    ($name:literal, $canonical:literal, $repr:ty) => {{
        static VALUE: $crate::Cached = $crate::Cached::new();
        match VALUE.get(|| $crate::enum_value($name, $canonical)) {
            usize::MAX => None,
            value => Some(value as $repr),
        }
    }};
}

/// The running game's offset of `Class.member`, recorded in the DLL's manifest. Hand-written code
/// takes every offset into engine data through this or a generated field or accessor.
#[macro_export]
macro_rules! member_offset {
    ($name:literal, $canonical:literal) => {{
        $crate::record!(2, 0, 0, $name);
        static OFFSET: $crate::Cached = $crate::Cached::new();
        OFFSET.get(|| $crate::member_offset($name, $canonical))
    }};
}

/// The running game's displacement of the base `Class:Base`, recorded in the DLL's manifest.
#[macro_export]
macro_rules! base_offset {
    ($name:literal, $canonical:literal) => {{
        $crate::record!(6, 0, 0, $name);
        static OFFSET: $crate::Cached = $crate::Cached::new();
        OFFSET.get(|| $crate::base_offset($name, $canonical))
    }};
}

/// The size of a class on the running game, for striding an array of them or allocating one:
/// `sre::class_size!("TESObjectREFR", sre::TESObjectREFR)`, the ledger's name and the struct. A
/// class with a partial layout answers its known lower bound on 1.7.104.
///
/// # Panics
/// When the class has no usable size on this runtime; the manifest check prevents that in a plugin
/// that loaded.
#[macro_export]
macro_rules! class_size {
    ($name:literal, $class:ty) => {{
        $crate::record!(5, 0, 0, $name);
        static SIZE: $crate::Cached = $crate::Cached::new();
        SIZE.get(|| $crate::class_size($name, size_of::<$class>()))
    }};
}

/// Asserts that the running game's size of a class is the struct's, for code that strides an array
/// of them as a Rust slice, and records it in the DLL's manifest.
#[macro_export]
macro_rules! fixed_size {
    ($name:literal, $class:ty) => {
        assert!($crate::class_size!($name, $class) == size_of::<$class>(), concat!("the size of ", $name, " differs on this runtime"));
    };
}

/// An id of `sre::raw::{fns, globals, vtables}`, recorded in the DLL's manifest so the loader can
/// refuse a plugin whose ids this runtime lacks. Hand-written code takes every id through this.
#[macro_export]
macro_rules! id {
    ($id:expr) => {{
        #[allow(unused_unsafe)]
        unsafe {
            core::arch::asm!(".pushsection .sremf,\"dr\"", ".byte 1", ".long {id}, 0", ".byte 0", ".popsection", id = const $id, options(nomem, nostack, preserves_flags))
        };
        $id
    }};
}

/// Gives a DLL a manifest section even if it records nothing, so that it does not read as built
/// before the manifest.
#[macro_export]
macro_rules! declare_manifest {
    () => {
        #[allow(unused_unsafe)]
        unsafe {
            core::arch::asm!(".pushsection .sremf,\"dr\"", ".byte 0", ".popsection", options(nomem, nostack, preserves_flags))
        };
    };
}

/// Records in the DLL's manifest the site `offset` bytes into the function at an id on 1.7.104.
#[macro_export]
macro_rules! record_site {
    ($id:expr, $offset:expr) => {
        #[allow(unused_unsafe)]
        unsafe {
            core::arch::asm!(
                ".pushsection .sremf,\"dr\"", ".byte 1", ".long {id}, 0", ".byte 0", ".byte 4", ".long {id}, {offset}", ".byte 0", ".popsection",
                id = const $id, offset = const $offset, options(nomem, nostack, preserves_flags)
            )
        };
    };
}

/// The live address of the site `offset` bytes into the function at an id on 1.7.104, wherever the
/// running game's profile places it, recorded in the DLL's manifest.
///
/// # Panics
/// When the function or the site has no usable row; the manifest check prevents that in a plugin
/// that loaded.
#[macro_export]
macro_rules! site {
    ($id:expr, $offset:expr) => {{
        $crate::record_site!($id, $offset);
        static SITE: $crate::Site = $crate::Site::new($id as u32, $offset as u32);
        SITE.address()
    }};
}

#[rustfmt::skip]
mod generated;
pub use generated::*;

/// The vtable at a module-relative address of the running game, every vtable placed through its id.
#[must_use]
pub fn vtable_at(rva: u32) -> Option<&'static raw::Vtable> {
    static PLACED: OnceLock<Box<[(u32, u32)]>> = OnceLock::new();
    let placed = PLACED.get_or_init(|| {
        let Ok(running) = versionlib::running() else { return Box::default() };
        raw::vtables::ALL.placed(|vtable| u32::try_from(running.offset(vtable.id)?).ok())
    });
    raw::vtables::ALL.get(placed[placed.binary_search_by_key(&rva, |entry| entry.0).ok()?].1 as usize)
}

/// The types of the raw tables, re-exported through `raw`.
mod tables {
    use std::boxed::Box;
    use std::vec::Vec;

    /// One vtable of a class as the RTTI records it; `offset` is where the subobject using it starts.
    pub struct Vtable {
        pub class: &'static str,
        pub offset: u32,
        pub id: u64,
        pub rva: u32,
    }

    /// How sure the ledger is of a name.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Confidence {
        High,
        Medium,
        Low,
        Unknown,
    }

    impl Confidence {
        #[must_use]
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::High => "high",
                Self::Medium => "medium",
                Self::Low => "low",
                Self::Unknown => "unknown",
            }
        }
    }

    /// One function the ledger names: its module-relative address, its Address Library id and the
    /// name as `fns` spells it in its docs; the semantic pass meaning, where one exists, is in
    /// `symbols::MEANINGS` under the same rva.
    pub struct Function {
        pub rva: u32,
        pub id: u32,
        pub confidence: Confidence,
        pub name: &'static str,
    }

    /// One global the ledger names.
    pub struct Global {
        pub rva: u32,
        pub id: u32,
        pub name: &'static str,
    }

    /// A table generated in parts, each in its own file.
    pub struct Table<T: 'static>(pub &'static [&'static [T]]);

    impl<T> Table<T> {
        #[must_use]
        pub fn len(&self) -> usize {
            self.0.iter().map(|p| p.len()).sum()
        }

        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.0.iter().all(|p| p.is_empty())
        }

        #[must_use]
        pub fn get(&self, mut index: usize) -> Option<&'static T> {
            for part in self.0 {
                if index < part.len() {
                    return Some(&part[index]);
                }
                index -= part.len();
            }
            None
        }

        pub fn iter(&self) -> impl Iterator<Item = &'static T> {
            self.0.iter().flat_map(|p| p.iter())
        }

        /// `(offset, row index)` of every row `offset_of` places, sorted by offset.
        pub fn placed(&self, mut offset_of: impl FnMut(&T) -> Option<u32>) -> Box<[(u32, u32)]> {
            let mut placed: Vec<(u32, u32)> = self.iter().enumerate().filter_map(|(index, row)| Some((offset_of(row)?, u32::try_from(index).ok()?))).collect();
            placed.sort_unstable();
            placed.into_boxed_slice()
        }

        /// The index of the last entry whose key is at most `key`, in a table sorted by `key_of`.
        pub fn floor<K: Ord + Copy>(&self, key: K, key_of: impl Fn(&T) -> K) -> Option<usize> {
            let mut found = None;
            let mut start = 0;
            for part in self.0 {
                let n = part.partition_point(|e| key_of(e) <= key);
                if n == 0 {
                    break;
                }
                found = Some(start + n - 1);
                if n < part.len() {
                    break;
                }
                start += part.len();
            }
            found
        }
    }
}
