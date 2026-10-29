//! `body/BodySnapshot`: the body manager's states of the last live frame, read by the camera
//! manager and the body parts without the manager's lock.

use std::sync::Mutex;

use super::manager::DirectionType;
use super::state::action::ActionType;
use super::state::hands::{EquipmentType, HandsType, InteractionType};
use super::state::movement::MovementType;
use super::state::race::RaceType;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Data {
    pub race: RaceType,
    pub movement: MovementType,
    pub direction: DirectionType,
    pub action: ActionType,
    pub hands: HandsType,
    pub left_hand: EquipmentType,
    pub right_hand: EquipmentType,
    pub interaction: InteractionType,
    /// The race carries the `ActorTypeNPC` keyword, and with it the character head-track rig.
    pub humanoid: bool,
}

impl Data {
    pub const NONE: Self = Self {
        race: RaceType::Unknown,
        movement: MovementType::None,
        direction: DirectionType::None,
        action: ActionType::None,
        hands: HandsType::None,
        left_hand: EquipmentType::Fist,
        right_hand: EquipmentType::Fist,
        interaction: InteractionType::None,
        humanoid: false,
    };
}

static SNAPSHOT: Mutex<Data> = Mutex::new(Data::NONE);

pub fn set(data: Data) {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = data;
}

#[must_use]
pub fn get() -> Data {
    *SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
