//! The host's registry: one lock serializes every change to the sites and answers the plugins' requests.

use core::any::Any;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::ops::{self, OpVisitor, RAW_MAX_ARITY, RAW_SLOTS};
use super::site::{self, AfterFn, AroundFn, BeforeFn, Callback, Chain, Entry, Meta, Op, Ranks, ReplaceFn, Signature, Site, SiteMeta, Stats, Timing};
use super::validate::{Validation, validate};
use super::{Compat, EntryReport, Error, Phase, Request, SiteReport, SubscriptionId, Target};

struct Registry {
    ranks: Ranks,
    next_id: u64,
    /// Address and arity per raw slot, in slot order.
    raw: Vec<(usize, usize)>,
    sites: Vec<&'static Site>,
    log: Option<fn(&str)>,
    compat: Compat,
    /// The unmanaged patches the last check found, by address with the site's name, so each is reported once.
    seen: BTreeMap<usize, (String, Validation)>,
    /// The last [`MESSAGES`] lines logged, oldest first.
    messages: VecDeque<String>,
}

/// How many log lines the registry keeps for [`messages`].
const MESSAGES: usize = 64;

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    ranks: Ranks::new(),
    next_id: 1,
    raw: Vec::new(),
    sites: Vec::new(),
    log: None,
    compat: Compat::Refuse,
    seen: BTreeMap::new(),
    messages: VecDeque::new(),
});

fn lock() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Registry {
    fn log(&mut self, line: &str) {
        if let Some(f) = self.log {
            f(line);
        }
        if self.messages.len() == MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(line.to_owned());
    }

    fn raw_slot(&mut self, address: usize, arity: usize) -> Result<usize, Error> {
        let refuse = |detail: String| Error::Raw { address, detail };
        if arity > RAW_MAX_ARITY {
            return Err(refuse(format!("arity {arity} is above {RAW_MAX_ARITY}")));
        }
        if let Some(slot) = self.raw.iter().position(|&(a, _)| a == address) {
            let taken = self.raw[slot].1;
            return if taken == arity { Ok(slot) } else { Err(refuse(format!("already hooked with arity {taken}"))) };
        }
        if self.raw.len() == RAW_SLOTS {
            return Err(refuse(format!("all {RAW_SLOTS} raw slots are taken")));
        }
        self.raw.push((address, arity));
        Ok(self.raw.len() - 1)
    }

    /// Records one site's check; true when it found an unmanaged patch the previous check did not.
    fn note(&mut self, name: &str, address: usize, now: Validation) -> bool {
        if now.accepted() {
            self.seen.remove(&address);
            return false;
        }
        self.seen.insert(address, (name.to_owned(), now)).is_none_or(|(_, before)| before != now)
    }
}

/// Routes the host's log lines; set once by the host before any plugin loads.
pub(crate) fn set_logger(f: fn(&str)) {
    lock().log = Some(f);
}

pub(crate) fn set_compat(compat: Compat) {
    lock().compat = compat;
}

pub(crate) fn compat() -> Compat {
    lock().compat
}

/// Validates every installed site and the entries in `ops`, and logs each patch the previous check
/// did not see: an unmanaged patch on a site nobody subscribed to yet, or an
/// installed detour another module overwrote, which breaks the site in refuse mode.
pub(crate) fn revalidate(moment: &str, ops: &[(&str, usize)]) {
    let mut reg = lock();
    let mut checked = Vec::new();
    for site in reg.sites.clone() {
        let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(meta) = guard.as_mut() else { continue };
        checked.push(meta.address);
        let now = validate(meta.address, Some(meta.detour));
        if !reg.note(&meta.name, meta.address, now) {
            continue;
        }
        let by = patcher(&now);
        let effect = match reg.compat {
            Compat::Refuse => {
                site.broken.store(true, Relaxed);
                "broken, its subscribers no longer run"
            }
            Compat::Stack => "stack mode, its subscribers still run behind it",
        };
        reg.log(&format!("{moment}: {} at {:#x}: managed detour overwritten by {by}: {now}; {effect}", meta.name, meta.address));
        meta.patched_by = Some(by);
    }
    for &(name, address) in ops {
        if checked.contains(&address) {
            continue;
        }
        checked.push(address);
        let now = validate(address, None);
        if reg.note(name, address, now) {
            reg.log(&format!("{moment}: {name} at {address:#x}: patched by {}: {now}", patcher(&now)));
        }
    }
    let line = format!("{moment}: {} sites checked, {} patched by another module, compatibility mode {}", checked.len(), reg.seen.len(), reg.compat);
    reg.log(&line);
}

