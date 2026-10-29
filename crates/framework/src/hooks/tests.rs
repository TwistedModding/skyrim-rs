use core::ffi::c_void;
use core::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::ops::Raw;
use super::*;

type Fn2 = unsafe extern "C" fn(usize, usize) -> usize;

// Each body holds its own name so the linker's identical-code folding keeps one address per target.
macro_rules! targets {
    ($($name:ident) => {$(
        #[inline(never)]
        unsafe extern "C" fn $name(a: usize, b: usize) -> usize {
            black_box(stringify!($name));
            black_box(a).wrapping_add(black_box(b))
        }
    };
}

targets! { phases exclusive cycle ordering removal reentrant shape prepatched overwritten toggled }

fn call(f: Fn2, a: usize, b: usize) -> usize {
    unsafe { black_box(f)(a, b) }
}

fn raw(f: Fn2) -> Raw<Fn2> {
    Raw::at(f as usize)
}

#[test]
fn phases_run_in_order_and_see_the_outcome() {
    let hooks = &Hooks::HOST;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    assert_eq!(validate(phases as *const () as usize, None), Validation::Pristine);
    hooks.before(raw(phases), Options::new("b1"), |args| args.0 += 1).unwrap();
    hooks
        .around(raw(phases), Options::new("a1"), |args, next| if args.0 == 100 { 7 } else { unsafe { next.call((args.0, args.1 * 2)) } })
        .unwrap();
    hooks.after(raw(phases), Options::new("f1"), move |args, ret, outcome| log.lock().unwrap().push((*args, *ret, outcome))).unwrap();
    assert_eq!(call(phases, 1, 2), 6);
    assert_eq!(call(phases, 99, 1), 7);
    assert_eq!(*seen.lock().unwrap(), [((2, 4), 6, Outcome::Original), ((100, 1), 7, Outcome::Skipped)]);

    let report = hooks.diagnostics().into_iter().find(|s| s.address == phases as *const () as usize).unwrap();
    assert_eq!(report.validation, Validation::Pristine);
    assert!(matches!(report.current, Validation::Managed { .. }), "{}", report.current);
    assert_eq!(report.timing.calls, 2);
    assert_eq!(report.last_outcome, Some(Outcome::Skipped));
    let entries: Vec<(&str, Phase, u64)> = report.entries.iter().map(|e| (e.owner.as_str(), e.phase, e.timing.calls)).collect();
    assert_eq!(entries, [("b1", Phase::Before, 2), ("a1", Phase::Around, 2), ("f1", Phase::After, 2)]);
    assert!(report.to_string().contains("a1 Around"));
}

#[test]
fn replace_is_exclusive() {
    let hooks = &Hooks::HOST;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    hooks.replace(raw(exclusive), Options::new("r1"), |args, original| unsafe { original.call((args.0, 0)) } + 1000).unwrap();
    hooks.after(raw(exclusive), Options::new("f"), move |_, ret, outcome| log.lock().unwrap().push((*ret, outcome))).unwrap();
    let refused = hooks.replace(raw(exclusive), Options::new("r2"), |args, _| args.0).unwrap_err();
    assert_eq!(refused, Error::Exclusive { site: format!("raw {:#x}/2", exclusive as *const () as usize), existing: "r1".into(), refused: "r2".into() });
    assert_eq!(call(exclusive, 3, 4), 1003);
    assert_eq!(*seen.lock().unwrap(), [(1003, Outcome::Replaced)]);
}

#[test]
fn constraint_cycle_is_refused() {
    let hooks = &Hooks::HOST;
    hooks.before(raw(cycle), Options::new("p").before(&["q"]), |_| {}).unwrap();
    hooks.before(raw(cycle), Options::new("q").after(&["p"]), |_| {}).unwrap();
    let refused = hooks.before(raw(cycle), Options::new("q").before(&["p"]), |_| {}).unwrap_err();
    assert_eq!(refused, Error::Cycle { site: format!("raw {:#x}/2", cycle as *const () as usize), names: vec!["p".into(), "q".into()] });
    assert_eq!(hooks.diagnostics().into_iter().find(|s| s.address == cycle as *const () as usize).unwrap().entries.len(), 2);
}

#[test]
fn constraints_then_priority_then_rank_then_registration() {
    let hooks = &Hooks::HOST;
    let order = Arc::new(Mutex::new(String::new()));
    let tag = |t: char| {
        let order = Arc::clone(&order);
        move |_: &mut (usize, usize)| order.lock().unwrap().push(t)
    };
    hooks.set_rank("w", -1);
    hooks.before(raw(ordering), Options::new("x"), tag('x')).unwrap();
    hooks.before(raw(ordering), Options::new("y").after(&["x"]), tag('y')).unwrap();
    hooks.before(raw(ordering), Options::new("z").priority(5), tag('z')).unwrap();
    hooks.before(raw(ordering), Options::new("w"), tag('w')).unwrap();
    hooks.before(raw(ordering), Options::new("z").priority(5), tag('Z')).unwrap();
    call(ordering, 0, 0);
    assert_eq!(*order.lock().unwrap(), "zZwxy");
    hooks.set_rank("w", 1);
    order.lock().unwrap().clear();
    call(ordering, 0, 0);
    assert_eq!(*order.lock().unwrap(), "zZxyw");
}

#[test]
fn removal_waits_for_the_subscriber_in_flight() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);
    let hooks = &Hooks::HOST;
    hooks
        .before(raw(removal), Options::new("slow"), |_| {
            std::thread::sleep(Duration::from_millis(40));
            RUNS.fetch_add(1, SeqCst);
        })
        .unwrap();
    let caller = std::thread::spawn(|| {
        while !STOP.load(SeqCst) {
            call(removal, 1, 1);
        }
    });
    while RUNS.load(SeqCst) == 0 {
        std::thread::yield_now();
    }
    // The caller runs until stopped, so a removal that waited for every call would never return.
    let start = Instant::now();
    assert_eq!(hooks.remove_owner("slow"), 1);
    let runs = RUNS.load(SeqCst);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(RUNS.load(SeqCst), runs, "a callback ran after remove_owner returned");
    STOP.store(true, SeqCst);
    caller.join().unwrap();
    assert_eq!(hooks.remove_owner("slow"), 0);
}

