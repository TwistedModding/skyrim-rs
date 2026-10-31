//! One detour per engine function, owned by the host, with every plugin's subscribers run in order.
//!
//! A call through a hooked function runs all `Before` subscribers (each may edit the arguments),
//! then the `Around` chain, whose innermost link is the engine function or the site's one
//! `Replace`, then all `After` subscribers, which see the arguments the innermost link received,
//! the result and the [`Outcome`]. An `Around` that returns without calling [`Next::call`] skips
//! the original; the `After`s still run and see `Outcome::Skipped`.
//!
//! Within a phase, subscribers run in the order the plugin loader's [`crate::order::sort`] gives
//! them: their `before`/`after` constraints on plugin names first, `priority` second (higher
//! first), then the plugins' load order and registration order. A registration that closes a
//! constraint cycle or adds a second `Replace` is refused and names the plugins involved.
//!
//! Before a site's detour goes in, its entry bytes are checked against the executable file
//! ([`Validation`]). What happens on an unmanaged patch (another module's jump at the entry) is the
//! host's [`Compat`] mode: refuse the detour, or stack on the patch as if it were the function. The
//! host also re-validates every typed operation's site at startup and at the loader's
//! `PLUGINS_LOADED_2` and `DATA_LOADED` messages, names the module behind each unmanaged patch, and
//! marks an installed site whose detour another module overwrote as broken.
//!
//! Besides function entries, a site can be one `call rel32`, tail `jmp rel32` or virtual call on
//! the first argument inside a function ([`ops::Call`]) or one vtable slot ([`ops::Vtable`]): the
//! call or the pointer is pointed at the dispatcher and the callee it reached is the original;
//! both are restored when their last subscriber goes. A call site can hand its subscribers a
//! register of the caller as one more argument ([`ops::Call::passing`]). Their
//! shape is raw (integer-class arguments) or one of the typed shapes [`ops::SHAPES`] lists, which
//! carry float arguments and results. A
//! plugin's byte patches ([`Hooks::patch`]) are kept in the same registry, which reserves their
//! bytes against each other and against the installed sites.
//!
//! Subscriptions are added and removed from any thread while the detours run; a removal returns
//! only once no call can still be inside the removed subscriber. Registering or removing from
//! inside a subscriber is refused, since it would wait for itself.
//!
//! The dispatcher's state is the host's alone: a plugin's [`Hooks`] reaches it through its
//! `HostApi` entries, its subscribers cross as `Callback`s over the call's argument slots
//! ([`raw`]), and [`Next::call`] runs the rest of the chain in the host.

use std::fmt;

use crate::abi::{self, Api, HostApi, RawError, Sink, Str, call};
use crate::order::Constraints;

pub mod ops;
pub mod raw;
pub(crate) mod registry;
mod site;
mod validate;

use raw::{RawLocation, RawPatch, RawPatchReport, RawSignature, RawSiteReport, RawSubscription, RawTarget};
pub use raw::{Next, Original, Slot};
pub(crate) use site::Subscriber;
pub use site::{Args, Op, Outcome, Ret, Signature, Site, Timing};
pub use validate::{Bytes, Validation, owner, validate, validate_call, validate_slot};

/// What the dispatcher does with a site another module has patched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compat {
    /// Refuse subscriptions on a pre-patched entry; stop running the subscribers of a site whose
    /// detour was overwritten.
    #[default]
    Refuse,
    /// Install on a pre-patched entry anyway, calling the patch as the original; keep running the
    /// subscribers of an overwritten site behind the other module's detour.
    Stack,
}

impl core::str::FromStr for Compat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "refuse" => Ok(Self::Refuse),
            "stack" => Ok(Self::Stack),
            other => Err(format!("unknown compatibility mode {other:?}, expected refuse or stack")),
        }
    }
}

impl fmt::Display for Compat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Refuse => "refuse",
            Self::Stack => "stack",
        })
    }
}

/// When a subscriber runs relative to the engine function.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Before = 0,
    Around = 1,
    Replace = 2,
    After = 3,
}

