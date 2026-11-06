// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The callable layer of the layout crate: engine functions as methods on their classes, named
//! vtable slots, base-class conversions and global accessors.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use crate::emit::{Class, KEYWORDS, Ty as EmitTy, colon, doc, ident, is_funclet, is_placeholder, screaming, ty_str, unique};
use crate::groups::{Items, Kind, family, hierarchy};
use crate::inputs::{Inputs, SemFunction, SemGlobal, SemSlot, Symbol};
use crate::versions::Variance;

/// Where a callable's signature comes from, most proven first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Semantic,
    High,
    Medium,
    Low,
    Untiered,
}

impl Tier {
    pub const ALL: [Self; 5] = [Self::Semantic, Self::High, Self::Medium, Self::Low, Self::Untiered];

    fn of(sig_confidence: &str) -> Self {
        match sig_confidence {
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            _ => Self::Untiered,
        }
    }

    /// The crate feature that compiles the tier; the semantic tier is always compiled.
    #[must_use]
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Self::Semantic => None,
            Self::High => Some("tier-high"),
            Self::Medium => Some("tier-medium"),
            Self::Low => Some("tier-low"),
            Self::Untiered => Some("untiered"),
        }
    }

    /// The stem of the tier's files in a group module.
    #[must_use]
    pub fn module(self) -> &'static str {
        match self {
            Self::Semantic => "api",
            Self::High => "tier_high",
            Self::Medium => "tier_medium",
            Self::Low => "tier_low",
            Self::Untiered => "untiered",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Untiered => "untiered",
        }
    }
}

/// What one tier emitted.
#[derive(Default, Clone)]
pub struct TierStats {
    pub safe_methods: usize,
    pub unsafe_methods: usize,
    pub safe_associated: usize,
    pub unsafe_associated: usize,
    pub safe_free: usize,
    pub unsafe_free: usize,
    pub safe_slots: usize,
    pub unsafe_slots: usize,
    /// Functions or slots left out because the signature has no Rust form, by reason.
    pub unexpressible: BTreeMap<Unexpressible, usize>,
}

#[derive(Default)]
pub struct ApiStats {
    pub tiers: BTreeMap<Tier, TierStats>,
    pub derefs: usize,
    pub derefs_refused: usize,
    pub base_accessors: usize,
    /// Pointer fields a pass names with high confidence, read as `Option<&X>`.
    pub field_accessors: usize,
    pub singletons: usize,
    pub statics: usize,
    /// Functions reached as a named slot of the same class, so not emitted as a direct call.
    pub virtual_only: usize,
    /// `vfunc_<n>` names, which say only that the function sits in a vtable.
    pub vfunc_names: usize,
    /// CRT static initializers: they run once before `main`, and a second call re-initializes their global.
    pub initializers: usize,
}

impl ApiStats {
    fn left_out(&mut self, tier: Tier, why: Unexpressible) {
        *self.tiers.entry(tier).or_default().unexpressible.entry(why).or_default() += 1;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Semantic,
    Decompiler,
}

#[derive(Clone, Debug, PartialEq)]
enum Ty {
    Void,
    Scalar(&'static str),
    Bool,
    Enum(String, &'static str),
    /// One pointer or reference to a laid-out class, and whether the pointee is const.
    Class(String, bool),
    /// Any other pointer, as its Rust type.
    Raw(String),
}

struct Param {
    name: String,
    /// The C type's name without `const`, pointers and references.
    base: String,
    ty: Ty,
    /// The pass marks it as possibly null.
    nullable: bool,
}

struct Sig {
    ret: Ty,
    params: Vec<Param>,
}

pub(crate) struct Ctx<'a> {
    pub(crate) classes:&'a BTreeMap<&'a str, Class<'a>>,
    rtti: HashSet<&'a str>,
    /// Semantic enum name to its integer type and its Rust name.
    pub(crate) enums: HashMap<String, (&'static str, String)>,
    /// Class name to its group module.
    pub(crate) groups: HashMap<&'a str, &'a str>,
    /// The math value types (the `nimath` pass): plain data the engine reads or writes by pointer
    /// as one value, so a non-const pointer to one is `&mut X`.
    pub(crate) values: HashSet<&'a str>,
    /// What is not identical on every runtime, so is read from the profile.
    pub(crate) variance: &'a Variance,
    /// The Rust paths of the enums whose numbers differ between runtimes.
    variant_enums: HashSet<String>,
}

impl Ctx<'_> {
    pub(crate) fn group(&self, class: &str) -> &str {
        self.groups[class]
    }
}

fn scalar(base: &str) -> Option<&'static str> {
    Some(match base {
        "uchar" | "u8" | "uint8" | "byte" | "undefined1" => "u8",
        "char" => "c_char",
        "i8" | "int8" | "s8" | "schar" => "i8",
        "ushort" | "u16" | "uint16" | "undefined2" | "wchar_t" | "WORD" | "u_short" => "u16",
        "short" | "i16" | "int16" | "s16" => "i16",
        "uint" | "u32" | "uint32" | "undefined4" | "DWORD" | "UINT" | "ulong" | "u_int" => "u32",
        "int" | "i32" | "int32" | "s32" | "long" | "LONG" | "BOOL" | "HRESULT" | "errno_t" => "i32",
        "ulonglong" | "u64" | "uint64" | "undefined8" | "undefined" => "u64",
        "longlong" | "i64" | "int64" | "s64" | "__int64" => "i64",
        "size_t" | "SIZE_T" | "rsize_t" | "uintptr_t" | "usize" | "WPARAM" | "UINT_PTR" => "usize",
        "LPARAM" | "LRESULT" | "LONG_PTR" | "intptr_t" => "isize",
        "float" | "f32" => "f32",
        "double" | "f64" => "f64",
        _ => return None,
    })
}

fn pointer(inner: &str, depth: usize, constant: bool) -> String {
    let mut out = inner.to_owned();
    for level in 0..depth {
        out = format!("{} {out}", if level == 0 && constant { "*const" } else { "*mut" });
    }
    out
}

/// A C type's name without `const`, pointers and references, its pointer depth, and whether the pointee is const.
fn parts(text: &str) -> (String, usize, bool) {
    let text = text.trim();
    let words = text.replace(" const*", " *").replace(" const&", " &");
    let words: Vec<&str> = words.split_whitespace().filter(|w| *w != "const").collect();
    let t = words.join(" ");
    let base = t.trim_end_matches(['*', '&', ' ']);
    let depth = t[base.len()..].chars().filter(|c| matches!(c, '*' | '&')).count();
    (base.to_owned(), depth, text.starts_with("const "))
}

/// Why a signature has no Rust form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Unexpressible {
    Varargs,
    /// A laid-out class by value that is not a plain value the ABI can carry.
    ClassByValue,
    /// A type the ledger does not lay out, by value.
    UnknownByValue,
    /// A parameter name that is not an identifier (`this+0x58`), or text that is not a prototype.
    Malformed,
}

impl Unexpressible {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Varargs => "varargs",
            Self::ClassByValue => "class by value",
            Self::UnknownByValue => "unknown type by value",
            Self::Malformed => "malformed",
        }
    }
}

