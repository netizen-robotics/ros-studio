//! Bridge immutable worker-prepared bodies into the shared instance pipeline.
use super::*;
use crate::spatial::{marker_geometry::Body, RigidTransform};

fn check(cancelled: &mut impl FnMut() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Marker projection cancelled".into())
    } else {
        Ok(())
    }
}
impl Frame {
    /// Append one prepared key atomically. The caller owns source/version/TF
    /// authorization and must run projection off the UI thread before sealing.
    pub fn push_geometry(
        &mut self,
        body: &Body,
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<usize, String> {
        check(&mut cancelled)?;
        let max = studio_protocol::marker::MAX_RETAINED_POINTS;
        let count = match body {
            Body::Instances { instances, .. } => instances.len(),
            Body::Points { points, .. } => points.len(),
            Body::Lines {
                segments, strip, ..
            } => {
                let points = if *strip {
                    segments.len().saturating_add(1)
                } else {
                    segments.len().saturating_mul(2)
                };
                if points > max {
                    return Err("Marker projected source point limit".into());
                }
                segments.len()
            }
            Body::Triangles { triangles } => {
                if triangles.len().saturating_mul(3) > max {
                    return Err("Marker projected source point limit".into());
                }
                triangles.len()
            }
            Body::Arrow { .. } => 2,
            Body::Text { .. } | Body::Mesh { .. } => {
                return Err("Marker text/mesh projection requires asset preparation".into())
            }
        };
        if count > max
            || self.len().saturating_add(count) > MAX_INSTANCES
            || (self.len() - self.glyph_count - self.mesh_count).saturating_add(count)
                > MAX_NON_TEXT_INSTANCES
        {
            return Err("Marker projected instance limit".into());
        }
        projection::Projection::new(camera, size)?;
        let start = self.len();
        let old_projection = self.projection.clone();
        let result = (|| {
            match body {
                Body::Instances {
                    primitive,
                    instances,
                } => {
                    for instance in instances.iter() {
                        check(&mut cancelled)?;
                        self.push_timed((*primitive).into(), instance, camera, size, deadline)?;
                    }
                }
                Body::Points { points, dimensions } => {
                    for p in points.iter() {
                        check(&mut cancelled)?;
                        self.push_point(*p, *dimensions, camera, size, deadline)?;
                    }
                }
                Body::Lines {
                    strip,
                    segments,
                    width,
                } => {
                    if *strip && !segments.is_empty() {
                        let mut points = Vec::with_capacity(segments.len() + 1);
                        points.push(segments[0][0]);
                        for [a, b] in segments.iter() {
                            check(&mut cancelled)?;
                            if points.last() != Some(a) {
                                return Err("Prepared Marker strip is disconnected".into());
                            }
                            points.push(*b);
                        }
                        self.push_line_strip(
                            &points,
                            *width,
                            camera,
                            size,
                            deadline,
                            &mut cancelled,
                        )?;
                    } else {
                        for [a, b] in segments.iter() {
                            check(&mut cancelled)?;
                            self.push_line(*a, *b, *width, camera, size, deadline)?;
                        }
                    }
                }
                Body::Triangles { triangles } => {
                    for triangle in triangles.iter() {
                        check(&mut cancelled)?;
                        self.push_triangle(*triangle, camera, size, deadline)?;
                    }
                }
                Body::Arrow {
                    pose,
                    shaft,
                    head,
                    shaft_diameters,
                    head_diameters,
                    rgba,
                } => {
                    if [*shaft, *head]
                        .iter()
                        .chain(shaft_diameters)
                        .chain(head_diameters)
                        .any(|v| !v.is_finite() || *v < 0. || *v > 1_000_000.)
                        || rgba
                            .iter()
                            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                    {
                        return Err("Invalid prepared Marker arrow".into());
                    }
                    for (kind, center, length, diameters) in [
                        (Kind::Cylinder, shaft * 0.5, *shaft, shaft_diameters),
                        (Kind::Cone, shaft + head * 0.5, *head, head_diameters),
                    ] {
                        check(&mut cancelled)?;
                        if length == 0. || diameters.contains(&0.) {
                            continue;
                        }
                        // Cyclic basis: local X->ROS Y, Y->Z, Z->X. This
                        // preserves the two distinct elliptical diameters.
                        let local = RigidTransform::new([center, 0., 0.], [0.5; 4]).unwrap();
                        let instance = Instance {
                            pose: pose.compose(local),
                            scale: [diameters[0], diameters[1], length],
                            rgba: *rgba,
                        };
                        self.push_timed(kind, &instance, camera, size, deadline)?;
                    }
                }
                Body::Text { .. } | Body::Mesh { .. } => unreachable!(),
            }
            check(&mut cancelled)?;
            Ok(self.len() - start)
        })();
        if result.is_err() {
            self.instances.truncate(start);
            self.kinds.truncate(start);
            self.key_slots.truncate(start);
            self.projection = old_projection;
        }
        result
    }
}