pub(crate) fn subscribe(req: Request) -> Result<SubscriptionId, Error> {
    if site::in_callback() {
        return Err(Error::Reentrant);
    }
    let mut reg = lock();
    let result = match req.target {
        Target::Op(index) => {
            let (name, id) = ops::OPS[index];
            let address = skyrim::reloc().address(id).map_err(|e| Error::Install { site: name.to_owned(), owner: req.owner.clone(), detail: e.to_string() });
            address.and_then(|address| ops::with_op(index, Subscriber { reg: &mut reg, req, name: name.to_owned(), address }).expect("index from the op table"))
        }
        Target::Raw { address, arity } => reg
            .raw_slot(address, arity)
            .and_then(|slot| ops::with_raw(arity, slot, Subscriber { reg: &mut reg, req, name: format!("raw {address:#x}/{arity}"), address }).expect("slot and arity checked")),
    };
    if let Err(e) = &result {
        reg.log(&format!("refused: {e}"));
    }
    result
}

struct Subscriber<'a> {
    reg: &'a mut Registry,
    req: Request,
    name: String,
    address: usize,
}

impl OpVisitor<Result<SubscriptionId, Error>> for Subscriber<'_> {
    fn visit<O: Op>(self) -> Result<SubscriptionId, Error> {
        let Self { reg, req, name, address } = self;
        let site = O::site();
        install::<O>(reg, site, &name, address, &req.owner)?;
        let callback = downcast::<O::Fn>(req.phase, req.callback).ok_or_else(|| Error::Callback { site: name.clone(), owner: req.owner.clone() })?;
        let id = reg.next_id;
        reg.next_id += 1;
        let meta = Arc::new(Meta { id, owner: req.owner, phase: req.phase, constraints: req.constraints, stats: Stats::new(), enabled: AtomicBool::new(true) });
        let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        let site_meta = guard.as_mut().expect("installed above");
        let new = {
            let chain = unsafe { site.chain::<O::Fn>() }.expect("published at install");
            let mut entries = chain.entries.clone();
            entries.push(Arc::new(Entry { meta: Arc::clone(&meta), callback }));
            Chain::build(chain.original, entries, &reg.ranks, &site_meta.name)?
        };
        site_meta.order = new.order();
        unsafe { site.publish(new) };
        reg.log(&format!("{name}: {} {:?} subscribed as #{id}", meta.owner, meta.phase));
        Ok(SubscriptionId(id))
    }
}

fn downcast<F: Signature>(phase: Phase, callback: Box<dyn Any + Send + Sync>) -> Option<Callback<F>> {
    Some(match phase {
        Phase::Before => Callback::Before(*callback.downcast::<Box<BeforeFn<F>>>().ok()?),
        Phase::Around => Callback::Around(*callback.downcast::<Box<AroundFn<F>>>().ok()?),
        Phase::Replace => Callback::Replace(*callback.downcast::<Box<ReplaceFn<F>>>().ok()?),
        Phase::After => Callback::After(*callback.downcast::<Box<AfterFn<F>>>().ok()?),
    })
}

/// Validates the entry and puts the one detour in, with an empty chain published first.
fn install<O: Op>(reg: &mut Registry, site: &'static Site, name: &str, address: usize, owner: &str) -> Result<(), Error> {
    let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.is_some() {
        return Ok(());
    }
    let detour = <O::Fn as Signature>::detour::<O>();
    let detour_address: usize = unsafe { core::mem::transmute_copy(&detour) };
    let validation = validate(address, Some(detour_address));
    let patched_by = validation.patched_by();
    if !validation.accepted() {
        // Stacking calls the patch as the original, which needs a jump to call; other edits are always refused.
        let Some(by) = patched_by.as_ref().filter(|_| reg.compat == Compat::Stack) else {
            return Err(Error::Site { site: name.to_owned(), owner: owner.to_owned(), validation });
        };
        reg.log(&format!("{name} at {address:#x}: stacking on the patch by {by}: {validation}"));
    }
    let failed = |e: hook::Error| Error::Install { site: name.to_owned(), owner: owner.to_owned(), detail: e.to_string() };
    let hook = unsafe { hook::Hook::create(address, detour) }.map_err(failed)?;
    let chain = Chain::build(hook.original(), Vec::new(), &reg.ranks, name).expect("an empty chain has no conflicts");
    unsafe { site.publish(chain) };
    hook.enable().map_err(failed)?;
    *guard = Some(SiteMeta { name: name.to_owned(), address, detour: detour_address, validation, patched_by, order: Vec::new(), rebuild: site::rebuild::<O::Fn> });
    reg.sites.push(site);
    reg.log(&format!("{name} at {address:#x}: detour installed, entry {validation}"));
    Ok(())
}

/// Rebuilds every site that holds an entry `keep` rejects, draining each; returns how many entries went.
fn edit(reg: &Registry, keep: &dyn Fn(&Meta) -> bool) -> usize {
    let mut removed = 0;
    for site in &reg.sites {
        let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(meta) = guard.as_mut() else { continue };
        if meta.order.iter().all(|m| keep(m)) {
            continue;
        }
        let rebuild = meta.rebuild;
        removed += unsafe { rebuild(site, meta, &reg.ranks, keep) };
    }
    removed
}

