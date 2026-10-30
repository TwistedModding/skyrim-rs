//! Per-plugin settings: typed values under string keys, kept by the host for the life of the game
//! (a reloaded plugin finds them unchanged) and stored as a flat JSON object in
//! `Data\SKSE\Plugins\skyrim-rs\<plugin>.settings.json`, read before the plugin's `load` and
//! written within a second of a change and when the plugin is unloaded.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::Table;

/// One stored value.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    Bool(bool),
    Number(f64),
    Text(String),
}

impl From<bool> for Setting {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<f64> for Setting {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}

impl From<f32> for Setting {
    /// Stored as the shortest decimal that reads back as `v` (0.1, not 0.10000000149011612).
    fn from(v: f32) -> Self {
        Self::Number(v.to_string().parse().unwrap_or(f64::from(v)))
    }
}

impl From<i32> for Setting {
    fn from(v: i32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<u32> for Setting {
    fn from(v: u32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<String> for Setting {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<&str> for Setting {
    fn from(v: &str) -> Self {
        Self::Text(v.to_owned())
    }
}

/// A type a setting can be read back as.
pub trait Value: Sized {
    fn from_setting(setting: &Setting) -> Option<Self>;
}

impl Value for bool {
    fn from_setting(s: &Setting) -> Option<Self> {
        if let Setting::Bool(v) = s { Some(*v) } else { None }
    }
}

impl Value for f64 {
    fn from_setting(s: &Setting) -> Option<Self> {
        if let Setting::Number(v) = s { Some(*v) } else { None }
    }
}

impl Value for f32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_setting(s: &Setting) -> Option<Self> {
        f64::from_setting(s).map(|v| v as f32)
    }
}

impl Value for i32 {
    #[allow(clippy::cast_possible_truncation)]
    fn from_setting(s: &Setting) -> Option<Self> {
        f64::from_setting(s).filter(|v| v.fract() == 0.0 && *v >= f64::from(i32::MIN) && *v <= f64::from(i32::MAX)).map(|v| v as i32)
    }
}

impl Value for u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn from_setting(s: &Setting) -> Option<Self> {
        f64::from_setting(s).filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= f64::from(u32::MAX)).map(|v| v as u32)
    }
}

impl Value for String {
    fn from_setting(s: &Setting) -> Option<Self> {
        if let Setting::Text(v) = s { Some(v.clone()) } else { None }
    }
}

/// One plugin's settings, from [`crate::Host::settings`].
#[derive(Clone, Copy)]
pub struct Settings {
    owner: &'static str,
    table: &'static Table,
}

impl Settings {
    pub(crate) const fn new(owner: &'static str, table: &'static Table) -> Self {
        Self { owner, table }
    }

    /// The value under `key`, `None` when it is unset or of another type.
    #[must_use]
    pub fn get<T: Value>(&self, key: &str) -> Option<T> {
        (self.table.setting)(self.owner, key).as_ref().and_then(T::from_setting)
    }

    /// The value under `key`, or `default` when it is unset or of another type.
    #[must_use]
    pub fn get_or<T: Value>(&self, key: &str, default: T) -> T {
        self.get(key).unwrap_or(default)
    }

    /// Stores `value` under `key`; the file is written within a second. A number that is not
    /// finite is not stored.
    pub fn set(&self, key: &str, value: impl Into<Setting>) {
        let value = value.into();
        if !matches!(value, Setting::Number(n) if !n.is_finite()) {
            (self.table.set_setting)(self.owner, key, value);
        }
    }

    /// Every key that starts with `prefix`, in order (`""` lists them all).
    #[must_use]
    pub fn keys(&self, prefix: &str) -> Vec<String> {
        (self.table.setting_keys)(self.owner, prefix)
    }

    /// Removes every key that starts with `prefix` (a whole key removes that one); returns how
    /// many went. The file is written within a second.
    #[must_use = "the count says whether anything was there"]
    pub fn remove(&self, prefix: &str) -> usize {
        (self.table.remove_settings)(self.owner, prefix)
    }
}

struct Store {
    path: PathBuf,
    values: BTreeMap<String, Setting>,
    dirty: bool,
}

static STORES: Mutex<BTreeMap<String, Store>> = Mutex::new(BTreeMap::new());

fn stores() -> std::sync::MutexGuard<'static, BTreeMap<String, Store>> {
    STORES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The settings file of `owner` in `dir`; characters a file name cannot hold become `_`.
pub(crate) fn file(dir: &Path, owner: &str) -> PathBuf {
    let stem: String = owner.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    dir.join(format!("{stem}.settings.json"))
}

/// Reads `owner`'s file once; a later call (a reload) keeps the values in memory. Returns how many
/// values were read, or why the file was not used. A file that does not parse is renamed to
/// `<file>.bad` and replaced by the values set from then on, written at the next flush; a file that
/// cannot be read is left alone until a value changes.
pub(crate) fn open(owner: &str, dir: &Path) -> Result<usize, String> {
    let mut stores = stores();
    if let Some(store) = stores.get(owner) {
        return Ok(store.values.len());
    }
    let path = file(dir, owner);
    let (values, dirty, read) = match std::fs::read_to_string(&path).map(|text| decode(&text)) {
        Ok(Ok(values)) => {
            let count = values.len();
            (values, false, Ok(count))
        }
        Ok(Err(e)) => {
            let bad = path.with_extension("json.bad");
            match std::fs::rename(&path, &bad) {
                Ok(()) => (BTreeMap::new(), true, Err(format!("{}: malformed at {e}; renamed to {}, the defaults are used", path.display(), bad.display()))),
                Err(r) => (BTreeMap::new(), false, Err(format!("{}: malformed at {e}; not renamed ({r}), the defaults are used and the file is left alone until a value changes", path.display()))),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (BTreeMap::new(), false, Ok(0)),
        Err(e) => (BTreeMap::new(), false, Err(format!("{}: {e}", path.display()))),
    };
    stores.insert(owner.to_owned(), Store { path, values, dirty });
    read
}

pub(crate) fn get(owner: &str, key: &str) -> Option<Setting> {
    stores().get(owner)?.values.get(key).cloned()
}

pub(crate) fn set(owner: &str, key: &str, value: Setting) {
    if let Some(store) = stores().get_mut(owner)
        && store.values.get(key) != Some(&value)
    {
        store.values.insert(key.to_owned(), value);
        store.dirty = true;
    }
}

pub(crate) fn keys(owner: &str, prefix: &str) -> Vec<String> {
    stores().get(owner).map(|store| store.values.range(prefix.to_owned()..).map(|(k, _)| k).take_while(|k| k.starts_with(prefix)).cloned().collect()).unwrap_or_default()
}

pub(crate) fn remove(owner: &str, prefix: &str) -> usize {
    let mut stores = stores();
    let Some(store) = stores.get_mut(owner) else { return 0 };
    let before = store.values.len();
    store.values.retain(|k, _| !k.starts_with(prefix));
    let removed = before - store.values.len();
    store.dirty |= removed > 0;
    removed
}

/// Writes every changed store, or only `owner`'s; returns each file written or the error.
pub(crate) fn flush(owner: Option<&str>) -> Vec<Result<PathBuf, String>> {
    let mut stores = stores();
    let mut written = Vec::new();
    for (name, store) in stores.iter_mut() {
        if !store.dirty || owner.is_some_and(|o| o != name) {
            continue;
        }
        store.dirty = false;
        written.push(std::fs::write(&store.path, encode(&store.values)).map(|()| store.path.clone()).map_err(|e| format!("{}: {e}", store.path.display())));
    }
    written
}

fn quote(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode(values: &BTreeMap<String, Setting>) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in values.iter().enumerate() {
        out.push_str(if i == 0 { "\n  " } else { ",\n  " });
        quote(&mut out, key);
        out.push_str(": ");
        match value {
            Setting::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Setting::Number(v) => {
                let _ = write!(out, "{v}");
            }
            Setting::Text(v) => quote(&mut out, v),
        }
    }
    out.push_str(if values.is_empty() { "}\n" } else { "\n}\n" });
    out
}

/// A JSON value as [`parse_json`] reads it; an object keeps its members in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The member `key` of an object; `None` for other values.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn write(&self, out: &mut String, depth: usize) {
        let indent = |out: &mut String, depth: usize| {
            out.push('\n');
            out.extend(core::iter::repeat_n('\t', depth));
        };
        match self {
            Self::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Self::Number(v) if v.is_finite() => {
                let _ = write!(out, "{v}");
            }
            Self::Null | Self::Number(_) => out.push_str("null"),
            Self::Text(v) => quote(out, v),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Object(members) if members.is_empty() => out.push_str("{}"),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    out.push_str(if i == 0 { "" } else { "," });
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                }
                indent(out, depth);
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    out.push_str(if i == 0 { "" } else { "," });
                    indent(out, depth + 1);
                    quote(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

/// The document as text, one tab per level; a number that is not finite is written as `null`.
/// [`parse_json`] reads it back.
impl core::fmt::Display for Json {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut out = String::new();
        self.write(&mut out, 0);
        f.write_str(&out)
    }
}

/// Reads a whole JSON document, such as another program's configuration a plugin imports. A
/// byte order mark is allowed; an error names its line and column.
pub fn parse_json(text: &str) -> Result<Json, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { chars: text.char_indices().peekable(), text };
    let value = p.value()?;
    p.end().map(|()| value)
}

/// A flat JSON object of booleans, numbers and strings; `null` values are skipped. A byte order
/// mark (Notepad and `PowerShell` write one) is allowed. An error names its line and column.
fn decode(text: &str) -> Result<BTreeMap<String, Setting>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser { chars: text.char_indices().peekable(), text };
    let mut values = BTreeMap::new();
    p.expect('{')?;
    if p.eat('}') {
        return p.end().map(|()| values);
    }
    loop {
        let key = p.string()?;
        p.expect(':')?;
        p.skip();
        let at = p.position();
        let value = match p.value()? {
            Json::Null => None,
            Json::Bool(v) => Some(Setting::Bool(v)),
            Json::Number(v) => Some(Setting::Number(v)),
            Json::Text(v) => Some(Setting::Text(v)),
            Json::Array(_) | Json::Object(_) => return Err(p.error(at, "expected a boolean, number or string")),
        };
        if let Some(value) = value {
            values.insert(key, value);
        }
        if p.eat('}') {
            return p.end().map(|()| values);
        }
        p.expect(',')?;
    }
}

struct Parser<'a> {
    chars: core::iter::Peekable<core::str::CharIndices<'a>>,
    text: &'a str,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip();
        self.chars.next_if(|&(_, x)| x == c).is_some()
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            let at = self.position();
            Err(self.error(at, &format!("expected '{c}'")))
        }
    }