impl Phase {
    pub(crate) fn from_u32(value: u32) -> Option<Self> {
        [Self::Before, Self::Around, Self::Replace, Self::After].into_iter().find(|p| *p as u32 == value)
    }
}

/// What a subscription says about itself: its owner (the plugin name constraints refer to), the
/// plugins it must run before or after ([`crate::order::ALL`] for every other one) and its
/// priority (higher runs first).
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub owner: &'a str,
    pub priority: i32,
    pub before: &'a [&'a str],
    pub after: &'a [&'a str],
}

impl<'a> Options<'a> {
    #[must_use]
    pub const fn new(owner: &'a str) -> Self {
        Self { owner, priority: 0, before: &[], after: &[] }
    }

    #[must_use]
    pub const fn priority(self, priority: i32) -> Self {
        Self { priority, ..self }
    }

    #[must_use]
    pub const fn before(self, plugins: &'a [&'a str]) -> Self {
        Self { before: plugins, ..self }
    }

    #[must_use]
    pub const fn after(self, plugins: &'a [&'a str]) -> Self {
        Self { after: plugins, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    /// The number the logs and reports show as `#n`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn new(n: u64) -> Self {
        Self(n)
    }
}

/// Why a registration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The constraints of the named plugins contradict each other on this site.
    Cycle { site: String, names: Vec<String> },
    /// `existing` already replaces the function; `refused` asked to as well.
    Exclusive { site: String, existing: String, refused: String },
    /// The entry bytes are not the function's own; [`Validation::patched_by`] names the module the patch leads to.
    Site { site: String, owner: String, validation: Validation },
    /// The address did not resolve or the detour could not be written.
    Install { site: String, owner: String, detail: String },
    /// A raw address with a shape or slot problem.
    Raw { address: usize, detail: String },
    /// The subscriber's function type does not have the site's slot shape (argument count and classes, result class).
    Callback { site: String, owner: String },
    /// Called from inside a subscriber.
    Reentrant,
    /// The `Options` owner is not the name of the plugin the handle belongs to.
    Owner { given: String, plugin: String },
    /// The bytes are held by `with`: another patch, an installed hook site or another module's patch.
    Overlap { site: String, owner: String, with: String },
    /// The live bytes are not the ones a patch expects to find (or, toggling it, the ones it left).
    Mismatch { site: String, owner: String, expected: Vec<u8>, found: Vec<u8> },
    /// No patch has this id.
    NoPatch(PatchId),
    /// The loader lacks what the call needs: the hook entries, or a target kind or operation of a later framework.
    Unsupported { what: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle { site, names } => write!(f, "{site}: ordering cycle between {}", names.join(", ")),
            Self::Exclusive { site, existing, refused } => write!(f, "{site}: {refused} cannot replace it, {existing} already does"),
            Self::Site { site, owner, validation } => {
                write!(f, "{site}: refused for {owner}, {validation}")?;
                validation.patched_by().map_or(Ok(()), |by| write!(f, ", patched by {by}"))
            }
            Self::Install { site, owner, detail } => write!(f, "{site}: refused for {owner}, {detail}"),
            Self::Raw { address, detail } => write!(f, "raw {address:#x}: {detail}"),
            Self::Callback { site, owner } => write!(f, "{site}: {owner}'s callback is not of the site's signature"),
            Self::Reentrant => f.write_str("registration from inside a hook subscriber"),
            Self::Owner { given, plugin } => write!(f, "owner {given} given by the plugin {plugin}"),
            Self::Overlap { site, owner, with } => write!(f, "{site}: refused for {owner}, the bytes overlap {with}"),
            Self::Mismatch { site, owner, expected, found } => write!(f, "{site}: refused for {owner}, expected {}, found {}", validate::hex(expected), validate::hex(found)),
            Self::NoPatch(id) => write!(f, "no patch #{}", id.0),
            Self::Unsupported { what } => write!(f, "{what}: not supported by this loader"),
        }
    }
}

