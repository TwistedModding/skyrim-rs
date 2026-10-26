//! What Papyrus code the crashing thread runs (RE request CR, `SkyrimRE-1.7.104/docs/subsystems/papyrus.md`):
//! the VM keeps no per-thread record, so the stack is found from the frozen thread's own frames
//! through four return-address anchors inside the executor, `NativeFunctionBase::Call`, the
//! interpreter and the job worker; from the `BSScript::Stack` each `StackFrame`'s function object
//! names a script function (with its source file and bit position) or a native (with the DLL that
//! implements it). Every read goes through the guarded source.

use core::fmt::{self, Write};

use layout::v1_7_104::{fns, symbols as ledger, vtables};

use crate::mem::Source;
use crate::report::{Context, code_location};
use crate::symbols;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A native called by the executor: the `Stack` and the native are in the executor's frame.
    Native,
    /// Inside `NativeFunctionBase::Call`, the binding running: the native's `StackFrame*` is saved below the return address.
    Call,
    /// The interpreter called the executor: its tasklet is below the return address.
    Interpreter,
    /// A job worker running bytecode: the job entry holds the tasklet.
    Job,
}

/// A return address as a function id plus the offset of the call's return inside it.
struct Anchor {
    function: u64,
    offset: u32,
    kind: Kind,
    what: &'static str,
}

const ANCHORS: [Anchor; 5] = [
    Anchor { function: fns::VM_ExecuteFunctionCall, offset: 0x789, kind: Kind::Native, what: "a native called by the executor" },
    Anchor { function: fns::BSScript__NF_util__NativeFunctionBase__Call, offset: 0x205, kind: Kind::Call, what: "a native's binding, inside Call" },
    Anchor { function: fns::BSScript__NF_util__NativeFunctionBase__Call, offset: 0x10c, kind: Kind::Call, what: "a latent native's binding, inside Call" },
    Anchor { function: fns::CodeTasklet_Interpret, offset: 0x834, kind: Kind::Interpreter, what: "the interpreter's call into the executor" },
    Anchor { function: fns::ExecuteJobListJobs, offset: 0xd8, kind: Kind::Job, what: "a job worker in script bytecode" },
];

const STACK_FRAME_COUNT: u64 = 0x58;
const STACK_TOP: u64 = 0x60;
const STACK_STATE: u64 = 0x68;
const STACK_ID: u64 = 0x80;
const FRAME_PREVIOUS: u64 = 0x8;
const FRAME_FUNCTION: u64 = 0x10;
const FRAME_TYPE: u64 = 0x18;
const FRAME_IP: u64 = 0x20;
const TYPE_NAME: u64 = 0x8;
const FUNCTION_NAME: u64 = 0x10;
const FUNCTION_TYPE_NAME: u64 = 0x18;
const SCRIPT_BIT_COUNT: u64 = 0x48;
const SCRIPT_SOURCE: u64 = 0x68;
const SCRIPT_LINE_COUNT: u64 = 0x70;
const NATIVE_CALLBACK: u64 = 0x50;
const AUTOPROP_TYPE_NAME: u64 = 0x10;
const AUTOPROP_NAME: u64 = 0x18;
const TASKLET_STACK: u64 = 0x10;
/// In the executor's frame (E = the slot above its return address): the `Stack`, the native, its name, its class, the call kind.
const EXECUTOR_STACK: u64 = 0x80;
const EXECUTOR_STACK_COPY: u64 = 0x0;
const EXECUTOR_NATIVE: u64 = 0x50;
const EXECUTOR_NAME: u64 = 0x30;
const EXECUTOR_CLASS: u64 = 0x38;
const EXECUTOR_KIND: u64 = 0x48;
/// Below `Call`'s return address: the native's `StackFrame*`.
const CALL_FRAME: u64 = 0x30;
/// Below the interpreter's return address: its tasklet.
const INTERPRETER_TASKLET: u64 = 0xa08;
/// Around the job worker's return address: the job list and its context, the index into the list, the tasklet in the entry.
const JOB_LIST: u64 = 0x20;
const JOB_CONTEXT: u64 = 0x10;
const JOB_INDEX: u64 = 0x10;
const JOB_ENTRY_SIZE: u64 = 0x10;
const JOB_TASKLET: u64 = 0x8;
const MAX_FRAMES: u32 = 16;

fn text(w: &mut dyn Write, pointer: Option<u64>, src: &dyn Source) -> fmt::Result {
    let mut buf = [0u8; 128];
    match pointer.filter(|&p| p != 0).and_then(|p| src.read_cstr(p, &mut buf)) {
        Some(len) => w.write_str(crate::record::text(&buf, len)),
        None => w.write_str("?"),
    }
}

