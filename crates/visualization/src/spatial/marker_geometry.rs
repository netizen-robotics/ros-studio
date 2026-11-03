//! Worker-only Marker preparation. No ROS, projection, GPU or asset IO.
//! Lists retain bounded instances instead of expanding each point to a mesh.
use super::{map_geometry::bounded_point, Point3, RigidTransform};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use studio_protocol::{
    latest_pose::LatestPose,
    marker::{self, Geometry, Key},
    marker_snapshot::{LiveEntry, Transform},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    Cube,
    Sphere,
    Cylinder,
}
#[derive(Clone, Debug)]
pub struct Instance {
    pub pose: RigidTransform,
    pub scale: [f64; 3],
    pub rgba: [f32; 4],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColoredPoint {
    pub position: Point3,
    pub rgba: [f32; 4],
}
#[derive(Clone, Debug)]
pub enum Body {
    Instances {
        primitive: Primitive,
        instances: Arc<[Instance]>,
    },
    Points {
        points: Arc<[ColoredPoint]>,
        dimensions: [f64; 2],
    },
    Lines {
        strip: bool,
        segments: Arc<[[ColoredPoint; 2]]>,
        width: f64,
    },
    Triangles {
        triangles: Arc<[[ColoredPoint; 3]]>,
    },
    /// Billboard orientation and glyph layout belong to the renderer.
    Text {
        position: Point3,
        text: Arc<str>,
        height: f64,
        space_width: Option<f64>,
        rgba: [f32; 4],
    },
    /// Reference only; explicit local asset policy/worker must resolve it later.
    Mesh {
        pose: RigidTransform,
        scale: [f64; 3],
        resource: Arc<str>,
        embedded: bool,
        rgba: [f32; 4],
    },
    /// Pose/endpoint convention is +X, with explicit shaft/head dimensions.
    Arrow {
        pose: RigidTransform,
        shaft: f64,
        head: f64,
        shaft_diameters: [f64; 2],
        head_diameters: [f64; 2],
        rgba: [f32; 4],
    },
}
#[derive(Clone, Debug)]
pub struct PreparedEntry {
    pub key: Key,
    pub version: u64,
    pub expires_at_monotonic_ns: Option<u64>,
    pub body: Body,
}
impl PreparedEntry {
    pub fn visible_at(&self, now: u64) -> bool {
        self.expires_at_monotonic_ns.is_none_or(|t| now < t)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Expired,
    MissingTransform,
    Empty,
    ZeroScale,
    Invalid,
    Bounds,
    Unsupported,
}
#[derive(Clone, Debug)]
pub struct Hidden {
    pub key: Key,
    pub status: Status,
}
#[derive(Clone, Debug, Default)]
pub struct PreparedMarkers {
    pub entries: Vec<PreparedEntry>,
    pub hidden: Vec<Hidden>,
}
fn canceled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Marker preparation canceled".into())
    } else {
        Ok(())
    }
}
fn point(pose: RigidTransform, p: [f64; 3]) -> Result<Point3, Status> {
    let p = pose.apply(Point3(p));
    if bounded_point(p) {
        Ok(p)
    } else {
        Err(Status::Bounds)
    }
}
fn extent(pose: RigidTransform, half: [f64; 3]) -> Result<(), Status> {
    for mask in 0..8 {
        point(
            pose,
            std::array::from_fn(|i| {
                if mask & (1 << i) == 0 {
                    -half[i]
                } else {
                    half[i]
                }
            }),
        )?;
    }
    Ok(())
}
fn offset(pose: RigidTransform, p: [f64; 3]) -> RigidTransform {
    pose.compose(RigidTransform::new(p, [0., 0., 0., 1.]).unwrap())
}
fn color(m: &marker::Marker, i: usize, triangles: bool) -> [f32; 4] {
    if let Some(c) = m.geometry.colors().get(i) {
        let mut c = *c;
        if triangles {
            c[3] *= m.color[3];
        }
        c
    } else {
        m.color
    }
}
/// Bounds apply before any preparation allocation. Key errors stay scoped;
/// aggregate/identity violations and cancellation fail the entire worker job.
pub fn prepare(
    entries: &[LiveEntry],
    now: u64,
    cancel: &AtomicBool,
) -> Result<PreparedMarkers, String> {
    canceled(cancel)?;
    if entries.len() > marker::MAX_MARKERS {
        return Err("Marker entry limit".into());
    }
    let (mut points, mut text, mut uris) = (0usize, 0usize, 0usize);
    let mut keys = BTreeSet::new();
    for e in entries {
        canceled(cancel)?;
        if e.version == 0 || !keys.insert(&e.marker.key) {
            return Err("Invalid Marker key/version identity".into());
        }
        points = points
            .checked_add(e.marker.geometry.points().len())
            .ok_or("Marker points overflow")?;
        text = text
            .checked_add(e.marker.geometry.text_bytes())
            .ok_or("Marker text overflow")?;
        uris = uris
            .checked_add(e.marker.geometry.resource_bytes())
            .ok_or("Marker URI overflow")?;
    }
    if points > marker::MAX_RETAINED_POINTS
        || text > marker::MAX_RETAINED_TEXT_BYTES
        || uris > marker::MAX_RETAINED_RESOURCE_BYTES
    {
        return Err("Marker aggregate preparation limit".into());
    }
    let mut out = PreparedMarkers::default();
    for e in entries {
        canceled(cancel)?;
        match prepare_entry(e, now, cancel)? {
            Ok(body) => out.entries.push(PreparedEntry {
                key: e.marker.key.clone(),
                version: e.version,
                expires_at_monotonic_ns: e.expires_at_monotonic_ns,
                body,
            }),
            Err(status) => out.hidden.push(Hidden {
                key: e.marker.key.clone(),
                status,
            }),
        }
    }
    Ok(out)
}
fn prepare_entry(
    e: &LiveEntry,
    now: u64,
    cancel: &AtomicBool,
) -> Result<Result<Body, Status>, String> {
    let m = &e.marker;
    if m.validate().is_err() {
        return Ok(Err(Status::Invalid));
    }
    if e.expires_at_monotonic_ns.is_some_and(|t| now >= t) {
        return Ok(Err(Status::Expired));
    }
    if let Transform::Resolved { pose } = &e.transform {
        if marker::validate_transform(m.lookup_time(), pose).is_err() {
            return Ok(Err(Status::Invalid));
        }
    }
    let Transform::Resolved {
        pose: LatestPose::Ready { components, .. },
    } = &e.transform
    else {
        return Ok(Err(Status::MissingTransform));
    };
    let Some(tf) = RigidTransform::new(
        components[..3].try_into().unwrap(),
        components[3..].try_into().unwrap(),
    ) else {
        return Ok(Err(Status::Invalid));
    };
    let local = RigidTransform::new(
        m.pose[..3].try_into().unwrap(),
        m.pose[3..].try_into().unwrap(),
    )
    .unwrap();
    let pose = tf.compose(local);
    let result = (|| -> Result<Body, Status> {
        point(pose, [0.; 3])?;
        match &m.geometry {
            Geometry::Cube
            | Geometry::Sphere
            | Geometry::Cylinder
            | Geometry::CubeList { .. }
            | Geometry::SphereList { .. } => {
                if m.scale.contains(&0.) {
                    return Err(Status::ZeroScale);
                }
                let primitive = match &m.geometry {
                    Geometry::Sphere | Geometry::SphereList { .. } => Primitive::Sphere,
                    Geometry::Cylinder => Primitive::Cylinder,
                    _ => Primitive::Cube,
                };
                let points = m.geometry.points();
                let single = matches!(
                    m.geometry,
                    Geometry::Cube | Geometry::Sphere | Geometry::Cylinder
                );
                if !single && points.is_empty() {
                    return Err(Status::Empty);
                }
                let origin = [0.; 3];
                let centers = if single {
                    std::slice::from_ref(&origin)
                } else {
                    points
                };
                let mut instances = Vec::with_capacity(centers.len());
                for (i, center) in centers.iter().enumerate() {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let p = offset(pose, *center);
                    extent(p, m.scale.map(|v| v * 0.5))?;
                    instances.push(Instance {
                        pose: p,
                        scale: m.scale,
                        rgba: color(m, i, false),
                    });
                }
                Ok(Body::Instances {
                    primitive,
                    instances: instances.into(),
                })
            }
            Geometry::Points { points, .. } => {
                if m.scale[0] == 0. || m.scale[1] == 0. {
                    return Err(Status::ZeroScale);
                }
                if points.is_empty() {
                    return Err(Status::Empty);
                }
                let mut out = Vec::with_capacity(points.len());
                for (i, p) in points.iter().enumerate() {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let position = point(pose, *p)?;
                    // Camera-facing rectangle can rotate within this sphere.
                    let radius = m.scale[0].hypot(m.scale[1]) * 0.5;
                    extent(offset(RigidTransform::identity(), position.0), [radius; 3])?;
                    out.push(ColoredPoint {
                        position,
                        rgba: color(m, i, false),
                    });
                }
                Ok(Body::Points {
                    points: out.into(),
                    dimensions: [m.scale[0], m.scale[1]],
                })
            }
            Geometry::LineStrip { points, .. } | Geometry::LineList { points, .. } => {
                if m.scale[0] == 0. {
                    return Err(Status::ZeroScale);
                }
                if points.is_empty() {
                    return Err(Status::Empty);
                }
                let strip = matches!(m.geometry, Geometry::LineStrip { .. });
                let count = if strip {
                    points.len() - 1
                } else {
                    points.len() / 2
                };
                let mut segments = Vec::with_capacity(count);
                for i in 0..count {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let a = if strip { i } else { i * 2 };
                    let b = a + 1;
                    let mut ends = [ColoredPoint {
                        position: Point3([0.; 3]),
                        rgba: [0.; 4],
                    }; 2];
                    for (end, index) in [a, b].into_iter().enumerate() {
                        let position = point(pose, points[index])?;
                        extent(
                            offset(RigidTransform::identity(), position.0),
                            [m.scale[0] * 0.5; 3],
                        )?;
                        ends[end] = ColoredPoint {
                            position,
                            rgba: color(m, index, false),
                        };
                    }
                    segments.push(ends);
                }
                Ok(Body::Lines {
                    strip,
                    segments: segments.into(),
                    width: m.scale[0],
                })
            }
            Geometry::Triangles { points, .. } => {
                if m.scale.contains(&0.) {
                    return Err(Status::ZeroScale);
                }
                if points.is_empty() {
                    return Err(Status::Empty);
                }
                let mut triangles = Vec::with_capacity(points.len() / 3);
                for (i, triangle) in points.chunks_exact(3).enumerate() {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let mut output = [ColoredPoint {
                        position: Point3([0.; 3]),
                        rgba: [0.; 4],
                    }; 3];
                    for c in 0..3 {
                        output[c] = ColoredPoint {
                            position: point(
                                pose,
                                std::array::from_fn(|a| triangle[c][a] * m.scale[a]),
                            )?,
                            rgba: color(m, i * 3 + c, true),
                        };
                    }
                    triangles.push(output);
                }
                Ok(Body::Triangles {
                    triangles: triangles.into(),
                })
            }
            Geometry::Text { text } => {
                if text.chars().any(|c| c.is_control() && c != '\n') {
                    return Err(Status::Invalid);
                }
                if m.scale[2] == 0. {
                    return Err(Status::ZeroScale);
                }
                if text.is_empty() {
                    return Err(Status::Empty);
                }
                Ok(Body::Text {
                    position: point(pose, [0.; 3])?,
                    text: Arc::from(text.as_str()),
                    height: m.scale[2],
                    space_width: (m.scale[0] != 0.).then_some(m.scale[0]),
                    rgba: m.color,
                })
            }
            Geometry::Mesh {
                resource,
                use_embedded_materials,
            } => {
                if m.scale.contains(&0.) {
                    return Err(Status::ZeroScale);
                }
                Ok(Body::Mesh {
                    pose,
                    scale: m.scale,
                    resource: Arc::from(resource.as_str()),
                    embedded: *use_embedded_materials,
                    rgba: m.color,
                })
            }
            Geometry::Arrow { points } => arrow(m, pose, points),
            Geometry::ArrowStrip { .. } => Err(Status::Unsupported),
        }
    })();
    canceled(cancel)?;
    Ok(result)
}
fn arrow(m: &marker::Marker, pose: RigidTransform, points: &[[f64; 3]]) -> Result<Body, Status> {
    let (pose, shaft, head, shaft_diameters, head_diameters) = if points.is_empty() {
        if m.scale.contains(&0.) {
            return Err(Status::ZeroScale);
        }
        (
            pose,
            m.scale[0] * 0.77,
            m.scale[0] * 0.23,
            [m.scale[1], m.scale[2]],
            [m.scale[1] * 2., m.scale[2] * 2.],
        )
    } else {
        let delta = std::array::from_fn::<_, 3, _>(|i| points[1][i] - points[0][i]);
        let distance = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
        if distance == 0. {
            return Err(Status::Empty);
        }
        if m.scale[0] + m.scale[1] == 0. {
            return Err(Status::ZeroScale);
        }
        let direction = delta.map(|v| v / distance);
        // Stable +X-to-direction quaternion, including exact antiparallel.
        let sine = direction[1].hypot(direction[2]);
        let q = if sine == 0. {
            if direction[0] < 0. {
                [0., 0., 1., 0.]
            } else {
                [0., 0., 0., 1.]
            }
        } else {
            let half = sine.atan2(direction[0]) * 0.5;
            [
                0.,
                -direction[2] / sine * half.sin(),
                direction[1] / sine * half.sin(),
                half.cos(),
            ]
        };
        let local = RigidTransform::new(points[0], q).ok_or(Status::Invalid)?;
        let head = if m.scale[2] == 0. {
            distance * 0.23
        } else {
            m.scale[2].min(distance)
        };
        (
            pose.compose(local),
            distance - head,
            head,
            [m.scale[0]; 2],
            [m.scale[1]; 2],
        )
    };
    let length = shaft + head;
    let center = offset(pose, [length * 0.5, 0., 0.]);
    extent(
        center,
        [
            length * 0.5,
            head_diameters[0].max(shaft_diameters[0]) * 0.5,
            head_diameters[1].max(shaft_diameters[1]) * 0.5,
        ],
    )?;
    Ok(Body::Arrow {
        pose,
        shaft,
        head,
        shaft_diameters,
        head_diameters,
        rgba: m.color,
    })
}
#[cfg(test)]
mod tests;
