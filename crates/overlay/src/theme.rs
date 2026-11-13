// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The framework theme, taken from the game's own menus (the journal, the system page, the
//! inventory and the message box: their SWF files' text and fill colours, read at tag level):
//! translucent black panels framed by thin grey-white rules, white text dimmed for what is not
//! selected, a faint white highlight, square corners, generous spacing, and Futura Condensed when
//! the game's font file could be read (`crate::font_source`). Hover text shows as soon as the
//! pointer is over its widget, at rest or not, on an opaque fill like every list and menu that
//! opens over a window; a window takes the translucent one with [`window`]. The look is the
//! loader's: [`Values`] holds it as plain data, which the loader hands to each framework plugin for
//! the plugin's own `egui` contexts. A window may change any of it through
//! `egui`'s own style API (`ui.style_mut()`, `egui::Window::frame`).

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Shadow, Stroke, Style, TextStyle, Theme, Ui, WidgetText};

/// The menus' text colour.
pub const TEXT: Color32 = Color32::WHITE;
/// Text that is not selected or is secondary (`#aaaaaa` in the journal).
pub const DIM: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);
/// Disabled text: white at 30 % (`#ffffff` alpha 76).
pub const DISABLED: Color32 = Color32::from_rgba_premultiplied(76, 76, 76, 76);
/// The panels' fill: black at 60 % (`#000000` alpha 153), a little darker to stay readable over the world.
pub const PANEL: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 180);
/// The fill of hover text, drop-down lists and menus: opaque, so what is under them does not show through their text.
pub const POPUP: Color32 = Color32::from_gray(14);
/// The rules framing panels and lists (`#bbbdbf`, a little more opaque than the files' 60 % to read as the journal's frame does).
pub const RULE: Color32 = Color32::from_rgba_premultiplied(147, 148, 150, 200);
/// The selection and hover highlight: white at 20 % (`#ffffff` alpha 51).
pub const HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(51, 51, 51, 51);
/// The family window titles and headings use.
pub const HEADING: &str = "heading";

/// A dim `(?)` that shows `text` while the pointer is over it: the help of the control beside it.
pub fn help(ui: &mut Ui, text: impl Into<WidgetText>) -> Response {
    ui.label(RichText::new("(?)").color(DIM)).on_hover_text(text)
}

/// A window's frame on the theme's translucent panel fill; `egui`'s own has the fill of hover text and lists.
pub fn window(style: &Style) -> Frame {
    Frame::window(style).fill(style.visuals.panel_fill)
}

/// A stroke: its width in points and its colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub width: f32,
    pub color: [u8; 4],
}

/// How a widget is drawn in one of its states.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widget {
    pub fill: [u8; 4],
    pub weak_fill: [u8; 4],
    pub frame: Line,
    pub text: Line,
    pub expansion: f32,
}

/// Every value of the theme, as it crosses the plugin ABI: sizes in points, colours premultiplied
/// RGBA. Fields are only ever appended, and none leaves padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    pub heading: f32,
    pub body: f32,
    pub button: f32,
    pub small: f32,
    pub monospace: f32,
    pub item_spacing: [f32; 2],
    pub button_padding: [f32; 2],
    pub interact_size: [f32; 2],
    pub window_margin: f32,
    pub menu_margin: f32,
    pub indent: f32,
    pub icon_width: f32,
    pub icon_width_inner: f32,
    pub icon_spacing: f32,
    pub corner_radius: f32,
    /// Seconds the pointer is over a widget before its hover text shows.
    pub hover_delay: f32,
    /// The fill of hover text, drop-down lists and menus.
    pub popup_fill: [u8; 4],
    /// The fill of windows and panels.
    pub panel_fill: [u8; 4],
    /// The fill of text fields and scroll bars.
    pub extreme_fill: [u8; 4],
    /// The fill of a table's striped rows.
    pub faint_fill: [u8; 4],
    pub code_fill: [u8; 4],
    pub hyperlink: [u8; 4],
    pub selection_fill: [u8; 4],
    pub selection: Line,
    /// The rule around windows, hover text, lists and menus.
    pub rule: Line,
    pub noninteractive: Widget,
    pub inactive: Widget,
    pub hovered: Widget,
    pub active: Widget,
    pub open: Widget,
    /// Hover text waits for the pointer to rest.
    pub hover_at_rest: bool,
    pub header_frame: bool,
    pub slider_trailing_fill: bool,
    pub indent_line: bool,
}

