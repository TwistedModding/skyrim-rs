//! Code pages within rel32 reach of the game module, and `call`/`jmp rel32` and virtual call patching.

use core::ffi::c_void;
use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};

use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect,
    VirtualQuery,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::Error;

const PAGE: usize = 4096;
const GRANULARITY: usize = 0x10000;
const REACH: usize = i32::MAX as usize - 2 * PAGE;
const CALL: u8 = 0xE8;
const JMP: u8 = 0xE9;
const NOP: u8 = 0x90;
/// `call [rax+disp32]`, a virtual call through the vtable in rax.
const VCALL: [u8; 2] = [0xFF, 0x90];
/// `mov rax, [rcx]; jmp [rax+disp32]`: a virtual call's original, the slot of the first argument's vtable.
const VCALL_ORIGINAL: [u8; 5] = [0x48, 0x8B, 0x01, 0xFF, 0xA0];
/// `jmp [rip+0]`; the 8-byte target follows the instruction.
const ABS_JMP: [u8; 6] = [0xFF, 0x25, 0, 0, 0, 0];

struct Page {
    base: usize,
    used: usize,
}

/// Executable pages placed within rel32 reach of the code that refers to them, bump-allocated in 16-byte slots.
pub struct Trampoline {
    pages: Vec<Page>,
}

static SHARED: Mutex<Trampoline> = Mutex::new(Trampoline::new());

fn within_reach(from: usize, to: usize) -> bool {
    (to as i128 - from as i128).unsigned_abs() <= REACH as u128
}

fn rel32(next: usize, target: usize) -> Option<i32> {
    i32::try_from(target as i128 - next as i128).ok()
}

fn query(addr: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = unsafe { core::mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
    (unsafe { VirtualQuery(addr as *const c_void, &raw mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0).then_some(info)
}

fn alloc_at(addr: usize, size: usize) -> Option<usize> {
    let p = unsafe { VirtualAlloc(addr as *const c_void, size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) };
    (!p.is_null()).then_some(p as usize)
}

/// Commits `size` bytes (rounded up to whole pages) of executable memory at a free granule within
/// rel32 reach of `near`, searching downward from it and then upward.
pub fn alloc_near(near: usize, size: usize) -> Result<usize, Error> {
    let size = size.max(1).next_multiple_of(PAGE);
    let origin = near & !(GRANULARITY - 1);
    let mut addr = origin;
    while addr >= GRANULARITY && within_reach(near, addr) {
        let Some(info) = query(addr) else { break };
        if info.State == MEM_FREE
            && addr + size <= info.BaseAddress as usize + info.RegionSize
            && let Some(page) = alloc_at(addr, size)
        {
            return Ok(page);
        }
        addr = (info.BaseAddress as usize).saturating_sub(1) & !(GRANULARITY - 1);
    }
    addr = origin + GRANULARITY;
    while within_reach(near, addr + size) {
        let Some(info) = query(addr) else { break };
        let end = info.BaseAddress as usize + info.RegionSize;
        if info.State == MEM_FREE
            && addr + size <= end
            && let Some(page) = alloc_at(addr, size)
        {
            return Ok(page);
        }
        addr = end.next_multiple_of(GRANULARITY);
    }
    Err(io::Error::other(format!("no free {size:#x} bytes within rel32 reach of {near:#x}")).into())
}

impl Trampoline {
    #[must_use]
    pub const fn new() -> Self {
        Self { pages: Vec::new() }
    }

    /// The allocator `write_call` and `write_branch` draw their absolute jumps from.
    pub fn shared() -> MutexGuard<'static, Trampoline> {
        SHARED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copies `code` into a slot within rel32 reach of `near` and returns the slot's address.
    pub fn alloc(&mut self, near: usize, code: &[u8]) -> Result<usize, Error> {
        let len = code.len().next_multiple_of(16);
        assert!(len <= PAGE, "trampoline code must fit in one page");
        let fits = |p: &Page| p.used + len <= PAGE && within_reach(near, p.base);
        if !self.pages.iter().any(fits) {
            self.pages.push(Page { base: alloc_near(near, PAGE)?, used: 0 });
        }
        let page = self.pages.iter_mut().find(|p| fits(p)).unwrap();
        let slot = page.base + page.used;
        page.used += len;
        unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), slot as *mut u8, code.len()) };
        flush(slot, code.len());
        Ok(slot)
    }

    fn holds(&self, addr: usize) -> bool {
        self.pages.iter().any(|p| (p.base..p.base + p.used).contains(&addr))
    }

    /// Sees through an absolute jump this allocator emitted; any other address is returned as is.
    fn resolve(&self, addr: usize) -> usize {
        if self.holds(addr) && unsafe { core::slice::from_raw_parts(addr as *const u8, ABS_JMP.len()) } == ABS_JMP {
            unsafe { ((addr + ABS_JMP.len()) as *const usize).read_unaligned() }
        } else {
            addr
        }
    }

    /// The displacement of the virtual call whose original this allocator emitted at `addr`.
    fn virtual_slot(&self, addr: usize) -> Option<[u8; 4]> {
        let code = self.holds(addr).then(|| unsafe { (addr as *const [u8; 9]).read_unaligned() })?;
        (code[..5] == VCALL_ORIGINAL).then(|| [code[5], code[6], code[7], code[8]])
    }

    /// `target`, or an absolute jump to it within rel32 reach of `site`.
    fn reachable(&mut self, site: usize, target: usize) -> Result<usize, Error> {
        if rel32(site + 5, target).is_some() {
            return Ok(target);
        }
        let mut thunk = [0u8; 14];
        thunk[..6].copy_from_slice(&ABS_JMP);
        thunk[6..].copy_from_slice(&target.to_le_bytes());
        self.alloc(site, &thunk)
    }
}

