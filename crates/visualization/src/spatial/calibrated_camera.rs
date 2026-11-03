//! Rectified optical-coordinate projection shared by calibrated scene consumers.
//! Inputs are effective (ROI/binning already applied) P intrinsics. This module
//! neither rectifies raw pixels nor grants authority to draw a live camera frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectifiedProjection {
    width: u32,
    height: u32,
    fx: f64,
    fy: f64,
    cx: f64,
    cy: f64,
    tx: f64,
    ty: f64,
    near: f64,
    far: f64,
}
impl RectifiedProjection {
    /// Move stereo translation into the virtual optical centre. All scene
    /// paths, including analytic Marker impostors, then share the same rays.
    pub(crate) fn centred(self) -> (Self, [f64; 2]) {
        let offset = [-self.tx / self.fx, -self.ty / self.fy];
        (
            Self {
                tx: 0.,
                ty: 0.,
                ..self
            },
            offset,
        )
    }

    /// Homogeneous optical point/vector projection, retaining GPU clipping at
    /// and behind the near plane. w=0 is a direction; translation applies only
    /// to points. No division by optical Z is needed here.
    pub(crate) fn homogeneous(&self, [x, y, z, w]: [f64; 4], aspect: f64, zoom: f64) -> [f32; 4] {
        let image_aspect = f64::from(self.width) / f64::from(self.height);
        let [sx, sy] = if image_aspect > aspect {
            [1., aspect / image_aspect]
        } else {
            [image_aspect / aspect, 1.]
        };
        [
            (2. * (self.fx * x + self.tx * w + self.cx * z) / f64::from(self.width) - z)
                * sx
                * zoom,
            (z - 2. * (self.fy * y + self.ty * w + self.cy * z) / f64::from(self.height))
                * sy
                * zoom,
            self.far / (self.far - self.near) * z
                - self.far * self.near / (self.far - self.near) * w,
            z,
        ]
        .map(|v| v as f32)
    }
    /// Standard row-major CameraInfo P, in effective image pixels.
    pub fn new(size: [u32; 2], p: [f64; 12], near: f64, far: f64) -> Result<Self, String> {
        if size.iter().any(|&n| n == 0 || n > 32768)
            || p.iter().any(|n| !n.is_finite() || n.abs() > 1e12)
            || p[0] <= 0.
            || p[5] <= 0.
            || [p[1], p[4], p[8], p[9], p[11]].iter().any(|&n| n != 0.)
            || p[10] != 1.
            || !near.is_finite()
            || !far.is_finite()
            || near < 0.001
            || far <= near
            || far > 10000.
        {
            return Err("Invalid rectified dimensions, canonical P or depth range".into());
        }
        Ok(Self {
            width: size[0],
            height: size[1],
            fx: p[0],
            fy: p[5],
            cx: p[2],
            cy: p[6],
            tx: p[3],
            ty: p[7],
            near,
            far,
        })
    }
    /// Optical coordinates: +X right, +Y down, +Z forward. Coordinates include
    /// P's stereo translation; callers must not also translate the optical pose.
    pub fn pixel(&self, optical: [f64; 3]) -> Option<[f64; 2]> {
        if optical.iter().any(|n| !n.is_finite() || n.abs() > 1e6) || optical[2] <= 0. {
            return None;
        }
        let [x, y, z] = optical;
        let pixel = [
            (self.fx * x + self.tx) / z + self.cx,
            (self.fy * y + self.ty) / z + self.cy,
        ];
        pixel.iter().all(|n| n.is_finite()).then_some(pixel)
    }
    /// WebGPU homogeneous clip coordinates with [0,1] depth. The same aspect-fit
    /// factors must position the background image; zoom 1 shows its full extent.
    pub fn clip(&self, optical: [f64; 3], viewport: [u32; 2], zoom: f64) -> Option<[f32; 4]> {
        let pixel = self.pixel(optical)?;
        if viewport.contains(&0) || !zoom.is_finite() || !(0.1..=10.).contains(&zoom) {
            return None;
        }
        let [sx, sy] = self.image_scale(viewport)?;
        let z = optical[2];
        let clip = [
            (2. * pixel[0] / f64::from(self.width) - 1.) * z * sx * zoom,
            (1. - 2. * pixel[1] / f64::from(self.height)) * z * sy * zoom,
            self.far / (self.far - self.near) * z - self.far * self.near / (self.far - self.near),
            z,
        ]
        .map(|v| v as f32);
        clip.iter().all(|n| n.is_finite()).then_some(clip)
    }
    pub fn image_scale(&self, viewport: [u32; 2]) -> Option<[f64; 2]> {
        if viewport.contains(&0) {
            return None;
        }
        let image_aspect = f64::from(self.width) / f64::from(self.height);
        let view_aspect = f64::from(viewport[0]) / f64::from(viewport[1]);
        Some(if image_aspect > view_aspect {
            [1., view_aspect / image_aspect]
        } else {
            [image_aspect / view_aspect, 1.]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection(tx: f64) -> RectifiedProjection {
        RectifiedProjection::new(
            [640, 480],
            [500., 0., 300., tx, 0., 450., 200., 0., 0., 0., 1., 0.],
            0.01,
            100.,
        )
        .unwrap()
    }
    #[test]
    fn off_center_intrinsics_optical_axes_and_stereo_translation_are_not_symmetric_orbit() {
        let p = projection(-50.);
        assert_eq!(p.pixel([0., 0., 2.]), Some([275., 200.]));
        assert_eq!(p.pixel([0.1, 0.2, 2.]), Some([300., 245.]));
        let clip = p.clip([0.1, 0.2, 2.], [640, 480], 1.).unwrap();
        assert!((clip[0] / clip[3] - (2. * 300. / 640. - 1.)).abs() < 1e-6);
        assert!((clip[1] / clip[3] - (1. - 2. * 245. / 480.)).abs() < 1e-6);
        assert!(p.clip([0., 1., 2.], [640, 480], 1.).unwrap()[1] < 0.);
        // Stereo offset equals shifting the virtual optical centre by baseline.
        assert_eq!(p.pixel([0.5, 0., 2.]), projection(0.).pixel([0.4, 0., 2.]));
    }
    #[test]
    fn image_and_geometry_share_letterboxing_and_webgpu_depth() {
        let p = projection(0.);
        assert_eq!(p.image_scale([1280, 480]), Some([0.5, 1.]));
        assert_eq!(p.image_scale([640, 960]), Some([1., 0.5]));
        let near = p.clip([0., 0., 0.01], [640, 480], 1.).unwrap();
        let far = p.clip([0., 0., 100.], [640, 480], 1.).unwrap();
        assert!((near[2] / near[3]).abs() < 1e-6);
        assert!((far[2] / far[3] - 1.).abs() < 1e-6);
        assert!(p.pixel([0., 0., 0.]).is_none());
        assert!(p.pixel([0., 0., -1.]).is_none());
        assert!(p.clip([0., 0., 1.], [0, 480], 1.).is_none());
    }
    #[test]
    fn malformed_calibration_and_unbounded_projection_are_rejected() {
        let good = [500., 0., 300., 0., 0., 450., 200., 0., 0., 0., 1., 0.];
        for (index, value) in [
            (0, 0.),
            (5, -1.),
            (2, f64::NAN),
            (8, 1.),
            (10, 0.),
            (11, 1.),
        ] {
            let mut bad = good;
            bad[index] = value;
            assert!(RectifiedProjection::new([640, 480], bad, 0.01, 100.).is_err());
        }
        for size in [[0, 480], [640, 32769]] {
            assert!(RectifiedProjection::new(size, good, 0.01, 100.).is_err());
        }
        assert!(RectifiedProjection::new([640, 480], good, 100., 100.).is_err());
        assert!(projection(0.).pixel([f64::INFINITY, 0., 1.]).is_none());
        assert!(projection(0.).clip([0., 0., 1.], [640, 480], 0.).is_none());
    }
}
