use core::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::ops::Raw;
use super::*;

type Fn2 = unsafe extern "C" fn(usize, usize) -> usize;

macro_rules! targets {
    ($($name:ident) => {$(
        #[inline(never)]
        unsafe extern "C" fn $name(a: usize, b: usize) -> usize {
            black_box(a).wrapping_add(black_box(b))
        }
    };
}

targets! { phases exclusive cycle ordering removal reentrant shape }

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
    let hooks = &Hooks::HOST;
    hooks
        .before(raw(removal), Options::new("slow"), |_| {
            std::thread::sleep(Duration::from_millis(40));
            RUNS.fetch_add(1, SeqCst);
        })
        .unwrap();
    let caller = std::thread::spawn(|| {
        for _ in 0..5 {
            call(removal, 1, 1);
        }
    });
    while RUNS.load(SeqCst) == 0 {
        std::thread::yield_now();
    }
    let start = Instant::now();
    assert_eq!(hooks.remove_owner("slow"), 1);
    let runs = RUNS.load(SeqCst);
    assert!(start.elapsed() < Duration::from_millis(200));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(RUNS.load(SeqCst), runs, "a callback ran after remove_owner returned");
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