/// A C type as the generator types it, or why it has no Rust form.
fn c_type(text: &str, source: Source, ctx: &Ctx) -> Result<Ty, Unexpressible> {
    let (base, depth, constant) = parts(text);
    let base = base.as_str();
    let raw = |inner: &str| Ok(Ty::Raw(pointer(inner, depth, constant)));
    match base {
        "void" | "undefined" | "code" if depth > 0 => return raw("c_void"),
        "void" => return Ok(Ty::Void),
        "va_list" if depth == 0 => return Err(Unexpressible::Varargs),
        "HWND" | "HINSTANCE" | "HMODULE" | "HANDLE" => return Ok(Ty::Raw(pointer("*mut c_void", depth, constant))),
        "bool" if source == Source::Semantic => return if depth == 0 { Ok(Ty::Bool) } else { raw("bool") },
        "bool" => return if depth == 0 { Ok(Ty::Scalar("u8")) } else { raw("u8") },
        _ => {}
    }
    if let Some(s) = scalar(base) {
        return if depth == 0 { Ok(Ty::Scalar(s)) } else { raw(s) };
    }
    if source == Source::Semantic
        && let Some((repr, name)) = ctx.enums.get(base)
    {
        return if depth == 0 { Ok(Ty::Enum(format!("crate::{name}"), repr)) } else { raw(&format!("crate::{name}")) };
    }
    if let Some(class) = ctx.classes.get(base) {
        return match depth {
            0 => Err(Unexpressible::ClassByValue),
            1 => Ok(Ty::Class(base.to_owned(), constant)),
            _ => raw(&class.path()),
        };
    }
    if depth == 0 {
        Err(Unexpressible::UnknownByValue)
    } else {
        raw("c_void")
    }
}

/// Splits at `sep` outside `<>` and `()`.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// The index of the last whitespace outside `<>`.
fn last_top_space(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut found = None;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => found = Some(i),
            _ => {}
        }
    }
    found
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && !s.as_bytes()[0].is_ascii_digit() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The text without its `/* ... */` comments.
fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..].find("*/").map_or("", |end| &rest[start + end + 2..]);
    }
    out + rest
}

/// A C prototype, with or without the function's name: `ret [name](type name, ...)`. An array
/// parameter is the pointer C passes; a bare `this` is the object as an untyped pointer.
fn parse_sig(sig: &str, source: Source, ctx: &Ctx) -> Result<Sig, Unexpressible> {
    let sig = strip_comments(sig);
    let body = sig.trim().strip_suffix(')').ok_or(Unexpressible::Malformed)?;
    let mut depth = 0;
    let mut open = None;
    for (i, c) in body.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth == 0 => {
                open = Some(i);
                break;
            }
            '(' => depth -= 1,
            _ => {}
        }
    }
    let open = open.ok_or(Unexpressible::Malformed)?;
    let head = body[..open].trim();
    let ret = match last_top_space(head) {
        _ if head.ends_with(['*', '&']) => head,
        Some(i) if !matches!(head[..i].trim(), "" | "const") => &head[..i],
        _ => head,
    };
    let ret = c_type(ret, source, ctx)?;
    let list = body[open + 1..].trim();
    let mut params = Vec::new();
    if !matches!(list, "" | "void") {
        for (i, p) in split_top(list, ',').into_iter().enumerate() {
            let p = p.trim();
            if p == "..." {
                return Err(Unexpressible::Varargs);
            }
            if let Some((_, rest)) = p.split_once("(*") {
                params.push(Param { name: rest.split(')').next().unwrap_or_default().trim().to_owned(), base: String::new(), ty: Ty::Raw("*const c_void".into()), nullable: false });
                continue;
            }
            if p == "this" {
                params.push(Param { name: p.to_owned(), base: String::new(), ty: Ty::Raw("*mut c_void".into()), nullable: false });
                continue;
            }
            let array = p.strip_suffix(']').and_then(|q| q.rfind('['));
            let p = array.map_or(p, |at| p[..at].trim_end());
            let (ty, name) = match p.rfind(|c: char| c.is_whitespace() || c == '*' || c == '&') {
                Some(at) => (&p[..=at], p[at + 1..].trim()),
                None => (p, ""),
            };
            let name = if name.is_empty() { format!("a{i}") } else { name.to_owned() };
            if !is_ident(&name) {
                return Err(Unexpressible::Malformed);
            }
            let ty = if array.is_some() { format!("{ty}*") } else { ty.to_owned() };
            params.push(Param { name, base: parts(&ty).0, ty: c_type(&ty, source, ctx)?, nullable: false });
        }
    }
    Ok(Sig { ret, params })
}

/// `GetFormEditorID` becomes `get_form_editor_id`; scopes keep their `__`.
fn snake(name: &str) -> String {
    let mut out = screaming(name).to_ascii_lowercase();
    if KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// The name before and after the last `::` outside template arguments.
fn split_scope(name: &str) -> (Option<&str>, &str) {
    let mut depth = 0i32;
    let mut at = None;
    let bytes = name.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') && (i == 0 || bytes[i - 1] != b':') => at = Some(i),
            _ => {}
        }
    }
    at.map_or((None, name), |i| (Some(&name[..i]), &name[i + 2..]))
}

