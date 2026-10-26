//! What the watchdog reads from the live process into the record: the module list from the
//! loader's own list in the PEB (no loader lock, which the crashing thread may hold), the
//! crashing thread's frames through the unwinder, every other thread's top frames from a paused
//! context, the thread names, and the game's state through guarded reads.

use core::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, SYSTEMTIME};
use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_FULL_AMD64, EXCEPTION_RECORD, GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetThreadDescription, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME};

use crate::mem::{Live, Source, stack_extent};
use crate::record::{Exception, Frame, Game, MAX_MODULES, MAX_THREADS, Module, NAME_LEN, PATH_LEN, Record, Registers, THREAD_FRAMES, Thread};
use crate::text::utf16_into;

/// The exception Windows raises to name a thread (`MS_VC_EXCEPTION`): the parameters hold
/// `0x1000`, the name pointer, the thread id (-1 for the caller) and flags.
pub const THREAD_NAME_EXCEPTION: u32 = 0x406D_1388;

const NAMES: usize = 256;

struct Name {
    thread: AtomicU32,
    len: AtomicU32,
    text: core::cell::UnsafeCell<[u8; NAME_LEN]>,
}

unsafe impl Sync for Name {}

static NAMES_TABLE: [Name; NAMES] = [const { Name { thread: AtomicU32::new(0), len: AtomicU32::new(0), text: core::cell::UnsafeCell::new([0; NAME_LEN]) } }; NAMES];

/// Records the name a thread-name exception carries.
pub(crate) unsafe fn note_thread_name(record: &EXCEPTION_RECORD, raiser: u32) {
    if record.NumberParameters < 3 || record.ExceptionInformation[0] != 0x1000 {
        return;
    }
    let pointer = record.ExceptionInformation[1] as u64;
    let thread = match u32::try_from(record.ExceptionInformation[2] & 0xFFFF_FFFF).unwrap_or(0) {
        0xFFFF_FFFF | 0 => raiser,
        id => id,
    };
    let mut text = [0u8; NAME_LEN];
    let Some(len) = Live.read_cstr(pointer, &mut text) else { return };
    for slot in &NAMES_TABLE {
        let current = slot.thread.load(Ordering::Acquire);
        if current == thread || (current == 0 && slot.thread.compare_exchange(0, thread, Ordering::AcqRel, Ordering::Acquire).is_ok()) {
            unsafe { (*slot.text.get()).copy_from_slice(&text) };
            slot.len.store(u32::try_from(len).unwrap_or(0), Ordering::Release);
            return;
        }
    }
}

/// The name a thread-name exception gave `thread`, into `out`; the bytes stored.
fn noted_name(thread: u32, out: &mut [u8; NAME_LEN]) -> usize {
    for slot in &NAMES_TABLE {
        if slot.thread.load(Ordering::Acquire) == thread {
            let len = (slot.len.load(Ordering::Acquire) as usize).min(NAME_LEN);
            out.copy_from_slice(unsafe { &*slot.text.get() });
            return len;
        }
    }
    0
}

/// The thread's description (`SetThreadDescription`, which Rust's `thread::Builder::name` uses)
/// or the name an exception gave it.
fn thread_name(handle: isize, thread: u32, out: &mut [u8; NAME_LEN]) -> usize {
    let mut description: *mut u16 = core::ptr::null_mut();
    if handle != 0 && unsafe { GetThreadDescription(handle as _, &raw mut description) } >= 0 && !description.is_null() {
        let mut len = 0;
        while len < 256 && unsafe { *description.add(len) } != 0 {
            len += 1;
        }
        let units = unsafe { core::slice::from_raw_parts(description, len) };
        let stored = utf16_into(units, out);
        unsafe { LocalFree(description.cast()) };
        if stored > 0 {
            return stored;
        }
    }
    noted_name(thread, out)
}

pub(crate) fn local_time() -> [u16; 6] {
    let mut t: SYSTEMTIME = unsafe { core::mem::zeroed() };
    unsafe { GetLocalTime(&raw mut t) };
    [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond]
}

pub(crate) fn exception(record: &EXCEPTION_RECORD) -> Exception {
    let mut parameters = [0u64; 15];
    for (p, v) in parameters.iter_mut().zip(record.ExceptionInformation) {
        *p = v as u64;
    }
    Exception { code: record.ExceptionCode.cast_unsigned(), flags: record.ExceptionFlags, address: record.ExceptionAddress as u64, parameter_count: record.NumberParameters, parameters }
}

