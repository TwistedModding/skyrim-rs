//! Each plugin file's fit with every runtime in the profiles folder, read from its manifest when
//! the host reads the file, for the Plugins tab and the console. A runtime's ids are checked
//! against its Address Library only where that file is installed, which on most installs is the
//! running game's alone.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use versionlib::manifest::{self, Fit};
use versionlib::profile::{self, Profile};
use versionlib::VersionDb;

struct Runtime {
    profile: &'static Profile,
    db: Option<&'static VersionDb>,
}

/// The running game's runtime and every other in the profiles folder, in version order.
fn runtimes() -> &'static [Runtime] {
    static RUNTIMES: OnceLock<Vec<Runtime>> = OnceLock::new();
    RUNTIMES.get_or_init(|| {
        let running = versionlib::running().ok();
        let mut found: Vec<Runtime> = running.iter().map(|r| Runtime { profile: r.profile(), db: Some(r.db()) }).collect();
        let plugins = versionlib::plugins_dir().unwrap_or_default();
        for read in profile::all(&plugins) {
            match read {
                Ok(other) if found.iter().any(|known| known.profile.runtime() >> 4 == other.runtime() >> 4) => {}
                Ok(other) => {
                    let db = VersionDb::open(&plugins.join(other.address_library())).ok();
                    found.push(Runtime { profile: Box::leak(Box::new(other)), db: db.map(|db| &*Box::leak(Box::new(db))) });
                }
                Err(e) => log!("runtime support: a profile is not read: {e}"),
            }
        }
        found.sort_by_key(|r| r.profile.runtime());
        found
    })
}

struct Support {
    runtime: String,
    fit: Fit,
    /// False when the runtime's Address Library is not here, so ids with no row went unchecked.
    ids_checked: bool,
}

static SUPPORT: Mutex<BTreeMap<String, Vec<Support>>> = Mutex::new(BTreeMap::new());

/// Remembers how the DLL with these bytes, the plugin file `file`, fits every runtime.
pub(super) fn note(file: &str, pe: &[u8]) {
    let support = runtimes().iter().map(|r| Support { runtime: r.profile.name(), fit: manifest::fit(pe, r.profile, r.db), ids_checked: r.db.is_some() }).collect();
    SUPPORT.lock().unwrap_or_else(PoisonError::into_inner).insert(file.to_owned(), support);
}

fn with<R>(file: &str, read: impl FnOnce(&[Support]) -> R) -> R {
    read(SUPPORT.lock().unwrap_or_else(PoisonError::into_inner).get(file).map_or(&[], Vec::as_slice))
}

fn plain(fit: &Fit) -> String {
    match fit {
        Fit::Fits => "ok".to_owned(),
        Fit::BeforeManifest => "built before the manifest".to_owned(),
        Fit::Unreadable(_) => "manifest unreadable".to_owned(),
        Fit::Lacks(_) => {
            let (missing, unverified) = fit.counts();
            format!("{missing} missing, {unverified} unverified")
        }
    }
}

/// One result per runtime, for `SkyrimRs gui`.
pub(super) fn per_runtime(file: &str) -> String {
    with(file, |support| support.iter().map(|s| format!("{} {}{}", s.runtime, plain(&s.fit), if s.ids_checked { "" } else { " (ids not checked)" })).collect::<Vec<_>>().join("; "))
}

/// Everything each runtime lacks of what the file uses, one line per item, for `SkyrimRs support`.
pub(super) fn details(file: &str) -> Vec<String> {
    with(file, |support| {
        let mut lines = vec![format!("{file}: {}", if support.is_empty() { "not read" } else { "runtime support" })];
        for s in support {
            let checked = if s.ids_checked { "" } else { ", ids not checked: its Address Library is not installed here" };
            lines.push(format!("{}: {}{checked}", s.runtime, plain(&s.fit)));
            match &s.fit {
                Fit::Lacks(lacks) => lines.extend(lacks.iter().map(|(record, why)| format!("{}: {why} {record}", s.runtime))),
                Fit::Unreadable(e) => lines.push(format!("{}: {e}", s.runtime)),
                Fit::Fits | Fit::BeforeManifest => {}
            }
        }
        lines
    })
}

/// The Plugins tab's cell for a file, and what its hover says about the versions it leaves out.
pub(super) fn summary(file: &str) -> (String, String) {
    with(file, |support| {
        if support.is_empty() {
            return (String::new(), String::new());
        }
        let names = |fits: bool| support.iter().filter(|s| (s.fit == Fit::Fits) == fits).map(|s| s.runtime.as_str()).collect::<Vec<_>>().join(", ");
        let (yes, no) = (names(true), names(false));
        let cell = if yes.is_empty() { "None of the known game versions".to_owned() } else { yes };
        let unchecked = support.iter().any(|s| s.fit == Fit::Fits && !s.ids_checked);
        let mut hover = Vec::new();
        if !no.is_empty() {
            hover.push(format!("Not on {no}: those game versions lack something this plugin uses, or it has not been checked on them."));
        }
        if unchecked {
            hover.push("A game version whose Address Library is not installed here is only partly checked.".to_owned());
        }
        (cell, hover.join(" "))
    })
}

/// What the Plugins tab says about a file refused because the running game lacks what it uses;
/// `None` when it was refused for something else. The log has the list.
pub(super) fn refused_here(file: &str) -> Option<String> {
    let running = versionlib::running().ok()?.profile().name();
    with(file, |support| {
        let here = support.iter().find(|s| s.runtime == running)?;
        (here.fit != Fit::Fits).then(|| format!("This game version ({running}) cannot run it: {}. It did not run at all, and the game is unaffected.", plain(&here.fit)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_never_read_has_no_support() {
        assert_eq!(per_runtime("never-read.dll"), "");
        assert_eq!(details("never-read.dll"), ["never-read.dll: not read"]);
        assert_eq!(summary("never-read.dll"), (String::new(), String::new()));
    }

    #[test]
    fn results_read_as_a_player_and_a_developer_want_them() {
        let lacks = |file: &str, fit: Fit, ids_checked| {
            SUPPORT.lock().unwrap().insert(file.to_owned(), vec![Support { runtime: "1.5.97".to_owned(), fit, ids_checked }, Support { runtime: "1.7.104".to_owned(), fit: Fit::Fits, ids_checked: true }]);
        };
        lacks("a.dll", Fit::Lacks(vec![(manifest::Record::Id(5), manifest::Lack::Missing), (manifest::Record::Member("A.b".to_owned()), manifest::Lack::Unverified)]), true);
        assert_eq!(per_runtime("a.dll"), "1.5.97 1 missing, 1 unverified; 1.7.104 ok");
        assert_eq!(details("a.dll"), ["a.dll: runtime support", "1.5.97: 1 missing, 1 unverified", "1.5.97: missing id 5", "1.5.97: unverified member A.b", "1.7.104: ok"]);
        let (cell, hover) = summary("a.dll");
        assert_eq!(cell, "1.7.104");
        assert!(hover.starts_with("Not on 1.5.97"), "{hover}");
        lacks("b.dll", Fit::BeforeManifest, false);
        assert_eq!(per_runtime("b.dll"), "1.5.97 built before the manifest (ids not checked); 1.7.104 ok");
    }
}
