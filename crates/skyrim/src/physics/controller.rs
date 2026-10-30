//! A character controller's collision shape (RE `camera.md` request IC7 A): the controller's kind
//! and root shape, the shapes' types, a list's children, a capsule's end points, a convex hull's
//! vertices, new hulls from the Havok heap, and a shape swapped in through the engine's own
//! `bhkCharacterController::ReplaceShape` with the world locked for writing.
//!
//! Main thread only: Havok objects come from the calling thread's memory router, and the swaps
//! take the controller's world lock. Shapes are in Havok units ([`super::HAVOK_SCALE`]).

use core::ffi::c_void;
use core::ptr::NonNull;

use sre::raw::{globals, vtables};
use sre::{bhkCharacterController, bhkShape, bhkWorld, hkReferencedObject, hkpCharacterProxy, hkpListShape, hkpShape, hkpShapeType};
use windows_sys::Win32::System::Threading::TlsGetValue;

use crate::RwGuard;

/// The controller's Havok object: from the controller base, the proxy's `hkpCharacterProxy`
/// (proxy object +0x350) or the rigid body's `hkpCharacterRigidBody` (+0x350).
const HAVOK_CONTROLLER: [usize; 2] = [0x340, 0x350];
/// `hkpCharacterRigidBody`+0x20, the `hkpRigidBody`.
const RIGID_BODY: usize = 0x20;
/// A world object's embedded collidable, whose first field is the shape.
const COLLIDABLE_SHAPE: usize = 0x20;
/// `bhkCharacterController` slot 15, `GetbhkWorld`.
const GET_WORLD_SLOT: usize = 15;
/// The thread's memory router +0x58: the heap allocator (vtable +8 alloc, +0x10 free).
const ROUTER_HEAP: usize = 0x58;
/// `hkContainerHeapAllocator` vtable +0x20: free(ptr, bytes).
const CONTAINER_FREE_SLOT: usize = 4;

/// The two controllers (the player always has the proxy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `bhkCharProxyController`, whose controller base sits 0x10 into the object.
    Proxy,
    /// `bhkCharRigidBodyController`.
    RigidBody,
}

/// The controller's kind by the vtable of its base.
#[must_use]
pub fn kind(controller: &bhkCharacterController) -> Option<Kind> {
    match crate::class_of(controller)?.id {
        vtables::bhkCharProxyController_vtable_10::ID => Some(Kind::Proxy),
        vtables::bhkCharRigidBodyController_vtable::ID => Some(Kind::RigidBody),
        _ => None,
    }
}

fn read_ptr<T>(at: *const u8) -> *const T {
    unsafe { at.cast::<*const T>().read_unaligned() }
}

/// The shape the controller collides with: the shape phantom's (proxy) or the rigid body's.
#[must_use]
pub fn root_shape(controller: &bhkCharacterController) -> Option<&hkpShape> {
    let base = core::ptr::from_ref(controller).cast::<u8>();
    let object = match kind(controller)? {
        Kind::Proxy => {
            let proxy = unsafe { read_ptr::<hkpCharacterProxy>(base.add(HAVOK_CONTROLLER[0])).as_ref() }?;
            core::ptr::from_ref(proxy.shape_phantom()?).cast::<u8>()
        }
        Kind::RigidBody => {
            let body = read_ptr::<u8>(unsafe { base.add(HAVOK_CONTROLLER[1]) });
            if body.is_null() {
                return None;
            }
            read_ptr::<u8>(unsafe { body.add(RIGID_BODY) })
        }
    };
    if object.is_null() {
        return None;
    }
    unsafe { read_ptr::<hkpShape>(object.add(COLLIDABLE_SHAPE)).as_ref() }
}

/// The controller's physics world (slot 15).
fn world(controller: &bhkCharacterController) -> Option<&bhkWorld> {
    type GetWorld = unsafe extern "C" fn(*const bhkCharacterController) -> *const bhkWorld;
    let vtable = read_ptr::<usize>(core::ptr::from_ref(controller).cast());
    let get: GetWorld = unsafe { core::mem::transmute::<usize, GetWorld>(vtable.add(GET_WORLD_SLOT).read()) };
    unsafe { get(controller).as_ref() }
}

