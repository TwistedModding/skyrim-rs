//! Code pages within rel32 reach of the game module and `call`/`jmp rel32` patching.

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

    /// Sees through an absolute jump this allocator emitted; any other address is returned as is.
    fn resolve(&self, addr: usize) -> usize {
        let ours = self.pages.iter().any(|p| (p.base..p.base + p.used).contains(&addr));
        if ours && unsafe { core::slice::from_raw_parts(addr as *const u8, ABS_JMP.len()) } == ABS_JMP {
            unsafe { ((addr + ABS_JMP.len()) as *const usize).read_unaligned() }
        } else {
            addr
        }
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
    let next = site + 5;
    let reachable = if rel32(next, target).is_some() {
        target
    } else {
        let mut thunk = [0u8; 14];
        thunk[..6].copy_from_slice(&ABS_JMP);
        thunk[6..].copy_from_slice(&target.to_le_bytes());
        tramp.alloc(site, &thunk)?
    };
    let rel = rel32(next, reachable).ok_or_else(|| io::Error::other("trampoline page out of rel32 reach"))?;
    let mut old = 0;
    if unsafe { VirtualProtect(site as *const c_void, 5, PAGE_EXECUTE_READWRITE, &raw mut old) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    unsafe { ((site + 1) as *mut i32).write_unaligned(rel) };
    unsafe { VirtualProtect(site as *const c_void, 5, old, &raw mut old) };
    flush(site, 5);
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

/// A redirected `call rel32`. `F` is the `extern "C" fn` type shared by the callee and the detour.
pub struct CallSite<F: Copy> {
    site: usize,
    detour: usize,
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
        if unsafe { *(site as *const u8) } != CALL {
            return Err(Error::NotRel32(site));
        }
        let previous = Trampoline::shared().resolve(rel32_target(site));
        Ok(Self { site, detour: unsafe { core::mem::transmute_copy(&detour) }, original: unsafe { core::mem::transmute_copy(&previous) } })
    }

    /// Points the site at the detour.
    ///
    /// # Safety
    /// Same as [`write_call`].
    pub unsafe fn enable(&self) -> Result<(), Error> {
        unsafe { write_call(self.site, self.detour) }.map(drop)
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
        unsafe { write_call(self.site, original) }.map(drop)
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
}
