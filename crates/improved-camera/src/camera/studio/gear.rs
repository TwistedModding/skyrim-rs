//! `camera/studio/StudioGear`: equipment hidden on posed actors for a shot, through the biped's
//! attached 3D rather than node names, so any actor and any gear placement mod is covered and the
//! bones the pose engine edits are left alone. Re-asserted every frame, since equipment 3D is
//! rebuilt on equip, unequip and 3D reload.

use std::collections::BTreeMap;

use skyrim::actors::{ActorExt, RefHandle, TESObjectREFRExt};
use skyrim::forms::TESFormExt;
use skyrim::render::NiAVObjectExt;
use sre::Actor;

/// The slots a screenshot fights (`Slot`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Helmet,
    Hair,
    Circlet,
    Shield,
    Quiver,
    /// Every drawn or sheathed weapon slot together.
    Weapons,
}

impl Slot {
    pub const ALL: [Self; 6] = [Self::Helmet, Self::Hair, Self::Circlet, Self::Shield, Self::Quiver, Self::Weapons];

    /// `SlotName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Helmet => "Helmet",
            Self::Hair => "Hair",
            Self::Circlet => "Circlet",
            Self::Shield => "Shield",
            Self::Quiver => "Quiver",
            Self::Weapons => "Weapons",
        }
    }

    const fn bit(self) -> u32 {
        1 << self as u32
    }

    /// `SlotObjects`: the biped indices behind the slot. The armor ones are the slot mask bits of
    /// the Creation Kit's biped slots (30 head, 31 hair, 41 long hair, 42 circlet, 39 shield),
    /// the weapons 32-40 and the quiver the ammo object 41 (RE `camera.md` IC8 I).
    #[must_use]
    pub const fn objects(self) -> &'static [usize] {
        match self {
            Self::Helmet => &[0],
            Self::Hair => &[1, 11],
            Self::Circlet => &[12],
            Self::Shield => &[9],
            Self::Quiver => &[41],
            Self::Weapons => &[32, 33, 34, 35, 36, 37, 38, 39, 40],
        }
    }
}

fn parts(actor: &Actor, slot: Slot) -> impl Iterator<Item = &sre::NiAVObject> {
    let objects = actor.biped_objects();
    slot.objects().iter().filter_map(|&i| objects.get(i)).filter_map(|o| unsafe { o.partClone.as_ref() })
}

/// `IsSlotFilled`: a slot with nothing equipped has no 3D to hide.
#[must_use]
pub fn is_slot_filled(actor: &Actor, slot: Slot) -> bool {
    parts(actor, slot).next().is_some()
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    handle: RefHandle,
    /// Requested slot bits.
    hidden: u32,
    /// Slot bits whose 3D the studio flagged.
    applied: u32,
}

/// `ApplyEntry`: hides what is asked for; shows again only parts the studio hid, since the engine
/// hides gear of its own accord (sheathing, ragdolls, scenes).
fn apply_entry(actor: &Actor, entry: &mut Entry, apply: bool) {
    for slot in Slot::ALL {
        let bit = slot.bit();
        if apply && entry.hidden & bit != 0 {
            parts(actor, slot).for_each(|p| p.set_hidden(true));
            entry.applied |= bit;
        } else if entry.applied & bit != 0 {
            parts(actor, slot).for_each(|p| p.set_hidden(false));
            entry.applied &= !bit;
        }
    }
}

/// `GearEngine`, by the actor's form id.
#[derive(Clone, Debug, Default)]
pub struct GearEngine {
    actors: BTreeMap<u32, Entry>,
}

impl GearEngine {
    pub fn set_hidden(&mut self, actor: &Actor, slot: Slot, hidden: bool) {
        let entry = self.actors.entry(actor.formID).or_insert(Entry { handle: actor.handle(), hidden: 0, applied: 0 });
        entry.hidden = if hidden { entry.hidden | slot.bit() } else { entry.hidden & !slot.bit() };
    }

    #[must_use]
    pub fn is_hidden(&self, id: u32, slot: Slot) -> bool {
        self.actors.get(&id).is_some_and(|e| e.hidden & slot.bit() != 0)
    }

    /// `Snapshot`: the actors with something hidden, and what.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(u32, u32)> {
        self.actors.iter().filter(|(_, e)| e.hidden != 0).map(|(id, e)| (*id, e.hidden)).collect()
    }

    /// `SetHiddenMask`.
    pub fn set_hidden_mask(&mut self, actor: &Actor, mask: u32) {
        let entry = self.actors.entry(actor.formID).or_insert(Entry { handle: actor.handle(), hidden: 0, applied: 0 });
        entry.hidden = mask;
    }

    /// `OnGameUpdate`: an actor out of reach keeps its entry while it still asks for something.
    pub fn on_game_update(&mut self, apply_all: bool) {
        self.actors.retain(|_, entry| match entry.handle.lookup() {
            Some(r) => {
                if let Some(actor) = r.cast::<Actor>() {
                    apply_entry(actor, entry, apply_all);
                }
                entry.hidden != 0 || entry.applied != 0
            }
            None => entry.hidden != 0,
        });
    }

    /// `RevertAll`: every part the studio hid is shown again.
    pub fn revert_all(&mut self) {
        for entry in self.actors.values_mut() {
            if let Some(r) = entry.handle.lookup()
                && let Some(actor) = r.cast::<Actor>()
            {
                entry.hidden = 0;
                apply_entry(actor, entry, false);
            }
        }
        self.actors.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Slot;

    #[test]
    fn the_slots_cover_the_biped_without_overlap() {
        let mut seen = Vec::new();
        for slot in Slot::ALL {
            for &i in slot.objects() {
                assert!(i < 42 && !seen.contains(&i), "{} uses {i}", slot.name());
                seen.push(i);
            }
        }
        assert_eq!(Slot::ALL.map(Slot::bit), [1, 2, 4, 8, 16, 32]);
    }
}
