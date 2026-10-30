//! IC's profiles (`plugin.cpp` `ReadConfig`, `ValidateConfig`, `ReadProfile`, `WriteProfile`,
//! `DeleteProfile`; `settings.h` `PluginConfig::Profile`), kept in the plugin's settings instead of
//! IC's files: the profile in use under its own keys (`General.bEnableBody`), each saved profile
//! under `Profiles.<name>.` (`Profiles.Default.General.bEnableBody`) and the active one's name
//! under `Config.Profile.active`.

use framework::settings::{Setting, Value};

use crate::settings::{Profile, Store};

/// IC's default profile, which the menu never deletes.
pub const DEFAULT: &str = "Default";
const ACTIVE: &str = "Config.Profile.active";
const SAVED: &str = "Profiles.";

/// The keys of one saved profile, below its prefix in another store.
struct Saved<'a, S: Store> {
    store: &'a S,
    prefix: String,
}

impl<'a, S: Store> Saved<'a, S> {
    fn new(store: &'a S, name: &str) -> Self {
        Self { store, prefix: format!("{SAVED}{name}.") }
    }
}

impl<S: Store> Store for Saved<'_, S> {
    fn get(&self, key: &str) -> Option<Setting> {
        self.store.get(&format!("{}{key}", self.prefix))
    }

    fn set(&self, key: &str, value: Setting) {
        self.store.set(&format!("{}{key}", self.prefix), value);
    }

    fn keys(&self, prefix: &str) -> Vec<String> {
        self.store.keys(&format!("{}{prefix}", self.prefix)).into_iter().map(|k| k[self.prefix.len()..].to_owned()).collect()
    }

    fn remove(&self, prefix: &str) -> usize {
        self.store.remove(&format!("{}{prefix}", self.prefix))
    }
}

/// A name the menu accepts for a new profile: IC's save dialog lets through ASCII letters and
/// digits only.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The saved profiles' names, in order.
#[must_use]
pub fn names(store: &impl Store) -> Vec<String> {
    let mut names: Vec<String> = store.keys(SAVED).iter().filter_map(|k| k[SAVED.len()..].split_once('.').map(|(name, _)| name.to_owned())).collect();
    names.dedup();
    names
}

/// The active profile's name; `None` before the first run.
#[must_use]
pub fn active(store: &impl Store) -> Option<String> {
    store.get(ACTIVE).as_ref().and_then(String::from_setting)
}

pub fn set_active(store: &impl Store, name: &str) {
    store.set(ACTIVE, Setting::Text(name.to_owned()));
}

/// The saved profile `name`, `None` when there is none.
#[must_use]
pub fn load(store: &impl Store, name: &str) -> Option<Profile> {
    let saved = Saved::new(store, name);
    (!saved.keys("").is_empty()).then(|| Profile::read(&saved))
}

/// The switch to the saved profile `name` (the menu's Profiles list, `ic profile <name>`): it
/// becomes the active one and is returned to replace the profile in use; `None`, with nothing
/// changed, when there is no such profile.
pub fn switch(store: &impl Store, name: &str) -> Option<Profile> {
    let profile = load(store, name)?;
    set_active(store, name);
    Some(profile)
}

/// `WriteProfile`: stores `profile` as `name`, replacing what was saved under it.
pub fn save(store: &impl Store, name: &str, profile: &Profile) {
    let saved = Saved::new(store, name);
    saved.remove("");
    profile.write(&saved);
}

/// Removes the saved profile `name`.
pub fn delete(store: &impl Store, name: &str) -> bool {
    Saved::new(store, name).remove("") > 0
}

/// `ValidateConfig` and `ReadProfile` at startup: the active profile's name, `Default` when the
/// saved one is gone; a missing saved profile is written from `live`, the profile in use (IC
/// writes its in-memory profile when the file is missing).
pub fn validate(store: &impl Store, live: &Profile) -> String {
    let mut name = active(store).unwrap_or_else(|| DEFAULT.to_owned());
    if load(store, &name).is_none() && name != DEFAULT {
        DEFAULT.clone_into(&mut name);
    }
    if load(store, &name).is_none() {
        save(store, &name, live);
    }
    set_active(store, &name);
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Memory;

    #[test]
    fn saved_profiles_are_listed_loaded_and_deleted() {
        let store = Memory::default();
        assert_eq!(names(&store), Vec::<String>::new());
        let mut warm = Profile::default();
        warm.FOV.fFirstPerson = 95.0;
        warm.General.bEnableHead = true;
        save(&store, "Warm", &warm);
        save(&store, DEFAULT, &Profile::default());
        assert_eq!(names(&store), [DEFAULT, "Warm"]);
        assert_eq!(store.get_f32("Profiles.Warm.FOV.fFirstPerson"), Some(95.0));
        assert_eq!(store.get_f32("FOV.fFirstPerson"), None, "the live keys are separate");
        assert_eq!(load(&store, "Warm"), Some(warm));
        assert_eq!(load(&store, "Cold"), None);
        assert!(delete(&store, "Warm"));
        assert!(!delete(&store, "Warm"));
        assert_eq!(names(&store), [DEFAULT]);
    }

    #[test]
    fn switching_hands_over_the_saved_values_and_the_active_name() {
        let store = Memory::default();
        let mut warm = Profile::default();
        warm.FOV.fFirstPerson = 95.0;
        save(&store, DEFAULT, &Profile::default());
        save(&store, "Warm", &warm);
        set_active(&store, DEFAULT);
        assert_eq!(switch(&store, "Warm"), Some(warm));
        assert_eq!(active(&store).as_deref(), Some("Warm"));
        assert_eq!(switch(&store, "Cold"), None);
        assert_eq!(active(&store).as_deref(), Some("Warm"));
        assert_eq!(switch(&store, DEFAULT), Some(Profile::default()));
        assert_eq!(load(&store, "Warm"), Some(warm), "a switch leaves the saved profiles alone");
    }

    #[test]
    fn a_saved_profile_does_not_shadow_a_longer_name() {
        let store = Memory::default();
        save(&store, "A", &Profile::default());
        save(&store, "AB", &Profile::default());
        assert!(delete(&store, "A"));
        assert_eq!(names(&store), ["AB"]);
    }

    #[test]
    fn validation_falls_back_to_default_and_writes_what_is_missing() {
        let store = Memory::default();
        let mut live = Profile::default();
        live.Hide.bBody = true;
        assert_eq!(validate(&store, &live), DEFAULT);
        assert_eq!(active(&store).as_deref(), Some(DEFAULT));
        assert_eq!(load(&store, DEFAULT), Some(live));
        set_active(&store, "Gone");
        assert_eq!(validate(&store, &Profile::default()), DEFAULT);
        assert_eq!(load(&store, DEFAULT), Some(live), "an existing Default is kept");
    }

    #[test]
    fn names_follow_ic_s_filter() {
        assert!(valid_name("Combat2"));
        assert!(!valid_name("") && !valid_name("my profile") && !valid_name("a.b") && !valid_name("é"));
    }
}