fn lock_world(controller: &bhkCharacterController) -> Option<RwGuard> {
    world(controller).map(|w| RwGuard::write(&w.worldLock))
}

/// A list's child entry (0x20 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ListChild {
    pub shape: *const hkpShape,
    pub filter_info: u32,
    reserved: [u32; 5],
}

const _: () = assert!(size_of::<ListChild>() == 0x20);

/// A capsule: two end points and a radius.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
}

pub trait ShapeExt {
    /// The shape's type (`hkpShape`+0x18).
    fn shape_type(&self) -> Option<hkpShapeType>;
    /// The engine's wrapper (`userData`), which every shape a controller uses has.
    fn wrapper(&self) -> Option<&bhkShape>;
    /// A list's children; empty for any other shape.
    fn children(&self) -> &[ListChild];
    /// A capsule's end points and radius.
    fn capsule(&self) -> Option<Capsule>;
    /// A capsule's material id, which its wrapper (0x28 bytes, as the controller shape's) keeps at
    /// +0x20: `CharacterBumper` is 0x5dd8a028, the only one the engine compares (RE `camera.md` IC7
    /// A, IC9 E). `None` for any other shape.
    fn material(&self) -> Option<u32>;
    /// A convex hull's vertices (x, y, z, w = 0) through `hkpConvexVerticesShape::GetVertices`
    /// (id 65093); empty for any other shape.
    fn convex_vertices(&self) -> Vec<[f32; 4]>;
}

impl ShapeExt for hkpShape {
    fn shape_type(&self) -> Option<hkpShapeType> {
        hkpShapeType::from_u32(self.type_)
    }

    fn wrapper(&self) -> Option<&bhkShape> {
        unsafe { self.userData.as_ref() }
    }

    fn children(&self) -> &[ListChild] {
        let Some(list) = as_list(self) else { return &[] };
        if list.childInfoData.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(list.childInfoData.cast::<ListChild>(), list.childInfoSize as usize) }
    }

    fn capsule(&self) -> Option<Capsule> {
        (self.shape_type()? == hkpShapeType::Capsule).then(|| {
            let c = unsafe { &*core::ptr::from_ref(self).cast::<sre::hkpCapsuleShape>() };
            Capsule { a: [c.vertexA.x, c.vertexA.y, c.vertexA.z], b: [c.vertexB.x, c.vertexB.y, c.vertexB.z], radius: c.radius }
        })
    }

    fn material(&self) -> Option<u32> {
        if self.shape_type()? != hkpShapeType::Capsule {
            return None;
        }
        self.wrapper().map(|w| unsafe { core::ptr::from_ref(w).byte_add(0x20).cast::<u32>().read() })
    }

    fn convex_vertices(&self) -> Vec<[f32; 4]> {
        if self.shape_type() != Some(hkpShapeType::ConvexVertices) {
            return Vec::new();
        }
        let convex = unsafe { &*core::ptr::from_ref(self).cast::<sre::hkpConvexVerticesShape>() };
        let mut out = HkArray { data: core::ptr::null_mut(), size: 0, capacity_and_flags: HkArray::DONT_FREE };
        unsafe { convex.get_vertices((&raw mut out).cast()) };
        let vertices = if out.data.is_null() { Vec::new() } else { unsafe { core::slice::from_raw_parts(out.data, usize::try_from(out.size).unwrap_or(0)) }.to_vec() };
        out.free();
        vertices
    }
}

fn as_list(shape: &hkpShape) -> Option<&hkpListShape> {
    (shape.shape_type()? == hkpShapeType::List).then(|| unsafe { &*core::ptr::from_ref(shape).cast::<hkpListShape>() })
}

/// An `hkArray<hkVector4>` the engine fills.
#[repr(C)]
struct HkArray {
    data: *mut [f32; 4],
    size: i32,
    capacity_and_flags: i32,
}

impl HkArray {
    /// The flag of an array whose storage is not the allocator's.
    const DONT_FREE: i32 = i32::MIN;

