//! The typed engine operations a plugin can subscribe to, and the raw slots for addresses outside the set.
//!
//! Each marker type below is one engine function: its Address Library id and `extern "C" fn` type
//! come from `skre`, so a subscriber's closure is checked against the function's real
//! signature. Adding one is one line in `ops!`.

use core::marker::PhantomData;

use skre::raw::fns;

use super::site::{Op, Signature, Site};
use super::{Hookable, Target};

/// Visits the `Op` type behind an index or a raw slot.
pub(crate) trait OpVisitor<R> {
    fn visit<O: Op>(self) -> R;
}

macro_rules! ops {
    ($($id:ident / $fn:ident = $name:literal,) => {
        #[allow(non_camel_case_types, clippy::enum_variant_names)]
        #[derive(Clone, Copy)]
        enum Index { $($id),* }

        $(
            #[doc = concat!("`", $name, "` (`fns::", stringify!($id), "`).")]
            #[allow(non_camel_case_types)]
            #[derive(Clone, Copy, Debug)]
            pub struct $id;

            impl Op for $id {
                type Fn = fns::$fn;

                fn site() -> &'static Site {
                    static SITE: Site = Site::new();
                    &SITE
                }
            }

            impl Hookable for $id {
                type Fn = fns::$fn;

                fn target(self) -> Target {
                    Target::Op(Index::$id as usize)
                }
            }
        

        /// Display name and Address Library id of every operation, by index.
        pub(crate) const OPS: &[(&str, u64)] = &[$(($name, fns::$id)),*];

        pub(crate) fn with_op<R>(index: usize, visitor: impl OpVisitor<R>) -> Option<R> {
            $(
                if index == Index::$id as usize {
                    return Some(visitor.visit::<$id>());
                }
            
            None
        }
    };
}

ops! {
    Actor__ModActorValueImpl / Actor__ModActorValueImpl_fn = "Actor::ModActorValueImpl",
    Actor__ActorValueOwner_ModActorValue / Actor__ActorValueOwner_ModActorValue_fn = "Actor::ActorValueOwner_ModActorValue",
    Actor__ActorValueOwner_AddBaseActorValue / Actor__ActorValueOwner_AddBaseActorValue_fn = "Actor::ActorValueOwner_AddBaseActorValue",
    Actor__ActorValueOwner_StoreBaseActorValue / Actor__ActorValueOwner_StoreBaseActorValue_fn = "Actor::ActorValueOwner_StoreBaseActorValue",
    Actor__ActorValueOwner_GetActorValue / Actor__ActorValueOwner_GetActorValue_fn = "Actor::ActorValueOwner_GetActorValue",
    Actor__ActorValueOwner_GetBaseActorValue / Actor__ActorValueOwner_GetBaseActorValue_fn = "Actor::ActorValueOwner_GetBaseActorValue",
    Actor__ActorValueOwner_GetPermanentActorValue / Actor__ActorValueOwner_GetPermanentActorValue_fn = "Actor::ActorValueOwner_GetPermanentActorValue",
    Actor__GetActorValueModifier / Actor__GetActorValueModifier_fn = "Actor::GetActorValueModifier",
    Actor__GetLevel / Actor__GetLevel_fn = "Actor::GetLevel",
    Actor__GetEquippedObject / Actor__GetEquippedObject_fn = "Actor::GetEquippedObject",
    Actor__IsDead / Actor__IsDead_fn = "Actor::IsDead",
    Actor__IsInCombat / Actor__IsInCombat_fn = "Actor::IsInCombat",
    TESObjectREFR__GetScale / TESObjectREFR__GetScale_fn = "TESObjectREFR::GetScale",
    TESObjectREFR__GetCurrentLocation / TESObjectREFR__GetCurrentLocation_fn = "TESObjectREFR::GetCurrentLocation",
    TESObjectREFR__GetWorldspace / TESObjectREFR__GetWorldspace_fn = "TESObjectREFR::GetWorldspace",
    TESObjectREFR__GetContainer / TESObjectREFR__GetContainer_fn = "TESObjectREFR::GetContainer",
    TESObjectREFR__GetInventoryChanges / TESObjectREFR__GetInventoryChanges_fn = "TESObjectREFR::GetInventoryChanges",
    TESObjectREFR__GetHandle / TESObjectREFR__GetHandle_fn = "TESObjectREFR::GetHandle",
}

/// How many raw addresses a process can hook at once.
pub const RAW_SLOTS: usize = 16;

/// The most integer-class arguments a raw hook can carry.
pub const RAW_MAX_ARITY: usize = 8;

static RAW_SITES: [Site; RAW_SLOTS] = [const { Site::new() }; RAW_SLOTS];

/// A function `skre` does not type, hooked by address with a shape the caller vouches for.
/// Only integer-class arguments and results are carried (`usize` each, in register order); a
/// float or a struct passed by value is not preserved. A second plugin may subscribe to the same
/// address with the same shape; a different shape is refused.
pub struct Raw<F: RawShape>(usize, PhantomData<F>);

impl<F: RawShape> Raw<F> {
    #[must_use]
    pub const fn at(address: usize) -> Self {
        Self(address, PhantomData)
    }
}

impl<F: RawShape> Hookable for Raw<F> {
    type Fn = F;

    fn target(self) -> Target {
        Target::Raw { address: self.0, arity: F::ARITY }
    }
}

/// `unsafe extern "C" fn(usize, ...) -> usize` with up to [`RAW_MAX_ARITY`] arguments.
pub trait RawShape: Signature {
    const ARITY: usize;
}

pub(crate) struct RawSlot<const N: usize, const K: usize>;

macro_rules! raw_shapes {
    ($($n:literal => ($($A:ty),*);) => {$(
        impl RawShape for unsafe extern "C" fn($($A),*) -> usize {
            const ARITY: usize = $n;
        }

        impl<const K: usize> Op for RawSlot<$n, K> {
            type Fn = unsafe extern "C" fn($($A),*) -> usize;

            fn site() -> &'static Site {
                &RAW_SITES[K]
            }
        }
    };
}

raw_shapes! {
    0 => ();
    1 => (usize);
    2 => (usize, usize);
    3 => (usize, usize, usize);
    4 => (usize, usize, usize, usize);
    5 => (usize, usize, usize, usize, usize);
    6 => (usize, usize, usize, usize, usize, usize);
    7 => (usize, usize, usize, usize, usize, usize, usize);
    8 => (usize, usize, usize, usize, usize, usize, usize, usize);
}

macro_rules! raw_slots {
    ($n:literal, $slot:expr, $visitor:expr) => {
        match $slot {
            0 => Some($visitor.visit::<RawSlot<$n, 0>>()),
            1 => Some($visitor.visit::<RawSlot<$n, 1>>()),
            2 => Some($visitor.visit::<RawSlot<$n, 2>>()),
            3 => Some($visitor.visit::<RawSlot<$n, 3>>()),
            4 => Some($visitor.visit::<RawSlot<$n, 4>>()),
            5 => Some($visitor.visit::<RawSlot<$n, 5>>()),
            6 => Some($visitor.visit::<RawSlot<$n, 6>>()),
            7 => Some($visitor.visit::<RawSlot<$n, 7>>()),
            8 => Some($visitor.visit::<RawSlot<$n, 8>>()),
            9 => Some($visitor.visit::<RawSlot<$n, 9>>()),
            10 => Some($visitor.visit::<RawSlot<$n, 10>>()),
            11 => Some($visitor.visit::<RawSlot<$n, 11>>()),
            12 => Some($visitor.visit::<RawSlot<$n, 12>>()),
            13 => Some($visitor.visit::<RawSlot<$n, 13>>()),
            14 => Some($visitor.visit::<RawSlot<$n, 14>>()),
            15 => Some($visitor.visit::<RawSlot<$n, 15>>()),
            _ => None,
        }
    };
}

pub(crate) fn with_raw<R>(arity: usize, slot: usize, visitor: impl OpVisitor<R>) -> Option<R> {
    match arity {
        0 => raw_slots!(0, slot, visitor),
        1 => raw_slots!(1, slot, visitor),
        2 => raw_slots!(2, slot, visitor),
        3 => raw_slots!(3, slot, visitor),
        4 => raw_slots!(4, slot, visitor),
        5 => raw_slots!(5, slot, visitor),
        6 => raw_slots!(6, slot, visitor),
        7 => raw_slots!(7, slot, visitor),
        8 => raw_slots!(8, slot, visitor),
        _ => None,
    }
}
