//! The dispatcher's crossings (`docs/subsystems/abi.md`, "Hooks"): arguments and results as slots,
//! the slot shape a subscription is checked by, targets, requests, patches, reports and errors as
//! `#[repr(C)]` records, and the plugin's side of each subscriber's `Callback`.

use core::ffi::c_void;
use core::marker::PhantomData;

use super::site::Subscriber;
use super::{Args, EntryReport, Error, Location, Outcome, PatchId, PatchReport, PatchRequest, PatchState, Phase, Register, Request, Ret, Signature, SiteReport, SubscriptionId, Target, Timing, Validation};
use crate::abi::{Callback, Crossing, Detail, RawError, Sink, Status, Str, slice};
use crate::order::Constraints;

mod sealed {
    pub trait Sealed {}
}

/// A type a hooked function takes or returns, carried in one 8-byte slot by bit copy: pointers,
/// integers, `bool`, `f32`, `f64`, and `()` for a function that returns nothing. No other type
/// implements it, so a signature with one that does not fit a slot does not compile.
pub trait Slot: sealed::Sealed + Copy + 'static {
    /// The value's class in a [`RawSignature`].
    const CLASS: u8;
    fn to_slot(self) -> u64;
    fn from_slot(bits: u64) -> Self;
}

macro_rules! integer_slots {
    ($($t:ty => $u:ty),*) => {$(
        impl sealed::Sealed for $t {}

        impl Slot for $t {
            const CLASS: u8 = RawSignature::INTEGER;

            #[allow(clippy::cast_sign_loss, clippy::cast_lossless)]
            fn to_slot(self) -> u64 {
                self as $u as u64
            }

            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            fn from_slot(bits: u64) -> Self {
                bits as $u as $t
            }
        }
    };
}

integer_slots!(u8 => u8, u16 => u16, u32 => u32, u64 => u64, usize => usize, i8 => u8, i16 => u16, i32 => u32, i64 => u64, isize => usize);

impl sealed::Sealed for bool {}
impl sealed::Sealed for f32 {}
impl sealed::Sealed for f64 {}
impl<T: 'static> sealed::Sealed for *const T {}
impl<T: 'static> sealed::Sealed for *mut T {}
impl sealed::Sealed for () {}

impl Slot for bool {
    const CLASS: u8 = RawSignature::INTEGER;

    fn to_slot(self) -> u64 {
        u64::from(self)
    }

    fn from_slot(bits: u64) -> Self {
        bits & 0xFF != 0
    }
}

impl Slot for f32 {
    const CLASS: u8 = RawSignature::F32;

    fn to_slot(self) -> u64 {
        u64::from(self.to_bits())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn from_slot(bits: u64) -> Self {
        Self::from_bits(bits as u32)
    }
}

impl Slot for f64 {
    const CLASS: u8 = RawSignature::F64;

    fn to_slot(self) -> u64 {
        self.to_bits()
    }

    fn from_slot(bits: u64) -> Self {
        Self::from_bits(bits)
    }
}

impl<T: 'static> Slot for *const T {
    const CLASS: u8 = RawSignature::INTEGER;

    fn to_slot(self) -> u64 {
        self as usize as u64
    }

    #[allow(clippy::cast_possible_truncation)]
    fn from_slot(bits: u64) -> Self {
        bits as usize as Self
    }
}

impl<T: 'static> Slot for *mut T {
    const CLASS: u8 = RawSignature::INTEGER;

    fn to_slot(self) -> u64 {
        self as usize as u64
    }

    #[allow(clippy::cast_possible_truncation)]
    fn from_slot(bits: u64) -> Self {
        bits as usize as Self
    }
}

impl Slot for () {
    const CLASS: u8 = RawSignature::NONE;

    fn to_slot(self) -> u64 {
        0
    }

    fn from_slot(_: u64) -> Self {}
}

/// The most arguments a hooked function can take.
pub const MAX_ARGS: usize = 9;

/// A hooked call's arguments, each in its own slot in order; the slots past the last are 0.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArgSlots {
    pub slots: [u64; MAX_ARGS],
}

