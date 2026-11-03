//! Immutable fixed-frame cell corners; preparation happens on the data worker.
use super::{combined::CombinedSceneError, CameraPose, Point3, RenderVertex, RigidTransform};
use std::sync::Arc;
use studio_protocol::mapping;
pub const MAX_MAP_VERTICES: usize = mapping::MAX_MAP_CELLS * 6;
pub const MAX_MAP_CORNERS: usize = 257 * 257;
pub const MAX_PROJECTED_CORNER_BYTES: usize = MAX_MAP_CORNERS * std::mem::size_of::<[f32; 4]>();
#[derive(Clone, Debug)]
pub struct PreparedMap {
    pub width: u32,
    pub height: u32,
    /// Row-major (width+1) × (height+1), already origin- and TF-transformed.
    pub corners: Arc<[Point3]>,
    pub cells: Arc<[i8]>,
}
pub fn bounded_point(p: Point3) -> bool {
    p.0.iter()
        .all(|v| v.is_finite() && v.abs() <= mapping::MAX_COORDINATE)
}
pub fn prepare_map(
    width: u32,
    height: u32,
    resolution: f64,
    origin: RigidTransform,
    tf: RigidTransform,
    cells: &[i8],
) -> Result<PreparedMap, CombinedSceneError> {
    let error = || {
        CombinedSceneError::new(
            "Map dimensions, resolution, cells or transformed bounds are invalid",
        )
    };
    if width > 256
        || height > 256
        || cells.len() != width as usize * height as usize
        || ((width == 0 || height == 0) && width != height)
        || !resolution.is_finite()
        || !(mapping::MIN_RESOLUTION..=mapping::MAX_RESOLUTION).contains(&resolution)
        || cells.iter().any(|c| !(-1..=100).contains(c))
    {
        return Err(error());
    }
    let mut corners = Vec::new();
    if width > 0 {
        corners.reserve_exact((width as usize + 1) * (height as usize + 1));
        for y in 0..=height {
            for x in 0..=width {
                let p = tf.apply(origin.apply(Point3([
                    x as f64 * resolution,
                    y as f64 * resolution,
                    0.,
                ])));
                if !bounded_point(p) {
                    return Err(error());
                }
                corners.push(p);
            }
        }
    }
    Ok(PreparedMap {
        width,
        height,
        corners: corners.into(),
        cells: cells.into(),
    })
}
impl PreparedMap {
    pub fn validate(&self) -> Result<(), CombinedSceneError> {
        if self.width > 256 || self.height > 256 {
            return Err(CombinedSceneError::new(
                "Prepared Map dimensions exceed 256",
            ));
        }
        let expected = if self.width == 0 && self.height == 0 {
            0
        } else {
            (self.width as usize + 1) * (self.height as usize + 1)
        };
        if self.width > 256
            || self.height > 256
            || ((self.width == 0 || self.height == 0) && self.width != self.height)
            || self.cells.len() != self.width as usize * self.height as usize
            || self.corners.len() != expected
            || self.cells.iter().any(|c| !(-1..=100).contains(c))
            || !self.corners.iter().copied().all(bounded_point)
        {
            return Err(CombinedSceneError::new(
                "Prepared Map shape, values or world bounds are invalid",
            ));
        }
        Ok(())
    }
}
/// Fixed opaque palette: unknown blue-grey, free light-grey, occupied charcoal.
pub fn occupancy_color(value: i8) -> [f32; 4] {
    if value < 0 {
        return [0.24, 0.34, 0.46, 1.];
    }
    let v = 0.92 - 0.87 * (value as f32 / 100.);
    [v, v, v, 1.]
}
pub fn map_vertices(
    map: &PreparedMap,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, CombinedSceneError> {
    map.validate()?;
    if size.contains(&0) {
        return Err(CombinedSceneError::new("Map viewport must be nonempty"));
    }
    let aspect = size[0] as f32 / size[1] as f32;
    let projection = camera.projection(aspect);
    // Each lattice corner is shared by neighboring cells and the two triangles
    // within each cell. Project once, retaining perspective w/near-plane values
    // unchanged for GPU clipping. The temporary is at most 66,049 × 16 bytes.
    let mut projected = Vec::with_capacity(map.corners.len());
    for &point in map.corners.iter() {
        let clip = projection.clip(point);
        if !clip.iter().all(|v| v.is_finite()) {
            return Err(CombinedSceneError::new("Map projection is nonfinite"));
        }
        projected.push(clip);
    }
    let mut out = Vec::with_capacity(map.cells.len() * 6);
    for y in 0..map.height as usize {
        for x in 0..map.width as usize {
            let stride = map.width as usize + 1;
            let a = y * stride + x;
            let corners = [a, a + 1, a + stride + 1, a + stride];
            let color = occupancy_color(map.cells[y * map.width as usize + x]);
            for index in [0, 1, 2, 0, 2, 3] {
                let clip = projected[corners[index]];
                out.push(RenderVertex { clip, color });
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn reference_vertices(
        map: &PreparedMap,
        camera: CameraPose,
        size: [u32; 2],
    ) -> Vec<RenderVertex> {
        let mut out = Vec::new();
        for y in 0..map.height as usize {
            for x in 0..map.width as usize {
                let stride = map.width as usize + 1;
                let a = y * stride + x;
                let corners = [a, a + 1, a + stride + 1, a + stride];
                let color = occupancy_color(map.cells[y * map.width as usize + x]);
                for index in [0, 1, 2, 0, 2, 3] {
                    out.push(RenderVertex {
                        clip: camera
                            .clip(map.corners[corners[index]], size[0] as f32 / size[1] as f32),
                        color,
                    });
                }
            }
        }
        out
    }
    #[test]
    fn lattice_projection_matches_per_triangle_reference_with_rotated_tf_and_near_plane_crossing() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let map = prepare_map(
            3,
            2,
            0.5,
            RigidTransform::new([0.2, -0.3, 0.4], [0., 0., q, q]).unwrap(),
            RigidTransform::new([1., 2., 3.], [0., q, 0., q]).unwrap(),
            &[-1, 0, 25, 50, 75, 100],
        )
        .unwrap();
        let cameras = [
            CameraPose::orbit(0.5, 0.8, 12., [0.; 3]),
            CameraPose::orbit(2.1, -0.4, 5., [1., 2., 3.]),
            CameraPose {
                orthographic_span: None,
                calibrated: None,
                eye: map.corners[0].0.map(|v| v as f32),
                right: [1., 0., 0.],
                up: [0., 0., 1.],
                forward: [0., 1., 0.],
            },
        ];
        for (index, camera) in cameras.into_iter().enumerate() {
            for size in [[640, 480], [2048, 512], [1, 1]] {
                let actual = map_vertices(&map, camera, size).unwrap();
                let expected = reference_vertices(&map, camera, size);
                assert_eq!(actual.len(), 36);
                for (actual, expected) in actual.iter().zip(expected.iter()) {
                    assert_eq!(
                        actual.clip.map(f32::to_bits),
                        expected.clip.map(f32::to_bits)
                    );
                    assert_eq!(
                        actual.color.map(f32::to_bits),
                        expected.color.map(f32::to_bits)
                    );
                }
                if index == 2 {
                    assert!(
                        actual.iter().any(|v| v.clip[2] < 0.),
                        "Near-plane side must remain for GPU triangle clipping"
                    );
                    assert!(actual.iter().any(|v| v.clip[2] >= 0. && v.clip[3] > 0.));
                }
            }
        }
    }
    #[test]
    fn map_origin_then_tf_once_and_palette_bounds() {
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let origin = RigidTransform::new([0.2, 0., 0.], [0., 0., q, q]).unwrap();
        let tf = RigidTransform::new([1., 2., 0.3], [0., 0., q, q]).unwrap();
        let map = prepare_map(2, 1, 1., origin, tf, &[-1, 100]).unwrap();
        for (a, b) in map.corners[1].0.into_iter().zip([0., 2.2, 0.3]) {
            assert!((a - b).abs() < 1e-10);
        }
        let vertices =
            map_vertices(&map, CameraPose::orbit(0.5, 0.8, 12., [0.; 3]), [640, 480]).unwrap();
        assert_eq!(vertices.len(), 12);
        assert_ne!(vertices[0].color, vertices[6].color);
        assert_ne!(occupancy_color(-1), occupancy_color(0));
        assert!(prepare_map(257, 1, 1., origin, tf, &[]).is_err());
    }
    #[test]
    fn maximum_map_is_bounded_and_empty_clears() {
        let map = prepare_map(
            256,
            256,
            1.,
            RigidTransform::identity(),
            RigidTransform::identity(),
            &vec![0; 65536],
        )
        .unwrap();
        assert_eq!(map.corners.len(), 66049);
        assert_eq!(map.corners.len(), MAX_MAP_CORNERS);
        assert_eq!(MAX_PROJECTED_CORNER_BYTES, 1_056_784);
        assert_eq!(
            map_vertices(
                &map,
                CameraPose::orbit(0.5, 0.8, 12., [0.; 3]),
                [2048, 2048]
            )
            .unwrap()
            .len(),
            393216
        );
        let empty = prepare_map(
            0,
            0,
            1.,
            RigidTransform::identity(),
            RigidTransform::identity(),
            &[],
        )
        .unwrap();
        assert!(
            map_vertices(&empty, CameraPose::orbit(0.5, 0.8, 12., [0.; 3]), [1, 1])
                .unwrap()
                .is_empty()
        );
    }
}
