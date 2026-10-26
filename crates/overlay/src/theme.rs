//! The framework theme, taken from the game's own menus (the journal, the system page, the
//! inventory and the message box: their SWF files' text and fill colours, read at tag level):
//! translucent black panels framed by thin grey-white rules, white text dimmed for what is not
//! selected, a faint white highlight, square corners, generous spacing, and Futura Condensed when
//! the game's font file could be read (`crate::font_source`). A window may change any of it through
//! `egui`'s own style API (`ui.style_mut()`, `egui::Window::frame`).

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, vec2};

/// The menus' text colour.
pub const TEXT: Color32 = Color32::WHITE;
/// Text that is not selected or is secondary (`#aaaaaa` in the journal).
pub const DIM: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);
/// Disabled text: white at 30 % (`#ffffff` alpha 76).
pub const DISABLED: Color32 = Color32::from_rgba_premultiplied(76, 76, 76, 76);
/// The panels' fill: black at 60 % (`#000000` alpha 153), a little darker to stay readable over the world.
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 180);
/// The rules framing panels and lists (`#bbbdbf`, a little more opaque than the files' 60 % to read as the journal's frame does).
pub const RULE: Color32 = Color32::from_rgba_premultiplied(147, 148, 150, 200);
/// The selection and hover highlight: white at 20 % (`#ffffff` alpha 51).
pub const HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(51, 51, 51, 51);
/// The family window titles and headings use.
pub const HEADING: &str = "heading";

/// Applies the theme to every style of `ctx`.
pub(crate) fn apply(ctx: &Context) {
    ctx.set_theme(Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(26.0, FontFamily::Name(HEADING.into()))),
            (TextStyle::Body, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(17.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(16.0, FontFamily::Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = vec2(10.0, 6.0);
        s.button_padding = vec2(12.0, 4.0);
        s.window_margin = Margin::same(18);
        s.menu_margin = Margin::same(10);
        s.indent = 20.0;
        s.interact_size = vec2(40.0, 26.0);
        s.icon_width = 16.0;
        s.icon_width_inner = 10.0;
        s.icon_spacing = 8.0;
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.window_fill = PANEL;
        v.panel_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, RULE);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = Color32::from_black_alpha(200);
        v.faint_bg_color = Color32::from_white_alpha(10);
        v.code_bg_color = Color32::from_black_alpha(120);
        v.hyperlink_color = TEXT;
        v.selection.bg_fill = HIGHLIGHT;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.collapsing_header_frame = false;
        v.slider_trailing_fill = true;
        v.indent_has_left_vline = true;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = Color32::TRANSPARENT;
        w.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
        w.noninteractive.bg_stroke = Stroke::new(1.0, RULE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xDD, 0xDD, 0xDD));
        w.inactive.bg_fill = Color32::from_white_alpha(40);
        w.inactive.weak_bg_fill = Color32::TRANSPARENT;
        w.inactive.bg_stroke = Stroke::new(1.0, RULE);
        w.inactive.fg_stroke = Stroke::new(1.0, DIM);
        w.hovered.bg_fill = HIGHLIGHT;
        w.hovered.weak_bg_fill = Color32::from_white_alpha(26);
        w.hovered.bg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        w.hovered.expansion = 0.0;
        w.active.bg_fill = Color32::from_white_alpha(70);
        w.active.weak_bg_fill = HIGHLIGHT;
        w.active.bg_stroke = Stroke::new(1.0, TEXT);
        w.active.fg_stroke = Stroke::new(2.0, TEXT);
        w.active.expansion = 0.0;
        w.open = w.hovered;
        for visuals in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            visuals.corner_radius = CornerRadius::ZERO;
        }
    });
}
