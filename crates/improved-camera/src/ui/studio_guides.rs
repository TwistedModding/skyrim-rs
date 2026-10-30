//! `ui/menu/StudioGuides`: the composition guides over the free camera while the photo mode's menu
//! is up (grid, aspect mask, centre mark, title-safe box, level indicator), and the target's bone
//! markers on the Bones page, painted through the host's HUD painter: under the menu and, since
//! the capture reads the frame before the overlay draws, never in a shot.

use framework::egui::{self, Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};
use framework::View;

use crate::camera::studio::capture::aspect_ratio;
use crate::camera::studio::manager::{self, Manager};
use crate::settings;

const LINE: Color32 = Color32::from_rgba_premultiplied(90, 90, 90, 90);
const SHADOW: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 90);
const ACCENT: Color32 = Color32::from_rgba_premultiplied(170, 143, 54, 170);
const MASK: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 160);

pub const GRID_NAMES: [&str; 4] = ["Off", "Rule of Thirds", "Golden Ratio", "Quarters"];

/// `CropRect`: the centred rectangle of the guide's aspect.
#[must_use]
pub fn crop_rect(screen: Rect, aspect: i32) -> Rect {
    let ratio = aspect_ratio(aspect);
    if ratio <= 0.0 || screen.width() <= 0.0 || screen.height() <= 0.0 {
        return screen;
    }
    let size = if screen.width() / screen.height() > ratio { vec2(screen.height() * ratio, screen.height()) } else { vec2(screen.width(), screen.width() / ratio) };
    Rect::from_center_size(screen.center(), size)
}

/// The fractions of the frame a grid divides on.
#[must_use]
pub fn grid_divisions(grid: i32) -> &'static [f32] {
    match grid {
        1 => &[1.0 / 3.0, 2.0 / 3.0],
        2 => &[0.382, 0.618],
        3 => &[0.25, 0.5, 0.75],
        _ => &[],
    }
}

/// `GuideLine`: a dark backing stroke under a light one, readable over snow and in a dungeon.
fn line(painter: &Painter, from: Pos2, to: Pos2, color: Color32) {
    let offset = vec2(1.0, 1.0);
    painter.line_segment([from + offset, to + offset], Stroke::new(1.0, SHADOW));
    painter.line_segment([from, to], Stroke::new(1.0, color));
}

/// `Draw`.
fn guides(painter: &Painter, screen: Rect) {
    let studio = settings::profile().Studio;
    let cropped = studio.iGuideAspect != 0;
    let rect = crop_rect(screen, studio.iGuideAspect);
    if cropped {
        for band in [Rect::from_min_max(screen.min, pos2(screen.max.x, rect.min.y)), Rect::from_min_max(pos2(screen.min.x, rect.max.y), screen.max), Rect::from_min_max(screen.min, pos2(rect.min.x, screen.max.y)), Rect::from_min_max(pos2(rect.max.x, screen.min.y), screen.max)] {
            if band.width() > 0.0 && band.height() > 0.0 {
                painter.rect_filled(band, 0.0, MASK);
            }
        }
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    }
    for f in grid_divisions(studio.iGuideGrid) {
        let (x, y) = (rect.min.x + rect.width() * f, rect.min.y + rect.height() * f);
        line(painter, pos2(x, rect.min.y), pos2(x, rect.max.y), LINE);
        line(painter, pos2(rect.min.x, y), pos2(rect.max.x, y), LINE);
    }
    if studio.bGuideSafeArea {
        let safe = rect.shrink2(vec2(rect.width() * 0.05, rect.height() * 0.05));
        painter.rect_stroke(safe.translate(vec2(1.0, 1.0)), 0.0, Stroke::new(1.0, SHADOW), egui::StrokeKind::Inside);
        painter.rect_stroke(safe, 0.0, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    }
    let centre = rect.center();
    if studio.bGuideCenter {
        let arm = 9.0;
        line(painter, centre - vec2(arm, 0.0), centre + vec2(arm, 0.0), LINE);
        line(painter, centre - vec2(0.0, arm), centre + vec2(0.0, arm), LINE);
    }
    if studio.bGuideHorizon {
        let roll = Manager::read_camera_pose().roll;
        let length = rect.width() * 0.22;
        let (sin, cos) = (-roll.to_radians()).sin_cos();
        let level = roll.abs() < 0.05;
        let color = if level { ACCENT } else { LINE };
        line(painter, centre - vec2(length * cos, length * sin), centre + vec2(length * cos, length * sin), color);
        painter.circle_filled(centre, 2.0, color);
        if !level {
            let at = pos2(centre.x, rect.max.y - 28.0);
            let font = egui::FontId::proportional(16.0);
            painter.text(at + vec2(1.0, 1.0), egui::Align2::CENTER_CENTER, format!("{roll:+.1}°"), font.clone(), SHADOW);
            painter.text(at, egui::Align2::CENTER_CENTER, format!("{roll:+.1}°"), font, ACCENT);
        }
    }
}

/// The HUD painter: guides and bone markers while the studio is open and its menu is up.
pub fn paint(painter: &Painter, view: &View) {
    super::studio_menu::markers(Vec::clear);
    if !manager::with(|m| m.active) || !super::studio_menu::input_captured() {
        return;
    }
    guides(painter, view.screen());
    super::studio_menu::paint_markers(painter, view);
}

#[cfg(test)]
mod tests {
    use framework::egui::{Rect, pos2};

    use super::{crop_rect, grid_divisions};

    #[test]
    fn the_crop_is_centred_and_grids_divide_it() {
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1920.0, 1080.0));
        assert_eq!(crop_rect(screen, 0), screen);
        assert_eq!(crop_rect(screen, 4), Rect::from_min_max(pos2(420.0, 0.0), pos2(1500.0, 1080.0)));
        let tall = crop_rect(screen, 2);
        assert!((tall.width() - 1920.0).abs() < 1e-3 && (tall.height() - 1920.0 * 9.0 / 21.0).abs() < 1e-3);
        assert_eq!(grid_divisions(1).len(), 2);
        assert_eq!((grid_divisions(0), grid_divisions(7)), (&[][..], &[][..]));
    }
}