pub(crate) fn unsubscribe(id: SubscriptionId) -> bool {
    if site::in_callback() {
        return false;
    }
    let mut reg = lock();
    let removed = edit(&reg, &|m| m.id != id.0) == 1;
    if removed {
        reg.log(&format!("#{} unsubscribed", id.0));
    }
    removed
}

pub(crate) fn remove_owner(owner: &str) -> usize {
    if site::in_callback() {
        return 0;
    }
    let mut reg = lock();
    let removed = edit(&reg, &|m| m.owner != owner);
    reg.log(&format!("{owner}: {removed} subscriptions removed"));
    removed
}

pub(crate) fn set_rank(owner: &str, rank: i32) {
    if site::in_callback() {
        return;
    }
    let mut reg = lock();
    reg.ranks.insert(owner.to_owned(), rank);
    for site in &reg.sites {
        let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(meta) = guard.as_mut() {
            let rebuild = meta.rebuild;
            unsafe { rebuild(site, meta, &reg.ranks, &|_| true) };
        }
    }
}

pub(crate) fn set_enabled(id: SubscriptionId, enabled: bool) -> bool {
    let mut reg = lock();
    let found = reg.sites.iter().find_map(|site| {
        let guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        let meta = guard.as_ref()?;
        let entry = meta.order.iter().find(|m| m.id == id.0)?;
        entry.enabled.store(enabled, Relaxed);
        Some(format!("{}: #{} {} {:?} {}", meta.name, id.0, entry.owner, entry.phase, if enabled { "enabled" } else { "disabled" }))
    });
    if let Some(line) = &found {
        reg.log(line);
    }
    found.is_some()
}

pub(crate) fn messages() -> Vec<String> {
    lock().messages.iter().cloned().collect()
}

/// The dispatcher's state for a crash log, written on the watchdog thread: what `thread` is
/// inside, the sites marked broken and the last log lines; a lock the crash holds is skipped.
pub(crate) fn crash_facts(w: &mut dyn core::fmt::Write, thread: u32) {
    match site::thread_state(thread) {
        Some((Some(site), Some(meta))) => {
            let _ = writeln!(w, "the crashing thread is inside hook site {site}, subscriber #{} {} {:?}", meta.id, meta.owner, meta.phase);
        }
        Some((Some(site), None)) => {
            let _ = writeln!(w, "the crashing thread is inside hook site {site}, between subscribers (in the engine function or the dispatcher)");
        }
        _ => {
            let _ = writeln!(w, "the crashing thread is not inside a hook subscriber");
        }
    }
    let Ok(reg) = REGISTRY.try_lock() else {
        let _ = writeln!(w, "hook registry: locked by another thread, not read");
        return;
    };
    for site in &reg.sites {
        if !site.broken.load(Relaxed) {
            continue;
        }
        match site.meta.try_lock() {
            Ok(guard) => {
                if let Some(meta) = guard.as_ref() {
                    let _ = writeln!(w, "broken site: {} at {:#x}, detour overwritten by {}", meta.name, meta.address, meta.patched_by.as_deref().unwrap_or("an unknown writer"));
                }
            }
            Err(_) => {
                let _ = writeln!(w, "broken site: its name is locked by another thread");
            }
        }
    }
    let _ = writeln!(w, "hook log, last {} lines:", reg.messages.len().min(16));
    for line in reg.messages.iter().skip(reg.messages.len().saturating_sub(16)) {
        let _ = writeln!(w, "  {line}");
    }
}

pub(crate) fn diagnostics() -> Vec<SiteReport> {
    let reg = lock();
    let mut reports: Vec<SiteReport> = reg
        .sites
        .iter()
        .filter_map(|site| {
            let guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
            let meta = guard.as_ref()?;
            let (timing, last_outcome) = site.snapshot();
            let entries = meta
                .order
                .iter()
                .map(|m| EntryReport {
                    id: SubscriptionId(m.id),
                    owner: m.owner.clone(),
                    phase: m.phase,
                    constraints: m.constraints.clone(),
                    rank: reg.ranks.get(&m.owner).copied().unwrap_or(0),
                    timing: m.stats.snapshot(),
                    enabled: m.enabled.load(Relaxed),
                })
                .collect();
            let current = validate(meta.address, Some(meta.detour));
            let broken = site.broken.load(Relaxed);
            Some(SiteReport { name: meta.name.clone(), address: meta.address, validation: meta.validation, current, patched_by: meta.patched_by.clone(), broken, timing, last_outcome, entries })
        })
        .collect();
    for (&address, (name, _)) in &reg.seen {
        let current = validate(address, None);
        if !current.accepted() && reports.iter().all(|r| r.address != address) {
            reports.push(SiteReport { name: name.clone(), address, validation: current, current, patched_by: current.patched_by(), broken: false, timing: Timing::default(), last_outcome: None, entries: Vec::new() });
        }
    }
    reports
}

fn patcher(v: &Validation) -> String {
    v.patched_by().unwrap_or_else(|| "an unknown writer".to_owned())
}
