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
