// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Guarded memory access: nothing in the report dereferences a pointer it has not asked about.

use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_FREE, MEM_IMAGE, MEM_MAPPED, MEM_PRIVATE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// What the address space says about one address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub known: bool,
    pub base: u64,
    pub size: u64,
    pub allocation_base: u64,
    /// `MEM_COMMIT`, `MEM_RESERVE` or `MEM_FREE`.
    pub state: u32,
    pub protect: u32,
    /// `MEM_IMAGE`, `MEM_MAPPED` or `MEM_PRIVATE`.
    pub kind: u32,
}

impl Region {
    #[must_use]
    pub fn committed(&self) -> bool {
        self.known && self.state == MEM_COMMIT
    }

    #[must_use]
    pub fn free(&self) -> bool {
        self.known && self.state == MEM_FREE
    }

    #[must_use]
    pub fn reserved(&self) -> bool {
        self.known && self.state == MEM_RESERVE
    }

    #[must_use]
    pub fn guard(&self) -> bool {
        self.committed() && self.protect & PAGE_GUARD != 0
    }

    #[must_use]
    pub fn readable(&self) -> bool {
        self.committed() && self.protect & PAGE_GUARD == 0 && self.protect & (PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY) != 0
    }

    #[must_use]
    pub fn writable(&self) -> bool {
        self.committed() && self.protect & (PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY) != 0
    }

    #[must_use]
    pub fn executable(&self) -> bool {
        self.committed() && self.protect & (PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY) != 0
    }

    #[must_use]
    pub fn no_access(&self) -> bool {
        self.committed() && self.protect & PAGE_NOACCESS != 0
    }

    #[must_use]
    pub fn image(&self) -> bool {
        self.kind == MEM_IMAGE
    }

    #[must_use]
    pub fn mapped(&self) -> bool {
        self.kind == MEM_MAPPED
    }

    #[must_use]
    pub fn private(&self) -> bool {
        self.kind == MEM_PRIVATE
    }
}

/// Memory as the report reads it: the live process, or a minidump offline.
pub trait Source {
    /// Fills `buf` from `address`; false when any byte is not readable.
    fn read(&self, address: u64, buf: &mut [u8]) -> bool;
    fn region(&self, address: u64) -> Region;

    fn read_u64(&self, address: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        self.read(address, &mut b).then(|| u64::from_le_bytes(b))
    }

    fn read_u32(&self, address: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        self.read(address, &mut b).then(|| u32::from_le_bytes(b))
    }

    fn read_u16(&self, address: u64) -> Option<u16> {
        let mut b = [0u8; 2];
        self.read(address, &mut b).then(|| u16::from_le_bytes(b))
    }

    fn read_u8(&self, address: u64) -> Option<u8> {
        let mut b = [0u8; 1];
        self.read(address, &mut b).then(|| b[0])
    }

    fn read_f32(&self, address: u64) -> Option<f32> {
        self.read_u32(address).map(f32::from_bits)
    }

    /// A NUL-terminated string at `address` into `buf`; the bytes stored, none when unreadable.
    fn read_cstr(&self, address: u64, buf: &mut [u8]) -> Option<usize> {
        if address == 0 {
            return None;
        }
        let mut len = 0;
        while len < buf.len() {
            let chunk = (buf.len() - len).min(64);
            let page_room = usize::try_from(0x1000 - ((address + len as u64) & 0xFFF)).unwrap_or(1);
            let n = chunk.min(page_room);
            if !self.read(address + len as u64, &mut buf[len..len + n]) {
                return (len > 0).then_some(len);
            }
            if let Some(nul) = buf[len..len + n].iter().position(|&b| b == 0) {
                return Some(len + nul);
            }
            len += n;
        }
        Some(len)
    }
}

/// The running process.
pub struct Live;

impl Source for Live {
    fn read(&self, address: u64, buf: &mut [u8]) -> bool {
        if buf.is_empty() {
            return true;
        }
        if address == 0 || address.checked_add(buf.len() as u64).is_none() {
            return false;
        }
        let mut read = 0usize;
        let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), address as *const core::ffi::c_void, buf.as_mut_ptr().cast(), buf.len(), &raw mut read) };
        ok != 0 && read == buf.len()
    }

    fn region(&self, address: u64) -> Region {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        let n = unsafe { VirtualQuery(address as *const core::ffi::c_void, &raw mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
        if n == 0 {
            return Region::default();
        }
        Region { known: true, base: info.BaseAddress as u64, size: info.RegionSize as u64, allocation_base: info.AllocationBase as u64, state: info.State, protect: info.Protect, kind: info.Type }
    }
}

/// The extent of the stack reservation holding `rsp`: every region sharing its allocation base.
pub fn stack_extent(src: &dyn Source, rsp: u64) -> Option<(u64, u64)> {
    let region = src.region(rsp);
    if !region.known || region.state == MEM_FREE {
        return None;
    }
    let base = region.allocation_base;
    let mut end = region.base + region.size;
    for _ in 0..64 {
        let next = src.region(end);
        if !next.known || next.allocation_base != base || next.state == MEM_FREE {
            break;
        }
        end = next.base + next.size;
    }
    Some((base, end))
}