    /// Gives the storage back to the container heap allocator the engine grew it from.
    fn free(&mut self) {
        type Free = unsafe extern "C" fn(*const c_void, *mut c_void, i32);
        if self.data.is_null() || self.capacity_and_flags < 0 {
            return;
        }
        let Ok(allocator) = crate::reloc().address(globals::g_hkContainerHeapAllocator) else { return };
        let free: Free = unsafe { core::mem::transmute::<usize, Free>(read_ptr::<usize>(allocator as *const u8).add(CONTAINER_FREE_SLOT).read()) };
        let bytes = (self.capacity_and_flags & 0x3fff_ffff) * 16;
        unsafe { free(allocator as *const c_void, self.data.cast(), bytes) };
        self.data = core::ptr::null_mut();
    }
}

/// One Havok reference on a shape, removed on drop (`hkReferencedObject::RemoveReference`, which
/// deletes the shape with its last reference).
pub struct Shape(NonNull<hkpShape>);

impl Shape {
    /// Takes a reference on `shape` (`AddReference`).
    #[must_use]
    pub fn hold(shape: &hkpShape) -> Self {
        referenced(shape).add_reference();
        Self(NonNull::from(shape))
    }

    #[must_use]
    pub fn get(&self) -> &hkpShape {
        unsafe { self.0.as_ref() }
    }

    #[must_use]
    pub fn is(&self, shape: &hkpShape) -> bool {
        core::ptr::eq(self.0.as_ptr(), shape)
    }
}

impl Drop for Shape {
    fn drop(&mut self) {
        referenced(self.get()).remove_reference();
    }
}

fn referenced(shape: &hkpShape) -> &hkReferencedObject {
    unsafe { &*core::ptr::from_ref(shape).cast::<hkReferencedObject>() }
}

/// `hkpConvexVerticesShape::BuildConfig`, the hull build's options (id 80831; defaults from
/// `InitDefaultParams`, id 80830).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ConvexBuild {
    /// Keep the hull's connectivity.
    pub connectivity: bool,
    /// Shrink the hull by the convex radius (with a radius above 0).
    pub shrink_by_radius: bool,
    /// The shrinking's third switch.
    pub optimized_shrinking: bool,
    pub convex_radius: f32,
    /// Above 3, the most vertices the hull keeps.
    pub max_vertices: i32,
    pub max_relative_shrink: f32,
    pub max_shrink_displacement: f32,
    pub max_cos_bevel_angle: f32,
}

const _: () = assert!(size_of::<ConvexBuild>() == 0x18);

/// `hkStridedVertices`.
#[repr(C)]
struct StridedVertices {
    data: *const f32,
    count: i32,
    stride: i32,
}

/// The calling thread's Havok heap; `None` on a thread without a memory router.
fn heap() -> Option<*const c_void> {
    let index = crate::reloc().address(globals::g_hkMemoryRouterTlsIndex).ok()?;
    let router = unsafe { TlsGetValue(*(index as *const u32)) }.cast::<u8>().cast_const();
    if router.is_null() {
        return None;
    }
    let heap = read_ptr::<c_void>(unsafe { router.add(ROUTER_HEAP) });
    (!heap.is_null()).then_some(heap)
}

/// The sizes of the objects built here, which their deleting destructors free.
const CONVEX_SIZE: u16 = 0x90;
const LIST_SIZE: u16 = 0x90;
const _: () = assert!(size_of::<sre::hkpConvexVerticesShape>() == CONVEX_SIZE as usize && size_of::<hkpListShape>() == LIST_SIZE as usize);

/// A block of `size` bytes from the heap with its size in the memory word, as the engine
/// allocates every Havok object before its constructor runs.
fn alloc(heap: *const c_void, size: u16) -> Option<NonNull<c_void>> {
    type Alloc = unsafe extern "C" fn(*const c_void, i32) -> *mut c_void;
    let alloc: Alloc = unsafe { core::mem::transmute::<usize, Alloc>(read_ptr::<usize>(heap.cast()).add(1).read()) };
    let block = NonNull::new(unsafe { alloc(heap, i32::from(size)) })?;
    unsafe { block.as_ptr().byte_add(8).cast::<u16>().write_unaligned(size) };
    Some(block)
}

