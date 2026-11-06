// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! `dinput8.dll`, the framework's standalone host. Placed next to `SkyrimSE.exe`, it is the
//! `DirectInput` the game imports: every export forwards to the system's `dinput8.dll`. When the
//! process starts it points the game's import of the C runtime's `_get_narrow_winmain_command_line`,
//! which the startup code calls once right before `WinMain`, at a function of its own. There, unless
//! the script extender is loaded (its loader then hosts everything, `skyrim_runtime_extender.dll` included), the
//! [`loader`] loads the plugins in `Data\SKSE\Plugins` itself and gives them the plugin interface the
//! script extender would (request S1), so SKSE plugins and the framework run without it.

// The linker asks for the COM exports to be PRIVATE, which only a .def file can say; they forward as is.
#![allow(linker_messages)]

use core::ffi::{CStr, c_char, c_void};
use std::sync::{Once, OnceLock};

use windows_sys::Win32::Foundation::{HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::core::BOOL;

mod loader;

const CRT_RUNTIME: &str = "api-ms-win-crt-runtime-l1-1-0.dll";
const BEFORE_WINMAIN: &CStr = c"_get_narrow_winmain_command_line";

type CommandLineFn = unsafe extern "C" fn() -> *mut c_char;

static COMMAND_LINE: OnceLock<CommandLineFn> = OnceLock::new();

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(_: HMODULE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { patch_import() };
    }
    TRUE
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Points the executable's import of `CRT_RUNTIME`'s `BEFORE_WINMAIN` at [`before_winmain`].
unsafe fn patch_import() {
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    let Some(slot) = (unsafe { hook::import_slot(base, CRT_RUNTIME, BEFORE_WINMAIN) }) else { return };
    let original = unsafe { (slot as *const usize).read() };
    let _ = COMMAND_LINE.set(unsafe { core::mem::transmute::<usize, CommandLineFn>(original) });
    let _ = unsafe { hook::write_pointer(slot, before_winmain as CommandLineFn as usize) };
}

unsafe extern "C" fn before_winmain() -> *mut c_char {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let runtime = skse::runtime::running().unwrap_or(0);
        if skse::runtime::script_extender_loaded(runtime) {
            loader::defer(runtime);
        } else {
            loader::run(runtime);
        }
    });
    unsafe { COMMAND_LINE.get().expect("set with the patch")() }
}

/// The system's `dinput8.dll` export `name`.
fn system_export(name: &CStr) -> Option<unsafe extern "system" fn() -> isize> {
    static SYSTEM: OnceLock<usize> = OnceLock::new();
    let module = *SYSTEM.get_or_init(|| {
        let mut dir = [0u16; 260];
        let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), 260) } as usize;
        let path = format!("{}\\dinput8.dll", String::from_utf16_lossy(&dir[..len]));
        unsafe { LoadLibraryW(wide(&path).as_ptr()) as usize }
    });
    if module == 0 {
        return None;
    }
    unsafe { GetProcAddress(module as HMODULE, name.as_ptr().cast()) }
}

const E_FAIL: i32 = 0x8000_4005_u32.cast_signed();

macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, $fail:expr;)*) => {$(
        /// Forwards to the system `dinput8.dll`'s export of the same name.
        ///
        /// # Safety
        /// As the system export.
        #[unsafe(no_mangle)]
        #[allow(non_snake_case)]
        pub unsafe extern "system" fn $name($($arg: $ty),*) -> $ret {
            type F = unsafe extern "system" fn($($ty),*) -> $ret;
            match system_export(&std::ffi::CString::new(stringify!($name)).expect("no nul")) {
                Some(f) => unsafe { core::mem::transmute::<unsafe extern "system" fn() -> isize, F>(f)($($arg),*) },
                None => $fail,
            }
        }
    )*};
}

forward! {
    DirectInput8Create(instance: *mut c_void, version: u32, iid: *const c_void, out: *mut *mut c_void, outer: *mut c_void) -> i32, E_FAIL;
    DllCanUnloadNow() -> i32, E_FAIL;
    DllGetClassObject(clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> i32, E_FAIL;
    DllRegisterServer() -> i32, E_FAIL;
    DllUnregisterServer() -> i32, E_FAIL;
    GetdfDIJoystick() -> *const c_void, core::ptr::null();
}
