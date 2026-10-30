//! `camera/studio/StudioFace`: expressions, modifiers, phonemes, the eyes, the blink and the
//! tongue bones, re-asserted every frame over the engine's writes as the pose engine does, which
//! also holds off blinks, eye wander and expression flicker without a byte patch.

use std::collections::BTreeMap;

use skyrim::actors::{RefHandle, TESObjectREFRExt};
use skyrim::animation::{BSFaceGenAnimationDataExt, FaceChannel, face_channel_names, face_data};
use skyrim::forms::TESFormExt;
use skyrim::render::{NiAVObjectExt, Transform};
use sre::Actor;

use super::adjustments::{BoneDelta, compose, third_person_3d};
use crate::util::skyrim_util as util;

pub const PHONEME_COUNT: usize = 16;
/// The engine's modifiers 14-16 are head tracking.
pub const MODIFIER_COUNT: usize = 14;
/// `MoodNeutral`.
const NEUTRAL: i32 = 7;

/// `FaceState`.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceState {
    pub expression: i32,
    pub expression_value: f32,
    pub phonemes: [f32; PHONEME_COUNT],
    pub modifiers: [f32; MODIFIER_COUNT],
    pub eyes_override: bool,
    pub eyes_heading: f32,
    pub eyes_pitch: f32,
    pub suppress_blink: bool,
    /// Keeps applying outside the studio and survives revert-on-exit.
    pub persistent: bool,
    /// Tongue (and similar accessory) bone deltas, saved with the expression.
    pub tongue: BTreeMap<String, BoneDelta>,
}

impl Default for FaceState {
    fn default() -> Self {
        Self { expression: NEUTRAL, expression_value: 0.0, phonemes: [0.0; PHONEME_COUNT], modifiers: [0.0; MODIFIER_COUNT], eyes_override: false, eyes_heading: 0.0, eyes_pitch: 0.0, suppress_blink: true, persistent: false, tongue: BTreeMap::new() }
    }
}

/// The engine's channel names, the position being the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaceNames {
    pub expressions: Vec<String>,
    pub modifiers: Vec<String>,
    pub phonemes: Vec<String>,
}

impl FaceNames {
    /// From the engine's tables.
    #[must_use]
    pub fn engine() -> Self {
        Self { expressions: face_channel_names(FaceChannel::Expression), modifiers: face_channel_names(FaceChannel::Modifier), phonemes: face_channel_names(FaceChannel::Phoneme) }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    handle: RefHandle,
    state: FaceState,
    /// The tongue bones' locals before the first edit, to put back.
    tongue_base: BTreeMap<String, Transform>,
}

/// `ApplyEntry`.
fn apply_entry(actor: &Actor, entry: &mut Entry) {
    if let Some(root) = third_person_3d(actor) {
        let mut done = Vec::new();
        for (name, delta) in &entry.state.tongue {
            let Some(node) = util::object_by_name(root, name) else { continue };
            let base = *entry.tongue_base.entry(name.clone()).or_insert_with(|| node.local_transform());
            if delta.is_identity() {
                node.set_local_transform(base);
                done.push(name.clone());
            } else {
                node.set_local_transform(compose(&base, delta));
            }
            node.update_transforms();
        }
        for name in done {
            entry.tongue_base.remove(&name);
            entry.state.tongue.remove(&name);
        }
    }
    let Some(data) = face_data(actor) else { return };
    let s = &entry.state;
    data.set_inputs(&s.phonemes, &s.modifiers);
    data.set_expression_override(s.expression, s.expression_value);
    if s.eyes_override {
        data.hold_eyes(s.eyes_heading, s.eyes_pitch);
    }
    if s.suppress_blink {
        data.hold_blink();
    }
}

/// `ResetFace`: the tongue bones' captured locals put back (nothing animates them), then the
/// face's override released and its expressions, modifiers and phonemes reset.
fn reset_face(actor: &Actor, entry: &mut Entry) {
    if let Some(root) = third_person_3d(actor) {
        for (name, base) in &entry.tongue_base {
            if let Some(node) = util::object_by_name(root, name) {
                node.set_local_transform(*base);
                node.update_transforms();
            }
        }
    }
    entry.tongue_base.clear();
    entry.state.tongue.clear();
    if let Some(data) = face_data(actor) {
        data.clear_expression_override();
        data.reset(0.0, true, true, false, false);
    }
}

fn with_actor(handle: RefHandle, f: impl FnOnce(&Actor)) {
    if let Some(r) = handle.lookup()
        && let Some(actor) = r.cast::<Actor>()
    {
        f(actor);
    }
}

/// `FaceEngine`, by the actor's form id.
#[derive(Clone, Debug, Default)]
pub struct FaceEngine {
    actors: BTreeMap<u32, Entry>,
}

impl FaceEngine {
    pub fn get_or_create(&mut self, actor: &Actor) -> &mut FaceState {
        let id = actor.formID;
        &mut self
            .actors
            .entry(id)
            .or_insert_with(|| {
                crate::log(&format!("<Photo Mode> Face override started for {id:08X}"));
                Entry { handle: actor.handle(), state: FaceState::default(), tongue_base: BTreeMap::new() }
            })
            .state
    }

