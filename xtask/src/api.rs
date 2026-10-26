//! The callable layer of the layout crate: engine functions as methods on their classes, named
//! vtable slots, base-class conversions and global accessors.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

use crate::emit::{Class, HEADER, KEYWORDS, doc, ident, is_funclet, is_placeholder, screaming, unique};
use crate::inputs::{Inputs, SemFunction, SemGlobal, SemSlot, Symbol};

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

    #[must_use]
    pub fn module(self) -> &'static str {
        self.file().trim_end_matches(".rs")
    }

    fn file(self) -> &'static str {
        match self {
            Self::Semantic => "api.rs",
            Self::High => "api_tier_high.rs",
            Self::Medium => "api_tier_medium.rs",
            Self::Low => "api_tier_low.rs",
            Self::Untiered => "api_untiered.rs",
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
#[derive(Default, Clone, Copy)]
pub struct TierStats {
    pub safe_methods: usize,
    pub unsafe_methods: usize,
    pub safe_associated: usize,
    pub unsafe_associated: usize,
    pub safe_free: usize,
    pub unsafe_free: usize,
    pub safe_slots: usize,
    pub unsafe_slots: usize,
    /// Functions or slots left out because a type in the signature has no Rust form.
    pub unexpressible: usize,
}

#[derive(Default)]
pub struct ApiStats {
    pub tiers: BTreeMap<Tier, TierStats>,
    pub derefs: usize,
    pub derefs_refused: usize,
    pub base_accessors: usize,
    pub singletons: usize,
    pub statics: usize,
    /// Functions reached as a named slot of the same class, so not emitted as a direct call.
    pub virtual_only: usize,
    /// `vfunc_<n>` names, which say only that the function sits in a vtable.
    pub vfunc_names: usize,
    /// CRT static initializers: they run once before `main`, and a second call re-initializes their global.
    pub initializers: usize,
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
    ty: Ty,
}

struct Sig {
    ret: Ty,
    params: Vec<Param>,
}

pub(crate) struct Ctx<'a> {
    pub(crate) classes:&'a BTreeMap<&'a str, Class<'a>>,
    rtti: HashSet<&'a str>,
    enums: HashMap<String, &'static str>,
}

