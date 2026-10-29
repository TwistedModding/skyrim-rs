//! The host's registry: one lock serializes every change to the sites and answers the plugins' requests.

use core::any::Any;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::ops::{self, OpVisitor, RAW_SLOTS, SHAPES};
use super::site::{self, Callback, Chain, Entry, Kind, Meta, Op, Ranks, Signature, Site, SiteMeta, Stats, Timing};
use super::validate::{Validation, branch_kind, hex, read_bytes, validate};
use super::{Compat, EntryReport, Error, Location, PatchId, PatchReport, PatchRequest, PatchState, Phase, Request, SiteReport, SubscriptionId, Target};

/// A managed byte patch.
struct Patch {
    id: u64,
    owner: String,
    at: Location,
    address: usize,
    original: Vec<u8>,
    replacement: Vec<u8>,
    enabled: bool,
}

/// The longest byte patch accepted.
const PATCH_MAX: usize = 64;

struct Registry {
    ranks: Ranks,
    /// The next subscription or patch id; both share the sequence.
    next_id: u64,
    /// Kind, address and shape per raw slot, in slot order.
    raw: Vec<(Kind, usize, usize)>,
    patches: Vec<Patch>,
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
    patches: Vec::new(),
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

    fn raw_slot(&mut self, kind: Kind, address: usize, shape: usize) -> Result<usize, Error> {
        let refuse = |detail: String| Error::Raw { address, detail };
        if shape >= SHAPES.len() {
            return Err(refuse(format!("no shape {shape}")));
        }
        if let Some(slot) = self.raw.iter().position(|&(k, a, _)| (k, a) == (kind, address)) {
            let taken = self.raw[slot].2;
            return if taken == shape { Ok(slot) } else { Err(refuse(format!("already hooked as {}", label(taken)))) };
        }
        if self.raw.len() == RAW_SLOTS {
            return Err(refuse(format!("all {RAW_SLOTS} raw slots are taken")));
        }
        self.raw.push((kind, address, shape));
        Ok(self.raw.len() - 1)
    }

    /// What already holds any of the `len` bytes at `address`: a patch, an installed site or
    /// another module's jump at an entry the last check saw.
    fn holder(&self, address: usize, len: usize) -> Option<String> {
        let overlaps = |start: usize, n: usize| start < address + len && address < start + n;
        if let Some(p) = self.patches.iter().find(|p| overlaps(p.address, p.original.len())) {
            return Some(format!("patch #{} of {} at {}", p.id, p.owner, p.at));
        }
        let site = self.sites.iter().find_map(|site| {
            let guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
            guard.as_ref().filter(|m| overlaps(m.address, m.kind.span())).map(|m| format!("the hook site {}", m.name))
        });
        site.or_else(|| self.seen.iter().find(|&(&a, _)| overlaps(a, 5)).map(|(_, (name, v))| format!("{name}, patched by {}", patcher(v))))
    }

    /// Forgets a patch, writing its original bytes back when it is applied and still intact.
    fn drop_patch(&mut self, id: u64) -> bool {
        let Some(index) = self.patches.iter().position(|p| p.id == id) else { return false };
        let p = self.patches.remove(index);
        let intact = read_bytes(p.address, p.replacement.len()).is_some_and(|b| b == p.replacement);
        let outcome = match (p.enabled, intact) {
            (false, _) => "removed".to_owned(),
            (true, true) => match unsafe { hook::write_code(p.address, &p.original) } {
                Ok(()) => "original bytes restored".to_owned(),
                Err(e) => format!("restoring failed: {e}"),
            },
            (true, false) => "left as it is, another writer changed it".to_owned(),
        };
        self.log(&format!("patch at {}: #{id} of {} {outcome}", p.at, p.owner));
        true
    }

