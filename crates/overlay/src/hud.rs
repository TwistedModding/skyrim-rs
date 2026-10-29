//! The HUD layer: shapes a mod draws into the game every frame, open or closed, under the
//! overlay's windows, placed in the world through the frame camera.

use egui::{Pos2, Rect, pos2};
use skyrim::render::NiAVObjectExt;
use sre::{NiAVObject, NiCamera, NiPoint3};

/// Points closer to the eye plane than this are not projected (`WorldPtToScreenPt`'s tolerance on `w`).
const ZERO_TOLERANCE: f32 = 1e-5;

/// The frame camera as the HUD layer sees it at the present: the camera the frame was drawn with.
pub struct View {
    camera: &'static NiCamera,
    screen: Rect,
}

impl View {
    /// The frame camera (`g_SceneGraph` +0x128), `None` until the scene graph exists.
    pub(crate) fn current(screen: Rect) -> Option<Self> {
        sre::g_scene_graph()?;
        Some(Self { camera: sre::get_main_camera()?, screen })
    }

    /// The screen in points, as the painter's coordinates use them.
    #[must_use]
    pub fn screen(&self) -> Rect {
        self.screen
    }

    /// The camera's world position.
    #[must_use]
    pub fn eye(&self) -> [f32; 3] {
        self.camera.world_transform().translation
    }

    /// Where the world point `p` lands, in points, through `NiCamera::WorldPtToScreenPt`; a point
    /// beside the view lands outside [`screen`](Self::screen). `None` behind the camera, nearer
    /// than the near plane or past the far plane.
    #[must_use]
    pub fn project(&self, p: [f32; 3]) -> Option<Pos2> {
        let point = NiPoint3 { x: p[0], y: p[1], z: p[2], ..NiPoint3::default() };
        let (mut x, mut y, mut depth) = (0.0, 0.0, 0.0);
        let ok = unsafe { self.camera.world_pt_to_screen_pt(&raw const point, &raw mut x, &raw mut y, &raw mut depth, ZERO_TOLERANCE) };
        (ok && (0.0..=1.0).contains(&depth)).then(|| pos2(self.screen.min.x + x * self.screen.width(), self.screen.min.y + (1.0 - y) * self.screen.height()))
    }

    /// The square on screen that a sphere covers, measured along the camera's up and right axes.
    #[must_use]
    pub fn project_sphere(&self, center: [f32; 3], radius: f32) -> Option<Rect> {
        let rotation = self.camera.world_transform().rotation;
        let axis = |column: usize, sign: f32| core::array::from_fn(|r| center[r] + sign * radius * rotation[r][column]);
        let [up, down, right, left] = [axis(1, 1.0), axis(1, -1.0), axis(2, 1.0), axis(2, -1.0)].map(|p| self.project(p));
        let rect = Rect::from_points(&[up?, down?, right?, left?]);
        Some(Rect::from_center_size(rect.center(), egui::Vec2::splat(rect.width().max(rect.height()))))
    }

    /// The square on screen that an object's world bound covers: an actor's 3D root
    /// (`loaded_data()?.node3_d()`) gives the box around the actor.
    #[must_use]
    pub fn project_bound(&self, object: &NiAVObject) -> Option<Rect> {
        let bound = &object.worldBound;
        if bound.radius <= 0.0 {
            return None;
        }
        self.project_sphere([bound.center.x, bound.center.y, bound.center.z], bound.radius)
    }
}
