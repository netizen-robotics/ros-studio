//! Worker-computed conservative visible bounds; UI fit only scans key metadata.
use super::*;
use crate::spatial::{camera_fit::Bounds, Point3, RigidTransform};
#[derive(Default)]
struct Accumulator {
    bounds: Option<Bounds>,
    invalid: bool,
}
impl Accumulator {
    fn point(&mut self, p: Point3) {
        if !crate::spatial::map_geometry::bounded_point(p) {
            self.invalid = true;
            return;
        }
        if let Some(b) = &mut self.bounds {
            for i in 0..3 {
                b.min[i] = b.min[i].min(p.0[i]);
                b.max[i] = b.max[i].max(p.0[i]);
            }
        } else {
            self.bounds = Some(Bounds { min: p.0, max: p.0 });
        }
    }
    fn box_at(&mut self, pose: RigidTransform, min: [f64; 3], max: [f64; 3]) {
        for mask in 0..8 {
            self.point(pose.apply(Point3(std::array::from_fn(|i| {
                if mask & (1 << i) == 0 {
                    min[i]
                } else {
                    max[i]
                }
            }))));
        }
    }
    fn sphere(&mut self, p: Point3, r: f64) {
        self.point(Point3(p.0.map(|v| v - r)));
        self.point(Point3(p.0.map(|v| v + r)));
    }
}
pub(super) fn prepare(drawing: &Drawing) -> Result<Option<Bounds>, String> {
    let mut a = Accumulator::default();
    match drawing {
        Drawing::Text(t) if t.rgba[3] > 0. => {
            if let Some(b) = t.local_bounds() {
                let radius = b.into_iter().map(|p| p[0].hypot(p[1])).fold(0., f64::max);
                a.sphere(t.position, radius);
            }
        }
        Drawing::Text(_) => (),
        Drawing::Mesh(m) => {
            if m.parts.iter().any(|p| p.base[3] > 0. || p.tint[3] > 0.) {
                let b = m.bounds();
                a.point(Point3(b[0]));
                a.point(Point3(b[1]));
            }
        }
        Drawing::Geometry(body) => match body {
            Body::Instances { instances, .. } => {
                for i in instances.iter().filter(|i| i.rgba[3] > 0.) {
                    a.box_at(i.pose, i.scale.map(|v| -v / 2.), i.scale.map(|v| v / 2.));
                }
            }
            Body::Points { points, dimensions } => {
                for p in points.iter().filter(|p| p.rgba[3] > 0.) {
                    a.sphere(p.position, dimensions[0].hypot(dimensions[1]) / 2.);
                }
            }
            Body::Lines {
                segments,
                width,
                strip,
            } => {
                for [x, y] in segments
                    .iter()
                    .filter(|[x, y]| x.rgba[3] > 0. || y.rgba[3] > 0.)
                {
                    let r = if *strip { 2. * width } else { width / 2. };
                    a.sphere(x.position, r);
                    a.sphere(y.position, r);
                }
            }
            Body::Triangles { triangles } => {
                for triangle in triangles
                    .iter()
                    .filter(|t| t.iter().any(|p| p.rgba[3] > 0.))
                {
                    for p in triangle {
                        a.point(p.position);
                    }
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
                if rgba[3] > 0. {
                    let y = shaft_diameters[0].max(head_diameters[0]) / 2.;
                    let z = shaft_diameters[1].max(head_diameters[1]) / 2.;
                    a.box_at(*pose, [0., -y, -z], [shaft + head, y, z]);
                }
            }
            Body::Text { .. } | Body::Mesh { .. } => {
                return Err("Unprepared Marker asset bounds".into())
            }
        },
    }
    if a.invalid {
        Err("Marker visible bounds exceed the supported world range".into())
    } else {
        Ok(a.bounds)
    }
}