impl Default for Trampoline {
    fn default() -> Self {
        Self::new()
    }
}

fn flush(addr: usize, len: usize) {
    unsafe { FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, len) };
}

fn rel32_target(site: usize) -> usize {
    let rel = unsafe { ((site + 1) as *const i32).read_unaligned() };
    (site + 5).wrapping_add_signed(rel as isize)
}

/// Address of the first `call rel32` in `start..start + len` whose target is `target`.
#[must_use]
pub fn find_call(start: usize, len: usize, target: usize) -> Option<usize> {
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, len) };
    code.iter().enumerate().take(len.saturating_sub(4)).find_map(|(i, &op)| {
        let site = start + i;
        (op == CALL && rel32_target(site) == target).then_some(site)
    })
}

unsafe fn write_rel32(site: usize, opcode: u8, target: usize) -> Result<usize, Error> {
    if unsafe { *(site as *const u8) } != opcode {
        return Err(Error::NotRel32(site));
    }
    let mut tramp = Trampoline::shared();
    let previous = tramp.resolve(rel32_target(site));
    let rel = rel32_to(site, tramp.reachable(site, target)?)?;
    unsafe { writable(site + 1, 4, || ((site + 1) as *mut i32).write_unaligned(rel)) }?;
    Ok(previous)
}

fn rel32_to(site: usize, reachable: usize) -> Result<i32, Error> {
    rel32(site + 5, reachable).ok_or_else(|| io::Error::other("trampoline page out of rel32 reach").into())
}

/// Writes the first 6 of the 8 bytes at `site` in one store.
unsafe fn write_six(site: usize, six: [u8; 6]) -> Result<(), Error> {
    let mut bytes = unsafe { (site as *const [u8; 8]).read_unaligned() };
    bytes[..6].copy_from_slice(&six);
    unsafe { writable(site, 8, || (site as *mut u64).write_unaligned(u64::from_le_bytes(bytes))) }
}

/// The length of the instruction at `site` a [`CallSite`] takes: 5 for a `call rel32` or
/// `jmp rel32`, 6 for a virtual call `call [rax+disp32]`.
#[must_use]
pub fn call_length(site: usize) -> Option<usize> {
    match unsafe { (site as *const [u8; 2]).read_unaligned() } {
        [CALL | JMP, _] => Some(5),
        VCALL => Some(6),
        _ => None,
    }
}

