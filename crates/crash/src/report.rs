//! The text log, formatted from a record and a memory source: the same code in the game and in
//! the offline decoder.

use core::cell::RefCell;
use core::fmt::{self, Write};

use crate::mem::Source;
use crate::pdb::DbgHelp;
use crate::probe;
use crate::record::{Record, Thread};
use crate::rules::{self, Access, Seen};
use crate::symbols::{self, Code, Data, Mode};

/// How many qwords above the stack pointer are probed.
const STACK_WORDS: u64 = 48;
/// Export names this far past the export are not a name but a guess, and left out.
const MAX_EXPORT_DISPLACEMENT: u64 = 0x4000;

/// Everything a section needs: the record, the memory it describes, the symbol server for DLLs
/// and what the probes have seen so far (for the explanation).
pub struct Context<'a> {
    pub rec: &'a Record,
    pub src: &'a dyn Source,
    pub pdb: Option<&'a DbgHelp>,
    pub seen: RefCell<Seen>,
}

/// Writes the whole report; `paths` are the dump and log files when written in the game.
pub fn write(w: &mut dyn Write, rec: &Record, src: &dyn Source, paths: Option<(&[u16], &[u16])>, pdb: Option<&DbgHelp>) {
    let ctx = Context { rec, src, pdb, seen: RefCell::new(Seen::default()) };
    let _ = header(w, &ctx, paths);
    // The probes note what they see for the rules; run them silently once so the summary can come first.
    let _ = registers(&mut crate::text::Discard, &ctx);
    let _ = stack_top(&mut crate::text::Discard, &ctx);
    let _ = summary(w, &ctx);
    let _ = loader(w, &ctx);
    let _ = exception(w, &ctx);
    let _ = registers(w, &ctx);
    let _ = stack(w, &ctx);
    let _ = stack_top(w, &ctx);
    let _ = threads(w, &ctx);
    let _ = modules(w, &ctx);
    let _ = state(w, &ctx);
    let _ = crate::papyrus::write(w, &ctx);
}

fn wide(w: &mut dyn Write, units: &[u16]) -> fmt::Result {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    for c in char::decode_utf16(units[..end].iter().copied()) {
        w.write_char(c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
    }
    Ok(())
}

fn header(w: &mut dyn Write, ctx: &Context<'_>, paths: Option<(&[u16], &[u16])>) -> fmt::Result {
    let rec = ctx.rec;
    let [year, month, day, hour, minute, second] = rec.time;
    let [major, minor, build, store] = skse::runtime::unpack(rec.runtime);
    writeln!(w, "Skyrim Runtime Extender crash report")?;
    writeln!(w, "time {year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} local, uptime {}", Uptime(rec.uptime_ms))?;
    writeln!(w, "game {major}.{minor}.{build}.{store}, SkyrimSE.exe at {:#x}", rec.exe_base)?;
    writeln!(w, "reported by {}: {}", rec.installer(), rec.build())?;
    match Mode::from_u8(rec.symbols) {
        Mode::None => writeln!(w, "game symbols: none, the runtime is not 1.7.104")?,
        Mode::Ledger => writeln!(w, "game symbols: {} functions at the ledger's addresses", symbols::count())?,
        Mode::AddressLibrary => writeln!(w, "game symbols: {} functions at the Address Library's offsets, {} differ from the ledger", symbols::count(), rec.symbol_mismatches)?,
    }
    if let Some((dump, log)) = paths {
        w.write_str("minidump ")?;
        if rec.dump_written != 0 {
            w.write_str("written to ")?;
            wide(w, dump)?;
            writeln!(w)?;
        } else {
            writeln!(w, "not written, error {}", rec.dump_error)?;
        }
        w.write_str("this log: ")?;
        wide(w, log)?;
        writeln!(w)?;
    }
    let game = &rec.game;
    w.write_str("game state: ")?;
    if game.frame_known != 0 {
        write!(w, "frame {}", game.frame)?;
    } else {
        w.write_str("frame unknown")?;
    }
    if game.save_len > 0 {
        write!(w, ", last save \"{}\"", crate::record::text(&game.save, game.save_len as usize))?;
    } else {
        w.write_str(", no save loaded")?;
    }
    if game.cell_known != 0 {
        write!(w, ", player in cell {:08X}", game.cell_id)?;
        if game.cell_name_len > 0 {
            write!(w, " \"{}\"", crate::record::text(&game.cell_name, game.cell_name_len as usize))?;
        }
        if game.worldspace_known != 0 {
            write!(w, " of worldspace {:08X}", game.worldspace_id)?;
        }
        write!(w, " at ({:.0}, {:.0}, {:.0})", game.position[0], game.position[1], game.position[2])?;
    } else {
        w.write_str(", player not in a cell")?;
    }
    writeln!(w)?;
    if rec.first_chance > 0 {
        writeln!(w, "first-chance error exceptions seen earlier in this session: {}", rec.first_chance)?;
    }
    Ok(())
}

fn loader(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    if ctx.rec.context_len > 0 {
        writeln!(w, "\nLoader\n{}", ctx.rec.context_text().trim_end())?;
    }
    Ok(())
}

struct Uptime(u64);

impl fmt::Display for Uptime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0 / 1000;
        if s >= 3600 {
            write!(f, "{}h {:02}m {:02}s", s / 3600, s / 60 % 60, s % 60)
        } else {
            write!(f, "{}m {:02}s", s / 60, s % 60)
        }
    }
}