impl std::error::Error for Error {}

/// An address in the game: an Address Library id plus an offset, or an address already resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Id { id: u64, offset: usize },
    Address(usize),
}

impl Location {
    #[must_use]
    pub const fn id(id: u64, offset: usize) -> Self {
        Self::Id { id, offset }
    }

    fn resolve(self) -> Result<usize, String> {
        match self {
            Self::Id { id, offset } => skyrim::reloc().address(id).map(|a| a + offset).map_err(|e| e.to_string()),
            Self::Address(address) => Ok(address),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id { id, offset } => write!(f, "id {id}+{offset:#x}"),
            Self::Address(address) => write!(f, "{address:#x}"),
        }
    }
}

/// Where a subscription goes: a typed operation of [`ops`], a raw function entry, a call site
/// (with the caller's register it hands on, if any) or a vtable slot; `shape` indexes [`ops::SHAPES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Op(usize),
    Raw { address: usize, shape: usize },
    Call { at: Location, shape: usize, register: Option<Register> },
    Vtable { table: Location, slot: usize, shape: usize },
}

/// A callee-saved register of a call site's caller, which [`ops::Call::passing`] hands the
/// subscribers as one more argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Register {
    Rbx,
    Rbp,
    Rsi,
    Rdi,
    R12,
    R13,
    R14,
    R15,
}

impl Register {
    const ALL: [Self; 8] = [Self::Rbx, Self::Rbp, Self::Rsi, Self::Rdi, Self::R12, Self::R13, Self::R14, Self::R15];

    pub(crate) fn from_number(number: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| u32::from(r.number()) == number)
    }

    /// The x86 register number.
    pub(crate) const fn number(self) -> u8 {
        match self {
            Self::Rbx => 3,
            Self::Rbp => 5,
            Self::Rsi => 6,
            Self::Rdi => 7,
            Self::R12 => 12,
            Self::R13 => 13,
            Self::R14 => 14,
            Self::R15 => 15,
        }
    }
}

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rbx => "rbx",
            Self::Rbp => "rbp",
            Self::Rsi => "rsi",
            Self::Rdi => "rdi",
            Self::R12 => "r12",
            Self::R13 => "r13",
            Self::R14 => "r14",
            Self::R15 => "r15",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PatchId(u64);

impl PatchId {
    /// The number the logs and reports show as `#n`, from the same sequence as subscriptions.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn new(n: u64) -> Self {
        Self(n)
    }
}

/// A byte patch as the host's registry takes it.
pub(crate) struct PatchRequest {
    pub owner: String,
    pub at: Location,
    pub expected: Vec<u8>,
    pub replacement: Vec<u8>,
    /// The code a jump in place of `expected` leads to; `replacement` is that jump, made by the registry.
    pub cave: Option<Vec<u8>>,
}

/// What a patch's bytes hold now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchState {
    /// The replacement is in place.
    Applied,
    /// The original bytes are back, held for the patch.
    Disabled,
    /// Another writer changed the bytes since; the patch can no longer be toggled.
    Overwritten { found: Vec<u8> },
}

/// One managed byte patch, as [`Hooks::patches`] reports it.
#[derive(Clone, Debug)]
pub struct PatchReport {
    pub id: PatchId,
    pub owner: String,
    pub site: Location,
    pub address: usize,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
    /// The code the replacement jumps to, for [`Hooks::patch_cave`].
    pub cave: Option<Vec<u8>>,
    pub state: PatchState,
}

impl fmt::Display for PatchReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "patch #{} {} at {} ({:#x}): {} -> {}, ", self.id.0, self.owner, self.site, self.address, validate::hex(&self.original), validate::hex(&self.replacement))?;
        if let Some(code) = &self.cave {
            write!(f, "cave {}, ", validate::hex(code))?;
        }
        match &self.state {
            PatchState::Applied => f.write_str("applied"),
            PatchState::Disabled => f.write_str("disabled"),
            PatchState::Overwritten { found } => write!(f, "overwritten by another writer, found {}", validate::hex(found)),
        }
    }
}

