//! The handlers and the watchdog. The vectored handler only watches: it counts first-chance
//! error exceptions, records thread names and puts the unhandled-exception filter back in front
//! when another module replaced it (chaining to that module's filter at crash time). The filter
//! copies the exception and context into statics, wakes the watchdog and waits for it, so the
//! crashing thread does no work on its own stack, which a stack overflow has used up. The
//! watchdog, a thread of its own with a reserved stack, fills the record, writes the minidump
//! with the record as a user stream, then the text log, and lets the filter return.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicIsize, AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ};
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, CONTEXT, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_EXECUTE_HANDLER, EXCEPTION_POINTERS, EXCEPTION_RECORD, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_USER_STREAM, MINIDUMP_USER_STREAM_INFORMATION, MiniDumpIgnoreInaccessibleMemory, MiniDumpWithFullMemoryInfo, MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{CreateEventW, CreateThread, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, INFINITE, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ResetEvent, STACK_SIZE_PARAM_IS_A_RESERVATION, SetEvent, WaitForSingleObject};

use crate::live::{self, GameAddresses, THREAD_NAME_EXCEPTION};
use crate::record::{self, Record, store};
use crate::text::{FileWriter, Writer};
use crate::{Crash, Error, FactsFn, Installed, Options, Section, images, report, symbols};

struct Shared<T>(UnsafeCell<T>);

unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    const fn new(v: T) -> Self {
        Self(UnsafeCell::new(v))
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get(&self) -> &mut T {
        unsafe { &mut *self.0.get() }
    }
}

/// What the filter copies out of the crashing thread.
struct Snapshot {
    record: EXCEPTION_RECORD,
    context: CONTEXT,
    pointers: EXCEPTION_POINTERS,
    thread: u32,
}

const IDLE: u32 = 0;
const REPORTING: u32 = 1;
const DONE: u32 = 2;
/// How long the crashing thread waits for the report before the process is allowed to end.
const REPORT_TIMEOUT_MS: u32 = 60_000;
const PATH_UNITS: usize = 320;

static STATE: AtomicU32 = AtomicU32::new(IDLE);
static CRASH_EVENT: AtomicIsize = AtomicIsize::new(0);
static DONE_EVENT: AtomicIsize = AtomicIsize::new(0);
static WATCHDOG: AtomicU32 = AtomicU32::new(0);
/// A handle of our own on the process for the symbol server, since another DLL may own the session on the pseudo-handle.
static SYMBOL_PROCESS: AtomicIsize = AtomicIsize::new(0);
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static NEXT_FILTER: AtomicUsize = AtomicUsize::new(0);
static FIRST_CHANCE: AtomicU32 = AtomicU32::new(0);
static START: AtomicUsize = AtomicUsize::new(0);
static RUNTIME: AtomicU32 = AtomicU32::new(0);
static SYMBOLS: AtomicU32 = AtomicU32::new(0);
static SYMBOL_MISMATCHES: AtomicU32 = AtomicU32::new(0);

static SNAPSHOT: Shared<Snapshot> = Shared::new(unsafe { core::mem::zeroed() });
static RECORD: Shared<Record> = Shared::new(Record::ZERO);
static IO: Shared<[u8; 64 * 1024]> = Shared::new([0; 64 * 1024]);
static VERSION_INFO: Shared<[u8; 64 * 1024]> = Shared::new([0; 64 * 1024]);
static LOG_DIR: Shared<([u16; PATH_UNITS], usize)> = Shared::new(([0; PATH_UNITS], 0));
static DUMP_PATH: Shared<[u16; PATH_UNITS]> = Shared::new([0; PATH_UNITS]);
static LOG_PATH: Shared<[u16; PATH_UNITS]> = Shared::new([0; PATH_UNITS]);
static SYMBOL_PATH: Shared<([u16; 1024], usize)> = Shared::new(([0; 1024], 0));
static NAME: Shared<([u8; record::NAME_LEN], usize)> = Shared::new(([0; record::NAME_LEN], 0));
static BUILD: Shared<([u8; 256], usize)> = Shared::new(([0; 256], 0));
static GAME: Shared<GameAddresses> = Shared::new(GameAddresses { frame_counter: 0, player: 0, save_manager: 0 });
static FACTS: Shared<Option<FactsFn>> = Shared::new(None);

