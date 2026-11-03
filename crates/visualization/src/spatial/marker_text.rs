//! Bounded worker-only font layout. All outputs are immutable; no UI context,
//! filesystem, ROS calls or GPU work is performed here.
use super::{marker_geometry::Body, Point3};
use eframe::egui::{self, epaint::text::Fonts};
use std::{collections::BTreeSet, sync::Arc};
pub const MAX_TEXT_BYTES: usize = 2 * studio_protocol::marker::MAX_RETAINED_TEXT_BYTES;
pub const MAX_GLYPHS: usize = MAX_TEXT_BYTES;
pub const MAX_UNIQUE_GLYPHS: usize = 2048;
pub const ATLAS_SIDE: usize = 2048;
pub const MAX_ATLAS_BYTES: usize = ATLAS_SIDE * ATLAS_SIDE;
const FONT_SIZE: f32 = 32.;
#[derive(Debug)]
pub struct Atlas {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Box<[u8]>,
}
impl Atlas {
    pub fn dimensions(&self) -> [u32; 2] {
        self.size
    }
    pub fn bytes(&self) -> usize {
        self.pixels.len()
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glyph {
    pub center: [f64; 2],
    pub dimensions: [f64; 2],
    pub uv: [f32; 4],
}
#[derive(Clone, Debug)]
pub struct Layout {
    pub(crate) atlas: Arc<Atlas>,
    pub(crate) glyphs: Arc<[Glyph]>,
    pub(crate) position: Point3,
    pub(crate) rgba: [f32; 4],
    pub dimensions: [f64; 2],
    pub missing_glyphs: usize,
    bounds: Option<[[f64; 2]; 2]>,
}
impl Layout {
    /// Cached rendered glyph bounds in the camera-facing local plane.
    pub fn local_bounds(&self) -> Option<[[f64; 2]; 2]> {
        self.bounds
    }
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }
    pub fn atlas(&self) -> &Arc<Atlas> {
        &self.atlas
    }
}
fn check(cancelled: &mut impl FnMut() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Marker text preparation cancelled".into())
    } else {
        Ok(())
    }
}
/// Prepare a combined two-owner set, so every layout shares one atlas identity.
/// Resource exhaustion is an aggregate error and publishes no partially reused
/// atlas. Callers retain per-key/source authorization outside this pure stage.
pub fn prepare(
    bodies: &[&Body],
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<Layout>, String> {
    if bodies.len() > 2 * studio_protocol::marker::MAX_MARKERS {
        return Err("Marker text entry limit".into());
    }
    let mut bytes = 0usize;
    let mut unique = BTreeSet::new();
    for body in bodies {
        check(&mut cancelled)?;
        let Body::Text {
            position,
            text,
            height,
            space_width,
            rgba,
        } = body
        else {
            return Err("Expected prepared Marker text".into());
        };
        bytes = bytes
            .checked_add(text.len())
            .ok_or("Marker text size overflow")?;
        if text.len() > studio_protocol::marker::MAX_TEXT_BYTES
            || bytes > MAX_TEXT_BYTES
            || !super::map_geometry::bounded_point(*position)
            || !height.is_finite()
            || *height <= 0.
            || *height > 1_000_000.
            || space_width.is_some_and(|v| !v.is_finite() || v <= 0. || v > 1_000_000.)
            || rgba
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Invalid Marker text size/style/bounds".into());
        }
        for c in text.chars() {
            check(&mut cancelled)?;
            if c == '\n' {
                continue;
            }
            if c.is_control() {
                return Err("Marker text contains an unsupported control character".into());
            }
            unique.insert(c);
            if unique.len() > MAX_UNIQUE_GLYPHS {
                return Err("Marker distinct glyph budget".into());
            }
        }
    }
    if bodies.is_empty() {
        return Ok(Vec::new());
    }
    check(&mut cancelled)?;
    let fonts = Fonts::new(1., ATLAS_SIDE, studio_ui_foundation::fonts::definitions());
    let id = egui::FontId::proportional(FONT_SIZE);
    // Prewarm one scalar at a time. epaint can reuse old atlas space on
    // overflow, so never publish a set that crosses our conservative fill cap.
    for c in unique {
        check(&mut cancelled)?;
        fonts.layout_no_wrap(c.to_string(), id.clone(), egui::Color32::WHITE);
        if fonts.texture_atlas().lock().fill_ratio() > 0.75 {
            return Err("Marker glyph atlas budget exhausted".into());
        }
    }
    let row_height = fonts.row_height(&id) as f64;
    let atlas_size = fonts.font_image_size();
    let mut layouts = Vec::with_capacity(bodies.len());
    let mut total_glyphs = 0usize;
    for body in bodies {
        check(&mut cancelled)?;
        let Body::Text {
            position,
            text,
            height,
            space_width,
            rgba,
        } = body
        else {
            unreachable!()
        };
        let scale = 2. * height / row_height;
        let line_count = text.split('\n').count();
        let block_height = line_count as f64 * (2. * height + 0.01) - 0.01;
        let mut glyphs = Vec::new();
        let mut width = 0f64;
        let mut missing = 0usize;
        for (line, caption) in text.split('\n').enumerate() {
            check(&mut cancelled)?;
            let galley = fonts.layout_no_wrap(caption.to_owned(), id.clone(), egui::Color32::WHITE);
            let mut shift = 0.;
            for row in &galley.rows {
                for glyph in &row.glyphs {
                    check(&mut cancelled)?;
                    missing += usize::from(!fonts.has_glyph(&id, glyph.chr));
                    let uv = glyph.uv_rect;
                    if glyph.chr == ' ' {
                        if let Some(space) = space_width {
                            shift += space - glyph.advance_width as f64 * scale;
                        }
                    } else if !uv.is_nothing() {
                        total_glyphs += 1;
                        if total_glyphs > MAX_GLYPHS {
                            return Err("Marker glyph instance budget".into());
                        }
                        let size = [uv.size.x as f64 * scale, uv.size.y as f64 * scale];
                        let left = (glyph.pos.x + uv.offset.x) as f64 * scale + shift;
                        let top = block_height * 0.5
                            - line as f64 * (2. * height + 0.01)
                            - (glyph.pos.y + uv.offset.y) as f64 * scale;
                        glyphs.push(Glyph {
                            center: [left + size[0] * 0.5, top - size[1] * 0.5],
                            dimensions: size,
                            uv: [
                                uv.min[0] as f32 / atlas_size[0] as f32,
                                uv.min[1] as f32 / atlas_size[1] as f32,
                                uv.max[0] as f32 / atlas_size[0] as f32,
                                uv.max[1] as f32 / atlas_size[1] as f32,
                            ],
                        });
                    }
                    width = width.max(glyph.max_x() as f64 * scale + shift);
                }
            }
        }
        for glyph in &mut glyphs {
            glyph.center[0] -= width * 0.5;
        }
        layouts.push((glyphs, *position, *rgba, [width, block_height], missing));
    }
    check(&mut cancelled)?;
    if fonts.font_image_size() != atlas_size || fonts.texture_atlas().lock().fill_ratio() > 0.75 {
        return Err("Marker font atlas changed after prewarm".into());
    }
    let image = fonts.image();
    if image.pixels.len() > MAX_ATLAS_BYTES {
        return Err("Marker font atlas storage limit".into());
    }
    let mut pixels = Vec::with_capacity(image.pixels.len());
    for chunk in image.pixels.chunks(4096) {
        check(&mut cancelled)?;
        pixels.extend(chunk.iter().map(|v| (v.clamp(0., 1.) * 255.).round() as u8));
    }
    let atlas = Arc::new(Atlas {
        size: atlas_size.map(|n| n as u32),
        pixels: pixels.into_boxed_slice(),
    });
    Ok(layouts
        .into_iter()
        .map(|(glyphs, position, rgba, dimensions, missing_glyphs)| {
            let bounds = glyph_bounds(&glyphs);
            Layout {
                atlas: atlas.clone(),
                glyphs: glyphs.into(),
                position,
                rgba,
                dimensions,
                missing_glyphs,
                bounds,
            }
        })
        .collect())
}

