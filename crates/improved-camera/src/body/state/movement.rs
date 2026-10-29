//! `body/state/BodyMovementState`: idle, walk, run and sprint, each easing the body toward its
//! profile offset; walk, run and sprint add the offset of the movement direction.

use super::approach;
use crate::body::manager::DirectionType;
use crate::settings::{BodyMovement, Offset};

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MovementType {
    None = 0,
    Idle,
    Walk,
    Run,
    Sprint,
}

impl MovementType {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Idle,
            2 => Self::Walk,
            3 => Self::Run,
            4 => Self::Sprint,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementState {
    pub kind: MovementType,
    pub(crate) offset: [f32; 3],
    pub(crate) directional: [f32; 3],
}

impl Default for MovementState {
    fn default() -> Self {
        Self { kind: MovementType::None, offset: [0.0; 3], directional: [0.0; 3] }
    }
}

/// `MovementDirectional`: the walk and run table, or the sprint table.
fn directional(m: &BodyMovement, direction: DirectionType, sprint: bool) -> Offset {
    use DirectionType as D;
    match (sprint, direction) {
        (false, D::Forward) => m.DirForward,
        (false, D::ForwardRight) => m.DirForwardRight,
        (false, D::ForwardLeft) => m.DirForwardLeft,
        (false, D::Back) => m.DirBack,
        (false, D::BackRight) => m.DirBackRight,
        (false, D::BackLeft) => m.DirBackLeft,
        (false, D::Right) => m.DirRight,
        (false, D::Left) => m.DirLeft,
        (false, D::None) => m.DirDefault,
        (true, D::Forward) => m.SprintForward,
        (true, D::ForwardRight) => m.SprintForwardRight,
        (true, D::ForwardLeft) => m.SprintForwardLeft,
        (true, D::Back) => m.SprintBack,
        (true, D::BackRight) => m.SprintBackRight,
        (true, D::BackLeft) => m.SprintBackLeft,
        (true, D::Right) => m.SprintRight,
        (true, D::Left) => m.SprintLeft,
        (true, D::None) => m.SprintDefault,
    }
}

impl MovementState {
    /// The state entered from `previous`, starting from its offset.
    #[must_use]
    pub fn enter(kind: MovementType, previous: &Self, m: &BodyMovement, direction: DirectionType) -> Self {
        let mut state = Self { kind, offset: previous.offset, directional: [0.0; 3] };
        if state.is_directional() {
            state.directional = state.directional_modifier(m, direction);
        }
        state
    }

    const fn is_directional(&self) -> bool {
        matches!(self.kind, MovementType::Walk | MovementType::Run | MovementType::Sprint)
    }

    fn lerp_speed(&self, m: &BodyMovement) -> f32 {
        match self.kind {
            MovementType::None => 2.0,
            MovementType::Idle => m.fIdleLerp,
            MovementType::Walk => m.fWalkLerp,
            MovementType::Run => m.fRunLerp,
            MovementType::Sprint => m.fSprintLerp,
        }
    }

    fn directional_modifier(&self, m: &BodyMovement, direction: DirectionType) -> [f32; 3] {
        let intensity = match self.kind {
            MovementType::Walk => m.fWalkIntensity,
            MovementType::Run => m.fRunIntensity,
            MovementType::Sprint => m.fSprintIntensity,
            MovementType::None | MovementType::Idle => 1.0,
        };
        directional(m, direction, self.kind == MovementType::Sprint).point().map(|v| v * intensity)
    }

    pub fn update(&mut self, m: &BodyMovement, direction: DirectionType, dt: f32) {
        match self.kind {
            MovementType::None => {}
            MovementType::Idle => self.offset = approach(self.offset, m.Idle.point(), self.lerp_speed(m), dt),
            MovementType::Walk | MovementType::Run | MovementType::Sprint => {
                let base = match self.kind {
                    MovementType::Walk => m.WalkBase,
                    MovementType::Run => m.RunBase,
                    _ => m.SprintBase,
                }
                .point();
                let speed = self.lerp_speed(m);
                self.directional = approach(self.directional, self.directional_modifier(m, direction), speed * 1.5, dt);
                let target = [base[0] + self.directional[0], base[1] + self.directional[1], base[2] + self.directional[2]];
                self.offset = approach(self.offset, target, speed, dt);
            }
        }
    }

    /// `GetOffset`: none in the no-movement state.
    #[must_use]
    pub const fn offset(&self) -> [f32; 3] {
        if matches!(self.kind, MovementType::None) { [0.0; 3] } else { self.offset }
    }
}
