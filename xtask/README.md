# xtask

`cargo xtask gen` turns the SkyrimRE-1.7.104 ledgers into `crates/layout/src/v1_7_104/` and `crates/layout/generated.lock`. The RE repository is found through `SKYRIMRE_DIR` or, by default, as `../SkyrimRE-1.7.104` next to this workspace. The output is committed; `cargo test -p xtask` regenerates in memory with the commit recorded in `generated.lock` and fails if anything on disk differs, so the test depends on the input files alone and the recorded commit moves only when `cargo xtask gen` runs (it skips with a message when the RE repository is absent).

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
- `fns.rs`: `pub const <name>: u64 = <id>` for every function with an Address Library id, a ledger name (not `ID_<n>`) and medium or high confidence, excluding exception-handling funclets. A function a semantic pass describes carries the meaning and the pass's typed signature in the doc, and when the pass rates it high and its signature has a Rust form (see Signatures) it also gets `pub type <name>_fn = unsafe extern "C" fn(..)` over raw pointers.
- `globals.rs`: every global with an id, same shape; the `g_*_ptr` globals say which class they point to, and a global a semantic pass describes carries its meaning, type and accessor id.
- `symbols.rs`: the crash logger's lookup tables. `FUNCTIONS` holds every function `fns.rs` names as a `layout::Function` (rva, id, confidence, name) sorted by rva, `MEANINGS` the semantic pass meanings by rva, `GLOBALS` every global with an id; `function_at(rva)`, `meaning_of(rva)` and `global_at(rva)` find the entry at or before an address, since the ledger records no sizes.
- `enums.rs`: `FormType` (`#[repr(u8)]`, `from_u8`, `signature`; a semantic pass may add its meaning and must agree with `formtypes.json`), every other semantic enum as a plain `#[repr(uN)]` enum with `from_uN` (`ActorValue` from the actors pass, width from the export), and one `#[repr(transparent)]` flags struct per semantic flag set, named `<Class>_<field>` with one `SCREAMING_CASE` const per named bit plus `bits` and `contains`.
- `api.rs`, `api_tier_high.rs`, `api_tier_medium.rs`, `api_tier_low.rs`, `api_untiered.rs`: the callable layer (next section), one file per signature tier. Each holds `impl classes::<Class>` blocks and a `functions` module; the semantic file also holds the `Deref` impls.
- `statics.rs`: one accessor per global with an id (below).
- `mod.rs`: the module list, the tier modules behind their features, `functions` (the re-export of every tier's free functions), the semantic passes folded in, and `VERSION`.

## Callable layer

Every engine function a signature can type is a Rust function that resolves its address through the Address Library on the first call and caches it in a static of its own, the way an author calls a member function: `TESDataHandler::singleton()?.get_file(..)`, `form.get_keyword_form()`, `functions::find_form_by_id(0x14)`. The resolver is `versionlib::running`, the process's one database (`<exe dir>\Data\SKSE\Plugins\versionlib-1-7-104-0.bin` with the executable's base), which `hook::Reloc` shares; a missing file or id panics naming both. Each body is one line through three macros of `layout/src/lib.rs`: `address!(id)`, `function!(id, (params) -> ret)` and `virtual_function!(self, index, (params) -> ret)`, which reads the slot from the object's own vtable at every call.

- Placement: a function whose first parameter is `this`, or a pointer to the class its name is scoped to, is a `&self` method on that class (the class of `this` when it is laid out, else the scope); a function scoped to a laid-out class without one is an associated function; every other function is a free function in `functions`. Names are the snake case of the engine name's last segment (the whole name for free functions, `::` as `__`), made unique within one `impl` across every tier with the most proven tier keeping the plain name; the engine name opens the doc.
- Slots: a slot of a class's primary vtable is a method when a semantic pass names it, or when its target is a ledger-named function of that same class (`Class::Name`, not `vfunc_<n>` or `ID_<n>`) with medium or high name confidence; it is typed by the pass's signature, or by the target's decompiler prototype in that prototype's tier. A derived class whose `Deref` chain already has the method at that slot, under the same name and in a tier at least as proven, does not repeat it. A function reached that way on its own class is not also emitted as a direct call (1,575).
- Inheritance: `impl Deref for <Class>` targets the primary base when the base sits at offset 0 non-virtually, is no larger than the class, every base field has a field at the same offset and width in the class (the prefix `classes.rs` flattens), and every reference to the class is aligned for the base: either the struct is at least as aligned, or no class embeds it by value, so a reference to it is a live engine object at the engine's natural alignment even where the ledger packs the struct. 5,959 classes qualify and 530 do not. A non-virtual secondary base at a non-zero displacement gets `as_<base>()` under the same size and alignment conditions, unless the primary base already provides it at that displacement.
- Globals: a class with exactly one `g_*_ptr` label gets `singleton() -> Option<&'static Self>` (22). Every other global is a function in `statics` named after it: a semantic `class:X` global returns `&'static X`, a `ptr:X` global (or a `g_*_ptr` label of a class with two) `Option<&'static X>`, a scalar its address as `*mut T`; a labelled global its address as `*mut X` or `*mut *mut X`; the rest `*mut c_void`.
- Tiers and features: the semantic passes' functions and slots, the `Deref` impls, base accessors and globals are always compiled. Functions typed only by the ledger's decompiler prototype are compiled by the crate feature named for the prototype's `sig_confidence`: `tier-high` (default), `tier-medium`, `tier-low`, `untiered`. Each tier is its own file, so a disabled tier is not even parsed. The ledger name filter of `fns.rs` applies, and two kinds of named functions are left out: `vfunc_<n>` names (20,325), which say only that a function sits in a vtable, and CRT static initializers (`crt_init` evidence, 11,634, `void <global>__init(void)`), which run once before `main` and re-initialize their global when called again.

