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

/// `FurnitureAnimation`: what the occupied furniture makes the player do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FurnitureAnimation {
    Crafting,
    Gathering,
    Sitting,
    Sleeping,
    Unknown,
}

/// `TESFurniture::activeMarkerFlags` (+0xe4) furniture-wide bits (RE `camera.md` request IC2 E).
const CAN_LEAN: u32 = 1 << 28;
const CAN_SIT: u32 = 1 << 30;
const CAN_SLEEP: u32 = 1 << 31;

/// `FurnitureGatherNames`: the models of the woodchopping block, the mill log pile and the pickaxe markers.
const GATHER_MODELS: [&str; 5] = [
    "Furniture\\Clutter\\MillLogPile.nif",
    "Furniture\\Clutter\\WoodChoppingBlock.nif",
    "Furniture\\PickaxeFloorMarker.nif",
    "Furniture\\PickaxeTableMarker.nif",
    "Furniture\\PickaxeWallMarker.nif",
];

/// Whether `model`, a path relative to `Data\Meshes`, is gathering furniture's. The engine's
/// string pool folds case, so the path may come back in another case than the plugin spelled it.
#[must_use]
pub fn is_gather_model(model: &str) -> bool {
    GATHER_MODELS.iter().any(|m| m.eq_ignore_ascii_case(model))
}

impl FurnitureAnimation {
    /// From the furniture's workbench type (+0xe0), whether its model is gathering furniture's,
    /// and its marker flags: a workbench, else gathering, else a bed, else a seat one cannot lean
    /// on, else an animation.
    #[must_use]
    pub const fn of(workbench: u8, gather: bool, markers: u32) -> Self {
        if workbench > 0 {
            Self::Crafting
        } else if gather {
            Self::Gathering
        } else if markers & CAN_SLEEP != 0 {
            Self::Sleeping
        } else if markers & CAN_SIT != 0 && markers & CAN_LEAN == 0 {
            Self::Sitting
        } else {
            Self::Unknown
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
    use super::{ActionState, ActionType, FurnitureAnimation, is_gather_model};
    use crate::settings::{BodyAction, Offset};

    #[test]
    fn furniture_reads_as_ic_tells_it_apart() {
        assert_eq!(FurnitureAnimation::of(0, false, 0x4000_0001), FurnitureAnimation::Sitting, "the chair of run-cam2a");
        assert_eq!(FurnitureAnimation::of(1, false, 0x6000_0001), FurnitureAnimation::Crafting, "a workbench, though it can be sat at");
        assert_eq!(FurnitureAnimation::of(0, false, 0xC000_0001), FurnitureAnimation::Sleeping, "a bed one can sit on");
        assert_eq!(FurnitureAnimation::of(0, false, 0x5000_0001), FurnitureAnimation::Unknown, "a seat one leans on");
        assert_eq!(FurnitureAnimation::of(0, false, 0x1), FurnitureAnimation::Unknown);
        assert_eq!(FurnitureAnimation::of(0, true, 0x1), FurnitureAnimation::Gathering);
        assert_eq!(FurnitureAnimation::of(0, true, 0xC000_0001), FurnitureAnimation::Gathering, "gathering before the bed flag");
        assert_eq!(FurnitureAnimation::of(1, true, 0x1), FurnitureAnimation::Crafting, "the workbench before gathering");
    }

    #[test]
    fn gathering_furniture_by_its_model_in_any_case() {
        assert!(is_gather_model("Furniture\\Clutter\\WoodChoppingBlock.nif"));
        assert!(is_gather_model("furniture\\clutter\\millLogPile.NIF"));
        assert!(is_gather_model("FURNITURE\\PICKAXEWALLMARKER.NIF"));
        assert!(is_gather_model("Furniture\\PickaxeFloorMarker.nif") && is_gather_model("Furniture\\PickaxeTableMarker.nif"));
        assert!(!is_gather_model("Furniture\\Common\\CommonChair01.nif"), "the chair of run-ic6");
        assert!(!is_gather_model("Furniture\\Clutter\\WoodChoppingBlock.nif.bak"));
        assert!(!is_gather_model("Meshes\\Furniture\\Clutter\\WoodChoppingBlock.nif"), "paths are relative to Data\\Meshes");
        assert!(!is_gather_model(""));
    }

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
