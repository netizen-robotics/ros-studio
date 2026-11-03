use eframe::egui;
use std::sync::{Arc, OnceLock};

pub const NOTICE: &str = concat!(
    "Noto Sans SC Regular\nCopyright 2014-2021 Adobe (http://www.adobe.com/). Noto is a trademark of Google Inc.\n\n",
    include_str!("../assets/fonts/OFL-NotoSansSC.txt"),
    "\n\nNoto Emoji\n\n",
    include_str!("../assets/fonts/OFL.txt")
);

pub const SEMIBOLD: &str = "SystemSemibold";
static DEFINITIONS: OnceLock<egui::FontDefinitions> = OnceLock::new();

/// Cached immutable font data shared by native UI and worker text layout.
/// Frontends initialize this during creation, before their first UI frame.
pub fn definitions() -> egui::FontDefinitions {
    DEFINITIONS.get_or_init(load_system_definitions).clone()
}

fn load_system_definitions() -> egui::FontDefinitions {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    with_system_fonts(&database)
}

fn with_system_fonts(database: &fontdb::Database) -> egui::FontDefinitions {
    let mut definitions = bundled_definitions();
    let (sans, mono): (&[&str], &[&str]) = if cfg!(target_os = "macos") {
        (
            &[
                ".AppleSystemUIFont",
                ".SF NS Text",
                ".SF NS",
                "SF Pro Text",
                "Helvetica Neue",
            ],
            &["SFMono-Regular", "SF Mono", "Menlo"],
        )
    } else if cfg!(target_os = "windows") {
        (
            &["Segoe UI", "Arial"],
            &["Consolas", "Cascadia Mono", "Courier New"],
        )
    } else {
        (
            &["Noto Sans", "DejaVu Sans", "Liberation Sans"],
            &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"],
        )
    };
    for (name, family, candidates, weight) in [
        (
            "system-sans",
            egui::FontFamily::Proportional,
            sans,
            fontdb::Weight::NORMAL,
        ),
        (
            "system-mono",
            egui::FontFamily::Monospace,
            mono,
            fontdb::Weight::NORMAL,
        ),
        (
            "system-semibold",
            egui::FontFamily::Name(SEMIBOLD.into()),
            sans,
            fontdb::Weight::SEMIBOLD,
        ),
    ] {
        if let Some(data) = resolve(database, candidates, weight) {
            definitions.font_data.insert(name.into(), Arc::new(data));
            definitions
                .families
                .entry(family)
                .or_default()
                .insert(0, name.into());
        }
    }
    definitions
}

fn resolve(
    database: &fontdb::Database,
    candidates: &[&str],
    weight: fontdb::Weight,
) -> Option<egui::FontData> {
    // Try in platform preference order. Never select a non-monospace generic face
    // for structured data, or a font egui cannot actually parse.
    for name in candidates {
        let id = database.query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight,
            ..Default::default()
        });
        if let Some(data) = id
            .and_then(|id| {
                database.with_face_data(id, |bytes, index| {
                    use ab_glyph::Font;
                    let font = ab_glyph::FontRef::try_from_slice_and_index(bytes, index).ok()?;
                    if "Aa09".chars().any(|c| font.glyph_id(c).0 == 0) {
                        return None;
                    }
                    let mut data = egui::FontData::from_owned(bytes.to_vec());
                    data.index = index;
                    Some(data)
                })
            })
            .flatten()
        {
            return Some(data);
        }
    }
    None
}

fn bundled_definitions() -> egui::FontDefinitions {
    let mut definitions = egui::FontDefinitions::default();
    definitions.font_data.insert(
        "noto-sans-sc".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSC-Regular.otf"
        ))),
    );
    definitions.font_data.insert(
        "noto-emoji".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoEmoji.ttf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fallbacks = definitions.families.entry(family).or_default();
        fallbacks.push("noto-sans-sc".into());
        fallbacks.push("noto-emoji".into());
    }
    definitions.families.insert(
        egui::FontFamily::Name(SEMIBOLD.into()),
        definitions.families[&egui::FontFamily::Proportional].clone(),
    );
    definitions
}

