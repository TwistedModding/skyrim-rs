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
pub use validate::{Bytes, Validation, owner, validate};

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
    /// The callback's type is not the site's (a mismatch between the plugin's and the host's framework builds).
    Callback { site: String, owner: String },
    /// Called from inside a subscriber.
    Reentrant,
    /// The `Options` owner is not the name of the plugin the handle belongs to.
    Owner { given: String, plugin: String },
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

/// The host's dispatcher as a plugin reaches it through `Host::hooks`: the handle of one plugin
/// accepts subscriptions whose `Options` owner is that plugin's name, so a reload removes exactly them.
pub struct Hooks {
    owner: Option<&'static str>,
    subscribe: fn(Request) -> Result<SubscriptionId, Error>,
    unsubscribe: fn(SubscriptionId) -> bool,
    remove_owner: fn(&str) -> usize,
    set_rank: fn(&str, i32),
    set_enabled: fn(SubscriptionId, bool) -> bool,
    diagnostics: fn() -> Vec<SiteReport>,
    messages: fn() -> Vec<String>,
}

impl Hooks {
    /// The host's own registry; only the host hands this out.
    pub(crate) const HOST: Self = Self {
        owner: None,
        subscribe: registry::subscribe,
        unsubscribe: registry::unsubscribe,
        remove_owner: registry::remove_owner,
        set_rank: registry::set_rank,
        set_enabled: registry::set_enabled,
        diagnostics: registry::diagnostics,
        messages: registry::messages,
    };

    /// The registry as the plugin `name` sees it.
    pub(crate) const fn for_plugin(name: &'static str) -> Self {
        Self { owner: Some(name), ..Self::HOST }
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

    fn request(&self, target: Target, phase: Phase, options: Options<'_>, callback: Box<dyn Any + Send + Sync>) -> Result<SubscriptionId, Error> {
        if let Some(plugin) = self.owner
            && plugin != options.owner
        {
            return Err(Error::Owner { given: options.owner.to_owned(), plugin: plugin.to_owned() });
        }
        (self.subscribe)(Request::new(target, phase, options, callback))
    }

    /// Runs `f` before the function with the arguments, which it may edit.
    pub fn before<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&mut Args<H::Fn>) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<BeforeFn<H::Fn>> = Box::new(f);
        self.request(target.target(), Phase::Before, options, Box::new(f))
    }

    /// Runs `f` around the function: it receives the arguments and the rest of the chain, which it
    /// may call once with any arguments or not at all, and returns the result.
    pub fn around<H: Hookable>(&self, target: H, options: Options<'_>, f: impl for<'a> Fn(Args<H::Fn>, Next<'a, H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<AroundFn<H::Fn>> = Box::new(f);
        self.request(target.target(), Phase::Around, options, Box::new(f))
    }

    /// Runs `f` in place of the function, with the original to delegate to; one per site.
    pub fn replace<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(Args<H::Fn>, Original<H::Fn>) -> Ret<H::Fn> + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<ReplaceFn<H::Fn>> = Box::new(f);
        self.request(target.target(), Phase::Replace, options, Box::new(f))
    }

    /// Runs `f` after the function with the arguments it received, its result and the outcome.
    pub fn after<H: Hookable>(&self, target: H, options: Options<'_>, f: impl Fn(&Args<H::Fn>, &Ret<H::Fn>, Outcome) + Send + Sync + 'static) -> Result<SubscriptionId, Error> {
        let f: Box<AfterFn<H::Fn>> = Box::new(f);
        self.request(target.target(), Phase::After, options, Box::new(f))
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

    /// Disables or re-enables one subscription; a disabled one keeps its place in the order and
    /// is skipped by the calls that start after this returns (an `Around` passes straight on, a
    /// `Replace` leaves the original to run). False when no such subscription exists.
    #[must_use]
    pub fn set_enabled(&self, id: SubscriptionId, enabled: bool) -> bool {
        (self.set_enabled)(id, enabled)
    }

    /// Every installed site with its subscribers in execution order.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<SiteReport> {
        (self.diagnostics)()
    }

    /// The registry's last log lines, oldest first: installs, subscriptions, refusals, removals and toggles.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        (self.messages)()
    }
}

#[cfg(test)]
mod tests;