pub(crate) fn registers(ctx: &CONTEXT) -> Registers {
    let xmm = unsafe { ctx.Anonymous.FltSave.XmmRegisters };
    Registers {
        rax: ctx.Rax,
        rcx: ctx.Rcx,
        rdx: ctx.Rdx,
        rbx: ctx.Rbx,
        rsp: ctx.Rsp,
        rbp: ctx.Rbp,
        rsi: ctx.Rsi,
        rdi: ctx.Rdi,
        r8: ctx.R8,
        r9: ctx.R9,
        r10: ctx.R10,
        r11: ctx.R11,
        r12: ctx.R12,
        r13: ctx.R13,
        r14: ctx.R14,
        r15: ctx.R15,
        rip: ctx.Rip,
        eflags: ctx.EFlags,
        mxcsr: ctx.MxCsr,
        segments: [ctx.SegCs, ctx.SegSs, ctx.SegDs, ctx.SegEs, ctx.SegFs, ctx.SegGs],
        xmm: core::array::from_fn(|i| [xmm[i].Low, xmm[i].High.cast_unsigned()]),
    }
}

/// Walks the frames above `ctx` with the unwinder, a leaf step where no function table covers
/// the address, stopping at an unreadable stack or a return into nothing.
pub(crate) fn walk(ctx: &CONTEXT, frames: &mut [Frame]) -> usize {
    let mut ctx: CONTEXT = *ctx;
    let mut n = 0;
    while n < frames.len() {
        frames[n] = Frame { rip: ctx.Rip, rsp: ctx.Rsp };
        n += 1;
        if ctx.Rip == 0 || !Live.region(ctx.Rsp).readable() {
            break;
        }
        let before = ctx.Rsp;
        let mut base = 0u64;
        let entry = unsafe { RtlLookupFunctionEntry(ctx.Rip, &raw mut base, core::ptr::null_mut()) };
        if entry.is_null() {
            let Some(ret) = Live.read_u64(ctx.Rsp) else { break };
            ctx.Rip = ret;
            ctx.Rsp += 8;
        } else {
            let mut handler_data = core::ptr::null_mut();
            let mut establisher = 0u64;
            unsafe { RtlVirtualUnwind(0, base, ctx.Rip, entry, &raw mut ctx, &raw mut handler_data, &raw mut establisher, core::ptr::null_mut()) };
        }
        if ctx.Rip == 0 || ctx.Rsp <= before {
            break;
        }
    }
    n
}

/// The loaded modules from the loader's in-memory-order list (`winternl.h`: `PEB` +0x18 `Ldr`,
/// `PEB_LDR_DATA` +0x20 `InMemoryOrderModuleList`, each `LDR_DATA_TABLE_ENTRY` 0x10 before its
/// link with `DllBase` +0x30 and `FullDllName` +0x48), sizes and timestamps from each PE header.
pub(crate) fn fill_modules(rec: &mut Record) {
    let src = Live;
    let peb: u64;
    unsafe { core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly, preserves_flags)) };
    let Some(ldr) = src.read_u64(peb + 0x18) else { return };
    let head = ldr + 0x20;
    let Some(mut link) = src.read_u64(head) else { return };
    let mut count = 0;
    while link != head && link != 0 && count < MAX_MODULES {
        let entry = link - 0x10;
        let (Some(base), Some(name_len), Some(name_ptr)) = (src.read_u64(entry + 0x30), src.read_u16(entry + 0x48), src.read_u64(entry + 0x50)) else { break };
        let module = &mut rec.modules[count];
        *module = Module { base, size: 0, timestamp: 0, version: [0; 4], version_known: 0, path_len: 0, path: [0; PATH_LEN] };
        if let Some(nt) = src.read_u32(base + 0x3c) {
            module.size = src.read_u32(base + u64::from(nt) + 0x50).unwrap_or(0);
            module.timestamp = src.read_u32(base + u64::from(nt) + 8).unwrap_or(0);
        }
        let units = (name_len as usize / 2).min(260);
        let mut wide = [0u16; 260];
        let bytes = unsafe { core::slice::from_raw_parts_mut(wide.as_mut_ptr().cast::<u8>(), units * 2) };
        if src.read(name_ptr, bytes) {
            module.path_len = u16::try_from(utf16_into(&wide[..units], &mut module.path)).unwrap_or(0);
        }
        count += 1;
        let Some(next) = src.read_u64(link) else { break };
        link = next;
    }
    rec.module_count = u32::try_from(count).unwrap_or(0);
}

