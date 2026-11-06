// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! Game objects named from a pointer: the vtable in the first word identifies the class through
//! the ledger, and the class family decides what else is read, every byte through the guarded
//! source: a form's id, type, flags and owning file; a reference's base object, cell and
//! position; an actor's name; a scene node's name and parents; a navmesh's file; an interned
//! string's text. The layouts are 1.7.104's, so on another runtime only the class is named.

use core::fmt::{self, Write};

use sre::FormType;
use sre::raw::symbols as ledger;

use crate::mem::Source;
use crate::record::Record;
use crate::report::Context;

const FORM_FLAGS: u64 = 0x10;
const FORM_ID: u64 = 0x14;
const FORM_TYPE: u64 = 0x1A;
const SOURCE_FILES: u64 = 0x8;
const FILE_NAME: u64 = 0x58;
const REF_COUNT: u64 = 0x28;
const BASE_OBJECT: u64 = 0x40;
const POSITION: u64 = 0x54;
const PARENT_CELL: u64 = 0x60;
const CELL_NAME: u64 = 0x28;
const NPC_FULL_NAME: u64 = 0xE0;
const ACTOR_RACE: u64 = 0x1F8;
const NODE_NAME: u64 = 0x10;
const NODE_PARENT: u64 = 0x30;
const STRING_ENTRY_LENGTH: u64 = 0x8;
const FLAG_DELETED: u32 = 0x20;
const FLAG_DISABLED: u32 = 0x800;
const LIGHT_INDEX: u32 = 0xFE;
const RUNTIME_INDEX: u32 = 0xFF;

/// The ledger's class for the object at `address`, from its first word.
fn class_of(address: u64, rec: &Record, src: &dyn Source) -> Option<&'static sre::raw::Vtable> {
    let vtable = src.read_u64(address)?;
    if vtable == 0 || rec.exe_base == 0 || rec.module_at(vtable).is_none_or(|m| m.base != rec.exe_base) {
        return None;
    }
    crate::symbols::vtable(u32::try_from(vtable - rec.exe_base).ok()?)
}

/// A string the pool interned, by the text pointer a `BSFixedString` holds: the entry header
/// before it must record the text's length.
fn fixed_string(w: &mut dyn Write, address: u64, src: &dyn Source) -> Result<bool, fmt::Error> {
    let Some(length) = src.read_u32(address.wrapping_sub(0x18).wrapping_add(STRING_ENTRY_LENGTH)) else { return Ok(false) };
    if length == 0 || length > 200 {
        return Ok(false);
    }
    let mut text = [0u8; 256];
    let Some(len) = src.read_cstr(address, &mut text) else { return Ok(false) };
    if len != length as usize || !text[..len].iter().all(|b| (0x20..0x7F).contains(b)) {
        return Ok(false);
    }
    write!(w, "BSFixedString \"{}\"", crate::record::text(&text, len))?;
    Ok(true)
}

fn text_at(w: &mut dyn Write, pointer: Option<u64>, src: &dyn Source) -> fmt::Result {
    let mut buf = [0u8; 128];
    match pointer.filter(|&p| p != 0).and_then(|p| src.read_cstr(p, &mut buf)) {
        Some(len) => write!(w, "\"{}\"", crate::record::text(&buf, len)),
        None => w.write_str("(no name)"),
    }
}

/// The form's id, type, flags and the last file it was read from.
fn form(w: &mut dyn Write, address: u64, ctx: &Context<'_>) -> fmt::Result {
    let src = ctx.src;
    let Some(id) = src.read_u32(address + FORM_ID) else { return w.write_str(" (form header unreadable)") };
    write!(w, " form {id:08X}")?;
    if let Some(kind) = src.read_u8(address + FORM_TYPE) {
        match FormType::from_u8(kind) {
            Some(t) => write!(w, " {}", t.signature())?,
            None => write!(w, " type {kind}")?,
        }
    }
    if let Some(flags) = src.read_u32(address + FORM_FLAGS) {
        if flags & FLAG_DELETED != 0 {
            w.write_str(" DELETED")?;
            ctx.seen.borrow_mut().deleted_form = true;
        }
        if flags & FLAG_DISABLED != 0 {
            w.write_str(" disabled")?;
        }
    }
    let index = id >> 24;
    match src.read_u64(address + SOURCE_FILES).filter(|&p| p != 0) {
        Some(array) => {
            let (Some(count), Some(files)) = (src.read_u32(array + 8), src.read_u64(array)) else {
                return w.write_str(", source files not readable");
            };
            if count == 0 || files == 0 {
                w.write_str(", no source file")?;
            } else if let Some(file) = src.read_u64(files + u64::from(count - 1) * 8).filter(|&f| f != 0) {
                w.write_str(", from ")?;
                text_at(w, Some(file + FILE_NAME), src)?;
                let mut name = [0u8; 64];
                if let Some(len) = src.read_cstr(file + FILE_NAME, &mut name) {
                    ctx.seen.borrow_mut().note_form_file(crate::record::text(&name, len));
                }
                if count > 1 {
                    write!(w, " (last of {count} files)")?;
                }
            }
        }
        None if index == RUNTIME_INDEX => w.write_str(", created at runtime")?,
        None => w.write_str(", no source file array")?,
    }
    if index == LIGHT_INDEX {
        write!(w, ", light file index {}", (id >> 12) & 0xFFF)?;
    }
    Ok(())
}

