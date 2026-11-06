// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The rules file: a signature (exception code, what the access address was, names on the top
//! frames, what the probes saw) matched to a plain explanation and who is likely at fault, and
//! the hash that makes one crash recognizable across reports. Seeded with the crashes this
//! repository understands; add a rule here when a crash is understood.

use core::fmt::{self, Write};

/// What the faulting access address turned out to be (`report::classify` in one word).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    None,
    Null,
    NearNull,
    Image,
    FreedImage,
    Stack,
    Unmapped,
    Guard,
    Heap,
}

/// What the probes of the registers and the stack top saw, as flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Seen {
    pub null_base: bool,
    pub deleted_form: bool,
    pub navmesh: bool,
    pub freed_image: bool,
    pub form_file_len: u8,
    /// The last file a probed form was read from (the first such form), the owner a rule may blame.
    pub form_file: [u8; 64],
    /// The freed plugin image a pointer led into.
    pub freed_plugin_len: u8,
    pub freed_plugin: [u8; 64],
}

impl Default for Seen {
    fn default() -> Self {
        Self { null_base: false, deleted_form: false, navmesh: false, freed_image: false, form_file_len: 0, form_file: [0; 64], freed_plugin_len: 0, freed_plugin: [0; 64] }
    }
}

impl Seen {
    pub fn note_form_file(&mut self, name: &str) {
        if self.form_file_len == 0 {
            self.form_file_len = u8::try_from(crate::record::store(&mut self.form_file, name)).unwrap_or(0);
        }
    }

    pub fn note_freed_plugin(&mut self, name: &str) {
        self.freed_image = true;
        if self.freed_plugin_len == 0 {
            self.freed_plugin_len = u8::try_from(crate::record::store(&mut self.freed_plugin, name)).unwrap_or(0);
        }
    }

    #[must_use]
    pub fn form_file(&self) -> &str {
        crate::record::text(&self.form_file, self.form_file_len as usize)
    }

    #[must_use]
    pub fn freed_plugin(&self) -> &str {
        crate::record::text(&self.freed_plugin, self.freed_plugin_len as usize)
    }
}

/// Who a rule blames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Culprit {
    Engine,
    /// The module of the faulting instruction.
    TopFrame,
    /// The first frame outside the game and the system, if any.
    FirstPlugin,
    /// The plugin file the probed form came from.
    FormOwner,
    /// The framework plugin whose unloaded image a pointer led into.
    FreedImage,
}

#[allow(clippy::struct_excessive_bools)]
pub struct Rule {
    pub name: &'static str,
    /// The exception code, or 0 for any.
    pub code: u32,
    /// Accepted access classes; empty for any.
    pub access: &'static [Access],
    /// Substrings that must each appear on one of the top frames' names.
    pub frames: &'static [&'static str],
    pub null_base: bool,
    pub deleted_form: bool,
    pub navmesh: bool,
    pub freed_image: bool,
    /// Whether the faulting address itself lies outside every module.
    pub rip_unmapped: bool,
    pub explanation: &'static str,
    pub culprit: Culprit,
}

const ANY: &[Access] = &[];

const fn rule(name: &'static str, explanation: &'static str, culprit: Culprit) -> Rule {
    Rule { name, code: 0, access: ANY, frames: &[], null_base: false, deleted_form: false, navmesh: false, freed_image: false, rip_unmapped: false, explanation, culprit }
}

