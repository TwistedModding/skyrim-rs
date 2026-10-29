//! Input filters: user events and buttons a plugin keeps from the game's handlers while it holds
//! the filter active. The overlay's sink, the first on the input source, asks [`swallows`] for
//! every button event of a frame that reaches the game and skips the matched ones before
//! `MenuControls` and `PlayerControls` read them. Filters only take events away, so their order
//! does not matter.

use std::sync::{Mutex, MutexGuard, PoisonError};

use skyrim::prelude::*;
use sre::{ButtonEvent, InputDevice};

struct Filter {
    owner: String,
    name: String,
    user_events: Vec<String>,
    buttons: Vec<(InputDevice, u32)>,
    active: bool,
    swallowed: u64,
}

impl Filter {
    fn matches(&self, user_event: &[u8], device: Option<InputDevice>, id: u32) -> bool {
        (!user_event.is_empty() && self.user_events.iter().any(|e| e.as_bytes().eq_ignore_ascii_case(user_event))) || device.is_some_and(|d| self.buttons.contains(&(d, id)))
    }
}

static FILTERS: Mutex<Vec<Filter>> = Mutex::new(Vec::new());

fn filters() -> MutexGuard<'static, Vec<Filter>> {
    FILTERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Declares `<owner>.<name>`, inactive, or finds it again for a reloaded plugin with what it now swallows.
pub(super) fn declare(owner: &str, name: &str, user_events: &[&str], buttons: &[(InputDevice, u32)]) -> usize {
    let user_events = user_events.iter().map(|e| (*e).to_owned()).collect();
    let buttons = buttons.to_vec();
    let mut list = filters();
    if let Some(i) = list.iter().position(|f| f.owner == owner && f.name == name) {
        list[i].user_events = user_events;
        list[i].buttons = buttons;
        return i;
    }
    list.push(Filter { owner: owner.to_owned(), name: name.to_owned(), user_events, buttons, active: false, swallowed: 0 });
    list.len() - 1
}

pub(super) fn set_active(index: usize, active: bool) {
    if let Some(f) = filters().get_mut(index) {
        f.active = active;
    }
}

pub(super) fn is_active(index: usize) -> bool {
    filters().get(index).is_some_and(|f| f.active)
}

/// Deactivates every filter of `owner`; a reloaded generation declares them again.
pub(super) fn remove_owner(owner: &str) -> usize {
    filters().iter_mut().filter(|f| f.owner == owner && f.active).map(|f| f.active = false).count()
}

fn swallow(user_event: &[u8], device: Option<InputDevice>, id: u32) -> bool {
    let mut hit = false;
    for f in filters().iter_mut().filter(|f| f.active && f.matches(user_event, device, id)) {
        f.swallowed += 1;
        hit = true;
    }
    hit
}

/// Whether an active filter takes the event; for the overlay's sink.
pub(super) fn swallows(event: &ButtonEvent) -> bool {
    swallow(event.user_event().as_bytes(), InputDevice::from_u32(event.device), event.idCode)
}

/// One line per filter, for `SkyrimRs keys`.
pub(super) fn lines() -> Vec<String> {
    filters()
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let buttons: Vec<String> = f.buttons.iter().map(|(d, id)| format!("{d:?} {id:#x}")).collect();
            format!("filter {i} {}.{}: events [{}], buttons [{}], {}, {} swallowed", f.owner, f.name, f.user_events.join(", "), buttons.join(", "), if f.active { "active" } else { "inactive" }, f.swallowed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_active_filter_takes_its_events_and_buttons() {
        let a = declare("test-filter", "menus", &["Favorites", "Quick Map"], &[(InputDevice::Gamepad, 0x100)]);
        assert!(!swallow(b"Favorites", Some(InputDevice::Keyboard), 0x10), "inactive");
        set_active(a, true);
        assert!(is_active(a));
        assert!(swallow(b"favorites", Some(InputDevice::Keyboard), 0x10), "names compare without case");
        assert!(swallow(b"Quick Map", Some(InputDevice::Gamepad), 0x20));
        assert!(swallow(b"", Some(InputDevice::Gamepad), 0x100), "by button");
        assert!(!swallow(b"", Some(InputDevice::Keyboard), 0x100), "a button is per device");
        assert!(!swallow(b"Activate", Some(InputDevice::Keyboard), 0x12));
        assert!(!swallow(b"", None, 0x100));
        assert!(lines()[a].ends_with("active, 3 swallowed"), "{}", lines()[a]);
        assert_eq!(declare("test-filter", "menus", &["Wait"], &[]), a, "a reload finds it again");
        assert!(!swallow(b"Favorites", Some(InputDevice::Keyboard), 0x10), "with what it now swallows");
        assert!(swallow(b"Wait", Some(InputDevice::Keyboard), 0x14));
        assert_eq!(remove_owner("test-filter"), 1);
        assert!(!is_active(a));
        assert!(!swallow(b"Wait", Some(InputDevice::Keyboard), 0x14));
    }
}