fn glyph_bounds(glyphs: &[Glyph]) -> Option<[[f64; 2]; 2]> {
    if glyphs.is_empty() {
        return None;
    }
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for g in glyphs {
        for (side, values) in bounds.iter_mut().enumerate() {
            for ((value, center), extent) in values.iter_mut().zip(g.center).zip(g.dimensions) {
                let coordinate = center + extent * if side == 0 { -0.5 } else { 0.5 };
                *value = if side == 0 {
                    value.min(coordinate)
                } else {
                    value.max(coordinate)
                };
            }
        }
    }
    Some(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(s: &str) -> Body {
        Body::Text {
            position: Point3([0.; 3]),
            text: s.into(),
            height: 0.5,
            space_width: None,
            rgba: [1.; 4],
        }
    }
    #[test]
    fn bundled_latin_chinese_and_emoji_have_real_coverage_in_shared_atlas() {
        let a = text("A中🤖");
        let b = text("café");
        let layouts = prepare(&[&a, &b], || false).unwrap();
        assert!(Arc::ptr_eq(layouts[0].atlas(), layouts[1].atlas()));
        assert_eq!(layouts[0].glyph_count(), 3);
        assert_eq!(layouts[0].missing_glyphs, 0);
        for layout in &layouts {
            let atlas = layout.atlas();
            assert!(atlas.bytes() <= MAX_ATLAS_BYTES);
            for g in layout.glyphs.iter() {
                let min = [
                    (g.uv[0] * atlas.size[0] as f32) as usize,
                    (g.uv[1] * atlas.size[1] as f32) as usize,
                ];
                let max = [
                    (g.uv[2] * atlas.size[0] as f32) as usize,
                    (g.uv[3] * atlas.size[1] as f32) as usize,
                ];
                assert!((min[1]..max[1])
                    .any(|y| (min[0]..max[0])
                        .any(|x| atlas.pixels[y * atlas.size[0] as usize + x] > 0)));
            }
        }
    }
    #[test]
    fn block_center_line_height_and_explicit_world_space_width() {
        let a = text("AA\nA");
        let layout = prepare(&[&a], || false).unwrap().remove(0);
        assert!((layout.dimensions[1] - 2.01).abs() < 1e-10);
        assert!((layout.glyphs[0].center[1] - layout.glyphs[2].center[1] - 1.01).abs() < 1e-10);
        let a = with_style("A A", 0.5, Some(0.2));
        let b = with_style("A A", 0.5, Some(0.8));
        let layouts = prepare(&[&a, &b], || false).unwrap();
        assert!((layouts[1].dimensions[0] - layouts[0].dimensions[0] - 0.6).abs() < 1e-6);
        assert!(
            (layouts[1].glyphs[0].center[0] - layouts[0].glyphs[0].center[0] + 0.3).abs() < 1e-6
        );
        assert!(
            (layouts[1].glyphs[1].center[0] - layouts[0].glyphs[1].center[0] - 0.3).abs() < 1e-6
        );
    }
    #[test]
    fn invalid_source_aggregate_unique_style_and_cancellation_fail_before_publication() {
        let large = text(&"A".repeat(studio_protocol::marker::MAX_TEXT_BYTES + 1));
        assert!(prepare(&[&large], || false).is_err());
        let a = text(&"A".repeat(4096));
        assert!(prepare(&vec![&a; 33], || false).is_err());
        let a = text(&char_range(0x4e00, 1024));
        let b = text(&char_range(0x5200, 1025));
        assert!(prepare(&[&a, &b], || false)
            .unwrap_err()
            .contains("distinct glyph"));
        assert!(prepare(&[&text("A\0B")], || false).is_err());
        assert!(prepare(&[&text("A")], || true).is_err());
        assert!(prepare(&[&with_style("A", f64::NAN, None)], || false).is_err());
    }
    fn char_range(start: u32, n: usize) -> String {
        (start..start + n as u32)
            .map(|c| char::from_u32(c).unwrap())
            .collect()
    }
    fn with_style(s: &str, height: f64, space_width: Option<f64>) -> Body {
        Body::Text {
            position: Point3([0.; 3]),
            text: s.into(),
            height,
            space_width,
            rgba: [1.; 4],
        }
    }
    #[test]
    fn maximum_two_owner_ascii_source_keeps_full_glyph_capacity() {
        let a = text(&"A".repeat(4096));
        let layouts = prepare(&vec![&a; 32], || false).unwrap();
        assert_eq!(
            layouts.iter().map(Layout::glyph_count).sum::<usize>(),
            MAX_GLYPHS
        );
        assert!(layouts
            .iter()
            .all(|l| Arc::ptr_eq(l.atlas(), layouts[0].atlas())));
    }
}
