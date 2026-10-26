//! Names for addresses in DLLs through `DbgHelp`: a PDB next to the DLL or on the symbol path gives
//! the function, file and line; without one `DbgHelp` still names the nearest export. Loaded lazily,
//! on the watchdog after the dump is written (`DbgHelp` allocates), or by the offline decoder.

use core::cell::{Cell, RefCell};
use core::fmt::{self, Write};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::Debug::{IMAGEHLP_LINEW64, SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymFromAddrW, SymGetLineFromAddrW64, SymInitializeW, SymLoadModuleExW, SymSetOptions};

use crate::record::{MAX_MODULES, Module};

const NAME_UNITS: usize = 1024;

/// A symbol server for the DLLs of one process, live or from a dump.
pub struct DbgHelp {
    process: HANDLE,
    initialized: Cell<Option<bool>>,
    loaded: RefCell<[u64; MAX_MODULES]>,
    loaded_count: Cell<usize>,
}

impl DbgHelp {
    /// `process` is the live process handle, or any value no other `DbgHelp` client uses when decoding offline.
    #[must_use]
    pub fn new(process: HANDLE) -> Self {
        Self { process, initialized: Cell::new(None), loaded: RefCell::new([0; MAX_MODULES]), loaded_count: Cell::new(0) }
    }

    fn ensure(&self) -> bool {
        if let Some(ok) = self.initialized.get() {
            return ok;
        }
        unsafe { SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES | SYMOPT_FAIL_CRITICAL_ERRORS) };
        let ok = unsafe { SymInitializeW(self.process, core::ptr::null(), 0) } != 0;
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
        let count = self.loaded_count.get();
        if count < MAX_MODULES {
            self.loaded.borrow_mut()[count] = module.base;
            self.loaded_count.set(count + 1);
        }
        base != 0
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
        if unsafe { SymFromAddrW(self.process, address, &raw mut displacement, symbol) } == 0 || displacement > max_displacement {
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
}
