// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! A graph's bones as a pose hook sees them (`crate::animation`): each skeleton bone's entry in
//! the graph's bone node array, a scene node or an entry of a flattened bone tree, with its local
//! transform as the pose write left it; and the graph's Havok skeleton, which says what each
//! bone is called and hangs from.

use sre::{BSFlattenedBoneTree, BShkbAnimationGraph, FlatBone, NiAVObject, hkaSkeleton};

use crate::animation::{BShkbAnimationGraphExt, HkQsTransform};
use crate::render::{BSFlattenedBoneTreeExt, Matrix3, NiAVObjectExt, Transform};

/// Where a bone's local transform lives.
pub enum Bone<'a> {
    Node(&'a NiAVObject),
    /// An entry of a flattened bone tree that has no node of its own.
    Flat(&'a BSFlattenedBoneTree, &'a FlatBone),
}

/// Skeleton bone `i` of `graph`. A flattened entry that has a node is that node: the pose is
/// written to the node and the tree takes the bone's world from it.
#[must_use]
pub fn bone(graph: &BShkbAnimationGraph, i: usize) -> Option<Bone<'_>> {
    let entry = graph.bone_nodes().get(i)?;
    let node = entry.node()?;
    let Some(flat) = i32::try_from(entry.flatIndex).ok().and_then(|flat| usize::try_from(flat).ok()) else { return Some(Bone::Node(node)) };
    // A flattened entry names its tree as its node.
    let tree = unsafe { &*core::ptr::from_ref(node).cast::<BSFlattenedBoneTree>() };
    let flat = BSFlattenedBoneTreeExt::bones(tree).get(flat)?;
    Some(match flat.node() {
        Some(node) => Bone::Node(node),
        None => Bone::Flat(tree, flat),
    })
}

impl Bone<'_> {
    #[must_use]
    pub fn local(&self) -> Transform {
        match self {
            Bone::Node(n) => n.local_transform(),
            Bone::Flat(_, b) => Transform::from(&b.local),
        }
    }

    /// The world transform the engine last computed for the bone: in a pose hook the frame
    /// before's.
    #[must_use]
    pub fn last_world(&self) -> Transform {
        match self {
            Bone::Node(n) => n.world_transform(),
            Bone::Flat(_, b) => Transform::from(&b.world),
        }
    }

    /// The world transform of what this bone hangs from when it is a skeleton root.
    #[must_use]
    pub fn parent_world(&self) -> Option<Transform> {
        match self {
            Bone::Node(n) => unsafe { n.parent.as_ref() }.map(|p| p.world_transform()),
            Bone::Flat(tree, _) => Some(tree.world_transform()),
        }
    }

    /// Changes the bone's local transform: a node's, or a flattened entry's, which the tree's
    /// update and the skins read the same way.
    fn update_local(&self, change: impl FnOnce(&mut Transform)) {
        let mut local = self.local();
        change(&mut local);
        match self {
            Bone::Node(n) => n.set_local_transform(local),
            Bone::Flat(_, b) => unsafe { (&raw const b.local).cast::<Transform>().cast_mut().write_volatile(local) },
        }
    }

    /// Writes the local rotation. From a pose hook, on a bone of the hooked graph only.
    pub fn set_rotation(&self, rotation: Matrix3) {
        self.update_local(|t| t.rotation = rotation);
    }

    /// Writes the local translation, under the same rule as [`set_rotation`](Self::set_rotation).
    pub fn set_translation(&self, translation: [f32; 3]) {
        self.update_local(|t| t.translation = translation);
    }
}

/// The Havok skeleton `graph` animates (`hkbCharacterSetup` +0x20).
#[must_use]
pub fn skeleton(graph: &BShkbAnimationGraph) -> Option<&hkaSkeleton> {
    let setup = graph.character.setup.cast::<u8>();
    if setup.is_null() {
        return None;
    }
    let skeleton = unsafe { setup.add(sre::member_offset!("hkbCharacterSetup.animationSkeleton")).cast::<*const hkaSkeleton>().read_unaligned() };
    unsafe { skeleton.as_ref() }
}

/// For each bone the index of the bone it hangs from, below zero for a root.
#[must_use]
pub fn parents(skeleton: &hkaSkeleton) -> &[i16] {
    let n = skeleton.parentIndexCount.min(skeleton.boneCount) as usize;
    if skeleton.parentIndices.is_null() || n == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(skeleton.parentIndices.cast::<i16>(), n) }
}

/// Each bone's parent-local transform in the pose the skeleton was built in.
#[must_use]
pub fn reference_pose(skeleton: &hkaSkeleton) -> &[HkQsTransform] {
    let n = skeleton.referencePoseCount.min(skeleton.boneCount) as usize;
    if skeleton.referencePose.is_null() || n == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(skeleton.referencePose.cast::<HkQsTransform>(), n) }
}

/// A bone's name: the 16-byte `hkaBone` record's first field, a string pointer whose low bit is a tag.
#[must_use]
pub fn bone_name(skeleton: &hkaSkeleton, i: usize) -> Option<String> {
    if skeleton.bones.is_null() || i >= skeleton.boneCount as usize {
        return None;
    }
    let tagged = unsafe { skeleton.bones.cast::<u8>().add(i * 16).cast::<usize>().read_unaligned() } & !1;
    let name = core::ptr::with_exposed_provenance::<core::ffi::c_char>(tagged);
    (!name.is_null()).then(|| unsafe { core::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned())
}
