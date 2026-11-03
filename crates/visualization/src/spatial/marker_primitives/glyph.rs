use super::*;
use crate::spatial::marker_text::Layout;
impl Frame {
    /// Worker-side camera-facing text projection with atomic key rollback.
    pub fn push_text(
        &mut self,
        layout: &Layout,
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, String> {
        if self.glyph_count.saturating_add(layout.glyphs.len())
            > super::super::marker_text::MAX_GLYPHS
            || self.len().saturating_add(layout.glyphs.len()) > MAX_INSTANCES
        {
            return Err("Marker glyph instance limit".into());
        }
        if self
            .atlas
            .as_ref()
            .is_some_and(|a| !Arc::ptr_eq(a, &layout.atlas))
        {
            return Err("Marker frame mixes glyph atlas identities".into());
        }
        let projection = projection::Projection::new(camera, size)?;
        let start = self.len();
        let old_projection = self.projection.clone();
        let old_count = self.glyph_count;
        let result = (|| {
            for glyph in layout.glyphs.iter() {
                if cancelled() {
                    return Err("Marker glyph projection cancelled".into());
                }
                let origin = Point3(std::array::from_fn(|i| {
                    layout.position.0[i]
                        + camera.right[i] as f64 * glyph.center[0]
                        + camera.up[i] as f64 * glyph.center[1]
                }));
                let x = camera.right.map(|v| v as f64 * glyph.dimensions[0]);
                let y = camera.up.map(|v| v as f64 * glyph.dimensions[1]);
                for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                    projection.point(Point3(std::array::from_fn(|i| {
                        origin.0[i] + x[i] * a + y[i] * b
                    })))?;
                }
                self.commit(
                    Kind::Glyph,
                    ClipInstance {
                        columns: [
                            projection.delta(x, 0.)?,
                            projection.delta(y, 0.)?,
                            [0.; 4],
                            projection.point(origin)?,
                        ],
                        rgba: layout.rgba,
                        eye: glyph.uv,
                        ray: [0.; 4],
                        shape: [5, 0, 0, 0],
                    },
                    camera,
                    projection.size,
                    deadline,
                )?;
            }
            if cancelled() {
                return Err("Marker glyph projection cancelled".into());
            }
            Ok(self.len() - start)
        })();
        if result.is_err() {
            self.instances.truncate(start);
            self.kinds.truncate(start);
            self.key_slots.truncate(start);
            self.projection = old_projection;
            self.glyph_count = old_count;
        } else if self.glyph_count > old_count {
            self.atlas = Some(layout.atlas.clone());
        }
        result
    }
}
