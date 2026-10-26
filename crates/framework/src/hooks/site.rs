//! The per-function state the detour reads without a lock, and the typed chain of subscribers it runs.

use core::cell::Cell;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU64, AtomicUsize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::validate::Validation;
use super::{Error, Phase};
use crate::order::{self, Constraints};

/// Plugin name to load position; the tie-breaker after priority.
pub(crate) type Ranks = BTreeMap<String, i32>;

/// An `unsafe extern "C" fn` type as the dispatcher sees it: the arguments as a tuple and the result.
pub trait Signature: Copy + Send + Sync + 'static {
    type Args: Copy + 'static;
    type Ret: Copy + 'static;

    /// # Safety
    /// `self` must be callable with `args`.
    unsafe fn call(self, args: Self::Args) -> Self::Ret;

    /// The detour that routes a call of this signature to `O`'s site.
    fn detour<O: Op<Fn = Self>>() -> Self;
}

pub type Args<F> = <F as Signature>::Args;
pub type Ret<F> = <F as Signature>::Ret;

/// A hooked function bound to its process-wide site.
pub trait Op: 'static {
    type Fn: Signature;
    fn site() -> &'static Site;
}

macro_rules! signatures {
    ($(($($a:ident $A:ident),*)) => {$(
        impl<$($A: Copy + 'static, R: Copy + 'static> Signature for unsafe extern "C" fn($($A),*) -> R {
            type Args = ($($A,);
            type Ret = R;

            unsafe fn call(self, ($($a,): Self::Args) -> R {
                unsafe { self($($a),*) }
            }

            fn detour<O: Op<Fn = Self>>() -> Self {
                unsafe extern "C" fn detour<O: Op<Fn = unsafe extern "C" fn($($A),*) -> R>, $($A: Copy + 'static, R: Copy + 'static>($($a: $A),*) -> R {
                    dispatch::<O>(($($a,))
                }
                detour::<O, $($A, R>
            }
        }
    };
}

signatures! {
    ()
    (a A)
    (a A, b B)
    (a A, b B, c C)
    (a A, b B, c C, d D)
    (a A, b B, c C, d D, e E)
    (a A, b B, c C, d D, e E, f F)
    (a A, b B, c C, d D, e E, f F, g G)
    (a A, b B, c C, d D, e E, f F, g G, h H)
}

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Whether this thread is inside a subscriber; a registration there would wait for itself.
pub(crate) fn in_callback() -> bool {
    DEPTH.with(|d| d.get() > 0)
}

fn dispatch<O: Op>(args: Args<O::Fn>) -> Ret<O::Fn> {
    let site = O::site();
    let parity = site.enter();
    DEPTH.with(|d| d.set(d.get() + 1));
    let chain = unsafe { &*site.chain.load(SeqCst).cast::<Chain<O::Fn>>() };
    let ret = chain.run(site, args);
    DEPTH.with(|d| d.set(d.get() - 1));
    site.active[parity].fetch_sub(1, SeqCst);
    ret
}

/// How a call through the site ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Outcome {
    /// The engine function ran.
    Original = 1,
    /// The site's `Replace` subscriber ran instead of the engine function.
    Replaced = 2,
    /// An `Around` returned without calling on.
    Skipped = 3,
}

impl Outcome {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Original),
            2 => Some(Self::Replaced),
            3 => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// Call count and timing of a site or a subscriber.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub calls: u64,
    pub last_ns: u64,
    pub mean_ns: u64,
}

pub(crate) struct Stats {
    calls: AtomicU64,
    total_ns: AtomicU64,
    last_ns: AtomicU64,
}

impl Stats {
    pub(crate) const fn new() -> Self {
        Self { calls: AtomicU64::new(0), total_ns: AtomicU64::new(0), last_ns: AtomicU64::new(0) }
    }

    fn timed<R>(&self, f: impl FnOnce() -> R) -> R {
        let start = Instant::now();
        let r = f();
        self.record(start.elapsed());
        r
    }

    fn record(&self, elapsed: Duration) {
        let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Relaxed);
        self.total_ns.fetch_add(ns, Relaxed);
        self.last_ns.store(ns, Relaxed);
    }

    pub(crate) fn snapshot(&self) -> Timing {
        let calls = self.calls.load(Relaxed);
        Timing { calls, last_ns: self.last_ns.load(Relaxed), mean_ns: self.total_ns.load(Relaxed) / calls.max(1) }
    }
}

/// What a subscription declared and what it has done since; shared by the chain and the reports.
pub(crate) struct Meta {
    pub id: u64,
    pub owner: String,
    pub phase: Phase,
    pub constraints: Constraints,
    pub stats: Stats,
}

pub type BeforeFn<F> = dyn Fn(&mut Args<F>) + Send + Sync + 'static;
pub type AroundFn<F> = dyn for<'a> Fn(Args<F>, Next<'a, F>) -> Ret<F> + Send + Sync + 'static;
pub type ReplaceFn<F> = dyn Fn(Args<F>, Original<F>) -> Ret<F> + Send + Sync + 'static;
pub type AfterFn<F> = dyn Fn(&Args<F>, &Ret<F>, Outcome) + Send + Sync + 'static;

pub(crate) enum Callback<F: Signature> {
    Before(Box<BeforeFn<F>>),
    Around(Box<AroundFn<F>>),
    Replace(Box<ReplaceFn<F>>),
    After(Box<AfterFn<F>>),
}

pub(crate) struct Entry<F: Signature> {
    pub meta: Arc<Meta>,
    pub callback: Callback<F>,
}

/// The engine function, handed to a `Replace` subscriber.
pub struct Original<F: Signature>(F);

impl<F: Signature> Original<F> {
    /// # Safety
    /// `args` must be valid for the engine function.
    pub unsafe fn call(&self, args: Args<F>) -> Ret<F> {
        unsafe { self.0.call(args) }
    }
}

struct State<F: Signature> {
    received: Args<F>,
    outcome: Outcome,
}

/// The rest of the `Around` chain, ending at the engine function or the `Replace`; calling it is optional.
pub struct Next<'a, F: Signature> {
    chain: &'a Chain<F>,
    state: &'a mut State<F>,
    index: usize,
}

impl<F: Signature> Next<'_, F> {
    /// Runs the next `Around`, or the innermost link, with `args`.
    ///
    /// # Safety
    /// `args` must be valid for the engine function.
    pub unsafe fn call(self, args: Args<F>) -> Ret<F> {
        if let Some(entry) = self.chain.arounds.get(self.index) {
            let Callback::Around(f) = &entry.callback else { unreachable!() };
            let next = Next { chain: self.chain, state: self.state, index: self.index + 1 };
            return entry.meta.stats.timed(|| f(args, next));
        }
        self.state.received = args;
        if let Some(entry) = &self.chain.replace {
            let Callback::Replace(f) = &entry.callback else { unreachable!() };
            self.state.outcome = Outcome::Replaced;
            return entry.meta.stats.timed(|| f(args, Original(self.chain.original)));
        }
        self.state.outcome = Outcome::Original;
        unsafe { self.chain.original.call(args) }
    }
}

/// An immutable snapshot of a site's subscribers in execution order; replaced whole on every change.
pub(crate) struct Chain<F: Signature> {
    pub original: F,
    /// Every entry in registration order, the input of the next rebuild.
    pub entries: Vec<Arc<Entry<F>>>,
    befores: Vec<Arc<Entry<F>>>,
    arounds: Vec<Arc<Entry<F>>>,
    replace: Option<Arc<Entry<F>>>,
    afters: Vec<Arc<Entry<F>>>,
}

impl<F: Signature> Chain<F> {
    /// Orders `entries` per phase with the plugin loader's sort (constraints, then priority, then
    /// position, which here is the plugin's load rank and then registration order); refuses a
    /// second `Replace` and any constraint cycle.
    pub(crate) fn build(original: F, entries: Vec<Arc<Entry<F>>>, ranks: &Ranks, site: &str) -> Result<Box<Self>, Error> {
        let replaces: Vec<_> = entries.iter().filter(|e| e.meta.phase == Phase::Replace).collect();
        if let [first, .., last] = replaces[..] {
            return Err(Error::Exclusive { site: site.to_owned(), existing: first.meta.owner.clone(), refused: last.meta.owner.clone() });
        }
        let sorted = |phase: Phase| -> Result<Vec<Arc<Entry<F>>>, Error> {
            let mut of_phase: Vec<&Arc<Entry<F>>> = entries.iter().filter(|e| e.meta.phase == phase).collect();
            of_phase.sort_by_key(|e| (ranks.get(&e.meta.owner).copied().unwrap_or(0), e.meta.id));
            let items: Vec<(&str, &Constraints)> = of_phase.iter().map(|e| (e.meta.owner.as_str(), &e.meta.constraints)).collect();
            let order = order::sort(&items);
            if !order.cycle.is_empty() {
                let mut names: Vec<String> = order.cycle.iter().map(|&i| items[i].0.to_owned()).collect();
                names.sort();
                names.dedup();
                return Err(Error::Cycle { site: site.to_owned(), names });
            }
            Ok(order.sorted.iter().map(|&i| Arc::clone(of_phase[i])).collect())
        };
        Ok(Box::new(Self {
            original,
            befores: sorted(Phase::Before)?,
            arounds: sorted(Phase::Around)?,
            replace: replaces.first().map(|e| Arc::clone(e)),
            afters: sorted(Phase::After)?,
            entries,
        }))
    }

    /// The subscribers as they run: befores, arounds, the replace, afters.
    pub(crate) fn order(&self) -> Vec<Arc<Meta>> {
        self.befores.iter().chain(&self.arounds).chain(&self.replace).chain(&self.afters).map(|e| Arc::clone(&e.meta)).collect()
    }

    fn run(&self, site: &Site, mut args: Args<F>) -> Ret<F> {
        let start = Instant::now();
        for entry in &self.befores {
            let Callback::Before(f) = &entry.callback else { unreachable!() };
            entry.meta.stats.timed(|| f(&mut args));
        }
        let mut state = State { received: args, outcome: Outcome::Skipped };
        let ret = unsafe { Next { chain: self, state: &mut state, index: 0 }.call(args) };
        for entry in &self.afters {
            let Callback::After(f) = &entry.callback else { unreachable!() };
            entry.meta.stats.timed(|| f(&state.received, &ret, state.outcome));
        }
        site.stats.record(start.elapsed());
        site.last_outcome.store(state.outcome as u8, Relaxed);
        ret
    }
}

/// Republishes a site's chain keeping the entries a predicate accepts; `rebuild::<F>` for the site's signature.
pub(crate) type Rebuild = unsafe fn(&Site, &mut SiteMeta, &Ranks, &dyn Fn(&Meta) -> bool) -> usize;

/// What the registry knows about an installed site.
pub(crate) struct SiteMeta {
    pub name: String,
    pub address: usize,
    pub detour: usize,
    pub validation: Validation,
    /// The subscribers in execution order, for the reports.
    pub order: Vec<Arc<Meta>>,
    pub rebuild: Rebuild,
}

/// One engine function's hook state. The detour reads `chain` under the epoch counters; the
/// registry, holding its lock, swaps a new chain in and waits for every call that could see the
/// old one to leave before freeing it.
pub struct Site {
    chain: AtomicPtr<()>,
    epoch: AtomicUsize,
    active: [AtomicUsize; 2],
    stats: Stats,
    last_outcome: AtomicU8,
    pub(crate) meta: Mutex<Option<SiteMeta>>,
}

impl Site {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chain: AtomicPtr::new(core::ptr::null_mut()),
            epoch: AtomicUsize::new(0),
            active: [AtomicUsize::new(0), AtomicUsize::new(0)],
            stats: Stats::new(),
            last_outcome: AtomicU8::new(0),
            meta: Mutex::new(None),
        }
    }

    /// Counts this call on the parity of the current epoch, retrying when a publish slips between
    /// reading the epoch and counting, so a publish always sees every call it has to wait for.
    fn enter(&self) -> usize {
        loop {
            let epoch = self.epoch.load(SeqCst);
            self.active[epoch & 1].fetch_add(1, SeqCst);
            if self.epoch.load(SeqCst) == epoch {
                return epoch & 1;
            }
            self.active[epoch & 1].fetch_sub(1, SeqCst);
        }
    }

    /// Swaps `chain` in and returns once no call can still be inside the previous one.
    ///
    /// # Safety
    /// `F` must be the signature the site was installed with, and the caller must hold the registry lock.
    pub(crate) unsafe fn publish<F: Signature>(&self, chain: Box<Chain<F>>) {
        let old = self.chain.swap(Box::into_raw(chain).cast(), SeqCst);
        let parity = self.epoch.fetch_add(1, SeqCst) & 1;
        let mut spins = 0u32;
        while self.active[parity].load(SeqCst) != 0 {
            spins += 1;
            if spins > 1000 {
                std::thread::sleep(Duration::from_millis(1));
            } else {
                std::thread::yield_now();
            }
        }
        if !old.is_null() {
            drop(unsafe { Box::from_raw(old.cast::<Chain<F>>()) });
        }
    }

    /// The published chain.
    ///
    /// # Safety
    /// Same as [`Site::publish`].
    pub(crate) unsafe fn chain<F: Signature>(&self) -> Option<&Chain<F>> {
        unsafe { self.chain.load(SeqCst).cast::<Chain<F>>().as_ref() }
    }

    pub(crate) fn snapshot(&self) -> (Timing, Option<Outcome>) {
        (self.stats.snapshot(), Outcome::from_u8(self.last_outcome.load(Relaxed)))
    }
}

impl Default for Site {
    fn default() -> Self {
        Self::new()
    }
}

/// Publishes the site's entries that `keep` accepts, reordered, and returns how many were dropped.
///
/// # Safety
/// Same as [`Site::publish`].
pub(crate) unsafe fn rebuild<F: Signature>(site: &Site, meta: &mut SiteMeta, ranks: &Ranks, keep: &dyn Fn(&Meta) -> bool) -> usize {
    let (new, removed) = {
        let chain = unsafe { site.chain::<F>() }.expect("published at install");
        let entries: Vec<_> = chain.entries.iter().filter(|e| keep(&e.meta)).cloned().collect();
        let removed = chain.entries.len() - entries.len();
        (Chain::build(chain.original, entries, ranks, &meta.name).expect("a subset of an ordered chain is ordered"), removed)
    };
    meta.order = new.order();
    unsafe { site.publish(new) };
    removed
}
