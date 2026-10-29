//! `body/state/BodyRaceState.h`: the player's race, by form id for the vanilla races and by IC's
//! own tags for the mod races found at the data load, with its static body offset.

use crate::settings::{BodyRace, Offset};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaceType {
    Unknown = 0x0,
    Argonian = 0x0001_3740,
    ArgonianVampire = 0x0008_883A,
    Breton = 0x0001_3741,
    BretonVampire = 0x0008_883C,
    Darkelf = 0x0001_3742,
    DarkelfVampire = 0x0008_883D,
    Highelf = 0x0001_3743,
    HighelfVampire = 0x0008_8840,
    Imperial = 0x0001_3744,
    ImperialVampire = 0x0008_8844,
    Khajiit = 0x0001_3745,
    KhajiitVampire = 0x0008_8845,
    Nord = 0x0001_3746,
    NordVampire = 0x0008_8794,
    Orc = 0x0001_3747,
    OrcVampire = 0x000A_82B9,
    Redguard = 0x0001_3748,
    RedguardVampire = 0x0008_8846,
    Woodelf = 0x0001_3749,
    WoodelfVampire = 0x0008_8884,
    Werewolf = 0x000C_DD84,
    Vampirelord = 0x0200_283A,
    Werebear = 0x0401_E17B,
    Lich = 0x1,
    HalfDragon = 0x2,
    HalfDragonVampire = 0x3,
    Hornedlord = 0x4,
    Deer = 0x5,
}

const ALL: [RaceType; 29] = [
    RaceType::Unknown,
    RaceType::Argonian,
    RaceType::ArgonianVampire,
    RaceType::Breton,
    RaceType::BretonVampire,
    RaceType::Darkelf,
    RaceType::DarkelfVampire,
    RaceType::Highelf,
    RaceType::HighelfVampire,
    RaceType::Imperial,
    RaceType::ImperialVampire,
    RaceType::Khajiit,
    RaceType::KhajiitVampire,
    RaceType::Nord,
    RaceType::NordVampire,
    RaceType::Orc,
    RaceType::OrcVampire,
    RaceType::Redguard,
    RaceType::RedguardVampire,
    RaceType::Woodelf,
    RaceType::WoodelfVampire,
    RaceType::Werewolf,
    RaceType::Vampirelord,
    RaceType::Werebear,
    RaceType::Lich,
    RaceType::HalfDragon,
    RaceType::HalfDragonVampire,
    RaceType::Hornedlord,
    RaceType::Deer,
];

impl RaceType {
    /// The race a form id or tag names; every other id is `Unknown`, as IC's custom races are.
    #[must_use]
    pub fn from_u32(id: u32) -> Self {
        ALL.into_iter().find(|r| *r as u32 == id).unwrap_or(Self::Unknown)
    }

    /// `RaceOffset`: the profile's offset for this race, none for the races without one.
    #[must_use]
    pub const fn offset(self, race: &BodyRace) -> Option<Offset> {
        Some(match self {
            Self::Argonian => race.Argonian,
            Self::ArgonianVampire => race.ArgonianVampire,
            Self::Breton => race.Breton,
            Self::BretonVampire => race.BretonVampire,
            Self::Darkelf => race.Darkelf,
            Self::DarkelfVampire => race.DarkelfVampire,
            Self::Highelf => race.Highelf,
            Self::HighelfVampire => race.HighelfVampire,
            Self::Imperial => race.Imperial,
            Self::ImperialVampire => race.ImperialVampire,
            Self::Khajiit => race.Khajiit,
            Self::KhajiitVampire => race.KhajiitVampire,
            Self::Nord => race.Nord,
            Self::NordVampire => race.NordVampire,
            Self::Orc => race.Orc,
            Self::OrcVampire => race.OrcVampire,
            Self::Redguard => race.Redguard,
            Self::RedguardVampire => race.RedguardVampire,
            Self::Woodelf => race.Woodelf,
            Self::WoodelfVampire => race.WoodelfVampire,
            Self::Werewolf => race.Werewolf,
            Self::Vampirelord => race.Vampirelord,
            Self::Werebear => race.Werebear,
            Self::Lich => race.Lich,
            Self::HalfDragon => race.HalfDragon,
            Self::HalfDragonVampire => race.HalfDragonVampire,
            Self::Unknown | Self::Hornedlord | Self::Deer => return None,
        })
    }
}

/// `RaceState`: the race and the offset applied each frame, without a blend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaceState {
    pub race: RaceType,
    pub offset: [f32; 3],
}

impl Default for RaceState {
    fn default() -> Self {
        Self { race: RaceType::Unknown, offset: [0.0; 3] }
    }
}

#[cfg(test)]
mod tests {
    use super::RaceType;
    use crate::settings::{BodyRace, Offset};

    #[test]
    fn race_ids_and_offsets() {
        assert_eq!(RaceType::from_u32(0x0001_3746), RaceType::Nord);
        assert_eq!(RaceType::from_u32(0x0401_E17B), RaceType::Werebear);
        assert_eq!(RaceType::from_u32(0x00AB_CDEF), RaceType::Unknown);
        let race = BodyRace { HalfDragon: Offset { fForward: 1.0, fUp: 2.0, fRight: 3.0 }, ..BodyRace::default() };
        assert_eq!(RaceType::HalfDragon.offset(&race), Some(race.HalfDragon));
        assert_eq!(RaceType::Deer.offset(&race), None);
    }
}