/// The exception code's name and plain meaning.
#[must_use]
pub fn code_name(code: u32) -> (&'static str, &'static str) {
    match code {
        0xC000_0005 => ("EXCEPTION_ACCESS_VIOLATION", "the code read, wrote or executed memory it has no right to"),
        0xC000_0006 => ("EXCEPTION_IN_PAGE_ERROR", "a memory page backed by a file could not be read"),
        0xC000_0008 => ("STATUS_INVALID_HANDLE", "a closed or bad handle was used"),
        0xC000_0017 => ("STATUS_NO_MEMORY", "the process ran out of memory"),
        0xC000_001D => ("EXCEPTION_ILLEGAL_INSTRUCTION", "the processor met bytes that are not an instruction, usually a jump into data or freed code"),
        0xC000_0025 => ("EXCEPTION_NONCONTINUABLE_EXCEPTION", "code tried to continue after an exception that cannot be continued"),
        0xC000_0026 => ("EXCEPTION_INVALID_DISPOSITION", "an exception handler returned a bad disposition"),
        0xC000_008C => ("EXCEPTION_ARRAY_BOUNDS_EXCEEDED", "an array index was out of bounds"),
        0xC000_008D => ("EXCEPTION_FLT_DENORMAL_OPERAND", "a floating-point operand was denormal"),
        0xC000_008E => ("EXCEPTION_FLT_DIVIDE_BY_ZERO", "a floating-point division by zero"),
        0xC000_008F => ("EXCEPTION_FLT_INEXACT_RESULT", "a floating-point result could not be represented"),
        0xC000_0090 => ("EXCEPTION_FLT_INVALID_OPERATION", "an invalid floating-point operation"),
        0xC000_0091 => ("EXCEPTION_FLT_OVERFLOW", "a floating-point overflow"),
        0xC000_0092 => ("EXCEPTION_FLT_STACK_CHECK", "the floating-point stack overflowed or underflowed"),
        0xC000_0093 => ("EXCEPTION_FLT_UNDERFLOW", "a floating-point underflow"),
        0xC000_0094 => ("EXCEPTION_INT_DIVIDE_BY_ZERO", "an integer division by zero"),
        0xC000_0095 => ("EXCEPTION_INT_OVERFLOW", "an integer overflow trapped"),
        0xC000_0096 => ("EXCEPTION_PRIV_INSTRUCTION", "a privileged instruction was executed, usually a jump into garbage"),
        0xC000_00FD => ("EXCEPTION_STACK_OVERFLOW", "the thread used up its stack, usually unbounded recursion"),
        0xC000_0135 => ("STATUS_DLL_NOT_FOUND", "a DLL the code depends on is missing"),
        0xC000_013A => ("STATUS_CONTROL_C_EXIT", "the process was ended with Ctrl+C"),
        0xC000_0142 => ("STATUS_DLL_INIT_FAILED", "a DLL's initialization failed"),
        0xC000_0374 => ("STATUS_HEAP_CORRUPTION", "the heap found its bookkeeping damaged: something wrote past an allocation or freed memory twice"),
        0xC000_0409 => ("STATUS_STACK_BUFFER_OVERRUN", "a fail-fast: a stack buffer overrun or an aborted program (Rust panics end here in release builds)"),
        0xC000_041D => ("STATUS_FATAL_USER_CALLBACK_EXCEPTION", "an exception escaped a user-mode callback"),
        0xC06D_007E => ("STATUS_DELAY_LOAD_MODULE_NOT_FOUND", "a delay-loaded DLL is missing"),
        0xC06D_007F => ("STATUS_DELAY_LOAD_PROCEDURE_NOT_FOUND", "a delay-loaded DLL lacks an export"),
        0xE06D_7363 => ("C++ exception", "an unhandled C++ throw"),
        0x8000_0003 => ("EXCEPTION_BREAKPOINT", "a breakpoint instruction without a debugger"),
        _ => ("unknown exception code", "see ntstatus.h"),
    }
}