impl ArgSlots {
    #[must_use]
    pub fn new(values: &[u64]) -> Self {
        let mut slots = [0; MAX_ARGS];
        slots[..values.len()].copy_from_slice(values);
        Self { slots }
    }
}

/// A hooked call's result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetSlot {
    pub bits: u64,
}

/// A function type as the slots see it: how many arguments, each one's class and the result's.
/// The host compares a subscription's with its own for the target, so a plugin built against
/// another `sre` still subscribes as long as the shape is the same.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawSignature {
    pub len: u8,
    pub ret: u8,
    /// The classes of the arguments; those past `len` are [`NONE`](Self::NONE).
    pub args: [u8; MAX_ARGS],
}

impl RawSignature {
    /// No value: a result of `()`.
    pub const NONE: u8 = 0;
    /// An integer, `bool` or pointer.
    pub const INTEGER: u8 = 1;
    pub const F32: u8 = 2;
    pub const F64: u8 = 3;

    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn new(args: &[u8], ret: u8) -> Self {
        let mut classes = [Self::NONE; MAX_ARGS];
        let mut i = 0;
        while i < args.len() {
            classes[i] = args[i];
            i += 1;
        }
        Self { len: args.len() as u8, ret, args: classes }
    }
}

/// The rest of an `Around` chain as the host hands it over: `call` runs it once with the
/// arguments and writes its result; a second call does nothing.
#[repr(C)]
pub struct NextApi {
    pub call: extern "C" fn(*const NextApi, *const ArgSlots, *mut RetSlot),
    pub state: *mut c_void,
}

pub type BeforeCallback = Callback<extern "C" fn(*mut c_void, *mut ArgSlots)>;
pub type AroundCallback = Callback<extern "C" fn(*mut c_void, *const ArgSlots, *const NextApi, *mut RetSlot)>;
/// Receives the original's address.
pub type ReplaceCallback = Callback<extern "C" fn(*mut c_void, *const ArgSlots, usize, *mut RetSlot)>;
/// Receives the [`Outcome`] as its number.
pub type AfterCallback = Callback<extern "C" fn(*mut c_void, *const ArgSlots, *const RetSlot, u8)>;

pub(crate) fn before<F: Signature, C: Fn(&mut Args<F>) + Send + Sync + 'static>(f: C) -> BeforeCallback {
    extern "C" fn call<F: Signature, C: Fn(&mut Args<F>)>(ctx: *mut c_void, slots: *mut ArgSlots) {
        let slots = unsafe { &mut *slots };
        let mut args = F::from_slots(slots);
        unsafe { (*ctx.cast::<C>())(&mut args) };
        *slots = F::to_slots(args);
    }
    BeforeCallback::boxed(f, call::<F, C>)
}

pub(crate) fn around<F: Signature, C: for<'a> Fn(Args<F>, Next<'a, F>) -> Ret<F> + Send + Sync + 'static>(f: C) -> AroundCallback {
    extern "C" fn call<F: Signature, C: for<'a> Fn(Args<F>, Next<'a, F>) -> Ret<F>>(ctx: *mut c_void, args: *const ArgSlots, next: *const NextApi, ret: *mut RetSlot) {
        let next = Next { api: unsafe { &*next }, signature: PhantomData };
        let value = unsafe { (*ctx.cast::<C>())(F::from_slots(&*args), next) };
        unsafe { ret.write(RetSlot { bits: value.to_slot() }) };
    }
    AroundCallback::boxed(f, call::<F, C>)
}

pub(crate) fn replace<F: Signature, C: Fn(Args<F>, Original<F>) -> Ret<F> + Send + Sync + 'static>(f: C) -> ReplaceCallback {
    extern "C" fn call<F: Signature, C: Fn(Args<F>, Original<F>) -> Ret<F>>(ctx: *mut c_void, args: *const ArgSlots, original: usize, ret: *mut RetSlot) {
        let original = Original(unsafe { core::mem::transmute_copy::<usize, F>(&original) });
        let value = unsafe { (*ctx.cast::<C>())(F::from_slots(&*args), original) };
        unsafe { ret.write(RetSlot { bits: value.to_slot() }) };
    }
    ReplaceCallback::boxed(f, call::<F, C>)
}