/// Custom/pure-render contexts may not have installed the suite fonts yet.
pub fn semibold_family(ctx: &egui::Context) -> egui::FontFamily {
    let family = egui::FontFamily::Name(SEMIBOLD.into());
    if ctx.fonts(|fonts| fonts.families().contains(&family)) {
        family
    } else {
        egui::FontFamily::Proportional
    }
}

pub fn configure(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_platform_fonts_keep_complete_bundled_fallbacks() {
        let definitions = with_system_fonts(&fontdb::Database::new());
        assert!(!definitions.font_data.contains_key("system-sans"));
        for family in [
            egui::FontFamily::Proportional,
            egui::FontFamily::Monospace,
            egui::FontFamily::Name(SEMIBOLD.into()),
        ] {
            let names = &definitions.families[&family];
            assert!(names.contains(&"noto-sans-sc".into()));
            assert!(names.contains(&"noto-emoji".into()));
            assert!(names.len() >= 3);
        }
    }

    #[test]
    fn ui_and_worker_share_the_same_cached_font_bytes() {
        let ui = definitions();
        let worker = std::thread::spawn(definitions).join().unwrap();
        assert_eq!(ui.families, worker.families);
        for (name, data) in &ui.font_data {
            assert!(Arc::ptr_eq(data, &worker.font_data[name]));
        }
        eprintln!(
            "Resolved platform font keys: {:?}",
            ui.font_data
                .keys()
                .filter(|name| name.starts_with("system-"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn chinese_fallback_is_static_regular_instead_of_thin_variable_default() {
        let bytes = include_bytes!("../assets/fonts/NotoSansSC-Regular.otf");
        assert_eq!(&bytes[..4], b"OTTO");
        let count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
        let tables = &bytes[12..12 + count * 16];
        assert!(!tables.chunks_exact(16).any(|table| &table[..4] == b"fvar"));
        assert!(tables.chunks_exact(16).any(|table| &table[..4] == b"CFF "));
        let os2 = tables
            .chunks_exact(16)
            .find(|table| &table[..4] == b"OS/2")
            .expect("OS/2 font weight metadata");
        let offset = u32::from_be_bytes(os2[8..12].try_into().unwrap()) as usize;
        assert_eq!(
            u16::from_be_bytes(bytes[offset + 4..offset + 6].try_into().unwrap()),
            400,
            "Use Regular weight rather than a variable font's Thin default"
        );
    }

    #[test]
    fn chinese_latin_and_emoji_have_real_raster_pixels_in_both_text_families() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            assert!(!ctx.fonts(|f| f.has_glyph(&egui::FontId::monospace(16.0), '🤖')));
        });
        configure(&ctx);
        let _ = ctx.run(Default::default(), |ctx| {
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let font_id = egui::FontId::new(16.0, family);
                ctx.fonts(|fonts| {
                    assert!(fonts.has_glyphs(&font_id, "工作空间 机器人 café 🤖"));
                    let replacement =
                        fonts.layout_no_wrap("�".into(), font_id.clone(), egui::Color32::WHITE);
                    for glyph in "工作空间机器人café🤖".chars() {
                        let galley = fonts.layout_no_wrap(
                            glyph.to_string(),
                            font_id.clone(),
                            egui::Color32::WHITE,
                        );
                        let uv = galley.rows[0].glyphs[0].uv_rect;
                        assert_ne!(uv, replacement.rows[0].glyphs[0].uv_rect, "{glyph}");
                        let image = fonts.image();
                        let rasterized = (uv.min[1]..uv.max[1]).any(|y| {
                            (uv.min[0]..uv.max[0]).any(|x| {
                                image.pixels[y as usize * image.size[0] + x as usize] > 0.0
                            })
                        });
                        assert!(rasterized, "Missing {glyph} outline pixels");
                    }
                });
            }
        });
    }
}