fn is_vfunc(last: &str) -> bool {
    last.strip_prefix("vfunc_").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Why a semantic callable stays `unsafe`, `None` when it can be a safe `fn`.
fn unsafe_reason(sig: &Sig, method: bool, confidence: &str, last: &str, class: Option<&str>, ctx: &Ctx) -> Option<String> {
    if confidence != "high" {
        return Some(format!("The pass rates the signature {confidence}."));
    }
    if last == "ctor" || last.to_ascii_lowercase().contains("dtor") || last.ends_with("Destroy") || last.starts_with('~') || class.is_some_and(|c| split_scope(c).1 == last) {
        return Some("It constructs or destroys an object in place.".into());
    }
    let raw: Vec<String> = sig
        .params
        .iter()
        .skip(usize::from(method))
        .filter(|p| match &p.ty {
            Ty::Scalar(_) | Ty::Bool | Ty::Enum(..) => false,
            Ty::Class(c, constant) => !(*constant || ctx.rtti.contains(c.as_str()) || ctx.values.contains(c.as_str())),
            Ty::Void | Ty::Raw(_) => true,
        })
        .map(|p| format!("`{}`", ident(&p.name)))
        .collect();
    let ret_raw = matches!(&sig.ret, Ty::Raw(r) if r != "*mut c_char" && r != "*const c_char");
    match (raw.is_empty(), ret_raw) {
        (true, false) => None,
        (false, false) => Some(format!("{} must point to what the engine expects.", raw.join(", "))),
        (true, true) => Some("The result is an untyped pointer.".into()),
        (false, true) => Some(format!("{} must point to what the engine expects, and the result is an untyped pointer.", raw.join(", "))),
    }
}

impl Ty {
    /// The type in a signature; a class pointer to `owner` names it `Self`.
    fn render(&self, safe: bool, ret: bool, owner: Option<&str>, ctx: &Ctx) -> String {
        match self {
            Self::Void => String::new(),
            Self::Scalar(s) => (*s).to_owned(),
            Self::Bool => "bool".into(),
            Self::Enum(path, repr) => if ret { (*repr).to_owned() } else { path.clone() },
            Self::Class(c, constant) => {
                let path = if owner == Some(c.as_str()) { "Self".to_owned() } else { ctx.classes[c.as_str()].path() };
                match (safe, ret) {
                    (true, true) => format!("Option<&'static {path}>"),
                    (true, false) if !*constant && ctx.values.contains(c.as_str()) => format!("&mut {path}"),
                    (true, false) => format!("&{path}"),
                    (false, _) => pointer(&path, 1, *constant),
                }
            }
            Self::Raw(r) => r.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// A `&self` method; the engine's `this` is the object's subobject at this offset.
    Method(u64),
    Associated,
    Free,
}

/// `pub [unsafe] fn NAME(params) -> ret { unsafe { call(args) } }` with `@NAME@` standing for the name.
fn render_call(doc_lines: &str, shape: Shape, safe: bool, sig: &Sig, callee: &str, owner: Option<&str>, ctx: &Ctx) -> String {
    let method = if let Shape::Method(offset) = shape { Some(offset) } else { None };
    let rest = &sig.params[usize::from(method.is_some())..];
    let mut names: Vec<String> = rest.iter().map(|p| ident(&p.name)).collect();
    unique(&mut names);
    let mut decl: Vec<String> = Vec::new();
    let mut types: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if let Some(offset) = method {
        decl.push("&self".into());
        types.push("*const Self".into());
        args.push(if offset == 0 { "core::ptr::from_ref(self)".into() } else { format!("core::ptr::from_ref(self).byte_add({})", base_offset(owner, offset, ctx)) });
    }
    // References cross as raw pointers: the engine writes through `this` and other objects it is
    // handed, which a reference argument would tell the compiler it cannot.
    for (p, name) in rest.iter().zip(&names) {
        if let Ty::Enum(path, repr) = &p.ty
            && ctx.variant_enums.contains(path)
        {
            decl.push(format!("{name}: {path}"));
            types.push((*repr).to_owned());
            args.push(format!("{name}.to_engine().expect(\"this value does not exist on this runtime\")"));
            continue;
        }
        let ty = p.ty.render(safe, false, owner, ctx);
        let Some(target) = ty.strip_prefix('&') else {
            decl.push(format!("{name}: {ty}"));
            types.push(ty);
            args.push(name.clone());
            continue;
        };
        let (pointer, from, null) = match target.strip_prefix("mut ") {
            Some(t) => (format!("*mut {t}"), "from_mut", "null_mut"),
            None => (format!("*const {target}"), "from_ref", "null"),
        };
        if p.nullable {
            decl.push(format!("{name}: Option<{ty}>"));
            args.push(format!("{name}.map_or(core::ptr::{null}(), core::ptr::{from})"));
        } else {
            decl.push(format!("{name}: {ty}"));
            args.push(format!("core::ptr::{from}({name})"));
        }
        types.push(pointer);
    }
    let ffi = match sig.ret.render(safe, true, owner, ctx) {
        r if r.is_empty() => r,
        r => format!(" -> {r}"),
    };
    let call = format!("{callee}({}){ffi})({})", types.join(", "), args.join(", "));
    // An enum result crosses as its integer; a value the enum does not name comes back as `Err`.
    let (result, body) = match &sig.ret {
        Ty::Enum(path, repr) if ctx.variant_enums.contains(path) => (format!(" -> Result<{path}, {repr}>"), format!("let raw = {call}; {path}::from_engine(raw).ok_or(raw)")),
        Ty::Enum(path, repr) => (format!(" -> Result<{path}, {repr}>"), format!("let raw = {call}; {path}::from_{repr}(raw).ok_or(raw)")),
        _ => (ffi, call),
    };
    let indent = if shape == Shape::Free { "" } else { "    " };
    format!("{doc_lines}{indent}#[inline]\n{indent}pub {}fn @NAME@({}){result} {{ unsafe {{ {body} }} }}\n", if safe { "" } else { "unsafe " }, decl.join(", "))
}

/// The displacement of the subobject of `owner` at `offset` on 1.7.104: the literal, or a read of
/// the profile when a base there is not identical on every runtime.
fn base_offset(owner: Option<&str>, offset: u64, ctx: &Ctx) -> String {
    let variant = owner.and_then(|o| ctx.classes.get(o)).and_then(|c| {
        let base = c.bases.iter().find(|&&(b, disp, virt)| !virt && u64::try_from(disp) == Ok(offset) && ctx.variance.base(c.name, b))?;
        Some(format!("{}:{}", c.name, base.0))
    });
    variant.map_or_else(|| format!("{offset:#X}"), |key| format!("base_at!({key:?}, {offset:#X})"))
}

/// What an emitted item is to its name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Callable,
    /// Left out when a callable of the same class has the name.
    Reader,
    /// Renamed when a callable or reader of the class or a class it derefs to has the name.
    Accessor,
}

/// One emitted item before naming: `text` holds `@NAME@` where its unique name goes.
struct Entry {
    tier: Tier,
    role: Role,
    group: String,
    owner: Option<String>,
    order: (u8, String, u64),
    base: String,
    text: String,
}

fn doc_block(indent: &str, lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines {
        if l.is_empty() {
            writeln!(out, "{indent}///").unwrap();
        } else {
            writeln!(out, "{indent}/// {}", doc(l)).unwrap();
        }
    }
    out
}

/// Where the ledger has the function, naming it only when it differs from `shown`.
fn ledger(s: &Symbol, shown: &str) -> String {
    let name = if s.name == shown { String::new() } else { format!(" `{}`", s.name) };
    format!("ledger{name} {} evidence, name {}; id {}, rva {:#x}", s.evidence, s.confidence, s.id.unwrap_or(0), s.rva)
}

const DECOMPILER_SAFETY: &str = "The decompiler's prototype: nothing checks the arguments or the result.";

/// The pass whose classes are the math value types.
const VALUE_PASS: &str = "nimath";

/// What signatures are typed against: the laid-out classes, which of them have RTTI, the semantic enums.
pub(crate) fn context<'a>(inp: &'a Inputs, classes: &'a BTreeMap<&'a str, Class<'a>>, variance: &'a Variance) -> Ctx<'a> {
    let taken: HashSet<&str> = classes.values().map(|c| c.ident.as_str()).collect();
    let name = |e: &str| if taken.contains(e) { format!("{e}Enum") } else { e.to_owned() };
    let mut enums: HashMap<String, (&'static str, String)> = HashMap::from([("FormType".to_owned(), ("u8", name("FormType")))]);
    for pass in &inp.semantic {
        for e in pass.enums.iter().filter(|e| e.name != "FormType") {
            let repr = match e.width {
                1 => "u8",
                2 => "u16",
                4 => "u32",
                _ => "u64",
            };
            enums.insert(e.name.clone(), (repr, name(&e.name)));
        }
    }
    let mut groups: HashMap<&str, &str> = classes.keys().map(|c| (*c, family(c))).collect();
    for pass in &inp.semantic {
        for c in pass.classes.iter().filter(|c| classes.contains_key(c.name.as_str())) {
            groups.insert(c.name.as_str(), pass.subsystem.as_str());
        }
    }
    for c in classes.values() {
        let names: Vec<&str> = std::iter::once(c.name).chain(c.bases.iter().map(|b| b.0)).collect();
        if let Some(group) = hierarchy(&names) {
            groups.insert(c.name, group);
        }
    }
    let values = inp.semantic.iter().filter(|p| p.subsystem == VALUE_PASS).flat_map(|p| &p.classes).map(|c| c.name.as_str()).filter(|c| classes.contains_key(c)).collect();
    let variant_enums = enums.iter().filter(|(e, _)| variance.enum_(e)).map(|(_, (_, name))| format!("crate::{name}")).collect();
    Ctx { classes, rtti: inp.layouts.keys().map(String::as_str).collect(), enums, groups, values, variance, variant_enums }
}

/// A semantic C signature as an `unsafe extern "C" fn` type over raw pointers, `None` when a type
/// in it has no Rust form. Enums stay integers: a detour of this type receives whatever the engine
/// passes.
pub(crate) fn fn_pointer_type(signature: &str, ctx: &Ctx) -> Option<String> {
    let sig = parse_sig(signature, Source::Semantic, ctx).ok()?;
    let ty = |t: &Ty| if let Ty::Enum(_, repr) = t { (*repr).to_owned() } else { t.render(false, false, None, ctx) };
    let params: Vec<String> = sig.params.iter().map(|p| format!("{}: {}", ident(&p.name), ty(&p.ty))).collect();
    let ret = match sig.ret.render(false, true, None, ctx) {
        r if r.is_empty() => r,
        r => format!(" -> {r}"),
    };
    Some(format!("unsafe extern \"C\" fn({}){ret}", params.join(", ")))
}

/// Adds the callable layer to every group's items and says what it holds.
pub(crate) fn emit(inp: &Inputs, ctx: &Ctx, items: &mut Items) -> ApiStats {
    let mut stats = ApiStats::default();
    let classes = ctx.classes;
    let by_rva: HashMap<u64, &Symbol> = inp.symbols.iter().map(|s| (s.rva, s)).collect();
    let by_id: HashMap<u64, &Symbol> = inp.symbols.iter().filter_map(|s| Some((s.id?, s))).collect();
    let mut entries: Vec<Entry> = Vec::new();

    let embedded: HashSet<&str> = classes
        .values()
        .flat_map(|c| &c.fields)
        .filter_map(|f| if let EmitTy::Embed(e) = &f.ty { Some(e.as_str()) } else { None })
        .collect();
    let deref_target = derefs_of(classes, &embedded, &mut stats);
    for (c, p) in &deref_target {
        let (c, p) = (&classes[c], &classes[p]);
        let this = if ctx.variance.base(c.name, p.name) { format!("core::ptr::from_ref(self).byte_add(base_at!({:?}, 0x0))", format!("{}:{}", c.name, p.name)) } else { "core::ptr::from_ref(self)".to_owned() };
        items.entry((ctx.group(c.name).to_owned(), Kind::Api(Tier::Semantic))).or_default().push(format!(
            "/// `{}` is laid out from its primary base `{}`, so its methods and fields are reachable through it.\nimpl core::ops::Deref for {} {{\n    type Target = {};\n    #[inline]\n    fn deref(&self) -> &Self::Target {{ unsafe {{ &*{this}.cast() }} }}\n}}\n",
            c.name,
            p.name,
            c.path(),
            p.path()
        ));
    }
    for c in classes.values() {
        let inherited: HashSet<(&str, i64)> = match deref_target.get(c.name) {
            Some(p) => classes[p].bases.iter().map(|(b, d, _)| (*b, *d)).collect(),
            None => HashSet::new(),
        };
        for &(b, disp, virt) in &c.bases {
            let Some(base) = classes.get(b) else { continue };
            let Ok(off) = u64::try_from(disp) else { continue };
            if virt || off == 0 || inherited.contains(&(b, disp)) || !holds_base(c, base, off, &embedded) {
                continue;
            }
            let text = format!(
                "    /// The `{b}` base at +{off:#X}.\n    #[inline]\n    pub fn @NAME@(&self) -> &{} {{ unsafe {{ &*core::ptr::from_ref(self).byte_add({}).cast() }} }}\n",
                base.path(),
                base_offset(Some(c.name), off, ctx)
            );
            entries.push(Entry { tier: Tier::Semantic, role: Role::Callable, group: ctx.group(c.name).to_owned(), owner: Some(c.name.to_owned()), order: (0, b.to_owned(), off), base: format!("as_{}", snake(b)), text });
            stats.base_accessors += 1;
        }
    }

    let virtual_targets = slots(inp, classes, ctx, &by_rva, &deref_target, &mut entries, &mut stats);
    functions(inp, ctx, &by_id, &virtual_targets, &mut entries, &mut stats);
    let statics = globals(inp, ctx, &mut entries, &mut stats);
    fields(inp, ctx, &mut entries);
    accessors(ctx, &mut entries);

    let mut by_owner: BTreeMap<Option<String>, Vec<Entry>> = BTreeMap::new();
    for e in entries {
        by_owner.entry(e.owner.clone()).or_default().push(e);
    }
    for list in by_owner.values_mut() {
        let taken: HashSet<String> = list.iter().filter(|e| e.role == Role::Callable).map(|e| e.base.clone()).collect();
        list.retain(|e| e.role != Role::Reader || !taken.contains(&e.base));
        stats.field_accessors += list.iter().filter(|e| e.role == Role::Reader).count();
    }
    let names: HashMap<String, HashSet<String>> =
        by_owner.iter().filter_map(|(owner, list)| Some((owner.clone()?, list.iter().filter(|e| e.role != Role::Accessor).map(|e| e.base.clone()).collect()))).collect();
    let mut free: BTreeMap<(String, Tier), Vec<String>> = BTreeMap::new();
    for (owner, mut list) in by_owner {
        if let Some(class) = &owner {
            let chain: Vec<&str> = std::iter::once(class.as_str()).chain(ancestors(class, &deref_target)).collect();
            for e in list.iter_mut().filter(|e| e.role == Role::Accessor) {
                if chain.iter().any(|c| names.get(*c).is_some_and(|n| n.contains(&e.base))) {
                    e.base.push_str("_field");
                }
            }
        }
        list.sort_by(|a, b| (a.tier, &a.order).cmp(&(b.tier, &b.order)));
        let mut names: Vec<String> = list.iter().map(|e| e.base.clone()).collect();
        unique(&mut names);
        let mut blocks: BTreeMap<(String, Tier), String> = BTreeMap::new();
        for (e, name) in list.into_iter().zip(names) {
            let text = e.text.replace("@NAME@", &name);
            match &owner {
                Some(_) => blocks.entry((e.group, e.tier)).or_default().push_str(&text),
                None => free.entry((e.group, e.tier)).or_default().push(text),
            }
        }
        if let Some(owner) = owner {
            for ((group, tier), body) in blocks {
                items.entry((group, Kind::Api(tier))).or_default().push(format!("impl {} {{\n{body}}}\n\n", classes[owner.as_str()].path()));
            }
        }
    }
    for ((group, tier), list) in free.into_iter().chain(statics.into_iter().map(|(g, l)| ((g, Tier::Semantic), l))) {
        items.entry((group, Kind::Api(tier))).or_default().extend(list);
    }
    stats
}

/// Whether a reference to `c` can be read as its `base` subobject at `off`: the struct reaches
/// past the subobject or `c` is partial (a live object holds every base, whatever the ledger knows
/// of its size), and the reference is aligned for the base (the struct is as aligned, or no class
/// embeds `c` by value, so a reference to it is a live engine object at the engine's alignment).
/// The two layouts are not compared field by field: every field type is plain data.
fn holds_base(c: &Class, base: &Class, off: u64, embedded: &HashSet<&str>) -> bool {
    let bytes = off + base.extent() <= c.extent() || !c.exact();
    let aligned = off.is_multiple_of(base.align) && (base.align <= c.align || !embedded.contains(c.name));
    bytes && aligned
}

/// Classes whose primary base can be reached through `Deref`, keyed by class name: the base at
/// offset 0, non-virtual, and held as `holds_base` says.
fn derefs_of<'a>(classes: &BTreeMap<&'a str, Class<'a>>, embedded: &HashSet<&str>, stats: &mut ApiStats) -> BTreeMap<&'a str, &'a str> {
    let mut out = BTreeMap::new();
    for c in classes.values() {
        let Some(p) = c.primary_base.and_then(|p| classes.get(p)) else { continue };
        if c.bases.iter().any(|&(b, disp, virt)| b == p.name && disp == 0 && !virt) && holds_base(c, p, 0, embedded) {
            out.insert(c.name, p.name);
            stats.derefs += 1;
        } else {
            stats.derefs_refused += 1;
        }
    }
    out
}