impl Default for Values {
    fn default() -> Self {
        let line = |width, color: Color32| Line { width, color: color.to_array() };
        let clear = Color32::TRANSPARENT.to_array();
        let hovered = Widget { fill: HIGHLIGHT.to_array(), weak_fill: Color32::from_white_alpha(26).to_array(), frame: line(1.0, TEXT), text: line(1.5, TEXT), expansion: 0.0 };
        Self {
            heading: 26.0,
            body: 22.0,
            button: 22.0,
            small: 17.0,
            monospace: 16.0,
            item_spacing: [10.0, 6.0],
            button_padding: [12.0, 4.0],
            interact_size: [40.0, 26.0],
            window_margin: 18.0,
            menu_margin: 10.0,
            indent: 20.0,
            icon_width: 16.0,
            icon_width_inner: 10.0,
            icon_spacing: 8.0,
            corner_radius: 0.0,
            hover_delay: 0.0,
            popup_fill: POPUP.to_array(),
            panel_fill: PANEL.to_array(),
            extreme_fill: Color32::from_black_alpha(200).to_array(),
            faint_fill: Color32::from_white_alpha(10).to_array(),
            code_fill: Color32::from_black_alpha(120).to_array(),
            hyperlink: TEXT.to_array(),
            selection_fill: HIGHLIGHT.to_array(),
            selection: line(1.0, TEXT),
            rule: line(1.0, RULE),
            noninteractive: Widget { fill: clear, weak_fill: clear, frame: line(1.0, RULE), text: line(1.0, Color32::from_rgb(0xDD, 0xDD, 0xDD)), expansion: 0.0 },
            inactive: Widget { fill: Color32::from_white_alpha(40).to_array(), weak_fill: clear, frame: line(1.0, RULE), text: line(1.0, DIM), expansion: 0.0 },
            hovered,
            active: Widget { fill: Color32::from_white_alpha(70).to_array(), weak_fill: HIGHLIGHT.to_array(), frame: line(1.0, TEXT), text: line(2.0, TEXT), expansion: 0.0 },
            open: hovered,
            hover_at_rest: false,
            header_frame: false,
            slider_trailing_fill: true,
            indent_line: true,
        }
    }
}

fn color([r, g, b, a]: [u8; 4]) -> Color32 {
    Color32::from_rgba_premultiplied(r, g, b, a)
}

fn stroke(line: Line) -> Stroke {
    Stroke::new(line.width, color(line.color))
}

/// Applies the theme `t` to every style of `ctx`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn apply(ctx: &Context, t: &Values) {
    let radius = CornerRadius::same(t.corner_radius as u8);
    ctx.set_theme(Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(t.heading, FontFamily::Name(HEADING.into()))),
            (TextStyle::Body, FontId::new(t.body, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(t.button, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(t.small, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(t.monospace, FontFamily::Monospace)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = t.item_spacing.into();
        s.button_padding = t.button_padding.into();
        s.window_margin = Margin::same(t.window_margin as i8);
        s.menu_margin = Margin::same(t.menu_margin as i8);
        s.indent = t.indent;
        s.interact_size = t.interact_size.into();
        s.icon_width = t.icon_width;
        s.icon_width_inner = t.icon_width_inner;
        s.icon_spacing = t.icon_spacing;
        style.interaction.tooltip_delay = t.hover_delay;
        style.interaction.show_tooltips_only_when_still = t.hover_at_rest;
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.window_fill = color(t.popup_fill);
        v.panel_fill = color(t.panel_fill);
        v.window_stroke = stroke(t.rule);
        v.window_corner_radius = radius;
        v.menu_corner_radius = radius;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.extreme_bg_color = color(t.extreme_fill);
        v.faint_bg_color = color(t.faint_fill);
        v.code_bg_color = color(t.code_fill);
        v.hyperlink_color = color(t.hyperlink);
        v.selection.bg_fill = color(t.selection_fill);
        v.selection.stroke = stroke(t.selection);
        v.collapsing_header_frame = t.header_frame;
        v.slider_trailing_fill = t.slider_trailing_fill;
        v.indent_has_left_vline = t.indent_line;
        let w = &mut v.widgets;
        for (visuals, state) in [(&mut w.noninteractive, t.noninteractive), (&mut w.inactive, t.inactive), (&mut w.hovered, t.hovered), (&mut w.active, t.active), (&mut w.open, t.open)] {
            visuals.bg_fill = color(state.fill);
            visuals.weak_bg_fill = color(state.weak_fill);
            visuals.bg_stroke = stroke(state.frame);
            visuals.fg_stroke = stroke(state.text);
            visuals.expansion = state.expansion;
            visuals.corner_radius = radius;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_text_is_opaque_and_a_window_is_not() {
        let ctx = Context::default();
        apply(&ctx, &Values::default());
        let style = ctx.global_style();
        assert!(Frame::popup(&style).fill.is_opaque());
        assert_eq!(window(&style).fill, PANEL);
    }

    #[test]
    fn the_values_have_no_padding() {
        let widget = 2 * 4 + 2 * 8 + 4;
        assert_eq!((size_of::<Line>(), size_of::<Widget>(), size_of::<Values>()), (8, widget, 19 * 4 + 7 * 4 + 2 * 8 + 5 * widget + 4));
    }
}
