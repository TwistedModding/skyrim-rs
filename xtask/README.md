# xtask

`cargo xtask gen` turns the SkyrimRE-1.7.104 ledgers into `crates/layout/src/v1_7_104/` and `crates/layout/generated.lock`. The RE repository is found through `SKYRIMRE_DIR` or, by default, as `../SkyrimRE-1.7.104` next to this workspace. The output is committed; `cargo test -p xtask` regenerates in memory and fails if anything on disk differs (it skips with a message when the RE repository is absent).

## Inputs

Exactly these files are read and hashed into `generated.lock`:

| file | used for |
|---|---|
| `export/symbols.csv` | function and global ids, names, evidence, confidence, decompiler signatures; vtable symbols and slot targets by rva |
| `data/layouts.json` | every RTTI class: fields, size, size kind and evidence, primary base |
| `data/class_structs.json` | the classes without RTTI (`BSTEventSource`, `TESDataHandler`, the event-source holders) and the typed `g_*_ptr` labels |
| `data/rtti.json` | class hierarchy with base displacements, vtables and their slot targets |
| `data/formtypes.json` | the `FormType` values |
| `export/semantic/*.json` | one file per subsystem pass (schema in the RE repo's `export/semantic/README.md`): class and field meanings, typed function and slot signatures, flag sets, global meanings |

`export/include/SkyrimRE/Types.h` was the structural reference for what a generated layout should contain; it is not read. The evidence CSVs under `data/evidence/` are already folded into the files above and are not read either.

## Output

- `classes.rs`: one `#[repr(C)]` struct per class. Ledger fields keep their ledger names (`__vftable`, `field_8`, `sinks`); bytes the ledger says nothing about are `unknown_<offset>: [u8; n]`. A class a semantic pass describes gets a `Meaning:` line, and every field a pass names (in the class itself or inherited from a base at the base's displacement) carries the meaning and its confidence after the ledger source. Classes with an exact size (`exact`, and `conflict` where the allocation size holds every field) are at the top level with a `size_of` assert; every other class (`lower`, `bounded`, and the three `conflict` classes whose fields pass the allocation size) is in `classes::partial`, padded to its lower bound and without a size assert. Every ledger field gets an `offset_of!` assert. A class is `#[repr(C, packed)]` when the ledger puts a field below its natural alignment (909 classes); the doc comment says so.
- `vtables.rs`: one `#[repr(C)]` struct per vtable (`<Class>_vtable`, `<Class>_vtable_<offset>` for secondary tables) with a `slot_N: *const c_void` per slot and the slot target's ledger name, id and confidence in the doc comment; `ID` is the vtable symbol's Address Library id. A slot a semantic pass names gets `SLOT_<name>: usize`; when the pass rates it high and its signature is fully typed, also `pub type <Class>_vtable_<name> = unsafe extern "C" fn(..)` and a `<name>(&self) -> Option<..>` accessor that reads the slot. `ALL` lists every vtable sorted by rva and `find(rva)` looks one up, which identifies a live object's class from its vtable pointer.
- `fns.rs`: `pub const <name>: u64 = <id>` for every function with an Address Library id, a ledger name (not `ID_<n>`) and medium or high confidence, excluding exception-handling funclets. The decompiler signature and its tier are documentation only. A function a semantic pass describes carries the meaning and the pass's typed signature in the doc, and when the pass rates it high and every type in the signature is expressible (`uint`, `int`, `uchar`, `ushort`, `bool`, `float`, `double`, `char*`, `void*`, pointers to laid-out classes) it also gets `pub type <name>_fn = unsafe extern "C" fn(..)`. A signature with a type the ledgers have not laid out (`BSFixedString*`) stays documentation.
- `globals.rs`: every global with an id, same shape; the `g_*_ptr` globals say which class they point to, and a global a semantic pass describes carries its meaning, type and accessor id.
- `enums.rs`: `FormType` (`#[repr(u8)]`, `from_u8`, `signature`; a semantic pass may add its meaning and must agree with `formtypes.json`), and one `#[repr(transparent)]` flags struct per semantic flag set, named `<Class>_<field>` with one `SCREAMING_CASE` const per named bit plus `bits` and `contains`.
- `mod.rs`: the module list, the semantic passes folded in, and `VERSION`.

Types: `undefined1/2/4/8` become `u8`..`u64`, `float`/`double` `f32`/`f64`, `pointer` `*mut c_void`, `ptr:X` `*mut X`, `class:X` an embedded `X` when `X` has an exact size equal to the field width (else `[u8; n]`), `vtbl:X:off` `*const vtables::X_vtable[_off]`, anything else `[u8; n]`.

Identifiers: `::` becomes `__`, any other run of non-identifier characters `_`, a leading digit gets `_`, Rust keywords get a trailing `_`, names over 120 characters are cut to 96 plus a hash of the original, and duplicates within one namespace get `_2`, `_3`, ... in sorted order. Output order is by original name everywhere, so a regeneration from unchanged inputs produces no diff.

## Not generated

Functions the ledger has not named (41,045 `ID_<n>` placeholders, among them `LookupFormByID`, id 14617) and low-confidence names (`_caller`, string names, `NoRTTI_`) are left out; a mod reaches them by raw id through `hook::Reloc`.