fn free(heap: *const c_void, block: NonNull<c_void>, size: u16) {
    type Free = unsafe extern "C" fn(*const c_void, *mut c_void, i32);
    let free: Free = unsafe { core::mem::transmute::<usize, Free>(read_ptr::<usize>(heap.cast()).add(2).read()) };
    unsafe { free(heap, block.as_ptr(), i32::from(size)) };
}

/// A convex hull of `points` (x, y, z; w is ignored) built by `hkpConvexVerticesShape`'s hull
/// constructor (id 80831) on the Havok heap; `None` on a thread without one or when the build
/// fails.
#[must_use]
pub fn build_convex(points: &[[f32; 4]], config: &ConvexBuild) -> Option<Shape> {
    let heap = heap()?;
    let count = i32::try_from(points.len()).ok()?;
    let block = alloc(heap, CONVEX_SIZE)?;
    let strided = StridedVertices { data: points.as_ptr().cast(), count, stride: 16 };
    let shape = unsafe { block.cast::<sre::hkpConvexVerticesShape>().as_ref() };
    unsafe { shape.build_from_points((&raw const strided).cast(), core::ptr::from_ref(config).cast()) };
    let built = Shape(block.cast());
    if referenced(built.get()).referenceCount.cast_signed() <= 0 {
        core::mem::forget(built);
        free(heap, block, CONVEX_SIZE);
        return None;
    }
    Some(built)
}

/// A new list of `children` with `filter_infos`, the enable mask and the disabled count of `like`.
fn rebuild_list(like: &hkpListShape, children: &[*const hkpShape], filter_infos: impl Iterator<Item = u32>) -> Option<Shape> {
    let count = i32::try_from(children.len()).ok()?;
    let block = alloc(heap()?, LIST_SIZE)?;
    let list = block.cast::<hkpListShape>().as_ptr();
    let mut shapes = children.to_vec();
    unsafe { (*list).ctor(shapes.as_mut_ptr(), count, 1) };
    let built = Shape(block.cast());
    unsafe {
        let entries = (*list).childInfoData.cast::<ListChild>();
        for (i, filter_info) in filter_infos.take((*list).childInfoSize as usize).enumerate() {
            (*entries.add(i)).filter_info = filter_info;
        }
        (*list).enabledChildren = like.enabledChildren;
        (*list).numDisabledChildren = like.numDisabledChildren;
    }
    Some(built)
}

/// Swaps the controller's root `old` for `new` through `ReplaceShape` (id 78308), which removes
/// the world object, sets the shape, adds it back and moves the wrapper's count: the root's wrapper
/// is pointed at `new` (slot 37), and `old` keeps it in its user data for the call, which reads it
/// there without a null test. With the world locked.
fn swap_root(controller: &bhkCharacterController, old: &hkpShape, new: &hkpShape) -> bool {
    let Some(wrapper) = old.wrapper() else { return false };
    wrapper.set_referenced_object(referenced(new));
    crate::store(&old.userData, core::ptr::from_ref(wrapper).cast_mut());
    let changed = controller.replace_shape(wrapper);
    crate::store(&old.userData, core::ptr::null_mut());
    if !changed {
        wrapper.set_referenced_object(referenced(old));
    }
    changed
}

/// A controller shape a mod swapped in, with what it replaced, so it can go back.
pub struct Replacement {
    controller: NonNull<bhkCharacterController>,
    root: Shape,
    installed: Shape,
    /// The list child replaced, by index, when the root is a list.
    child: Option<(usize, Shape)>,
}