/// `module+offset` and, for the game, the ledger name, for a DLL its PDB or export name; for a
/// return address the lookup uses the byte before it so the call instruction's function is named.
pub fn code_location(w: &mut dyn Write, address: u64, ctx: &Context<'_>, return_address: bool) -> fmt::Result {
    let rec = ctx.rec;
    let lookup = if return_address { address.wrapping_sub(1) } else { address };
    if let Some(module) = rec.module_at(lookup) {
        write!(w, "{}+{:#x}", module.name(), address - module.base)?;
        if module.base == rec.exe_base {
            if let Ok(rva) = u32::try_from(lookup - module.base)
                && let Some(Code { function, offset, meaning }) = symbols::code(rva)
            {
                write!(w, " {}+{:#x} ({}", function.name, u64::from(offset) + u64::from(return_address), function.confidence.as_str())?;
                if let Some(meaning) = meaning {
                    write!(w, "; {meaning}")?;
                }
                w.write_str(")")?;
            }
        } else if let Some(pdb) = ctx.pdb {
            let _ = pdb.describe(lookup, module, w, MAX_EXPORT_DISPLACEMENT)?;
        }
        if let Some(image) = rec.images().iter().find(|i| i.live != 0 && i.contains(lookup)) {
            write!(w, " (framework plugin {} generation {})", image.name(), image.generation)?;
        }
        return Ok(());
    }
    if let Some(image) = rec.images().iter().find(|i| i.contains(lookup)) {
        ctx.seen.borrow_mut().note_freed_plugin(image.name());
        return write!(w, "freed image of {} generation {} +{:#x}", image.name(), image.generation, address - image.base);
    }
    write!(w, "{address:#x}")?;
    if address == 0 {
        w.write_str(" (null)")?;
    }
    Ok(())
}

