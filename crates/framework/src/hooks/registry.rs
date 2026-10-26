//! The host's registry: one lock serializes every change to the sites and answers the plugins' requests.

use core::any::Any;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::ops::{self, OpVisitor, RAW_MAX_ARITY, RAW_SLOTS};
use super::site::{self, AfterFn, AroundFn, BeforeFn, Callback, Chain, Entry, Meta, Op, Ranks, ReplaceFn, Signature, Site, SiteMeta, Stats};
use super::validate::validate;
use super::{EntryReport, Error, Phase, Request, SiteReport, SubscriptionId, Target};

struct Registry {
    ranks: Ranks,
    next_id: u64,
    /// Address and arity per raw slot, in slot order.
    raw: Vec<(usize, usize)>,
    sites: Vec<&'static Site>,
    log: Option<fn(&str)>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry { ranks: Ranks::new(), next_id: 1, raw: Vec::new(), sites: Vec::new(), log: None });

fn lock() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Registry {
    fn log(&self, line: &str) {
        if let Some(f) = self.log {
            f(line);
        }
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
}

/// Routes the host's log lines; set once by the host before any plugin loads.
pub(crate) fn set_logger(f: fn(&str)) {
    lock().log = Some(f);
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
        let meta = Arc::new(Meta { id, owner: req.owner, phase: req.phase, constraints: req.constraints, stats: Stats::new() });
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
    if !validation.accepted() {
        return Err(Error::Site { site: name.to_owned(), owner: owner.to_owned(), validation });
    }
    let failed = |e: hook::Error| Error::Install { site: name.to_owned(), owner: owner.to_owned(), detail: e.to_string() };
    let hook = unsafe { hook::Hook::create(address, detour) }.map_err(failed)?;
    let chain = Chain::build(hook.original(), Vec::new(), &reg.ranks, name).expect("an empty chain has no conflicts");
    unsafe { site.publish(chain) };
    hook.enable().map_err(failed)?;
    *guard = Some(SiteMeta { name: name.to_owned(), address, detour: detour_address, validation, order: Vec::new(), rebuild: site::rebuild::<O::Fn> });
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
    let reg = lock();
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
    let reg = lock();
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

pub(crate) fn diagnostics() -> Vec<SiteReport> {
    let reg = lock();
    reg.sites
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
                })
                .collect();
            Some(SiteReport { name: meta.name.clone(), address: meta.address, validation: meta.validation, current: validate(meta.address, Some(meta.detour)), timing, last_outcome, entries })
        })
        .collect()
}
