//! One precise projection convention for every Marker geometry path.
use super::*;
pub(super) struct Projection {
    pub camera: CameraPose,
    pub size: [u32; 2],
}
impl Projection {
    pub fn new(camera: CameraPose, size: [u32; 2]) -> Result<Self, String> {
        if size.contains(&0)
            || [camera.eye, camera.right, camera.up, camera.forward]
                .iter()
                .flatten()
                .any(|v| !v.is_finite())
            || camera
                .orthographic_span
                .is_some_and(|s| !s.is_finite() || s <= 0.)
        {
            return Err("Invalid Marker camera/viewport".into());
        }
        let basis = [camera.right, camera.up, camera.forward];
        for (i, a) in basis.iter().enumerate() {
            if (a.iter().map(|v| v * v).sum::<f32>() - 1.).abs() > 1e-4
                || basis[..i]
                    .iter()
                    .any(|b| a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>().abs() > 1e-4)
            {
                return Err("Invalid Marker camera basis".into());
            }
        }
        Ok(Self {
            camera,
            size: crate::spatial::gpu::bounded_dimensions(size),
        })
    }
    pub fn delta(&self, p: [f64; 3], w: f64) -> Result<[f32; 4], String> {
        if let Some(clip) =
            self.camera
                .calibrated_delta(p, w, self.size[0] as f64 / self.size[1] as f64)
        {
            return if clip.iter().all(|v| v.is_finite()) {
                Ok(clip)
            } else {
                Err("Marker calibrated projection exceeds GPU precision".into())
            };
        }
        let dot = |b: [f32; 3]| p.iter().zip(b).map(|(a, b)| a * b as f64).sum::<f64>();
        let (x, y, z) = (
            dot(self.camera.right),
            dot(self.camera.up),
            dot(self.camera.forward),
        );
        let aspect = self.size[0] as f64 / self.size[1] as f64;
        let near = crate::spatial::camera3d::NEAR as f64;
        let far = crate::spatial::camera3d::FAR as f64;
        let clip = if let Some(span) = self.camera.orthographic_span {
            [
                x * 2. / span as f64 / aspect,
                y * 2. / span as f64,
                (z - near * w) / (far - near),
                w,
            ]
        } else {
            let focal = 1. / std::f64::consts::FRAC_PI_6.tan();
            [
                x * focal / aspect,
                y * focal,
                far / (far - near) * z - far * near / (far - near) * w,
                z,
            ]
        }
        .map(|v| v as f32);
        if clip.iter().any(|v| !v.is_finite()) {
            return Err("Marker projection exceeds GPU precision".into());
        }
        Ok(clip)
    }
    pub fn point(&self, p: Point3) -> Result<[f32; 4], String> {
        if !crate::spatial::map_geometry::bounded_point(p) {
            return Err("Marker point bounds".into());
        }
        self.delta(
            std::array::from_fn(|i| p.0[i] - self.camera.origin()[i]),
            1.,
        )
    }
}