/// What an address the code touched is: null, a module, a stack, a freed image, free memory, and so on.
pub fn classify(w: &mut dyn Write, address: u64, ctx: &Context<'_>) -> Result<Access, fmt::Error> {
    let (rec, src) = (ctx.rec, ctx.src);
    if address == 0 {
        w.write_str("null")?;
        return Ok(Access::Null);
    }
    if address < 0x10000 {
        write!(w, "near null: offset {address:#x} from a null object")?;
        return Ok(Access::NearNull);
    }
    if let Some(module) = rec.module_at(address) {
        write!(w, "inside {}+{:#x}", module.name(), address - module.base)?;
        if module.base == rec.exe_base
            && let Ok(rva) = u32::try_from(address - module.base)
        {
            match symbols::data(rva) {
                Some(Data::Vtable { vtable, offset }) => write!(w, ", {}::vftable+{:#x} (slot {})", vtable.class, offset, offset / 8)?,
                Some(Data::Global { global, offset }) => write!(w, ", {}+{:#x}", global.name, offset)?,
                None => {
                    if let Some(Code { function, offset, .. }) = symbols::code(rva) {
                        write!(w, ", {}+{:#x}", function.name, offset)?;
                    }
                }
            }
        }
        let region = src.region(address);
        if region.known {
            write!(w, ", {}", Protection(&region))?;
        }
        return Ok(Access::Image);
    }
    if let Some(image) = rec.images().iter().find(|i| i.contains(address)) {
        ctx.seen.borrow_mut().note_freed_plugin(image.name());
        write!(w, "inside the freed image of {} generation {} (+{:#x}): a pointer kept across a hot reload", image.name(), image.generation, address - image.base)?;
        return Ok(Access::FreedImage);
    }
    if let Some(thread) = rec.threads().iter().find(|t| t.stack_base != 0 && address >= t.stack_base && address < t.stack_end) {
        w.write_str("in the stack of ")?;
        thread_kind(w, ctx, thread.id)?;
        return Ok(Access::Stack);
    }
    let region = src.region(address);
    if !region.known {
        w.write_str("outside the address space")?;
        return Ok(Access::Unmapped);
    }
    if region.free() {
        w.write_str("unmapped memory: freed or never allocated")?;
        return Ok(Access::Unmapped);
    }
    if region.reserved() {
        w.write_str("reserved but not committed memory")?;
        return Ok(Access::Unmapped);
    }
    if region.guard() {
        w.write_str("a guard page (the end of a stack or a heap's guard)")?;
        return Ok(Access::Guard);
    }
    if region.no_access() {
        w.write_str("committed memory with no access")?;
        return Ok(Access::Unmapped);
    }
    let kind = if region.mapped() { "mapped file memory" } else { "heap or other private memory" };
    write!(w, "{kind}, {}", Protection(&region))?;
    Ok(Access::Heap)
}

struct Protection<'a>(&'a crate::mem::Region);

impl fmt::Display for Protection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.0;
        if r.guard() {
            return f.write_str("guard page");
        }
        if r.no_access() {
            return f.write_str("no access");
        }
        match (r.readable(), r.writable(), r.executable()) {
            (_, true, true) => f.write_str("read-write-execute"),
            (_, true, false) => f.write_str("read-write"),
            (true, false, true) => f.write_str("read-execute"),
            (true, false, false) => f.write_str("read-only"),
            (false, false, true) => f.write_str("execute-only"),
            (false, false, false) => f.write_str("no access"),
        }
    }
}

/// What kind of engine thread the frames say this is, from the functions at the bottom of its stack.
fn thread_role(rec: &Record, id: u32) -> Option<&'static str> {
    let names = |rip: u64| -> Option<&'static str> {
        let module = rec.module_at(rip.wrapping_sub(1))?;
        if module.base != rec.exe_base {
            return None;
        }
        Some(symbols::code(u32::try_from(rip.wrapping_sub(1) - module.base).ok()?)?.function.name)
    };
    let role = |name: &str| {
        if name.contains("JobThread") || name.starts_with("Job_") || name.contains("ExecuteJobList") {
            Some("a job thread")
        } else if name.contains("TaskletManager") || name.contains("Tasklet") {
            Some("a tasklet thread")
        } else if name.contains("BSScript") || name.contains("SkyrimVM") || name.contains("VirtualMachine") {
            Some("a Papyrus VM thread")
        } else if name.contains("LibAudio") || name.contains("BSAudio") {
            Some("the audio thread")
        } else if name.contains("BSResource") || name.contains("IOManager") {
            Some("a resource loader thread")
        } else {
            None
        }
    };
    if id == rec.thread {
        return rec.frames().iter().map(|f| f.rip).filter_map(names).find_map(role);
    }
    let thread = rec.threads().iter().find(|t| t.id == id)?;
    thread.frames[..(thread.frame_count as usize).min(thread.frames.len())].iter().copied().filter_map(names).find_map(role)
}

pub fn thread_kind(w: &mut dyn Write, ctx: &Context<'_>, id: u32) -> fmt::Result {
    let rec = ctx.rec;
    if id == rec.main_thread {
        return write!(w, "thread {id} (the main thread)");
    }
    write!(w, "thread {id}")?;
    if let Some(thread) = rec.threads().iter().find(|t| t.id == id)
        && thread.name_len > 0
    {
        write!(w, " \"{}\"", thread.name())?;
    }
    if let Some(role) = thread_role(rec, id) {
        write!(w, " ({role})")?;
    }
    Ok(())
}