/// Named slots of each class's primary vtable as methods; returns the `(class, rva)` of every
/// function reached that way.
fn slots(
    inp: &Inputs,
    classes: &BTreeMap<&str, Class>,
    ctx: &Ctx,
    by_rva: &HashMap<u64, &Symbol>,
    deref_target: &BTreeMap<&str, &str>,
    entries: &mut Vec<Entry>,
    stats: &mut ApiStats,
) -> HashSet<(String, u64)> {
    let mut semantic: HashMap<(&str, u64), (&SemSlot, &str)> = HashMap::new();
    for pass in &inp.semantic {
        for vt in &pass.vtables {
            for slot in &vt.slots {
                semantic.insert((vt.class.as_str(), slot.index), (slot, pass.subsystem.as_str()));
            }
        }
    }
    // (class, index) -> (method name, tier) of the slots already emitted, for the derived classes.
    let mut named: HashMap<(&str, usize), (String, Tier)> = HashMap::new();
    let mut targets = HashSet::new();
    let mut order: Vec<&str> = classes.keys().copied().collect();
    order.sort_by_key(|c| (ancestors(c, deref_target).len(), *c));
    for class in order {
        let Some(rtti) = inp.rtti.classes.iter().find(|r| r.name == class) else { continue };
        let Some(col) = rtti.cols.iter().find(|c| c.offset == 0) else { continue };
        for (i, &target) in col.slots.iter().enumerate() {
            let sym = by_rva.get(&target).copied();
            let (tier, engine, sig, lines, safe_conf) = if let Some((slot, pass)) = semantic.get(&(class, i as u64)) {
                let mut lines = vec![
                    format!("`{class}::{}`, vtable slot {i}{}", slot.name, colon(&slot.meaning)),
                    String::new(),
                    format!("Semantic, {pass} pass, {}; called through the object's vtable.", slot.confidence),
                    format!("`{}`", slot.signature),
                ];
                let sig = annotate(parse_sig(&slot.signature, Source::Semantic, ctx), &slot.nullable, slot.retyped.as_deref(), &mut lines);
                if sig.as_ref().is_ok_and(|s| s.params.iter().skip(1).any(|p| matches!(p.ty, Ty::Class(..)) && !p.nullable)) {
                    lines.push("A class parameter without a null mark had no null test in the overrides read; that does not prove the engine never passes null to one.".into());
                }
                (Tier::Semantic, slot.name.clone(), sig, lines, slot.confidence.as_str())
            } else {
                let Some(s) = sym else { continue };
                let (scope, last) = split_scope(&s.name);
                if scope != Some(class) || is_vfunc(last) || is_placeholder(last) || !matches!(s.confidence.as_str(), "high" | "medium") || is_funclet(s) || s.signature.is_empty() {
                    continue;
                }
                let tier = Tier::of(&s.sig_confidence);
                let lines = vec![
                    format!("`{}`, vtable slot {i}, through the object's vtable (decompiler signature, tier {}; {}).", s.name, tier.label(), ledger(s, &s.name)),
                    format!("`{}`", s.signature),
                ];
                (tier, last.to_owned(), parse_sig(&s.signature, Source::Decompiler, ctx), lines, "")
            };
            let sig = match sig.and_then(|s| if s.params.first().is_some_and(|p| p.name == "this") { Ok(s) } else { Err(Unexpressible::Malformed) }) {
                Ok(sig) => sig,
                Err(why) => {
                    stats.left_out(tier, why);
                    continue;
                }
            };
            let base = snake(split_scope(&engine).1);
            if ancestors(class, deref_target).iter().any(|a| named.get(&(*a, i)).is_some_and(|(n, t)| *n == base && *t <= tier)) {
                continue;
            }
            named.insert((class, i), (base.clone(), tier));
            if let Some(s) = sym {
                targets.insert((class.to_owned(), s.rva));
            }
            let reason = if tier == Tier::Semantic { unsafe_reason(&sig, true, safe_conf, &engine, None, ctx) } else { Some(DECOMPILER_SAFETY.to_owned()) };
            let mut lines = lines;
            if let Some(r) = &reason {
                lines.extend([String::new(), "# Safety".into(), r.clone()]);
            }
            let callee = if ctx.variance.slot(class, 0, i as u64) { format!("virtual_function!(self, {class:?}, 0x0, {i}, ") } else { format!("virtual_function!(self, {i}, ") };
            let text = render_call(&doc_block("    ", &lines), Shape::Method(0), reason.is_none(), &sig, &callee, Some(class), ctx);
            let t = stats.tiers.entry(tier).or_default();
            if reason.is_none() { t.safe_slots += 1 } else { t.unsafe_slots += 1 }
            entries.push(Entry { tier, role: Role::Callable, group: ctx.group(class).to_owned(), owner: Some(class.to_owned()), order: (1, engine, i as u64), base, text });
        }
    }
    targets
}