/// Whether the code before the virtual call at `site` loads rax from the first argument's object:
/// `mov rax, [rcx]`, or `mov rax, [r]; mov rcx, r` for a legacy register r.
fn calls_first_argument(site: usize) -> bool {
    let before = unsafe { ((site - 6) as *const [u8; 6]).read_unaligned() };
    let r = before[2];
    before[3..] == [0x48, 0x8B, 0x01] || (before[..2] == [0x48, 0x8B] && r < 8 && r != 4 && r != 5 && before[3..] == [0x48, 0x8B, 0xC8 | r])
}

/// Points the call or jump at `site` back at `original`; for a virtual call's original from
/// [`CallSite::create`], writes the virtual call back.
///
/// # Safety
/// Same as [`write_call`].
pub unsafe fn restore_call(site: usize, original: usize) -> Result<(), Error> {
    let slot = Trampoline::shared().virtual_slot(original);
    match slot {
        Some(d) => unsafe { write_six(site, [VCALL[0], VCALL[1], d[0], d[1], d[2], d[3]]) },
        None => unsafe { retarget(site, original) }.map(drop),
    }
}

/// Runs `write` with `len` bytes at `address` writable, then restores the protection and flushes the instruction cache.
unsafe fn writable(address: usize, len: usize, write: impl FnOnce()) -> Result<(), Error> {
    let mut old = 0;
    if unsafe { VirtualProtect(address as *const c_void, len, PAGE_EXECUTE_READWRITE, &raw mut old) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    write();
    unsafe { VirtualProtect(address as *const c_void, len, old, &raw mut old) };
    flush(address, len);
    Ok(())
}

/// Copies `bytes` over the code or read-only data at `address`.
///
/// # Safety
/// No thread may be executing or reading the bytes while they change, and the result must be valid where it lands.
pub unsafe fn write_code(address: usize, bytes: &[u8]) -> Result<(), Error> {
    unsafe { writable(address, bytes.len(), || core::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len())) }
}

/// Stores `value` in the pointer at `address` (a vtable slot) in one write and returns the previous value.
///
/// # Safety
/// `address` must be an aligned pointer slot whose readers accept `value` in the previous value's place.
pub unsafe fn write_pointer(address: usize, value: usize) -> Result<usize, Error> {
    let slot = unsafe { &*(address as *const core::sync::atomic::AtomicUsize) };
    let mut previous = 0;
    unsafe { writable(address, size_of::<usize>(), || previous = slot.swap(value, core::sync::atomic::Ordering::SeqCst)) }?;
    Ok(previous)
}

/// Points the `call rel32` at `site` to `target`, through an absolute jump when `target` is out of reach,
/// and returns the previous target.
///
/// # Safety
/// `site` must be a 5-byte `E8` instruction that no thread is executing, and `target` must be callable
/// in the previous target's place.
pub unsafe fn write_call(site: usize, target: usize) -> Result<usize, Error> {
    unsafe { write_rel32(site, CALL, target) }
}

/// Same as [`write_call`] for a `jmp rel32` (`E9`).
///
/// # Safety
/// Same as [`write_call`].
pub unsafe fn write_branch(site: usize, target: usize) -> Result<usize, Error> {
    unsafe { write_rel32(site, JMP, target) }
}

/// Copies `code` near `site` followed by a jump to `site + len`, and returns the `len` bytes that
/// lead there from `site`: a `jmp rel32` padded with `nop`s. Nothing is written at `site`.
pub fn cave(site: usize, len: usize, code: &[u8]) -> Result<Vec<u8>, Error> {
    if len < 5 {
        return Err(io::Error::other(format!("a jump to a cave needs 5 bytes, not {len}")).into());
    }
    let mut body = code.to_vec();
    body.extend(ABS_JMP);
    body.extend((site + len).to_le_bytes());
    let rel = rel32_to(site, Trampoline::shared().alloc(site, &body)?)?;
    let mut bytes = vec![JMP];
    bytes.extend(rel.to_le_bytes());
    bytes.resize(len, NOP);
    Ok(bytes)
}

