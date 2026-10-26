//! Game definitions generated from the `SkyrimRE` ledgers, one module per runtime.
#![no_std]

pub mod v1_7_104;

/// One vtable of a class as the RTTI records it; `offset` is where the subobject using it starts.
pub struct Vtable {
    pub class: &'static str,
    pub offset: u32,
    pub id: u64,
    pub rva: u32,
}

/// How sure the ledger is of a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Confidence {
    High,
    Medium,
    Low,
    Unknown,
}

impl Confidence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }
}

/// One function the ledger names: its module-relative address, its Address Library id and the
/// name as `fns` spells it in its docs; the semantic pass meaning, where one exists, is in
/// `symbols::MEANINGS` under the same rva.
pub struct Function {
    pub rva: u32,
    pub id: u32,
    pub confidence: Confidence,
    pub name: &'static str,
}

/// One global the ledger names.
pub struct Global {
    pub rva: u32,
    pub id: u32,
    pub name: &'static str,
}
