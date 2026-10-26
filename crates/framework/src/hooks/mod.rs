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
//! constraint cycle or adds a second `Replace` is refused and names the plugins involved. Before a site's detour goes in, its entry bytes are checked against the executable
//! file ([`Validation`]); an unmanaged patch refuses the detour.
//!
//! Subscriptions are added and removed from any thread while the detours run; a removal returns
//! only once no call can still be inside the removed subscriber. Registering or removing from
//! inside a subscriber is refused, since it would wait for itself.

use core::any::Any;
use std::fmt;

use crate::order::Constraints;

pub mod ops;
mod registry;
mod site;
mod validate;

pub use site::{AfterFn, Args, AroundFn, BeforeFn, Next, Op, Original, Outcome, ReplaceFn, Ret, Signature, Site, Timing};
pub use validate::{Bytes, Validation, validate};

/// When a subscriber runs relative to the engine function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Before,
    Around,
    Replace,
    After,
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

/// Why a registration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The constraints of the named plugins contradict each other on this site.
    Cycle { site: String, names: Vec<String> },
    /// `existing` already replaces the function; `refused` asked to as well.
    Exclusive { site: String, existing: String, refused: String },
    /// The entry bytes are not the function's own.
    Site { site: String, owner: String, validation: Validation },
    /// The address did not resolve or the detour could not be written.
    Install { site: String, owner: String, detail: String },
    /// A raw address with a shape or slot problem.
    Raw { address: usize, detail: String },
    /// The callback's type is not the site's (a mismatch between the plugin's and the host's framework builds).
    Callback { site: String, owner: String },
    /// Called from inside a subscriber.
    Reentrant,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle { site, names } => write!(f, "{site}: ordering cycle between {}", names.join(", ")),
            Self::Exclusive { site, existing, refused } => write!(f, "{site}: {refused} cannot replace it, {existing} already does"),
            Self::Site { site, owner, validation } => write!(f, "{site}: refused for {owner}, {validation}"),
            Self::Install { site, owner, detail } => write!(f, "{site}: refused for {owner}, {detail}"),
            Self::Raw { address, detail } => write!(f, "raw {address:#x}: {detail}"),
            Self::Callback { site, owner } => write!(f, "{site}: {owner}'s callback is not of the site's signature"),
            Self::Reentrant => f.write_str("registration from inside a hook subscriber"),
        }
    }
}

impl std::error::Error for Error {}

/// Where a subscription goes: a typed operation of [`ops`] or a raw address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Op(usize),
    Raw { address: usize, arity: usize },
}

/// Something a subscriber can attach to, with the signature its callbacks are typed by.
pub trait Hookable {
    type Fn: Signature;
    fn target(self) -> Target;
}

/// A registration as it crosses from a plugin to the host.
pub(crate) struct Request {
    pub target: Target,
    pub phase: Phase,
    pub owner: String,
    pub constraints: Constraints,
    pub callback: Box<dyn Any + Send + Sync>,
}

impl Request {
    fn new(target: Target, phase: Phase, options: Options<'_>, callback: Box<dyn Any + Send + Sync>) -> Self {
        let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect();
        let constraints = Constraints { before: names(options.before), after: names(options.after), priority: options.priority };
        Self { target, phase, owner: options.owner.to_owned(), constraints, callback }
    }
}

/// One site's state and subscribers, as [`Hooks::diagnostics`] reports it.
#[derive(Clone, Debug)]
pub struct SiteReport {
    pub name: String,
    pub address: usize,
    /// The entry as it was before the detour went in.
    pub validation: Validation,
    /// The entry now.
    pub current: Validation,
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
}

impl fmt::Display for SiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {:#x}: {} calls, last {} ns, mean {} ns, last outcome {:?}, entry {}", self.name, self.address, self.timing.calls, self.timing.last_ns, self.timing.mean_ns, self.last_outcome, self.current)?;
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
        write!(f, ": {} calls, last {} ns, mean {} ns", self.timing.calls, self.timing.last_ns, self.timing.mean_ns)
    }
}

/// The host's dispatcher as a plugin reaches it through `Host::hooks`.
pub struct Hooks {
    subscribe: fn(Request) -> Result<SubscriptionId, Error>,
    unsubscribe: fn(SubscriptionId) -> bool,
    remove_owner: fn(&str) -> usize,
    set_rank: fn(&str, i32),
    diagnostics: fn() -> Vec<SiteReport>,
}

impl Hooks {
    /// The host's own registry; only the host hands this out.
    pub(crate) const HOST: Self = Self { subscribe: registry::subscribe, unsubscribe: registry::unsubscribe, remove_owner: registry::remove_owner, set_rank: registry::set_rank, diagnostics: registry::diagnostics };

    pub(crate) fn set_logger(f: fn(&str)) {
        registry::set_logger(f);
    }

    /// Runs `f` before the function with the arguments, which it may edit.
    pub fn before<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&mut Args<H::Fn>) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<BeforeFn<H::Fn>> = Box::new(f);
        (self.subscribe)(Request::new(target.target(), Phase::Before, options, Box::new(f)))
    }

    /// Runs `f` around the function: it receives the arguments and the rest of the chain, which it
    /// may call once with any arguments or not at all, and returns the result.
    pub fn around<H: Hookable>(&self, target: H, options: Options<'_>, f: impl for<'a> Fn(Args<H::Fn>, Next<'a, H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<AroundFn<H::Fn>> = Box::new(f);
        (self.subscribe)(Request::new(target.target(), Phase::Around, options, Box::new(f)))
    }

    /// Runs `f` in place of the function, with the original to delegate to; one per site.
    pub fn replace<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(Args<H::Fn>, Original<H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<ReplaceFn<H::Fn>> = Box::new(f);
        (self.subscribe)(Request::new(target.target(), Phase::Replace, options, Box::new(f)))
    }

    /// Runs `f` after the function with the arguments it received, its result and the outcome.
    pub fn after<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&Args<H::Fn>, &Ret<H::Fn>, Outcome) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<AfterFn<H::Fn>> = Box::new(f);
        (self.subscribe)(Request::new(target.target(), Phase::After, options, Box::new(f)))
    }

    /// Removes one subscription; true when it existed. Returns once no call is inside it.
    #[must_use]
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        (self.unsubscribe)(id)
    }

    /// Removes every subscription of `owner` and returns how many; once it returns, no call is
    /// inside any of them.
    #[must_use]
    pub fn remove_owner(&self, owner: &str) -> usize {
        (self.remove_owner)(owner)
    }

    /// Sets a plugin's load rank, the tie-breaker after `priority`; the host sets it from the
    /// resolved load order before any plugin's `load` runs.
    pub fn set_rank(&self, owner: &str, rank: i32) {
        (self.set_rank)(owner, rank);
    }

    /// Every installed site with its subscribers in execution order.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<SiteReport> {
        (self.diagnostics)()
    }
}

#[cfg(test)]
mod tests;