fn scalar(base: &str) -> Option<&'static str> {
    Some(match base {
        "uchar" | "u8" | "uint8" | "byte" | "undefined1" => "u8",
        "char" => "c_char",
        "i8" | "int8" | "schar" => "i8",
        "ushort" | "u16" | "uint16" | "undefined2" | "wchar_t" | "WORD" | "u_short" => "u16",
        "short" | "i16" | "int16" => "i16",
        "uint" | "u32" | "uint32" | "undefined4" | "DWORD" | "UINT" | "ulong" | "u_int" => "u32",
        "int" | "i32" | "int32" | "long" | "LONG" | "BOOL" | "HRESULT" | "errno_t" => "i32",
        "ulonglong" | "u64" | "uint64" | "undefined8" | "undefined" => "u64",
        "longlong" | "i64" | "int64" | "__int64" => "i64",
        "size_t" | "SIZE_T" | "rsize_t" | "uintptr_t" | "usize" => "usize",
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

/// A C type as the generator types it, `None` when it has no Rust form (a class or unknown type by value).
fn c_type(text: &str, source: Source, ctx: &Ctx) -> Option<Ty> {
    let text = text.trim();
    let constant = text.starts_with("const ");
    let words = text.replace("const*", "*").replace("const&", "&");
    let words: Vec<&str> = words.split_whitespace().filter(|w| *w != "const").collect();
    let t = words.join(" ");
    let base = t.trim_end_matches(['*', '&', ' ']);
    let depth = t[base.len()..].chars().filter(|c| matches!(c, '*' | '&')).count();
    let raw = |inner: &str| Some(Ty::Raw(pointer(inner, depth, constant)));
    match base {
        "void" | "undefined" | "code" if depth > 0 => return raw("c_void"),
        "void" => return Some(Ty::Void),
        "bool" if source == Source::Semantic => return if depth == 0 { Some(Ty::Bool) } else { raw("bool") },
        "bool" => return if depth == 0 { Some(Ty::Scalar("u8")) } else { raw("u8") },
        _ => {}
    }
    if let Some(s) = scalar(base) {
        return if depth == 0 { Some(Ty::Scalar(s)) } else { raw(s) };
    }
    if source == Source::Semantic
        && let Some(repr) = ctx.enums.get(base)
    {
        return if depth == 0 { Some(Ty::Enum(format!("enums::{base}"), repr)) } else { raw(&format!("enums::{base}")) };
    }
    if let Some(class) = ctx.classes.get(base) {
        return match depth {
            0 => None,
            1 => Some(Ty::Class(base.to_owned(), constant)),
            _ => raw(&class.path()),
        };
    }
    if depth == 0 { None } else { raw("c_void") }
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

/// A C prototype, with or without the function's name: `ret [name](type name, ...)`.
fn parse_sig(sig: &str, source: Source, ctx: &Ctx) -> Option<Sig> {
    let body = sig.trim().strip_suffix(')')?;
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
    let open = open?;
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
                return None;
            }
            if let Some((_, rest)) = p.split_once("(*") {
                params.push(Param { name: rest.split(')').next()?.trim().to_owned(), ty: Ty::Raw("*const c_void".into()) });
                continue;
            }
            let (ty, name) = match p.rfind(|c: char| c.is_whitespace() || c == '*' || c == '&') {
                Some(at) => (&p[..=at], p[at + 1..].trim()),
                None => (p, ""),
            };
            let name = if name.is_empty() { format!("a{i}") } else { name.to_owned() };
            if !is_ident(&name) {
                return None;
            }
            params.push(Param { name, ty: c_type(ty, source, ctx)? });
        }
    }
    Some(Sig { ret, params })
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
    if last == "ctor" || last.to_ascii_lowercase().contains("dtor") || last.starts_with('~') || class.is_some_and(|c| split_scope(c).1 == last) {
        return Some("It constructs or destroys an object in place.".into());
    }
    let raw: Vec<String> = sig
        .params
        .iter()
        .skip(usize::from(method))
        .filter(|p| match &p.ty {
            Ty::Scalar(_) | Ty::Bool | Ty::Enum(..) => false,
            Ty::Class(c, constant) => !(*constant || ctx.rtti.contains(c.as_str())),
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
    Method,
    Associated,
    Free,
}

/// `pub [unsafe] fn NAME(params) -> ret { unsafe { call(args) } }` with `@NAME@` standing for the name.
fn render_call(doc_lines: &str, shape: Shape, safe: bool, sig: &Sig, callee: &str, owner: Option<&str>, ctx: &Ctx) -> String {
    let rest = &sig.params[usize::from(shape == Shape::Method)..];
    let mut names: Vec<String> = rest.iter().map(|p| ident(&p.name)).collect();
    unique(&mut names);
    let mut decl: Vec<String> = Vec::new();
    let mut types: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if shape == Shape::Method {
        decl.push("&self".into());
        types.push("&Self".into());
        args.push("self".into());
    }
    for (p, name) in rest.iter().zip(&names) {
        let ty = p.ty.render(safe, false, owner, ctx);
        decl.push(format!("{name}: {ty}"));
        types.push(ty);
        args.push(name.clone());
    }
    let result = match sig.ret.render(safe, true, owner, ctx) {
        r if r.is_empty() => r,
        r => format!(" -> {r}"),
    };
    let indent = if shape == Shape::Free { "" } else { "    " };
    format!(
        "{doc_lines}{indent}#[inline]\n{indent}pub {}fn @NAME@({}){result} {{ unsafe {{ {callee}({}){result})({}) }} }}\n",
        if safe { "" } else { "unsafe " },
        decl.join(", "),
        types.join(", "),
        args.join(", ")
    )
}

/// One emitted item before naming: `text` holds `@NAME@` where its unique name goes.
struct Entry {
    tier: Tier,
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

/// What signatures are typed against: the laid-out classes, which of them have RTTI, the semantic enums.
pub(crate) fn context<'a>(inp: &'a Inputs, classes: &'a BTreeMap<&'a str, Class<'a>>) -> Ctx<'a> {
    let mut enums: HashMap<String, &'static str> = HashMap::from([("FormType".to_owned(), "u8")]);
    for pass in &inp.semantic {
        for e in pass.enums.iter().filter(|e| e.name != "FormType") {
            let repr = match e.width {
                1 => "u8",
                2 => "u16",
                4 => "u32",
                _ => "u64",
            };
            enums.insert(e.name.clone(), repr);
        }
    }
    Ctx { classes, rtti: inp.layouts.keys().map(String::as_str).collect(), enums }
}

/// A semantic C signature as an `unsafe extern "C" fn` type over raw pointers, `None` when a type
/// in it has no Rust form.
pub(crate) fn fn_pointer_type(signature: &str, ctx: &Ctx) -> Option<String> {
    let sig = parse_sig(signature, Source::Semantic, ctx)?;
    let params: Vec<String> = sig.params.iter().map(|p| format!("{}: {}", ident(&p.name), p.ty.render(false, false, None, ctx))).collect();
    let ret = match sig.ret.render(false, true, None, ctx) {
        r if r.is_empty() => r,
        r => format!(" -> {r}"),
    };
    Some(format!("unsafe extern \"C\" fn({}){ret}", params.join(", ")))
}

/// Everything the callable layer emits: the files keyed by name, and what they hold.
pub(crate) fn emit(inp: &Inputs, ctx: &Ctx) -> (BTreeMap<&'static str, String>, ApiStats) {
    let mut stats = ApiStats::default();
    let classes = ctx.classes;
    let by_rva: HashMap<u64, &Symbol> = inp.symbols.iter().map(|s| (s.rva, s)).collect();
    let by_id: HashMap<u64, &Symbol> = inp.symbols.iter().filter_map(|s| Some((s.id?, s))).collect();
    let mut entries: Vec<Entry> = Vec::new();
    let mut derefs = String::new();

    let embedded: HashSet<&str> = classes
        .values()
        .flat_map(|c| &c.fields)
        .filter_map(|f| if let crate::emit::Ty::Embed(e) = &f.ty { Some(e.as_str()) } else { None })
        .collect();
    let deref_target = derefs_of(classes, &embedded, &mut stats);
    for (c, p) in &deref_target {
        let (c, p) = (&classes[c], &classes[p]);
        writeln!(
            derefs,
            "/// `{}` is laid out from its primary base `{}`, so its methods and fields are reachable through it.\nimpl core::ops::Deref for {} {{\n    type Target = {};\n    #[inline]\n    fn deref(&self) -> &Self::Target {{ unsafe {{ &*core::ptr::from_ref(self).cast() }} }}\n}}\n",
            c.name,
            p.name,
            c.path(),
            p.path()
        )
        .unwrap();
    }
    for c in classes.values() {
        let inherited: HashSet<(&str, i64)> = match deref_target.get(c.name) {
            Some(p) => classes[p].bases.iter().map(|(b, d, _)| (*b, *d)).collect(),
            None => HashSet::new(),
        };
        for &(b, disp, virt) in &c.bases {
            let Some(base) = classes.get(b) else { continue };
            let Ok(off) = u64::try_from(disp) else { continue };
            if virt || off == 0 || inherited.contains(&(b, disp)) || off % base.align != 0 || !holds_alignment(c, base.align, &embedded) || off + base.extent() > c.extent() {
                continue;
            }
            let text = format!(
                "    /// The `{b}` base at +{off:#X}.\n    #[inline]\n    pub fn @NAME@(&self) -> &{} {{ unsafe {{ &*core::ptr::from_ref(self).byte_add({off:#X}).cast() }} }}\n",
                base.path()
            );
            entries.push(Entry { tier: Tier::Semantic, owner: Some(c.name.to_owned()), order: (0, b.to_owned(), off), base: format!("as_{}", snake(b)), text });
            stats.base_accessors += 1;
        }
    }

    let virtual_targets = slots(inp, classes, ctx, &by_rva, &deref_target, &mut entries, &mut stats);
    functions(inp, ctx, &by_id, &virtual_targets, &mut entries, &mut stats);
    let statics = globals(inp, ctx, &mut entries, &mut stats);

    let mut files = BTreeMap::new();
    let mut groups: BTreeMap<(Tier, Option<String>), Vec<Entry>> = BTreeMap::new();
    let mut by_owner: BTreeMap<Option<String>, Vec<Entry>> = BTreeMap::new();
    for e in entries {
        by_owner.entry(e.owner.clone()).or_default().push(e);
    }
    for (owner, mut list) in by_owner {
        list.sort_by(|a, b| (a.tier, &a.order).cmp(&(b.tier, &b.order)));
        let mut names: Vec<String> = list.iter().map(|e| e.base.clone()).collect();
        unique(&mut names);
        for (mut e, name) in list.into_iter().zip(names) {
            e.text = e.text.replace("@NAME@", &name);
            groups.entry((e.tier, owner.clone())).or_default().push(e);
        }
    }
    for tier in Tier::ALL {
        let mut out = String::from(HEADER);
        out.push_str("use core::ffi::{c_char, c_void};\n\nuse super::{classes, enums};\n\n");
        if tier == Tier::Semantic {
            out.push_str(&derefs);
        }
        for ((_, owner), list) in groups.iter().filter(|((t, o), _)| *t == tier && o.is_some()) {
            let owner = owner.as_deref().unwrap();
            writeln!(out, "impl {} {{", classes[owner].path()).unwrap();
            for e in list {
                out.push_str(&e.text);
            }
            out.push_str("}\n\n");
        }
        out.push_str("/// Functions of no laid-out class; `super::functions` re-exports them.\npub mod functions {\n    use super::*;\n\n");
        for e in groups.get(&(tier, None)).into_iter().flatten() {
            out.push_str(&e.text);
        }
        out.push_str("}\n");
        files.insert(tier.file(), out);
    }
    files.insert("statics.rs", statics);
    (files, stats)
}

/// Whether every reference to `c` is aligned to `align`: it is when the struct is, and when no
/// class embeds `c` by value, since then a reference to it is a live engine object, which the
/// engine allocates at its natural alignment even where the ledger packs the struct.
fn holds_alignment(c: &Class, align: u64, embedded: &HashSet<&str>) -> bool {
    align <= c.align || !embedded.contains(c.name)
}

/// Classes whose primary base can be reached through `Deref`, keyed by class name: the base at
/// offset 0, no larger than the class, every base field at the same offset and width in the
/// class, and every reference to the class aligned for the base.
fn derefs_of<'a>(classes: &BTreeMap<&'a str, Class<'a>>, embedded: &HashSet<&str>, stats: &mut ApiStats) -> BTreeMap<&'a str, &'a str> {
    let mut out = BTreeMap::new();
    for c in classes.values() {
        let Some(p) = c.primary_base.and_then(|p| classes.get(p)) else { continue };
        let at_zero = c.bases.iter().any(|&(b, disp, virt)| b == p.name && disp == 0 && !virt);
        let prefix = p.fields.iter().all(|pf| c.fields.iter().any(|cf| cf.f.off == pf.f.off && cf.f.size == pf.f.size));
        if at_zero && prefix && p.extent() <= c.extent() && holds_alignment(c, p.align, embedded) {
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
                let lines = vec![
                    format!("`{class}::{}`, vtable slot {i}: {}", slot.name, slot.meaning),
                    String::new(),
                    format!("Semantic, {pass} pass, {}; called through the object's vtable.", slot.confidence),
                    format!("`{}`", slot.signature),
                ];
                (Tier::Semantic, slot.name.clone(), parse_sig(&slot.signature, Source::Semantic, ctx), lines, slot.confidence.as_str())
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
            let Some(sig) = sig.filter(|s| s.params.first().is_some_and(|p| p.name == "this")) else {
                stats.tiers.entry(tier).or_default().unexpressible += 1;
                continue;
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
            let text = render_call(&doc_block("    ", &lines), Shape::Method, reason.is_none(), &sig, &format!("virtual_function!(self, {i}, "), Some(class), ctx);
            let t = stats.tiers.entry(tier).or_default();
            if reason.is_none() { t.safe_slots += 1 } else { t.unsafe_slots += 1 }
            entries.push(Entry { tier, owner: Some(class.to_owned()), order: (1, engine, i as u64), base, text });
        }
    }
    targets
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
        let Some(sig) = parse_sig(signature, source, ctx) else {
            stats.tiers.entry(tier).or_default().unexpressible += 1;
            continue;
        };
        let (scope, last) = split_scope(name);
        let scope = scope.filter(|c| ctx.classes.contains_key(c));
        let this = sig.params.first().filter(|p| p.name == "this" || matches!(&p.ty, Ty::Class(c, _) if Some(c.as_str()) == scope));
        let owner = match this.map(|p| &p.ty) {
            Some(Ty::Class(c, _)) => Some(c.as_str()),
            _ => scope,
        };
        let shape = match (this.is_some(), owner) {
            (true, Some(_)) => Shape::Method,
            (false, Some(_)) => Shape::Associated,
            (_, None) => Shape::Free,
        };
        let owner = if shape == Shape::Free { None } else { owner };
        if let Some(o) = owner
            && virtual_targets.contains(&(o.to_owned(), s.rva))
        {
            stats.virtual_only += 1;
            continue;
        }
        let id = s.id.unwrap();
        let (mut lines, reason) = match sem {
            Some((f, pass)) => (
                vec![
                    format!("`{name}`: {}", f.meaning),
                    String::new(),
                    format!("Semantic, {pass} pass, {}; {}.", f.confidence, ledger(s, name)),
                    format!("`{signature}`"),
                ],
                unsafe_reason(&sig, shape == Shape::Method, &f.confidence, last, owner, ctx),
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
            Shape::Method if safe => t.safe_methods += 1,
            Shape::Method => t.unsafe_methods += 1,
            Shape::Associated if safe => t.safe_associated += 1,
            Shape::Associated => t.unsafe_associated += 1,
            Shape::Free if safe => t.safe_free += 1,
            Shape::Free => t.unsafe_free += 1,
        }
        let base = if shape == Shape::Free { snake(name) } else { snake(last) };
        entries.push(Entry { tier, owner: owner.map(str::to_owned), order: (2, name.to_owned(), id), base, text });
    }
}

/// Accessors for the globals: `singleton()` on a class with exactly one `g_*_ptr` label, the rest
/// in `statics.rs`.
fn globals(inp: &Inputs, ctx: &Ctx, entries: &mut Vec<Entry>, stats: &mut ApiStats) -> String {
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
    let mut out = String::from(HEADER);
    out.push_str("//! Every global with an Address Library id, as an accessor; `singleton()` on a class replaces the accessor of its one `g_*_ptr` global.\n\nuse core::ffi::{c_char, c_void};\n\nuse super::{classes, enums};\n\n");
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
                        Some(Ty::Class(c, _)) => (None, Some(c), String::new()),
                        Some(Ty::Scalar(t)) => (None, None, format!("*mut {t}")),
                        Some(Ty::Bool) => (None, None, "*mut bool".into()),
                        Some(Ty::Raw(r)) => (None, None, format!("*mut {r}")),
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
            && label.is_some_and(|l| l.pointer && l.name.ends_with("_ptr") && l.cls == *c)
            && singles.get(c.as_str()) == Some(&1)
        {
            lines.push(String::new());
            lines.push("`None` until the engine creates it.".into());
            let text = format!("{}    #[inline]\n    pub fn @NAME@() -> Option<&'static Self> {{ unsafe {{ *(address!({id}) as *const Option<&'static Self>) }} }}\n", doc_block("    ", &lines));
            entries.push(Entry { tier: Tier::Semantic, owner: Some(c.clone()), order: (0, String::new(), id), base: "singleton".into(), text });
            stats.singletons += 1;
            continue;
        }
        write!(out, "{}#[inline]\npub fn {name}() -> {ret} {{ {body} }}\n", doc_block("", &lines)).unwrap();
        stats.statics += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::test_classes;

    fn ctx<'a>(classes: &'a BTreeMap<&'a str, Class<'a>>) -> Ctx<'a> {
        Ctx { classes, rtti: ["TESForm"].into_iter().collect(), enums: HashMap::from([("ActorValue".to_owned(), "u32")]) }
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
        assert_eq!(sig.params[1].ty, Ty::Enum("enums::ActorValue".into(), "u32"));
        assert_eq!(sig.params[2].name, "cb");
        let sig = parse_sig("undefined FUN(TESForm * this, undefined8 * param_1, bool param_2, BSTArray<Foo,Bar> * p)", Source::Decompiler, &ctx).unwrap();
        assert_eq!(sig.ret, Ty::Scalar("u64"));
        assert_eq!(sig.params[1].ty, Ty::Raw("*mut u64".into()));
        assert_eq!(sig.params[2].ty, Ty::Scalar("u8"));
        assert_eq!(sig.params[3].ty, Ty::Raw("*mut c_void".into()));
        assert_eq!(parse_sig("void (InputEvent* const* events)", Source::Semantic, &ctx).unwrap().params[0].ty, Ty::Raw("*mut *mut c_void".into()));
        assert!(parse_sig("void Print(const char* fmt, ...)", Source::Semantic, &ctx).is_none());
        assert!(parse_sig("void Set(NiPoint3 value)", Source::Semantic, &ctx).is_none());
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