/// [`write_call`] or [`write_branch`], whichever instruction `site` holds.
///
/// # Safety
/// Same as [`write_call`].
pub unsafe fn retarget(site: usize, target: usize) -> Result<usize, Error> {
    match unsafe { *(site as *const u8) } {
        op @ (CALL | JMP) => unsafe { write_rel32(site, op, target) },
        _ => Err(Error::NotRel32(site)),
    }
}

/// A redirected `call rel32`, or a tail `jmp rel32` to a function, which reaches the detour with
/// the caller's arguments and return address as a call reaches its callee. `F` is the
/// `extern "C" fn` type shared by the callee and the detour.
///
/// A virtual call `call [rax+disp32]` (`FF 90`) whose rax holds the vtable of the first argument
/// (`mov rax, [rcx]` before it, or `mov rax, [r]; mov rcx, r`) is taken too: its 6 bytes become a
/// `call rel32` and a `nop`, and the original reads the slot from the first argument's vtable
/// again, so every object still reaches its own function.
pub struct CallSite<F: Copy> {
    site: usize,
    /// What the call reaches: the detour, or a stub that hands it a register first.
    entry: usize,
    original: F,
}

impl<F: Copy> CallSite<F> {
    /// Redirects the call at `site` to `detour`: [`create`](Self::create) and [`enable`](Self::enable) in one step.
    ///
    /// # Safety
    /// Same as [`write_call`]; `F` must be a function pointer type matching the callee.
    pub unsafe fn install(site: usize, detour: F) -> Result<Self, Error> {
        let hook = unsafe { Self::create(site, detour) }?;
        unsafe { hook.enable() }?;
        Ok(hook)
    }

    /// Reads the callee at `site` without writing anything, so the hook can be stored where the
    /// detour finds [`original`](Self::original) before [`enable`](Self::enable) redirects the call.
    ///
    /// # Safety
    /// `F` must be a function pointer type matching the callee.
    pub unsafe fn create(site: usize, detour: F) -> Result<Self, Error> {
        const { assert!(size_of::<F>() == size_of::<usize>(), "F must be a function pointer type") };
        let previous = match call_length(site) {
            Some(5) => Trampoline::shared().resolve(rel32_target(site)),
            Some(_) if calls_first_argument(site) => {
                let mut code = [0u8; 9];
                code[..5].copy_from_slice(&VCALL_ORIGINAL);
                code[5..].copy_from_slice(&unsafe { ((site + 2) as *const [u8; 4]).read_unaligned() });
                Trampoline::shared().alloc(site, &code)?
            }
            _ => return Err(Error::NotRel32(site)),
        };
        Ok(Self { site, entry: unsafe { core::mem::transmute_copy(&detour) }, original: unsafe { core::mem::transmute_copy(&previous) } })
    }

    /// Hands the caller's register `from` to the detour in the argument register `to` (x86
    /// register numbers: rcx 1, rdx 2, rbx 3, rsi 6, r8 8), through a stub near the site that the
    /// call reaches first. The callee must take no argument in `to`.
    ///
    /// # Safety
    /// The detour must be of `F` with that argument added; `from` and `to` must be below 16.
    pub unsafe fn passing(self, from: u8, to: u8) -> Result<Self, Error> {
        let mut code = vec![0x48 | ((from >> 3) << 2) | (to >> 3), 0x89, 0xC0 | ((from & 7) << 3) | (to & 7)];
        code.extend(ABS_JMP);
        code.extend(self.entry.to_le_bytes());
        let entry = Trampoline::shared().alloc(self.site, &code)?;
        Ok(Self { entry, ..self })
    }

    /// Points the site at the detour.
    ///
    /// # Safety
    /// Same as [`write_call`].
    pub unsafe fn enable(&self) -> Result<(), Error> {
        if call_length(self.site) != Some(6) {
            return unsafe { retarget(self.site, self.entry) }.map(drop);
        }
        let rel = rel32_to(self.site, Trampoline::shared().reachable(self.site, self.entry)?)?.to_le_bytes();
        unsafe { write_six(self.site, [CALL, rel[0], rel[1], rel[2], rel[3], NOP]) }
    }

