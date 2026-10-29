//! `body/state/BodyActionState`: what the player is doing (sneaking, swimming, riding, in a
//! dialogue, a mod's animation), each easing the body toward its profile offset.

use super::approach;
use crate::settings::{BodyAction, Offset};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionType {
    None = 0,
    AI,
    Animation,
    Bathe,
    BoatRide,
    CartRide,
    Craft,
    Dialogue,
    Fall,
    Fish,
    Gather,
    Giantess,
    LichTransition,
    Parkour,
    Ride,
    Sex,
    Sit,
    Sleep,
    Sneak,
    Stagger,
    Swim,
    VampirelordTransition,
    View,
    WerewolfTransition,
}

const ALL: [ActionType; 24] = [
    ActionType::None,
    ActionType::AI,
    ActionType::Animation,
    ActionType::Bathe,
    ActionType::BoatRide,
    ActionType::CartRide,
    ActionType::Craft,
    ActionType::Dialogue,
    ActionType::Fall,
    ActionType::Fish,
    ActionType::Gather,
    ActionType::Giantess,
    ActionType::LichTransition,
    ActionType::Parkour,
    ActionType::Ride,
    ActionType::Sex,
    ActionType::Sit,
    ActionType::Sleep,
    ActionType::Sneak,
    ActionType::Stagger,
    ActionType::Swim,
    ActionType::VampirelordTransition,
    ActionType::View,
    ActionType::WerewolfTransition,
];

impl ActionType {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        if value <= Self::WerewolfTransition as u32 { ALL[value as usize] } else { Self::None }
    }

    const fn target(self, a: &BodyAction) -> Offset {
        match self {
            Self::None => Offset { fForward: 0.0, fUp: 0.0, fRight: 0.0 },
            Self::AI => a.AI,
            Self::Animation => a.Animation,
            Self::Bathe => a.Bathing,
            Self::BoatRide => a.BoatRide,
            Self::CartRide => a.CartRide,
            Self::Craft => a.Craft,
            Self::Dialogue => a.Dialogue,
            Self::Fall => a.Fall,
            Self::Fish => a.Fish,
            Self::Gather => a.Gather,
            Self::Giantess => a.Giantess,
            Self::LichTransition => a.LichTransition,
            Self::Parkour => a.Parkour,
            Self::Ride => a.Ride,
            Self::Sex => a.Sex,
            Self::Sit => a.Sit,
            Self::Sleep => a.Sleep,
            Self::Sneak => a.Sneak,
            Self::Stagger => a.Stagger,
            Self::Swim => a.Swim,
            Self::VampirelordTransition => a.VampirelordTransition,
            Self::View => a.View,
            Self::WerewolfTransition => a.WerewolfTransition,
        }
    }
}

/// `ActionState`: the target is the profile's offset when the state is entered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActionState {
    pub kind: ActionType,
    pub(crate) offset: [f32; 3],
    pub(crate) target: [f32; 3],
    pub(crate) lerp: f32,
}

impl Default for ActionState {
    fn default() -> Self {
        Self { kind: ActionType::None, offset: [0.0; 3], target: [0.0; 3], lerp: 12.0 }
    }
}

impl ActionState {
    /// The state entered from `previous`; leaving an action eases back to no offset, slower out of a sneak.
    #[must_use]
    pub fn enter(kind: ActionType, previous: &Self, a: &BodyAction) -> Self {
        let lerp = match (kind, previous.kind) {
            (ActionType::None, ActionType::Sneak) => 8.0,
            (ActionType::None, _) => 12.0,
            _ => a.fLerp,
        };
        Self { kind, offset: previous.offset, target: kind.target(a).point(), lerp }
    }

    pub fn update(&mut self, dt: f32) {
        self.offset = approach(self.offset, self.target, self.lerp, dt);
    }

    #[must_use]
    pub const fn offset(&self) -> [f32; 3] {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionState, ActionType};
    use crate::settings::{BodyAction, Offset};

    #[test]
    fn an_action_eases_to_its_offset_and_back_to_none() {
        let a = BodyAction { Sneak: Offset { fForward: 0.0, fUp: -10.0, fRight: 0.0 }, ..BodyAction::default() };
        let mut sneak = ActionState::enter(ActionType::Sneak, &ActionState::default(), &a);
        for _ in 0..200 {
            sneak.update(1.0 / 60.0);
        }
        assert!((sneak.offset()[2] + 10.0).abs() < 1e-3);
        let mut none = ActionState::enter(ActionType::None, &sneak, &a);
        assert!((none.offset()[2] + 10.0).abs() < 1e-3);
        none.update(1.0 / 60.0);
        let eased = 1.0 - (-8.0f32 / 60.0).exp();
        assert!((none.offset()[2] - (-10.0 + 10.0 * eased)).abs() < 1e-4);
    }
}
