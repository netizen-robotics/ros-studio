//! Worker-prepared robot visuals and bounded camera projection. No TF ownership,
//! joint kinematics, asset IO, subscriptions or GPU resources live in this module.
use super::{CameraPose, Point3, RenderVertex, RigidTransform};
use crate::robot::{
    RobotError, RobotErrorKind, RobotGeometry, RobotModel, RobotTexture, Triangle, TriangleUv,
    MAX_COORDINATE, MAX_DIMENSION, MAX_LINKS, MAX_NAME_BYTES, MAX_ROBOT_TRIANGLES, MAX_VISUALS,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const SEGMENTS: usize = 32;
const SPHERE_RINGS: usize = 16;

#[derive(Clone, Debug)]
pub struct PreparedVisual {
    pub role: crate::robot::GeometryRole,
    /// Local presentation only. Hidden parts still count against all budgets.
    pub visible: bool,
    pub texture: Option<Arc<RobotTexture>>,
    /// Preserved per-corner UVs; model and prepared snapshot share immutable storage.
    pub uvs: Option<Arc<[TriangleUv]>>,
    pub link_index: usize,
    /// Link-space triangles: mesh scale, then visual origin, already applied.
    pub triangles: Arc<[Triangle]>,
    pub rgba: [f32; 4],
}
#[derive(Clone, Debug)]
pub struct PreparedRobot {
    /// Stable description order, including links with no visible geometry.
    pub link_names: Vec<String>,
    pub visuals: Vec<PreparedVisual>,
    /// Expanded instances, including alpha-zero visuals, count against the cap.
    pub triangle_count: usize,
}
fn error(kind: RobotErrorKind, message: &str) -> RobotError {
    RobotError::new(kind, message)
}
fn cancelled(cancel: &AtomicBool) -> Result<(), RobotError> {
    if cancel.load(Ordering::Relaxed) {
        Err(error(
            RobotErrorKind::Cancelled,
            "Robot geometry preparation cancelled",
        ))
    } else {
        Ok(())
    }
}
fn coordinate(p: [f64; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
}
fn dimension(v: f64) -> bool {
    v.is_finite() && v > 0.0 && v <= MAX_DIMENSION
}
fn material(rgba: [f32; 4]) -> Result<(), RobotError> {
    if !rgba
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    {
        return Err(error(
            RobotErrorKind::InvalidModel,
            "Robot material components must be finite and in [0,1]",
        ));
    }
    Ok(())
}
pub(super) fn triangle_valid(t: &Triangle) -> bool {
    if !t.0.iter().all(|p| coordinate(*p)) {
        return false;
    }
    let [a, b, c] = t.0;
    let u = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
    let v = std::array::from_fn::<_, 3, _>(|i| c[i] - a[i]);
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    cross.iter().all(|v| v.is_finite()) && cross.iter().any(|v| *v != 0.0)
}
pub(super) fn validate_uvs(uvs: &[TriangleUv], triangles: usize) -> Result<(), RobotError> {
    if uvs.len() != triangles
        || uvs
            .iter()
            .flatten()
            .flatten()
            .any(|n| !n.is_finite() || n.abs() > MAX_COORDINATE as f32)
    {
        return Err(error(
            RobotErrorKind::InvalidModel,
            "Mesh UVs must match triangle corners and be finite within coordinate bounds",
        ));
    }
    Ok(())
}
pub(super) fn validate_texture(image: &RobotTexture) -> Result<(), RobotError> {
    if image.width == 0
        || image.height == 0
        || image.width > 2048
        || image.height > 2048
        || image.rgba.len() != image.width as usize * image.height as usize * 4
    {
        return Err(error(
            RobotErrorKind::InvalidModel,
            "Invalid prepared image dimensions/RGBA data",
        ));
    }
    Ok(())
}
fn geometry_count(g: &RobotGeometry) -> Result<usize, RobotError> {
    if let RobotGeometry::MeshWithUv {
        texture: Some(image),
        ..
    } = g
    {
        validate_texture(image)?;
    }
    if let RobotGeometry::MeshWithUv { triangles, uvs, .. } = g {
        validate_uvs(uvs, triangles.len())?;
    }
    match g {
        RobotGeometry::Box { size } if size.iter().all(|v| dimension(*v)) => Ok(12),
        RobotGeometry::Cylinder { radius, length } if dimension(*radius) && dimension(*length) => {
            Ok(SEGMENTS * 4)
        }
        RobotGeometry::Sphere { radius } if dimension(*radius) => {
            Ok(SEGMENTS * (SPHERE_RINGS - 1) * 2)
        }
        RobotGeometry::Mesh { triangles, scale }
        | RobotGeometry::MeshWithUv {
            triangles, scale, ..
        } if scale.iter().all(|v| dimension(*v)) => {
            if triangles.is_empty() {
                Err(error(
                    RobotErrorKind::InvalidModel,
                    "Robot mesh contains no triangles",
                ))
            } else {
                Ok(triangles.len())
            }
        }
        _ => Err(error(
            RobotErrorKind::InvalidModel,
            "Robot dimensions and mesh scales must be finite, positive and at most 10000",
        )),
    }
}
fn primitive(g: &RobotGeometry, cancel: &AtomicBool) -> Result<Vec<Triangle>, RobotError> {
    let mut triangles = Vec::with_capacity(geometry_count(g)?);
    match g {
        RobotGeometry::Box { size } => {
            let p = super::primitive_mesh::CUBE_CORNERS
                .map(|p| std::array::from_fn(|i| p[i] * size[i]));
            for [a, b, c, d] in super::primitive_mesh::CUBE_FACES {
                cancelled(cancel)?;
                triangles.push(Triangle([p[a], p[b], p[c]]));
                triangles.push(Triangle([p[a], p[c], p[d]]));
            }
        }
        RobotGeometry::Cylinder { radius, length } => {
            let half = length * 0.5;
            for i in 0..SEGMENTS {
                cancelled(cancel)?;
                let ring = |i: usize, z: f64| {
                    let a = std::f64::consts::TAU * i as f64 / SEGMENTS as f64;
                    [radius * a.cos(), radius * a.sin(), z]
                };
                let a = ring(i, -half);
                let b = ring(i + 1, -half);
                let c = ring(i + 1, half);
                let d = ring(i, half);
                triangles.extend([
                    Triangle([a, b, c]),
                    Triangle([a, c, d]),
                    Triangle([[0.0, 0.0, -half], b, a]),
                    Triangle([[0.0, 0.0, half], d, c]),
                ]);
            }
        }
        RobotGeometry::Sphere { radius } => {
            let ring = |lat: usize, lon: usize| {
                let t = std::f64::consts::PI * lat as f64 / SPHERE_RINGS as f64;
                let a = std::f64::consts::TAU * lon as f64 / SEGMENTS as f64;
                [
                    radius * t.sin() * a.cos(),
                    radius * t.sin() * a.sin(),
                    radius * t.cos(),
                ]
            };
            for lon in 0..SEGMENTS {
                cancelled(cancel)?;
                triangles.push(Triangle([
                    [0.0, 0.0, *radius],
                    ring(1, lon),
                    ring(1, lon + 1),
                ]));
                for lat in 1..SPHERE_RINGS - 1 {
                    cancelled(cancel)?;
                    let a = ring(lat, lon);
                    let b = ring(lat + 1, lon);
                    let c = ring(lat + 1, lon + 1);
                    let d = ring(lat, lon + 1);
                    triangles.extend([Triangle([a, b, c]), Triangle([a, c, d])]);
                }
                triangles.push(Triangle([
                    ring(SPHERE_RINGS - 1, lon),
                    [0.0, 0.0, -*radius],
                    ring(SPHERE_RINGS - 1, lon + 1),
                ]));
            }
        }
        RobotGeometry::Mesh { .. } | RobotGeometry::MeshWithUv { .. } => {
            unreachable!("mesh triangles are shared input, never tessellated")
        }
    }
    Ok(triangles)
}
pub(crate) fn primitive_texture_count(g: &RobotGeometry) -> Result<usize, RobotError> {
    geometry_count(g)
}
/// Worker-only deterministic UV charts; poles/cylinder seams use explicit cell
/// coordinates rather than wrapping atan2 and interpolating across the seam.
pub(crate) fn textured_primitive(
    g: RobotGeometry,
    image: Arc<RobotTexture>,
    cancel: &AtomicBool,
) -> Result<RobotGeometry, RobotError> {
    let triangles = primitive(&g, cancel)?;
    let mut uvs = Vec::with_capacity(triangles.len());
    match g {
        RobotGeometry::Box { .. } => {
            for _ in 0..6 {
                uvs.extend([
                    [[0., 0.], [1., 0.], [1., 1.]],
                    [[0., 0.], [1., 1.], [0., 1.]],
                ]);
            }
        }
        RobotGeometry::Cylinder { radius, .. } => {
            for i in 0..SEGMENTS {
                cancelled(cancel)?;
                let a = i as f32 / SEGMENTS as f32;
                let b = (i + 1) as f32 / SEGMENTS as f32;
                uvs.extend([[[a, 0.], [b, 0.], [b, 1.]], [[a, 0.], [b, 1.], [a, 1.]]]);
                for t in &triangles[i * 4 + 2..i * 4 + 4] {
                    uvs.push(t.0.map(|p| {
                        [
                            (p[0] / (2. * radius) + 0.5) as f32,
                            (p[1] / (2. * radius) + 0.5) as f32,
                        ]
                    }));
                }
            }
        }
        RobotGeometry::Sphere { .. } => {
            for i in 0..SEGMENTS {
                cancelled(cancel)?;
                let a = i as f32 / SEGMENTS as f32;
                let b = (i + 1) as f32 / SEGMENTS as f32;
                let mid = (a + b) * 0.5;
                let v = |lat: usize| 1. - lat as f32 / SPHERE_RINGS as f32;
                uvs.push([[mid, 1.], [a, v(1)], [b, v(1)]]);
                for lat in 1..SPHERE_RINGS - 1 {
                    uvs.extend([
                        [[a, v(lat)], [a, v(lat + 1)], [b, v(lat + 1)]],
                        [[a, v(lat)], [b, v(lat + 1)], [b, v(lat)]],
                    ]);
                }
                uvs.push([
                    [a, v(SPHERE_RINGS - 1)],
                    [mid, 0.],
                    [b, v(SPHERE_RINGS - 1)],
                ]);
            }
        }
        _ => {
            return Err(error(
                RobotErrorKind::InvalidModel,
                "UV primitive requires a box, cylinder or sphere",
            ))
        }
    }
    validate_uvs(&uvs, triangles.len())?;
    cancelled(cancel)?;
    Ok(RobotGeometry::MeshWithUv {
        triangles: triangles.into(),
        uvs: uvs.into(),
        texture: Some(image),
        scale: [1.; 3],
    })
}
/// Run on the asset worker. Validation and cancellation precede every bounded
/// expansion; the total includes instances even when their material is hidden.
pub fn prepare_robot(model: &RobotModel, cancel: &AtomicBool) -> Result<PreparedRobot, RobotError> {
    cancelled(cancel)?;
    if model.links.is_empty() || model.links.len() > MAX_LINKS {
        return Err(error(RobotErrorKind::Limit, "Robot link budget is 1..128"));
    }
    let mut visual_count = 0usize;
    let mut triangle_count = 0usize;
    let mut link_names = Vec::with_capacity(model.links.len());
    for link in &model.links {
        cancelled(cancel)?;
        if !studio_protocol::spatial::valid_frame(&link.name)
            || link.name.len() > MAX_NAME_BYTES
            || link_names.contains(&link.name)
        {
            return Err(error(
                RobotErrorKind::InvalidModel,
                "Robot link names must be unique, bounded TF frames without leading slash, whitespace or control characters",
            ));
        }
        link_names.push(link.name.clone());
        visual_count = visual_count
            .checked_add(link.visuals.len())
            .filter(|n| *n <= MAX_VISUALS)
            .ok_or_else(|| error(RobotErrorKind::Limit, "Robot exceeds 256 visuals"))?;
        for visual in &link.visuals {
            cancelled(cancel)?;
            material(visual.rgba)?;
            triangle_count = triangle_count
                .checked_add(geometry_count(&visual.geometry)?)
                .filter(|n| *n <= MAX_ROBOT_TRIANGLES)
                .ok_or_else(|| {
                    error(
                        RobotErrorKind::Limit,
                        "Expanded robot exceeds 50000 triangles",
                    )
                })?;
        }
    }
    let mut images: Vec<&Arc<RobotTexture>> = Vec::new();
    let mut bytes = 0usize;
    for image in model
        .links
        .iter()
        .flat_map(|l| &l.visuals)
        .filter_map(|v| match &v.geometry {
            RobotGeometry::MeshWithUv {
                texture: Some(t), ..
            } => Some(t),
            _ => None,
        })
    {
        if !images.iter().any(|old| Arc::ptr_eq(old, image)) {
            images.push(image);
            bytes += image.rgba.len();
        }
        if images.len() > 16 || bytes > 32 * 1024 * 1024 {
            return Err(error(
                RobotErrorKind::Limit,
                "Prepared textures exceed16images/32MiB",
            ));
        }
    }
    let mut visuals = Vec::with_capacity(visual_count);
    for (link_index, link) in model.links.iter().enumerate() {
        for visual in &link.visuals {
            cancelled(cancel)?;
            let generated;
            let (source, scale) = match &visual.geometry {
                RobotGeometry::Mesh { triangles, scale }
                | RobotGeometry::MeshWithUv {
                    triangles, scale, ..
                } => (triangles.as_ref(), *scale),
                geometry => {
                    generated = primitive(geometry, cancel)?;
                    (generated.as_slice(), [1.0; 3])
                }
            };
            let mut prepared = Vec::with_capacity(source.len());
            for t in source {
                cancelled(cancel)?;
                if !triangle_valid(t) {
                    return Err(error(RobotErrorKind::InvalidModel,"Robot triangle is degenerate, nonfinite or outside local coordinate bounds"));
                }
                let scaled = Triangle(t.0.map(|p| std::array::from_fn(|i| p[i] * scale[i])));
                if !triangle_valid(&scaled) {
                    return Err(error(
                        RobotErrorKind::InvalidModel,
                        "Scaled robot triangle exceeds coordinate bounds or degenerates",
                    ));
                }
                let t = Triangle(scaled.0.map(|p| visual.origin.apply(Point3(p)).0));
                if !triangle_valid(&t) {
                    return Err(error(
                        RobotErrorKind::InvalidModel,
                        "Robot visual origin produces invalid link-space geometry",
                    ));
                }
                prepared.push(t);
            }
            visuals.push(PreparedVisual {
                role: visual.role,
                visible: visual.role == crate::robot::GeometryRole::Visual,
                texture: match &visual.geometry {
                    RobotGeometry::MeshWithUv { texture, .. } => texture.clone(),
                    _ => None,
                },
                uvs: match &visual.geometry {
                    RobotGeometry::MeshWithUv { uvs, .. } => Some(Arc::clone(uvs)),
                    _ => None,
                },
                link_index,
                triangles: prepared.into(),
                rgba: visual.rgba,
            });
        }
    }
    cancelled(cancel)?;
    Ok(PreparedRobot {
        link_names,
        visuals,
        triangle_count,
    })
}
/// Applies only externally supplied fixed<-link poses. No joint origins or FK.
/// Missing poses hide complete links. Clip coordinates retain w for GPU clipping.
pub fn robot_vertices(
    prepared: &PreparedRobot,
    poses: &[Option<RigidTransform>],
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, RobotError> {
    robot_vertices_filtered(prepared, poses, camera, size, None)
}
#[derive(Clone)]
pub(super) struct TextureBatch {
    pub vertices: Vec<RenderVertex>,
    pub image: Arc<RobotTexture>,
    pub rgba: [f32; 4],
    pub transparent: bool,
}
pub(super) fn texture_batches(
    prepared: &PreparedRobot,
    poses: &[Option<RigidTransform>],
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<TextureBatch>, RobotError> {
    static WHITE: std::sync::OnceLock<Arc<RobotTexture>> = std::sync::OnceLock::new();
    let mut out = Vec::new();
    let mut images: Vec<&Arc<RobotTexture>> = Vec::new();
    let mut bytes = 0usize;
    for (i, v) in prepared.visuals.iter().enumerate() {
        if v.visible && (v.texture.is_some() || (v.rgba[3] > 0. && v.rgba[3] < 1.)) {
            let image = v.texture.as_ref().unwrap_or_else(|| {
                WHITE.get_or_init(|| {
                    Arc::new(RobotTexture {
                        width: 1,
                        height: 1,
                        rgba: vec![255; 4],
                        has_transparency: false,
                    })
                })
            });
            validate_texture(image)?;
            if v.texture.is_some() && !images.iter().any(|old| Arc::ptr_eq(old, image)) {
                images.push(image);
                bytes += image.rgba.len();
            }
            if images.len() > 16 || bytes > 32 * 1024 * 1024 {
                return Err(error(RobotErrorKind::Limit, "Texture budget exceeded"));
            }
            let vertices = robot_vertices_filtered(prepared, poses, camera, size, Some(i))?;
            if !vertices.is_empty() {
                out.push(TextureBatch {
                    vertices,
                    image: Arc::clone(image),
                    rgba: v.rgba,
                    transparent: image.has_transparency || v.rgba[3] < 1.,
                });
            }
        }
    }
    Ok(out)
}
fn robot_vertices_filtered(
    prepared: &PreparedRobot,
    poses: &[Option<RigidTransform>],
    camera: CameraPose,
    size: [u32; 2],
    textured: Option<usize>,
) -> Result<Vec<RenderVertex>, RobotError> {
    if prepared.link_names.len() > MAX_LINKS || poses.len() != prepared.link_names.len() {
        return Err(error(
            RobotErrorKind::Pose,
            "Robot pose batch must match stable link indices",
        ));
    }
    if prepared.visuals.len() > MAX_VISUALS || prepared.triangle_count > MAX_ROBOT_TRIANGLES {
        return Err(error(
            RobotErrorKind::Limit,
            "Prepared robot exceeds geometry budget",
        ));
    }
    let actual = prepared
        .visuals
        .iter()
        .try_fold(0usize, |n, v| n.checked_add(v.triangles.len()))
        .ok_or_else(|| error(RobotErrorKind::Limit, "Robot triangle count overflow"))?;
    if actual != prepared.triangle_count || actual > MAX_ROBOT_TRIANGLES {
        return Err(error(
            RobotErrorKind::Limit,
            "Prepared robot triangle budget is inconsistent",
        ));
    }
    if size.contains(&0) {
        return Err(error(
            RobotErrorKind::InvalidModel,
            "Robot viewport dimensions must be nonzero",
        ));
    }
    let aspect = size[0] as f32 / size[1] as f32;
    let capacity = prepared
        .visuals
        .iter()
        .enumerate()
        .filter(|(i, v)| {
            v.visible
                && textured.map_or(
                    v.texture.is_none() && (v.rgba[3] == 0. || v.rgba[3] == 1.),
                    |t| t == *i,
                )
        })
        .map(|(_, v)| v.triangles.len() * 3)
        .sum();
    let mut vertices = Vec::with_capacity(capacity);
    for (visual_index, visual) in prepared.visuals.iter().enumerate() {
        if !visual.visible {
            continue;
        }
        if textured.map_or(
            visual.texture.is_some() || (visual.rgba[3] > 0. && visual.rgba[3] < 1.),
            |i| i != visual_index,
        ) {
            continue;
        }
        if let Some(uvs) = &visual.uvs {
            validate_uvs(uvs, visual.triangles.len())?;
        }
        material(visual.rgba)?;
        let pose = poses
            .get(visual.link_index)
            .ok_or_else(|| error(RobotErrorKind::Pose, "Robot visual has invalid link index"))?;
        let Some(pose) = pose else { continue };
        if !coordinate(pose.apply(Point3([0.0; 3])).0) {
            return Err(error(
                RobotErrorKind::Pose,
                "Robot link pose exceeds world coordinate bounds",
            ));
        }
        if visual.rgba[3] == 0.0 {
            continue;
        }
        for (triangle_index, triangle) in visual.triangles.iter().enumerate() {
            if !triangle_valid(triangle) {
                return Err(error(
                    RobotErrorKind::InvalidModel,
                    "Prepared robot triangle is invalid",
                ));
            }
            for (corner, p) in triangle.0.into_iter().enumerate() {
                let world = pose.apply(Point3(p));
                if !coordinate(world.0) {
                    return Err(error(
                        RobotErrorKind::Pose,
                        "Robot transformed geometry exceeds world coordinate bounds",
                    ));
                }
                let clip = camera.clip(world, aspect);
                if !clip.iter().all(|v| v.is_finite()) {
                    return Err(error(
                        RobotErrorKind::Pose,
                        "Robot camera projection is nonfinite",
                    ));
                }
                vertices.push(RenderVertex {
                    clip,
                    color: if textured.is_some() {
                        let uv = if visual.texture.is_some() {
                            visual.uvs.as_ref().ok_or_else(|| {
                                error(RobotErrorKind::InvalidModel, "Textured visual has no UVs")
                            })?[triangle_index][corner]
                        } else {
                            [0.5, 0.5]
                        };
                        [uv[0], uv[1], 0., 1.]
                    } else {
                        visual.rgba
                    },
                });
            }
        }
    }
    Ok(vertices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::{RobotLink, RobotVisual};
    fn triangle() -> Triangle {
        Triangle([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
    }
    fn visual(geometry: RobotGeometry) -> RobotVisual {
        RobotVisual {
            role: crate::robot::GeometryRole::Visual,
            origin: RigidTransform::identity(),
            geometry,
            rgba: [0.2, 0.4, 0.6, 1.0],
        }
    }
    fn model(visuals: Vec<RobotVisual>) -> RobotModel {
        RobotModel {
            name: "fixture".into(),
            links: vec![RobotLink {
                name: "base".into(),
                visuals,
            }],
            joints: vec![],
            root_link: 0,
        }
    }
    fn prepare(m: &RobotModel) -> Result<PreparedRobot, RobotError> {
        prepare_robot(m, &AtomicBool::new(false))
    }
    fn near(actual: [f64; 3], expected: [f64; 3]) {
        for (a, b) in actual.into_iter().zip(expected) {
            assert!(
                (a - b).abs() < 1e-10,
                "actual={actual:?} expected={expected:?}"
            );
        }
    }
    #[test]
    fn primitive_texture_charts_preserve_counts_bounds_and_seams() {
        let image = Arc::new(RobotTexture {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            has_transparency: false,
        });
        for (g, count) in [
            (RobotGeometry::Box { size: [1., 2., 3.] }, 12),
            (
                RobotGeometry::Cylinder {
                    radius: 1.,
                    length: 2.,
                },
                128,
            ),
            (RobotGeometry::Sphere { radius: 1. }, 960),
        ] {
            let m = model(vec![visual(
                textured_primitive(g, Arc::clone(&image), &AtomicBool::new(false)).unwrap(),
            )]);
            let p = prepare(&m).unwrap();
            assert_eq!(p.triangle_count, count);
            let uv = p.visuals[0].uvs.as_ref().unwrap();
            assert_eq!(uv.len(), count);
            assert!(uv
                .iter()
                .flatten()
                .flatten()
                .all(|n| (0.0..=1.0).contains(n)));
            assert!(uv.iter().flatten().any(|p| p[0] == 0.));
            assert!(uv.iter().flatten().any(|p| p[0] == 1.));
            assert!(Arc::ptr_eq(p.visuals[0].texture.as_ref().unwrap(), &image));
        }
        assert!(textured_primitive(
            RobotGeometry::Box { size: [1.; 3] },
            image,
            &AtomicBool::new(true)
        )
        .is_err());
    }
    #[test]
    fn uv_preparation_preserves_storage_and_rejects_invalid_cardinality_or_values() {
        let triangles: Arc<[Triangle]> =
            vec![Triangle([[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]])].into();
        let uvs: Arc<[TriangleUv]> = vec![[[0., 0.], [2., 0.], [0., 1.]]].into();
        let mut v = visual(RobotGeometry::MeshWithUv {
            texture: None,
            triangles: Arc::clone(&triangles),
            uvs: Arc::clone(&uvs),
            scale: [2., 3., 4.],
        });
        v.origin = RigidTransform::new([5., 6., 7.], [0., 0., 0., 1.]).unwrap();
        let p = prepare(&model(vec![v])).unwrap();
        assert!(Arc::ptr_eq(p.visuals[0].uvs.as_ref().unwrap(), &uvs));
        assert_eq!(
            p.visuals[0].triangles[0],
            Triangle([[5., 6., 7.], [7., 6., 7.], [5., 9., 7.]])
        );
        for invalid in [
            Vec::new(),
            vec![[[f32::NAN, 0.]; 3]],
            vec![[[1_000_001., 0.]; 3]],
        ] {
            let m = model(vec![visual(RobotGeometry::MeshWithUv {
                texture: None,
                triangles: Arc::clone(&triangles),
                uvs: invalid.into(),
                scale: [1.; 3],
            })]);
            assert_eq!(prepare(&m).unwrap_err().kind, RobotErrorKind::InvalidModel);
        }
    }
    #[test]
    fn primitive_extents_and_tessellation_are_bounded() {
        for (geometry, count, extent) in [
            (
                RobotGeometry::Box {
                    size: [2.0, 4.0, 6.0],
                },
                12,
                [1.0, 2.0, 3.0],
            ),
            (
                RobotGeometry::Cylinder {
                    radius: 2.0,
                    length: 6.0,
                },
                128,
                [2.0, 2.0, 3.0],
            ),
            (RobotGeometry::Sphere { radius: 2.0 }, 960, [2.0; 3]),
        ] {
            let robot = prepare(&model(vec![visual(geometry)])).unwrap();
            assert_eq!(robot.triangle_count, count);
            let triangles = &robot.visuals[0].triangles;
            assert!(triangles.iter().all(triangle_valid));
            for axis in 0..3 {
                let max = triangles
                    .iter()
                    .flat_map(|t| t.0)
                    .map(|p| p[axis])
                    .fold(f64::NEG_INFINITY, f64::max);
                let min = triangles
                    .iter()
                    .flat_map(|t| t.0)
                    .map(|p| p[axis])
                    .fold(f64::INFINITY, f64::min);
                assert!((max - extent[axis]).abs() < 1e-10);
                assert!((min + extent[axis]).abs() < 1e-10);
            }
        }
    }
    #[test]
    fn mesh_scale_then_visual_origin_then_external_pose_without_fk() {
        let q = std::f64::consts::FRAC_PI_4;
        let mut v = visual(RobotGeometry::Mesh {
            triangles: Arc::from([triangle()]),
            scale: [2.0, 1.0, 0.5],
        });
        v.origin = RigidTransform::new([0.2, 0.0, 0.0], [0.0, 0.0, q.sin(), q.cos()]).unwrap();
        let robot = prepare(&model(vec![v])).unwrap();
        near(robot.visuals[0].triangles[0].0[0], [0.2, 2.0, 0.0]);
        let pose = RigidTransform::new([1.0, 2.0, 0.2], [0.0, 0.0, q.sin(), q.cos()]).unwrap();
        let camera = CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]);
        let vertices = robot_vertices(&robot, &[Some(pose)], camera, [800, 600]).unwrap();
        assert_eq!(vertices.len(), 3);
        let expected = camera.clip(Point3([-1.0, 2.2, 0.2]), 800.0 / 600.0);
        for (a, b) in vertices[0].clip.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-5);
        }
        assert_eq!(vertices[0].color, [0.2, 0.4, 0.6, 1.0]);
    }
    #[test]
    fn missing_and_alpha_zero_hide_geometry_without_shifting_link_indices() {
        let v = visual(RobotGeometry::Box { size: [1.0; 3] });
        let mut hidden = v.clone();
        hidden.rgba[3] = 0.0;
        let mut m = model(vec![v]);
        m.links.push(RobotLink {
            name: "hidden".into(),
            visuals: vec![hidden],
        });
        m.links.push(RobotLink {
            name: "no_visuals".into(),
            visuals: vec![],
        });
        let robot = prepare(&m).unwrap();
        assert_eq!(robot.link_names, ["base", "hidden", "no_visuals"]);
        assert_eq!(robot.visuals[1].link_index, 1);
        assert_eq!(robot.triangle_count, 24);
        let camera = CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]);
        assert!(robot_vertices(
            &robot,
            &[None, Some(RigidTransform::identity()), None],
            camera,
            [800, 600]
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            robot_vertices(
                &robot,
                &[Some(RigidTransform::identity()), None, None],
                camera,
                [800, 600]
            )
            .unwrap()
            .len(),
            36
        );
        assert_eq!(
            robot_vertices(&robot, &[None], camera, [800, 600])
                .unwrap_err()
                .kind,
            RobotErrorKind::Pose
        );
    }
    #[test]
    fn expanded_instances_and_hidden_visuals_still_obey_total_budget() {
        let mesh = RobotGeometry::Mesh {
            triangles: vec![triangle(); MAX_ROBOT_TRIANGLES].into(),
            scale: [1.0; 3],
        };
        let mut v = visual(mesh);
        v.rgba[3] = 0.0;
        assert_eq!(
            prepare(&model(vec![v.clone()])).unwrap().triangle_count,
            MAX_ROBOT_TRIANGLES
        );
        assert_eq!(
            prepare(&model(vec![v.clone(), v])).unwrap_err().kind,
            RobotErrorKind::Limit
        );
        let sphere = visual(RobotGeometry::Sphere { radius: 1.0 });
        assert_eq!(
            prepare(&model(vec![sphere; 53])).unwrap_err().kind,
            RobotErrorKind::Limit
        );
    }
    #[test]
    fn invalid_material_geometry_pose_and_cancellation_fail_explicitly() {
        let mut v = visual(RobotGeometry::Box { size: [1.0; 3] });
        v.rgba[3] = 1.5;
        assert_eq!(
            prepare(&model(vec![v])).unwrap_err().kind,
            RobotErrorKind::InvalidModel
        );
        for geometry in [
            RobotGeometry::Sphere { radius: f64::NAN },
            RobotGeometry::Cylinder {
                radius: 1.0,
                length: 0.0,
            },
            RobotGeometry::Box {
                size: [10001.0, 1.0, 1.0],
            },
            RobotGeometry::Mesh {
                triangles: Arc::from([Triangle([[0.0; 3]; 3])]),
                scale: [1.0; 3],
            },
            RobotGeometry::Mesh {
                triangles: Arc::from([triangle()]),
                scale: [-1.0, 1.0, 1.0],
            },
        ] {
            assert_eq!(
                prepare(&model(vec![visual(geometry)])).unwrap_err().kind,
                RobotErrorKind::InvalidModel
            );
        }
        let m = model(vec![visual(RobotGeometry::Box { size: [1.0; 3] })]);
        assert_eq!(
            prepare_robot(&m, &AtomicBool::new(true)).unwrap_err().kind,
            RobotErrorKind::Cancelled
        );
        let robot = prepare(&m).unwrap();
        let camera = CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]);
        let pose = RigidTransform::new([MAX_COORDINATE, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]).unwrap();
        assert_eq!(
            robot_vertices(&robot, &[Some(pose)], camera, [800, 600])
                .unwrap_err()
                .kind,
            RobotErrorKind::Pose
        );
        assert!(robot_vertices(&robot, &[None], camera, [0, 600]).is_err());
    }
    #[test]
    fn constructed_models_reject_names_the_pose_protocol_cannot_address() {
        for name in ["/base", "base link", "base\nlink", "base\u{0}link", ""] {
            let mut m = model(vec![visual(RobotGeometry::Box { size: [1.0; 3] })]);
            m.links[0].name = name.into();
            assert_eq!(
                prepare(&m).unwrap_err().kind,
                RobotErrorKind::InvalidModel,
                "invalid TF link name {name:?}"
            );
        }
        let mut m = model(vec![visual(RobotGeometry::Box { size: [1.0; 3] })]);
        m.links[0].name = "robot/base_link".into();
        assert_eq!(prepare(&m).unwrap().link_names, ["robot/base_link"]);
    }
    #[test]
    fn mutated_prepared_data_cannot_bypass_triangle_budget() {
        let mut robot =
            prepare(&model(vec![visual(RobotGeometry::Box { size: [1.0; 3] })])).unwrap();
        robot.triangle_count = 0;
        assert_eq!(
            robot_vertices(
                &robot,
                &[Some(RigidTransform::identity())],
                CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]),
                [800, 600]
            )
            .unwrap_err()
            .kind,
            RobotErrorKind::Limit
        );
    }
}
