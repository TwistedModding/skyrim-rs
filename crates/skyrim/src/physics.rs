//! Ray picks against the Havok world (RE `camera.md` request IC5 A): a `bhkPickData` filled as the
//! engine fills its own, cast by `TESObjectCELL::Pick` (`bhkWorld::PickObject` on the cell's world,
//! which takes the world's read lock itself), and the hit's position, normal, fraction, collision
//! layer and reference.
//!
//! Pick on the main thread (or on a job the engine runs Havok work on): `PickObject` reads Havok
//! thread-local state a thread the engine did not set up does not have.

use sre::raw::{fns, globals};
use sre::{CollisionLayer, PlayerCharacter, TESObjectCELL, bhkPickData};

use crate::actors::{RefHandle, TESObjectREFRExt};

/// Game units to Havok units (`g_HavokWorldScale`).
pub const HAVOK_SCALE: f32 = 0.014_287_5;
/// Havok units to game units (`g_HavokWorldScaleInverse`).
pub const HAVOK_SCALE_INVERSE: f32 = 69.991_25;

/// Offset of the filter info in a collidable (`bhkPickData::rootCollidable`'s row).
const COLLIDABLE_FILTER_INFO: usize = 0x2c;
/// Offset of the group counter in the collision filter (`bhkCollisionFilter::nextCollisionGroup`).
const NEXT_COLLISION_GROUP: usize = 0x4c;

/// Where a ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Game units.
    pub position: [f32; 3],
    /// World space, unit length.
    pub normal: [f32; 3],
    /// Share of `from` to `to` at the hit.
    pub fraction: f32,
    /// The hit body's layer; `None` for a layer the binary's table does not name.
    pub layer: Option<CollisionLayer>,
    /// The reference that owns the hit body; `None` for the land, the ground layer and loose
    /// geometry.
    pub reference: Option<RefHandle>,
}

/// A ray's collision filter info: the layer in bits 0-6, the group in bits 16-31. A group of 0
/// collides with every layer whatever the layer matrix says, so a ray needs a non-zero one.
#[must_use]
pub const fn filter_info(layer: CollisionLayer, group: u16) -> u32 {
    (layer as u32 & 0x7f) | (group as u32) << 16
}

/// The layer of a filter info.
#[must_use]
pub const fn layer_of(filter_info: u32) -> Option<CollisionLayer> {
    CollisionLayer::from_u32(filter_info & 0x7f)
}

/// The group the engine's counter hands out after `counter`: the next 16-bit value, 10 in place
/// of 0 (`CastRayGetHitDistance`).
#[must_use]
pub const fn next_group(counter: u32) -> u16 {
    match counter.wrapping_add(1) as u16 {
        0 => 10,
        group => group,
    }
}

/// A fresh collision group from the world filter's counter, as the engine takes one for its own
/// rays: a group no body belongs to. `None` before the filter exists.
#[must_use]
pub fn fresh_group() -> Option<u16> {
    let holder = crate::reloc().address(globals::g_bhkCollisionFilter_ptr).ok()? as *const *const u8;
    let filter = unsafe { *holder };
    if filter.is_null() {
        return None;
    }
    let counter = unsafe { &*filter.add(NEXT_COLLISION_GROUP).cast::<u32>() };
    let group = next_group(*counter);
    crate::store(counter, u32::from(group));
    Some(group)
}

/// The point `fraction` of the way from `from` to `to`.
#[must_use]
pub fn lerp(from: [f32; 3], to: [f32; 3], fraction: f32) -> [f32; 3] {
    core::array::from_fn(|i| from[i] + (to[i] - from[i]) * fraction)
}

fn havok_vector(p: [f32; 3]) -> [u8; 16] {
    let mut out = [0; 16];
    for (chunk, v) in out.chunks_exact_mut(4).zip(p) {
        chunk.copy_from_slice(&(v * HAVOK_SCALE).to_le_bytes());
    }
    out
}

fn vector(bytes: &[u8; 16]) -> [f32; 3] {
    core::array::from_fn(|i| f32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]]))
}

#[repr(C, align(16))]
struct Aligned(bhkPickData);

