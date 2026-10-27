//! SkyrimSE.exe 1.7.104 as the `SkyrimRE` ledgers describe it: every class as a struct with its
//! engine functions as methods, the enums and flag sets, and the globals as accessors.
#![no_std]

use core::sync::atomic::{AtomicUsize, Ordering};

/// An Address Library id of the running game, resolved on first use and cached.
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
    /// When the runtime's Address Library file cannot be opened or does not hold the id.
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
        let [major, minor, build] = VERSION;
        let file = skse::runtime::versionlib_name(skse::runtime::pack(major, minor, build, 0));
        let running = versionlib::running(&file).unwrap_or_else(|e| panic!("id {}: {e}", self.id));
        let address = running
            .address(u64::from(self.id))
            .unwrap_or_else(|| panic!("id {} is not in {}", self.id, running.path().display()));
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

/// The live address of an Address Library id, cached in a static of the call site.
#[allow(unused_macros)]
macro_rules! address {
    ($id:literal) => {{
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

/// Vtable slot `index` of an object as an `unsafe extern "C" fn` of the given parameters.
#[allow(unused_macros)]
macro_rules! virtual_function {
    ($object:expr, $index:literal, ($($param:ty),*) $(-> $ret:ty)?) => {
        core::mem::transmute::<usize, unsafe extern "C" fn($($param),*) $(-> $ret)?>($crate::vtable_entry($object, $index))
    };
}

#[rustfmt::skip]
mod generated;
pub use generated::*;

/// The types of the raw tables, re-exported through `raw`.
mod tables {
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