    fn add_patch(&mut self, req: PatchRequest, site: &str) -> Result<usize, Error> {
        let refuse = |detail: String| Error::Install { site: site.to_owned(), owner: req.owner.clone(), detail };
        let len = req.expected.len();
        if !(1..=PATCH_MAX).contains(&len) || req.replacement.len() != len {
            return Err(refuse(format!("{len} expected and {} replacement bytes, both must be 1 to {PATCH_MAX} and equal", req.replacement.len())));
        }
        let address = req.at.resolve().map_err(refuse)?;
        if let Some(with) = self.holder(address, len) {
            return Err(Error::Overlap { site: site.to_owned(), owner: req.owner.clone(), with });
        }
        let found = read_bytes(address, len).ok_or_else(|| refuse(format!("{len} bytes at {address:#x} are not readable")))?;
        if found != req.expected {
            return Err(Error::Mismatch { site: site.to_owned(), owner: req.owner.clone(), expected: req.expected, found });
        }
        unsafe { hook::write_code(address, &req.replacement) }.map_err(|e| refuse(e.to_string()))?;
        self.patches.push(Patch { id: self.next_id, owner: req.owner, at: req.at, address, original: req.expected, replacement: req.replacement, enabled: true });
        self.next_id += 1;
        Ok(self.patches.len() - 1)
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
        let now = meta.kind.validate(meta.address, Some(meta.detour));
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

fn label(shape: usize) -> &'static str {
    SHAPES.get(shape).copied().unwrap_or("?")
}

pub(crate) fn subscribe(req: Request) -> Result<SubscriptionId, Error> {
    if site::in_callback() {
        return Err(Error::Reentrant);
    }
    let mut reg = lock();
    let unresolved = |name: String, detail: String| Error::Install { site: name, owner: req.owner.clone(), detail };
    let place = match req.target {
        Target::Op(index) => {
            let (name, id) = ops::OPS[index];
            skyrim::reloc().address(id).map(|address| (Kind::Entry, address, None, name.to_owned())).map_err(|e| unresolved(name.to_owned(), e.to_string()))
        }
        Target::Raw { address, shape } => Ok((Kind::Entry, address, Some(shape), format!("raw {address:#x}/{}", label(shape)))),
        Target::Call { at, shape } => {
            let name = |kind: &str| format!("{kind} {at}/{}", label(shape));
            at.resolve().map(|address| (Kind::Call, address, Some(shape), name(branch_kind(address)))).map_err(|e| unresolved(name("call"), e))
        }
        Target::Vtable { table, slot, shape } => {
            let name = format!("vtable {table}[{slot}]/{}", label(shape));
            table.resolve().map(|t| (Kind::Slot, t + slot * size_of::<usize>(), Some(shape), name.clone())).map_err(|e| unresolved(name, e))
        }
    };
    let result = place.and_then(|(kind, address, shape, name)| match (req.target, shape) {
        (Target::Op(index), _) => ops::with_op(index, Subscriber { reg: &mut reg, req, name, kind, address }).expect("index from the op table"),
        (_, shape) => {
            let shape = shape.expect("every raw target has a shape");
            reg.raw_slot(kind, address, shape).and_then(|slot| ops::with_raw(shape, slot, Subscriber { reg: &mut reg, req, name, kind, address }).expect("slot and shape checked"))
        }
    });
    if let Err(e) = &result {
        reg.log(&format!("refused: {e}"));
        edit(&mut reg, &|_| true);
    }
    result
}

struct Subscriber<'a> {
    reg: &'a mut Registry,
    req: Request,
    name: String,
    kind: Kind,
    address: usize,
}

impl OpVisitor<Result<SubscriptionId, Error>> for Subscriber<'_> {
    fn visit<O: Op>(self) -> Result<SubscriptionId, Error> {
        let Self { reg, req, name, kind, address } = self;
        let site = O::site();
        install::<O>(reg, site, &name, kind, address, &req.owner)?;
        let callback = downcast::<O::Fn>(req.phase, req.signature, req.callback).ok_or_else(|| Error::Callback { site: name.clone(), owner: req.owner.clone() })?;
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

fn downcast<F: Signature>(phase: Phase, signature: &str, callback: Box<dyn Any + Send + Sync>) -> Option<Callback<F>> {
    Some(match phase {
        Phase::Before => Callback::Before(cast(signature, callback)?),
        Phase::Around => Callback::Around(cast(signature, callback)?),
        Phase::Replace => Callback::Replace(cast(signature, callback)?),
        Phase::After => Callback::After(cast(signature, callback)?),
    })
}

/// The callback as `T` when the plugin named its type as the host names `T`. A plugin is loaded
/// only when its descriptor's ABI number, framework version, rustc and source hash match the
/// host's, so the same type name stands for the same type even where crate metadata differs.
fn cast<T: Send + Sync + 'static>(signature: &str, callback: Box<dyn Any + Send + Sync>) -> Option<T> {
    (signature == core::any::type_name::<T>()).then(|| *unsafe { Box::from_raw(Box::into_raw(callback).cast::<T>()) })
}

