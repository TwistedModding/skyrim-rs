//! The binding table: every hotkey of the host and its plugins as `<owner>.<action>` with its
//! default and current key, stored in the host's settings as `key.<owner>.<action>` (a key name,
//! a scan code number on read, or empty for unbound; removing the entry restores the default) and handed to the overlay. Two actions never share a key: a set
//! that would is refused, and a key the file gives a second action leaves that action unbound.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use overlay::{Hotkey, KeyUse, Scope};
use skyrim::prelude::*;
use sre::{InputContextEnum, InputDevice};

use super::NAME;
use crate::settings::{self, Setting, Value};

/// The host's own actions, declared first so they keep their keys against any plugin's.
pub(super) const TOGGLE: usize = 0;
pub(super) const CLOSE: usize = 1;

#[derive(Clone)]
pub(super) struct Binding {
    /// `<owner>.<action>`.
    pub name: String,
    pub owner: String,
    pub default: Option<Hotkey>,
    pub key: Option<Hotkey>,
    pub scope: Scope,
    /// What the game's control map does with the key, once the control map is loaded.
    pub game: String,
}

impl Binding {
    pub fn action(&self) -> &str {
        &self.name[self.owner.len() + 1..]
    }
}

static BINDINGS: Mutex<Vec<Binding>> = Mutex::new(Vec::new());
/// The last assignment or refusal, for the Settings tab and `SkyrimRs keys`.
static LAST: Mutex<String> = Mutex::new(String::new());
/// The binding a capture is armed for, `usize::MAX` for none.
static CAPTURE_FOR: AtomicUsize = AtomicUsize::new(usize::MAX);

fn bindings() -> MutexGuard<'static, Vec<Binding>> {
    BINDINGS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn setting_key(name: &str) -> String {
    format!("key.{name}")
}

pub(super) fn show(key: Option<Hotkey>) -> String {
    key.map_or_else(|| "unbound".to_owned(), |k| k.to_string())
}

fn holder(list: &[Binding], key: Hotkey, except: usize) -> Option<&str> {
    list.iter().enumerate().find(|(i, b)| *i != except && b.key == Some(key)).map(|(_, b)| b.name.as_str())
}

/// Declares the host's toggle and close keys and gives the overlay's sink the host's key listener
/// and input filters; before the overlay is installed.
pub(super) fn declare_host() {
    declare(NAME, "toggle", &overlay::DEFAULT_TOGGLE.to_string(), Scope::Overlay);
    declare(NAME, "close", &overlay::DEFAULT_CLOSE.to_string(), Scope::Overlay);
    overlay::on_key(on_key);
    overlay::filter_game_buttons(super::filters::swallows);
}

/// Declares `<owner>.<action>`, or finds it again for a reloaded plugin, and returns its index.
pub(super) fn declare(owner: &str, action: &str, default: &str, scope: Scope) -> usize {
    let name = format!("{owner}.{action}");
    let default_key = match default.parse::<Hotkey>() {
        _ if default.is_empty() => None,
        Ok(key) => Some(key),
        Err(e) => {
            report(format!("{name}: the default {default:?} is not a key: {e}"));
            None
        }
    };
    let mut list = bindings();
    if let Some(i) = list.iter().position(|b| b.name == name) {
        list[i].default = default_key;
        list[i].scope = scope;
        drop(list);
        apply();
        return i;
    }
    let stored = settings::get(NAME, &setting_key(&name));
    let read = match &stored {
        None => Ok(default_key),
        Some(Setting::Text(text)) if text.is_empty() => Ok(None),
        Some(Setting::Text(text)) => text.parse::<Hotkey>().map(Some),
        Some(other) => u32::from_setting(other).filter(|code| (1..0x100).contains(code)).map(|code| Some(Hotkey::new(code))).ok_or_else(|| format!("{other:?} is not a key name or scan code")),
    };
    let wanted = read.unwrap_or_else(|e| {
        report(format!("{} in the settings: {e}; {name} keeps its default {}", setting_key(&name), show(default_key)));
        default_key
    });
    let key = match wanted.and_then(|k| holder(&list, k, usize::MAX).map(|other| (k, other.to_owned()))) {
        Some((k, other)) => {
            report(format!("{name}: {k} is bound to {other}; {name} stays unbound until a key is set"));
            None
        }
        None => wanted,
    };
    if let Some(k) = key
        && stored != Some(Setting::Text(k.to_string()))
    {
        settings::set(NAME, &setting_key(&name), Setting::Text(k.to_string()));
    }
    list.push(Binding { name, owner: owner.to_owned(), default: default_key, key, scope, game: String::new() });
    let index = list.len() - 1;
    drop(list);
    describe_game_use(index);
    apply();
    index
}