pub(crate) fn after<F: Signature, C: Fn(&Args<F>, &Ret<F>, Outcome) + Send + Sync + 'static>(f: C) -> AfterCallback {
    extern "C" fn call<F: Signature, C: Fn(&Args<F>, &Ret<F>, Outcome)>(ctx: *mut c_void, args: *const ArgSlots, ret: *const RetSlot, outcome: u8) {
        let (args, ret) = unsafe { (F::from_slots(&*args), Ret::<F>::from_slot((*ret).bits)) };
        // An outcome a later host adds reads as the original having run.
        unsafe { (*ctx.cast::<C>())(&args, &ret, Outcome::from_u8(outcome).unwrap_or(Outcome::Original)) };
    }
    AfterCallback::boxed(f, call::<F, C>)
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

/// The rest of the `Around` chain, ending at the engine function or the `Replace`; calling it is
/// optional. The host runs it.
pub struct Next<'a, F: Signature> {
    api: &'a NextApi,
    signature: PhantomData<F>,
}

impl<F: Signature> Next<'_, F> {
    /// Runs the next `Around`, or the innermost link, with `args`.
    ///
    /// # Safety
    /// `args` must be valid for the engine function.
    pub unsafe fn call(self, args: Args<F>) -> Ret<F> {
        let mut ret = RetSlot::default();
        (self.api.call)(self.api, &F::to_slots(args), &raw mut ret);
        Ret::<F>::from_slot(ret.bits)
    }
}

/// A [`Location`]: `kind` 0 is an Address Library id plus an offset, 1 an address.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawLocation {
    pub kind: u32,
    pub id: u64,
    pub offset: usize,
    pub address: usize,
}

impl RawLocation {
    const ID: u32 = 0;
    const ADDRESS: u32 = 1;

    pub(crate) fn new(at: Location) -> Self {
        match at {
            Location::Id { id, offset } => Self { kind: Self::ID, id, offset, address: 0 },
            Location::Address(address) => Self { kind: Self::ADDRESS, id: 0, offset: 0, address },
        }
    }

    /// `None` for a kind this side does not know.
    pub(crate) fn to_location(self) -> Option<Location> {
        match self.kind {
            Self::ID => Some(Location::Id { id: self.id, offset: self.offset }),
            Self::ADDRESS => Some(Location::Address(self.address)),
            _ => None,
        }
    }
}

/// A [`Target`]: `kind` 0 a typed operation (`op`, its index in `ops!`), 1 a raw entry at
/// `location`, 2 a call site at `location` handing on `register` (0 none, else its x86 number),
/// 3 slot `slot` of the vtable at `location`; `shape` indexes `ops::SHAPES`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawTarget {
    pub kind: u32,
    pub op: u32,
    pub location: RawLocation,
    pub slot: usize,
    pub register: u32,
    pub shape: u32,
}

impl RawTarget {
    pub(crate) fn new(target: Target) -> Self {
        let index = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let none = Self { kind: 0, op: 0, location: RawLocation::new(Location::Address(0)), slot: 0, register: 0, shape: 0 };
        match target {
            Target::Op(op) => Self { op: index(op), ..none },
            Target::Raw { address, shape } => Self { kind: 1, location: RawLocation::new(Location::Address(address)), shape: index(shape), ..none },
            Target::Call { at, shape, register } => Self { kind: 2, location: RawLocation::new(at), register: register.map_or(0, |r| u32::from(r.number())), shape: index(shape), ..none },
            Target::Vtable { table, slot, shape } => Self { kind: 3, location: RawLocation::new(table), slot, shape: index(shape), ..none },
        }
    }

    /// `None` for a kind, location or register this side does not know.
    pub(crate) fn to_target(self) -> Option<Target> {
        let (shape, location) = (self.shape as usize, self.location.to_location());
        Some(match self.kind {
            0 => Target::Op(self.op as usize),
            1 => Target::Raw { address: match location? {
                Location::Address(address) => address,
                Location::Id { .. } => return None,
            }, shape },
            2 => Target::Call { at: location?, shape, register: if self.register == 0 { None } else { Some(Register::from_number(self.register)?) } },
            3 => Target::Vtable { table: location?, slot: self.slot, shape },
            _ => return None,
        })
    }
}