/// Puts a created detour in.
type Enable = Box<dyn FnOnce() -> Result<(), hook::Error>>;

/// Validates the site and puts the one detour in, with an empty chain published first.
fn install<O: Op>(reg: &mut Registry, site: &'static Site, name: &str, kind: Kind, address: usize, owner: &str) -> Result<(), Error> {
    let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.is_some() {
        return Ok(());
    }
    if let Some(with) = reg.holder(address, kind.span()) {
        return Err(Error::Overlap { site: name.to_owned(), owner: owner.to_owned(), with });
    }
    let detour = <O::Fn as Signature>::detour::<O>();
    let detour_address: usize = unsafe { core::mem::transmute_copy(&detour) };
    let validation = kind.validate(address, (kind == Kind::Entry).then_some(detour_address));
    let patched_by = validation.patched_by();
    if !validation.accepted() {
        // Stacking calls the patch as the original, which needs a jump to call; other edits are always refused.
        let Some(by) = patched_by.as_ref().filter(|_| reg.compat == Compat::Stack) else {
            return Err(Error::Site { site: name.to_owned(), owner: owner.to_owned(), validation });
        };
        reg.log(&format!("{name} at {address:#x}: stacking on the patch by {by}: {validation}"));
    }
    let failed = |e: hook::Error| Error::Install { site: name.to_owned(), owner: owner.to_owned(), detail: e.to_string() };
    let (original, enable): (O::Fn, Enable) = match kind {
        Kind::Entry => {
            let hook = unsafe { hook::Hook::create(address, detour) }.map_err(failed)?;
            (hook.original(), Box::new(move || hook.enable()))
        }
        Kind::Call => {
            let call = unsafe { hook::CallSite::create(address, detour) }.map_err(failed)?;
            (call.original(), Box::new(move || unsafe { call.enable() }))
        }
        Kind::Slot => {
            if !address.is_multiple_of(size_of::<usize>()) {
                return Err(Error::Raw { address, detail: "a vtable slot must be pointer-aligned".to_owned() });
            }
            let previous = unsafe { (address as *const usize).read() };
            (unsafe { core::mem::transmute_copy(&previous) }, Box::new(move || unsafe { hook::write_pointer(address, detour_address) }.map(drop)))
        }
    };
    let chain = Chain::build(original, Vec::new(), &reg.ranks, name).expect("an empty chain has no conflicts");
    unsafe { site.publish(chain) };
    enable().map_err(failed)?;
    site.broken.store(false, Relaxed);
    let original = unsafe { core::mem::transmute_copy(&original) };
    *guard = Some(SiteMeta { name: name.to_owned(), kind, address, detour: detour_address, original, validation, patched_by, order: Vec::new(), rebuild: site::rebuild::<O::Fn> });
    reg.sites.push(site);
    reg.log(&format!("{name} at {address:#x}: detour installed, entry {validation}"));
    Ok(())
}

/// Rebuilds every site that holds an entry `keep` rejects, draining each, and restores each call
/// or slot site left without subscribers; returns how many entries went.
fn edit(reg: &mut Registry, keep: &dyn Fn(&Meta) -> bool) -> usize {
    let mut removed = 0;
    let mut retired = Vec::new();
    for &site in &reg.sites {
        let mut guard = site.meta.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(meta) = guard.as_mut() else { continue };
        if !meta.order.iter().all(|m| keep(m)) {
            let rebuild = meta.rebuild;
            removed += unsafe { rebuild(site, meta, &reg.ranks, keep) };
        }
        if meta.order.is_empty() && meta.kind != Kind::Entry {
            retired.push((site, meta.address, restore(meta)));
            *guard = None;
        }
    }
    reg.sites.retain(|s| !retired.iter().any(|(r, _, _)| core::ptr::eq(*r, *s)));
    for (_, address, line) in retired {
        reg.seen.remove(&address);
        reg.log(&line);
    }
    removed
}