#[test]
fn registration_inside_a_subscriber_is_refused() {
    static INSIDE: OnceLock<(Result<SubscriptionId, Error>, bool)> = OnceLock::new();
    let hooks = &Hooks::HOST;
    let id = hooks
        .before(raw(reentrant), Options::new("outer"), |_| {
            let nested = Hooks::HOST.before(raw(reentrant), Options::new("inner"), |_| {});
            let _ = INSIDE.set((nested, Hooks::HOST.remove_owner("outer") == 0));
        })
        .unwrap();
    call(reentrant, 1, 1);
    assert_eq!(INSIDE.get(), Some(&(Err(Error::Reentrant), true)));
    assert!(hooks.unsubscribe(id));
    assert!(!hooks.unsubscribe(id));
}

#[test]
fn raw_shape_is_fixed_per_address() {
    let hooks = &Hooks::HOST;
    hooks.before(raw(shape), Options::new("s"), |_| {}).unwrap();
    let other = Raw::<unsafe extern "C" fn(usize) -> usize>::at(shape as *const () as usize);
    assert!(matches!(hooks.before(other, Options::new("s"), |_| {}), Err(Error::Raw { .. })));
}

fn exe_name() -> String {
    std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned()
}

/// Writes `jmp rel32` to `to` at `at`, as another module's hook would, and returns the target of
/// the jump that was there.
fn patch_jump(at: usize, to: usize) -> usize {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let previous = unsafe { (at as *const [u8; 5]).read_unaligned() };
    let rel = |next: usize, to: usize| i32::try_from(to.wrapping_sub(next).cast_signed()).unwrap();
    let mut old = 0;
    unsafe { VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &raw mut old) };
    let mut jump = [0xE9, 0, 0, 0, 0];
    jump[1..].copy_from_slice(&rel(at + 5, to).to_le_bytes());
    unsafe { (at as *mut [u8; 5]).write_unaligned(jump) };
    unsafe { VirtualProtect(at as *const c_void, 5, old, &raw mut old) };
    (at + 5).wrapping_add_signed(i32::from_le_bytes([previous[1], previous[2], previous[3], previous[4]]) as isize)
}

