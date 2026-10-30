//! `camera/studio/StudioShaders`: effect shaders and art objects held on the target for a shot,
//! the glows, membranes and auras the game shows only for a spell's duration. Held with no
//! duration and torn down when the studio closes, since a stuck effect follows the actor for the
//! rest of the session.

use skyrim::actors::RefHandle;
use skyrim::forms::{TESDataHandlerExt, TESFormExt};
use skyrim::render;
use sre::{BGSArtObject, FormType, TESEffectShader};

use super::environment::form_label;

/// One effect held on a reference (`Applied`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    pub handle: RefHandle,
    pub effect: u32,
    /// An art object rather than an effect shader.
    pub art: bool,
}

/// One entry of the browser (`EffectInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectInfo {
    pub form_id: u32,
    pub label: String,
}

/// `CollectEffects`: every art object or every effect shader, sorted by label.
#[must_use]
pub fn effects(art: bool) -> Vec<EffectInfo> {
    let Some(data) = sre::TESDataHandler::singleton() else { return Vec::new() };
    let mut list: Vec<EffectInfo> = data.forms_of_type(if art { FormType::ARTO } else { FormType::EFSH }).iter().flatten().map(|f| EffectInfo { form_id: f.formID, label: form_label(f) }).collect();
    list.sort_by(|a, b| a.label.cmp(&b.label));
    list
}

/// `ShaderEngine`: what the studio holds, so it can take it off again.
#[derive(Clone, Debug, Default)]
pub struct ShaderEngine {
    applied: Vec<Applied>,
}

impl ShaderEngine {
    #[must_use]
    pub fn is_applied(&self, entry: Applied) -> bool {
        self.applied.contains(&entry)
    }

    /// `Apply`: false when it is already held.
    pub fn apply(&mut self, entry: Applied) -> bool {
        if entry.effect == 0 || self.is_applied(entry) {
            return false;
        }
        self.applied.push(entry);
        true
    }

    /// `Remove`: false when it was not held.
    pub fn remove(&mut self, entry: Applied) -> bool {
        let before = self.applied.len();
        self.applied.retain(|a| *a != entry);
        self.applied.len() != before
    }

    /// `RemoveAll`'s list, the engine left to stop them.
    pub fn take_all(&mut self) -> Vec<Applied> {
        core::mem::take(&mut self.applied)
    }
}

/// Starts a held effect (-1 duration: until it is stopped). Main thread.
pub fn play(entry: Applied) {
    let (Some(target), Some(form)) = (entry.handle.lookup(), sre::find_form_by_id(entry.effect)) else { return };
    if entry.art {
        if let Some(art) = form.cast::<BGSArtObject>() {
            render::play_art_object(&target, art, -1.0);
        }
    } else if let Some(shader) = form.cast::<TESEffectShader>() {
        render::play_effect_shader(&target, shader, -1.0);
    }
}

/// `StopEffects`. Main thread.
pub fn stop(entry: Applied) {
    render::stop_reference_effects(entry.handle.0, entry.effect, entry.art);
}

#[cfg(test)]
mod tests {
    use skyrim::actors::RefHandle;

    use super::{Applied, ShaderEngine};

    #[test]
    fn an_effect_is_held_once_and_taken_off_with_the_rest() {
        let mut engine = ShaderEngine::default();
        let glow = Applied { handle: RefHandle(0x10_0001), effect: 0x2_0000, art: false };
        let aura = Applied { art: true, ..glow };
        assert!(engine.apply(glow) && !engine.apply(glow), "held once");
        assert!(engine.apply(aura) && engine.is_applied(aura), "an art object of the same id is another entry");
        assert!(!engine.apply(Applied { effect: 0, ..glow }));
        assert!(engine.remove(glow) && !engine.remove(glow) && !engine.is_applied(glow));
        assert_eq!(engine.take_all(), [aura]);
        assert_eq!(engine.take_all(), []);
    }
}
