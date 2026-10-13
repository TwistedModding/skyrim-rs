//! Crash reporting for the runs: a vectored exception handler that logs the exception code, the
//! faulting address as module plus offset, the thread and the return addresses on the stack that
//! fall inside a module, then lets the exception continue; and a panic hook that logs the panic
//! before `panic = "abort"` ends the process. Nothing here changes what the game does.

use std::fmt::Write as _;

use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, HMODULE};
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// `module!offset` for an address inside a loaded module, or the bare address.
fn locate(address: usize) -> String {
    let mut module: HMODULE = core::ptr::null_mut();
    let found = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, address as *const u16, &raw mut module) } != 0;
    if !found || module.is_null() {
        return format!("{address:#x}");
    }
    let mut name = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&name[..len]);
    let file = path.rsplit('\\').next().unwrap_or(&path);
    format!("{file}+{:#x}", address - module as usize)
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let Some(info) = (unsafe { info.as_ref() }) else { return EXCEPTION_CONTINUE_SEARCH };
    let Some(record) = (unsafe { info.ExceptionRecord.as_ref() }) else { return EXCEPTION_CONTINUE_SEARCH };
    // Error-class codes only; the C++ exception code and the thread-name and breakpoint codes are normal traffic.
    let code = record.ExceptionCode as u32;
    if code < 0xC000_0000 || code == 0xE06D_7363 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let mut line = format!("CRASH code {:#x} at {} on thread {}", record.ExceptionCode, locate(record.ExceptionAddress as usize), unsafe { GetCurrentThreadId() });
    if record.ExceptionCode == EXCEPTION_ACCESS_VIOLATION && record.NumberParameters >= 2 {
        let _ = write!(line, " ({} {:#x})", if record.ExceptionInformation[0] == 0 { "reading" } else { "writing" }, record.ExceptionInformation[1]);
    }
    if let Some(context) = unsafe { info.ContextRecord.as_ref() } {
        let _ = write!(line, "; rcx {:#x} rdx {:#x} rsp {:#x}; stack:", context.Rcx, context.Rdx, context.Rsp);
        let stack = context.Rsp as *const usize;
        let mut shown = 0;
        for i in 0..256 {
            let word = unsafe { stack.add(i).read_unaligned() };
            let located = locate(word);
            if located.contains('!') || located.contains('+') {
                let _ = write!(line, " {located}");
                shown += 1;
                if shown == 12 {
                    break;
                }
            }
        }
    }
    if let Ok(mut guard) = crate::LOG.try_lock()
        && let Some(f) = guard.as_mut()
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Registers the exception handler first in line and the panic hook.
pub fn install() {
    unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut guard) = crate::LOG.try_lock()
            && let Some(f) = guard.as_mut()
        {
            use std::io::Write;
            let _ = writeln!(f, "PANIC {info}");
            let _ = f.flush();
        }
    }));
}