    fn position(&mut self) -> usize {
        self.chars.peek().map_or(self.text.len(), |&(i, _)| i)
    }

    /// `what` at the byte `at`, as a line and a column counted from 1.
    fn error(&self, at: usize, what: &str) -> String {
        let before = &self.text[..at];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or_default().chars().count() + 1;
        format!("line {line}, column {column}: {what}")
    }

    fn end(&mut self) -> Result<(), String> {
        self.skip();
        let at = self.position();
        if at == self.text.len() { Ok(()) } else { Err(self.error(at, "unexpected text after the object")) }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut out = String::new();
        let unterminated = |p: &Self| p.error(p.text.len(), "unterminated string");
        loop {
            let (_, c) = self.chars.next().ok_or_else(|| unterminated(self))?;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let (at, e) = self.chars.next().ok_or_else(|| unterminated(self))?;
                    out.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let hex: String = (0..4).filter_map(|_| self.chars.next().map(|(_, c)| c)).collect();
                            u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).ok_or_else(|| self.error(at, "bad \\u escape"))?
                        }
                        other => other,
                    });
                }
                c => out.push(c),
            }
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip();
        let start = self.position();
        match self.chars.peek().map(|&(_, c)| c) {
            Some('"') => return self.string().map(Json::Text),
            Some('{') => {
                self.chars.next();
                let mut members = Vec::new();
                if !self.eat('}') {
                    loop {
                        let key = self.string()?;
                        self.expect(':')?;
                        members.push((key, self.value()?));
                        if self.eat('}') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                return Ok(Json::Object(members));
            }
            Some('[') => {
                self.chars.next();
                let mut items = Vec::new();
                if !self.eat(']') {
                    loop {
                        items.push(self.value()?);
                        if self.eat(']') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                return Ok(Json::Array(items));
            }
            Some('t' | 'f' | 'n') => {
                while self.chars.next_if(|(_, c)| c.is_ascii_alphabetic()).is_some() {}
                return match &self.text[start..self.position()] {
                    "true" => Ok(Json::Bool(true)),
                    "false" => Ok(Json::Bool(false)),
                    "null" => Ok(Json::Null),
                    other => Err(self.error(start, &format!("unknown value {other}"))),
                };
            }
            _ => {}
        }
        while self.chars.next_if(|(_, c)| c.is_ascii_digit() || "+-.eE".contains(*c)).is_some() {}
        let end = self.position();
        self.text[start..end].parse::<f64>().map(Json::Number).map_err(|_| self.error(start, "expected a value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_document_reads_back() {
        let doc = Json::Object(vec![
            ("version".to_owned(), Json::Number(1.0)),
            ("name".to_owned(), Json::Text("a \"b\"\n".to_owned())),
            ("pos".to_owned(), Json::Array(vec![Json::Number(0.5), Json::Number(-2.0), Json::Number(f64::NAN)])),
            ("bones".to_owned(), Json::Object(vec![("NPC Head [Head]".to_owned(), Json::Object(vec![("scale".to_owned(), Json::Bool(true))]))])),
            ("empty".to_owned(), Json::Array(Vec::new())),
            ("none".to_owned(), Json::Object(Vec::new())),
        ]);
        let text = doc.to_string();
        assert!(text.starts_with("{\n\t\"version\": 1,\n\t\"name\""), "{text}");
        let back = parse_json(&text).unwrap();
        assert_eq!(back.get("pos"), Some(&Json::Array(vec![Json::Number(0.5), Json::Number(-2.0), Json::Null])));
        assert_eq!(back.get("name"), doc.get("name"));
        assert_eq!(back.get("bones"), doc.get("bones"));
        assert_eq!(back.get("empty"), Some(&Json::Array(Vec::new())));
        assert_eq!(back.get("none"), Some(&Json::Object(Vec::new())));
        assert_eq!(Json::Number(1.0).get("x"), None);
    }

    #[test]
    fn round_trips_through_the_file_format() {
        let mut values = BTreeMap::new();
        values.insert("scale".to_owned(), Setting::Number(1.5));
        values.insert("count".to_owned(), Setting::Number(3.0));
        values.insert("on".to_owned(), Setting::Bool(true));
        values.insert("name \"x\"".to_owned(), Setting::Text("a\\b\nc\u{1}é".to_owned()));
        let text = encode(&values);
        assert!(text.contains("\"count\": 3,"), "{text}");
        assert_eq!(decode(&text).unwrap(), values);
        assert_eq!(decode("{}").unwrap(), BTreeMap::new());
        assert_eq!(decode(" { \"a\" : null , \"b\":-2e1 } ").unwrap().get("b"), Some(&Setting::Number(-20.0)));
        assert_eq!(decode("{\"a\": 1"), Err("line 1, column 8: expected ','".to_owned()));
        assert_eq!(decode("{\"a\": 1,\n  \"b\" true\n}"), Err("line 2, column 7: expected ':'".to_owned()));
        assert_eq!(decode("{\"a\": yes}"), Err("line 1, column 7: expected a value".to_owned()));
        assert_eq!(decode("{\"a\": nope}"), Err("line 1, column 7: unknown value nope".to_owned()));
        assert_eq!(decode("{\"a\": \"x"), Err("line 1, column 9: unterminated string".to_owned()));
        assert!(decode("{\"a\": 1} x").is_err());
        assert_eq!(decode("\u{feff}{\"a\": 1}").unwrap().len(), 1);
        assert_eq!(decode("{\"a\": [1]}"), Err("line 1, column 7: expected a boolean, number or string".to_owned()));
    }

    #[test]
    fn parses_nested_json() {
        let doc = parse_json("\u{feff}{\"General\": [{\"on\": true, \"x\": -1.5}], \"name\": \"Default\", \"e\": [], \"o\": {}, \"n\": null}").unwrap();
        let general = Json::Array(vec![Json::Object(vec![("on".to_owned(), Json::Bool(true)), ("x".to_owned(), Json::Number(-1.5))])]);
        assert_eq!(
            doc,
            Json::Object(vec![
                ("General".to_owned(), general),
                ("name".to_owned(), Json::Text("Default".to_owned())),
                ("e".to_owned(), Json::Array(Vec::new())),
                ("o".to_owned(), Json::Object(Vec::new())),
                ("n".to_owned(), Json::Null),
            ])
        );
        assert_eq!(parse_json("[1, 2"), Err("line 1, column 6: expected ','".to_owned()));
        assert_eq!(parse_json("{\"a\": [1,\n x]}"), Err("line 2, column 2: expected a value".to_owned()));
        assert!(parse_json("[] x").is_err());
    }

    #[test]
    fn lists_and_removes_keys_by_prefix() {
        let owner = "settings-test-prefix";
        let dir = std::env::temp_dir().join(format!("skyrim-rs-settings-prefix-{}", std::process::id()));
        assert_eq!(open(owner, &dir), Ok(0));
        for key in ["Profiles.A.x", "Profiles.A.y", "Profiles.B.x", "Profilesx", "other"] {
            set(owner, key, Setting::Bool(true));
        }
        assert_eq!(keys(owner, "Profiles."), ["Profiles.A.x", "Profiles.A.y", "Profiles.B.x"]);
        assert_eq!(keys(owner, "").len(), 5);
        assert_eq!(keys("settings-test-none", ""), Vec::<String>::new());
        assert_eq!(remove(owner, "Profiles.A."), 2);
        assert_eq!(remove(owner, "Profiles.A."), 0);
        assert_eq!(keys(owner, "Profiles"), ["Profiles.B.x", "Profilesx"]);
        assert_eq!(get(owner, "Profiles.A.x"), None);
        assert_eq!(remove(owner, "other"), 1);
    }

    #[test]
    fn converts_between_types() {
        assert_eq!(i32::from_setting(&Setting::Number(4.0)), Some(4));
        assert_eq!(i32::from_setting(&Setting::Number(4.5)), None);
        assert_eq!(u32::from_setting(&Setting::Number(-1.0)), None);
        assert_eq!(f32::from_setting(&Setting::from(0.25f32)), Some(0.25));
        assert_eq!(bool::from_setting(&Setting::Number(1.0)), None);
        assert_eq!(String::from_setting(&Setting::from("x")), Some("x".to_owned()));
    }

    #[test]
    fn stores_survive_reopening_and_write_when_changed() {
        let dir = std::env::temp_dir().join(format!("skyrim-rs-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let owner = "settings-test/one";
        let path = file(&dir, owner);
        assert!(path.ends_with("settings-test_one.settings.json"));
        std::fs::write(&path, "{\"kept\": 7}").unwrap();
        assert_eq!(open(owner, &dir), Ok(1));
        set(owner, "kept", Setting::Number(7.0));
        assert_eq!(flush(Some(owner)), Vec::new());
        set(owner, "added", Setting::Bool(false));
        assert_eq!(open(owner, &dir), Ok(2));
        assert_eq!(flush(Some(owner)), [Ok(path.clone())]);
        assert_eq!(decode(&std::fs::read_to_string(&path).unwrap()).unwrap().len(), 2);
        assert!(open("settings-test-bad", &dir).is_ok());
        let broken = file(&dir, "settings-test-broken");
        std::fs::write(&broken, "{").unwrap();
        let error = open("settings-test-broken", &dir).unwrap_err();
        assert!(error.contains("malformed at line 1, column 2: expected '\"'") && error.contains("renamed to"), "{error}");
        assert_eq!(std::fs::read_to_string(broken.with_extension("json.bad")).unwrap(), "{");
        assert_eq!(get("settings-test-broken", "x"), None);
        set("settings-test-broken", "x", Setting::Bool(true));
        assert_eq!(flush(Some("settings-test-broken")), [Ok(broken.clone())]);
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "{\n  \"x\": true\n}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