fn wide(s: &str, out: &mut [u16]) -> usize {
    let mut n = 0;
    for unit in s.encode_utf16() {
        if n + 1 >= out.len() {
            break;
        }
        out[n] = unit;
        n += 1;
    }
    out[n] = 0;
    n
}

pub(crate) fn install(options: &Options, log_dir: &str, game: GameAddresses) -> Result<Installed, Error> {
    let mut event_name = [0u16; 64];
    wide(&format!("Local\\skyrim-runtime-extender-crash-{}", unsafe { GetCurrentProcessId() }), &mut event_name);
    let crash = unsafe { CreateEventW(core::ptr::null(), 1, 0, event_name.as_ptr()) };
    if crash.is_null() {
        return Err(Error::Event(unsafe { GetLastError() }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(crash) };
        return Ok(Installed::Elsewhere);
    }
    let done = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
    if done.is_null() {
        return Err(Error::Event(unsafe { GetLastError() }));
    }
    CRASH_EVENT.store(crash as isize, Ordering::Release);
    DONE_EVENT.store(done as isize, Ordering::Release);
    SYMBOL_PROCESS.store(unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, GetCurrentProcessId()) } as isize, Ordering::Release);
    unsafe {
        let dir = LOG_DIR.get();
        dir.1 = wide(log_dir, &mut dir.0);
        let name = NAME.get();
        name.1 = store(&mut name.0, options.name);
        let build = BUILD.get();
        build.1 = store(&mut build.0, options.build);
        *GAME.get() = game;
        *FACTS.get() = options.facts;
        if let Some(path) = &options.symbol_path {
            let symbols = SYMBOL_PATH.get();
            symbols.1 = wide(path, &mut symbols.0);
        }
    }
    let (mode, mismatches) = symbols::init();
    SYMBOLS.store(mode as u32, Ordering::Release);
    SYMBOL_MISMATCHES.store(mismatches, Ordering::Release);
    RUNTIME.store(options.runtime_version, Ordering::Release);
    START.store(usize::try_from(unsafe { GetTickCount64() }).unwrap_or(0), Ordering::Release);
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let mut tid = 0u32;
    let thread = unsafe { CreateThread(core::ptr::null(), 2 << 20, Some(watchdog), core::ptr::null(), STACK_SIZE_PARAM_IS_A_RESERVATION, &raw mut tid) };
    if thread.is_null() {
        return Err(Error::Thread(unsafe { GetLastError() }));
    }
    unsafe { CloseHandle(thread) };
    WATCHDOG.store(tid, Ordering::Release);
    if unsafe { AddVectoredExceptionHandler(1, Some(vectored)) }.is_null() {
        return Err(Error::Handler);
    }
    if let Some(previous) = unsafe { SetUnhandledExceptionFilter(Some(filter)) } {
        NEXT_FILTER.store(previous as usize, Ordering::Release);
    }
    Ok(Installed::Here { watchdog: tid, symbols: mode, symbol_mismatches: mismatches })
}

