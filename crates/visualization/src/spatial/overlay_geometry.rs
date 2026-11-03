//! Camera-dependent bounded ribbons for fixed-frame Path and oriented Pose triad.
use super::{
    combined::{CombinedSceneError, PointStyle},
    map_geometry::bounded_point,
    CameraPose, Point3, RenderVertex, RigidTransform,
};
pub const MAX_PATH_VERTICES: usize = 1023 * 6;
pub const POSE_VERTICES: usize = 18;
pub const POSE_LENGTH: f64 = 0.35;
pub fn pose_segments(
    pose: RigidTransform,
) -> Result<[(Point3, Point3, [f32; 4]); 3], CombinedSceneError> {
    let origin = pose.apply(Point3([0.; 3]));
    let result = std::array::from_fn(|axis| {
        let mut p = [0.; 3];
        p[axis] = POSE_LENGTH;
        let color = [
            [0.95, 0.15, 0.12, 1.],
            [0.18, 0.9, 0.25, 1.],
            [0.25, 0.5, 1., 1.],
        ][axis];
        (origin, pose.apply(Point3(p)), color)
    });
    if result
        .iter()
        .any(|(a, b, _)| !bounded_point(*a) || !bounded_point(*b))
    {
        return Err(CombinedSceneError::new("Pose triad exceeds world bounds"));
    }
    Ok(result)
}
/// Near-clip before expanding the segment to a constant-width screen ribbon.
pub fn ribbon(
    out: &mut Vec<RenderVertex>,
    camera: CameraPose,
    size: [u32; 2],
    a: Point3,
    b: Point3,
    color: [f32; 4],
    width: f32,
) -> Result<(), CombinedSceneError> {
    PointStyle {
        rgba: color,
        size_px: width,
    }
    .validate()?;
    if size.contains(&0) || !bounded_point(a) || !bounded_point(b) {
        return Err(CombinedSceneError::new(
            "Overlay segment or viewport is invalid",
        ));
    }
    let aspect = size[0] as f32 / size[1] as f32;
    let mut a = camera.clip(a, aspect);
    let mut b = camera.clip(b, aspect);
    if a.iter().chain(b.iter()).any(|v| !v.is_finite()) {
        return Err(CombinedSceneError::new("Overlay projection is nonfinite"));
    }
    if a[2] < 0. && b[2] < 0. {
        return Ok(());
    }
    if a[2] < 0. || b[2] < 0. {
        let t = a[2] / (a[2] - b[2]);
        let mut c = std::array::from_fn(|i| a[i] + t * (b[i] - a[i]));
        c[2] = 0.;
        if a[2] < 0. {
            a = c;
        } else {
            b = c;
        }
    }
    if a[3] <= 0. || b[3] <= 0. {
        return Ok(());
    }
    let dx = (b[0] / b[3] - a[0] / a[3]) * size[0] as f32;
    let dy = (b[1] / b[3] - a[1] / a[3]) * size[1] as f32;
    let len = dx.hypot(dy).max(0.001);
    let offset = [
        -dy / len / size[0] as f32 * width * 0.5,
        dx / len / size[1] as f32 * width * 0.5,
    ];
    let shift = |mut p: [f32; 4], sign: f32| {
        p[0] += offset[0] * p[3] * sign;
        p[1] += offset[1] * p[3] * sign;
        p
    };
    let corners = [shift(a, -1.), shift(b, -1.), shift(b, 1.), shift(a, 1.)];
    for i in [0, 1, 2, 0, 2, 3] {
        out.push(RenderVertex {
            clip: corners[i],
            color,
        });
    }
    Ok(())
}
pub fn path_vertices(
    points: &[Point3],
    style: PointStyle,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, CombinedSceneError> {
    style.validate()?;
    if points.len() > 1024 || !points.iter().copied().all(bounded_point) {
        return Err(CombinedSceneError::new(
            "Path exceeds 1024 poses or world bounds",
        ));
    }
    let mut out = Vec::with_capacity(points.len().saturating_sub(1) * 6);
    for segment in points.windows(2) {
        ribbon(
            &mut out,
            camera,
            size,
            segment[0],
            segment[1],
            style.rgba,
            style.size_px,
        )?;
    }
    Ok(out)
}
pub fn pose_vertices(
    pose: RigidTransform,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, CombinedSceneError> {
    let mut out = Vec::with_capacity(POSE_VERTICES);
    for (a, b, color) in pose_segments(pose)? {
        ribbon(&mut out, camera, size, a, b, color, 2.)?;
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oriented_triad_and_path_clip_without_bridging() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let pose = RigidTransform::new([1., 2., 3.], [0., 0., q, q]).unwrap();
        let s = pose_segments(pose).unwrap();
        assert!((s[0].1 .0[1] - 2.35).abs() < 1e-10);
        assert!((s[1].1 .0[0] - 0.65).abs() < 1e-10);
        let camera = CameraPose::orbit(0., 0., 10., [0.; 3]);
        for points in [
            [Point3([10., 0., 0.]), Point3([9., 0.1, 0.])],
            [Point3([9., 0.1, 0.]), Point3([10., 0., 0.])],
        ] {
            let v = path_vertices(&points, PointStyle::default(), camera, [640, 480]).unwrap();
            assert_eq!(v.len(), 6);
            assert!(v.iter().all(|v| v.clip[2] >= 0.
                && v.clip[3] > 0.
                && v.clip.iter().all(|n| n.is_finite())));
        }
        assert!(
            path_vertices(&[], PointStyle::default(), camera, [640, 480])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            path_vertices(
                &vec![Point3([0.; 3]); 1024],
                PointStyle::default(),
                camera,
                [640, 480]
            )
            .unwrap()
            .len(),
            6138
        );
    }
}
