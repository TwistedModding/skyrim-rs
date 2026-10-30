//! The state a hot reload hands from one build to the next: the body manager (states, offsets,
//! the full scales of the parts it shrinks and the frozen pose), the camera manager's state type,
//! modifier, mode, look and FOV and near-plane state, the engine values the old build had
//! written, FSMP's interface it received, and the API's owners and consumers' callbacks. The new build makes the state object again without its `OnEnter` (what a state keeps
//! of its own starts over) and with no transition running. Little-endian fields behind a version
//! byte; bytes of another version are ignored.

use crate::api::provider::Saved as Api;
use crate::body::manager::{DirectionType, Forced, Manager as Body};
use crate::body::parts::Scales;
use crate::body::state::action::{ActionState, ActionType};
use crate::body::state::hands::{EquipmentType, HandsState, HandsType, InteractionType};
use crate::body::state::movement::{MovementState, MovementType};
use crate::body::state::race::{RaceState, RaceType};
use crate::camera::manager::Manager as Camera;
use crate::camera::state::{CameraMode, CameraType};
use crate::camera::states::State;
use crate::camera::util::look::LookState;

const VERSION: u8 = 6;

const MODES: [CameraMode; 4] = [CameraMode::First, CameraMode::Head, CameraMode::Third, CameraMode::None];

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

    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
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

    fn u64(&mut self) -> Option<u64> {
        let (head, rest) = self.0.split_first_chunk::<8>()?;
        self.0 = rest;
        Some(u64::from_le_bytes(*head))
    }
}

#[must_use]
pub fn save(body: &Body, camera: &Camera, written: Written, fsmp: u64, api: &Api) -> Vec<u8> {
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
    let c = &camera.current;
    w.u32(camera.state.kind() as u32);
    w.u32(camera.previous_camera as u32);
    w.u32(MODES.iter().zip(0u32..).find(|(m, _)| **m == c.mode).map_or(3, |(_, i)| i));
    w.v3(c.modifier.local_offset);
    for v in [c.modifier.fov, camera.look.state.yaw, camera.look.state.pitch, camera.external_fov, camera.resolve_fov, camera.pending_fov_reset, camera.zoom_modifier, camera.near, written.world_fov, written.near] {
        w.f32(v);
    }
    w.u32(u32::from(camera.defer_fov) | u32::from(camera.zoom_active) << 1 | u32::from(c.modifier.sim_first_person) << 2 | u32::from(c.first_person) << 3);
    w.u64(fsmp);
    for v in [api.camera_owner, api.crosshair_owner, u32::from(api.unlocked_horse_aim), u32::from(api.pov.cast_unsigned()), api.thread_id, u32::try_from(api.callbacks.len()).unwrap_or(0)] {
        w.u32(v);
    }
    for &(handle, callback) in &api.callbacks {
        w.u32(handle);
        w.u64(callback);
    }
    w.0
}

/// The camera manager the new build continues from.
fn camera(kind: CameraType, previous: CameraType, mode: CameraMode, offset: [f32; 3], f: [f32; 8], flags: u32) -> Camera {
    let (state, look_mode) = State::new(kind);
    let mut camera = Camera::default();
    (camera.state, camera.previous_camera, camera.defer_fov, camera.zoom_active) = (state, previous, flags & 1 != 0, flags & 2 != 0);
    (camera.external_fov, camera.resolve_fov, camera.pending_fov_reset, camera.zoom_modifier, camera.near) = (f[3], f[4], f[5], f[6], f[7]);
    camera.look.mode = look_mode;
    camera.look.state = LookState { yaw: f[1], pitch: f[2] };
    let c = &mut camera.current;
    c.camera = kind;
    c.mode = mode;
    c.first_person = flags & 8 != 0;
    c.modifier.fov = f[0];
    c.modifier.local_offset = offset;
    c.modifier.sim_first_person = flags & 4 != 0;
    camera.present = camera.current;
    camera
}