/// Every thread of the process with its top frames; `crashing` is taken from its saved context,
/// the watchdog itself is left out, the rest are paused for the read.
pub(crate) fn fill_threads(rec: &mut Record, crashing: (u32, &CONTEXT), watchdog: u32) {
    let pid = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        rec.thread_count = u32::MAX;
        return;
    }
    let mut entry: THREADENTRY32 = unsafe { core::mem::zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(0);
    let mut count = 0;
    let mut ok = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
    while ok && count < MAX_THREADS {
        if entry.th32OwnerProcessID == pid && entry.th32ThreadID != watchdog {
            let id = entry.th32ThreadID;
            let mut ctx: CONTEXT = unsafe { core::mem::zeroed() };
            let handle = unsafe { OpenThread(THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION, 0, id) };
            let have_context = if id == crashing.0 {
                ctx = *crashing.1;
                true
            } else if handle.is_null() {
                false
            } else {
                ctx.ContextFlags = CONTEXT_FULL_AMD64;
                let suspended = unsafe { SuspendThread(handle) } != u32::MAX;
                let got = suspended && unsafe { GetThreadContext(handle, &raw mut ctx) } != 0;
                if suspended {
                    unsafe { ResumeThread(handle) };
                }
                got
            };
            let thread = &mut rec.threads[count];
            *thread = Thread { id, frame_count: 0, frames: [0; THREAD_FRAMES], rsp: 0, stack_base: 0, stack_end: 0, name_len: 0, name: [0; NAME_LEN] };
            thread.name_len = u8::try_from(thread_name(handle as isize, id, &mut thread.name)).unwrap_or(0);
            if have_context {
                let mut frames = [Frame::default(); THREAD_FRAMES];
                let n = walk(&ctx, &mut frames);
                for (dst, f) in thread.frames.iter_mut().zip(&frames[..n]) {
                    *dst = f.rip;
                }
                thread.frame_count = u32::try_from(n).unwrap_or(0);
                thread.rsp = ctx.Rsp;
                if let Some((base, end)) = stack_extent(&Live, ctx.Rsp) {
                    thread.stack_base = base;
                    thread.stack_end = end;
                }
            }
            if !handle.is_null() {
                unsafe { CloseHandle(handle) };
            }
            count += 1;
        }
        ok = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    rec.thread_count = u32::try_from(count).unwrap_or(0);
}

/// The game globals the probes read, as absolute addresses resolved at install (0 when unknown).
#[derive(Clone, Copy, Default)]
pub struct GameAddresses {
    pub frame_counter: u64,
    pub player: u64,
    pub save_manager: u64,
}

/// The `char*` to the current save name sits 0x240 into the save manager.
const SAVE_NAME: u64 = 0x240;
/// `TESObjectREFR::parentCell` +0x60; `TESObjectCELL`'s `TESFullName` string at +0x28; the player's worldspace at +0x638.
const PARENT_CELL: u64 = 0x60;
const CELL_NAME: u64 = 0x28;
const PLAYER_WORLDSPACE: u64 = 0x638;
const FORM_ID: u64 = 0x14;
const POSITION: u64 = 0x54;

pub(crate) fn fill_game(game: &mut Game, addresses: &GameAddresses) {
    let src = Live;
    if let Some(frame) = (addresses.frame_counter != 0).then(|| src.read_u32(addresses.frame_counter)).flatten() {
        game.frame = frame;
        game.frame_known = 1;
    }
    if let Some(name) = (addresses.save_manager != 0).then(|| src.read_u64(addresses.save_manager)).flatten().filter(|&m| m != 0).and_then(|m| src.read_u64(m + SAVE_NAME)) {
        game.save_len = src.read_cstr(name, &mut game.save).map_or(0, |n| u8::try_from(n).unwrap_or(u8::MAX));
    }
    let Some(player) = (addresses.player != 0).then(|| src.read_u64(addresses.player)).flatten().filter(|&p| p != 0) else { return };
    for (i, p) in game.position.iter_mut().enumerate() {
        *p = src.read_f32(player + POSITION + 4 * i as u64).unwrap_or(f32::NAN);
    }
    if let Some(cell) = src.read_u64(player + PARENT_CELL).filter(|&c| c != 0) {
        if let Some(id) = src.read_u32(cell + FORM_ID) {
            game.cell_id = id;
            game.cell_known = 1;
        }
        if let Some(name) = src.read_u64(cell + CELL_NAME).filter(|&n| n != 0) {
            game.cell_name_len = src.read_cstr(name, &mut game.cell_name).map_or(0, |n| u8::try_from(n).unwrap_or(u8::MAX));
        }
    }
    if let Some(id) = src.read_u64(player + PLAYER_WORLDSPACE).filter(|&w| w != 0).and_then(|w| src.read_u32(w + FORM_ID)) {
        game.worldspace_id = id;
        game.worldspace_known = 1;
    }
}

pub(crate) fn last_error() -> u32 {
    unsafe { GetLastError() }
}