/// The access class of the exception's address, with the line that describes it.
fn access_line(w: &mut dyn Write, ctx: &Context<'_>) -> Result<Access, fmt::Error> {
    let e = &ctx.rec.exception;
    if matches!(e.code, 0xC000_0005 | 0xC000_0006 | 0xC000_00FD) && e.parameter_count >= 2 {
        let verb = match e.parameters[0] {
            0 => "reading",
            1 => "writing",
            8 => "executing",
            _ => "accessing",
        };
        write!(w, "{verb} address {:#x}: ", e.parameters[1])?;
        let access = classify(w, e.parameters[1], ctx)?;
        writeln!(w)?;
        return Ok(access);
    }
    if e.parameter_count > 0 {
        w.write_str("parameters:")?;
        for p in &e.parameters[..(e.parameter_count as usize).min(15)] {
            write!(w, " {p:#x}")?;
        }
        writeln!(w)?;
    }
    Ok(Access::None)
}

fn exception(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    let e = &rec.exception;
    let (name, meaning) = code_name(e.code);
    writeln!(w, "\nException")?;
    writeln!(w, "code {:#010x} {name}: {meaning}", e.code)?;
    w.write_str("at ")?;
    code_location(w, e.address, ctx, false)?;
    writeln!(w)?;
    let _ = access_line(w, ctx)?;
    w.write_str("on ")?;
    thread_kind(w, ctx, rec.thread)?;
    writeln!(w)?;
    let mut bytes = [0u8; 16];
    w.write_str("instruction bytes:")?;
    if e.address != 0 && ctx.src.read(e.address, &mut bytes) {
        for b in bytes {
            write!(w, " {b:02x}")?;
        }
    } else {
        w.write_str(" unreadable")?;
    }
    writeln!(w)
}

fn registers(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let r = &ctx.rec.registers;
    writeln!(w, "\nRegisters")?;
    for (name, value) in r.general() {
        write!(w, "{name:>3} {value:#018x}")?;
        if name != "rsp" && name != "rip" {
            w.write_str("  ")?;
            describe_value(w, value, ctx)?;
        }
        writeln!(w)?;
    }
    writeln!(w, "eflags {:#010x}  mxcsr {:#010x}  cs {:#x} ss {:#x} ds {:#x} es {:#x} fs {:#x} gs {:#x}", r.eflags, r.mxcsr, r.segments[0], r.segments[1], r.segments[2], r.segments[3], r.segments[4], r.segments[5])?;
    for (i, [lo, hi]) in r.xmm.iter().enumerate() {
        writeln!(w, "xmm{i:<2} {hi:016x}{lo:016x}  ({}, {})", f32::from_bits((*lo & 0xFFFF_FFFF) as u32), f32::from_bits((*lo >> 32) as u32))?;
    }
    Ok(())
}

/// A word from a register or the stack: a code or data address in a module, a stack address, a
/// game object named by its vtable, or what the memory it points at is.
pub fn describe_value(w: &mut dyn Write, value: u64, ctx: &Context<'_>) -> fmt::Result {
    if value == 0 {
        return w.write_str("null");
    }
    if value < 0x10000 {
        return write!(w, "small value {value}");
    }
    if ctx.rec.module_at(value).is_some() {
        if ctx.src.region(value).executable() {
            return code_location(w, value, ctx, false);
        }
        classify(w, value, ctx)?;
        return Ok(());
    }
    if probe::describe(w, value, ctx)? {
        return Ok(());
    }
    classify(w, value, ctx)?;
    Ok(())
}

fn stack(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    writeln!(w, "\nCall stack of the crashing thread ({} frames)", rec.frame_count)?;
    for (i, frame) in rec.frames().iter().enumerate() {
        write!(w, "#{i:<2} {:#018x} ", frame.rip)?;
        code_location(w, frame.rip, ctx, i > 0)?;
        writeln!(w)?;
    }
    if let Some(pdb) = ctx.pdb {
        pdb.status(w)?;
        writeln!(w)?;
    }
    Ok(())
}