/// Binds `key` to the action at `index` unless another action holds it; the outcome is logged and kept as the last message.
pub(super) fn set(index: usize, key: Hotkey) -> Result<String, String> {
    let mut list = bindings();
    let name = list.get(index).map(|b| b.name.clone()).ok_or_else(|| format!("no key {index}"))?;
    let result = if let Some(other) = holder(&list, key, index) {
        Err(format!("{key} for {name} refused: {key} is bound to {other}"))
    } else {
        list[index].key = Some(key);
        settings::set(NAME, &setting_key(&name), Setting::Text(key.to_string()));
        Ok(format!("{name} set to {key}"))
    };
    drop(list);
    if result.is_ok() {
        describe_game_use(index);
        apply();
    }
    match &result {
        Ok(line) | Err(line) => report(line.clone()),
    }
    result
}

/// Leaves the action at `index` without a key, also on later runs.
pub(super) fn unbind(index: usize) {
    let mut list = bindings();
    let Some(binding) = list.get_mut(index) else { return };
    binding.key = None;
    let name = binding.name.clone();
    settings::set(NAME, &setting_key(&name), Setting::Text(String::new()));
    drop(list);
    describe_game_use(index);
    apply();
    report(format!("{name} unbound"));
}

/// Logs `line` and keeps it as the last message.
fn report(line: String) {
    log!("keys: {line}");
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = line;
}

/// Hands the toggle, the close key and the overlay actions' keys to the overlay.
fn apply() {
    let list = bindings();
    overlay::set_toggle(list.get(TOGGLE).and_then(|b| b.key));
    overlay::set_close(list.get(CLOSE).and_then(|b| b.key));
    overlay::set_overlay_keys(list.iter().skip(CLOSE + 1).filter(|b| b.scope == Scope::Overlay).filter_map(|b| b.key).collect());
}

pub(super) fn list() -> Vec<Binding> {
    bindings().clone()
}

