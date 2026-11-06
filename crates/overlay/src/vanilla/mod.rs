// Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
// distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
// This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
//! The game's own menu font and cursor, read at startup from its interface files (loose under
//! `Data\Interface`, or in `Skyrim - Interface.bsa`): Futura Condensed Light, the font
//! `fontconfig.txt` maps to `$EverywhereFont`, and Futura Condensed for headings, from
//! `fonts_en.swf`, converted to TrueType for `egui`; the mouse cursor's shape from `cursormenu.swf`,
//! rasterized. Nothing of it is shipped; a missing or unreadable file leaves the system fonts and
//! the overlay's own arrow in place.

use std::path::Path;

mod bsa;
mod inflate;
mod swf;
mod ttf;

pub(crate) const BODY: &str = "Futura CondensedLight";
pub(crate) const HEADING: &str = "Futura Condensed";
/// The cursor's size in points per twip of its shape, measured against the game's cursor in the journal at 1080 lines.
const CURSOR_POINTS_PER_TWIP: f32 = 0.054;
/// Pixels per point of the cursor's image, so it stays sharp up to 2160 lines.
const CURSOR_DENSITY: f32 = 2.0;
/// Samples per pixel side when the cursor is rasterized.
const SAMPLES: usize = 4;

/// The body and heading fonts as TrueType files, and where the SWF came from.
pub(crate) struct Fonts {
    pub body: Vec<u8>,
    pub heading: Vec<u8>,
    pub source: String,
}

/// The game's cursor as an image, its top left corner the point it points at.
pub(crate) struct Cursor {
    pub image: egui::ColorImage,
    /// The image's size in points.
    pub size: egui::Vec2,
    pub source: String,
}

/// An interface file from the game's `Data` folder and where it came from.
fn interface_file(data: &Path, name: &str) -> Result<(Vec<u8>, String), String> {
    let loose = data.join("Interface").join(name);
    if let Ok(bytes) = std::fs::read(&loose) {
        return Ok((bytes, loose.display().to_string()));
    }
    let archive = data.join("Skyrim - Interface.bsa");
    Ok((bsa::read(&archive, "interface", name)?, format!("{}\\interface\\{name}", archive.display())))
}

/// Reads and converts the fonts from the game's `Data` folder.
pub(crate) fn load(data: &Path) -> Result<Fonts, String> {
    let (swf, source) = interface_file(data, "fonts_en.swf")?;
    let convert = |name: &str| swf::font(&swf, name).map(|f| ttf::build(&f));
    Ok(Fonts { body: convert(BODY)?, heading: convert(HEADING)?, source })
}

/// Reads and rasterizes the cursor from the game's `Data` folder.
pub(crate) fn cursor(data: &Path) -> Result<Cursor, String> {
    let (swf, source) = interface_file(data, "cursormenu.swf")?;
    let image = rasterize(&swf::first_shape(&swf)?, CURSOR_POINTS_PER_TWIP * CURSOR_DENSITY);
    #[allow(clippy::cast_precision_loss)]
    let size = egui::vec2(image.size[0] as f32, image.size[1] as f32) / CURSOR_DENSITY;
    Ok(Cursor { image, size, source })
}