    #[must_use]
    pub fn get(&self, id: u32) -> Option<&FaceState> {
        self.actors.get(&id).map(|e| &e.state)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut FaceState> {
        self.actors.get_mut(&id).map(|e| &mut e.state)
    }

    /// `OverriddenActors`.
    pub fn overridden(&self) -> impl Iterator<Item = (u32, &FaceState)> {
        self.actors.iter().map(|(id, e)| (*id, &e.state))
    }

    /// `Remove`: the face reset and no longer overridden.
    pub fn remove(&mut self, id: u32) {
        if let Some(mut entry) = self.actors.remove(&id) {
            with_actor(entry.handle, |a| reset_face(a, &mut entry));
            crate::log(&format!("<Photo Mode> Face override cleared for {id:08X}"));
        }
    }

    /// `OnGameUpdate`: outside the studio only persistent faces keep asserting.
    pub fn on_game_update(&mut self, apply_all: bool) {
        for entry in self.actors.values_mut().filter(|e| apply_all || e.state.persistent) {
            let handle = entry.handle;
            with_actor(handle, |a| apply_entry(a, entry));
        }
    }

    /// `RevertAll`: a persistent face stays; `everything` takes those too (an unload).
    pub fn revert_all(&mut self, everything: bool) {
        self.actors.retain(|_, entry| {
            if entry.state.persistent && !everything {
                return true;
            }
            let handle = entry.handle;
            with_actor(handle, |a| reset_face(a, entry));
            false
        });
    }
}

/// The index of `name` in `names`, any case (`IndexFromName`).
#[must_use]
pub fn index_of(names: &[String], count: usize, name: &str) -> Option<usize> {
    names.iter().take(count).position(|n| n.eq_ignore_ascii_case(name))
}

/// `LoadSamExpression`'s parse: SAM's `FaceMorphs` YAML (`expression`, `modifier` and `phoneme`
/// sections of `Name: percent`) as a fresh face, the tongue kept.
#[must_use]
pub fn parse_sam_expression(text: &str, names: &FaceNames, previous: &FaceState) -> FaceState {
    let mut out = FaceState { tongue: previous.tongue.clone(), ..FaceState::default() };
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim_end();
        let Some((key, value)) = line.split_once(':') else { continue };
        if !line.starts_with([' ', '\t']) {
            key.trim().clone_into(&mut section);
            continue;
        }
        let Ok(number) = value.trim().parse::<f32>() else { continue };
        let scaled = (number / 100.0).clamp(0.0, 1.0);
        let key = key.trim();
        match section.as_str() {
            "expression" => {
                if let Some(i) = index_of(&names.expressions, names.expressions.len(), key) {
                    out.expression = i32::try_from(i).unwrap_or(NEUTRAL);
                    out.expression_value = scaled;
                }
            }
            "modifier" => {
                if let Some(i) = index_of(&names.modifiers, MODIFIER_COUNT, key) {
                    out.modifiers[i] = scaled;
                }
            }
            "phoneme" => {
                if let Some(i) = index_of(&names.phonemes, PHONEME_COUNT, key) {
                    out.phonemes[i] = scaled;
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::{FaceNames, FaceState, MODIFIER_COUNT, parse_sam_expression};
    use crate::camera::studio::adjustments::BoneDelta;

    /// The engine's tables as the RE lists them (`camera.md` IC8 H).
    pub fn names() -> FaceNames {
        let list = |s: &str| s.split(", ").map(str::to_owned).collect();
        FaceNames {
            expressions: list("DialogueAnger, DialogueFear, DialogueHappy, DialogueSad, DialogueSurprise, DialoguePuzzled, DialogueDisgusted, MoodNeutral, MoodAnger, MoodFear, MoodHappy, MoodSad, MoodSurprise, MoodPuzzled, MoodDisgusted, CombatAnger, CombatShout"),
            modifiers: list("BlinkLeft, BlinkRight, BrowDownLeft, BrowDownRight, BrowInLeft, BrowInRight, BrowUpLeft, BrowUpRight, LookDown, LookLeft, LookRight, LookUp, SquintLeft, SquintRight, HeadPitch, HeadRoll, HeadYaw"),
            phonemes: list("Aah, BigAah, BMP, ChJSh, DST, Eee, Eh, FV, I, K, N, Oh, OohQ, R, Th, W"),
        }
    }

    #[test]
    fn sam_face_morphs_map_onto_the_engine_channels() {
        let previous = FaceState { expression: 3, expression_value: 0.9, tongue: [("PA Tongue".to_owned(), BoneDelta { scale: 1.2, ..BoneDelta::default() })].into(), ..FaceState::default() };
        let yaml = "expression:\n  moodhappy: 80 # smile\nmodifier:\n  BlinkLeft: 100\n  SquintRight: 250\n  HeadYaw: 50\nphoneme:\n  BigAah: 35.5\n  W: 10\n  Nope: 40\nother:\n  Aah: 90\n";
        let face = parse_sam_expression(yaml, &names(), &previous);
        assert_eq!((face.expression, face.expression_value), (10, 0.8), "MoodHappy, any case");
        assert!((face.modifiers[0] - 1.0).abs() < f32::EPSILON && (face.modifiers[13] - 1.0).abs() < f32::EPSILON, "clamped to 1");
        assert_eq!(face.modifiers.iter().filter(|v| **v != 0.0).count(), 2, "HeadYaw is a head-tracking channel past the {MODIFIER_COUNT} the studio drives");
        assert!((face.phonemes[1] - 0.355).abs() < 1e-6 && (face.phonemes[15] - 0.1).abs() < 1e-6 && face.phonemes[0] == 0.0);
        assert_eq!(face.tongue, previous.tongue, "the tongue survives a SAM import");
        assert!(face.suppress_blink && !face.persistent, "a fresh state otherwise");
    }
}