/// Marks the parameters a pass says may be null, and adds to the doc what the pass retyped and
/// those marks.
fn annotate(sig: Result<Sig, Unexpressible>, nullable: &[String], retyped: Option<&str>, lines: &mut Vec<String>) -> Result<Sig, Unexpressible> {
    if let Some(r) = retyped {
        lines.push(format!("Retyped: {r}."));
    }
    if !nullable.is_empty() {
        lines.push(format!("May be null: {}.", nullable.iter().map(|n| format!("`{}`", ident(n))).collect::<Vec<_>>().join(", ")));
    }
    let mut sig = sig?;
    for p in &mut sig.params {
        p.nullable = nullable.contains(&p.name);
    }
    Ok(sig)
}

/// The classes reachable from `class` through `Deref`, nearest first.
fn ancestors<'a>(class: &'a str, deref_target: &BTreeMap<&'a str, &'a str>) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut c = class;
    while let Some(p) = deref_target.get(c) {
        out.push(*p);
        c = p;
    }
    out
}

/// A function to emit: tier, engine name, signature, semantic entry with its pass, ledger symbol.
type Work<'a> = (Tier, &'a str, &'a str, Option<(&'a SemFunction, &'a str)>, &'a Symbol);

fn functions(
    inp: &Inputs,
    ctx: &Ctx,
    by_id: &HashMap<u64, &Symbol>,
    virtual_targets: &HashSet<(String, u64)>,
    entries: &mut Vec<Entry>,
    stats: &mut ApiStats,
) {
    let mut semantic: BTreeMap<u64, (&SemFunction, &str)> = BTreeMap::new();
    for pass in &inp.semantic {
        for f in pass.functions.iter().filter(|f| f.id.is_some()) {
            semantic.insert(f.id.unwrap(), (f, pass.subsystem.as_str()));
        }
    }
    // The offsets of the vtables of each class that hold a function, keyed by (class, rva).
    let mut columns: HashMap<(&str, u64), Vec<u64>> = HashMap::new();
    for class in &inp.rtti.classes {
        for col in &class.cols {
            for &target in &col.slots {
                let offsets = columns.entry((class.name.as_str(), target)).or_default();
                if !offsets.contains(&col.offset) {
                    offsets.push(col.offset);
                }
            }
        }
    }
    let mut work: Vec<Work> = Vec::new();
    for (id, (f, pass)) in &semantic {
        if let Some(s) = by_id.get(id).filter(|_| !is_placeholder(split_scope(&f.name).1)) {
            work.push((Tier::Semantic, f.name.as_str(), f.signature.as_str(), Some((f, pass)), s));
        }
    }
    for s in &inp.symbols {
        if s.kind != "function" || s.id.is_none_or(|id| semantic.contains_key(&id)) || !matches!(s.confidence.as_str(), "high" | "medium") || is_placeholder(&s.name) || is_funclet(s) || s.signature.is_empty() {
            continue;
        }
        if is_vfunc(split_scope(&s.name).1) {
            stats.vfunc_names += 1;
            continue;
        }
        if s.evidence == "crt_init" {
            stats.initializers += 1;
            continue;
        }
        work.push((Tier::of(&s.sig_confidence), s.name.as_str(), s.signature.as_str(), None, s));
    }
    for (tier, name, signature, sem, s) in work {
        let source = if sem.is_some() { Source::Semantic } else { Source::Decompiler };
        let mut notes = Vec::new();
        let sig = parse_sig(signature, source, ctx);
        let sig = match sem {
            Some((f, _)) => annotate(sig, &f.nullable, f.retyped.as_deref(), &mut notes),
            None => sig,
        };
        let Ok(sig) = sig.inspect_err(|why| stats.left_out(tier, *why)) else { continue };
        let (scope, last) = split_scope(name);
        let scope = scope.and_then(|c| ctx.classes.get(c));
        let this = sig.params.first().filter(|p| p.name == "this" || scope.is_some_and(|c| p.base == c.name));
        let (owner, shape) = placement(this, scope, scope.and_then(|c| columns.get(&(c.name, s.rva))));
        if let Some(o) = owner
            && virtual_targets.contains(&(o.to_owned(), s.rva))
        {
            stats.virtual_only += 1;
            continue;
        }
        let id = s.id.unwrap();
        let (mut lines, reason) = match sem {
            Some((f, pass)) => (
                [format!("`{name}`{}", colon(&f.meaning)), String::new(), format!("Semantic, {pass} pass, {}; {}.", f.confidence, ledger(s, name)), format!("`{signature}`")]
                    .into_iter()
                    .chain(notes)
                    .collect(),
                if shape == Shape::Associated && this.is_some() {
                    Some(format!("`this` must point to the part of the object the function takes; the ledger does not say which subobject of `{}` that is.", owner.unwrap_or_default()))
                } else {
                    unsafe_reason(&sig, matches!(shape, Shape::Method(_)), &f.confidence, last, owner, ctx)
                },
            ),
            None => (
                vec![format!("`{name}` (decompiler signature, tier {}; {}).", tier.label(), ledger(s, name)), format!("`{signature}`")],
                Some(DECOMPILER_SAFETY.to_owned()),
            ),
        };
        if let Some(r) = &reason {
            lines.extend([String::new(), "# Safety".into(), r.clone()]);
        }
        let indent = if shape == Shape::Free { "" } else { "    " };
        let text = render_call(&doc_block(indent, &lines), shape, reason.is_none(), &sig, &format!("function!({id}, "), owner, ctx);
        let t = stats.tiers.entry(tier).or_default();
        let safe = reason.is_none();
        match shape {
            Shape::Method(_) if safe => t.safe_methods += 1,
            Shape::Method(_) => t.unsafe_methods += 1,
            Shape::Associated if safe => t.safe_associated += 1,
            Shape::Associated => t.unsafe_associated += 1,
            Shape::Free if safe => t.safe_free += 1,
            Shape::Free => t.unsafe_free += 1,
        }
        let base = if shape == Shape::Free { snake(name) } else { snake(last) };
        let group = match (owner, sem) {
            (Some(o), _) => ctx.group(o),
            (None, Some((_, pass))) => pass,
            (None, None) => family(name),
        };
        entries.push(Entry { tier, role: Role::Callable, group: group.to_owned(), owner: owner.map(str::to_owned), order: (2, name.to_owned(), id), base, text });
    }
}