pub(super) fn last_message() -> String {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

pub(super) fn fired(index: usize) -> bool {
    let binding = bindings().get(index).map(|b| (b.key, b.scope));
    binding.is_some_and(|(key, scope)| key.is_some_and(|key| overlay::pressed(key, scope)))
}

pub(super) fn key(index: usize) -> Option<Hotkey> {
    bindings().get(index).and_then(|b| b.key)
}

/// Whether the action's key and its modifiers are down in the game.
pub(super) fn held(index: usize) -> bool {
    key(index).is_some_and(|k| key_held(k, overlay::game_key_held))
}

/// The seconds the action's key was held when it was released in the game this frame; the
/// modifiers are not asked for, since they may be let go first.
pub(super) fn released(index: usize) -> Option<f32> {
    key(index).and_then(|k| overlay::game_key_released(k.scan_code))
}

fn key_held(key: Hotkey, down: impl Fn(u32) -> bool) -> bool {
    let either = |a, b| down(a) || down(b);
    down(key.scan_code) && (!key.shift || either(0x2A, 0x36)) && (!key.ctrl || either(0x1D, 0x9D)) && (!key.alt || either(0x38, 0xB8))
}

/// Arms the capture of the next key pressed for the action at `index`, or disarms it with `None`.
pub(super) fn capture(index: Option<usize>) {
    CAPTURE_FOR.store(index.unwrap_or(usize::MAX), Ordering::Relaxed);
    overlay::capture_next_key(index.is_some());
}

/// The action a capture is armed for.
pub(super) fn capturing() -> Option<usize> {
    let index = CAPTURE_FOR.load(Ordering::Relaxed);
    (index != usize::MAX && overlay::capturing()).then_some(index)
}

fn on_key(used: KeyUse) {
    match used {
        KeyUse::Toggled(key, open) => log!("overlay {} with {key}", if open { "opened" } else { "closed" }),
        KeyUse::Closed(key) => log!("overlay closed with {key}"),
        KeyUse::FocusReleased(key) => log!("{key} released the focused widget; the overlay stays open"),
        KeyUse::Captured(key) => {
            let index = CAPTURE_FOR.swap(usize::MAX, Ordering::Relaxed);
            if index != usize::MAX {
                let _ = set(index, key);
            }
        }
    }
}

fn context_name(context: u32) -> String {
    InputContextEnum::from_u32(context).map_or_else(|| format!("context {context}"), |c| format!("{c:?}"))
}

/// What the game's control map does with a key in every context that maps it, empty before it is loaded.
fn game_use(key: Hotkey) -> String {
    let uses = sre::g_control_map().user_events(InputDevice::Keyboard, key.scan_code);
    uses.iter()
        .map(|(context, event, modifier)| if *modifier == 0 { format!("{} {event}", context_name(*context)) } else { format!("{} {event} (with {:#04X})", context_name(*context), modifier) })
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_game_use(index: usize) {
    if !super::data_loaded() {
        return;
    }
    let mut list = bindings();
    if let Some(b) = list.get_mut(index) {
        b.game = b.key.map(game_use).unwrap_or_default();
    }
}

/// Reads what the game does with every bound key once its control map is loaded and logs it.
pub(super) fn describe_game_uses() {
    let count = bindings().len();
    for index in 0..count {
        describe_game_use(index);
    }
    for b in list() {
        log!("keys: {} {} ({:?}): in the game's control map {}", b.name, show(b.key), b.scope, if b.game.is_empty() { "nothing" } else { b.game.as_str() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_held_with_its_modifiers() {
        let k: Hotkey = "Shift+Q".parse().unwrap();
        assert!(key_held(k, |c| c == 0x10 || c == 0x36), "either shift");
        assert!(!key_held(k, |c| c == 0x10), "shift missing");
        assert!(key_held(Hotkey::new(0x10), |c| c == 0x10 || c == 0x1D), "an extra modifier does not stop a hold");
        assert!(!key_held(Hotkey::new(0x10), |_| false));
    }

    #[test]
    fn resolves_the_file_and_refuses_conflicts() {
        let dir = std::env::temp_dir().join(format!("skyrim-rs-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "{\"key.skyrim-rs.toggle\": 68, \"key.skyrim-rs.close\": \"F10\", \"key.test.a\": \"shift+k\", \"key.test.d\": \"\", \"by.hand\": \"kept\"}";
        std::fs::write(settings::file(&dir, NAME), text).unwrap();
        assert_eq!(settings::open(NAME, &dir), Ok(5));
        declare_host();
        let f10: Hotkey = "F10".parse().unwrap();
        let shift_k: Hotkey = "Shift+K".parse().unwrap();
        assert_eq!((key(TOGGLE), key(CLOSE)), (Some(f10), None));
        assert!(last_message().contains("skyrim-rs.close: F10 is bound to skyrim-rs.toggle"));
        let a = declare("test", "a", "J", Scope::Overlay);
        assert_eq!(key(a), Some(shift_k));
        let b = declare("test", "b", "Shift+K", Scope::Game);
        assert_eq!(key(b), None);
        assert_eq!(set(b, f10), Err("F10 for test.b refused: F10 is bound to skyrim-rs.toggle".to_owned()));
        assert_eq!(set(b, "L".parse().unwrap()), Ok("test.b set to L".to_owned()));
        assert_eq!(declare("test", "b", "Shift+K", Scope::Game), b);
        assert_eq!(key(b), Some("L".parse().unwrap()));
        assert_eq!(settings::get(NAME, "key.skyrim-rs.toggle"), Some(Setting::Text("F10".to_owned())));
        assert_eq!(settings::get(NAME, "key.skyrim-rs.close"), Some(Setting::Text("F10".to_owned())));
        assert_eq!(settings::get(NAME, "key.test.a"), Some(Setting::Text("Shift+K".to_owned())));
        assert_eq!(settings::get(NAME, "by.hand"), Some(Setting::Text("kept".to_owned())));
        assert!(!fired(a));
        assert_eq!(released(a), None);
        let last = last_message();
        let c = declare("test", "c", "", Scope::Game);
        assert_eq!(key(c), None);
        assert_eq!(last_message(), last, "no default is not a bad default");
        assert_eq!(settings::get(NAME, "key.test.c"), None);
        assert_eq!(set(c, "M".parse().unwrap()), Ok("test.c set to M".to_owned()));
        let d = declare("test", "d", "N", Scope::Game);
        assert_eq!(key(d), None, "an empty entry overrides the default");
        assert_eq!(last_message(), "test.c set to M", "an empty entry is not a bad entry");
        assert_eq!(settings::get(NAME, "key.test.d"), Some(Setting::Text(String::new())));
        unbind(TOGGLE);
        assert_eq!(key(TOGGLE), None);
        assert_eq!(last_message(), "skyrim-rs.toggle unbound");
        assert_eq!(settings::get(NAME, "key.skyrim-rs.toggle"), Some(Setting::Text(String::new())));
        assert_eq!(set(d, f10), Ok("test.d set to F10".to_owned()), "unbinding frees the key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
