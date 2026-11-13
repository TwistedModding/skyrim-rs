// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
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
    ($($name:ident)*) => {$(
        #[inline(never)]
        unsafe extern "C" fn $name(a: usize, b: usize) -> usize {
            black_box(stringify!($name));
            black_box(a).wrapping_add(black_box(b))
        }
    )*};
}

targets! { phases exclusive cycle ordering removal reentrant shape prepatched overwritten toggled mixed }

fn call(f: Fn2, a: usize, b: usize) -> usize {
    unsafe { black_box(f)(a, b) }
}

fn raw(f: Fn2) -> Raw<Fn2> {
    Raw::at(f as usize)
}

#[test]
fn phases_run_in_order_and_see_the_outcome() {
    let hooks = crate::host::hooks();
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
    let hooks = crate::host::hooks();
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
    let hooks = crate::host::hooks();
    hooks.before(raw(cycle), Options::new("p").before(&["q"]), |_| {}).unwrap();
    hooks.before(raw(cycle), Options::new("q").after(&["p"]), |_| {}).unwrap();
    let refused = hooks.before(raw(cycle), Options::new("q").before(&["p"]), |_| {}).unwrap_err();
    assert_eq!(refused, Error::Cycle { site: format!("raw {:#x}/2", cycle as *const () as usize), names: vec!["p".into(), "q".into()] });
    assert_eq!(hooks.diagnostics().into_iter().find(|s| s.address == cycle as *const () as usize).unwrap().entries.len(), 2);
}

#[test]
fn constraints_then_priority_then_rank_then_registration() {
    let hooks = crate::host::hooks();
    let order = Arc::new(Mutex::new(String::new()));
    let tag = |t: char| {
        let order = Arc::clone(&order);
        move |_: &mut (usize, usize)| order.lock().unwrap().push(t)
    };
    Hooks::set_rank("w", -1);
    hooks.before(raw(ordering), Options::new("x"), tag('x')).unwrap();
    hooks.before(raw(ordering), Options::new("y").after(&["x"]), tag('y')).unwrap();
    hooks.before(raw(ordering), Options::new("z").priority(5), tag('z')).unwrap();
    hooks.before(raw(ordering), Options::new("w"), tag('w')).unwrap();
    hooks.before(raw(ordering), Options::new("z").priority(5), tag('Z')).unwrap();
    call(ordering, 0, 0);
    assert_eq!(*order.lock().unwrap(), "zZwxy");
    Hooks::set_rank("w", 1);
    order.lock().unwrap().clear();
    call(ordering, 0, 0);
    assert_eq!(*order.lock().unwrap(), "zZxyw");
}

#[test]
fn removal_waits_for_the_subscriber_in_flight() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);
    let hooks = crate::host::hooks();
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
    let hooks = crate::host::hooks();
    let id = hooks
        .before(raw(reentrant), Options::new("outer"), |_| {
            let nested = crate::host::hooks().before(raw(reentrant), Options::new("inner"), |_| {});
            let _ = INSIDE.set((nested, crate::host::hooks().remove_owner("outer") == 0));
        })
        .unwrap();
    call(reentrant, 1, 1);
    assert_eq!(INSIDE.get(), Some(&(Err(Error::Reentrant), true)));
    assert!(hooks.unsubscribe(id));
    assert!(!hooks.unsubscribe(id));
}

#[test]
fn raw_shape_is_fixed_per_address() {
    let hooks = crate::host::hooks();
    hooks.before(raw(shape), Options::new("s"), |_| {}).unwrap();
    let other = Raw::<unsafe extern "C" fn(usize) -> usize>::at(shape as *const () as usize);
    assert!(matches!(hooks.before(other, Options::new("s"), |_| {}), Err(Error::Raw { .. })));
}

struct Present;