/// Paints the shape's fills in order, each where an even number of its boundary edges lies to the
/// left (edges with the fill on one side only), `scale` pixels per twip from the shape's origin.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rasterize(shape: &swf::Shape, scale: f32) -> egui::ColorImage {
    let extent = |i: usize| shape.edges.iter().flat_map(|e| [e.from[i], e.to[i]]).fold(0.0f32, f32::max);
    let (w, h) = ((extent(0) * scale).ceil() as usize + 1, (extent(1) * scale).ceil() as usize + 1);
    let mut pixels = vec![[0.0f32; 4]; w * h];
    for (i, fill) in shape.fills.iter().enumerate() {
        let style = i + 1;
        let edges: Vec<([f32; 2], [f32; 2])> = shape.edges.iter().filter(|e| (e.fill0 == style) != (e.fill1 == style)).map(|e| (e.from.map(|v| v * scale), e.to.map(|v| v * scale))).collect();
        let color = fill.map(|c| f32::from(c) / 255.0);
        for (at, pixel) in pixels.iter_mut().enumerate() {
            let mut inside = 0;
            for s in 0..SAMPLES * SAMPLES {
                let x = (at % w) as f32 + ((s % SAMPLES) as f32 + 0.5) / SAMPLES as f32;
                let y = (at / w) as f32 + ((s / SAMPLES) as f32 + 0.5) / SAMPLES as f32;
                let crossings = edges.iter().filter(|(a, b)| (a[1] > y) != (b[1] > y) && x > a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1])).count();
                inside += crossings % 2;
            }
            let cover = inside as f32 / (SAMPLES * SAMPLES) as f32 * color[3];
            for c in 0..3 {
                pixel[c] = color[c] * cover + pixel[c] * (1.0 - cover);
            }
            pixel[3] = cover + pixel[3] * (1.0 - cover);
        }
    }
    let bytes: Vec<u8> = pixels.iter().flat_map(|p| p.map(|v| (v * 255.0).round() as u8)).collect();
    egui::ColorImage::from_rgba_premultiplied([w, h], &bytes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn data_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("SKYRIM_DATA_DIR").unwrap_or_else(|| r"C:\Program Files (x86)\Steam\steamapps\common\Skyrim Special Edition\Data".into()));
        dir.is_dir().then_some(dir)
    }

    #[test]
    fn converts_the_vanilla_font() {
        let Some(data) = data_dir() else {
            eprintln!("skipped: no Skyrim Data folder (set SKYRIM_DATA_DIR)");
            return;
        };
        let fonts = load(&data).unwrap();
        let ctx = egui::Context::default();
        let mut defs = egui::FontDefinitions::empty();
        defs.font_data.insert("body".into(), egui::FontData::from_owned(fonts.body).into());
        defs.families.insert(egui::FontFamily::Proportional, vec!["body".into()]);
        ctx.set_fonts(defs);
        let mut galley = None;
        let mut output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            galley = Some(ctx.fonts_mut(|f| f.layout_no_wrap("Skyrim MW".into(), egui::FontId::proportional(20.0), egui::Color32::WHITE)));
        });
        output.textures_delta.clear();
        let galley = galley.unwrap();
        let width = galley.size().x;
        assert!(width > 40.0 && width < 120.0, "width {width}");
        let glyphs = &galley.rows[0].row.glyphs;
        assert_eq!(glyphs.len(), 9);
        assert!(glyphs.iter().filter(|g| g.chr != ' ').all(|g| g.uv_rect.size.x > 0.0 && g.uv_rect.size.y > 0.0), "a glyph rasterized empty");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn rasterizes_the_vanilla_cursor() {
        let Some(data) = data_dir() else {
            eprintln!("skipped: no Skyrim Data folder (set SKYRIM_DATA_DIR)");
            return;
        };
        let cursor = cursor(&data).unwrap();
        assert!(cursor.size.x > 40.0 && cursor.size.x < 50.0 && cursor.size.y > 40.0 && cursor.size.y < 50.0, "size {:?}", cursor.size);
        let [w, _] = cursor.image.size;
        let at = |x: f32, y: f32| cursor.image.pixels[(y * CURSOR_DENSITY) as usize * w + (x * CURSOR_DENSITY) as usize];
        assert_eq!(at(40.0, 4.0).a(), 0, "outside the arrow");
        assert!(at(4.0, 2.5).a() > 150, "on its black edge: {:?}", at(4.0, 2.5));
        assert!(at(16.2, 12.4).r() > 60, "on its grey inner arrow: {:?}", at(16.2, 12.4));
        assert_eq!(at(20.0, 20.0).r(), 0, "on the black stroke inside it: {:?}", at(20.0, 20.0));
    }
}