#[test]
fn unmanaged_patches_are_refused_stacked_or_break_the_site() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn early(a: usize, b: usize) -> usize {
        black_box(a).wrapping_add(black_box(b)) * 10
    }
    unsafe extern "C" fn late(a: usize, b: usize) -> usize {
        unsafe { core::mem::transmute::<usize, Fn2>(NEXT.load(SeqCst))(a, b) * 100 }
    }
    let hooks = &Hooks::HOST;
    let address = prepatched as *const () as usize;
    patch_jump(address, early as *const () as usize);
    let validation = validate(address, None);
    assert!(matches!(validation, Validation::Unmanaged { target: Some(t), .. } if t == early as *const () as usize), "{validation}");
    assert_eq!(validation.patched_by(), Some(exe_name()));

    registry::set_compat(Compat::Refuse);
    let refused = hooks.before(raw(prepatched), Options::new("r"), |args| args.0 += 1).unwrap_err();
    assert!(matches!(&refused, Error::Site { validation: v, .. } if v.patched_by() == Some(exe_name())), "{refused}");
    assert!(refused.to_string().ends_with(&format!(", patched by {}", exe_name())), "{refused}");
    assert_eq!(call(prepatched, 1, 2), 30);

    registry::set_compat(Compat::Stack);
    hooks.before(raw(prepatched), Options::new("s"), |args| args.0 += 1).unwrap();
    assert_eq!(call(prepatched, 1, 2), 40, "the subscriber runs, then the earlier patch as the original");
    let report = hooks.diagnostics().into_iter().find(|r| r.address == address).unwrap();
    assert!(!report.validation.accepted() && matches!(report.current, Validation::Managed { .. }) && report.patched_by == Some(exe_name()) && !report.broken, "{report}");

    registry::set_compat(Compat::Refuse);
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&runs);
    hooks.before(raw(overwritten), Options::new("o"), move |_| _ = counted.fetch_add(1, SeqCst)).unwrap();
    assert_eq!(call(overwritten, 1, 2), 3);
    NEXT.store(patch_jump(overwritten as *const () as usize, late as *const () as usize), SeqCst);
    assert_eq!(call(overwritten, 1, 2), 300);
    assert_eq!(runs.load(SeqCst), 2, "the later patch calls on through the dispatcher until the check");
    registry::revalidate("test", &[]);
    assert_eq!(call(overwritten, 1, 2), 300);
    assert_eq!(runs.load(SeqCst), 2, "a broken site's subscribers no longer run");
    let report = hooks.diagnostics().into_iter().find(|r| r.address == overwritten as *const () as usize).unwrap();
    assert!(report.broken && report.patched_by == Some(exe_name()) && !report.current.accepted(), "{report}");
    assert!(report.to_string().contains(&format!("patched by {}, broken; #", exe_name())), "{report}");
}