impl ops::OpVisitor<()> for Present {
    fn visit<F: Signature>(self, _: &'static Site, _: F) {}
}

#[test]
fn every_raw_slot_has_a_dispatcher() {
    for shape in 0..ops::SHAPES.len() {
        assert!((0..ops::RAW_SLOTS).all(|slot| ops::with_raw(shape, slot, Present).is_some()));
        assert!(ops::with_raw(shape, ops::RAW_SLOTS, Present).is_none());
    }
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
    static MIXED_NEXT: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn chained(a: usize, b: usize) -> usize {
        unsafe { core::mem::transmute::<usize, Fn2>(MIXED_NEXT.load(SeqCst))(a, b) * 100 }
    }
    unsafe extern "C" fn early(a: usize, b: usize) -> usize {
        black_box(a).wrapping_add(black_box(b)) * 10
    }
    unsafe extern "C" fn late(a: usize, b: usize) -> usize {
        unsafe { core::mem::transmute::<usize, Fn2>(NEXT.load(SeqCst))(a, b) * 100 }
    }
    let hooks = crate::host::hooks();
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
    registry::revalidate("test", &[("prepatched", address)]);
    assert!(matches!(hooks.before(raw(prepatched), Options::new("strict-first").exclusive(), |_| {}), Err(Error::Site { .. })));
    hooks.before(raw(prepatched), Options::new("s"), |args| args.0 += 1).unwrap();
    assert!(matches!(hooks.before(raw(prepatched), Options::new("strict-later").exclusive(), |_| {}), Err(Error::Site { .. })), "an intact dispatcher can still wrap an earlier foreign patch");
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

    registry::set_compat(Compat::default());
    registry::revalidate("test", &[]);
    assert_eq!(call(overwritten, 1, 2), 300);
    assert_eq!(runs.load(SeqCst), 3, "stack mode resumes a reachable chain even when the foreign patch is unchanged");
    assert_eq!(Compat::default(), Compat::Stack);

    let normal = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&normal);
    hooks.before(raw(mixed), Options::new("shared"), move |args| {
        counted.fetch_add(1, SeqCst);
        args.0 += 1;
    }).unwrap();
    let strict = Arc::new([const { AtomicUsize::new(0) }; 4]);
    let counted = Arc::clone(&strict);
    hooks.before(raw(mixed), Options::new("strict-before").exclusive(), move |_| { counted[0].fetch_add(1, SeqCst); }).unwrap();
    let counted = Arc::clone(&strict);
    hooks.around(raw(mixed), Options::new("strict-around").exclusive(), move |args, next| {
        counted[1].fetch_add(1, SeqCst);
        unsafe { next.call(args) }
    }).unwrap();
    let counted = Arc::clone(&strict);
    hooks.replace(raw(mixed), Options::new("strict-replace").exclusive(), move |args, original| {
        counted[2].fetch_add(1, SeqCst);
        unsafe { original.call(args) + 10 }
    }).unwrap();
    let counted = Arc::clone(&strict);
    let after = hooks.after(raw(mixed), Options::new("strict-after").exclusive(), move |_, _, _| { counted[3].fetch_add(1, SeqCst); }).unwrap();
    assert_eq!(call(mixed, 1, 2), 14);
    let mixed_address = mixed as *const () as usize;
    let managed = unsafe { (mixed_address as *const [u8; 5]).read_unaligned() };
    MIXED_NEXT.store(patch_jump(mixed_address, chained as *const () as usize), SeqCst);
    assert_eq!(call(mixed, 1, 2), 1400);
    registry::revalidate("test", &[]);
    assert_eq!(call(mixed, 1, 2), 400);
    assert_eq!(normal.load(SeqCst), 3, "the shared subscriber keeps running");
    assert!(strict.iter().all(|n| n.load(SeqCst) == 2), "all exclusive phases are skipped");
    let report = report_at(mixed_address).unwrap();
    assert!(!report.broken && report.entries.iter().all(|e| e.enabled && e.exclusive == e.blocked));
    assert!(report.to_string().contains("blocked by foreign patch"));
    assert!(matches!(hooks.before(raw(mixed), Options::new("strict-new").exclusive(), |_| {}), Err(Error::Site { .. })));
    assert!(hooks.set_enabled(after, false));
    unsafe { hook::write_code(mixed_address, &managed) }.unwrap();
    registry::revalidate("test", &[]);
    assert_eq!(call(mixed, 1, 2), 14);
    assert_eq!(strict.each_ref().map(|n| n.load(SeqCst)), [3, 3, 3, 2], "repair resumes exclusives without overriding a manual disable");
    assert!(report_at(mixed_address).unwrap().entries.iter().all(|e| !e.blocked));
    for owner in ["shared", "strict-before", "strict-around", "strict-replace", "strict-after"] {
        assert_eq!(hooks.remove_owner(owner), 1);
    }
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
    let hooks = crate::host::hooks();
    let before = hooks.before(raw(toggled), Options::new("t1"), |args| args.0 += 10).unwrap();
    let around = hooks.around(raw(toggled), Options::new("t2"), |args, next| unsafe { next.call((args.0, args.1 * 3)) }).unwrap();
    let replace = hooks.replace(raw(toggled), Options::new("t3"), |args, _| args.0 * 100).unwrap();
    assert_eq!(call(toggled, 1, 1), 1100);
    assert!(hooks.set_enabled(before, false));
    logged(&format!("#{} t1 Before disabled", before.get()));
    assert!(hooks.set_enabled(replace, false));
    assert_eq!(call(toggled, 1, 1), 4);
    assert!(hooks.set_enabled(around, false));
    assert_eq!(call(toggled, 1, 1), 2);
    let report = hooks.diagnostics().into_iter().find(|s| s.address == toggled as *const () as usize).unwrap();
    let entries: Vec<(&str, bool, u64)> = report.entries.iter().map(|e| (e.owner.as_str(), e.enabled, e.timing.calls)).collect();
    assert_eq!(entries, [("t1", false, 1), ("t2", false, 2), ("t3", false, 1)]);
    assert!(report.to_string().contains("disabled"), "{report}");
    assert!(hooks.set_enabled(before, true));
    logged(&format!("#{} t1 Before enabled", before.get()));
    assert_eq!(call(toggled, 1, 1), 12);
    assert!(!hooks.set_enabled(SubscriptionId::new(u64::MAX), false));
}

