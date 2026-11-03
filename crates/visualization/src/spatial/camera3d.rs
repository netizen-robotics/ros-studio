//! Right-handed ROS world coordinates, perspective depth in WebGPU's [0, 1].
use super::{calibrated_camera::RectifiedProjection, Point3, RigidTransform};
pub const NEAR: f32 = 0.05;
pub const FAR: f32 = 10_000.0;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub(crate) orthographic_span: Option<f32>,
    pub(crate) calibrated: Option<CalibratedView>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CalibratedView {
    projection: RectifiedProjection,
    zoom: f64,
    origin: [f64; 3],
    axes: [[f64; 3]; 3], // optical right/down/forward in the fixed frame
}
/// Perspective constants prepared once for an immutable camera/viewport batch.
#[derive(Clone, Copy, Debug)]
pub struct CameraProjection {
    camera: CameraPose,
    aspect: f32,
    focal: f32,
    depth_scale: f32,
    depth_offset: f32,
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
impl CameraPose {
    /// Captured fixed<-optical TF with effective rectified Image intrinsics.
    /// This constructs a separate camera; the saved orbit camera is untouched.
    pub fn calibrated(
        transform: RigidTransform,
        projection: RectifiedProjection,
        zoom: f64,
    ) -> Result<Self, String> {
        if !zoom.is_finite() || !(0.1..=10.).contains(&zoom) {
            return Err("Camera zoom must be between 0.1 and 10".into());
        }
        let origin = transform.apply(Point3([0.; 3])).0;
        let axes = std::array::from_fn::<_, 3, _>(|i| {
            let mut axis = [0.; 3];
            axis[i] = 1.;
            let point = transform.apply(Point3(axis)).0;
            std::array::from_fn(|j| point[j] - origin[j])
        });
        let (projection, stereo) = projection.centred();
        let origin =
            std::array::from_fn(|i| origin[i] + axes[0][i] * stereo[0] + axes[1][i] * stereo[1]);
        if !super::map_geometry::bounded_point(Point3(origin)) {
            return Err("Camera optical centre exceeds fixed-frame bounds".into());
        }
        Ok(Self {
            eye: origin.map(|v| v as f32),
            right: axes[0].map(|v| v as f32),
            up: axes[1].map(|v| -v as f32),
            forward: axes[2].map(|v| v as f32),
            orthographic_span: None,
            calibrated: Some(CalibratedView {
                projection,
                zoom,
                origin,
                axes,
            }),
        })
    }
    pub(crate) fn origin(&self) -> [f64; 3] {
        self.calibrated
            .map_or(self.eye.map(f64::from), |c| c.origin)
    }
    pub(crate) fn calibrated_delta(
        &self,
        delta: [f64; 3],
        w: f64,
        aspect: f64,
    ) -> Option<[f32; 4]> {
        self.calibrated.map(|c| {
            let optical = c
                .axes
                .map(|axis| delta.iter().zip(axis).map(|(a, b)| a * b).sum());
            c.projection
                .homogeneous([optical[0], optical[1], optical[2], w], aspect, c.zoom)
        })
    }
    pub fn projection(&self, aspect: f32) -> CameraProjection {
        CameraProjection {
            camera: *self,
            aspect: if aspect.is_finite() && aspect > 0. {
                aspect
            } else {
                1.
            },
            focal: 1.0 / (std::f32::consts::FRAC_PI_6).tan(),
            depth_scale: FAR / (FAR - NEAR),
            depth_offset: FAR * NEAR / (FAR - NEAR),
        }
    }
    pub fn orbit(azimuth: f32, elevation: f32, distance: f32, target: [f32; 3]) -> Self {
        let finite = |v: f32, default| if v.is_finite() { v } else { default };
        let a = finite(azimuth, 0.5).rem_euclid(std::f32::consts::TAU);
        let e = finite(elevation, 0.8).clamp(-1.5, 1.5);
        let d = finite(distance, 12.0).clamp(0.2, 2000.0);
        let target = target.map(|v| finite(v, 0.0).clamp(-1_000_000.0, 1_000_000.0));
        let (sa, ca) = a.sin_cos();
        let (se, ce) = e.sin_cos();
        let forward = [-ce * ca, -ce * sa, -se];
        Self {
            eye: std::array::from_fn(|i| target[i] - d * forward[i]),
            forward,
            right: [-sa, ca, 0.0],
            up: [-se * ca, -se * sa, ce],
            orthographic_span: None,
            calibrated: None,
        }
    }
    /// A real orthographic projection: world-space sizes do not shrink with depth.
    /// Looking down ROS +Z, +X is right and +Y is up. Keep the target centered
    /// within the existing 10 km depth envelope.
    pub fn top_down(span: f32, target: [f32; 3]) -> Self {
        let finite = |v: f32, default| if v.is_finite() { v } else { default };
        let target = target.map(|v| finite(v, 0.).clamp(-1_000_000., 1_000_000.));
        Self {
            eye: [target[0], target[1], target[2] + FAR * 0.5],
            right: [1., 0., 0.],
            up: [0., 1., 0.],
            forward: [0., 0., -1.],
            orthographic_span: Some(finite(span, 12.).clamp(0.2, 2000.)),
            calibrated: None,
        }
    }
    /// Clip coordinates retain w for GPU perspective division and clipping.
    pub fn clip(&self, point: Point3, aspect: f32) -> [f32; 4] {
        if let Some(clip) = self.calibrated_delta(
            std::array::from_fn(|i| point.0[i] - self.origin()[i]),
            1.,
            if aspect.is_finite() && aspect > 0. {
                f64::from(aspect)
            } else {
                1.
            },
        ) {
            return clip;
        }
        let delta = std::array::from_fn(|i| point.0[i] as f32 - self.eye[i]);
        let z = dot(delta, self.forward);
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let focal = 1.0 / (std::f32::consts::FRAC_PI_6).tan();
        if let Some(span) = self.orthographic_span {
            return [
                dot(delta, self.right) * 2. / span / aspect,
                dot(delta, self.up) * 2. / span,
                (z - NEAR) / (FAR - NEAR),
                1.,
            ];
        }
        [
            dot(delta, self.right) * focal / aspect,
            dot(delta, self.up) * focal,
            FAR / (FAR - NEAR) * z - FAR * NEAR / (FAR - NEAR),
            z,
        ]
    }
    pub fn visible(&self, point: Point3, aspect: f32) -> bool {
        let [x, y, z, w] = self.clip(point, aspect);
        [x, y, z, w].iter().all(|v| v.is_finite())
            && w > 0.0
            && x.abs() <= w
            && y.abs() <= w
            && z >= 0.0
            && z <= w
    }
}
impl CameraProjection {
    /// Keep the exact arithmetic order of CameraPose::clip; do not fold focal /
    /// aspect into a scale, which would change near-plane/pixel rounding.
    pub fn clip(&self, point: Point3) -> [f32; 4] {
        if self.camera.calibrated.is_some() {
            return self.camera.clip(point, self.aspect);
        }
        let delta = std::array::from_fn(|i| point.0[i] as f32 - self.camera.eye[i]);
        let z = dot(delta, self.camera.forward);
        if let Some(span) = self.camera.orthographic_span {
            return [
                dot(delta, self.camera.right) * 2. / span / self.aspect,
                dot(delta, self.camera.up) * 2. / span,
                (z - NEAR) / (FAR - NEAR),
                1.,
            ];
        }
        [
            dot(delta, self.camera.right) * self.focal / self.aspect,
            dot(delta, self.camera.up) * self.focal,
            self.depth_scale * z - self.depth_offset,
            z,
        ]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibrated_world_projection_retains_optical_tf_stereo_letterbox_and_large_origin_precision()
    {
        let p = RectifiedProjection::new(
            [640, 480],
            [500., 0., 290., -50., 0., 450., 210., 22.5, 0., 0., 1., 0.],
            0.05,
            1000.,
        )
        .unwrap();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for transform in [
            RigidTransform::identity(),
            RigidTransform::new([999_900.123, -40., 8.], [0., 0., q, q]).unwrap(),
        ] {
            for zoom in [0.5, 1., 2.] {
                let camera = CameraPose::calibrated(transform, p, zoom).unwrap();
                for size in [[640, 480], [1280, 480], [640, 960]] {
                    let aspect = size[0] as f32 / size[1] as f32;
                    for optical in [[0., 0., 2.], [0.1, 0.2, 2.], [-0.3, -0.1, 10.]] {
                        let reference = p.clip(optical, size, zoom).unwrap();
                        let world = transform.apply(Point3(optical));
                        let actual = camera.clip(world, aspect);
                        let prepared = camera.projection(aspect).clip(world);
                        assert_eq!(actual, prepared);
                        for i in 0..4 {
                            assert!(
                                (actual[i] - reference[i]).abs() < 3e-6,
                                "{actual:?} vs {reference:?}"
                            );
                        }
                    }
                }
            }
        }
        assert!(CameraPose::calibrated(RigidTransform::identity(), p, f64::NAN).is_err());
        let camera = CameraPose::calibrated(RigidTransform::identity(), p, 1.).unwrap();
        assert!(!camera.visible(Point3([0., 0., -1.]), 1.));
        assert!(!camera.visible(Point3([0., 0., 0.01]), 1.));
        assert!(camera
            .clip(Point3([0.; 3]), 1.)
            .iter()
            .all(|v| v.is_finite()));
    }
    #[test]
    fn top_down_keeps_world_size_across_depth_and_respects_aspect_and_clipping() {
        let camera = CameraPose::top_down(10., [2., 3., 4.]);
        for aspect in [0.25, 1., 4.] {
            let near = camera.clip(Point3([3., 4., 5.]), aspect);
            let far = camera.clip(Point3([3., 4., -5.]), aspect);
            assert_eq!(near[..2], far[..2]);
            assert_eq!(near[3], 1.);
            assert!(near[2] < far[2]);
            assert!((near[0] - 0.2 / aspect).abs() < 1e-6);
            assert!((near[1] - 0.2).abs() < 1e-6);
            assert_eq!(
                near.map(f32::to_bits),
                camera
                    .projection(aspect)
                    .clip(Point3([3., 4., 5.]))
                    .map(f32::to_bits)
            );
        }
        assert!(!camera.visible(Point3([2., 3., 6000.]), 1.));
        assert!(!camera.visible(Point3([2., 3., -6000.]), 1.));
        assert!(!camera.visible(Point3([100., 3., 4.]), 1.));
    }
    #[test]
    fn prepared_projection_preserves_reference_bits_and_aspect_fallback() {
        let camera = CameraPose::orbit(0.5, 0.8, 12., [1., -2., 0.3]);
        for aspect in [0.25, 1., 4., 0., -1., f32::NAN] {
            let projection = camera.projection(aspect);
            for point in [
                Point3([0.; 3]),
                Point3([1., 2., 3.]),
                Point3([-1_000_000., 1_000_000., -1_000_000.]),
                Point3(camera.eye.map(f64::from)),
            ] {
                assert_eq!(
                    projection.clip(point).map(f32::to_bits),
                    camera.clip(point, aspect).map(f32::to_bits)
                );
            }
        }
    }
    #[test]
    fn perspective_and_clipping_are_real_depth() {
        let camera = CameraPose::orbit(0.0, 0.0, 10.0, [0.0; 3]);
        let near = camera.clip(Point3([5.0, 1.0, 0.0]), 1.0);
        let far = camera.clip(Point3([0.0, 1.0, 0.0]), 1.0);
        assert!(near[0] / near[3] > far[0] / far[3]);
        assert!(near[2] / near[3] < far[2] / far[3]);
        assert!(!camera.visible(Point3([10.0, 0.0, 0.0]), 1.0));
        assert!(!camera.visible(Point3([-20_000.0, 0.0, 0.0]), 1.0));
    }
}