/// The class a function is a method of and the subobject it takes as `this`, from its `this`
/// parameter and the laid-out class its name is scoped to. A non-virtual base of `scope` the type
/// of `this` names gives its displacement; another laid-out class makes the function that class's.
/// Otherwise a function only a secondary vtable of `scope` holds (`columns`, the offsets of the
/// vtables holding it) is an override reached through that base and takes the subobject at the
/// vtable's offset; one the primary vtable holds, or a function of the class in no vtable, takes
/// the object. When that is unknown (several secondary vtables, or a type naming no class and no
/// vtable holding the function), `this` stays a parameter of an associated function.
fn placement<'a>(this: Option<&'a Param>, scope: Option<&'a Class>, columns: Option<&Vec<u64>>) -> (Option<&'a str>, Shape) {
    let class_of_this = this.and_then(|p| if let Ty::Class(c, _) = &p.ty { Some(c.as_str()) } else { None });
    let (Some(p), Some(class)) = (this, scope) else {
        return match (this, scope, class_of_this) {
            (_, Some(c), _) => (Some(c.name), Shape::Associated),
            (_, None, Some(t)) => (Some(t), Shape::Method(0)),
            _ => (None, Shape::Free),
        };
    };
    let named = |c: &str| c == p.base || split_scope(c).1 == p.base;
    let own = named(class.name);
    if !own {
        let mut bases = class.bases.iter().filter(|&&(b, _, virt)| !virt && named(b)).map(|&(_, disp, _)| disp);
        if let Some(disp) = bases.next() {
            return match u64::try_from(disp) {
                Ok(offset) if bases.all(|d| d == disp) => (Some(class.name), Shape::Method(offset)),
                _ => (Some(class.name), Shape::Associated),
            };
        }
        if let Some(t) = class_of_this {
            return (Some(t), Shape::Method(0));
        }
    }
    let shape = match columns.map(Vec::as_slice) {
        Some([offset]) => Shape::Method(*offset),
        Some(offsets) if !offsets.contains(&0) => Shape::Associated,
        Some(_) => Shape::Method(0),
        None if own => Shape::Method(0),
        None => Shape::Associated,
    };
    (Some(class.name), shape)
}

/// `Option<&X>` readers for the pointer fields a pass names with high confidence on its own class;
/// a derived class reaches them through `Deref`.
fn fields(inp: &Inputs, ctx: &Ctx, entries: &mut Vec<Entry>) {
    for pass in &inp.semantic {
        for sc in &pass.classes {
            let Some(class) = ctx.classes.get(sc.name.as_str()) else { continue };
            for sf in sc.fields.iter().filter(|f| f.confidence == "high") {
                let Some(fo) = class.fields.iter().find(|fo| fo.f.off == sf.offset && fo.f.name == sf.name) else { continue };
                let EmitTy::Ptr(target) = &fo.ty else { continue };
                let lines = [
                    format!("`{}::{}` at +{:#X}{}", sc.name, sf.name, sf.offset, colon(&sf.meaning)),
                    String::new(),
                    format!("Semantic field, {} pass, high; `None` while the pointer is null.", pass.subsystem),
                ];
                let value = if ctx.variance.member(class.name, &fo.f.name) { format!("Self::__member_{}(core::ptr::from_ref(self)).read_unaligned()", fo.ident) } else { format!("{{ self.{} }}", fo.ident) };
                let text = format!(
                    "{}    #[inline]
    pub fn @NAME@(&self) -> Option<&{}> {{ unsafe {{ {value}.as_ref() }} }}
",
                    doc_block("    ", &lines),
                    ctx.classes[target.as_str()].path()
                );
                entries.push(Entry { tier: Tier::Semantic, role: Role::Reader, group: ctx.group(class.name).to_owned(), owner: Some(class.name.to_owned()), order: (3, sf.name.clone(), sf.offset), base: snake(&sf.name), text });
            }
        }
    }
}