/// A subscription: the target, the `Options` and the slot shape of the subscriber's function type.
#[repr(C)]
pub struct RawSubscription {
    pub target: RawTarget,
    pub owner: Str,
    pub priority: i32,
    pub before: *const Str,
    pub before_len: usize,
    pub after: *const Str,
    pub after_len: usize,
    pub signature: RawSignature,
}

unsafe fn strings(ptr: *const Str, len: usize) -> Vec<String> {
    unsafe { slice(ptr, len) }.iter().map(|s| unsafe { s.as_str() }.to_owned()).collect()
}

fn strs(list: &[String]) -> Vec<Str> {
    list.iter().map(|s| Str::from(s.as_str())).collect()
}

impl RawSubscription {
    /// The request with `subscriber`, the owner's name, constraints and target copied.
    pub(crate) unsafe fn to_request(&self, subscriber: Subscriber) -> Result<Request, Error> {
        let target = self.target.to_target().ok_or_else(|| Error::Unsupported { what: format!("hook target kind {}", self.target.kind) })?;
        let constraints = Constraints { before: unsafe { strings(self.before, self.before_len) }, after: unsafe { strings(self.after, self.after_len) }, priority: self.priority };
        Ok(Request { target, owner: unsafe { self.owner.as_str() }.to_owned(), constraints, signature: self.signature, subscriber })
    }
}

/// A [`Validation`]: `kind` 0 pristine, 1 plausible, 2 managed (`address` the detour), 3 unmanaged
/// (`address` the jump's target when `has_address`, `expected` the file's bytes when `has_expected`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RawValidation {
    pub kind: u32,
    pub address: usize,
    pub has_address: bool,
    pub found: [u8; 16],
    pub expected: [u8; 16],
    pub has_expected: bool,
}

impl From<Validation> for RawValidation {
    fn from(v: Validation) -> Self {
        match v {
            Validation::Pristine => Self::default(),
            Validation::Plausible => Self { kind: 1, ..Self::default() },
            Validation::Managed { detour } => Self { kind: 2, address: detour, has_address: true, ..Self::default() },
            Validation::Unmanaged { found, expected, target } => {
                Self { kind: 3, address: target.unwrap_or_default(), has_address: target.is_some(), found, expected: expected.unwrap_or_default(), has_expected: expected.is_some() }
            }
        }
    }
}

impl From<RawValidation> for Validation {
    /// A kind this side does not know reads as an unmanaged patch.
    fn from(v: RawValidation) -> Self {
        match v.kind {
            0 => Self::Pristine,
            1 => Self::Plausible,
            2 => Self::Managed { detour: v.address },
            _ => Self::Unmanaged { found: v.found, expected: v.has_expected.then_some(v.expected), target: v.has_address.then_some(v.address) },
        }
    }
}

/// An [`EntryReport`]; the constraint names are borrowed for the call.
#[repr(C)]
pub struct RawEntryReport {
    pub id: u64,
    pub owner: Str,
    /// 0 before, 1 around, 2 replace, 3 after.
    pub phase: u32,
    pub priority: i32,
    pub before: *const Str,
    pub before_len: usize,
    pub after: *const Str,
    pub after_len: usize,
    pub rank: i32,
    pub timing: Timing,
    pub enabled: bool,
}

/// A [`SiteReport`]; `patched_by` is null for none, and the entries are borrowed for the call.
#[repr(C)]
pub struct RawSiteReport {
    pub name: Str,
    pub address: usize,
    pub validation: RawValidation,
    pub current: RawValidation,
    pub patched_by: Str,
    pub broken: bool,
    pub timing: Timing,
    /// The [`Outcome`]'s number, 0 for none yet.
    pub last_outcome: u8,
    pub entries: *const RawEntryReport,
    pub len: usize,
}

