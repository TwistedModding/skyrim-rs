//! `body/state/BodyHandsState`: what the hands do (weapon state, casting, attacking, aiming), what
//! each hand holds and the interaction animation playing, with the profile offset of the hands state.

use crate::settings::BodyHands;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandsType {
    None = 0,
    Sheath,
    Show,
    Aim,
    Attack,
    Block,
    Cast,
    PowerAttack,
}

impl HandsType {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Sheath,
            2 => Self::Show,
            3 => Self::Aim,
            4 => Self::Attack,
            5 => Self::Block,
            6 => Self::Cast,
            7 => Self::PowerAttack,
            _ => Self::None,
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquipmentType {
    Fist = 0,
    Sword,
    Dagger,
    Axe,
    Mace,
    Greatsword,
    Greataxe,
    Bow,
    Staff,
    Crossbow,
    Shield,
    Spell,
    Torch,
}

impl EquipmentType {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Sword,
            2 => Self::Dagger,
            3 => Self::Axe,
            4 => Self::Mace,
            5 => Self::Greatsword,
            6 => Self::Greataxe,
            7 => Self::Bow,
            8 => Self::Staff,
            9 => Self::Crossbow,
            10 => Self::Shield,
            11 => Self::Spell,
            12 => Self::Torch,
            _ => Self::Fist,
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InteractionType {
    None = 0,
    Applaud,
    Climb,
    Container,
    Door,
    Harvest,
    Instrument,
    Knock,
    Lock,
    Loot,
    Pet,
    PickPocket,
    Pray,
    Puzzle,
    Touch,
    Unknown,
    Wave,
}

impl InteractionType {
    /// `GetInteracting`: the value of First Person Interactions' `FP_AnimTrigger` (or 1 for an
    /// unknown mod's offset movement) as the interaction it plays.
    #[must_use]
    pub const fn from_trigger(value: u32) -> Self {
        match value {
            0 => Self::None,
            10..=14 | 100..=102 | 106..=125 => Self::Loot,
            20..=22 => Self::Door,
            23..=25 => Self::Container,
            26 => Self::Climb,
            30..=33 => Self::Lock,
            40 => Self::Knock,
            50..=56 => Self::Harvest,
            60 => Self::Pray,
            70..=72 => Self::Touch,
            80..=96 => Self::Puzzle,
            103..=105 => Self::PickPocket,
            130..=132 => Self::Pet,
            140 => Self::Wave,
            150 => Self::Applaud,
            200..=202 => Self::Instrument,
            _ => Self::Unknown,
        }
    }
}

/// `HandsState`: the offset is the profile's for the state, taken each update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandsState {
    pub kind: HandsType,
    pub(crate) offset: [f32; 3],
}

impl Default for HandsState {
    fn default() -> Self {
        Self { kind: HandsType::None, offset: [0.0; 3] }
    }
}

impl HandsState {
    #[must_use]
    pub const fn enter(kind: HandsType, previous: &Self) -> Self {
        Self { kind, offset: previous.offset() }
    }

    pub const fn update(&mut self, h: &BodyHands) {
        let offset = match self.kind {
            HandsType::None => return,
            HandsType::Sheath => h.Sheath,
            HandsType::Show => h.Show,
            HandsType::Aim => h.Aim,
            HandsType::Attack => h.Attack,
            HandsType::Block => h.Block,
            HandsType::Cast => h.Cast,
            HandsType::PowerAttack => h.PowerAttack,
        };
        self.offset = offset.point();
    }

    #[must_use]
    pub const fn offset(&self) -> [f32; 3] {
        if matches!(self.kind, HandsType::None) { [0.0; 3] } else { self.offset }
    }
}

#[cfg(test)]
mod tests {
    use super::{HandsState, HandsType, InteractionType};
    use crate::settings::BodyHands;

    #[test]
    fn interaction_ranges() {
        assert_eq!(InteractionType::from_trigger(0), InteractionType::None);
        assert_eq!(InteractionType::from_trigger(12), InteractionType::Loot);
        assert_eq!(InteractionType::from_trigger(110), InteractionType::Loot);
        assert_eq!(InteractionType::from_trigger(104), InteractionType::PickPocket);
        assert_eq!(InteractionType::from_trigger(201), InteractionType::Instrument);
        assert_eq!(InteractionType::from_trigger(1), InteractionType::Unknown);
    }

    #[test]
    fn hands_take_the_profile_offset_without_a_blend() {
        let mut state = HandsState::enter(HandsType::PowerAttack, &HandsState::default());
        state.update(&BodyHands::default());
        assert_eq!(state.offset(), [-6.4, -12.6, 0.0]);
        assert_eq!(HandsState::enter(HandsType::None, &state).offset(), [0.0; 3]);
    }
}