/// What takes the place of a member that is not identical on every runtime: a hidden pointer function
/// holding the one read of the profile, and over it the value or a reference to it, a setter, a pointer.
fn accessors(ctx: &Ctx, entries: &mut Vec<Entry>) {
    for class in ctx.classes.values() {
        let group = ctx.group(class.name);
        let mut push = |order: String, base: String, text: String| {
            entries.push(Entry { tier: Tier::Semantic, role: Role::Accessor, group: group.to_owned(), owner: Some(class.name.to_owned()), order: (4, order, 0), base, text });
        };
        for field in class.fields.iter().filter(|f| ctx.variance.member(class.name, &f.f.name)) {
            let name = &field.ident;
            let ty = ty_str(&field.ty, ctx.classes);
            let at = format!("Self::__member_{name}");
            let key = format!("{}.{}", class.name, field.f.name);
            push(name.clone(), format!("__member_{name}"), format!("    #[doc(hidden)]\n    #[inline]\n    pub fn @NAME@(this: *const Self) -> *mut {ty} {{ unsafe {{ member!(this, {key:?}, {:#X}, {ty}) }} }}\n", field.f.off));
            let head = format!("    /// `{}::{}`, at +{:#X} on 1.7.104.\n    #[inline]\n", class.name, field.f.name, field.f.off);
            if let EmitTy::Embed(target) = &field.ty {
                if field.f.off.is_multiple_of(ctx.classes[target.as_str()].align) {
                    push(name.clone(), name.clone(), format!("{head}    pub fn @NAME@(&self) -> &{ty} {{ unsafe {{ &*{at}(core::ptr::from_ref(self)) }} }}\n"));
                }
            } else {
                push(name.clone(), name.clone(), format!("{head}    pub fn @NAME@(&self) -> {ty} {{ unsafe {{ {at}(core::ptr::from_ref(self)).read_unaligned() }} }}\n"));
                push(name.clone(), format!("set_{name}"), format!("    #[inline]\n    pub fn @NAME@(&mut self, value: {ty}) {{ unsafe {{ {at}(core::ptr::from_mut(self).cast_const()).write_unaligned(value) }} }}\n"));
            }
            push(name.clone(), format!("{name}_ptr"), format!("    #[inline]\n    pub fn @NAME@(&self) -> *mut {ty} {{ {at}(core::ptr::from_ref(self)) }}\n"));
        }
    }
}

