//! Plugin names a framework plugin serves (`Host::serve_alias`). The stub DLL `skse::alias_stub!`
//! builds gives the name a record and a messaging index in SKSE 2.3.1 and never runs; once every
//! `Load` has run (`PLUGINS_LOADED`) the host finds the index through `GetPluginInfo`, checks that
//! the loader names that index as the alias, then listens to every sender and dispatches with it
//! on the serving plugin's behalf (RE `skse-abi.md` "Request IC9 A").

use core::ffi::{c_char, c_void};
use std::ffi::CString;
use std::sync::{Mutex, PoisonError};

use super::owned::Owned;
use super::{HOSTED_BY, LegacyPlugin, NAME, SKSE, skse_plugins};
use crate::{Error, MessageFn};

/// How many names the host serves: one listener function each, since a message does not say
/// which name it was addressed to.
pub(super) const SLOTS: usize = 4;

/// A message kind no plugin protocol uses, dispatched with no data as the alias's index to the
/// host's own name to read back the sender name the loader gives that index.
const PROBE_KIND: u32 = 0x534b_5250;

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Declared before `PLUGINS_LOADED`, when the loader can name no plugin yet.
    Waiting,
    Served { index: u32, file: String },
    Refused(String),
}

struct Alias {
    name: String,
    owner: &'static str,
    state: State,
}

static ALIASES: Mutex<Vec<Alias>> = Mutex::new(Vec::new());
static HANDLERS: [Owned<MessageFn>; SLOTS] = [const { Owned::new() }; SLOTS];
const LISTENERS: [skse::MessageCallback; SLOTS] = [on_alias::<0>, on_alias::<1>, on_alias::<2>, on_alias::<3>];
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Idle,
    Running,
    /// The sender name the probe's message arrived with.
    Seen(String),
}

static PROBE: Mutex<Probe> = Mutex::new(Probe::Idle);

unsafe extern "C" fn on_alias<const I: usize>(msg: *mut skse::Message) {
    let Some(msg) = (unsafe { msg.as_ref() }) else { return };
    HANDLERS[I].each(|f| {
        f(msg);
        true
    });
}

/// Registers `owner`'s handler for `name` and returns the name's slot; the name is resolved at
/// once when `started` (the loader's `PLUGINS_LOADED` has been seen), else at that message.
pub(super) fn serve(owner: &'static str, name: &str, f: MessageFn, started: bool) -> Result<usize, Error> {
    let slot = {
        let mut aliases = ALIASES.lock().unwrap_or_else(PoisonError::into_inner);
        match aliases.iter().position(|a| a.name.eq_ignore_ascii_case(name)) {
            Some(i) if aliases[i].owner != owner => return Err(Error::Taken { what: format!("plugin name {name}"), owner: aliases[i].owner.to_owned() }),
            Some(i) => i,
            None if aliases.len() == SLOTS => return Err(Error::Full { what: format!("plugin name {name}"), limit: SLOTS }),
            None => {
                aliases.push(Alias { name: name.to_owned(), owner, state: State::Waiting });
                aliases.len() - 1
            }
        }
    };
    HANDLERS[slot].add(owner, f);
    if started {
        resolve(slot);
    }
    Ok(slot)
}

/// Resolves every name declared before the loader's `PLUGINS_LOADED`.
pub(super) fn resolve_waiting() {
    let waiting: Vec<usize> = ALIASES.lock().unwrap_or_else(PoisonError::into_inner).iter().enumerate().filter(|(_, a)| a.state == State::Waiting).map(|(i, _)| i).collect();
    for slot in waiting {
        resolve(slot);
    }
}

fn resolve(slot: usize) {
    let Some((name, owner)) = ALIASES.lock().unwrap_or_else(PoisonError::into_inner).get(slot).filter(|a| a.state == State::Waiting).map(|a| (a.name.clone(), a.owner)) else { return };
    let state = match find_index(&name, slot) {
        Ok((index, file)) => {
            log!("{owner}: serves the plugin name {name} through {file}, messaging index {index}");
            State::Served { index, file }
        }
        Err(reason) => {
            log!("{owner}: the plugin name {name} is not served: {reason}");
            State::Refused(reason)
        }
    };
    if let Some(a) = ALIASES.lock().unwrap_or_else(PoisonError::into_inner).get_mut(slot) {
        a.state = state;
    }
}