/// Something a subscriber can attach to, with the signature its callbacks are typed by.
pub trait Hookable {
    type Fn: Signature;
    fn target(self) -> Target;
}

/// A subscription as the host's registry takes it.
pub(crate) struct Request {
    pub target: Target,
    pub owner: String,
    pub constraints: Constraints,
    /// The slot shape of the subscriber's function type, compared with the target's.
    pub signature: RawSignature,
    pub subscriber: site::Subscriber,
}

/// One site's state and subscribers, as [`Hooks::diagnostics`] reports it. A typed operation's
/// site that no one subscribed to is reported only while another module has patched it.
#[derive(Clone, Debug)]
pub struct SiteReport {
    pub name: String,
    pub address: usize,
    /// The entry as it was before the detour went in.
    pub validation: Validation,
    /// The entry now.
    pub current: Validation,
    /// The module (or anonymous page) another module's jump at the entry leads to, at install or since.
    pub patched_by: Option<String>,
    /// The detour was overwritten by another module in [`Compat::Refuse`] mode; the subscribers no longer run.
    pub broken: bool,
    pub timing: Timing,
    pub last_outcome: Option<Outcome>,
    /// In execution order: befores, arounds, the replace, afters.
    pub entries: Vec<EntryReport>,
}

/// One subscriber as [`Hooks::diagnostics`] reports it. An `Around`'s timing includes everything inside it.
#[derive(Clone, Debug)]
pub struct EntryReport {
    pub id: SubscriptionId,
    pub owner: String,
    pub phase: Phase,
    pub constraints: Constraints,
    /// The owner's position in the load order.
    pub rank: i32,
    pub timing: Timing,
    /// False while [`Hooks::set_enabled`] has the subscriber skipped.
    pub enabled: bool,
}

impl fmt::Display for SiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {:#x}: {} calls, last {} ns, mean {} ns, last outcome {:?}, entry {}", self.name, self.address, self.timing.calls, self.timing.last_ns, self.timing.mean_ns, self.last_outcome, self.current)?;
        if let Some(by) = &self.patched_by {
            write!(f, ", patched by {by}")?;
        }
        if self.broken {
            f.write_str(", broken")?;
        }
        for e in &self.entries {
            write!(f, "; {e}")?;
        }
        Ok(())
    }
}

impl fmt::Display for EntryReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{} {} {:?} rank {}", self.id.0, self.owner, self.phase, self.rank)?;
        if self.constraints != Constraints::default() {
            write!(f, " ({})", self.constraints)?;
        }
        write!(f, ": {} calls, last {} ns, mean {} ns", self.timing.calls, self.timing.last_ns, self.timing.mean_ns)?;
        if !self.enabled {
            f.write_str(", disabled")?;
        }
        Ok(())
    }
}

/// The host's dispatcher as a plugin reaches it through `Host::hooks`, a wrapper over the
/// plugin's `HostApi` entries: subscriptions and patches are accepted only for an `Options` owner
/// that is the plugin's own name, so a reload removes exactly them. On a loader without the
/// entries, registrations fail with [`Error::Unsupported`] and the rest answer as if empty.
pub struct Hooks {
    api: Api,
}

impl Hooks {
    pub(crate) fn new(api: *const HostApi) -> Self {
        Self { api: Api(api) }
    }

    pub(crate) fn set_logger(f: fn(&str)) {
        registry::set_logger(f);
    }

    pub(crate) fn set_compat(compat: Compat) {
        registry::set_compat(compat);
    }

    /// The host's compatibility mode.
    pub(crate) fn compat() -> Compat {
        registry::compat()
    }

    /// Writes the dispatcher's state for a crash log: see `registry::crash_facts`.
    pub(crate) fn crash_facts(w: &mut dyn fmt::Write, thread: u32) {
        registry::crash_facts(w, thread);
    }

