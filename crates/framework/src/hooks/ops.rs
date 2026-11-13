// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The typed engine operations a plugin can subscribe to, and the raw slots for function entries,
//! call sites and vtable slots outside the set.
//!
//! Each marker type below is one engine function: its Address Library id and `extern "C" fn` type
//! come from `sre`, so a subscriber's closure is checked against the function's real
//! signature. Adding one is one line in `ops!`.

use core::marker::PhantomData;

use sre::raw::fns;

use super::site::{Op, Signature, Site};
use super::{Hookable, Location, Register, Target};

/// Visits the `Op` type behind an index or a raw slot.
pub(crate) trait OpVisitor<R> {
    fn visit<O: Op>(self) -> R;
}

macro_rules! ops {
    ($($id:ident / $fn:ident = $name:literal,)*) => {
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
                    sre::id!(fns::$id);
                    Target::Op(Index::$id as usize)
                }
            }
        )*

        /// Display name and Address Library id of every operation, by index.
        pub(crate) const OPS: &[(&str, u64)] = &[$(($name, fns::$id)),*];

        pub(crate) fn with_op<R>(index: usize, visitor: impl OpVisitor<R>) -> Option<R> {
            $(
                if index == Index::$id as usize {
                    return Some(visitor.visit::<$id>());
                }
            )*
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
    PlayerCharacter__SetFirstPerson / PlayerCharacter__SetFirstPerson_fn = "PlayerCharacter::SetFirstPerson",
    Sky__Update / Sky__Update_fn = "Sky::Update",
}

/// How many raw addresses, call sites and vtable slots a process can hook together. A slot stays
/// bound to its address for the session, so a reloaded plugin finds its own again.
pub const RAW_SLOTS: usize = 128;

/// The most integer-class arguments a raw hook can carry.
pub const RAW_MAX_ARITY: usize = 8;

static RAW_SITES: [Site; RAW_SLOTS] = [const { Site::new() }; RAW_SLOTS];

/// A function `sre` does not type, hooked by address with a shape the caller vouches for: a raw
/// shape carries integer-class arguments and results only (`usize` each, in register order; a
/// float or a struct passed by value is not preserved), a typed shape the function type it names.
/// A second plugin may subscribe to the same address with the same shape; a different shape is refused.
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
        Target::Raw { address: self.0, shape: F::SHAPE }
    }
}

/// One `call rel32` (`E8`) inside a function, of a callee with the shape `F`: the call is
/// pointed at the dispatcher and the callee it reached is the original, so only this caller is
/// affected. A function-entry subscription on the callee runs as well, inside the original.
/// Restored once its last subscriber goes. A tail `jmp rel32` (`E9`) to a function is taken the
/// same way (the dispatcher returns to the caller's caller, as the callee did); its site is named
/// `jmp <location>/<shape>`. A virtual call `call [rax+disp32]` (`FF 90`) whose rax is the first
/// argument's vtable (`mov rax, [rcx]` before it, or `mov rax, [r]; mov rcx, r`) is taken too: the
/// original calls the same slot of the first argument's vtable, so each object reaches its own
/// function.
pub struct Call<F: RawShape>(Location, Option<Register>, PhantomData<F>);

impl<F: RawShape> Call<F> {
    #[must_use]
    pub const fn at(instruction: Location) -> Self {
        Self(instruction, None, PhantomData)
    }

    /// Hands the subscribers the caller's `register` as the last argument of `F`, which is a raw
    /// shape of 1 to 4 arguments: the callee's own, then the register (the original receives it
    /// too, in an argument register the callee does not read). The site is named
    /// `call <location>/<shape> +<register>`.
    #[must_use]
    pub const fn passing(self, register: Register) -> Self {
        Self(self.0, Some(register), PhantomData)
    }
}

impl<F: RawShape> Hookable for Call<F> {
    type Fn = F;

