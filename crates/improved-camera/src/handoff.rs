//! The state a hot reload hands from one build to the next: the body manager (states, offsets,
//! the full scales of the parts it shrinks and the frozen pose), the camera manager's types and
//! FOV and near-plane state, and the engine values the old build had written. Little-endian fields
//! behind a version byte; bytes of another version are ignored.

use crate::body::manager::{DirectionType, Forced, Manager as Body};
use crate::body::parts::Scales;
use crate::body::state::action::{ActionState, ActionType};
use crate::body::state::hands::{EquipmentType, HandsState, HandsType, InteractionType};
use crate::body::state::movement::{MovementState, MovementType};
use crate::body::state::race::{RaceState, RaceType};
use crate::camera::manager::Manager as Camera;
use crate::camera::state::CameraType;

const VERSION: u8 = 3;

/// What the old build had written into the engine, which its `unload` put back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Written {
    pub world_fov: f32,
    /// `fNearDistance`.
    pub near: f32,
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    fn v3(&mut self, v: [f32; 3]) {
        for x in v {
            self.f32(x);
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u32(&mut self) -> Option<u32> {
        let (head, rest) = self.0.split_first_chunk::<4>()?;
        self.0 = rest;
        Some(u32::from_le_bytes(*head))
    }

    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }

    fn v3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
}

#[must_use]
pub fn save(body: &Body, camera: Camera, written: Written) -> Vec<u8> {
    let mut w = Writer(vec![VERSION]);
    w.u32(body.race.race as u32);
    w.v3(body.race.offset);
    w.u32(body.movement.kind as u32);
    w.v3(body.movement.offset);
    w.v3(body.movement.directional);
    w.u32(body.action.kind as u32);
    w.v3(body.action.offset);
    w.v3(body.action.target);
    w.f32(body.action.lerp);
    w.u32(body.hands.kind as u32);
    w.v3(body.hands.offset);
    let s = &body.parts.scales;
    for v in [s.fade_amount, s.head, s.left_first, s.right_first, s.left_third, s.right_third, s.quiver].into_iter().chain(s.sheathed) {
        w.f32(v);
    }
    w.u32(u32::from(body.forced.is_some()));
    let forced = body.forced.unwrap_or(Forced { translate: [0.0; 3], rotate: [[0.0; 3]; 3] });
    w.v3(forced.translate);
    forced.rotate.into_iter().for_each(|row| w.v3(row));
    w.v3(body.final_offset);
    for v in [body.left_hand as u32, body.right_hand as u32, body.force_update, body.interacting, body.prev_action as u32, body.direction as u32] {
        w.u32(v);
    }
    w.f32(body.land_timer);
    w.v3(body.directional_offset);
    w.v3(body.target_directional_offset);
    w.f32(body.key_release_timer);
    w.u32(camera.camera as u32);
    w.u32(camera.previous as u32);
    for v in [camera.fov, camera.external_fov, camera.resolve_fov, camera.pending_fov_reset, camera.zoom_modifier, camera.dialogue_fov, camera.near, written.world_fov, written.near] {
        w.f32(v);
    }
    w.u32(u32::from(camera.defer_fov) | u32::from(camera.zoom_active) << 1);
    w.0
}