    /// What the call reaches once enabled: the detour, or the stub of [`passing`](Self::passing).
    #[must_use]
    pub fn entry(&self) -> usize {
        self.entry
    }

    /// The callee the site reached before, callable from the detour.
    pub fn original(&self) -> F {
        self.original
    }

    #[must_use]
    pub fn site(&self) -> usize {
        self.site
    }

    /// Points the site back at the original callee.
    ///
    /// # Safety
    /// Same as [`write_call`].
    pub unsafe fn remove(self) -> Result<(), Error> {
        let original: usize = unsafe { core::mem::transmute_copy(&self.original) };
        unsafe { restore_call(self.site, original) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fn0 = extern "C" fn() -> u32;

    extern "C" fn one() -> u32 {
        1
    }

    extern "C" fn two() -> u32 {
        2
    }

    static SITE: std::sync::OnceLock<CallSite<Fn0>> = std::sync::OnceLock::new();

    extern "C" fn site_detour() -> u32 {
        SITE.get().unwrap().original()() + 100
    }

    fn func(addr: usize) -> Fn0 {
        unsafe { core::mem::transmute::<usize, Fn0>(addr) }
    }

    /// `mov eax, 3; ret` in the low 4 GB, out of rel32 reach of this executable.
    fn far_three() -> usize {
        let page = (0x1000_0000..0x8000_0000).step_by(0x100_0000).find_map(|a| alloc_at(a, PAGE)).unwrap();
        unsafe { core::ptr::copy_nonoverlapping([0xB8, 3, 0, 0, 0, 0xC3].as_ptr(), page as *mut u8, 6) };
        flush(page, 6);
        page
    }

    #[test]
    fn call_site_round_trips() {
        let one_addr = one as Fn0 as usize;
        let two_addr = two as Fn0 as usize;
        let stub = [0x48, 0x83, 0xEC, 0x28, CALL, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x28, 0xC3];
        let stub_addr = Trampoline::shared().alloc(one_addr, &stub).unwrap();
        assert_eq!(stub_addr % 16, 0);
        let site = stub_addr + 4;
        assert_eq!(unsafe { write_call(site, one_addr) }.unwrap(), site + 5);
        assert_eq!(func(stub_addr)(), 1);
        assert_eq!(find_call(stub_addr, stub.len(), one_addr), Some(site));
        assert_eq!(find_call(stub_addr, stub.len(), two_addr), None);

        assert_eq!(unsafe { write_call(site, two_addr) }.unwrap(), one_addr);
        assert_eq!(func(stub_addr)(), 2);

        let far = far_three();
        assert!(rel32(site + 5, far).is_none());
        assert_eq!(unsafe { write_call(site, far) }.unwrap(), two_addr);
        assert_eq!(func(stub_addr)(), 3);
        assert_eq!(unsafe { write_call(site, two_addr) }.unwrap(), far);

        let hook = unsafe { CallSite::install(site, site_detour as Fn0) }.unwrap();
        assert_eq!(hook.site(), site);
        assert!(SITE.set(hook).is_ok());
        assert_eq!(func(stub_addr)(), 102);
        assert_eq!(unsafe { write_call(site, two_addr) }.unwrap(), site_detour as Fn0 as usize);
        assert_eq!(func(stub_addr)(), 2);
    }

    #[test]
    fn code_and_pointer_writes_land() {
        let stub = Trampoline::shared().alloc(one as Fn0 as usize, &[0xB8, 7, 0, 0, 0, 0xC3]).unwrap();
        assert_eq!(func(stub)(), 7);
        unsafe { write_code(stub + 1, &[9]) }.unwrap();
        assert_eq!(func(stub)(), 9);
        let table = [one as Fn0 as usize, two as Fn0 as usize];
        assert_eq!(unsafe { write_pointer(table.as_ptr() as usize + 8, stub) }.unwrap(), two as Fn0 as usize);
        assert_eq!(func(core::hint::black_box(&table)[1])(), 9);
    }

    #[test]
    fn branch_site_round_trips() {
        let one_addr = one as Fn0 as usize;
        let two_addr = two as Fn0 as usize;
        let site = Trampoline::shared().alloc(one_addr, &[JMP, 0, 0, 0, 0]).unwrap();
        assert_eq!(unsafe { write_branch(site, one_addr) }.unwrap(), site + 5);
        assert_eq!(func(site)(), 1);
        assert_eq!(unsafe { write_branch(site, two_addr) }.unwrap(), one_addr);
        assert_eq!(func(site)(), 2);
        assert!(matches!(unsafe { write_call(site, one_addr) }, Err(Error::NotRel32(s)) if s == site));
    }

    static TAIL: std::sync::OnceLock<CallSite<Fn0>> = std::sync::OnceLock::new();

    extern "C" fn tail_detour() -> u32 {
        TAIL.get().unwrap().original()() + 10
    }

    #[test]
    fn tail_jump_site_round_trips() {
        let one_addr = one as Fn0 as usize;
        let site = Trampoline::shared().alloc(one_addr, &[JMP, 0, 0, 0, 0]).unwrap();
        unsafe { write_branch(site, one_addr) }.unwrap();
        let hook = unsafe { CallSite::install(site, tail_detour as Fn0) }.unwrap();
        assert!(TAIL.set(hook).is_ok());
        assert_eq!(func(site)(), 11, "the jump reaches the detour, which returns to the jump's caller");
        assert_eq!(unsafe { *(site as *const u8) }, JMP, "the opcode is kept");
        assert_eq!(unsafe { retarget(site, one_addr) }.unwrap(), tail_detour as Fn0 as usize);
        assert_eq!(func(site)(), 1);
        let nops = Trampoline::shared().alloc(one_addr, &[0x90; 5]).unwrap();
        assert!(matches!(unsafe { retarget(nops, one_addr) }, Err(Error::NotRel32(s)) if s == nops));
    }

    type Fn1 = extern "C" fn(usize) -> usize;
    type Fn2 = extern "C" fn(usize, usize) -> usize;

    extern "C" fn first_slot(_: usize) -> usize {
        1
    }

    extern "C" fn second_slot(_: usize) -> usize {
        2
    }

    /// An object of a vtable whose slot 1 is `slot`.
    fn object(slot: Fn1) -> usize {
        let table: &'static [usize; 2] = Box::leak(Box::new([0, slot as usize]));
        core::ptr::from_ref::<usize>(Box::leak(Box::new(table.as_ptr() as usize))) as usize
    }

    static VIRTUAL: std::sync::OnceLock<CallSite<Fn1>> = std::sync::OnceLock::new();

    extern "C" fn virtual_detour(this: usize) -> usize {
        VIRTUAL.get().unwrap().original()(this) + 100
    }

    #[test]
    fn virtual_call_site_round_trips() {
        let near = one as Fn0 as usize;
        // sub rsp, 0x28; mov rax, [rcx]; call [rax+8]; add rsp, 0x28; ret
        let code = [0x48, 0x83, 0xEC, 0x28, 0x48, 0x8B, 0x01, 0xFF, 0x90, 8, 0, 0, 0, 0x48, 0x83, 0xC4, 0x28, 0xC3];
        let caller = Trampoline::shared().alloc(near, &code).unwrap();
        let site = caller + 7;
        let call = |this| unsafe { core::mem::transmute::<usize, Fn1>(caller) }(this);
        let (a, b) = (object(first_slot), object(second_slot));
        assert_eq!((call(a), call(b), call_length(site)), (1, 2, Some(6)));
        let hook = unsafe { CallSite::create(site, virtual_detour as Fn1) }.unwrap();
        assert!(VIRTUAL.set(hook).is_ok());
        unsafe { VIRTUAL.get().unwrap().enable() }.unwrap();
        assert_eq!(unsafe { core::slice::from_raw_parts(site as *const u8, 6) }[5], NOP);
        assert_eq!((call(a), call(b)), (101, 102), "each object reaches its own slot through the original");
        assert_eq!(rel32_target(site), virtual_detour as Fn1 as usize);
        unsafe { restore_call(site, VIRTUAL.get().unwrap().original() as usize) }.unwrap();
        assert_eq!(unsafe { core::slice::from_raw_parts(site as *const u8, 6) }, [0xFF, 0x90, 8, 0, 0, 0]);
        assert_eq!((call(a), call(b)), (1, 2));

        // push rsi; sub rsp, 0x20; mov rsi, rcx; mov rax, [rsi]; mov rcx, rsi; call [rax+8]; add rsp, 0x20; pop rsi; ret
        let through_rsi = [0x56, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x89, 0xCE, 0x48, 0x8B, 0x06, 0x48, 0x8B, 0xCE, 0xFF, 0x90, 8, 0, 0, 0, 0x48, 0x83, 0xC4, 0x20, 0x5E, 0xC3];
        let caller = Trampoline::shared().alloc(near, &through_rsi).unwrap();
        assert!(unsafe { CallSite::create(caller + 14, virtual_detour as Fn1) }.is_ok());
        // mov rax, [rdx]; call [rax+8]: rax is not the first argument's vtable
        let other = Trampoline::shared().alloc(near, &[0x90, 0x90, 0x90, 0x48, 0x8B, 0x02, 0xFF, 0x90, 8, 0, 0, 0]).unwrap();
        assert!(matches!(unsafe { CallSite::create(other + 6, virtual_detour as Fn1) }, Err(Error::NotRel32(_))));
    }

    #[test]
    fn a_cave_runs_its_code_and_returns_after_the_bytes() {
        // mov eax, 1; add eax, 2; ret
        let site = Trampoline::shared().alloc(one as Fn0 as usize, &[0xB8, 1, 0, 0, 0, 0x83, 0xC0, 0x02, 0xC3]).unwrap();
        assert_eq!(func(site)(), 3);
        let jump = cave(site, 5, &[0xB8, 10, 0, 0, 0]).unwrap();
        assert_eq!((jump.len(), jump[0]), (5, JMP));
        unsafe { write_code(site, &jump) }.unwrap();
        assert_eq!(func(site)(), 12, "the cave's mov, then the add after the replaced bytes");
        assert_eq!(cave(site, 8, &[0x90]).unwrap()[5..], [NOP; 3]);
        assert!(cave(site, 4, &[0x90]).is_err());
    }

    extern "C" fn plus_one(x: usize) -> usize {
        x + 1
    }

    static PASSING: std::sync::OnceLock<CallSite<Fn2>> = std::sync::OnceLock::new();

    extern "C" fn passing_detour(x: usize, rbx: usize) -> usize {
        PASSING.get().unwrap().original()(x, rbx) + rbx * 10
    }

    #[test]
    fn a_call_site_hands_the_detour_a_register() {
        // push rbx; sub rsp, 0x20; mov rbx, rdx; call plus_one; add rsp, 0x20; pop rbx; ret
        let code = [0x53, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x89, 0xD3, CALL, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x20, 0x5B, 0xC3];
        let caller = Trampoline::shared().alloc(plus_one as Fn1 as usize, &code).unwrap();
        let site = caller + 8;
        unsafe { write_call(site, plus_one as Fn1 as usize) }.unwrap();
        let call = |x, y| unsafe { core::mem::transmute::<usize, Fn2>(caller) }(x, y);
        assert_eq!(call(5, 7), 6);
        let hook = unsafe { CallSite::create(site, passing_detour as Fn2).and_then(|h| h.passing(3, 2)) }.unwrap();
        assert_ne!(hook.entry(), passing_detour as Fn2 as usize, "the call reaches the stub");
        assert!(PASSING.set(hook).is_ok());
        unsafe { PASSING.get().unwrap().enable() }.unwrap();
        assert_eq!(call(5, 7), 76, "the caller's rbx arrives as the second argument");
        unsafe { restore_call(site, plus_one as Fn1 as usize) }.unwrap();
        assert_eq!(call(5, 7), 6);
    }
}
