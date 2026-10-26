//! The game's own menu font, read at startup from `Interface\fonts_en.swf` (loose under `Data`, or
//! in `Skyrim - Interface.bsa`) and converted to TrueType for `egui`: Futura Condensed Light, the
//! font `fontconfig.txt` maps to `$EverywhereFont`, and Futura Condensed for headings. Nothing of
//! it is shipped; a missing or unreadable file leaves the system fonts in place.

use std::path::Path;

mod bsa;
mod inflate;
mod swf;
mod ttf;

pub(crate) const BODY: &str = "Futura CondensedLight";
pub(crate) const HEADING: &str = "Futura Condensed";

/// The body and heading fonts as TrueType files, and where the SWF came from.
pub(crate) struct Fonts {
    pub body: Vec<u8>,
    pub heading: Vec<u8>,
    pub source: String,
}

/// Reads and converts the fonts from the game's `Data` folder.
pub(crate) fn load(data: &Path) -> Result<Fonts, String> {
    let loose = data.join("Interface").join("fonts_en.swf");
    let (swf, source) = if let Ok(bytes) = std::fs::read(&loose) {
        (bytes, loose.display().to_string())
    } else {
        let archive = data.join("Skyrim - Interface.bsa");
        (bsa::read(&archive, "interface", "fonts_en.swf")?, format!("{}\\interface\\fonts_en.swf", archive.display()))
    };
    let convert = |name: &str| swf::font(&swf, name).map(|f| ttf::build(&f));
    Ok(Fonts { body: convert(BODY)?, heading: convert(HEADING)?, source })
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
}