    fn target(self) -> Target {
        Target::Call { at: self.0, shape: F::SHAPE, register: self.1 }
    }
}

/// Slot `n` of a vtable, of a virtual function with the shape `F`: the slot's pointer is
/// swapped for the dispatcher's and the previous one is the original, so only objects of that
/// vtable are affected. Restored once its last subscriber goes.
pub struct Vtable<F: RawShape>(Location, usize, PhantomData<F>);

impl<F: RawShape> Vtable<F> {
    /// `table` is the vtable itself (`sre::raw::vtables::<Class>_vtable::ID`, or its address).
    #[must_use]
    pub const fn slot(table: Location, n: usize) -> Self {
        Self(table, n, PhantomData)
    }
}

impl<F: RawShape> Hookable for Vtable<F> {
    type Fn = F;

    fn target(self) -> Target {
        Target::Vtable { table: self.0, slot: self.1, shape: F::SHAPE }
    }
}

/// A shape a raw entry, call site or vtable slot carries: `unsafe extern "C" fn(usize, ...) ->
/// usize` with up to [`RAW_MAX_ARITY`] arguments, or one of the `sre` function types `shapes!`
/// lists, whose float arguments and float result reach the original intact. Adding a typed shape
/// is one line there.
pub trait RawShape: Signature {
    /// The shape's index in [`SHAPES`].
    const SHAPE: usize;
}

pub(crate) struct RawSlot<const N: usize, const K: usize>;

macro_rules! raw_slots {
    ($n:literal, $slot:expr, $visitor:expr) => {
        raw_slots!(@ $n, $slot, $visitor,
            0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
            32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
            64 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95
            96 97 98 99 100 101 102 103 104 105 106 107 108 109 110 111 112 113 114 115 116 117 118 119 120 121 122 123 124 125 126 127)
    };
    (@ $n:literal, $slot:expr, $visitor:expr, $($k:literal)*) => {
        match $slot {
            $($k => Some($visitor.visit::<RawSlot<$n, $k>>()),)*
            _ => None,
        }
    };
}

macro_rules! shapes {
    ($($n:literal $label:literal => $fn:ty;)*) => {
        $(
            impl RawShape for $fn {
                const SHAPE: usize = $n;
            }

            impl<const K: usize> Op for RawSlot<$n, K> {
                type Fn = $fn;

                fn site() -> &'static Site {
                    &RAW_SITES[K]
                }
            }
        )*

        /// Each shape as site names show it: a raw shape's arity, a typed shape's function.
        pub const SHAPES: &[&str] = &[$($label),*];

        pub(crate) fn with_raw<R>(shape: usize, slot: usize, visitor: impl OpVisitor<R>) -> Option<R> {
            match shape {
                $($n => raw_slots!($n, slot, visitor),)*
                _ => None,
            }
        }
    };
}

shapes! {
    0 "0" => unsafe extern "C" fn() -> usize;
    1 "1" => unsafe extern "C" fn(usize) -> usize;
    2 "2" => unsafe extern "C" fn(usize, usize) -> usize;
    3 "3" => unsafe extern "C" fn(usize, usize, usize) -> usize;
    4 "4" => unsafe extern "C" fn(usize, usize, usize, usize) -> usize;
    5 "5" => unsafe extern "C" fn(usize, usize, usize, usize, usize) -> usize;
    6 "6" => unsafe extern "C" fn(usize, usize, usize, usize, usize, usize) -> usize;
    7 "7" => unsafe extern "C" fn(usize, usize, usize, usize, usize, usize, usize) -> usize;
    8 "8" => unsafe extern "C" fn(usize, usize, usize, usize, usize, usize, usize, usize) -> usize;
    9 "NiCamera::SetPerspectiveFromFov" => fns::NiCamera__SetPerspectiveFromFov_fn;
    10 "PlayerCamera::GetDefaultFOVForCurrentState" => fns::PlayerCamera__GetDefaultFOVForCurrentState_fn;
}
