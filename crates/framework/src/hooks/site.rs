//! The per-function state the detour reads without a lock, and the chain of subscribers it runs
//! over the call's argument slots.

use core::cell::Cell;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use super::raw::{AfterCallback, ArgSlots, AroundCallback, BeforeCallback, NextApi, RawSignature, ReplaceCallback, RetSlot, Slot};
use super::validate::{self, Validation};
use super::{Error, Phase};
use crate::order::{self, Constraints};

/// Plugin name to load position; the tie-breaker after priority.
pub(crate) type Ranks = BTreeMap<String, i32>;

/// An `unsafe extern "C" fn` type as the dispatcher sees it: the arguments as a tuple, the result,
/// and both as the slots they cross in.
pub trait Signature: Copy + Send + Sync + 'static {
    type Args: Copy + 'static;
    type Ret: Slot;

    /// The slot shape the host checks a subscription by.
    const SLOTS: RawSignature;

    /// # Safety
    /// `self` must be callable with `args`.
    unsafe fn call(self, args: Self::Args) -> Self::Ret;

    /// Each argument in its own slot, in order.
    fn to_slots(args: Self::Args) -> ArgSlots;

    /// The arguments back from their slots.
    fn from_slots(slots: &ArgSlots) -> Self::Args;

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
    ($(($($a:ident $A:ident $i:tt),*)) => {$(
        impl<$($A: Slot, R: Slot> Signature for unsafe extern "C" fn($($A),*) -> R {
            type Args = ($($A,);
            type Ret = R;

            const SLOTS: RawSignature = RawSignature::new(&[$($A::CLASS),*], R::CLASS);

            unsafe fn call(self, ($($a,): Self::Args) -> R {
                unsafe { self($($a),*) }
            }

            fn to_slots(($($a,): Self::Args) -> ArgSlots {
                ArgSlots::new(&[$($a.to_slot()),*])
            }

            #[allow(unused_variables, clippy::unused_unit)]
            fn from_slots(slots: &ArgSlots) -> Self::Args {
                ($($A::from_slot(slots.slots[$i]),)
            }

            fn detour<O: Op<Fn = Self>>() -> Self {
                unsafe extern "C" fn detour<O: Op, $($A: Slot, R: Slot>($($a: $A),*) -> R {
                    R::from_slot(dispatch(O::site(), ArgSlots::new(&[$($a.to_slot()),*])).bits)
                }
                detour::<O, $($A, R>
            }
        }
    };
}

signatures! {
    ()
    (a A 0)
    (a A 0, b B 1)
    (a A 0, b B 1, c C 2)
    (a A 0, b B 1, c C 2, d D 3)
    (a A 0, b B 1, c C 2, d D 3, e E 4)
    (a A 0, b B 1, c C 2, d D 3, e E 4, f F 5)
    (a A 0, b B 1, c C 2, d D 3, e E 4, f F 5, g G 6)
    (a A 0, b B 1, c C 2, d D 3, e E 4, f F 5, g G 6, h H 7)
    (a A 0, b B 1, c C 2, d D 3, e E 4, f F 5, g G 6, h H 7, i I 8)
}

/// Calls the function at `address`, of the type `F`, with the arguments in `args`.
unsafe fn call_slots<F: Signature>(address: usize, args: &ArgSlots) -> RetSlot {
    let f = unsafe { core::mem::transmute_copy::<usize, F>(&address) };
    RetSlot { bits: unsafe { f.call(F::from_slots(args)) }.to_slot() }
}

/// What the chain's innermost link calls: the engine function (or what stands in for it) and how.
#[derive(Clone, Copy)]
pub(crate) struct Callee {
    pub address: usize,
    call: unsafe fn(usize, &ArgSlots) -> RetSlot,
}

impl Callee {
    /// `address` called as `F`.
    pub(crate) fn new<F: Signature>(address: usize) -> Self {
        Self { address, call: call_slots::<F> }
    }

    fn call(self, args: &ArgSlots) -> RetSlot {
        unsafe { (self.call)(self.address, args) }
    }
}

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static SLOT: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// Whether this thread is inside a subscriber; a registration there would wait for itself.
pub(crate) fn in_callback() -> bool {
    DEPTH.with(|d| d.get() > 0)
}

/// Which site and subscriber a thread is inside, readable from another thread by the crash
/// logger: one slot per thread, claimed on its first dispatch and kept.
struct ThreadSlot {
    thread: AtomicU32,
    site_name: AtomicPtr<u8>,
    site_len: AtomicUsize,
    entry: AtomicPtr<Meta>,
}

const THREAD_SLOTS: usize = 64;

static THREADS: [ThreadSlot; THREAD_SLOTS] = [const { ThreadSlot { thread: AtomicU32::new(0), site_name: AtomicPtr::new(core::ptr::null_mut()), site_len: AtomicUsize::new(0), entry: AtomicPtr::new(core::ptr::null_mut()) } }; THREAD_SLOTS];

fn slot() -> Option<&'static ThreadSlot> {
    SLOT.with(|s| {
        if s.get() != usize::MAX {
            return Some(&THREADS[s.get()]);
        }
        let me = unsafe { GetCurrentThreadId() };
        let (i, slot) = THREADS.iter().enumerate().find(|(_, slot)| slot.thread.compare_exchange(0, me, Relaxed, Relaxed).is_ok())?;
        s.set(i);
        Some(slot)
    })
}

/// The site name and the subscriber `thread` is inside right now, for the crash log of a frozen thread.
pub(crate) fn thread_state(thread: u32) -> Option<(Option<&'static str>, Option<&'static Meta>)> {
    let slot = THREADS.iter().find(|s| s.thread.load(Relaxed) == thread)?;
    let name = slot.site_name.load(Relaxed);
    let site = (!name.is_null()).then(|| unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(name, slot.site_len.load(Relaxed))) });
    let entry = unsafe { slot.entry.load(Relaxed).as_ref() };
    Some((site, entry))
}

/// Runs one subscriber with its `Meta` marked as the thread's current entry, timed.
fn track<R>(meta: &Arc<Meta>, f: impl FnOnce() -> R) -> R {
    let slot = slot();
    let previous = slot.map(|s| s.entry.swap(Arc::as_ptr(meta).cast_mut(), Relaxed));
    let r = meta.stats.timed(f);
    if let (Some(s), Some(p)) = (slot, previous) {
        s.entry.store(p, Relaxed);
    }
    r
}

fn dispatch(site: &Site, mut args: ArgSlots) -> RetSlot {
    let parity = site.enter();
    DEPTH.with(|d| d.set(d.get() + 1));
    let chain = unsafe { &*site.chain.load(SeqCst) };
    let slot = slot();
    let previous = slot.map(|s| (s.site_name.swap(chain.name.as_ptr().cast_mut(), Relaxed), s.site_len.swap(chain.name.len(), Relaxed)));
    let ret = if site.broken.load(Relaxed) { chain.callee.call(&args) } else { chain.run(site, &mut args) };
    if let (Some(s), Some((name, len))) = (slot, previous) {
        s.site_name.store(name, Relaxed);
        s.site_len.store(len, Relaxed);
    }
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
    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Original),
            2 => Some(Self::Replaced),
            3 => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// Call count and timing of a site or a subscriber; frozen, as it crosses the plugin ABI in the reports.
#[repr(C)]
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
    /// A disabled subscriber keeps its place in the order and is skipped by the dispatch.
    pub enabled: AtomicBool,
}

impl Meta {
    fn runs(&self) -> bool {
        self.enabled.load(Relaxed)
    }
}

/// A subscriber as the plugin handed it over.
pub(crate) enum Subscriber {
    Before(BeforeCallback),
    Around(AroundCallback),
    Replace(ReplaceCallback),
    After(AfterCallback),
}

impl Subscriber {
    pub(crate) fn phase(&self) -> Phase {
        match self {
            Self::Before(_) => Phase::Before,
            Self::Around(_) => Phase::Around,
            Self::Replace(_) => Phase::Replace,
            Self::After(_) => Phase::After,
        }
    }
}

pub(crate) struct Entry {
    pub meta: Arc<Meta>,
    pub subscriber: Subscriber,
}

struct State {
    received: ArgSlots,
    outcome: Outcome,
}

/// The rest of the `Around` chain from `index`, which a plugin's `Next` runs through [`NextApi`].
struct Link<'a> {
    chain: &'a Chain,
    state: &'a mut State,
    index: usize,
}

impl Link<'_> {
    /// Runs the next `Around`, or the innermost link, with `args`.
    fn run(self, args: ArgSlots) -> RetSlot {
        if let Some(entry) = self.chain.arounds.get(self.index) {
            let next = Link { chain: self.chain, state: self.state, index: self.index + 1 };
            if !entry.meta.runs() {
                return next.run(args);
            }
            let Subscriber::Around(f) = &entry.subscriber else { unreachable!() };
            let mut link = Some(next);
            let api = NextApi { call: next_call, state: (&raw mut link).cast() };
            let mut ret = RetSlot::default();
            track(&entry.meta, || (f.call)(f.ctx, &raw const args, &raw const api, &raw mut ret));
            return ret;
        }
        self.state.received = args;
        if let Some(entry) = self.chain.replace.as_ref().filter(|e| e.meta.runs()) {
            let Subscriber::Replace(f) = &entry.subscriber else { unreachable!() };
            self.state.outcome = Outcome::Replaced;
            let mut ret = RetSlot::default();
            track(&entry.meta, || (f.call)(f.ctx, &raw const args, self.chain.callee.address, &raw mut ret));
            return ret;
        }
        self.state.outcome = Outcome::Original;
        self.chain.callee.call(&args)
    }
}

/// [`NextApi::call`]: runs the [`Link`] its state holds, the first time only.
extern "C" fn next_call(api: *const NextApi, args: *const ArgSlots, ret: *mut RetSlot) {
    let link = unsafe { &mut *(*api).state.cast::<Option<Link<'_>>>() };
    if let Some(link) = link.take() {
        unsafe { ret.write(link.run(*args)) };
    }
}

/// An immutable snapshot of a site's subscribers in execution order; replaced whole on every change.
pub(crate) struct Chain {
    pub callee: Callee,
    /// The site's name, read by the dispatch for the thread's slot.
    pub name: String,
    /// Every entry in registration order, the input of the next rebuild.
    pub entries: Vec<Arc<Entry>>,
    befores: Vec<Arc<Entry>>,
    arounds: Vec<Arc<Entry>>,
    replace: Option<Arc<Entry>>,
    afters: Vec<Arc<Entry>>,
}

impl Chain {
    /// Orders `entries` per phase with the plugin loader's sort (constraints, then priority, then
    /// position, which here is the plugin's load rank and then registration order); refuses a
    /// second `Replace` and any constraint cycle.
    pub(crate) fn build(callee: Callee, entries: Vec<Arc<Entry>>, ranks: &Ranks, site: &str) -> Result<Box<Self>, Error> {
        let replaces: Vec<_> = entries.iter().filter(|e| e.meta.phase == Phase::Replace).collect();
        if let [first, .., last] = replaces[..] {
            return Err(Error::Exclusive { site: site.to_owned(), existing: first.meta.owner.clone(), refused: last.meta.owner.clone() });
        }
        let sorted = |phase: Phase| -> Result<Vec<Arc<Entry>>, Error> {
            let mut of_phase: Vec<&Arc<Entry>> = entries.iter().filter(|e| e.meta.phase == phase).collect();
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
            callee,
            name: site.to_owned(),
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

    fn run(&self, site: &Site, args: &mut ArgSlots) -> RetSlot {
        let start = Instant::now();
        for entry in self.befores.iter().filter(|e| e.meta.runs()) {
            let Subscriber::Before(f) = &entry.subscriber else { unreachable!() };
            track(&entry.meta, || (f.call)(f.ctx, args));
        }
        let mut state = State { received: *args, outcome: Outcome::Skipped };
        let ret = Link { chain: self, state: &mut state, index: 0 }.run(*args);
        for entry in self.afters.iter().filter(|e| e.meta.runs()) {
            let Subscriber::After(f) = &entry.subscriber else { unreachable!() };
            track(&entry.meta, || (f.call)(f.ctx, &raw const state.received, &raw const ret, state.outcome as u8));
        }
        site.stats.record(start.elapsed());
        site.last_outcome.store(state.outcome as u8, Relaxed);
        ret
    }
}

/// What a site's detour replaces: a function entry (through `MinHook`), a `call rel32` or a vtable slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Entry,
    Call,
    Slot,
}

impl Kind {
    pub(crate) fn validate(self, address: usize, ours: Option<usize>) -> Validation {
        match self {
            Self::Entry => validate::validate(address, ours),
            Self::Call => validate::validate_call(address, ours),
            Self::Slot => validate::validate_slot(address, ours),
        }
    }

    /// The bytes a site of this kind at `address` holds once installed: an entry's first 16, which
    /// `MinHook` may relocate into its trampoline, the call instruction, or the pointer.
    pub(crate) fn span(self, address: usize) -> usize {
        match self {
            Self::Entry => 16,
            Self::Call => hook::call_length(address).unwrap_or(5),
            Self::Slot => size_of::<usize>(),
        }
    }
}

/// What the registry knows about an installed site.
pub(crate) struct SiteMeta {
    pub name: String,
    pub kind: Kind,
    pub address: usize,
    /// The bytes the site holds, as [`Kind::span`] measured them at install.
    pub span: usize,
    /// What the site leads to: the detour, or the stub that hands a call's subscribers a register.
    pub detour: usize,
    /// The callee or the slot's pointer before the detour, restored when the last subscriber of a call or slot site goes.
    pub original: usize,
    pub validation: Validation,
    /// The module another module's jump at the entry leads to: the patch stacked on at install, or the one that overwrote the detour since.
    pub patched_by: Option<String>,
    /// The subscribers in execution order, for the reports.
    pub order: Vec<Arc<Meta>>,
}

/// One engine function's hook state. The detour reads `chain` under the epoch counters; the
/// registry, holding its lock, swaps a new chain in and waits for every call that could see the
/// old one to leave before freeing it.
pub struct Site {
    chain: AtomicPtr<Chain>,
    epoch: AtomicUsize,
    active: [AtomicUsize; 2],
    stats: Stats,
    last_outcome: AtomicU8,
    /// Set when another module overwrote the detour in refuse mode: calls go straight to the original.
    pub(crate) broken: AtomicBool,
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
            broken: AtomicBool::new(false),
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
    /// The caller must hold the registry lock, and `chain` must call the site's own original.
    pub(crate) unsafe fn publish(&self, chain: Box<Chain>) {
        let old = self.chain.swap(Box::into_raw(chain), SeqCst);
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
            drop(unsafe { Box::from_raw(old) });
        }
    }

    /// The published chain; the registry lock keeps it alive.
    pub(crate) fn chain(&self) -> Option<&Chain> {
        unsafe { self.chain.load(SeqCst).as_ref() }
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
/// The caller must hold the registry lock.
pub(crate) unsafe fn rebuild(site: &Site, meta: &mut SiteMeta, ranks: &Ranks, keep: &dyn Fn(&Meta) -> bool) -> usize {
    let (new, removed) = {
        let chain = site.chain().expect("published at install");
        let entries: Vec<_> = chain.entries.iter().filter(|e| keep(&e.meta)).cloned().collect();
        let removed = chain.entries.len() - entries.len();
        (Chain::build(chain.callee, entries, ranks, &meta.name).expect("a subset of an ordered chain is ordered"), removed)
    };
    meta.order = new.order();
    unsafe { site.publish(new) };
    removed
}
