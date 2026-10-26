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
/// values were read, or why the file was not used (it is then left alone until a value changes).
pub(crate) fn open(owner: &str, dir: &Path) -> Result<usize, String> {
    let mut stores = stores();
    if let Some(store) = stores.get(owner) {
        return Ok(store.values.len());
    }
    let path = file(dir, owner);
    let read = match std::fs::read_to_string(&path) {
        Ok(text) => decode(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    };
    let count = read.as_ref().map_or(0, BTreeMap::len);
    stores.insert(owner.to_owned(), Store { path, values: read.clone().unwrap_or_default(), dirty: false });
    read.map(|_| count)
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

/// A flat JSON object of booleans, numbers and strings; `null` values are skipped.
fn decode(text: &str) -> Result<BTreeMap<String, Setting>, String> {
    let mut p = Parser { chars: text.char_indices().peekable(), text };
    let mut values = BTreeMap::new();
    p.expect('{')?;
    if p.eat('}') {
        return p.end().map(|()| values);
    }
    loop {
        let key = p.string()?;
        p.expect(':')?;
        if let Some(value) = p.value()? {
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
        if self.eat(c) { Ok(()) } else { Err(format!("expected '{c}' at byte {}", self.position())) }
    }

    fn position(&mut self) -> usize {
        self.chars.peek().map_or(self.text.len(), |&(i, _)| i)
    }

    fn end(&mut self) -> Result<(), String> {
        self.skip();
        if self.chars.peek().is_none() { Ok(()) } else { Err(format!("unexpected text at byte {}", self.position())) }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let (_, c) = self.chars.next().ok_or("unterminated string")?;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let (_, e) = self.chars.next().ok_or("unterminated string")?;
                    out.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let hex: String = (0..4).filter_map(|_| self.chars.next().map(|(_, c)| c)).collect();
                            u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).ok_or("bad \\u escape")?
                        }
                        other => other,
                    });
                }
                c => out.push(c),
            }
        }
    }

    fn value(&mut self) -> Result<Option<Setting>, String> {
        self.skip();
        let start = self.position();
        match self.chars.peek().map(|&(_, c)| c) {
            Some('"') => return self.string().map(|s| Some(Setting::Text(s))),
            Some('t' | 'f' | 'n') => {
                while self.chars.next_if(|(_, c)| c.is_ascii_alphabetic()).is_some() {}
                return match &self.text[start..self.position()] {
                    "true" => Ok(Some(Setting::Bool(true))),
                    "false" => Ok(Some(Setting::Bool(false))),
                    "null" => Ok(None),
                    other => Err(format!("unknown value {other} at byte {start}")),
                };
            }
            _ => {}
        }
        while self.chars.next_if(|(_, c)| c.is_ascii_digit() || "+-.eE".contains(*c)).is_some() {}
        let number = &self.text[start..self.position()];
        number.parse::<f64>().map(|n| Some(Setting::Number(n))).map_err(|_| format!("bad value at byte {start}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(decode("{\"a\": 1").is_err());
        assert!(decode("{\"a\": yes}").is_err());
        assert!(decode("{\"a\": 1} x").is_err());
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
        std::fs::write(file(&dir, "settings-test-broken"), "{").unwrap();
        assert!(open("settings-test-broken", &dir).is_err());
        assert_eq!(get("settings-test-broken", "x"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