/// Puts `report` through `sink`.
pub(crate) fn put_site(report: &SiteReport, sink: &Sink<'_, RawSiteReport>) {
    let names: Vec<(Vec<Str>, Vec<Str>)> = report.entries.iter().map(|e| (strs(&e.constraints.before), strs(&e.constraints.after))).collect();
    let entries: Vec<RawEntryReport> = report
        .entries
        .iter()
        .zip(&names)
        .map(|(e, (before, after))| RawEntryReport {
            id: e.id.get(),
            owner: Str::from(e.owner.as_str()),
            phase: e.phase as u32,
            priority: e.constraints.priority,
            before: before.as_ptr(),
            before_len: before.len(),
            after: after.as_ptr(),
            after_len: after.len(),
            rank: e.rank,
            timing: e.timing,
            enabled: e.enabled,
        })
        .collect();
    sink.put(RawSiteReport {
        name: Str::from(report.name.as_str()),
        address: report.address,
        validation: report.validation.into(),
        current: report.current.into(),
        patched_by: Str::maybe(report.patched_by.as_deref().map(str::as_bytes)),
        broken: report.broken,
        timing: report.timing,
        last_outcome: report.last_outcome.map_or(0, |o| o as u8),
        entries: entries.as_ptr(),
        len: entries.len(),
    });
}

impl RawSiteReport {
    /// The report, copied; an entry of a phase this side does not know is left out.
    pub(crate) unsafe fn to_report(&self) -> SiteReport {
        let entries = unsafe { slice(self.entries, self.len) }
            .iter()
            .filter_map(|e| {
                Some(EntryReport {
                    id: SubscriptionId::new(e.id),
                    owner: unsafe { e.owner.as_str() }.to_owned(),
                    phase: Phase::from_u32(e.phase)?,
                    constraints: Constraints { before: unsafe { strings(e.before, e.before_len) }, after: unsafe { strings(e.after, e.after_len) }, priority: e.priority },
                    rank: e.rank,
                    timing: e.timing,
                    enabled: e.enabled,
                })
            })
            .collect();
        SiteReport {
            name: unsafe { self.name.as_str() }.to_owned(),
            address: self.address,
            validation: self.validation.into(),
            current: self.current.into(),
            patched_by: unsafe { self.patched_by.maybe_bytes() }.map(|b| String::from_utf8_lossy(b).into_owned()),
            broken: self.broken,
            timing: self.timing,
            last_outcome: Outcome::from_u8(self.last_outcome),
            entries,
        }
    }
}

/// A byte patch: the bytes expected at `at` and their replacement, or with `cave` (null for none)
/// the code a jump in their place leads to.
#[repr(C)]
pub struct RawPatch {
    pub owner: Str,
    pub at: RawLocation,
    pub expected: Str,
    pub replacement: Str,
    pub cave: Str,
}

impl RawPatch {
    /// The request with its owner and bytes copied.
    pub(crate) unsafe fn to_request(&self) -> Result<PatchRequest, Error> {
        let at = self.at.to_location().ok_or_else(|| Error::Unsupported { what: format!("patch location kind {}", self.at.kind) })?;
        let bytes = |s: Str| unsafe { s.bytes() }.to_vec();
        Ok(PatchRequest { owner: unsafe { self.owner.as_str() }.to_owned(), at, expected: bytes(self.expected), replacement: bytes(self.replacement), cave: unsafe { self.cave.maybe_bytes() }.map(<[u8]>::to_vec) })
    }
}

/// A [`PatchReport`]: `state` 0 applied, 1 disabled, 2 overwritten (with the bytes `found`).
#[repr(C)]
pub struct RawPatchReport {
    pub id: u64,
    pub owner: Str,
    pub site: RawLocation,
    pub address: usize,
    pub original: Str,
    pub replacement: Str,
    pub cave: Str,
    pub state: u32,
    pub found: Str,
}

impl RawPatchReport {
    pub(crate) fn new(p: &PatchReport) -> Self {
        let (state, found): (u32, &[u8]) = match &p.state {
            PatchState::Applied => (0, &[]),
            PatchState::Disabled => (1, &[]),
            PatchState::Overwritten { found } => (2, found),
        };
        Self {
            id: p.id.get(),
            owner: Str::from(p.owner.as_str()),
            site: RawLocation::new(p.site),
            address: p.address,
            original: Str::new(&p.original),
            replacement: Str::new(&p.replacement),
            cave: Str::maybe(p.cave.as_deref()),
            state,
            found: Str::new(found),
        }
    }

