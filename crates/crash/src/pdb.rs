//! Names for addresses in DLLs through `DbgHelp`: a PDB next to the DLL or on the symbol path gives
//! the function, file and line; without one `DbgHelp` still names the nearest export. Loaded lazily,
//! on the watchdog after the dump is written (`DbgHelp` allocates), or by the offline decoder.

use core::cell::{Cell, RefCell};
use core::fmt::{self, Write};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{IMAGEHLP_LINEW64, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymFromAddrW, SymGetLineFromAddrW64, SymInitializeW, SymLoadModuleExW, SymSetOptions};

use crate::record::{MAX_MODULES, Module};

const NAME_UNITS: usize = 1024;

/// A symbol server for the DLLs of one process, live or from a dump.
pub struct DbgHelp {
    process: HANDLE,
    search_path: Option<&'static [u16]>,
    initialized: Cell<Option<bool>>,
    loaded: RefCell<[u64; MAX_MODULES]>,
    loaded_count: Cell<usize>,
    init_error: Cell<u32>,
    load_failures: Cell<u32>,
    load_error: Cell<u32>,
    lookups: Cell<u32>,
    lookup_failures: Cell<u32>,
    lookup_error: Cell<u32>,
}

impl DbgHelp {
    /// `process` is a handle of the live process that no other `DbgHelp` client uses (the pseudo-handle
    /// may already be another DLL's session), or any constant when decoding offline; `search_path` the
    /// NUL-terminated folders searched for PDBs besides each DLL's own.
    #[must_use]
    pub fn new(process: HANDLE, search_path: Option<&'static [u16]>) -> Self {
        Self { process, search_path, initialized: Cell::new(None), loaded: RefCell::new([0; MAX_MODULES]), loaded_count: Cell::new(0), init_error: Cell::new(0), load_failures: Cell::new(0), load_error: Cell::new(0), lookups: Cell::new(0), lookup_failures: Cell::new(0), lookup_error: Cell::new(0) }
    }

    fn ensure(&self) -> bool {
        if let Some(ok) = self.initialized.get() {
            return ok;
        }
        unsafe { SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES | SYMOPT_FAIL_CRITICAL_ERRORS) };
        let path = self.search_path.map_or(core::ptr::null(), <[u16]>::as_ptr);
        let ok = unsafe { SymInitializeW(self.process, path, 0) } != 0;
        if !ok {
            self.init_error.set(unsafe { GetLastError() });
        }
        self.initialized.set(Some(ok));
        ok
    }

    fn load(&self, module: &Module) -> bool {
        if !self.ensure() {
            return false;
        }
        if self.loaded.borrow()[..self.loaded_count.get()].contains(&module.base) {
            return true;
        }
        let mut path = [0u16; 520];
        for (n, unit) in module.path().encode_utf16().take(519).enumerate() {
            path[n] = unit;
        }
        let base = unsafe { SymLoadModuleExW(self.process, core::ptr::null_mut(), path.as_ptr(), core::ptr::null(), module.base, module.size, core::ptr::null(), 0) };
        // Zero with no error means the module was already loaded.
        let ok = base != 0 || unsafe { GetLastError() } == 0;
        if !ok {
            self.load_failures.set(self.load_failures.get() + 1);
            self.load_error.set(unsafe { GetLastError() });
            return false;
        }
        let count = self.loaded_count.get();
        if count < MAX_MODULES {
            self.loaded.borrow_mut()[count] = module.base;
            self.loaded_count.set(count + 1);
        }
        true
    }

    /// Writes ` name+0x..` and ` (file:line)` for an address inside `module`; false when `DbgHelp`
    /// has nothing, or only an export more than `max_displacement` bytes before the address (a
    /// guess, not a name).
    pub fn describe(&self, address: u64, module: &Module, w: &mut dyn Write, max_displacement: u64) -> Result<bool, fmt::Error> {
        if !self.load(module) {
            return Ok(false);
        }
        let mut buffer = [0u64; (size_of::<SYMBOL_INFOW>() + NAME_UNITS * 2) / 8 + 1];
        let symbol = buffer.as_mut_ptr().cast::<SYMBOL_INFOW>();
        unsafe {
            (*symbol).SizeOfStruct = u32::try_from(size_of::<SYMBOL_INFOW>()).unwrap_or(88);
            (*symbol).MaxNameLen = u32::try_from(NAME_UNITS).unwrap_or(0);
        }
        let mut displacement = 0u64;
        self.lookups.set(self.lookups.get() + 1);
        if unsafe { SymFromAddrW(self.process, address, &raw mut displacement, symbol) } == 0 {
            self.lookup_failures.set(self.lookup_failures.get() + 1);
            self.lookup_error.set(unsafe { GetLastError() });
            return Ok(false);
        }
        if displacement > max_displacement {
            return Ok(false);
        }
        let len = unsafe { (*symbol).NameLen } as usize;
        let name = unsafe { core::slice::from_raw_parts((*symbol).Name.as_ptr(), len.min(NAME_UNITS)) };
        w.write_char(' ')?;
        for c in char::decode_utf16(name.iter().copied()) {
            w.write_char(c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        write!(w, "+{displacement:#x}")?;
        let mut line: IMAGEHLP_LINEW64 = unsafe { core::mem::zeroed() };
        line.SizeOfStruct = u32::try_from(size_of::<IMAGEHLP_LINEW64>()).unwrap_or(0);
        let mut line_displacement = 0u32;
        if unsafe { SymGetLineFromAddrW64(self.process, address, &raw mut line_displacement, &raw mut line) } != 0 && !line.FileName.is_null() {
            let mut n = 0;
            while n < 520 && unsafe { *line.FileName.add(n) } != 0 {
                n += 1;
            }
            let file = unsafe { core::slice::from_raw_parts(line.FileName, n) };
            w.write_str(" (")?;
            for c in char::decode_utf16(file.iter().copied()) {
                w.write_char(c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
            }
            write!(w, ":{})", line.LineNumber)?;
        }
        Ok(true)
    }

    /// One line on how the symbol server fared, for the log.
    pub fn status(&self, w: &mut dyn Write) -> fmt::Result {
        match self.initialized.get() {
            None => w.write_str("DLL symbols: DbgHelp not used"),
            Some(false) => write!(w, "DLL symbols: SymInitialize failed with error {}", self.init_error.get()),
            Some(true) => write!(w, "DLL symbols: DbgHelp on handle {:#x}, {} modules loaded, {} loads failed (last error {}), {} lookups, {} without a name (last error {})", self.process as usize, self.loaded_count.get(), self.load_failures.get(), self.load_error.get(), self.lookups.get(), self.lookup_failures.get(), self.lookup_error.get()),
        }
    }
}