/// The pick record for a ray from `from` to `to` (game units) with `filter_info`.
fn pick_data(from: [f32; 3], to: [f32; 3], filter_info: u32) -> Aligned {
    let mut shape_keys = [0; 32];
    shape_keys[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    shape_keys[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    Aligned(bhkPickData {
        from: havok_vector(from),
        to: havok_vector(to),
        enableShapeCollectionFilter: 0,
        unknown_21: [0; 3],
        filterInfo: filter_info,
        field_28: 0,
        normal: [0; 16],
        hitFraction: 1.0,
        extraInfo: u32::MAX,
        shapeKeys: shape_keys,
        unknown_68: [0; 8],
        shapeKeyIndex: 0,
        unknown_74: [0; 12],
        rootCollidable: core::ptr::null_mut(),
        unknown_88: [0; 8],
        ray: [0; 16],
        rayCache: core::ptr::null_mut(),
        closestCollector: core::ptr::null_mut(),
        allHitsCollector: core::ptr::null_mut(),
        allHitsCollector2: core::ptr::null_mut(),
        skipped: 0,
        unknown_C1: [0; 15],
        __engine: sre::Engine::new(),
    })
}

/// Casts a ray from `from` to `to` (game units) through `cell`'s physics world with
/// `filter_info` ([`filter_info`]); the closest hit, if any.
#[must_use]
pub fn pick_in(cell: &TESObjectCELL, from: [f32; 3], to: [f32; 3], filter_info: u32) -> Option<Hit> {
    let mut data = pick_data(from, to, filter_info);
    let cast: fns::TESObjectCELL__Pick_fn = unsafe { crate::engine_fn(fns::TESObjectCELL__Pick) };
    let node = unsafe { cast(core::ptr::from_ref(cell).cast_mut(), &raw mut data.0) };
    let pick = &data.0;
    if pick.rootCollidable.is_null() {
        return None;
    }
    let layer = layer_of(unsafe { pick.rootCollidable.cast::<u8>().add(COLLIDABLE_FILTER_INFO).cast::<u32>().read_unaligned() });
    // On a ground hit the cell's pick returns an object that is not a scene node.
    let reference = if node.is_null() || layer == Some(CollisionLayer::GROUND) {
        None
    } else {
        let find: fns::FindReferenceFromNode_fn = unsafe { crate::engine_fn(fns::FindReferenceFromNode) };
        unsafe { find(node).as_ref() }.map(TESObjectREFRExt::handle)
    };
    Some(Hit { position: lerp(from, to, pick.hitFraction), normal: vector(&pick.normal), fraction: pick.hitFraction, layer, reference })
}

/// [`pick_in`] the player's cell.
#[must_use]
pub fn pick(from: [f32; 3], to: [f32; 3], filter_info: u32) -> Option<Hit> {
    let cell = unsafe { PlayerCharacter::singleton()?.parentCell.as_ref() }?;
    pick_in(cell, from, to, filter_info)
}

#[cfg(test)]
mod tests {
    use sre::CollisionLayer;

    use super::{HAVOK_SCALE, HAVOK_SCALE_INVERSE, filter_info, havok_vector, layer_of, lerp, next_group, pick_data, vector};

    #[test]
    fn havok_units_are_game_units_scaled_and_back() {
        assert!((HAVOK_SCALE * HAVOK_SCALE_INVERSE - 1.0).abs() < 1e-6);
        let bytes = havok_vector([70.0, -140.0, 0.0]);
        let back = vector(&bytes).map(|v| v * HAVOK_SCALE_INVERSE);
        assert!((back[0] - 70.0).abs() < 1e-3 && (back[1] + 140.0).abs() < 1e-3 && back[2] == 0.0);
        assert_eq!(&bytes[12..], &[0; 4]);
    }

    #[test]
    fn the_filter_word_holds_the_layer_low_and_the_group_high() {
        assert_eq!(filter_info(CollisionLayer::CAMERAPICK, 0x1234), 0x1234_0027);
        assert_eq!(filter_info(CollisionLayer::PATHPICK, 1), 0x0001_002a);
        assert_eq!(layer_of(0x0003_8027), Some(CollisionLayer::CAMERAPICK));
        assert_eq!(layer_of(0x7f), None);
    }

    #[test]
    fn a_fresh_group_is_never_zero() {
        assert_eq!(next_group(9), 10);
        assert_eq!(next_group(0xffff), 10);
        assert_eq!(next_group(0x1_0005), 6);
    }

    #[test]
    fn the_hit_lies_on_the_ray_at_its_fraction() {
        assert_eq!(lerp([0.0, 0.0, 0.0], [100.0, -50.0, 20.0], 0.25), [25.0, -12.5, 5.0]);
        assert_eq!(lerp([1.0, 2.0, 3.0], [9.0, 9.0, 9.0], 0.0), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_record_starts_as_the_engine_fills_it() {
        let data = pick_data([0.0; 3], [0.0, 0.0, -100.0], 0x10_0027);
        let pick = &data.0;
        assert_eq!(core::ptr::from_ref(pick).addr() % 16, 0);
        assert_eq!((pick.hitFraction, pick.extraInfo, pick.filterInfo), (1.0, u32::MAX, 0x10_0027));
        assert_eq!((&pick.shapeKeys[..4], &pick.shapeKeys[8..12]), (&[0xff; 4][..], &[0xff; 4][..]));
        assert!(pick.rootCollidable.is_null() && pick.closestCollector.is_null() && pick.skipped == 0);
    }
}