    /// The report, copied; a state this side does not know reads as overwritten.
    pub(crate) unsafe fn to_report(&self) -> PatchReport {
        let bytes = |s: Str| unsafe { s.bytes() }.to_vec();
        PatchReport {
            id: PatchId::new(self.id),
            owner: unsafe { self.owner.as_str() }.to_owned(),
            site: self.site.to_location().unwrap_or(Location::Address(self.address)),
            address: self.address,
            original: bytes(self.original),
            replacement: bytes(self.replacement),
            cave: unsafe { self.cave.maybe_bytes() }.map(<[u8]>::to_vec),
            state: match self.state {
                0 => PatchState::Applied,
                1 => PatchState::Disabled,
                _ => PatchState::Overwritten { found: bytes(self.found) },
            },
        }
    }
}

/// A [`Error`] crosses as [`Status::Hook`] with its variant in `code`, in declaration order from
/// `Cycle` 0 to `NoPatch` 10: the site in `what`, the owner in `owner`, the third text (the
/// refused owner, the install or raw detail, the holder of the bytes) in `other`, a raw address
/// or patch id in `limit`, the cycle's names in `names`, a mismatch's bytes in `expected` and
/// `found`, a site's validation in `validation`. `Owner` puts the given name in `what` and the
/// plugin's in `owner`; `Unsupported` crosses as [`Status::Unsupported`].
impl Crossing for Error {
    fn fail(&self, detail: &Sink<'_, RawError>) -> u32 {
        let text = self.to_string();
        let names: Vec<Str> = match self {
            Self::Cycle { names, .. } => strs(names),
            _ => Vec::new(),
        };
        let at = |code: u32, what: &str, owner: &str| RawError { code, what: Str::from(what), owner: Str::from(owner), ..RawError::text(&text) };
        let raw = match self {
            Self::Unsupported { what } => {
                detail.put(RawError { what: Str::from(what.as_str()), ..RawError::text(&text) });
                return Status::Unsupported as u32;
            }
            Self::Cycle { site, .. } => RawError { names: names.as_ptr(), names_len: names.len(), ..at(0, site, "") },
            Self::Exclusive { site, existing, refused } => RawError { other: Str::from(refused.as_str()), ..at(1, site, existing) },
            Self::Site { site, owner, validation } => RawError { validation: (*validation).into(), ..at(2, site, owner) },
            Self::Install { site, owner, detail } => RawError { other: Str::from(detail.as_str()), ..at(3, site, owner) },
            Self::Raw { address, detail } => RawError { limit: *address, other: Str::from(detail.as_str()), ..at(4, "", "") },
            Self::Callback { site, owner } => at(5, site, owner),
            Self::Reentrant => at(6, "", ""),
            Self::Owner { given, plugin } => at(7, given, plugin),
            Self::Overlap { site, owner, with } => RawError { other: Str::from(with.as_str()), ..at(8, site, owner) },
            Self::Mismatch { site, owner, expected, found } => RawError { expected: Str::new(expected), found: Str::new(found), ..at(9, site, owner) },
            Self::NoPatch(id) => RawError { limit: usize::try_from(id.get()).unwrap_or(usize::MAX), ..at(10, "", "") },
        };
        detail.put(raw);
        Status::Hook as u32
    }

    fn read(status: u32, d: Detail) -> Self {
        if status == Status::Unsupported as u32 {
            return Self::Unsupported { what: d.what };
        }
        if status != Status::Hook as u32 {
            return Self::Unsupported { what: d.text };
        }
        match d.code {
            0 => Self::Cycle { site: d.what, names: d.names },
            1 => Self::Exclusive { site: d.what, existing: d.owner, refused: d.other },
            2 => Self::Site { site: d.what, owner: d.owner, validation: d.validation.into() },
            3 => Self::Install { site: d.what, owner: d.owner, detail: d.other },
            4 => Self::Raw { address: d.limit, detail: d.other },
            5 => Self::Callback { site: d.what, owner: d.owner },
            6 => Self::Reentrant,
            7 => Self::Owner { given: d.what, plugin: d.owner },
            8 => Self::Overlap { site: d.what, owner: d.owner, with: d.other },
            9 => Self::Mismatch { site: d.what, owner: d.owner, expected: d.expected, found: d.found },
            10 => Self::NoPatch(PatchId::new(d.limit as u64)),
            _ => Self::Unsupported { what: d.text },
        }
    }