/// The first-chance watch: counts error-class exceptions, keeps thread names and reinstates the filter.
unsafe extern "system" fn vectored(info: *mut EXCEPTION_POINTERS) -> i32 {
    let Some(record) = (unsafe { info.as_ref() }).and_then(|i| unsafe { i.ExceptionRecord.as_ref() }) else { return EXCEPTION_CONTINUE_SEARCH };
    let code = record.ExceptionCode.cast_unsigned();
    if code == THREAD_NAME_EXCEPTION {
        unsafe { live::note_thread_name(record, GetCurrentThreadId()) };
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // Severity "error" without the C++ and CLR throw codes, which are normal traffic.
    if code >> 30 != 3 || code == 0xE06D_7363 || code == 0xE043_4352 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    FIRST_CHANCE.fetch_add(1, Ordering::Relaxed);
    let ours = filter as *const () as usize;
    if let Some(current) = unsafe { SetUnhandledExceptionFilter(Some(filter)) }
        && current as usize != ours
    {
        NEXT_FILTER.store(current as usize, Ordering::Release);
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Hands the exception to the filter another module installed, if any, else ends the process.
unsafe fn chain(info: *const EXCEPTION_POINTERS) -> i32 {
    let next = NEXT_FILTER.load(Ordering::Acquire);
    if next == 0 || next == filter as *const () as usize {
        return EXCEPTION_EXECUTE_HANDLER;
    }
    let next: unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32 = unsafe { core::mem::transmute(next) };
    unsafe { next(info) }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    let me = unsafe { GetCurrentThreadId() };
    if me == WATCHDOG.load(Ordering::Acquire) || info.is_null() {
        return unsafe { chain(info) };
    }
    let done = DONE_EVENT.load(Ordering::Acquire) as HANDLE;
    if STATE.compare_exchange(IDLE, REPORTING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        let snapshot = unsafe { SNAPSHOT.get() };
        unsafe {
            if let Some(record) = (*info).ExceptionRecord.as_ref() {
                snapshot.record = *record;
            }
            if let Some(context) = (*info).ContextRecord.as_ref() {
                snapshot.context = *context;
            }
        }
        snapshot.thread = me;
        snapshot.pointers = EXCEPTION_POINTERS { ExceptionRecord: &raw mut snapshot.record, ContextRecord: &raw mut snapshot.context };
        unsafe { SetEvent(CRASH_EVENT.load(Ordering::Acquire) as HANDLE) };
    }
    unsafe { WaitForSingleObject(done, REPORT_TIMEOUT_MS) };
    unsafe { chain(info) }
}

unsafe extern "system" fn watchdog(_: *mut core::ffi::c_void) -> u32 {
    loop {
        unsafe { WaitForSingleObject(CRASH_EVENT.load(Ordering::Acquire) as HANDLE, INFINITE) };
        report();
        STATE.store(DONE, Ordering::Release);
        unsafe {
            SetEvent(DONE_EVENT.load(Ordering::Acquire) as HANDLE);
            ResetEvent(CRASH_EVENT.load(Ordering::Acquire) as HANDLE);
        }
    }
}

/// `<log dir>\skyrim-runtime-extender-crash-<yyyymmdd-hhmmss>.<ext>` into `out`.
fn path(out: &mut [u16; PATH_UNITS], time: [u16; 6], ext: &str) {
    let dir = unsafe { LOG_DIR.get() };
    let mut n = dir.1.min(PATH_UNITS - 64);
    out[..n].copy_from_slice(&dir.0[..n]);
    let mut buf = [0u8; 64];
    let mut w = Writer::new(&mut buf);
    let _ = core::fmt::Write::write_fmt(&mut w, format_args!("\\skyrim-runtime-extender-crash-{:04}{:02}{:02}-{:02}{:02}{:02}.{ext}", time[0], time[1], time[2], time[3], time[4], time[5]));
    for unit in w.as_str().encode_utf16() {
        out[n] = unit;
        n += 1;
    }
    out[n] = 0;
}

fn create(path: &[u16]) -> HANDLE {
    unsafe { CreateFileW(path.as_ptr(), GENERIC_WRITE, FILE_SHARE_READ, core::ptr::null(), CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, core::ptr::null_mut()) }
}

fn facts(rec: &mut Record, section: Section) {
    let Some(f) = (unsafe { *FACTS.get() }) else { return };
    let crash = Crash { thread: rec.thread, main_thread: rec.main_thread, exception: rec.exception.code };
    let buf = if section == Section::Context { &mut rec.context } else { &mut rec.state };
    let mut w = Writer::new(buf);
    f(&mut w, &crash, section);
    let len = u32::try_from(w.len()).unwrap_or(0);
    if section == Section::Context {
        rec.context_len = len;
    } else {
        rec.state_len = len;
    }
}

fn report() {
    let snapshot = unsafe { SNAPSHOT.get() };
    let rec = unsafe { RECORD.get() };
    *rec = Record::ZERO;
    rec.magic = record::MAGIC;
    rec.size = u32::try_from(size_of::<Record>()).unwrap_or(0);
    rec.version = record::VERSION;
    rec.time = live::local_time();
    rec.uptime_ms = (unsafe { GetTickCount64() }).saturating_sub(START.load(Ordering::Acquire) as u64);
    rec.runtime = RUNTIME.load(Ordering::Acquire);
    rec.main_thread = MAIN_THREAD.load(Ordering::Acquire);
    rec.thread = snapshot.thread;
    rec.unhandled = 1;
    rec.symbols = u8::try_from(SYMBOLS.load(Ordering::Acquire)).unwrap_or(0);
    rec.symbol_mismatches = SYMBOL_MISMATCHES.load(Ordering::Acquire);
    rec.first_chance = FIRST_CHANCE.load(Ordering::Relaxed).saturating_sub(1);
    unsafe {
        let name = NAME.get();
        rec.installer[..name.1].copy_from_slice(&name.0[..name.1]);
        rec.installer_len = u8::try_from(name.1).unwrap_or(0);
        let build = BUILD.get();
        rec.build[..build.1].copy_from_slice(&build.0[..build.1]);
        rec.build_len = u16::try_from(build.1).unwrap_or(0);
    }
    rec.exception = live::exception(&snapshot.record);
    rec.registers = live::registers(&snapshot.context);
    rec.exe_base = unsafe { GetModuleHandleW(core::ptr::null()) } as u64;
    live::fill_modules(rec);
    rec.frame_count = u32::try_from(live::walk(&snapshot.context, &mut rec.frames)).unwrap_or(0);
    images::copy_into(rec);
    live::fill_game(&mut rec.game, unsafe { GAME.get() });
    facts(rec, Section::Context);
    facts(rec, Section::State);

    let dump_path = unsafe { DUMP_PATH.get() };
    let log_path = unsafe { LOG_PATH.get() };
    path(dump_path, rec.time, "dmp");
    path(log_path, rec.time, "log");
    let dump = create(dump_path);
    if dump == INVALID_HANDLE_VALUE {
        rec.dump_error = live::last_error();
    } else {
        let mut stream = MINIDUMP_USER_STREAM { Type: record::STREAM_TYPE, BufferSize: rec.size, Buffer: core::ptr::from_mut(rec).cast() };
        let streams = MINIDUMP_USER_STREAM_INFORMATION { UserStreamCount: 1, UserStreamArray: &raw mut stream };
        let exception = MINIDUMP_EXCEPTION_INFORMATION { ThreadId: snapshot.thread, ExceptionPointers: &raw mut snapshot.pointers, ClientPointers: 0 };
        let kind = MiniDumpWithIndirectlyReferencedMemory | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules | MiniDumpWithFullMemoryInfo | MiniDumpIgnoreInaccessibleMemory;
        let ok = unsafe { MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), dump, kind, &raw const exception, &raw const streams, core::ptr::null()) };
        rec.dump_written = u8::from(ok != 0);
        if ok == 0 {
            rec.dump_error = live::last_error();
        }
        unsafe { CloseHandle(dump) };
    }

    live::fill_threads(rec, (snapshot.thread, &snapshot.context), WATCHDOG.load(Ordering::Acquire));
    live::fill_versions(rec, unsafe { VERSION_INFO.get() });
    let log = create(log_path);
    if log == INVALID_HANDLE_VALUE {
        return;
    }
    let symbol_process = SYMBOL_PROCESS.load(Ordering::Acquire) as HANDLE;
    let symbol_path = unsafe { SYMBOL_PATH.get() };
    let search_path = (symbol_path.1 > 0).then(|| &symbol_path.0[..=symbol_path.1]);
    let pdb = crate::pdb::DbgHelp::new(if symbol_process.is_null() { unsafe { GetCurrentProcess() } } else { symbol_process }, search_path);
    let mut out = FileWriter::new(log, unsafe { IO.get() });
    report::write(&mut out, rec, &crate::mem::Live, Some((unsafe { &*DUMP_PATH.get() }, unsafe { &*LOG_PATH.get() })), Some(&pdb));
    out.flush();
    unsafe { CloseHandle(log) };
}