#[must_use]
pub fn restore(bytes: &[u8]) -> Option<(Body, Camera, Written, u64, Api)> {
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
    let mode = *MODES.get(r.u32()? as usize)?;
    let offset = r.v3()?;
    let f = [r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let (world_fov, written_near) = (r.f32()?, r.f32()?);
    let flags = r.u32()?;
    let camera = camera(kind, previous, mode, offset, f, flags);
    let fsmp = r.u64()?;
    let mut api = Api { camera_owner: r.u32()?, crosshair_owner: r.u32()?, unlocked_horse_aim: r.u32()? != 0, pov: u8::try_from(r.u32()?).ok()?.cast_signed(), thread_id: r.u32()?, callbacks: Vec::new() };
    for _ in 0..r.u32()? {
        api.callbacks.push((r.u32()?, r.u64()?));
    }
    r.0.is_empty().then_some((body, camera, Written { world_fov, near: written_near }, fsmp, api))
}

#[cfg(test)]
mod tests {
    use super::{Api, Written, restore, save};
    use crate::body::manager::{DirectionType, Forced, Manager as Body};
    use crate::body::state::action::{ActionState, ActionType};
    use crate::body::state::hands::{EquipmentType, InteractionType};
    use crate::body::state::race::RaceType;
    use crate::camera::manager::Manager as Camera;
    use crate::camera::state::{CameraMode, CameraType, Modifier};
    use crate::camera::states::State;
    use crate::camera::util::look::LookMode;
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
        let mut camera = Camera::default();
        (camera.state, camera.previous_camera, camera.defer_fov, camera.zoom_active) = (State::new(CameraType::Horse).0, CameraType::ThirdPerson, true, true);
        (camera.external_fov, camera.resolve_fov, camera.pending_fov_reset, camera.zoom_modifier, camera.near) = (70.0, 84.5, 65.0, -12.5, 6.25);
        camera.current.modifier = Modifier { fov: 90.0, local_offset: [1.0, 2.0, 3.0], sim_first_person: true };
        camera.current.mode = CameraMode::Third;
        camera.look.state.pitch = 0.3;
        let written = Written { world_fov: 84.5, near: 7.0 };
        let api = Api { camera_owner: 12, crosshair_owner: u32::MAX, unlocked_horse_aim: true, pov: -1, thread_id: 4321, callbacks: vec![(12, 0x7ff6_0000_1000), (30, 0x7ff6_0000_2000)] };
        let (b, c, w, fsmp, a) = restore(&save(&body, &camera, written, 0x7ff6_1234_5678, &api)).unwrap();
        assert_eq!((b, w, fsmp, a), (body, written, 0x7ff6_1234_5678, api));
        assert_eq!(c.state.kind(), CameraType::Horse);
        assert_eq!(c.look.mode, LookMode::ThirdPerson);
        assert_eq!((c.current.modifier, c.current.mode, c.present.modifier), (camera.current.modifier, CameraMode::Third, camera.current.modifier));
        assert_eq!((c.previous_camera, c.defer_fov, c.zoom_active, c.near.to_bits(), c.look.state.pitch.to_bits()), (CameraType::ThirdPerson, true, true, 6.25f32.to_bits(), 0.3f32.to_bits()));
        assert_eq!((c.external_fov, c.resolve_fov, c.pending_fov_reset, c.zoom_modifier), (70.0, 84.5, 65.0, -12.5));
    }

    #[test]
    fn other_versions_and_short_bytes_are_ignored() {
        let bytes = save(&Body::default(), &Camera::default(), Written { world_fov: 80.0, near: 15.0 }, 0, &Api::default());
        assert!(restore(&bytes[..bytes.len() - 1]).is_none());
        let mut other = bytes;
        other[0] = 0;
        assert!(restore(&other).is_none());
        assert!(restore(&[]).is_none());
    }
}
