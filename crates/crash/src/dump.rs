//! A minidump read back offline: the record the logger attached as a user stream, the dump's own
//! streams where the record is missing, and the dump's memory as a `Source`. Stream layouts follow
//! `minidumpapiset.h` (the header, the directory, the thread, module, memory, memory-info and
//! exception streams).

use std::fmt;

use crate::mem::{Region, Source};
use crate::record::{self, Frame, MAX_FRAMES, MAX_MODULES, MAX_THREADS, Module, PATH_LEN, Record, Thread};
use crate::text::utf16_into;

const SIGNATURE: u32 = 0x504D_444D;
const THREAD_LIST: u32 = 3;
const MODULE_LIST: u32 = 4;
const MEMORY_LIST: u32 = 5;
const EXCEPTION: u32 = 6;
const MEMORY64_LIST: u32 = 9;
const MEMORY_INFO_LIST: u32 = 16;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Format(what) => write!(f, "not a usable minidump: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct Range {
    start: u64,
    len: u64,
    file_offset: u64,
}

/// A dump file in memory.
pub struct Dump {
    bytes: Vec<u8>,
    ranges: Vec<Range>,
    regions: Vec<Region>,
    streams: Vec<(u32, u32, u32)>,
}

impl Dump {
    pub fn open(path: &std::path::Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path)?;
        let mut dump = Self { bytes, ranges: Vec::new(), regions: Vec::new(), streams: Vec::new() };
        if dump.u32(0) != Some(SIGNATURE) {
            return Err(Error::Format("bad signature"));
        }
        let count = dump.u32(8).ok_or(Error::Format("truncated header"))?;
        let directory = u64::from(dump.u32(12).ok_or(Error::Format("truncated header"))?);
        for i in 0..u64::from(count) {
            let at = directory + i * 12;
            let (Some(kind), Some(size), Some(rva)) = (dump.u32(at), dump.u32(at + 4), dump.u32(at + 8)) else { return Err(Error::Format("truncated directory")) };
            dump.streams.push((kind, size, rva));
        }
        dump.index_memory();
        Ok(dump)
    }

    fn u8(&self, at: u64) -> Option<u8> {
        self.bytes.get(usize::try_from(at).ok()?).copied()
    }

    fn u16(&self, at: u64) -> Option<u16> {
        Some(u16::from_le_bytes([self.u8(at)?, self.u8(at + 1)?]))
    }

    fn u32(&self, at: u64) -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        Some(u32::from_le_bytes(self.bytes.get(at..at + 4)?.try_into().ok()?))
    }

    fn u64(&self, at: u64) -> Option<u64> {
        let at = usize::try_from(at).ok()?;
        Some(u64::from_le_bytes(self.bytes.get(at..at + 8)?.try_into().ok()?))
    }

    fn stream(&self, kind: u32) -> Option<(u64, u64)> {
        self.streams.iter().find(|s| s.0 == kind).map(|s| (u64::from(s.2), u64::from(s.1)))
    }

    fn index_memory(&mut self) {
        if let Some((at, _)) = self.stream(MEMORY64_LIST)
            && let (Some(count), Some(base)) = (self.u64(at), self.u64(at + 8))
        {
            let mut offset = base;
            for i in 0..count.min(1 << 20) {
                let entry = at + 16 + i * 16;
                let (Some(start), Some(len)) = (self.u64(entry), self.u64(entry + 8)) else { break };
                self.ranges.push(Range { start, len, file_offset: offset });
                offset += len;
            }
        }
        if let Some((at, _)) = self.stream(MEMORY_LIST)
            && let Some(count) = self.u32(at)
        {
            for i in 0..u64::from(count) {
                let entry = at + 4 + i * 16;
                let (Some(start), Some(len), Some(rva)) = (self.u64(entry), self.u32(entry + 8), self.u32(entry + 12)) else { break };
                self.ranges.push(Range { start, len: u64::from(len), file_offset: u64::from(rva) });
            }
        }
        self.ranges.sort_by_key(|r| r.start);
        if let Some((at, _)) = self.stream(MEMORY_INFO_LIST)
            && let (Some(header), Some(entry_size), Some(count)) = (self.u32(at), self.u32(at + 4), self.u64(at + 8))
        {
            for i in 0..count.min(1 << 20) {
                let e = at + u64::from(header) + i * u64::from(entry_size);
                let (Some(base), Some(allocation_base), Some(size), Some(state), Some(protect), Some(kind)) = (self.u64(e), self.u64(e + 8), self.u64(e + 24), self.u32(e + 32), self.u32(e + 36), self.u32(e + 40)) else { break };
                self.regions.push(Region { known: true, base, size, allocation_base, state, protect, kind });
            }
            self.regions.sort_by_key(|r| r.base);
        }
    }

    /// The logger's record from its user stream, when the dump carries one of this version.
    #[must_use]
    pub fn record(&self) -> Option<Box<Record>> {
        let (at, size) = self.stream(record::STREAM_TYPE)?;
        if usize::try_from(size).ok() != Some(size_of::<Record>()) {
            return None;
        }
        let at = usize::try_from(at).ok()?;
        let bytes = self.bytes.get(at..at + size_of::<Record>())?;
        let mut rec = Box::new(Record::ZERO);
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), core::ptr::from_mut(&mut *rec).cast::<u8>(), size_of::<Record>()) };
        (rec.magic == record::MAGIC && rec.version == record::VERSION).then_some(rec)
    }

    /// A record built from the dump's own streams, for a dump without the logger's user stream:
    /// the exception and its context, the modules, the threads with their stack tops.
    #[must_use]
    pub fn reconstruct(&self) -> Box<Record> {
        let mut rec = Box::new(Record::ZERO);
        rec.magic = record::MAGIC;
        rec.version = record::VERSION;
        rec.symbols = crate::symbols::Mode::Ledger as u8;
        if let Some((at, _)) = self.stream(MODULE_LIST)
            && let Some(count) = self.u32(at)
        {
            for i in 0..(count as usize).min(MAX_MODULES) {
                let e = at + 4 + i as u64 * 108;
                let (Some(base), Some(size), Some(stamp), Some(name_rva)) = (self.u64(e), self.u32(e + 8), self.u32(e + 16), self.u32(e + 20)) else { break };
                let module = &mut rec.modules[i];
                *module = Module { base, size, timestamp: stamp, version: [0; 4], version_known: 0, path_len: 0, path: [0; PATH_LEN] };
                let (ms, ls, mn, ln) = (self.u32(e + 32), self.u32(e + 36), self.u32(e + 24), self.u32(e + 28));
                if let (Some(_), Some(ms), Some(ls)) = (mn.zip(ln), ms, ls) {
                    module.version = [(ms >> 16) as u16, (ms & 0xFFFF) as u16, (ls >> 16) as u16, (ls & 0xFFFF) as u16];
                    module.version_known = u8::from(ms != 0 || ls != 0);
                }
                if let Some(len) = self.u32(u64::from(name_rva)) {
                    let units: Vec<u16> = (0..(len / 2).min(u32::try_from(PATH_LEN).unwrap_or(u32::MAX))).filter_map(|k| self.u16(u64::from(name_rva) + 4 + u64::from(k) * 2)).collect();
                    module.path_len = u16::try_from(utf16_into(&units, &mut module.path)).unwrap_or(0);
                }
                rec.module_count = u32::try_from(i + 1).unwrap_or(0);
                if i == 0 {
                    rec.exe_base = base;
                }
            }
        }
        if let Some((at, _)) = self.stream(EXCEPTION) {
            rec.thread = self.u32(at).unwrap_or(0);
            rec.exception.code = self.u32(at + 8).unwrap_or(0);
            rec.exception.flags = self.u32(at + 12).unwrap_or(0);
            rec.exception.address = self.u64(at + 24).unwrap_or(0);
            rec.exception.parameter_count = self.u32(at + 32).unwrap_or(0);
            for (i, p) in rec.exception.parameters.iter_mut().enumerate() {
                *p = self.u64(at + 40 + i as u64 * 8).unwrap_or(0);
            }
            if let Some(ctx) = self.u32(at + 164) {
                rec.registers = self.registers(u64::from(ctx));
            }
            rec.unhandled = 1;
        }
        if let Some((at, _)) = self.stream(THREAD_LIST)
            && let Some(count) = self.u32(at)
        {
            for i in 0..(count as usize).min(MAX_THREADS) {
                let e = at + 4 + i as u64 * 48;
                let (Some(id), Some(stack_start), Some(stack_len), Some(ctx)) = (self.u32(e), self.u64(e + 16), self.u32(e + 24), self.u32(e + 36)) else { break };
                let registers = self.registers(u64::from(ctx));
                let mut frames = [Frame::default(); record::THREAD_FRAMES];
                let n = self.scan_stack(&rec, registers.rip, registers.rsp, &mut frames);
                let thread = &mut rec.threads[i];
                *thread = Thread { id, frame_count: 0, frames: [0; record::THREAD_FRAMES], rsp: registers.rsp, stack_base: stack_start, stack_end: stack_start + u64::from(stack_len), name_len: 0, name: [0; record::NAME_LEN] };
                for (dst, f) in thread.frames.iter_mut().zip(&frames[..n]) {
                    *dst = f.rip;
                }
                thread.frame_count = u32::try_from(n).unwrap_or(0);
                rec.thread_count = u32::try_from(i + 1).unwrap_or(0);
            }
        }
        let mut frames = [Frame::default(); MAX_FRAMES];
        let n = self.scan_stack(&rec, rec.registers.rip, rec.registers.rsp, &mut frames);
        rec.frames[..n].copy_from_slice(&frames[..n]);
        rec.frame_count = u32::try_from(n).unwrap_or(0);
        rec
    }

    /// Registers from a `CONTEXT` stored in the dump (the x64 layout: `Rax` at 0x78, `Rip` at 0xF8, `MxCsr` at 0x34, `EFlags` at 0x44, the XMM registers at 0x1A0).
    fn registers(&self, at: u64) -> record::Registers {
        let r = |o: u64| self.u64(at + o).unwrap_or(0);
        let mut xmm = [[0u64; 2]; 16];
        for (i, x) in xmm.iter_mut().enumerate() {
            *x = [r(0x1A0 + i as u64 * 16), r(0x1A8 + i as u64 * 16)];
        }
        record::Registers {
            rax: r(0x78),
            rcx: r(0x80),
            rdx: r(0x88),
            rbx: r(0x90),
            rsp: r(0x98),
            rbp: r(0xA0),
            rsi: r(0xA8),
            rdi: r(0xB0),
            r8: r(0xB8),
            r9: r(0xC0),
            r10: r(0xC8),
            r11: r(0xD0),
            r12: r(0xD8),
            r13: r(0xE0),
            r14: r(0xE8),
            r15: r(0xF0),
            rip: r(0xF8),
            eflags: self.u32(at + 0x44).unwrap_or(0),
            mxcsr: self.u32(at + 0x34).unwrap_or(0),
            segments: [self.u16(at + 0x38).unwrap_or(0), self.u16(at + 0x42).unwrap_or(0), self.u16(at + 0x3A).unwrap_or(0), self.u16(at + 0x3C).unwrap_or(0), self.u16(at + 0x3E).unwrap_or(0), self.u16(at + 0x40).unwrap_or(0)],
            xmm,
        }
    }

    /// Without the logger's frames, the return addresses on the stack that point into a module's
    /// executable memory, oldest last; a guess, marked as such by the report.
    fn scan_stack(&self, rec: &Record, rip: u64, rsp: u64, frames: &mut [Frame]) -> usize {
        let mut n = 0;
        if rip == 0 {
            return 0;
        }
        frames[n] = Frame { rip, rsp };
        n += 1;
        let mut at = rsp;
        while n < frames.len() {
            let Some(word) = self.read_u64(at) else { break };
            if rec.module_at(word).is_some() && self.region(word).executable() {
                frames[n] = Frame { rip: word, rsp: at };
                n += 1;
            }
            at += 8;
            if at - rsp > 0x4000 {
                break;
            }
        }
        n
    }
}

impl Source for Dump {
    fn read(&self, address: u64, buf: &mut [u8]) -> bool {
        if buf.is_empty() {
            return true;
        }
        let i = self.ranges.partition_point(|r| r.start <= address);
        let Some(range) = i.checked_sub(1).map(|i| &self.ranges[i]) else { return false };
        let Some(end) = address.checked_add(buf.len() as u64) else { return false };
        if end > range.start.saturating_add(range.len) {
            return false;
        }
        let Ok(at) = usize::try_from(range.file_offset + (address - range.start)) else { return false };
        match self.bytes.get(at..at + buf.len()) {
            Some(src) => {
                buf.copy_from_slice(src);
                true
            }
            None => false,
        }
    }

    fn region(&self, address: u64) -> Region {
        let i = self.regions.partition_point(|r| r.base <= address);
        match i.checked_sub(1).map(|i| self.regions[i]) {
            Some(r) if address < r.base.saturating_add(r.size) => r,
            _ => Region::default(),
        }
    }
}