/// Checks the shared hook log right away: the parallel tests fill its 64 lines quickly.
fn logged(line: &str) {
    let messages = crate::host::hooks().messages();
    assert!(messages.iter().any(|m| m.ends_with(line)), "{line} not in {messages:?}");
}

targets! { called rerouted elsewhere slotted spare_slot tail_called }

#[test]
fn a_tail_jump_is_a_call_site() {
    let hooks = crate::host::hooks();
    let callee = tail_called as *const () as usize;
    let stub = hook::Trampoline::shared().alloc(callee, &[0xE9, 0, 0, 0, 0]).unwrap();
    unsafe { hook::write_branch(stub, callee) }.unwrap();
    let jumper = unsafe { core::mem::transmute::<usize, Fn2>(stub) };
    assert_eq!(call(jumper, 1, 2), 3);
    hooks.before(call_at(stub), Options::new("tail"), |args| args.0 += 10).unwrap();
    hooks.after(call_at(stub), Options::new("tail"), |_, ret, _| assert_eq!(*ret, 13)).unwrap();
    assert_eq!(call(jumper, 1, 2), 13, "the dispatcher returns to the jumper's caller with the callee's result");
    let report = report_at(stub).unwrap();
    assert_eq!(report.name, format!("jmp {stub:#x}/2"));
    assert!(matches!(report.current, Validation::Managed { .. }), "{report}");
    assert_eq!(unsafe { *(stub as *const u8) }, 0xE9, "still a jump");
    assert_eq!(hooks.remove_owner("tail"), 2);
    assert_eq!(callee_at(stub), callee, "the jump reaches the callee again");
    assert_eq!(call(jumper, 1, 2), 3);
}

/// `code` in a code page near the test executable, its `E8` at `call` pointed at `callee`; the code's address.
fn stub_calling(callee: usize, code: &[u8], call: usize) -> usize {
    let stub = hook::Trampoline::shared().alloc(callee, code).unwrap();
    unsafe { hook::write_call(stub + call, callee) }.unwrap();
    stub
}

/// `sub rsp, 0x28; call callee; add rsp, 0x28; ret`; register arguments and the result pass through.
const CALLER: [u8; 14] = [0x48, 0x83, 0xEC, 0x28, 0xE8, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x28, 0xC3];

/// A caller of `callee` and the address of its call.
fn caller_of(callee: Fn2) -> (Fn2, usize) {
    let stub = stub_calling(callee as usize, &CALLER, 4);
    (unsafe { core::mem::transmute::<usize, Fn2>(stub) }, stub + 4)
}

fn callee_at(site: usize) -> usize {
    let rel = unsafe { ((site + 1) as *const i32).read_unaligned() };
    (site + 5).wrapping_add_signed(rel as isize)
}

