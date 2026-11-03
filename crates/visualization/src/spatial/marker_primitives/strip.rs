//! Continuous camera-facing strips. Adjacent quads share exactly the same
//! projected cross-section, so ordinary corners need no overlapping caps.
use super::*;
use crate::spatial::marker_geometry::ColoredPoint;

fn normalized(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = v.iter().map(|v| v * v).sum::<f64>().sqrt();
    (n > 1e-12).then(|| v.map(|v| v / n))
}
fn normal(a: Point3, b: Point3, at: Point3, camera: CameraPose) -> Option<[f64; 3]> {
    let d = normalized(std::array::from_fn(|i| b.0[i] - a.0[i]))?;
    let view = if camera.orthographic_span.is_some() {
        camera.forward.map(f64::from)
    } else {
        normalized(std::array::from_fn(|i| at.0[i] - camera.origin()[i]))?
    };
    normalized([
        d[1] * view[2] - d[2] * view[1],
        d[2] * view[0] - d[0] * view[2],
        d[0] * view[1] - d[1] * view[0],
    ])
}
impl Frame {
    /// Worker-only transactional projection. At most one instance per segment;
    /// duplicate positions collapse to the last color at that location. The
    /// miter limit is four half-widths; self-crossings/backtracking may overlap.
    pub fn push_line_strip(
        &mut self,
        points: &[ColoredPoint],
        width: f64,
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, String> {
        if points.len() > studio_protocol::marker::MAX_RETAINED_POINTS
            || !width.is_finite()
            || width <= 0.
            || width > 1_000_000.
        {
            return Err("Invalid Marker strip size/width".into());
        }
        let projection = projection::Projection::new(camera, size)?;
        let mut unique: Vec<ColoredPoint> = Vec::with_capacity(points.len());
        for p in points {
            if cancelled() {
                return Err("Marker strip projection cancelled".into());
            }
            projection.point(p.position)?;
            if p.rgba
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err("Invalid Marker strip color".into());
            }
            if unique
                .last()
                .is_some_and(|last| last.position == p.position)
            {
                *unique.last_mut().unwrap() = *p;
            } else {
                unique.push(*p);
            }
        }
        if self.len().saturating_add(unique.len().saturating_sub(1)) > MAX_INSTANCES
            || (self.len() - self.glyph_count - self.mesh_count)
                .saturating_add(unique.len().saturating_sub(1))
                > MAX_NON_TEXT_INSTANCES
        {
            return Err("Marker instance limit exceeded".into());
        }
        let mut corners = Vec::with_capacity(unique.len());
        for (i, p) in unique.iter().enumerate() {
            if cancelled() {
                return Err("Marker strip projection cancelled".into());
            }
            let prev = i
                .checked_sub(1)
                .and_then(|j| normal(unique[j].position, p.position, p.position, camera));
            let next = unique
                .get(i + 1)
                .and_then(|q| normal(p.position, q.position, p.position, camera));
            let offset = match (prev, next) {
                (Some(a), Some(b)) => {
                    if let Some(miter) = normalized(std::array::from_fn(|i| a[i] + b[i])) {
                        let dot = miter.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
                        miter.map(|v| v * width * 0.5 * (1. / dot.max(0.25)).min(4.))
                    } else {
                        b.map(|v| v * width * 0.5)
                    }
                }
                (Some(n), None) | (None, Some(n)) => n.map(|v| v * width * 0.5),
                (None, None) => [0.; 3],
            };
            let mut pair = [[0.; 4]; 2];
            for (j, side) in [-1., 1.].into_iter().enumerate() {
                pair[j] = projection.point(Point3(std::array::from_fn(|k| {
                    p.position.0[k] + offset[k] * side
                })))?;
            }
            corners.push(pair);
        }
        let start = self.len();
        let previous_projection = self.projection.clone();
        let result = (|| {
            for (i, segment) in unique.windows(2).enumerate() {
                if cancelled() {
                    return Err("Marker strip projection cancelled".into());
                }
                let center = Point3(std::array::from_fn(|j| {
                    (segment[0].position.0[j] + segment[1].position.0[j]) * 0.5
                }));
                if normal(segment[0].position, segment[1].position, center, camera).is_none() {
                    continue;
                }
                self.commit(
                    Kind::Ribbon,
                    ClipInstance {
                        columns: [
                            corners[i][0],
                            corners[i + 1][0],
                            corners[i + 1][1],
                            corners[i][1],
                        ],
                        rgba: segment[0].rgba,
                        eye: segment[1].rgba,
                        ray: [0.; 4],
                        shape: [4, 0, 0, 0],
                    },
                    camera,
                    projection.size,
                    deadline,
                )?;
            }
            Ok(self.len() - start)
        })();
        if result.is_err() {
            self.instances.truncate(start);
            self.kinds.truncate(start);
            self.key_slots.truncate(start);
            self.projection = previous_projection;
        }
        result
    }
}