/// The qwords above the stack pointer, each described; nulls and plain numbers are skipped.
fn stack_top(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rsp = ctx.rec.registers.rsp;
    writeln!(w, "\nStack top ({STACK_WORDS} words from rsp {rsp:#x}; nulls and small values left out)")?;
    for i in 0..STACK_WORDS {
        let at = rsp + i * 8;
        let Some(value) = ctx.src.read_u64(at) else {
            writeln!(w, "rsp+{:#05x} unreadable", i * 8)?;
            break;
        };
        if value < 0x10000 {
            continue;
        }
        write!(w, "rsp+{:#05x} {value:#018x}  ", i * 8)?;
        describe_value(w, value, ctx)?;
        writeln!(w)?;
    }
    Ok(())
}

fn thread_line(w: &mut dyn Write, ctx: &Context<'_>, thread: &Thread) -> fmt::Result {
    thread_kind(w, ctx, thread.id)?;
    if thread.id == ctx.rec.thread {
        w.write_str(" (crashing)")?;
    }
    if thread.frame_count == 0 {
        return writeln!(w, ": no context");
    }
    writeln!(w)?;
    for (i, rip) in thread.frames[..(thread.frame_count as usize).min(thread.frames.len())].iter().enumerate() {
        write!(w, "    #{i} ")?;
        code_location(w, *rip, ctx, i > 0)?;
        writeln!(w)?;
    }
    Ok(())
}

fn threads(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    if rec.thread_count == u32::MAX {
        return writeln!(w, "\nThreads: the thread list could not be read");
    }
    writeln!(w, "\nThreads ({})", rec.thread_count)?;
    for thread in rec.threads() {
        thread_line(w, ctx, thread)?;
    }
    Ok(())
}

fn modules(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    writeln!(w, "\nModules ({})", rec.module_count)?;
    for module in rec.modules() {
        let in_crash = rec.frames().iter().any(|f| module.contains(f.rip));
        let in_threads = rec.threads().iter().any(|t| t.frames[..(t.frame_count as usize).min(t.frames.len())].iter().any(|&rip| module.contains(rip)));
        write!(w, "{:#018x} {:>10x} ", module.base, module.size)?;
        if module.version_known != 0 {
            let v = module.version;
            write!(w, "{}.{}.{}.{}", v[0], v[1], v[2], v[3])?;
        } else {
            w.write_str("-")?;
        }
        write!(w, " stamp {:#010x} {}", module.timestamp, module.path())?;
        if let Some(image) = rec.images().iter().find(|i| i.live != 0 && i.base == module.base) {
            write!(w, " (framework plugin {} generation {})", image.name(), image.generation)?;
        }
        if in_crash {
            w.write_str(" [in the crashing stack]")?;
        } else if in_threads {
            w.write_str(" [in another thread's stack]")?;
        }
        writeln!(w)?;
    }
    for image in rec.images().iter().filter(|i| i.live == 0) {
        writeln!(w, "{:#018x} {:>10x} freed: {} generation {}, unloaded by a hot reload", image.base, image.size, image.name(), image.generation)?;
    }
    Ok(())
}

fn state(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    if ctx.rec.state_len > 0 {
        writeln!(w, "\nFramework state\n{}", ctx.rec.state_text().trim_end())?;
    }
    Ok(())
}

/// One top frame's name as the hash and the rules see it: `module!function` (the ledger's name in
/// the game, the PDB's or export's in a DLL) or `module+offset`; the bytes stored in `buf`.
fn frame_name(ctx: &Context<'_>, index: usize, buf: &mut [u8]) -> usize {
    let rec = ctx.rec;
    let frame = &rec.frames()[index];
    let lookup = if index > 0 { frame.rip.wrapping_sub(1) } else { frame.rip };
    let Some(module) = rec.module_at(lookup) else {
        return crate::record::store(buf, "unmapped");
    };
    let mut w = crate::text::Writer::new(buf);
    let _ = w.write_str(module.name());
    let module_len = w.len();
    let named = if module.base == rec.exe_base {
        match u32::try_from(lookup - module.base).ok().and_then(symbols::code) {
            Some(code) => {
                let _ = write!(w, "!{}", code.function.name);
                true
            }
            None => false,
        }
    } else {
        ctx.pdb.is_some_and(|pdb| pdb.describe(lookup, module, &mut w, MAX_EXPORT_DISPLACEMENT).unwrap_or(false))
    };
    if !named {
        let _ = write!(w, "+{:#x}", lookup - module.base);
        return w.len();
    }
    let len = w.len();
    if buf[module_len] == b' ' {
        buf[module_len] = b'!';
    }
    buf[module_len..len].iter().position(|&b| b == b'+').map_or(len, |p| module_len + p)
}

