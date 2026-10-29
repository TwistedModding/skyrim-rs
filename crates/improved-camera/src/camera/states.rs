//! The FOV part of IC's camera states (`camera/base/*.cpp`, `camera/extended/*.cpp`): the FOV each
//! state's `OnUpdate` writes into the modifier. Their entry and exit FOV work is in the manager;
//! the rest of each state (offsets, zoom, controls) comes with the camera states' own port.

use super::state::CameraType;
use crate::settings::Fov;

/// What a state reads besides the profile's FOV section.
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    /// The body's hands are past `None` (`FirstPersonState`, fake first person).
    pub hands_drawn: bool,
    /// `bEnableFakeFirstPerson` with the third-person state parked at the fake first-person zoom.
    pub fake_first_person: bool,
    /// The camera's world FOV, which the free cameras pass through.
    pub world_fov: f32,
}

/// `a_modifier.fov` as the state of `camera` sets it, `None` for the states that leave it.
#[must_use]
pub fn fov(camera: CameraType, fov: &Fov, inputs: Inputs) -> Option<f32> {
    let first_person = if inputs.hands_drawn { fov.fFirstPersonCombat } else { fov.fFirstPerson };
    Some(match camera {
        CameraType::FirstPerson => first_person,
        CameraType::ThirdPerson if inputs.fake_first_person => first_person,
        CameraType::ThirdPerson | CameraType::Spectator | CameraType::Vanity => fov.fThirdPerson,
        CameraType::VATS => fov.fKillmove,
        CameraType::Free | CameraType::SexTFC => inputs.world_fov,
        CameraType::Furniture => fov.fFurniture,
        CameraType::Horse => fov.fHorse,
        CameraType::HorseMounting | CameraType::HorseDismounting => fov.fHorseTransition,
        CameraType::Bleedout => fov.fRagdoll,
        CameraType::Dead => fov.fDeath,
        CameraType::Dragon => fov.fDragon,
        CameraType::DragonMounting | CameraType::DragonDismounting => fov.fDragonTransition,
        CameraType::Crafting | CameraType::Gathering => fov.fCrafting,
        CameraType::Vampirelord | CameraType::VampirelordTransition => fov.fVampireLord,
        CameraType::Werewolf | CameraType::WerewolfTransition => fov.fWerewolf,
        CameraType::Lich | CameraType::LichTransition => fov.fLich,
        CameraType::Sex => fov.fSex,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{Inputs, fov};
    use crate::camera::state::CameraType;
    use crate::settings::Fov;

    #[test]
    fn each_state_reads_its_own_option() {
        let profile = Fov { fFirstPerson: 70.0, fFirstPersonCombat: 75.0, fThirdPerson: 85.0, fHorseTransition: 60.0, fCrafting: 50.0, ..Fov::default() };
        let plain = Inputs { hands_drawn: false, fake_first_person: false, world_fov: 42.0 };
        let fov = |camera, inputs| fov(camera, &profile, inputs);
        assert_eq!(fov(CameraType::FirstPerson, plain), Some(70.0));
        assert_eq!(fov(CameraType::FirstPerson, Inputs { hands_drawn: true, ..plain }), Some(75.0));
        assert_eq!(fov(CameraType::ThirdPerson, plain), Some(85.0));
        assert_eq!(fov(CameraType::ThirdPerson, Inputs { fake_first_person: true, ..plain }), Some(70.0));
        assert_eq!(fov(CameraType::HorseDismounting, plain), Some(60.0));
        assert_eq!(fov(CameraType::Gathering, plain), Some(50.0));
        assert_eq!(fov(CameraType::Free, plain), Some(42.0));
        assert_eq!(fov(CameraType::TweenMenu, plain), None);
        assert_eq!(fov(CameraType::Dialogue, plain), None);
    }
}
