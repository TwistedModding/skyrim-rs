//! Checks a function's entry bytes against the executable file before a detour goes in.

use core::ffi::c_void;
use std::fmt;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};

use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};

const LEN: usize = 16;

/// The first bytes of a function entry.
pub type Bytes = [u8; LEN];

/// What a patch site looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    /// The live bytes equal the executable file's at the same rva.
    Pristine,
    /// No file bytes to compare with, and the entry is not a jump, call or breakpoint.
    Plausible,
    /// This dispatcher's own jump is at the entry (directly or through a relay) and leads to `detour`.
    Managed { detour: usize },
    /// Something else changed the entry: a jump elsewhere, or bytes that differ from the file.
    Unmanaged { found: Bytes, expected: Option<Bytes>, target: Option<usize> },
}

impl Validation {
    /// Whether a detour may go in.
    #[must_use]
    pub fn accepted(&self) -> bool {
        !matches!(self, Self::Unmanaged { .. })
    }
}

impl fmt::Display for Validation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = |b: &Bytes| b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ");
        match self {
            Self::Pristine => f.write_str("pristine, equal to the executable file"),
            Self::Plausible => f.write_str("plausible, no file bytes to compare and no jump at the entry"),
            Self::Managed { detour } => write!(f, "managed, this dispatcher's jump to {detour:#x}"),
            Self::Unmanaged { found, expected, target } => {
                write!(f, "unmanaged patch, found {}", hex(found))?;
                if let Some(expected) = expected {
                    write!(f, ", the file has {}", hex(expected))?;
                }
                if let Some(target) = target {
                    write!(f, ", jumps to {target:#x}")?;
                }
                Ok(())
            }
        }
    }
}

/// Classifies the entry at `address`; `detour` is the jump target this dispatcher recognizes as its own.
#[must_use]
pub fn validate(address: usize, detour: Option<usize>) -> Validation {
    let Some(found) = read::<LEN>(address) else {
        return Validation::Unmanaged { found: [0; LEN], expected: None, target: None };
    };
    let expected = file_bytes(address);
    if let Some(target) = jump_target(address, &found).map(through_relay) {
        return if detour == Some(target) { Validation::Managed { detour: target } } else { Validation::Unmanaged { found, expected, target: Some(target) } };
    }
    match expected {
        Some(e) if e == found => Validation::Pristine,
        Some(e) => Validation::Unmanaged { found, expected: Some(e), target: None },
        None if matches!(found[0], 0xE8 | 0xCC) || (found[0] == 0xFF && found[1] & 0x38 == 0x20) => Validation::Unmanaged { found, expected: None, target: None },
        None => Validation::Plausible,
    }
}

fn rel32(next: usize, bytes: [u8; 4]) -> usize {
    next.wrapping_add_signed(i32::from_le_bytes(bytes) as isize)
}

/// The target of a `jmp rel32` at `site`, or of the `jmp rel32` a `jmp rel8` there leads to (the
/// hot-patch form, where the long jump sits in the padding before the function).
fn jump_target(site: usize, found: &Bytes) -> Option<usize> {
    match found[0] {
        0xE9 => Some(rel32(site + 5, [found[1], found[2], found[3], found[4]])),
        0xEB => {
            let short = (site + 2).wrapping_add_signed(isize::from(found[1].cast_signed()));
            read::<5>(short).filter(|b| b[0] == 0xE9).map(|b| rel32(short + 5, [b[1], b[2], b[3], b[4]]))
        }
        _ => None,
    }
}

/// Sees through a `jmp [rip+disp32]` relay at `target`.
fn through_relay(target: usize) -> usize {
    match read::<6>(target) {
        Some([0xFF, 0x25, d0, d1, d2, d3]) => read::<8>(rel32(target + 6, [d0, d1, d2, d3])).map_or(target, usize::from_le_bytes),
        _ => target,
    }
}

fn readable(address: usize, len: usize) -> bool {
    let mut info = unsafe { core::mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
    if unsafe { VirtualQuery(address as *const c_void, &raw mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
        return false;
    }
    info.State == MEM_COMMIT && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && address + len <= info.BaseAddress as usize + info.RegionSize
}

fn read<const N: usize>(address: usize) -> Option<[u8; N]> {
    readable(address, N).then(|| unsafe { (address as *const [u8; N]).read_unaligned() })
}

/// The bytes the executable file holds for `address`, located through the loaded image's section
/// table (`e_lfanew` at 0x3c; the section count and the optional header size at +6 and +20 of the
/// NT headers; 40-byte section headers with the virtual address, raw size and raw offset at +12, +16 and +20).
fn file_bytes(address: usize) -> Option<Bytes> {
    let base = hook::module_base();
    let rva = address.checked_sub(base)?;
    let u32_at = |offset: usize| unsafe { ((base + offset) as *const u32).read_unaligned() } as usize;
    let u16_at = |offset: usize| usize::from(unsafe { ((base + offset) as *const u16).read_unaligned() });
    let nt = u32_at(0x3c);
    let sections = nt + 24 + u16_at(nt + 20);
    let offset = (0..u16_at(nt + 6)).find_map(|i| {
        let section = sections + 40 * i;
        let (va, raw_size, raw) = (u32_at(section + 12), u32_at(section + 16), u32_at(section + 20));
        (rva >= va && rva + LEN <= va + raw_size).then(|| rva - va + raw)
    })?;
    let mut file = File::open(std::env::current_exe().ok()?).ok()?;
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut bytes = [0; LEN];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}
