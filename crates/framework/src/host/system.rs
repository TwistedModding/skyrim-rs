//! The machine the game runs on, for the Debug tab: Windows, the processor, memory, the graphics
//! adapter and what the game's process uses of them.

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::ProcessStatus::{K32EnumProcessModules, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, GlobalMemoryStatusEx, MEMORYSTATUSEX, RelationProcessorCore};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// What does not change while the game runs.
pub(super) struct Machine {
    pub windows: String,
    pub cpu: String,
    pub cores: usize,
    pub threads: usize,
    pub memory: u64,
    pub adapter: Option<overlay::Adapter>,
}

/// What the process uses now, read at most once a second.
#[derive(Clone, Copy, Default)]
pub(super) struct Usage {
    pub memory_free: u64,
    pub working_set: u64,
    pub private: u64,
    pub modules: usize,
    pub video: Option<(u64, u64)>,
}

static MACHINE: OnceLock<Machine> = OnceLock::new();
static USAGE: Mutex<Option<(Instant, Usage)>> = Mutex::new(None);

/// The machine's facts, read at the first call: the overlay draws with the game's device, so it exists by then.
pub(super) fn machine() -> &'static Machine {
    MACHINE.get_or_init(|| Machine { windows: windows(), cpu: cpu(), cores: cores(), threads: std::thread::available_parallelism().map_or(0, usize::from), memory: memory_status().ullTotalPhys, adapter: overlay::adapter() })
}

pub(super) fn usage() -> Usage {
    let mut cached = USAGE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, usage)) = *cached
        && at.elapsed() < Duration::from_secs(1)
    {
        return usage;
    }
    let mut counters = PROCESS_MEMORY_COUNTERS_EX { cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0), ..unsafe { core::mem::zeroed() } };
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(), counters.cb) };
    let usage = Usage {
        memory_free: memory_status().ullAvailPhys,
        working_set: counters.WorkingSetSize as u64,
        private: counters.PrivateUsage as u64,
        modules: module_count(),
        video: overlay::video_memory().map(|v| (v.usage, v.budget)),
    };
    *cached = Some((Instant::now(), usage));
    usage
}

fn memory_status() -> MEMORYSTATUSEX {
    let mut status = MEMORYSTATUSEX { dwLength: u32::try_from(size_of::<MEMORYSTATUSEX>()).unwrap_or(0), ..unsafe { core::mem::zeroed() } };
    unsafe { GlobalMemoryStatusEx(&raw mut status) };
    status
}

/// Every module mapped in the process: the game, Windows' DLLs, the script extender and its plugins.
pub(super) fn module_count() -> usize {
    let mut needed = 0u32;
    unsafe { K32EnumProcessModules(GetCurrentProcess(), core::ptr::null_mut(), 0, &raw mut needed) };
    needed as usize / size_of::<HMODULE>()
}

/// The processor's brand string from `cpuid` leaves 0x80000002 to 0x80000004.
fn cpu() -> String {
    use core::arch::x86_64::__cpuid;
    if __cpuid(0x8000_0000).eax < 0x8000_0004 {
        return "unknown processor".to_owned();
    }
    let bytes: Vec<u8> = (0x8000_0002..=0x8000_0004u32)
        .flat_map(|leaf| {
            let r = __cpuid(leaf);
            [r.eax, r.ebx, r.ecx, r.edx].into_iter().flat_map(u32::to_le_bytes)
        })
        .collect();
    String::from_utf8_lossy(&bytes).trim_matches(['\0', ' ']).to_owned()
}

/// Physical cores: one `RelationProcessorCore` record each.
fn cores() -> usize {
    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, core::ptr::null_mut(), &raw mut len) };
    let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
    if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, buffer.as_mut_ptr().cast(), &raw mut len) } == 0 {
        return 0;
    }
    let bytes = unsafe { core::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), len as usize) };
    let (mut at, mut count) = (0, 0);
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        if size == 0 {
            break;
        }
        count += 1;
        at += size;
    }
    count
}

const CURRENT_VERSION: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn registry_text(value: &str) -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut len = u32::try_from(size_of_val(&buffer)).unwrap_or(0);
    let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, wide(CURRENT_VERSION).as_ptr(), wide(value).as_ptr(), RRF_RT_REG_SZ, core::ptr::null_mut(), buffer.as_mut_ptr().cast::<c_void>(), &raw mut len) };
    (status == 0).then(|| String::from_utf16_lossy(&buffer[..(len as usize / 2).saturating_sub(1)]))
}

fn registry_number(value: &str) -> Option<u32> {
    let mut number = 0u32;
    let mut len = 4u32;
    let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, wide(CURRENT_VERSION).as_ptr(), wide(value).as_ptr(), RRF_RT_REG_DWORD, core::ptr::null_mut(), (&raw mut number).cast::<c_void>(), &raw mut len) };
    (status == 0).then_some(number)
}

/// `Windows 11 Pro 24H2, build 26100.2314`. Windows 11 still names itself Windows 10 in `ProductName`;
/// its builds start at 22000.
fn windows() -> String {
    let build = registry_text("CurrentBuildNumber").unwrap_or_default();
    let mut name = registry_text("ProductName").unwrap_or_else(|| "Windows".to_owned());
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        name = name.replace("Windows 10", "Windows 11");
    }
    let release = registry_text("DisplayVersion").map(|v| format!(" {v}")).unwrap_or_default();
    let patch = registry_number("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    format!("{name}{release}, build {build}{patch}")
}

/// Bytes as `1.5 GB`, `340 MB` or `12 KB`.
#[allow(clippy::cast_precision_loss)]
pub(super) fn bytes(n: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= 1024.0 * 1024.0 {
        format!("{:.0} MB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", n / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_machine() {
        assert!(!cpu().is_empty() && cores() > 0 && memory_status().ullTotalPhys > 0);
        assert!(windows().contains("build "), "{}", windows());
        assert!(module_count() > 3);
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(bytes(340 * 1024 * 1024), "340 MB");
    }
}