### Build cost

Measured on 2026-10-26 on the development machine (16 threads, shared with another session's builds, so a few seconds of noise), each a build after `cargo clean -p layout`; before the callable layer the same builds took 35.0 s (debug `layout`) and 29.2 s (release `sample-plugin`).

| features | source parsed | debug `cargo build -p layout` | release `cargo build --release -p sample-plugin` |
|---|---|---|---|
| none (`--no-default-features`) | 83 MB | 44.7 s | |
| default (`tier-high`) | 86 MB | 53.6 s | 38.5 s |
| `tier-medium` alone / with default | 86 MB | 45.7 s | 38.5 s |
| `tier-low` alone / with default | 91 MB | 48.6 s | 41.2 s |
| `untiered` alone / with default | 94 MB | 69.9 s | 45.1 s |
| all four | 109 MB | 70.2 s | 61.5 s |

The always-compiled part (semantic methods and slots, 5,959 `Deref` impls, 6,834 global accessors) costs about 8 s of debug build, mostly type checking and borrow checking of the bodies; a tier adds its parsing and checking only, since every method is `#[inline]` and is code-generated only in the plugin that calls it. The sample plugin's DLL is the same size (972,800 bytes against 971,264).

### Signatures

Semantic and decompiler prototypes go through one parser. `char` is `c_char`; `uchar`/`ushort`/`uint`/`ulonglong` and `undefined1/2/4/8` are `u8`..`u64`, `int`/`longlong` `i32`/`i64`, `float`/`double` `f32`/`f64`, the few Windows typedefs by width; `undefined` (an unknown result) is the raw `u64`. `bool` is `bool` in a semantic signature and `u8` in a decompiler one. A semantic enum is its `enums` type as a parameter and its integer as a result. A pointer or C++ reference to a laid-out class is `*mut X` (`*const X` for a const pointee), to anything else `*mut c_void`; function pointers are `*const c_void`. A signature with a class or unknown type by value, an array parameter or varargs has no Rust form and is left out (counted as unexpressible).

### Safety rule

A function is a safe `fn` only when its signature comes from a semantic pass rated high, it is not a constructor or destructor (`ctor`, `dtor`, `vdtor`, `~X`, `X::X`), and every parameter other than `this` is a scalar, `bool`, semantic enum, or a pointer to a laid-out class that is const or has RTTI, taken as `&X`; a class-pointer result comes back as `Option<&'static X>` (valid as long as the engine keeps the object, as in the hand-written layer) and a `char*` result as a raw pointer. Everything else is `unsafe fn` with a `# Safety` section saying why: the pass's lower rating, the raw-pointer parameters by name (a non-const pointer to a class without RTTI, `char*`, `void*`, `BSFixedString*` out-parameters, function pointers), an untyped pointer result, or, for every decompiler-typed function, that nothing checks its prototype. Engine thread requirements a pass states are in its meaning, which opens the doc.

Types: `undefined1/2/4/8` become `u8`..`u64`, `float`/`double` `f32`/`f64`, `pointer` `*mut c_void`, `ptr:X` `*mut X`, `class:X` an embedded `X` when `X` has an exact size equal to the field width (else `[u8; n]`), `vtbl:X:off` `*const vtables::X_vtable[_off]`, anything else `[u8; n]`.

Identifiers: `::` becomes `__`, any other run of non-identifier characters `_`, a leading digit gets `_`, Rust keywords get a trailing `_`, names over 120 characters are cut to 96 plus a hash of the original, and duplicates within one namespace get `_2`, `_3`, ... in sorted order. Output order is by original name everywhere, so a regeneration from unchanged inputs produces no diff.

## Not generated

Functions the ledger has not named (`ID_<n>` placeholders) and low-confidence names (`_caller`, string names, `NoRTTI_`) are left out of `fns.rs` and the callable layer, unless a semantic pass names them; a mod reaches them by raw id through `hook::Reloc`. A semantic parameter that accepts null is still `&X`: the passes say so only in prose, which the generator does not read.