/// Puts `new` in place of `target`, the controller's root shape or a child of its root list (the
/// engine writes a list's children only when it builds one, so a new list with the new child
/// replaces the root). `new` takes over `target`'s wrapper. `None` when `target` is neither, has
/// no wrapper or the swap was refused.
pub fn replace(controller: &bhkCharacterController, target: &hkpShape, new: Shape) -> Option<Replacement> {
    let root = root_shape(controller)?;
    let target_wrapper = target.wrapper()?;
    let _lock = lock_world(controller)?;
    if core::ptr::eq(root, target) {
        let held_root = Shape::hold(root);
        return swap_root(controller, root, new.get()).then(|| Replacement { controller: NonNull::from(controller), root: held_root, installed: new, child: None });
    }
    let list = as_list(root)?;
    let index = root.children().iter().position(|c| core::ptr::eq(c.shape, target))?;
    let shapes: Vec<_> = root.children().iter().enumerate().map(|(i, c)| if i == index { core::ptr::from_ref(new.get()) } else { c.shape }).collect();
    let rebuilt = rebuild_list(list, &shapes, root.children().iter().map(|c| c.filter_info))?;
    let (held_root, held_target) = (Shape::hold(root), Shape::hold(target));
    target_wrapper.set_referenced_object(referenced(new.get()));
    if !swap_root(controller, root, rebuilt.get()) {
        target_wrapper.set_referenced_object(referenced(target));
        return None;
    }
    Some(Replacement { controller: NonNull::from(controller), root: held_root, installed: rebuilt, child: Some((index, held_target)) })
}

impl Replacement {
    /// The shape this replacement put in: the new root, or the new child.
    #[must_use]
    pub fn installed(&self) -> &hkpShape {
        match self.child {
            Some((index, _)) => self.installed.get().children().get(index).and_then(|c| unsafe { c.shape.as_ref() }).unwrap_or(self.installed.get()),
            None => self.installed.get(),
        }
    }

    /// Whether `controller` is the one swapped and still collides with the replacement (a load or
    /// the engine's own shape switch replaces it).
    #[must_use]
    pub fn is_current(&self, controller: &bhkCharacterController) -> bool {
        core::ptr::eq(self.controller.as_ptr(), controller) && root_shape(controller).is_some_and(|r| self.installed.is(r)) && self.installed.get().wrapper().is_some()
    }

    /// Puts the original shape back when the replacement is still current; dropped otherwise.
    pub fn restore(self, controller: &bhkCharacterController) -> bool {
        if !self.is_current(controller) {
            return false;
        }
        let Some(_lock) = lock_world(controller) else { return false };
        if let Some((_, original)) = &self.child
            && let Some(wrapper) = self.installed().wrapper()
        {
            wrapper.set_referenced_object(referenced(original.get()));
        }
        swap_root(controller, self.installed.get(), self.root.get())
    }
}

/// Writes `capsule` into a capsule shape the controller collides with, as the engine's own
/// bumper builder edits one in place (end points with w = radius, then the radius), and
/// recomputes the root list's cached bounds. With the world locked.
pub fn set_capsule(controller: &bhkCharacterController, shape: &hkpShape, capsule: Capsule) -> bool {
    if shape.shape_type() != Some(hkpShapeType::Capsule) {
        return false;
    }
    let Some(_lock) = lock_world(controller) else { return false };
    let c = core::ptr::from_ref(shape).cast::<sre::hkpCapsuleShape>().cast_mut();
    let [ax, ay, az] = capsule.a;
    let [bx, by, bz] = capsule.b;
    unsafe {
        ((*c).vertexA.x, (*c).vertexA.y, (*c).vertexA.z, (*c).vertexA.w) = (ax, ay, az, capsule.radius);
        ((*c).vertexB.x, (*c).vertexB.y, (*c).vertexB.z, (*c).vertexB.w) = (bx, by, bz, capsule.radius);
        (*c).radius = capsule.radius;
    }
    if let Some(list) = root_shape(controller).and_then(as_list) {
        list.recompute_bounds();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{ConvexBuild, HkArray, ListChild, StridedVertices};

    #[test]
    fn the_engine_records_have_their_sizes() {
        assert_eq!(size_of::<ListChild>(), 0x20);
        assert_eq!(size_of::<ConvexBuild>(), 0x18);
        assert_eq!(core::mem::offset_of!(ConvexBuild, convex_radius), 4);
        assert_eq!(core::mem::offset_of!(ConvexBuild, max_vertices), 8);
        assert_eq!(core::mem::offset_of!(ConvexBuild, max_cos_bevel_angle), 0x14);
        assert_eq!(size_of::<StridedVertices>(), 0x10);
        assert_eq!(size_of::<HkArray>(), 0x10);
    }
}