fn reference(w: &mut dyn Write, address: u64, ctx: &Context<'_>) -> fmt::Result {
    let (rec, src) = (ctx.rec, ctx.src);
    if let Some(count) = src.read_u32(address + REF_COUNT) {
        write!(w, ", refcount {}", count & 0x3FF)?;
    }
    match src.read_u64(address + BASE_OBJECT) {
        Some(0) => {
            w.write_str(", NULL BASE OBJECT")?;
            ctx.seen.borrow_mut().null_base = true;
        }
        Some(base) => {
            w.write_str(", base ")?;
            match class_of(base, rec, src) {
                Some(vt) => {
                    w.write_str(vt.class)?;
                    form(w, base, ctx)?;
                }
                None => write!(w, "{base:#x} (not a known object)")?,
            }
        }
        None => w.write_str(", base object unreadable")?,
    }
    match src.read_u64(address + PARENT_CELL) {
        Some(0) => w.write_str(", not in a cell")?,
        Some(cell) => {
            write!(w, ", in cell {:08X} ", src.read_u32(cell + FORM_ID).unwrap_or(0))?;
            text_at(w, src.read_u64(cell + CELL_NAME), src)?;
        }
        None => {}
    }
    let pos: [f32; 3] = core::array::from_fn(|i| src.read_f32(address + POSITION + 4 * i as u64).unwrap_or(f32::NAN));
    write!(w, " at ({:.0}, {:.0}, {:.0})", pos[0], pos[1], pos[2])
}

fn actor(w: &mut dyn Write, address: u64, src: &dyn Source) -> fmt::Result {
    if let Some(base) = src.read_u64(address + BASE_OBJECT).filter(|&b| b != 0) {
        w.write_str(", name ")?;
        text_at(w, src.read_u64(base + NPC_FULL_NAME), src)?;
    }
    if let Some(race) = src.read_u64(address + ACTOR_RACE).filter(|&r| r != 0) {
        write!(w, ", race {:08X}", src.read_u32(race + FORM_ID).unwrap_or(0))?;
    }
    Ok(())
}

fn node(w: &mut dyn Write, address: u64, rec: &Record, src: &dyn Source) -> fmt::Result {
    w.write_str(" ")?;
    text_at(w, src.read_u64(address + NODE_NAME), src)?;
    let mut parent = src.read_u64(address + NODE_PARENT).unwrap_or(0);
    let mut depth = 0;
    while parent != 0 && depth < 8 {
        w.write_str(if depth == 0 { ", under " } else { " < " })?;
        match class_of(parent, rec, src) {
            Some(vt) => write!(w, "{} ", vt.class)?,
            None => write!(w, "{parent:#x} ")?,
        }
        text_at(w, src.read_u64(parent + NODE_NAME), src)?;
        parent = src.read_u64(parent + NODE_PARENT).unwrap_or(0);
        depth += 1;
    }
    Ok(())
}

/// Names the object at `address`; false when the word there is not a vtable the ledger knows and
/// the address is not an interned string.
pub fn describe(w: &mut dyn Write, address: u64, ctx: &Context<'_>) -> Result<bool, fmt::Error> {
    let (rec, src) = (ctx.rec, ctx.src);
    let canonical = skse::runtime::is_canonical(rec.runtime);
    let Some(vt) = class_of(address, rec, src) else {
        return if canonical { fixed_string(w, address, src) } else { Ok(false) };
    };
    if !canonical {
        w.write_str(vt.class)?;
        return Ok(true);
    }
    if vt.offset != 0 {
        write!(w, "{} (the subobject at +{:#x}; the object starts at {:#x})", vt.class, vt.offset, address.wrapping_sub(u64::from(vt.offset)))?;
        return Ok(true);
    }
    w.write_str(vt.class)?;
    let derives = |base: &str| ledger::derives(vt.class, base);
    if derives("TESForm") {
        form(w, address, ctx)?;
        if derives("TESObjectREFR") {
            reference(w, address, ctx)?;
        }
        if derives("Actor") {
            actor(w, address, src)?;
        }
        if derives("NavMesh") {
            w.write_str(" (navmesh; its cell is not in the ledger)")?;
            ctx.seen.borrow_mut().navmesh = true;
        }
    } else if derives("NiAVObject") {
        node(w, address, rec, src)?;
    }
    Ok(true)
}
