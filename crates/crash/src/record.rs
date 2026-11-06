// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! What the watchdog gathers about a crash, as plain data: filled in place in a static (no heap),
//! attached to the minidump as a user stream and formatted into the text log by `report`; the
//! offline decoder reads the same record back from the dump.

pub const MAGIC: [u8; 8] = *b"SKYRMCR1";
pub const VERSION: u32 = 1;
/// The minidump user stream type carrying the record (above the reserved range).
pub const STREAM_TYPE: u32 = 0x1_5A4B;

pub const MAX_FRAMES: usize = 64;
pub const MAX_MODULES: usize = 512;
pub const MAX_THREADS: usize = 256;
pub const THREAD_FRAMES: usize = 8;
pub const MAX_IMAGES: usize = 64;
pub const PATH_LEN: usize = 512;
pub const NAME_LEN: usize = 64;
pub const TEXT_LEN: usize = 32 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Registers {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub eflags: u32,
    pub mxcsr: u32,
    pub segments: [u16; 6],
    pub xmm: [[u64; 2]; 16],
}

impl Registers {
    /// The general-purpose registers with their names, in the order the log shows them.
    #[must_use]
    pub fn general(&self) -> [(&'static str, u64); 17] {
        [
            ("rax", self.rax),
            ("rcx", self.rcx),
            ("rdx", self.rdx),
            ("rbx", self.rbx),
            ("rsp", self.rsp),
            ("rbp", self.rbp),
            ("rsi", self.rsi),
            ("rdi", self.rdi),
            ("r8", self.r8),
            ("r9", self.r9),
            ("r10", self.r10),
            ("r11", self.r11),
            ("r12", self.r12),
            ("r13", self.r13),
            ("r14", self.r14),
            ("r15", self.r15),
            ("rip", self.rip),
        ]
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Exception {
    pub code: u32,
    pub flags: u32,
    pub address: u64,
    pub parameter_count: u32,
    pub parameters: [u64; 15],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub rip: u64,
    pub rsp: u64,
}

/// A loaded module: base, size, link timestamp, file version when read, path as UTF-8.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Module {
    pub base: u64,
    pub size: u32,
    pub timestamp: u32,
    pub version: [u16; 4],
    pub version_known: u8,
    pub path_len: u16,
    pub path: [u8; PATH_LEN],
}

impl Module {
    #[must_use]
    pub fn path(&self) -> &str {
        text(&self.path, self.path_len as usize)
    }

    /// The file name without its folder.
    #[must_use]
    pub fn name(&self) -> &str {
        let path = self.path();
        path.rsplit(['\\', '/']).next().unwrap_or(path)
    }

    #[must_use]
    pub fn contains(&self, address: u64) -> bool {
        address >= self.base && address < self.base + u64::from(self.size)
    }
}

/// A framework plugin's image as the host loaded it: live, or unloaded by a reload.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Image {
    pub base: u64,
    pub size: u32,
    pub generation: u32,
    pub live: u8,
    pub name_len: u8,
    pub name: [u8; NAME_LEN],
}

impl Image {
    #[must_use]
    pub fn name(&self) -> &str {
        text(&self.name, self.name_len as usize)
    }

    #[must_use]
    pub fn contains(&self, address: u64) -> bool {
        address >= self.base && address < self.base + u64::from(self.size)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Thread {
    pub id: u32,
    pub frame_count: u32,
    pub frames: [u64; THREAD_FRAMES],
    pub rsp: u64,
    /// The stack reservation holding `rsp`, for naming addresses in it.
    pub stack_base: u64,
    pub stack_end: u64,
    pub name_len: u8,
    pub name: [u8; NAME_LEN],
}

impl Thread {
    #[must_use]
    pub fn name(&self) -> &str {
        text(&self.name, self.name_len as usize)
    }
}

/// What the watchdog read of the game's state through guarded reads, each item with its own flag.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Game {
    pub frame: u32,
    pub frame_known: u8,
    pub save_len: u8,
    pub save: [u8; 128],
    pub cell_id: u32,
    pub cell_known: u8,
    pub cell_name_len: u8,
    pub cell_name: [u8; NAME_LEN],
    pub worldspace_id: u32,
    pub worldspace_known: u8,
    pub position: [f32; 3],
}

#[repr(C)]
pub struct Record {
    pub magic: [u8; 8],
    pub size: u32,
    pub version: u32,
    /// Local time: year, month, day, hour, minute, second.
    pub time: [u16; 6],
    pub uptime_ms: u64,
    pub runtime: u32,
    pub main_thread: u32,
    pub thread: u32,
    pub unhandled: u8,
    /// 0 none (another runtime), 1 the ledger's rvas, 2 the Address Library's offsets.
    pub symbols: u8,
    pub symbol_mismatches: u32,
    pub first_chance: u32,
    pub installer_len: u8,
    pub installer: [u8; NAME_LEN],
    pub build_len: u16,
    pub build: [u8; 256],
    pub exception: Exception,
    pub registers: Registers,
    pub exe_base: u64,
    pub frame_count: u32,
    pub frames: [Frame; MAX_FRAMES],
    pub module_count: u32,
    pub modules: [Module; MAX_MODULES],
    pub image_count: u32,
    pub images: [Image; MAX_IMAGES],
    pub thread_count: u32,
    pub threads: [Thread; MAX_THREADS],
    pub game: Game,
    pub context_len: u32,
    pub context: [u8; TEXT_LEN],
    pub state_len: u32,
    pub state: [u8; TEXT_LEN],
    pub dump_written: u8,
    pub dump_error: u32,
}

impl Record {
    pub const ZERO: Self = unsafe { core::mem::zeroed() };

    #[must_use]
    pub fn frames(&self) -> &[Frame] {
        &self.frames[..(self.frame_count as usize).min(MAX_FRAMES)]
    }

    #[must_use]
    pub fn modules(&self) -> &[Module] {
        &self.modules[..(self.module_count as usize).min(MAX_MODULES)]
    }

    #[must_use]
    pub fn images(&self) -> &[Image] {
        &self.images[..(self.image_count as usize).min(MAX_IMAGES)]
    }

    #[must_use]
    pub fn threads(&self) -> &[Thread] {
        &self.threads[..(self.thread_count as usize).min(MAX_THREADS)]
    }

    #[must_use]
    pub fn module_at(&self, address: u64) -> Option<&Module> {
        self.modules().iter().find(|m| m.contains(address))
    }

    #[must_use]
    pub fn installer(&self) -> &str {
        text(&self.installer, self.installer_len as usize)
    }

    #[must_use]
    pub fn build(&self) -> &str {
        text(&self.build, self.build_len as usize)
    }

    #[must_use]
    pub fn context_text(&self) -> &str {
        text(&self.context, self.context_len as usize)
    }

    #[must_use]
    pub fn state_text(&self) -> &str {
        text(&self.state, self.state_len as usize)
    }

    /// The record's bytes, for the dump's user stream.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<u8>(), size_of::<Self>()) }
    }
}

/// The longest valid UTF-8 prefix of `len` bytes of `bytes`.
pub(crate) fn text(bytes: &[u8], len: usize) -> &str {
    let bytes = &bytes[..len.min(bytes.len())];
    match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// Copies `src` into `dst`, cut at a character boundary, and returns the length stored.
pub(crate) fn store(dst: &mut [u8], src: &str) -> usize {
    let mut len = src.len().min(dst.len());
    while len > 0 && !src.is_char_boundary(len) {
        len -= 1;
    }
    dst[..len].copy_from_slice(&src.as_bytes()[..len]);
    len
}