/// The class of the object at `address` by its vtable, when the ledger knows it.
fn class_of(address: u64, ctx: &Context<'_>) -> Option<&'static str> {
    let vtable = ctx.src.read_u64(address)?;
    let exe = ctx.rec.module_at(vtable).filter(|m| m.base == ctx.rec.exe_base)?;
    vtables::find(u32::try_from(vtable - exe.base).ok()?).map(|v| v.class)
}

/// Whether the address is a `CodeTasklet`.
fn is_tasklet(address: u64, ctx: &Context<'_>) -> bool {
    class_of(address, ctx) == Some("BSScript::Internal::CodeTasklet")
}

/// Whether the address looks like a `Stack`: a plausible frame count and a non-null top when frames exist.
fn is_stack(address: u64, ctx: &Context<'_>) -> bool {
    let src = ctx.src;
    match (src.read_u32(address + STACK_FRAME_COUNT), src.read_u64(address + STACK_TOP), src.read_u32(address + STACK_ID)) {
        (Some(count), Some(top), Some(id)) => count < 1000 && (count == 0 || top != 0) && id != 0,
        _ => false,
    }
}

/// The `Stack` an anchor's frame leads to; `e` is the slot above the return address.
fn stack_from(anchor: &Anchor, e: u64, ctx: &Context<'_>) -> Option<u64> {
    let src = ctx.src;
    let a = e - 8;
    let stack = match anchor.kind {
        Kind::Native => {
            let stack = src.read_u64(e + EXECUTOR_STACK)?;
            if src.read_u64(e + EXECUTOR_STACK_COPY) != Some(stack) {
                return None;
            }
            stack
        }
        Kind::Call => src.read_u64(src.read_u64(a + CALL_FRAME)?)?,
        Kind::Interpreter => {
            let tasklet = src.read_u64(a + INTERPRETER_TASKLET)?;
            if !is_tasklet(tasklet, ctx) {
                return None;
            }
            src.read_u64(tasklet + TASKLET_STACK)?
        }
        Kind::Job => {
            let list = src.read_u64(a - JOB_LIST)?;
            let context = src.read_u64(a - JOB_CONTEXT)?;
            let index = src.read_u32(context + JOB_INDEX)?;
            let entry = src.read_u64(list)? + u64::from(index) * JOB_ENTRY_SIZE;
            let tasklet = src.read_u64(entry + JOB_TASKLET)?;
            if !is_tasklet(tasklet, ctx) {
                return None;
            }
            src.read_u64(tasklet + TASKLET_STACK)?
        }
    };
    (stack != 0 && is_stack(stack, ctx)).then_some(stack)
}

/// Names one frame's function object: a script function, a native with its DLL, an auto property accessor or a stub.
fn function(w: &mut dyn Write, object: u64, ctx: &Context<'_>) -> fmt::Result {
    let src = ctx.src;
    let class = class_of(object, ctx);
    let native_layout = class.is_none_or(|c| ledger::derives(c, "BSScript::NF_util::NativeFunctionBase") || c == "BSScript::Internal::NativeFunctionStub");
    match class {
        Some("BSScript::Internal::ScriptFunction") => {
            text(w, src.read_u64(object + FUNCTION_TYPE_NAME), src)?;
            w.write_char('.')?;
            text(w, src.read_u64(object + FUNCTION_NAME), src)?;
            w.write_str(": script function from \"")?;
            text(w, src.read_u64(object + SCRIPT_SOURCE), src)?;
            write!(w, "\", {} bits of code", src.read_u32(object + SCRIPT_BIT_COUNT).unwrap_or(0))?;
            match src.read_u32(object + SCRIPT_LINE_COUNT) {
                Some(0) | None => w.write_str(", no line table (bLoadDebugInformation:Papyrus is off)"),
                Some(n) => write!(w, ", a line table of {n} entries (the instruction index is not decoded)"),
            }
        }
        Some(c) if c == "BSScript::Internal::AutoPropGetFunction" || c == "BSScript::Internal::AutoPropSetFunction" => {
            text(w, src.read_u64(object + AUTOPROP_TYPE_NAME), src)?;
            w.write_char('.')?;
            text(w, src.read_u64(object + AUTOPROP_NAME), src)?;
            write!(w, ": auto property {}", if c.contains("Get") { "getter" } else { "setter" })
        }
        _ if native_layout => {
            text(w, src.read_u64(object + FUNCTION_TYPE_NAME), src)?;
            w.write_char('.')?;
            text(w, src.read_u64(object + FUNCTION_NAME), src)?;
            if class == Some("BSScript::Internal::NativeFunctionStub") {
                return w.write_str(": native declared by the script but bound to no implementation");
            }
            w.write_str(": native")?;
            if class.is_none()
                && let Some(vtable) = src.read_u64(object).filter(|&v| v != 0)
            {
                w.write_str(" (function object of ")?;
                match ctx.rec.module_at(vtable) {
                    Some(module) => w.write_str(module.name())?,
                    None => write!(w, "{vtable:#x}")?,
                }
                w.write_char(')')?;
            }
            match src.read_u64(object + NATIVE_CALLBACK).filter(|&c| c != 0) {
                Some(callback) => {
                    w.write_str(", implemented at ")?;
                    code_location(w, callback, ctx, false)
                }
                None => w.write_str(", no callback"),
            }
        }
        Some(c) => write!(w, "a function object of class {c}"),
        None => unreachable!(),
    }
}

/// The frames of `stack` from its top, as the log shows them.
fn frames(w: &mut dyn Write, stack: u64, ctx: &Context<'_>) -> fmt::Result {
    let src = ctx.src;
    let count = src.read_u32(stack + STACK_FRAME_COUNT).unwrap_or(0);
    let state = src.read_u32(stack + STACK_STATE).unwrap_or(u32::MAX);
    let state_name = match state {
        0 => "running",
        1 => "returned",
        3 => "waiting on a latent native",
        _ => "queued or suspended",
    };
    writeln!(w, "stack {} ({count} frames, state {state} {state_name})", src.read_u32(stack + STACK_ID).unwrap_or(0))?;
    let mut frame = src.read_u64(stack + STACK_TOP).unwrap_or(0);
    let mut i = 0;
    while frame != 0 && i < MAX_FRAMES {
        write!(w, "  #{i} ")?;
        match src.read_u64(frame + FRAME_FUNCTION).filter(|&f| f != 0) {
            Some(function_object) => {
                function(w, function_object, ctx)?;
                if let Some(ip) = src.read_u32(frame + FRAME_IP) {
                    write!(w, ", at bit {ip}")?;
                }
                if let Some(type_info) = src.read_u64(frame + FRAME_TYPE).filter(|&t| t != 0) {
                    w.write_str(", on ")?;
                    text(w, src.read_u64(type_info + TYPE_NAME), src)?;
                }
            }
            None => w.write_str("a frame without a function")?,
        }
        writeln!(w)?;
        frame = src.read_u64(frame + FRAME_PREVIOUS).unwrap_or(0);
        i += 1;
    }
    Ok(())
}

/// The section: which anchor the crashing thread's frames hit, the stack it leads to and its frames.
pub fn write(w: &mut dyn Write, ctx: &Context<'_>) -> fmt::Result {
    let rec = ctx.rec;
    writeln!(w, "\nPapyrus")?;
    if rec.exe_base == 0 {
        return writeln!(w, "no game module");
    }
    for (i, frame) in rec.frames().iter().enumerate().skip(1) {
        let Some(rva) = frame.rip.checked_sub(rec.exe_base).and_then(|d| u32::try_from(d).ok()) else { continue };
        for anchor in &ANCHORS {
            if symbols::function_rva(anchor.function).and_then(|start| start.checked_add(anchor.offset)) != Some(rva) {
                continue;
            }
            let Some(stack) = stack_from(anchor, frame.rsp, ctx) else { continue };
            write!(w, "the crashing thread runs Papyrus: {} (frame #{i}), ", anchor.what)?;
            if anchor.kind == Kind::Native {
                w.write_str("the native in flight is ")?;
                text(w, ctx.src.read_u64(frame.rsp + EXECUTOR_CLASS).and_then(|c| ctx.src.read_u64(c + TYPE_NAME)), ctx.src)?;
                w.write_char('.')?;
                text(w, ctx.src.read_u64(frame.rsp + EXECUTOR_NAME), ctx.src)?;
                write!(w, " (call kind {}, function object {:#x}), ", ctx.src.read_u32(frame.rsp + EXECUTOR_KIND).unwrap_or(0), ctx.src.read_u64(frame.rsp + EXECUTOR_NATIVE).unwrap_or(0))?;
            }
            return frames(w, stack, ctx);
        }
    }
    writeln!(w, "the crashing thread is not inside the Papyrus VM (none of its frames returns into the executor, a native's Call, the interpreter or a job worker's bytecode)")
}
