//! `camera/CameraState.h`: the camera types IC tells apart (the first thirteen are the engine's
//! player camera states by id; the rest are IC's own, derived from menus, the body's action and
//! race), the camera modes and the modifier a state's update fills.

use crate::util::point::P3;

/// `CameraMode`: how the presented camera is placed. `First` is the engine's first person, `Head`
/// anchors it to the head node (head bob), `Third` is fake first person at the head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMode {
    First,
    Head,
    Third,
    #[default]
    None,
}

/// `Modifier`: what the current state asks of the presented camera.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modifier {
    /// The FOV, 0 until a state sets one.
    pub fov: f32,
    /// The camera offset (x right, y forward, z up) the head anchor adds.
    pub local_offset: P3,
    /// Simulate first person in a third-person state (fake first person).
    pub sim_first_person: bool,
}

#[allow(clippy::upper_case_acronyms)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CameraType {
    FirstPerson = 0,
    Vanity,
    VATS,
    Free,
    IronSight,
    Furniture,
    Transition,
    TweenMenu,
    Animated,
    ThirdPerson,
    Horse,
    Bleedout,
    Dragon,
    Animation,
    Crafting,
    Dead,
    Dialogue,
    DragonDismounting,
    DragonMounting,
    Gathering,
    HorseDismounting,
    HorseMounting,
    MapMenu,
    RaceMenu,
    Spectator,
    VampirelordTransition,
    Vampirelord,
    WerewolfTransition,
    Werewolf,
    Cinematic,
    LichTransition,
    Lich,
    Sex,
    SexTFC,
    Unknown,
}

const ALL: [CameraType; 35] = [
    CameraType::FirstPerson,
    CameraType::Vanity,
    CameraType::VATS,
    CameraType::Free,
    CameraType::IronSight,
    CameraType::Furniture,
    CameraType::Transition,
    CameraType::TweenMenu,
    CameraType::Animated,
    CameraType::ThirdPerson,
    CameraType::Horse,
    CameraType::Bleedout,
    CameraType::Dragon,
    CameraType::Animation,
    CameraType::Crafting,
    CameraType::Dead,
    CameraType::Dialogue,
    CameraType::DragonDismounting,
    CameraType::DragonMounting,
    CameraType::Gathering,
    CameraType::HorseDismounting,
    CameraType::HorseMounting,
    CameraType::MapMenu,
    CameraType::RaceMenu,
    CameraType::Spectator,
    CameraType::VampirelordTransition,
    CameraType::Vampirelord,
    CameraType::WerewolfTransition,
    CameraType::Werewolf,
    CameraType::Cinematic,
    CameraType::LichTransition,
    CameraType::Lich,
    CameraType::Sex,
    CameraType::SexTFC,
    CameraType::Unknown,
];

impl CameraType {
    /// The type of an engine camera state id; ids past the engine's thirteen are `Unknown`.
    #[must_use]
    pub const fn from_state(id: u32) -> Self {
        if id <= Self::Dragon as u32 { Self::from_u32(id) } else { Self::Unknown }
    }

    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        if value <= Self::Unknown as u32 { ALL[value as usize] } else { Self::Unknown }
    }
}

#[cfg(test)]
mod tests {
    use super::CameraType;

    #[test]
    fn engine_state_ids_map_one_to_one() {
        assert_eq!(CameraType::from_state(0), CameraType::FirstPerson);
        assert_eq!(CameraType::from_state(9), CameraType::ThirdPerson);
        assert_eq!(CameraType::from_state(12), CameraType::Dragon);
        assert_eq!(CameraType::from_state(13), CameraType::Unknown);
        assert_eq!(CameraType::from_u32(CameraType::SexTFC as u32), CameraType::SexTFC);
        assert_eq!(CameraType::from_u32(99), CameraType::Unknown);
    }
}