/// The stub's file, its index once the loader is known to name the index as `name`, and the
/// listener registered with it.
fn find_index(name: &str, slot: usize) -> Result<(u32, String), String> {
    let file = stub_file(name, &skse_plugins())?;
    if HOSTED_BY.get().is_none_or(|by| *by != "the script extender") {
        return Err("serving another plugin's name needs the script extender's loader".to_owned());
    }
    let skse = SKSE.get().ok_or_else(|| "no SKSE interface".to_owned())?;
    let messaging = skse.messaging().ok_or_else(|| "no messaging interface".to_owned())?;
    let cname = CString::new(name).map_err(|_| "a nul in the name".to_owned())?;
    let info = unsafe { (skse.get_plugin_info)(cname.as_ptr()).as_ref() }.ok_or_else(|| format!("the loader did not accept {file}"))?;
    let index = unsafe { info.loader_handle() };
    *PROBE.lock().unwrap_or_else(PoisonError::into_inner) = Probe::Running;
    let host = CString::new(NAME).expect("no nul in the host's name");
    let _ = unsafe { messaging.send(index, PROBE_KIND, core::ptr::null_mut(), 0, host.as_ptr()) };
    let sender = match core::mem::replace(&mut *PROBE.lock().unwrap_or_else(PoisonError::into_inner), Probe::Idle) {
        Probe::Seen(sender) => Some(sender),
        _ => None,
    };
    probe_verdict(name, index, sender.as_deref())?;
    if !unsafe { (messaging.register_listener)(index, core::ptr::null(), LISTENERS[slot]) } {
        return Err(format!("the loader refused a listener for index {index}"));
    }
    Ok((index, file))
}

/// The file of the stub that declares `name`; refused when a DLL that is not a stub declares it
/// too (the loader would give the name to whichever it scans first).
fn stub_file(name: &str, plugins: &[LegacyPlugin]) -> Result<String, String> {
    let named: Vec<&LegacyPlugin> = plugins.iter().filter(|p| p.data.as_ref().is_some_and(|d| d.name.eq_ignore_ascii_case(name))).collect();
    if let Some(real) = named.iter().find(|p| !p.stub) {
        return Err(format!("{} is installed: a real {name} plugin, not the stub; remove one of the two", real.file));
    }
    named.first().map(|p| p.file.clone()).ok_or_else(|| format!("no plugin named {name} in Data\\SKSE\\Plugins: install its stub"))
}

/// Whether the probe sent as `index` arrived under `name`. It does not after an earlier plugin
/// failed its `Load`: the loader then names every later index after the plugin one slot on.
fn probe_verdict(name: &str, index: u32, sender: Option<&str>) -> Result<(), String> {
    match sender {
        None => Err(format!("the loader delivered nothing sent as index {index}")),
        Some(s) if !s.eq_ignore_ascii_case(name) => Err(format!("the loader names index {index} {s}, not {name}: a plugin before it failed to load and the loader shifted the names after it")),
        Some(_) => Ok(()),
    }
}

/// Takes the probe's message, which no plugin handler sees.
pub(super) fn take_probe(msg: &skse::Message) -> bool {
    if msg.kind != PROBE_KIND || !msg.data.is_null() {
        return false;
    }
    let mut probe = PROBE.lock().unwrap_or_else(PoisonError::into_inner);
    if *probe != Probe::Running {
        return false;
    }
    *probe = Probe::Seen(if msg.sender.is_null() { String::new() } else { unsafe { core::ffi::CStr::from_ptr(msg.sender) }.to_string_lossy().into_owned() });
    true
}

/// Dispatches as the name in `slot` to `receiver` (null: every listener of the name); false
/// while the name is not served.
pub(super) fn send(slot: usize, receiver: *const c_char, kind: u32, data: *mut c_void, len: u32) -> bool {
    let Some(State::Served { index, .. }) = ALIASES.lock().unwrap_or_else(PoisonError::into_inner).get(slot).map(|a| a.state.clone()) else { return false };
    SKSE.get().and_then(|s| s.messaging()).is_some_and(|m| unsafe { m.send(index, kind, data, len, receiver) })
}

/// Drops `owner`'s handlers; the names stay resolved for its next generation.
pub(super) fn remove_owner(owner: &str) -> usize {
    HANDLERS.iter().map(|h| h.remove_owner(owner)).sum()
}