pub static RULES: &[Rule] = &[
    Rule {
        frames: &["BSLightingShader::SetupGeometry"],
        access: &[Access::NearNull],
        ..rule(
            "sleeping-giant-inn-light",
            "The engine's own use after free of a light during a cell transition (SkyrimRE frame.md section 9): a render pass still lists a BSLight whose destructor ran within the last millisecond, so the lighting shader reads the light's dimmer through a null NiLight pointer. Seen on the Sleeping Giant Inn transition with no plugin loaded; heavy hooks that slow the frame hide it.",
            Culprit::Engine,
        )
    },
    Rule {
        frames: &["BSEffectShader"],
        access: &[Access::NearNull],
        ..rule(
            "sleeping-giant-inn-light-effect",
            "The engine's own use after free of a light during a cell transition (SkyrimRE frame.md section 9), the effect-shader site: the pass reads a destroyed BSLight through a null NiLight pointer.",
            Culprit::Engine,
        )
    },
    Rule {
        frames: &["SetCategoryVolume"],
        code: 0xC000_0005,
        ..rule(
            "set-category-volume-argument",
            "SetCategoryVolume takes a pointer to a variable holding the category's BSISoundCategory interface (form+0x30), not the interface itself (SkyrimRE request 11b): passed the interface, the first load returns the vtable and the call through it faults. A plugin called it with the wrong argument.",
            Culprit::FirstPlugin,
        )
    },
    Rule {
        freed_image: true,
        ..rule(
            "freed-generation",
            "A pointer into a framework plugin's unloaded image: the engine or another plugin kept a function or object pointer across a hot reload (docs/subsystems/reload.md). Everything the engine keeps must be registered through the Host so the host re-points it; a plugin must not hand raw pointers into its own image to others.",
            Culprit::FreedImage,
        )
    },
    Rule {
        null_base: true,
        ..rule(
            "null-base-reference",
            "A placed reference without a base object: its base record is gone or was never loaded, the classic plugin conflict (a plugin deletes or fails to carry a record that another plugin's reference still points at, or a master is missing). The reference's owning plugin is named with it.",
            Culprit::FormOwner,
        )
    },
    Rule {
        deleted_form: true,
        navmesh: true,
        ..rule(
            "deleted-navmesh",
            "A deleted navmesh was still in use: a plugin deletes a navmesh another plugin or the engine still walks. Undelete and disable instead of deleting (xEdit's cleaning does this).",
            Culprit::FormOwner,
        )
    },
    Rule {
        deleted_form: true,
        ..rule(
            "deleted-record",
            "A deleted record was still in use: a plugin deletes a record (a reference, a navmesh) that another plugin or the save still refers to. Undelete and disable instead of deleting.",
            Culprit::FormOwner,
        )
    },
    Rule {
        navmesh: true,
        code: 0xC000_0005,
        ..rule(
            "navmesh",
            "A navmesh was among the objects in play: overlapping or broken navmeshes between plugins in one cell are the usual cause.",
            Culprit::FormOwner,
        )
    },
    Rule {
        code: 0xC000_00FD,
        ..rule("stack-overflow", "The thread ran out of stack: unbounded recursion or very large stack frames. The module whose frames repeat is the culprit.", Culprit::TopFrame)
    },
    Rule {
        code: 0xC000_0005,
        access: &[Access::Image],
        ..rule(
            "write-into-image",
            "A write into a module's code or read-only data: a patcher writing without changing the page protection, or a wild pointer that happened to land in an image. The faulting instruction's module did the write.",
            Culprit::TopFrame,
        )
    },
    Rule {
        rip_unmapped: true,
        ..rule(
            "call-into-nothing",
            "The thread was executing memory no module owns: a call through a dangling function pointer (a DLL unloaded, a trampoline freed, a vtable overwritten). The first frame below it is where the call came from.",
            Culprit::FirstPlugin,
        )
    },
    Rule {
        code: 0xC000_0374,
        ..rule(
            "heap-corruption",
            "The heap found its bookkeeping damaged: something wrote past an allocation or freed memory twice, usually long before this point. The reporting thread is only the one that noticed.",
            Culprit::FirstPlugin,
        )
    },
    Rule {
        code: 0xC000_0409,
        ..rule(
            "fail-fast",
            "A fail-fast: a Rust DLL's panic (release builds abort) or a stack cookie failure. The module of the top frames is the one that stopped the game.",
            Culprit::TopFrame,
        )
    },
    Rule {
        code: 0xC000_0005,
        access: &[Access::Null, Access::NearNull],
        ..rule(
            "null-dereference",
            "A null pointer dereference: an object the code expected is missing, uninitialized or already released. The game function at the top says which object, and the registers above name the ones that were still valid.",
            Culprit::FirstPlugin,
        )
    },
    Rule {
        code: 0xC000_0005,
        access: &[Access::Unmapped, Access::Heap],
        ..rule(
            "stale-pointer",
            "An access to memory that is no longer (or not yet) an object: a stale pointer to a freed object, or garbage read as a pointer. The registers above say which objects were still valid.",
            Culprit::FirstPlugin,
        )
    },
];

/// What a rule is matched against.
pub struct Signature<'a> {
    pub code: u32,
    pub access: Access,
    /// The top frames' names as the report prints them.
    pub frames: &'a [&'a str],
    pub seen: &'a Seen,
    pub rip_unmapped: bool,
}

impl Rule {
    #[must_use]
    pub fn matches(&self, s: &Signature<'_>) -> bool {
        (self.code == 0 || self.code == s.code)
            && (self.access.is_empty() || self.access.contains(&s.access))
            && self.frames.iter().all(|needle| s.frames.iter().any(|f| f.contains(needle)))
            && (!self.null_base || s.seen.null_base)
            && (!self.deleted_form || s.seen.deleted_form)
            && (!self.navmesh || s.seen.navmesh)
            && (!self.freed_image || s.seen.freed_image)
            && (!self.rip_unmapped || s.rip_unmapped)
    }
}

/// The first rule that matches, in file order.
#[must_use]
pub fn explain(s: &Signature<'_>) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.matches(s))
}

/// FNV-1a over the top frames' names: the same crash hashes the same across sessions, since the
/// names carry no load addresses.
#[must_use]
pub fn signature_hash(frames: &[&str]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for name in frames {
        for byte in name.bytes().chain(*b"\n") {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// Writes the hash as the log shows it.
pub fn write_hash(w: &mut dyn Write, frames: &[&str]) -> fmt::Result {
    write!(w, "{:016x}", signature_hash(frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matching_rule_wins() {
        let seen = Seen::default();
        let frames = ["SkyrimSE.exe!BSLightingShader::SetupGeometry+0x6db", "SkyrimSE.exe!RenderPassImmediately+0x1"];
        let s = Signature { code: 0xC000_0005, access: Access::NearNull, frames: &frames, seen: &seen, rip_unmapped: false };
        assert_eq!(explain(&s).map(|r| r.name), Some("sleeping-giant-inn-light"));
        let plain = ["skyrim_rs_crash_test.dll+0x1234"];
        let s = Signature { code: 0xC000_0005, access: Access::Null, frames: &plain, seen: &seen, rip_unmapped: false };
        assert_eq!(explain(&s).map(|r| r.name), Some("null-dereference"));
        let seen = Seen { null_base: true, ..Seen::default() };
        let s = Signature { code: 0xC000_0005, access: Access::Heap, frames: &plain, seen: &seen, rip_unmapped: false };
        assert_eq!(explain(&s).map(|r| r.name), Some("null-base-reference"));
    }

    #[test]
    fn hash_ignores_nothing_but_order() {
        assert_eq!(signature_hash(&["a", "b"]), signature_hash(&["a", "b"]));
        assert_ne!(signature_hash(&["a", "b"]), signature_hash(&["b", "a"]));
    }
}