#[test]
fn owner_names_the_module_or_the_page() {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
    let target = phases as *const () as usize;
    assert_eq!(owner(target), exe_name());
    let page = unsafe { VirtualAlloc(core::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
    assert_ne!(page, 0);
    let mut relay = [0xFF, 0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    relay[6..].copy_from_slice(&target.to_le_bytes());
    unsafe { core::ptr::copy_nonoverlapping(relay.as_ptr(), page as *mut u8, relay.len()) };
    assert_eq!(owner(page), format!("no module, trampoline page {page:#x} jumping to {}", exe_name()));
    unsafe { (page as *mut u8).write(0xC3) };
    assert_eq!(owner(page), format!("no module, trampoline page {page:#x}"));
}

#[test]
fn compat_parses() {
    assert_eq!(" Stack ".parse::<Compat>(), Ok(Compat::Stack));
    assert_eq!("refuse".parse::<Compat>(), Ok(Compat::Refuse));
    assert!("maybe".parse::<Compat>().is_err());
}

#[test]
fn a_disabled_subscriber_keeps_its_place_and_is_skipped() {
    let hooks = &Hooks::HOST;
    let before = hooks.before(raw(toggled), Options::new("t1"), |args| args.0 += 10).unwrap();
    let around = hooks.around(raw(toggled), Options::new("t2"), |args, next| unsafe { next.call((args.0, args.1 * 3)) }).unwrap();
    let replace = hooks.replace(raw(toggled), Options::new("t3"), |args, _| args.0 * 100).unwrap();
    assert_eq!(call(toggled, 1, 1), 1100);
    assert!(hooks.set_enabled(before, false));
    assert!(hooks.set_enabled(replace, false));
    assert_eq!(call(toggled, 1, 1), 4);
    assert!(hooks.set_enabled(around, false));
    assert_eq!(call(toggled, 1, 1), 2);
    let report = hooks.diagnostics().into_iter().find(|s| s.address == toggled as *const () as usize).unwrap();
    let entries: Vec<(&str, bool, u64)> = report.entries.iter().map(|e| (e.owner.as_str(), e.enabled, e.timing.calls)).collect();
    assert_eq!(entries, [("t1", false, 1), ("t2", false, 2), ("t3", false, 1)]);
    assert!(report.to_string().contains("disabled"), "{report}");
    assert!(hooks.set_enabled(before, true));
    assert_eq!(call(toggled, 1, 1), 12);
    assert!(!hooks.set_enabled(SubscriptionId::new(u64::MAX), false));
    let messages = hooks.messages();
    assert!(messages.iter().any(|m| m.ends_with(&format!("#{} t1 Before disabled", before.get()))), "{messages:?}");
    assert!(messages.iter().any(|m| m.ends_with(&format!("#{} t1 Before enabled", before.get()))), "{messages:?}");
}

targets! { called rerouted elsewhere slotted spare_slot }

/// `sub rsp, 0x28; call callee; add rsp, 0x28; ret` in a code page near the test executable, and the address of its call.
fn caller_of(callee: Fn2) -> (Fn2, usize) {
    let code = [0x48, 0x83, 0xEC, 0x28, 0xE8, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x28, 0xC3];
    let stub = hook::Trampoline::shared().alloc(callee as usize, &code).unwrap();
    unsafe { hook::write_call(stub + 4, callee as usize) }.unwrap();
    (unsafe { core::mem::transmute::<usize, Fn2>(stub) }, stub + 4)
}

fn callee_at(site: usize) -> usize {
    let rel = unsafe { ((site + 1) as *const i32).read_unaligned() };
    (site + 5).wrapping_add_signed(rel as isize)
}

fn report_at(address: usize) -> Option<SiteReport> {
    Hooks::HOST.diagnostics().into_iter().find(|s| s.address == address)
}

fn call_at(site: usize) -> ops::Call<Fn2> {
    ops::Call::at(Location::Address(site))
}

#[test]
fn call_sites_share_one_chain_beside_the_callee_entry() {
    let hooks = &Hooks::HOST;
    let (caller, site) = caller_of(called);
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&outcomes);
    hooks.before(raw(called), Options::new("entry"), |args| args.1 += 100).unwrap();
    hooks.before(call_at(site), Options::new("c2").after(&["c1"]), |args| args.0 *= 10).unwrap();
    hooks.before(call_at(site), Options::new("c1"), |args| args.0 += 1).unwrap();
    hooks.after(call_at(site), Options::new("c2"), move |args, ret, outcome| log.lock().unwrap().push((*args, *ret, outcome))).unwrap();
    assert_eq!(call(caller, 1, 2), 20 + 102, "the call site's chain, then the callee's entry chain inside the original");
    assert_eq!(call(called, 1, 2), 103, "a direct call runs the entry chain only");
    assert_eq!(*outcomes.lock().unwrap(), [((20, 2), 122, Outcome::Original)]);

    let report = report_at(site).unwrap();
    assert_eq!(report.name, format!("call {site:#x}/2"));
    assert_eq!(report.validation, Validation::Plausible);
    assert!(matches!(report.current, Validation::Managed { .. }), "{report}");
    let order: Vec<(&str, Phase)> = report.entries.iter().map(|e| (e.owner.as_str(), e.phase)).collect();
    assert_eq!(order, [("c1", Phase::Before), ("c2", Phase::Before), ("c2", Phase::After)]);

    assert_eq!(hooks.remove_owner("c1") + hooks.remove_owner("c2"), 3);
    assert_eq!(callee_at(site), called as *const () as usize, "the call reaches the callee again");
    assert!(report_at(site).is_none());
    assert_eq!(call(caller, 1, 2), 103);

    let id = hooks.replace(call_at(site), Options::new("c3"), |args, _| args.0 + 7).unwrap();
    assert_eq!(call(caller, 1, 2), 8, "the slot bound to the site is installed again");
    assert!(hooks.unsubscribe(id));
    assert_eq!(callee_at(site), called as *const () as usize);
}

#[test]
fn a_call_site_another_writer_changed_is_left_as_it_is() {
    let hooks = &Hooks::HOST;
    let (caller, site) = caller_of(rerouted);
    hooks.before(call_at(site), Options::new("rw"), |args| args.0 += 1).unwrap();
    assert_eq!(call(caller, 1, 1), 3);
    unsafe { hook::write_call(site, elsewhere as *const () as usize) }.unwrap();
    let ours = report_at(site).map(|r| match r.current {
        Validation::Unmanaged { target, .. } => target,
        _ => None,
    });
    assert_eq!(ours, Some(Some(elsewhere as *const () as usize)), "an installed call site reached elsewhere is another writer's");
    assert_eq!(hooks.remove_owner("rw"), 1);
    assert_eq!(callee_at(site), elsewhere as *const () as usize);
    let not_a_call = call_at(caller as usize);
    assert!(matches!(hooks.before(not_a_call, Options::new("rw"), |_| {}), Err(Error::Site { .. })));
}

static TABLE: [Fn2; 2] = [slotted, spare_slot];

#[test]
fn vtable_slots_dispatch_in_order_and_restore() {
    let hooks = &Hooks::HOST;
    assert_eq!(validate_slot(core::ptr::from_ref(&TABLE[1]) as usize, None), Validation::Pristine, "the file's pointer, relocated to the loaded image");
    let table: &'static [usize; 2] = Box::leak(Box::new([slotted as *const () as usize, spare_slot as *const () as usize]));
    let entry = core::ptr::from_ref(&table[1]) as usize;
    let slot = || ops::Vtable::<Fn2>::slot(Location::Address(table.as_ptr() as usize), 1);
    let virtual_call = |a, b| call(unsafe { core::mem::transmute::<usize, Fn2>(core::ptr::read_volatile(entry as *const usize)) }, a, b);
    hooks.before(slot(), Options::new("v1"), |args| args.0 += 5).unwrap();
    hooks.around(slot(), Options::new("v3"), |args, next| unsafe { next.call((args.0 * 2, args.1)) }).unwrap();
    hooks.around(slot(), Options::new("v2").priority(1), |args, next| unsafe { next.call((args.0, args.1 * 3)) }).unwrap();
    assert_eq!(virtual_call(1, 1), 12 + 3, "v1, then v2 (priority 1) around v3");
    assert_eq!(call(spare_slot, 1, 1), 2, "the function itself is untouched");
    let report = report_at(entry).unwrap();
    assert_eq!(report.name, format!("vtable {:#x}[1]/2", table.as_ptr() as usize));
    assert!(matches!(report.current, Validation::Managed { .. }), "{report}");
    assert_eq!(hooks.remove_owner("v1") + hooks.remove_owner("v2") + hooks.remove_owner("v3"), 3);
    assert_eq!(unsafe { core::ptr::read_volatile(entry as *const usize) }, spare_slot as *const () as usize);
    assert_eq!(virtual_call(1, 1), 2);
    let unaligned = ops::Vtable::<Fn2>::slot(Location::Address(table.as_ptr() as usize + 1), 0);
    assert!(matches!(hooks.before(unaligned, Options::new("v1"), |_| {}), Err(Error::Raw { .. })));
}

/// `mov eax, n; ret` in a code page near the test executable.
fn returning(n: u8) -> (unsafe extern "C" fn() -> u32, usize) {
    let stub = hook::Trampoline::shared().alloc(called as *const () as usize, &[0xB8, n, 0, 0, 0, 0xC3]).unwrap();
    (unsafe { core::mem::transmute::<usize, unsafe extern "C" fn() -> u32>(stub) }, stub)
}

fn run(f: unsafe extern "C" fn() -> u32) -> u32 {
    unsafe { black_box(f)() }
}

#[test]
fn patches_are_checked_reserved_toggled_and_restored() {
    let hooks = &Hooks::HOST;
    let (f, stub) = returning(1);
    let at = Location::Address(stub + 1);
    let id = unsafe { hooks.patch("p1", at, &[1, 0], &[2, 0]) }.unwrap();
    assert_eq!(run(f), 2);
    let overlap = unsafe { hooks.patch("p2", Location::Address(stub + 2), &[0, 0], &[1, 1]) }.unwrap_err();
    assert!(matches!(&overlap, Error::Overlap { with, .. } if *with == format!("patch #{} of p1 at {at}", id.get())), "{overlap}");

    let state = |id: PatchId| hooks.patches().into_iter().find(|p| p.id == id).map(|p| p.state);
    assert_eq!(state(id), Some(PatchState::Applied));
    hooks.set_patch_enabled(id, false).unwrap();
    assert_eq!((run(f), state(id)), (1, Some(PatchState::Disabled)));
    hooks.set_patch_enabled(id, true).unwrap();
    assert_eq!(run(f), 2);

    unsafe { hook::write_code(stub + 1, &[3]) }.unwrap();
    assert_eq!(state(id), Some(PatchState::Overwritten { found: vec![3, 0] }));
    assert!(matches!(hooks.set_patch_enabled(id, false), Err(Error::Mismatch { .. })));
    unsafe { hook::write_code(stub + 1, &[2]) }.unwrap();
    assert!(hooks.patches().iter().any(|p| p.id == id && p.to_string() == format!("patch #{} p1 at {at} ({:#x}): 01 00 -> 02 00, applied", id.get(), stub + 1)));

    assert_eq!(hooks.remove_owner("p1"), 0);
    assert_eq!(run(f), 1, "the owner's removal restores its patches");
    assert!(state(id).is_none() && !hooks.unpatch(id));
    assert!(matches!(hooks.set_patch_enabled(id, true), Err(Error::NoPatch(_))));
}

#[test]
fn patches_refuse_other_bytes_lengths_and_owners() {
    let hooks = &Hooks::HOST;
    let (f, stub) = returning(4);
    let at = Location::Address(stub + 1);
    let mismatch = unsafe { hooks.patch("q", at, &[5], &[6]) }.unwrap_err();
    assert_eq!(mismatch, Error::Mismatch { site: format!("patch at {at}"), owner: "q".into(), expected: vec![5], found: vec![4] });
    assert!(hooks.messages().iter().any(|m| m == &format!("refused: {mismatch}")));
    assert!(matches!(unsafe { hooks.patch("q", at, &[], &[]) }, Err(Error::Install { .. })));
    assert!(matches!(unsafe { hooks.patch("q", at, &[4], &[5, 6]) }, Err(Error::Install { .. })));
    assert!(matches!(unsafe { Hooks::for_plugin("q").patch("r", at, &[4], &[5]) }, Err(Error::Owner { .. })));
    let id = unsafe { Hooks::for_plugin("q").patch("q", at, &[4], &[5]) }.unwrap();
    assert_eq!(run(f), 5);
    assert!(hooks.unpatch(id));
    assert_eq!(run(f), 4);
}

#[test]
fn patches_and_hook_sites_hold_their_bytes_against_each_other() {
    targets! { reserved }
    let hooks = &Hooks::HOST;
    let (caller, site) = caller_of(reserved);
    let rel = unsafe { ((site + 1) as *const [u8; 4]).read_unaligned() };
    let id = unsafe { hooks.patch("h1", Location::Address(site + 1), &rel, &rel) }.unwrap();
    let refused = hooks.before(call_at(site), Options::new("h2"), |_| {}).unwrap_err();
    assert!(matches!(&refused, Error::Overlap { with, .. } if with.starts_with(&format!("patch #{} of h1", id.get()))), "{refused}");
    assert!(hooks.unpatch(id));

    hooks.before(call_at(site), Options::new("h2"), |args| args.0 += 1).unwrap();
    let call_bytes = unsafe { ((site + 1) as *const [u8; 4]).read_unaligned() };
    let refused = unsafe { hooks.patch("h1", Location::Address(site + 2), &call_bytes[1..], &call_bytes[1..]) }.unwrap_err();
    assert!(matches!(&refused, Error::Overlap { with, .. } if *with == format!("the hook site call {site:#x}/2")), "{refused}");
    hooks.before(raw(reserved), Options::new("h2"), |_| {}).unwrap();
    assert!(matches!(unsafe { hooks.patch("h1", Location::Address(reserved as *const () as usize + 3), &[0], &[0]) }, Err(Error::Overlap { .. })), "an entry site holds its first 16 bytes");
    assert_eq!(call(caller, 1, 1), 3);
    assert_eq!(hooks.remove_owner("h2"), 2);
}
