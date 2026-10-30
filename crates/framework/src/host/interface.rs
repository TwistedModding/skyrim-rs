//! C++ interface objects a framework plugin hands to other SKSE plugins (`Host::export_interface`).
//! The object and its virtual table belong to the host; each slot is a thunk that jumps through an
//! address the host re-points: at the plugin's function while it is loaded, at a stand-in that
//! returns zero once it is unloaded, so a pointer another plugin keeps never reaches a freed DLL.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

use crate::{Error, Method, Returns};

const PAGE: usize = 4096;
/// `jmp [rip+2]`, two pad bytes, then the 8-byte target, aligned.
const THUNK: usize = 16;
const JMP: [u8; 8] = [0xFF, 0x25, 0x02, 0x00, 0x00, 0x00, 0xCC, 0xCC];
pub(super) const MAX_METHODS: usize = PAGE / THUNK;

struct Exported {
    owner: &'static str,
    name: String,
    /// The object: one word, the virtual table's address.
    object: usize,
    page: usize,
    returns: Vec<Returns>,
}

static EXPORTED: Mutex<Vec<Exported>> = Mutex::new(Vec::new());

extern "C" fn zero() -> u64 {
    0
}

extern "C" fn zero_float() -> f64 {
    0.0
}

/// A method returning a structure through the caller's buffer, which comes after `this`.
extern "C" fn buffer(_this: usize, out: usize) -> usize {
    out
}

fn stand_in(returns: Returns) -> usize {
    match returns {
        Returns::Value => zero as *const () as usize,
        Returns::Float => zero_float as *const () as usize,
        Returns::Buffer => buffer as *const () as usize,
    }
}

fn target(page: usize, slot: usize) -> &'static AtomicUsize {
    unsafe { AtomicUsize::from_ptr((page + slot * THUNK + JMP.len()) as *mut usize) }
}

/// The object `owner` exports as `name`, made at the first call and re-pointed at `methods` by a
/// later one (a new generation); the method list's length is fixed by the first.
pub(super) fn export(owner: &'static str, name: &str, methods: &[Method]) -> Result<usize, Error> {
    let mut exported = EXPORTED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(e) = exported.iter().find(|e| e.name == name) {
        if e.owner != owner {
            return Err(Error::Taken { what: format!("interface {name}"), owner: e.owner.to_owned() });
        }
        for (slot, m) in methods.iter().enumerate().take(e.returns.len()) {
            target(e.page, slot).store(m.f, Ordering::Release);
        }
        return Ok(e.object);
    }
    if methods.len() > MAX_METHODS {
        return Err(Error::Full { what: format!("methods of interface {name}"), limit: MAX_METHODS });
    }
    let page = unsafe { VirtualAlloc(core::ptr::null(), PAGE, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
    if page == 0 {
        return Err(Error::Full { what: format!("interface {name}: no executable page"), limit: 0 });
    }
    for (slot, m) in methods.iter().enumerate() {
        unsafe { core::ptr::copy_nonoverlapping(JMP.as_ptr(), (page + slot * THUNK) as *mut u8, JMP.len()) };
        target(page, slot).store(m.f, Ordering::Release);
    }
    let table: &'static [usize] = Box::leak((0..methods.len()).map(|slot| page + slot * THUNK).collect());
    let object = core::ptr::from_ref::<usize>(Box::leak(Box::new(table.as_ptr() as usize))) as usize;
    exported.push(Exported { owner, name: name.to_owned(), object, page, returns: methods.iter().map(|m| m.returns).collect() });
    Ok(object)
}

/// Points every method of `owner`'s objects at its stand-in; returns how many objects.
pub(super) fn remove_owner(owner: &str) -> usize {
    let exported = EXPORTED.lock().unwrap_or_else(PoisonError::into_inner);
    let mine: Vec<&Exported> = exported.iter().filter(|e| e.owner == owner).collect();
    for e in &mine {
        for (slot, returns) in e.returns.iter().enumerate() {
            target(e.page, slot).store(stand_in(*returns), Ordering::Release);
        }
    }
    mine.len()
}

/// One line per object for the console inspector.
pub(super) fn lines() -> Vec<String> {
    EXPORTED.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|e| format!("interface {} ({}): object at {:#x}, {} methods", e.name, e.owner, e.object, e.returns.len())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Point {
        x: f32,
        y: f32,
        z: f32,
    }

    type Vtable = [usize; 3];

    extern "C" fn twice(_this: *const usize, x: u32) -> u32 {
        x * 2
    }

    extern "C" fn half(_this: *const usize, x: f32) -> f32 {
        x / 2.0
    }

    extern "C" fn origin(_this: *const usize, out: *mut Point) -> *mut Point {
        unsafe { out.write(Point { x: 1.0, y: 2.0, z: 3.0 }) };
        out
    }

    extern "C" fn thrice(_this: *const usize, x: u32) -> u32 {
        x * 3
    }

    fn call(object: usize) -> (u32, f32, f32) {
        let table = unsafe { &*(*(object as *const usize) as *const Vtable) };
        let first: extern "C" fn(usize, u32) -> u32 = unsafe { core::mem::transmute(table[0]) };
        let second: extern "C" fn(usize, f32) -> f32 = unsafe { core::mem::transmute(table[1]) };
        let third: extern "C" fn(usize, *mut Point) -> *mut Point = unsafe { core::mem::transmute(table[2]) };
        let mut p = Point { x: -1.0, y: -1.0, z: -1.0 };
        let out = third(object, &raw mut p);
        assert_eq!(out, &raw mut p, "the buffer comes back");
        (first(object, 21), second(object, 3.0), p.z)
    }

    #[test]
    fn calls_reach_the_plugin_until_it_goes_and_its_next_generation_after() {
        let methods = [Method::new(twice as *const (), Returns::Value), Method::new(half as *const (), Returns::Float), Method::new(origin as *const (), Returns::Buffer)];
        let object = export("iface-a", "TestInterface", &methods).unwrap();
        assert_eq!(call(object), (42, 1.5, 3.0));
        assert!(matches!(export("iface-b", "TestInterface", &methods), Err(Error::Taken { .. })));
        assert_eq!(remove_owner("iface-a"), 1);
        assert_eq!(call(object), (0, 0.0, -1.0), "the stand-ins return zero and leave the buffer");
        let again = [Method::new(thrice as *const (), Returns::Value), methods[1], methods[2]];
        assert_eq!(export("iface-a", "TestInterface", &again).unwrap(), object, "the same object");
        assert_eq!(call(object), (63, 1.5, 3.0));
        assert!(lines().iter().any(|l| l.starts_with("interface TestInterface (iface-a)") && l.ends_with("3 methods")));
        assert_eq!(remove_owner("iface-a"), 1);
    }
}