    fn unsupported(what: String) -> Self {
        Self::Unsupported { what }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::outcome;

    fn round_trip<T: Slot + PartialEq + core::fmt::Debug>(values: &[T]) {
        for &v in values {
            assert_eq!(T::from_slot(v.to_slot()), v);
        }
    }

    #[test]
    fn every_slot_type_round_trips() {
        round_trip(&[0u8, 1, u8::MAX]);
        round_trip(&[0u16, 7, u16::MAX]);
        round_trip(&[0u32, 24, u32::MAX]);
        round_trip(&[0u64, u64::MAX]);
        round_trip(&[0usize, usize::MAX]);
        round_trip(&[i8::MIN, -1, 0, i8::MAX]);
        round_trip(&[i16::MIN, -1, i16::MAX]);
        round_trip(&[i32::MIN, -2, 0, i32::MAX]);
        round_trip(&[i64::MIN, -1, i64::MAX]);
        round_trip(&[isize::MIN, -1, isize::MAX]);
        round_trip(&[false, true]);
        round_trip(&[0.0f32, -11.5, f32::MAX, f32::INFINITY]);
        round_trip(&[0.0f64, -1e300, f64::MIN_POSITIVE]);
        round_trip(&[core::ptr::null::<u8>(), 0x7ff7_aae8_5220 as *const u8]);
        round_trip(&[core::ptr::null_mut::<sre::Actor>(), 0x1000 as *mut sre::Actor]);
        round_trip(&[()]);
        assert_eq!(f32::from_slot((-0.0f32).to_slot()).to_bits(), (-0.0f32).to_bits());
        assert!(f64::from_slot(f64::NAN.to_slot()).is_nan());
        assert_eq!((-1i32).to_slot(), 0xFFFF_FFFF, "a bit copy into the low bytes, the rest zero");
        assert_eq!(1.5f32.to_slot(), u64::from(1.5f32.to_bits()));
        assert!(bool::from_slot(0x100 | 1) && !bool::from_slot(0x100), "only the low byte is the bool");
    }

    type ModActorValue = sre::raw::fns::Actor__ModActorValueImpl_fn;
    type Projection = sre::raw::fns::NiCamera__SetPerspectiveFromFov_fn;

    #[test]
    fn arguments_cross_one_per_slot() {
        let actor = 0x40 as *mut sre::Actor;
        let args = (actor, 2, 24, -10.5, core::ptr::null_mut());
        let slots = ModActorValue::to_slots(args);
        assert_eq!(slots.slots, [0x40, 2, 24, u64::from((-10.5f32).to_bits()), 0, 0, 0, 0, 0]);
        assert_eq!(ModActorValue::from_slots(&slots), args);
        let nine = (core::ptr::null_mut(), 80.0, 5.0, 10100.0, 1920, 1080, false, true, 75.0);
        assert_eq!(Projection::from_slots(&Projection::to_slots(nine)), nine);
    }

    #[test]
    fn signatures_compare_by_slot_shape() {
        use RawSignature as S;
        assert_eq!(ModActorValue::SLOTS, S::new(&[S::INTEGER, S::INTEGER, S::INTEGER, S::F32, S::INTEGER], S::NONE));
        assert_eq!(Projection::SLOTS.len, 9);
        assert_eq!(<unsafe extern "C" fn(*mut u8, u64) -> isize as Signature>::SLOTS, <unsafe extern "C" fn(usize, usize) -> usize as Signature>::SLOTS, "pointers, integers and bool share a class");
        assert_eq!(<unsafe extern "C" fn(bool) -> u16 as Signature>::SLOTS, <unsafe extern "C" fn(usize) -> usize as Signature>::SLOTS);
        assert_ne!(<unsafe extern "C" fn(f32, usize) -> usize as Signature>::SLOTS, <unsafe extern "C" fn(usize, usize) -> usize as Signature>::SLOTS);
        assert_ne!(<unsafe extern "C" fn(f64) as Signature>::SLOTS, <unsafe extern "C" fn(f32) as Signature>::SLOTS);
        assert_ne!(<unsafe extern "C" fn(usize) -> f32 as Signature>::SLOTS, <unsafe extern "C" fn(usize) -> usize as Signature>::SLOTS);
        assert_ne!(<unsafe extern "C" fn(usize) as Signature>::SLOTS, <unsafe extern "C" fn(usize) -> usize as Signature>::SLOTS);
        assert_ne!(<unsafe extern "C" fn(usize) -> usize as Signature>::SLOTS, <unsafe extern "C" fn(usize, usize) -> usize as Signature>::SLOTS);
    }

    #[test]
    fn targets_and_locations_cross_and_come_back() {
        for target in [
            Target::Op(3),
            Target::Raw { address: 0x1234, shape: 2 },
            Target::Call { at: Location::id(52_000, 0x1f5), shape: 9, register: Some(Register::R14) },
            Target::Call { at: Location::Address(0x10), shape: 1, register: None },
            Target::Vtable { table: Location::id(208_040, 0), slot: 113, shape: 1 },
        ] {
            assert_eq!(RawTarget::new(target).to_target(), Some(target));
        }
        assert!(RawTarget { kind: 9, ..RawTarget::new(Target::Op(0)) }.to_target().is_none(), "a kind from a later side");
        assert!(RawTarget { register: 4, ..RawTarget::new(Target::Call { at: Location::Address(0), shape: 1, register: None }) }.to_target().is_none(), "rsp is no register a call hands on");
        assert!(RawLocation { kind: 2, ..RawLocation::new(Location::Address(1)) }.to_location().is_none());
    }

    #[test]
    fn validations_cross_and_come_back() {
        let found = [0xE9; 16];
        for v in [
            Validation::Pristine,
            Validation::Plausible,
            Validation::Managed { detour: 0x7fff_0000 },
            Validation::Unmanaged { found, expected: Some([0x48; 16]), target: Some(0x7ff9_264d_c860) },
            Validation::Unmanaged { found, expected: None, target: None },
        ] {
            assert_eq!(Validation::from(RawValidation::from(v)), v);
        }
    }

    fn cross(e: &Error) -> Error {
        outcome::<Error>("x", |sink| Some(e.fail(&sink))).unwrap_err()
    }

    #[test]
    fn every_hook_error_comes_back_typed() {
        let validation = Validation::Unmanaged { found: [0xE9; 16], expected: Some([0x48; 16]), target: Some(0x1000) };
        for e in [
            Error::Cycle { site: "s".into(), names: vec!["a".into(), "b, c".into()] },
            Error::Exclusive { site: "s".into(), existing: "a".into(), refused: "b".into() },
            Error::Site { site: "s".into(), owner: "o".into(), validation },
            Error::Install { site: "s".into(), owner: "o".into(), detail: "d".into() },
            Error::Raw { address: 0x7ff7_0000, detail: "already hooked as 2 +rbx".into() },
            Error::Callback { site: "s".into(), owner: "o".into() },
            Error::Reentrant,
            Error::Owner { given: "r".into(), plugin: "q".into() },
            Error::Overlap { site: "s".into(), owner: "o".into(), with: "patch #3 of p".into() },
            Error::Mismatch { site: "s".into(), owner: "o".into(), expected: vec![5], found: vec![4, 0] },
            Error::NoPatch(PatchId::new(17)),
            Error::Unsupported { what: "hook operation 40".into() },
        ] {
            assert_eq!(cross(&e), e);
        }
        let later = |status: Status, code: u32| move |sink: Sink<'_, RawError>| {
            sink.put(RawError { code, ..RawError::text("a later error") });
            Some(status as u32)
        };
        assert_eq!(outcome::<Error>("x", later(Status::Hook, 11)), Err(Error::Unsupported { what: "a later error".into() }));
        assert_eq!(outcome::<Error>("x", later(Status::Taken, 0)), Err(Error::Unsupported { what: "a later error".into() }));
        assert_eq!(outcome::<Error>("hook subscriptions", |_| None), Err(Error::Unsupported { what: "hook subscriptions".into() }));
    }
}