#[must_use]
pub fn restore(bytes: &[u8]) -> Option<(Body, Camera, Written)> {
    let (&version, rest) = bytes.split_first()?;
    if version != VERSION {
        return None;
    }
    let mut r = Reader(rest);
    let race = RaceState { race: RaceType::from_u32(r.u32()?), offset: r.v3()? };
    let movement = MovementState { kind: MovementType::from_u32(r.u32()?), offset: r.v3()?, directional: r.v3()? };
    let action = ActionState { kind: ActionType::from_u32(r.u32()?), offset: r.v3()?, target: r.v3()?, lerp: r.f32()? };
    let hands = HandsState { kind: HandsType::from_u32(r.u32()?), offset: r.v3()? };
    let mut scales = Scales { fade_amount: r.f32()?, head: r.f32()?, left_first: r.f32()?, right_first: r.f32()?, left_third: r.f32()?, right_third: r.f32()?, quiver: r.f32()?, ..Scales::default() };
    for v in &mut scales.sheathed {
        *v = r.f32()?;
    }
    let has_forced = r.u32()? != 0;
    let forced = Forced { translate: r.v3()?, rotate: [r.v3()?, r.v3()?, r.v3()?] };
    let mut body = Body { race, movement, action, hands, forced: has_forced.then_some(forced), final_offset: r.v3()?, ..Body::default() };
    body.parts.scales = scales;
    body.left_hand = EquipmentType::from_u32(r.u32()?);
    body.right_hand = EquipmentType::from_u32(r.u32()?);
    body.force_update = r.u32()?;
    body.interacting = r.u32()?;
    body.interaction = InteractionType::from_trigger(body.interacting);
    body.prev_action = ActionType::from_u32(r.u32()?);
    body.direction = DirectionType::from_u32(r.u32()?);
    body.land_timer = r.f32()?;
    body.directional_offset = r.v3()?;
    body.target_directional_offset = r.v3()?;
    body.key_release_timer = r.f32()?;
    let (kind, previous) = (CameraType::from_u32(r.u32()?), CameraType::from_u32(r.u32()?));
    let [fov, external_fov, resolve_fov, pending_fov_reset, zoom_modifier, dialogue_fov, near, world_fov, written_near] = [r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let flags = r.u32()?;
    let camera = Camera { camera: kind, previous, fov, defer_fov: flags & 1 != 0, external_fov, resolve_fov, pending_fov_reset, zoom_active: flags & 2 != 0, zoom_modifier, dialogue_fov, near };
    r.0.is_empty().then_some((body, camera, Written { world_fov, near: written_near }))
}

#[cfg(test)]
mod tests {
    use super::{Written, restore, save};
    use crate::body::manager::{DirectionType, Forced, Manager as Body};
    use crate::body::state::action::{ActionState, ActionType};
    use crate::body::state::hands::{EquipmentType, InteractionType};
    use crate::body::state::race::RaceType;
    use crate::camera::manager::Manager as Camera;
    use crate::camera::state::CameraType;
    use crate::settings::BodyAction;

    #[test]
    fn a_reload_gets_back_what_the_old_build_had() {
        let mut body = Body::default();
        body.race.race = RaceType::HalfDragon;
        body.race.offset = [1.0, 2.0, 3.0];
        body.action = ActionState::enter(ActionType::Sneak, &ActionState::default(), &BodyAction::default());
        body.parts.scales.head = 1.15;
        body.parts.scales.sheathed[8] = 0.9;
        body.forced = Some(Forced { translate: [4.0, 5.0, 6.0], rotate: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] });
        body.left_hand = EquipmentType::Shield;
        body.interacting = 104;
        body.interaction = InteractionType::PickPocket;
        body.direction = DirectionType::BackLeft;
        let camera = Camera { camera: CameraType::FirstPerson, previous: CameraType::ThirdPerson, fov: 90.0, defer_fov: true, external_fov: 70.0, resolve_fov: 84.5, pending_fov_reset: 65.0, zoom_active: true, zoom_modifier: -12.5, dialogue_fov: 65.0, near: 6.25 };
        let written = Written { world_fov: 84.5, near: 7.0 };
        assert_eq!(restore(&save(&body, camera, written)), Some((body, camera, written)));
    }

    #[test]
    fn other_versions_and_short_bytes_are_ignored() {
        let bytes = save(&Body::default(), Camera::default(), Written { world_fov: 80.0, near: 15.0 });
        assert!(restore(&bytes[..bytes.len() - 1]).is_none());
        let mut other = bytes;
        other[0] = 0;
        assert!(restore(&other).is_none());
        assert!(restore(&[]).is_none());
    }
}