/// Accessors for the globals: `singleton()` on a class with exactly one `g_*_ptr` label, or from its
/// `g_<Class>_ptr` label when it has several, the rest as functions keyed by group: the semantic pass's, else the group of the class the global holds
/// or points to, else the group of its name.
fn globals(inp: &Inputs, ctx: &Ctx, entries: &mut Vec<Entry>, stats: &mut ApiStats) -> BTreeMap<String, Vec<String>> {
    let mut semantic: HashMap<u64, (&SemGlobal, &str)> = HashMap::new();
    for pass in &inp.semantic {
        for g in pass.globals.iter().filter(|g| g.id.is_some()) {
            semantic.insert(g.id.unwrap(), (g, pass.subsystem.as_str()));
        }
    }
    let labels: HashMap<u64, _> = inp.structs.labels.iter().filter(|l| ctx.classes.contains_key(l.cls.as_str())).map(|l| (l.rva, l)).collect();
    let mut singles: HashMap<&str, usize> = HashMap::new();
    for l in labels.values().filter(|l| l.pointer && l.name.ends_with("_ptr")) {
        *singles.entry(l.cls.as_str()).or_default() += 1;
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut list: Vec<&Symbol> = inp.symbols.iter().filter(|s| s.kind == "global" && s.id.is_some()).collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let mut idents: Vec<String> = list.iter().map(|s| snake(&s.name)).collect();
    unique(&mut idents);
    for (s, name) in list.into_iter().zip(idents) {
        let id = s.id.unwrap();
        let label = labels.get(&s.rva);
        let sem = semantic.get(&id);
        let mut lines = vec![format!("`{}` at {:#x}, id {id} ({}, {})", s.name, s.rva, s.evidence, s.confidence)];
        if let Some(l) = label {
            write!(lines[0], "; {} `{}`: {}", if l.pointer { "pointer to" } else { "a" }, l.cls, l.detail).unwrap();
        }
        if let Some((g, pass)) = sem {
            write!(lines[0], "; {} ({}, {pass} pass), type `{}`", g.meaning, g.confidence, g.ty).unwrap();
        }
        // What the global holds: an instance of a class, a pointer to one, or something else at the address.
        let (instance, pointee, raw) = match (sem, label) {
            (Some((g, _)), _) => {
                let ty = g.ty.as_str();
                if let Some(c) = ty.strip_prefix("class:").filter(|c| ctx.classes.contains_key(c)) {
                    (Some(c.to_owned()), None, String::new())
                } else if let Some(c) = ty.strip_prefix("ptr:").filter(|c| ctx.classes.contains_key(c)) {
                    (None, Some(c.to_owned()), String::new())
                } else {
                    match c_type(ty, Source::Semantic, ctx) {
                        Ok(Ty::Class(c, _)) => (None, Some(c), String::new()),
                        Ok(Ty::Scalar(t)) => (None, None, format!("*mut {t}")),
                        Ok(Ty::Bool) => (None, None, "*mut bool".into()),
                        Ok(Ty::Raw(r)) => (None, None, format!("*mut {r}")),
                        _ if ctx.classes.contains_key(ty) => (Some(ty.to_owned()), None, String::new()),
                        _ if matches!(ty, "pointer" | "ptr") => (None, None, "*mut *mut c_void".into()),
                        _ => (None, None, "*mut c_void".into()),
                    }
                }
            }
            (None, Some(l)) if l.pointer && singles.contains_key(l.cls.as_str()) && l.name.ends_with("_ptr") => (None, Some(l.cls.clone()), String::new()),
            (None, Some(l)) if l.pointer => (None, None, format!("*mut *mut {}", ctx.classes[l.cls.as_str()].path())),
            (None, Some(l)) => (None, None, format!("*mut {}", ctx.classes[l.cls.as_str()].path())),
            (None, None) => (None, None, "*mut c_void".into()),
        };
        let (ret, body) = match (&instance, &pointee) {
            (Some(c), _) => (format!("&'static {}", ctx.classes[c.as_str()].path()), format!("unsafe {{ &*(address!({id}) as *const {}) }}", ctx.classes[c.as_str()].path())),
            (_, Some(c)) => {
                let ty = format!("Option<&'static {}>", ctx.classes[c.as_str()].path());
                (ty.clone(), format!("unsafe {{ *(address!({id}) as *const {ty}) }}"))
            }
            _ => (raw.clone(), format!("address!({id}) as {raw}")),
        };
        if let Some(c) = &pointee
            && label.is_some_and(|l| l.pointer && l.name.ends_with("_ptr") && l.cls == *c && (singles.get(c.as_str()) == Some(&1) || l.name == format!("g_{c}_ptr")))
        {
            lines.push(String::new());
            lines.push("`None` until the engine creates it.".into());
            let text = format!("{}    #[inline]\n    pub fn @NAME@() -> Option<&'static Self> {{ unsafe {{ *(address!({id}) as *const Option<&'static Self>) }} }}\n", doc_block("    ", &lines));
            entries.push(Entry { tier: Tier::Semantic, role: Role::Callable, group: ctx.group(c).to_owned(), owner: Some(c.clone()), order: (0, String::new(), id), base: "singleton".into(), text });
            stats.singletons += 1;
            continue;
        }
        let group = match (sem, instance.as_ref().or(pointee.as_ref()), label) {
            (Some((_, pass)), _, _) => pass,
            (None, Some(c), _) => ctx.group(c),
            (None, None, Some(l)) => ctx.group(&l.cls),
            (None, None, None) => family(s.name.strip_prefix("g_").unwrap_or(&s.name)),
        };
        out.entry(group.to_owned()).or_default().push(format!("{}#[inline]\npub fn {name}() -> {ret} {{ {body} }}\n", doc_block("", &lines)));
        stats.statics += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::test_classes;

    fn ctx<'a>(classes: &'a BTreeMap<&'a str, Class<'a>>) -> Ctx<'a> {
        Ctx { classes, rtti: ["TESForm"].into_iter().collect(), enums: HashMap::from([("ActorValue".to_owned(), ("u32", "ActorValue".to_owned()))]), groups: HashMap::new(), values: HashSet::new(), variance: Box::leak(Box::default()), variant_enums: HashSet::new() }
    }

    #[test]
    fn names_are_snake_case() {
        assert_eq!(snake("GetFormEditorID"), "get_form_editor_id");
        assert_eq!(snake("FindFormByID"), "find_form_by_id");
        assert_eq!(snake("BSPlatform::Foo"), "bs_platform__foo");
        assert_eq!(snake("Move"), "move_");
        assert_eq!(snake("vfunc_9"), "vfunc_9");
        assert_eq!(split_scope("BSTEventSource<A::B>::Notify"), (Some("BSTEventSource<A::B>"), "Notify"));
        assert_eq!(split_scope("FindFormByID"), (None, "FindFormByID"));
    }

    #[test]
    fn signatures_parse() {
        let classes = test_classes();
        let ctx = ctx(&classes);
        let sig = parse_sig("TESFile* TESDataHandler::GetFile(TESDataHandler* this, const char* name)", Source::Semantic, &ctx).unwrap();
        assert_eq!(sig.ret, Ty::Class("TESFile".into(), false));
        assert_eq!(sig.params[0].ty, Ty::Class("TESDataHandler".into(), false));
        assert_eq!(sig.params[1].ty, Ty::Raw("*const c_char".into()));
        let sig = parse_sig("bool (const TESForm* this, ActorValue av, void (*cb)(void*, int))", Source::Semantic, &ctx).unwrap();
        assert_eq!(sig.ret, Ty::Bool);
        assert_eq!(sig.params[0].ty, Ty::Class("TESForm".into(), true));
        assert_eq!(sig.params[1].ty, Ty::Enum("crate::ActorValue".into(), "u32"));
        assert_eq!(sig.params[2].name, "cb");
        let sig = parse_sig("undefined FUN(TESForm * this, undefined8 * param_1, bool param_2, BSTArray<Foo,Bar> * p)", Source::Decompiler, &ctx).unwrap();
        assert_eq!(sig.ret, Ty::Scalar("u64"));
        assert_eq!(sig.params[1].ty, Ty::Raw("*mut u64".into()));
        assert_eq!(sig.params[2].ty, Ty::Scalar("u8"));
        assert_eq!(sig.params[3].ty, Ty::Raw("*mut c_void".into()));
        assert_eq!(parse_sig("void (InputEvent* const* events)", Source::Semantic, &ctx).unwrap().params[0].ty, Ty::Raw("*mut *mut c_void".into()));
        assert_eq!(parse_sig("void Print(const char* fmt, ...)", Source::Semantic, &ctx).err(), Some(Unexpressible::Varargs));
        assert_eq!(parse_sig("void Set(NiPoint3 value)", Source::Semantic, &ctx).err(), Some(Unexpressible::UnknownByValue));
        assert_eq!(parse_sig("void Set(TESFile file)", Source::Semantic, &ctx).err(), Some(Unexpressible::ClassByValue));
        assert_eq!(parse_sig("void (BGSMoviePlayer* this+0x58)", Source::Semantic, &ctx).err(), Some(Unexpressible::Malformed));
        let sig = parse_sig("bool (this, TESForm forms[6] /* six */, u8 out[3], s32 n, HWND window)  /* note */", Source::Semantic, &ctx).unwrap();
        assert_eq!(sig.params[0].ty, Ty::Raw("*mut c_void".into()));
        assert_eq!(sig.params[1].ty, Ty::Class("TESForm".into(), false));
        assert_eq!(sig.params[2].ty, Ty::Raw("*mut u8".into()));
        assert_eq!(sig.params[3].ty, Ty::Scalar("i32"));
        assert_eq!(sig.params[4].ty, Ty::Raw("*mut c_void".into()));
    }

    #[test]
    fn enum_results_are_typed() {
        let classes = test_classes();
        let ctx = ctx(&classes);
        let sig = parse_sig("ActorValue TESForm::Get(TESForm* this)", Source::Semantic, &ctx).unwrap();
        let text = render_call("", Shape::Method(0), true, &sig, "function!(1, ", Some("TESForm"), &ctx);
        assert!(text.contains("-> Result<crate::ActorValue, u32> { unsafe { let raw = function!(1, (*const Self) -> u32)(core::ptr::from_ref(self)); crate::ActorValue::from_u32(raw).ok_or(raw) } }"), "{text}");
    }

    #[test]
    fn safety_follows_the_rule() {
        let classes = test_classes();
        let ctx = ctx(&classes);
        let check = |sig: &str, conf: &str| {
            let (_, last) = split_scope(sig.split('(').next().unwrap().rsplit(' ').next().unwrap());
            unsafe_reason(&parse_sig(sig, Source::Semantic, &ctx).unwrap(), true, conf, last, Some("TESDataHandler"), &ctx)
        };
        assert_eq!(check("bool TESDataHandler::IsReady(TESDataHandler* this, uint index, float f, ActorValue av)", "high"), None);
        assert_eq!(check("TESFile* TESDataHandler::First(TESDataHandler* this, TESForm* form)", "high"), None);
        assert_eq!(check("const char* TESDataHandler::Name(TESDataHandler* this)", "high"), None);
        assert!(check("bool TESDataHandler::IsReady(TESDataHandler* this)", "medium").is_some());
        assert!(check("void TESDataHandler::Get(TESDataHandler* this, const char* name)", "high").is_some());
        assert!(check("void TESDataHandler::Put(TESDataHandler* this, TESFile* file)", "high").is_some());
        assert_eq!(check("void TESDataHandler::Put(TESDataHandler* this, const TESFile* file)", "high"), None);
        assert!(check("void* TESDataHandler::Data(TESDataHandler* this)", "high").is_some());
        assert!(check("TESDataHandler* TESDataHandler::ctor(TESDataHandler* this)", "high").is_some());
        assert!(check("void TESDataHandler::vdtor(TESDataHandler* this, uint flags)", "high").is_some());
    }
}