fn report_at(address: usize) -> Option<SiteReport> {
    crate::host::hooks().diagnostics().into_iter().find(|s| s.address == address)
}

fn call_at(site: usize) -> ops::Call<Fn2> {
    ops::Call::at(Location::Address(site))
}

#[test]
fn call_sites_share_one_chain_beside_the_callee_entry() {
    let hooks = crate::host::hooks();
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
    let hooks = crate::host::hooks();
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

#[inline(never)]
unsafe extern "C" fn first_object(this: usize, _: usize) -> usize {
    black_box(this) + 1
}

#[inline(never)]
unsafe extern "C" fn second_object(this: usize, _: usize) -> usize {
    black_box(this) + 2
}

/// An object whose vtable's slot 1 is `slot`.
fn object(slot: Fn2) -> usize {
    let table: &'static [usize; 2] = Box::leak(Box::new([0, slot as usize]));
    core::ptr::from_ref::<usize>(Box::leak(Box::new(table.as_ptr() as usize))) as usize
}

#[test]
fn a_virtual_call_site_hands_on_the_callers_register() {
    let hooks = crate::host::hooks();
    // push rbx; sub rsp, 0x20; mov rbx, rdx; mov rax, [rcx]; call [rax+8]; add rsp, 0x20; pop rbx; ret
    let code = [0x53, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x89, 0xD3, 0x48, 0x8B, 0x01, 0xFF, 0x90, 8, 0, 0, 0, 0x48, 0x83, 0xC4, 0x20, 0x5B, 0xC3];
    let stub = hook::Trampoline::shared().alloc(called as *const () as usize, &code).unwrap();
    let site = stub + 11;
    let caller = unsafe { core::mem::transmute::<usize, Fn2>(stub) };
    let (a, b) = (object(first_object), object(second_object));
    assert_eq!(call(caller, a, 7), a + 1);
    let at = || ops::Call::<Fn2>::at(Location::Address(site)).passing(Register::Rbx);
    hooks.around(at(), Options::new("vc"), |args, next| unsafe { next.call(args) } + args.1 * 10).unwrap();
    assert_eq!((call(caller, a, 7), call(caller, b, 3)), (a + 1 + 70, b + 2 + 30), "each object reaches its own slot, the caller's rbx comes second");
    let report = report_at(site).unwrap();
    assert_eq!(report.name, format!("call {site:#x}/2 +rbx"));
    assert_eq!(report.validation, Validation::Plausible);
    assert!(matches!(report.current, Validation::Managed { .. }), "{report}");
    let plain = hooks.before(ops::Call::<Fn2>::at(Location::Address(site)), Options::new("vc"), |_| {}).unwrap_err();
    assert!(matches!(&plain, Error::Raw { detail, .. } if detail == "already hooked as 2 +rbx"), "{plain}");
    let wide = ops::Call::<unsafe extern "C" fn(usize, usize, usize, usize, usize) -> usize>::at(Location::Address(stub)).passing(Register::Rsi);
    assert!(matches!(hooks.before(wide, Options::new("vc"), |_| {}), Err(Error::Raw { .. })), "the register must land in an argument register");
    assert!(matches!(unsafe { hooks.patch("vc", Location::Address(site + 5), &[0x90], &[0x90]) }, Err(Error::Overlap { .. })), "the site holds all 6 bytes");
    assert_eq!(hooks.remove_owner("vc"), 1);
    assert_eq!(unsafe { core::slice::from_raw_parts(site as *const u8, 6) }, [0xFF, 0x90, 8, 0, 0, 0], "the virtual call is back");
    assert_eq!(call(caller, b, 3), b + 2);
}

static TABLE: [Fn2; 2] = [slotted, spare_slot];

#[test]
fn vtable_slots_dispatch_in_order_and_restore() {
    let hooks = crate::host::hooks();
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
    let hooks = crate::host::hooks();
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
fn a_cave_patch_jumps_to_its_code_and_back() {
    let hooks = crate::host::hooks();
    // mov eax, 1; add eax, 2; ret
    let stub = hook::Trampoline::shared().alloc(called as *const () as usize, &[0xB8, 1, 0, 0, 0, 0x83, 0xC0, 0x02, 0xC3]).unwrap();
    let f = unsafe { core::mem::transmute::<usize, unsafe extern "C" fn() -> u32>(stub) };
    let at = Location::Address(stub);
    let short = unsafe { hooks.patch_cave("cave", Location::Address(stub + 5), &[0x83, 0xC0, 0x02], &[0x90]) }.unwrap_err();
    assert!(matches!(&short, Error::Install { detail, .. } if detail.contains("needs 5")), "{short}");
    let id = unsafe { hooks.patch_cave("cave", at, &[0xB8, 1, 0, 0, 0], &[0xB8, 10, 0, 0, 0]) }.unwrap();
    assert_eq!(run(f), 12, "the cave's code, then the bytes after the patch");
    let report = hooks.patches().into_iter().find(|p| p.id == id).unwrap();
    assert_eq!((report.replacement[0], report.state.clone()), (0xE9, PatchState::Applied));
    assert!(report.to_string().ends_with(", cave b8 0a 00 00 00, applied"), "{report}");
    assert!(matches!(unsafe { hooks.patch("cave", Location::Address(stub + 4), &[0], &[0]) }, Err(Error::Overlap { .. })));
    hooks.set_patch_enabled(id, false).unwrap();
    assert_eq!(run(f), 3);
    hooks.set_patch_enabled(id, true).unwrap();
    assert_eq!(run(f), 12);
    assert_eq!(hooks.remove_owner("cave"), 0);
    assert_eq!(run(f), 3, "restored with its owner");
}

#[test]
fn patches_refuse_other_bytes_lengths_and_owners() {
    let hooks = crate::host::hooks();
    let (f, stub) = returning(4);
    let at = Location::Address(stub + 1);
    let mismatch = unsafe { hooks.patch("q", at, &[5], &[6]) }.unwrap_err();
    assert_eq!(mismatch, Error::Mismatch { site: format!("patch at {at}"), owner: "q".into(), expected: vec![5], found: vec![4] });
    assert!(hooks.messages().iter().any(|m| m == &format!("refused: {mismatch}")));
    assert!(matches!(unsafe { hooks.patch("q", at, &[], &[]) }, Err(Error::Install { .. })));
    assert!(matches!(unsafe { hooks.patch("q", at, &[4], &[5, 6]) }, Err(Error::Install { .. })));
    let id = unsafe { hooks.patch("q", at, &[4], &[5]) }.unwrap();
    assert_eq!(run(f), 5);
    assert!(hooks.unpatch(id));
    assert_eq!(run(f), 4);
}

#[test]
fn patches_and_hook_sites_hold_their_bytes_against_each_other() {
    targets! { reserved }
    let hooks = crate::host::hooks();
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

type Projection = sre::raw::fns::NiCamera__SetPerspectiveFromFov_fn;
type DefaultFov = sre::raw::fns::PlayerCamera__GetDefaultFOVForCurrentState_fn;
type Projected = (f32, f32, f32, u32, u32, bool, bool, f32);

static PROJECTED: Mutex<Option<Projected>> = Mutex::new(None);

#[inline(never)]
unsafe extern "C" fn projection(_: *mut sre::NiCamera, fov: f32, near: f32, far: f32, width: u32, height: u32, spans: bool, lod: bool, reference: f32) {
    *PROJECTED.lock().unwrap() = Some(black_box((fov, near, far, width, height, spans, lod, reference)));
}

#[inline(never)]
unsafe extern "C" fn default_fov(_: *mut sre::PlayerCamera) -> f32 {
    black_box(80.0)
}

#[test]
fn typed_call_sites_carry_float_arguments_and_results() {
    let hooks = crate::host::hooks();
    // sub rsp, 0x58; the caller's five stack arguments copied below the home space; call; add rsp, 0x58; ret
    let mut code = vec![0x48, 0x83, 0xEC, 0x58];
    for i in 0..5u8 {
        code.extend([0x48, 0x8B, 0x84, 0x24, 0x80 + 8 * i, 0, 0, 0, 0x48, 0x89, 0x44, 0x24, 0x20 + 8 * i]);
    }
    let call = code.len();
    code.extend([0xE8, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x58, 0xC3]);
    let stub = stub_calling(projection as *const () as usize, &code, call);
    let caller = unsafe { core::mem::transmute::<usize, Projection>(stub) };
    let site = ops::Call::<Projection>::at(Location::Address(stub + call));
    hooks.before(site, Options::new("hands"), |args| args.1 = 55.0).unwrap();
    unsafe { black_box(caller)(core::ptr::null_mut(), 80.0, 5.0, 10100.0, 1920, 1080, false, true, 75.0) };
    assert_eq!(*PROJECTED.lock().unwrap(), Some((55.0, 5.0, 10100.0, 1920, 1080, false, true, 75.0)), "the edited FOV, every other register and stack argument intact");
    assert_eq!(report_at(stub + call).unwrap().name, format!("call {:#x}/NiCamera::SetPerspectiveFromFov", stub + call));
    assert_eq!(hooks.remove_owner("hands"), 1);

    let stub = stub_calling(default_fov as *const () as usize, &CALLER, 4);
    let caller = unsafe { core::mem::transmute::<usize, DefaultFov>(stub) };
    let site = || ops::Call::<DefaultFov>::at(Location::Address(stub + 4));
    let seen = Arc::new(Mutex::new(None));
    let log = Arc::clone(&seen);
    hooks.after(site(), Options::new("tween"), move |_, ret, _| *log.lock().unwrap() = Some(ret.to_bits())).unwrap();
    assert_eq!(unsafe { black_box(caller)(core::ptr::null_mut()) }.to_bits(), 80f32.to_bits());
    assert_eq!(*seen.lock().unwrap(), Some(80f32.to_bits()), "the original's float result");
    hooks.replace(site(), Options::new("tween"), |_, _| 65.5).unwrap();
    assert_eq!(unsafe { black_box(caller)(core::ptr::null_mut()) }.to_bits(), 65.5f32.to_bits(), "the replacement's float result reaches the caller");
    let raw = ops::Call::<unsafe extern "C" fn(usize) -> usize>::at(Location::Address(stub + 4));
    assert!(matches!(hooks.before(raw, Options::new("tween"), |_| {}), Err(Error::Raw { .. })), "one shape per site");
    assert_eq!(hooks.remove_owner("tween"), 2);
}

targets! { kinds shaped nested }

/// Counts its drops, to see a subscriber's closure freed exactly once.
struct Counted(&'static AtomicUsize);

impl Counted {
    fn alive(&self) -> bool {
        self.0.load(SeqCst) == 0
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_add(1, SeqCst);
    }
}

#[test]
fn each_kind_of_subscriber_crosses_runs_and_drops_once() {
    static DROPS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
    let hooks = crate::host::hooks();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let [b, a, r, f] = [0, 1, 2, 3].map(|i| Counted(&DROPS[i]));
    hooks.before(raw(kinds), Options::new("k"), move |args| args.0 += usize::from(b.alive())).unwrap();
    hooks.around(raw(kinds), Options::new("k"), move |args, next| unsafe { next.call(args) } * if a.alive() { 2 } else { 0 }).unwrap();
    hooks.replace(raw(kinds), Options::new("k"), move |args, original| unsafe { original.call(args) } + if r.alive() { 100 } else { 0 }).unwrap();
    hooks.after(raw(kinds), Options::new("k"), move |args, ret, outcome| log.lock().unwrap().push((*args, *ret, outcome, f.alive()))).unwrap();
    assert_eq!(call(kinds, 1, 2), 208);
    assert_eq!(*seen.lock().unwrap(), [((2, 2), 208, Outcome::Replaced, true)]);
    assert_eq!(DROPS.each_ref().map(|d| d.load(SeqCst)), [0; 4], "a call keeps the closures");
    assert_eq!(hooks.remove_owner("k"), 4);
    assert_eq!(DROPS.each_ref().map(|d| d.load(SeqCst)), [1; 4]);
    assert_eq!(call(kinds, 1, 2), 3);
}

/// A raw entry of shape 2 subscribed with the function type `F`, as a plugin built against
/// another `sre` might type it.
struct Shaped<F>(usize, core::marker::PhantomData<F>);

impl<F: Signature> Hookable for Shaped<F> {
    type Fn = F;

    fn target(self) -> Target {
        Target::Raw { address: self.0, shape: 2 }
    }
}

/// An operation of a later framework, past the host's `ops!` list.
struct Later;

impl Hookable for Later {
    type Fn = Fn2;

    fn target(self) -> Target {
        Target::Op(ops::OPS.len() + 7)
    }
}

#[test]
fn a_patch_takes_its_bytes_from_the_sites_row() {
    let site = |expected: &[u8], replacement: Option<&[u8]>| ProfileSite { offset: 0x20, expected: expected.to_vec(), replacement: replacement.map(<[u8]>::to_vec) };
    let row = |r: Row<&ProfileSite>| patch_row(r, vec![1, 2], vec![3, 4]);
    assert_eq!(row(Row::Same).unwrap(), (vec![1, 2], vec![3, 4]), "no row: the canonical bytes");
    assert_eq!(row(Row::Here(&site(&[9, 9, 9], Some(&[8, 8, 8])))).unwrap(), (vec![9, 9, 9], vec![8, 8, 8]), "the row's bytes");
    assert_eq!(row(Row::Here(&site(&[1, 2], None))).unwrap(), (vec![1, 2], vec![3, 4]), "moved, with the same code");
    assert!(row(Row::Missing).unwrap_err().contains("does not exist"));
    assert!(row(Row::Unverified).unwrap_err().contains("unverified"));
    assert!(row(Row::Here(&site(&[9, 9], None))).unwrap_err().contains("no replacement"), "canonical bytes never go over other code");
}

#[test]
fn a_vtable_slot_takes_its_index_from_the_row() {
    assert_eq!(slot_row(Row::Same, 113), Ok(113));
    assert_eq!(slot_row(Row::Here(120), 113), Ok(120));
    assert!(slot_row(Row::Missing, 113).unwrap_err().contains("does not exist"));
    assert!(slot_row(Row::Unverified, 113).unwrap_err().contains("unverified"));
}

#[test]
fn a_subscription_is_checked_by_its_slot_shape() {
    let hooks = crate::host::hooks();
    let address = shaped as *const () as usize;
    let float_first = Shaped::<unsafe extern "C" fn(f32, usize) -> usize>(address, core::marker::PhantomData);
    let refused = hooks.before(float_first, Options::new("sh"), |_| {}).unwrap_err();
    assert_eq!(refused, Error::Callback { site: format!("raw {address:#x}/2"), owner: "sh".into() });
    assert!(report_at(address).is_none(), "refused before the detour went in");
    let same_shape = Shaped::<unsafe extern "C" fn(*mut u8, u64) -> isize>(address, core::marker::PhantomData);
    hooks.before(same_shape, Options::new("sh"), |args| args.1 += 10).unwrap();
    assert_eq!(call(shaped, 1, 2), 13, "other types of the same classes are the site's shape");
    assert_eq!(hooks.remove_owner("sh"), 1);
    let unknown = hooks.after(Later, Options::new("sh"), |_, _, _| {}).unwrap_err();
    assert_eq!(unknown, Error::Unsupported { what: format!("hook operation {}", ops::OPS.len() + 7) });
}

#[test]
fn next_runs_the_rest_of_the_chain_in_the_host() {
    fn inside() -> Option<u64> {
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        site::thread_state(thread).and_then(|(_, entry)| entry).map(|m| m.id)
    }
    let hooks = crate::host::hooks();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (outer_log, inner_log) = (Arc::clone(&seen), Arc::clone(&seen));
    let outer = hooks
        .around(raw(nested), Options::new("n1").priority(1), move |args, next| {
            let entered = inside();
            let r = unsafe { next.call((args.0 + 1, args.1)) };
            outer_log.lock().unwrap().push(("outer", entered, inside()));
            r * 10
        })
        .unwrap();
    let inner = hooks
        .around(raw(nested), Options::new("n2"), move |args, next| {
            inner_log.lock().unwrap().push(("inner", inside(), None));
            unsafe { next.call(args) + 1 }
        })
        .unwrap();
    assert_eq!(call(nested, 1, 2), 50, "the outer's edit, the inner, the original, back out");
    let (o, i) = (Some(outer.get()), Some(inner.get()));
    assert_eq!(*seen.lock().unwrap(), [("inner", i, None), ("outer", o, o)], "the crash logger sees the inner around inside the outer's Next::call");
    let report = report_at(nested as *const () as usize).unwrap();
    assert_eq!(report.entries.iter().map(|e| (e.owner.as_str(), e.timing.calls)).collect::<Vec<_>>(), [("n1", 1), ("n2", 1)]);
    assert_eq!(report.last_outcome, Some(Outcome::Original));
    assert_eq!(hooks.remove_owner("n1") + hooks.remove_owner("n2"), 2);
}