/// Points a call or slot site back at its original, unless another module has changed it since.
/// The site's chain stays published: a call already past the patch reaches the original through it.
fn restore(meta: &SiteMeta) -> String {
    let now = meta.kind.validate(meta.address, Some(meta.detour));
    if !matches!(now, Validation::Managed { .. }) {
        return format!("{} at {:#x}: no subscriber left, left as it is: {now}", meta.name, meta.address);
    }
    let written = match meta.kind {
        Kind::Call => unsafe { hook::retarget(meta.address, meta.original) }.map(drop),
        Kind::Slot => unsafe { hook::write_pointer(meta.address, meta.original) }.map(drop),
        Kind::Entry => Ok(()),
    };
    match written {
        Ok(()) => format!("{} at {:#x}: no subscriber left, original {:#x} restored", meta.name, meta.address, meta.original),
        Err(e) => format!("{} at {:#x}: no subscriber left, restoring failed: {e}", meta.name, meta.address),
    }
}

pub(crate) fn unsubscribe(id: SubscriptionId) -> bool {
    if site::in_callback() {
        return false;
    }
    let mut reg = lock();
    let removed = edit(&mut reg, &|m| m.id != id.0) == 1;
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
    let removed = edit(&mut reg, &|m| m.owner != owner);
    reg.log(&format!("{owner}: {removed} subscriptions removed"));
    let ids: Vec<u64> = reg.patches.iter().filter(|p| p.owner == owner).map(|p| p.id).collect();
    for id in ids {
        reg.drop_patch(id);
    }
    removed
}

pub(crate) fn patch(req: PatchRequest) -> Result<PatchId, Error> {
    if site::in_callback() {
        return Err(Error::Reentrant);
    }
    let mut reg = lock();
    let site = format!("patch at {}", req.at);
    match reg.add_patch(req, &site) {
        Ok(index) => {
            let p = &reg.patches[index];
            let (id, line) = (p.id, format!("{site} ({:#x}): #{} of {} applied, {} -> {}", p.address, p.id, p.owner, hex(&p.original), hex(&p.replacement)));
            reg.log(&line);
            Ok(PatchId(id))
        }
        Err(e) => {
            reg.log(&format!("refused: {e}"));
            Err(e)
        }
    }
}

pub(crate) fn set_patch_enabled(id: PatchId, enabled: bool) -> Result<(), Error> {
    let mut reg = lock();
    let Some(p) = reg.patches.iter_mut().find(|p| p.id == id.0) else {
        return Err(Error::NoPatch(id));
    };
    if p.enabled == enabled {
        return Ok(());
    }
    let (from, to) = if enabled { (&p.original, &p.replacement) } else { (&p.replacement, &p.original) };
    let site = format!("patch at {}", p.at);
    let found = read_bytes(p.address, from.len()).unwrap_or_default();
    let result = if found == *from {
        unsafe { hook::write_code(p.address, to) }.map_err(|e| Error::Install { site: site.clone(), owner: p.owner.clone(), detail: e.to_string() })
    } else {
        Err(Error::Mismatch { site: site.clone(), owner: p.owner.clone(), expected: from.clone(), found })
    };
    let line = match &result {
        Ok(()) => {
            p.enabled = enabled;
            format!("{site}: #{} of {} {}", id.0, p.owner, if enabled { "enabled" } else { "disabled" })
        }
        Err(e) => format!("refused: {e}"),
    };
    reg.log(&line);
    result
}

pub(crate) fn unpatch(id: PatchId) -> bool {
    lock().drop_patch(id.0)
}

pub(crate) fn patches() -> Vec<PatchReport> {
    lock()
        .patches
        .iter()
        .map(|p| {
            let found = read_bytes(p.address, p.original.len()).unwrap_or_default();
            let state = match p.enabled {
                true if found == p.replacement => PatchState::Applied,
                false if found == p.original => PatchState::Disabled,
                _ => PatchState::Overwritten { found },
            };
            PatchReport { id: PatchId(p.id), owner: p.owner.clone(), site: p.at, address: p.address, original: p.original.clone(), replacement: p.replacement.clone(), state }
        })
        .collect()
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
            let current = meta.kind.validate(meta.address, Some(meta.detour));
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
