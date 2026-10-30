//! `event/GameEventManager`'s object-loaded sink: when an actor's 3D loads (a game load, a cell
//! attach, a 3D reset from equipment) its persistent skeleton and race adjustments go back on.
//! The sink only notes the form id; the photo mode reconciles the actor on the next game tick.

use std::sync::atomic::{AtomicBool, Ordering};

use framework::Host;
use sre::{BSEventNotifyControl, TESObjectLoadedEvent};

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// `Register`'s object-loaded sink. The script event holder exists from the data load on, so a
/// first try before it is repeated at `DATA_LOADED`.
pub fn register(host: &Host) {
    if REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    let added = host.add_sink::<TESObjectLoadedEvent>(|event, _| {
        if event.loaded != 0 {
            crate::camera::studio::manager::queue_loaded(event.formID);
        }
        BSEventNotifyControl::Continue
    });
    REGISTERED.store(added.is_ok(), Ordering::Relaxed);
}