const TOP_FRAMES: usize = 8;

/// The verdict first: what happened where and on which thread, the rule's explanation, the culprit, the signature.
fn summary(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    let mut buffers = [[0u8; 192]; TOP_FRAMES];
    let count = rec.frames().len().min(TOP_FRAMES);
    let lens: [usize; TOP_FRAMES] = core::array::from_fn(|i| if i < count { frame_name(ctx, i, &mut buffers[i]) } else { 0 });
    let names: [&str; TOP_FRAMES] = core::array::from_fn(|i| crate::record::text(&buffers[i], lens[i]));
    let names = &names[..count];
    writeln!(w, "\nSummary")?;
    let (name, _) = code_name(rec.exception.code);
    write!(w, "{name} at ")?;
    code_location(w, rec.exception.address, ctx, false)?;
    w.write_str(" on ")?;
    thread_kind(w, ctx, rec.thread)?;
    writeln!(w)?;
    let access = access_line(w, ctx)?;
    let seen = ctx.seen.borrow();
    let signature = rules::Signature { code: rec.exception.code, access, frames: names, seen: &seen, rip_unmapped: rec.module_at(rec.exception.address).is_none() };
    match rules::explain(&signature) {
        Some(rule) => {
            writeln!(w, "rule {}: {}", rule.name, rule.explanation)?;
            write!(w, "likely culprit: ")?;
            culprit(w, ctx, rule.culprit, &seen, names)?;
            writeln!(w)?;
        }
        None => writeln!(w, "no rule matches this crash; the sections below are what there is")?,
    }
    write!(w, "crash signature {:016x} over the top {} frames:", rules::signature_hash(names), count)?;
    for (i, name) in names.iter().enumerate() {
        write!(w, "{}{name}", if i == 0 { " " } else { " < " })?;
    }
    writeln!(w)
}

fn culprit(w: &mut dyn Write, ctx: &Context<'_>, culprit: rules::Culprit, seen: &Seen, names: &[&str]) -> fmt::Result {
    let rec = ctx.rec;
    let module_of = |frame: usize| rec.frames().get(frame).and_then(|f| rec.module_at(if frame > 0 { f.rip.wrapping_sub(1) } else { f.rip }));
    let describe_module = |w: &mut dyn Write, module: &crate::record::Module| -> fmt::Result {
        w.write_str(module.name())?;
        if let Some(image) = rec.images().iter().find(|i| i.live != 0 && i.base == module.base) {
            write!(w, " (framework plugin {} generation {})", image.name(), image.generation)?;
        }
        Ok(())
    };
    match culprit {
        rules::Culprit::Engine => w.write_str("the engine itself"),
        rules::Culprit::TopFrame => match module_of(0) {
            Some(module) => describe_module(w, module),
            None => write!(w, "unknown code at {:#x}", rec.exception.address),
        },
        rules::Culprit::FirstPlugin => {
            let system = |m: &crate::record::Module| m.base == rec.exe_base || m.path().to_ascii_lowercase().contains("\\windows\\");
            match (0..rec.frames().len()).filter_map(module_of).find(|m| !system(m)) {
                Some(module) => describe_module(w, module),
                None => write!(w, "no plugin on this stack ({} frames, all engine or system): the engine, or a plugin's earlier change to the data", names.len()),
            }
        }
        rules::Culprit::FormOwner => {
            if seen.form_file_len > 0 {
                write!(w, "the plugin file {} (the owner of the form involved)", seen.form_file())
            } else {
                w.write_str("the plugin owning the form involved (its file was not readable)")
            }
        }
        rules::Culprit::FreedImage => write!(w, "whoever kept a pointer into the unloaded generation of {}, or that plugin for handing it out", seen.freed_plugin()),
    }
}