/// One line per name for the console inspector.
pub(super) fn lines() -> Vec<String> {
    ALIASES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|a| match &a.state {
            State::Waiting => format!("plugin name {} ({}): waiting for PLUGINS_LOADED", a.name, a.owner),
            State::Served { index, file } => format!("plugin name {} ({}): served through {file}, index {index}", a.name, a.owner),
            State::Refused(reason) => format!("plugin name {} ({}): not served, {reason}", a.name, a.owner),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::host::VersionData;

    fn plugin(file: &str, name: &str, stub: bool) -> LegacyPlugin {
        LegacyPlugin { file: file.to_owned(), loaded: !stub, stub, data: Some(VersionData { name: name.to_owned(), version: 1, author: String::new(), flags: 0, method: 0 }) }
    }

    #[test]
    fn only_a_stub_alone_with_the_name_is_served() {
        let stub = plugin("skyrim_rs_improved_camera_stub.dll", "ImprovedCameraSE", true);
        let other = plugin("hdtsmp64.dll", "hdtsmp64", false);
        assert_eq!(stub_file("improvedcamerase", &[other.clone(), stub.clone()]), Ok("skyrim_rs_improved_camera_stub.dll".to_owned()));
        let real = plugin("ImprovedCameraSE.dll", "ImprovedCameraSE", false);
        assert!(stub_file("ImprovedCameraSE", &[stub, real.clone()]).unwrap_err().starts_with("ImprovedCameraSE.dll is installed"));
        assert!(stub_file("ImprovedCameraSE", &[real]).is_err());
        assert!(stub_file("ImprovedCameraSE", &[other]).unwrap_err().starts_with("no plugin named"));
    }

    #[test]
    fn the_probe_must_come_back_under_the_name() {
        assert_eq!(probe_verdict("ImprovedCameraSE", 7, Some("improvedcamerase")), Ok(()));
        assert!(probe_verdict("ImprovedCameraSE", 7, Some("NextPlugin")).unwrap_err().contains("shifted"));
        assert!(probe_verdict("ImprovedCameraSE", 7, None).is_err());
    }

    #[test]
    fn the_probe_is_taken_only_while_one_runs() {
        let mut msg = skse::Message { sender: c"Stub".as_ptr(), kind: PROBE_KIND, data_len: 0, data: core::ptr::null_mut() };
        assert!(!take_probe(&msg));
        *PROBE.lock().unwrap() = Probe::Running;
        msg.kind = 0;
        assert!(!take_probe(&msg));
        msg.kind = PROBE_KIND;
        assert!(take_probe(&msg));
        assert!(!take_probe(&msg), "one message per probe");
        assert_eq!(core::mem::replace(&mut *PROBE.lock().unwrap(), Probe::Idle), Probe::Seen("Stub".to_owned()));
    }

    #[test]
    fn messages_to_a_name_reach_its_owner_until_it_goes() {
        static SEEN: AtomicU32 = AtomicU32::new(0);
        fn a(msg: &skse::Message) {
            SEEN.fetch_add(msg.kind, Ordering::Relaxed);
        }
        fn b(_: &skse::Message) {
            SEEN.fetch_add(1000, Ordering::Relaxed);
        }
        let slot = serve("alias-a", "AliasTestOne", a, false).unwrap();
        assert!(matches!(serve("alias-b", "aliastestone", b, false), Err(Error::Taken { .. })));
        let other = serve("alias-b", "AliasTestTwo", b, false).unwrap();
        assert_ne!(slot, other);
        let mut msg = skse::Message { sender: c"Consumer".as_ptr(), kind: 3, data_len: 0, data: core::ptr::null_mut() };
        unsafe { LISTENERS[slot](&raw mut msg) };
        assert_eq!(SEEN.load(Ordering::Relaxed), 3);
        assert!(!send(slot, c"Consumer".as_ptr(), 0, core::ptr::null_mut(), 0), "not served before PLUGINS_LOADED");
        assert_eq!(remove_owner("alias-a"), 1);
        unsafe { LISTENERS[slot](&raw mut msg) };
        assert_eq!(SEEN.load(Ordering::Relaxed), 3);
        assert_eq!(serve("alias-a", "AliasTestOne", a, false).unwrap(), slot, "a later generation keeps the slot");
        assert!(lines().iter().any(|l| l == "plugin name AliasTestOne (alias-a): waiting for PLUGINS_LOADED"));
        assert_eq!(remove_owner("alias-a") + remove_owner("alias-b"), 2);
    }
}