    /// Checks every typed operation's site and every installed one, logging what changed since the last check.
    pub(crate) fn revalidate(moment: &str) {
        let resolved: Vec<(&str, usize)> = ops::OPS.iter().filter_map(|&(name, id)| Some((name, skyrim::reloc().address(id).ok()?))).collect();
        registry::revalidate(moment, &resolved);
    }

    /// Sets a plugin's load rank, the tie-breaker after `priority`; the host sets it from the
    /// resolved load order before any plugin's `load` runs.
    pub(crate) fn set_rank(owner: &str, rank: i32) {
        registry::set_rank(owner, rank);
    }

    /// Hands `request` to `enter` with the target, options and slot shape of `F`, and reads back
    /// the subscription's id.
    fn subscribe<F: Signature>(target: Target, options: Options<'_>, enter: impl FnOnce(&RawSubscription, *mut u64, Sink<'_, *const RawError>) -> Option<u32>) -> Result<SubscriptionId, Error> {
        let names = |list: &[&str]| list.iter().map(|s| Str::from(*s)).collect::<Vec<_>>();
        let (before, after) = (names(options.before), names(options.after));
        let request = RawSubscription {
            size: abi::size::<RawSubscription>(),
            target: RawTarget::new(target),
            owner: Str::from(options.owner),
            priority: options.priority,
            before: before.as_ptr(),
            before_len: before.len(),
            after: after.as_ptr(),
            after_len: after.len(),
            signature: F::SLOTS,
        };
        let mut id = 0;
        abi::outcome("hook subscriptions", |error| enter(&request, &raw mut id, error)).map(|()| SubscriptionId(id))
    }

    fn add_patch(&self, owner: &str, at: Location, expected: &[u8], replacement: &[u8], cave: Option<&[u8]>) -> Result<PatchId, Error> {
        let request = RawPatch { size: abi::size::<RawPatch>(), owner: Str::from(owner), at: RawLocation::new(at), expected: Str::new(expected), replacement: Str::new(replacement), cave: Str::maybe(cave) };
        let mut id = 0;
        abi::outcome("byte patches", |error| call!(self.api, hook_patch(&raw const request, &raw mut id, error))).map(|()| PatchId(id))
    }

    /// Writes `replacement` over the bytes at `at`, which must hold `expected` (of the same length,
    /// 1 to 64 bytes). Refused when the bytes differ, or overlap another patch, an installed hook
    /// site or another module's patch the host has seen; a hook subscription whose site would
    /// overlap the patch is refused in turn. The patch belongs to `owner` (this plugin's name) and
    /// its original bytes are written back when it is disabled or removed, and before the
    /// plugin's DLL is unloaded.
    ///
    /// # Safety
    /// The replacement must be valid code or data where it lands for every thread that runs or
    /// reads it, and no thread may be executing those bytes while they are written.
    pub unsafe fn patch(&self, owner: &str, at: Location, expected: &[u8], replacement: &[u8]) -> Result<PatchId, Error> {
        self.add_patch(owner, at, expected, replacement, None)
    }

    /// A patch whose replacement is a `jmp rel32` (padded with `nop`s) to `code`, which the host
    /// copies near `at` and follows with a jump back to the instruction after the `expected`
    /// bytes (5 to 64): for a change that does not fit in place. Checked, reserved, toggled,
    /// reported and restored as [`patch`](Self::patch) does; the report shows the cave's code.
    ///
    /// # Safety
    /// As [`patch`](Self::patch); `expected` must end on an instruction boundary that nothing
    /// jumps into, and `code` must run anywhere: no rip-relative operand and no relative branch
    /// out of it.
    pub unsafe fn patch_cave(&self, owner: &str, at: Location, expected: &[u8], code: &[u8]) -> Result<PatchId, Error> {
        self.add_patch(owner, at, expected, &[], Some(code))
    }

    /// Writes a patch's replacement back (`true`) or its original bytes (`false`); refused with
    /// `Error::Mismatch` when another writer changed the bytes since.
    pub fn set_patch_enabled(&self, id: PatchId, enabled: bool) -> Result<(), Error> {
        abi::outcome("byte patches", |error| call!(self.api, hook_set_patch_enabled(id.0, enabled, error)))
    }

    /// Restores a patch's original bytes and forgets it; false when no such patch exists. Bytes
    /// another writer changed since are left as they are.
    #[must_use]
    pub fn unpatch(&self, id: PatchId) -> bool {
        call!(self.api, hook_unpatch(id.0)).unwrap_or(false)
    }

    /// Every managed patch with its owner, bytes and state.
    #[must_use]
    pub fn patches(&self) -> Vec<PatchReport> {
        let mut reports = Vec::new();
        let mut keep = |p: *const RawPatchReport| reports.extend(unsafe { abi::read_sized(p) }.map(|p| unsafe { p.to_report() }));
        call!(self.api, hook_patches(Sink::new(&mut keep)));
        reports
    }

    /// Runs `f` before the function with the arguments, which it may edit.
    pub fn before<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&mut Args<H::Fn>) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        Self::subscribe::<H::Fn>(target.target(), options, |request, id, error| call!(self.api, hook_before(request, raw::before::<H::Fn, _>(f), id, error)))
    }

    /// Runs `f` around the function: it receives the arguments and the rest of the chain, which it
    /// may call once with any arguments or not at all, and returns the result.
    pub fn around<H: Hookable>(&self, target: H, options: Options<'_>, f: impl for<'a> Fn(Args<H::Fn>, Next<'a, H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        Self::subscribe::<H::Fn>(target.target(), options, |request, id, error| call!(self.api, hook_around(request, raw::around::<H::Fn, _>(f), id, error)))
    }

    /// Runs `f` in place of the function, with the original to delegate to; one per site.
    pub fn replace<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(Args<H::Fn>, Original<H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        Self::subscribe::<H::Fn>(target.target(), options, |request, id, error| call!(self.api, hook_replace(request, raw::replace::<H::Fn, _>(f), id, error)))
    }

    /// Runs `f` after the function with the arguments it received, its result and the outcome.
    pub fn after<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&Args<H::Fn>, &Ret<H::Fn>, Outcome) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        Self::subscribe::<H::Fn>(target.target(), options, |request, id, error| call!(self.api, hook_after(request, raw::after::<H::Fn, _>(f), id, error)))
    }

    /// Removes one subscription; true when it existed. Returns once no call is inside it.
    #[must_use]
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        call!(self.api, hook_unsubscribe(id.0)).unwrap_or(false)
    }

    /// Removes every subscription of `owner` and returns how many; once it returns, no call is
    /// inside any of them. The owner's patches are restored and removed with them.
    #[must_use]
    pub fn remove_owner(&self, owner: &str) -> usize {
        call!(self.api, hook_remove_owner(Str::from(owner))).unwrap_or(0)
    }

    /// Disables or re-enables one subscription; a disabled one keeps its place in the order and
    /// is skipped by the calls that start after this returns (an `Around` passes straight on, a
    /// `Replace` leaves the original to run). False when no such subscription exists.
    #[must_use]
    pub fn set_enabled(&self, id: SubscriptionId, enabled: bool) -> bool {
        call!(self.api, hook_set_enabled(id.0, enabled)).unwrap_or(false)
    }

    /// Every installed site with its subscribers in execution order.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<SiteReport> {
        let mut reports = Vec::new();
        let mut keep = |r: *const RawSiteReport| reports.extend(unsafe { abi::read_sized(r) }.map(|r| unsafe { r.to_report() }));
        call!(self.api, hook_diagnostics(Sink::new(&mut keep)));
        reports
    }

    /// The registry's last log lines, oldest first: installs, subscriptions, refusals, removals and toggles.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut keep = |line: Str| lines.push(unsafe { line.as_str() }.to_owned());
        call!(self.api, hook_messages(Sink::new(&mut keep)));
        lines
    }
}

#[cfg(test)]
mod tests;
