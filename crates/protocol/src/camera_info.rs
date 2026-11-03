//! Bounded CameraInfo source metadata. This module does not negotiate or enable
//! a Camera display. Projection assumes already rectified image pixels.
use serde::{Deserialize, Serialize};

pub const MESSAGE_TYPE: &str = "sensor_msgs/msg/CameraInfo";
pub const MAX_ENCODED_BYTES: usize = 8 * 1024;
pub const MAX_SENSOR_DIMENSION: u32 = 32768;
pub const MAX_DISTORTION: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionOfInterest {
    pub x_offset: u32,
    pub y_offset: u32,
    pub height: u32,
    pub width: u32,
    pub do_rectify: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub stamp_ns: i64,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub distortion_model: String,
    #[serde(deserialize_with = "crate::serde_bounded::distortion")]
    pub d: Vec<f64>,
    pub k: [f64; 9],
    pub r: [f64; 9],
    pub p: [f64; 12],
    pub binning_x: u32,
    pub binning_y: u32,
    pub roi: RegionOfInterest,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectiveCalibration {
    pub size: [u32; 2],
    /// Row-major rectified projection, including stereo translation exactly once.
    pub p: [f64; 12],
}

impl Calibration {
    /// Repeated calibration stamps do not change the optical model identity.
    pub fn same_parameters(&self, other: &Self) -> bool {
        let mut value = self.clone();
        value.stamp_ns = other.stamp_ns;
        value == *other
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(0..=crate::latest_pose::MAX_STAMP_NS).contains(&self.stamp_ns)
            || !crate::spatial::valid_frame(&self.frame_id)
            || [self.width, self.height]
                .iter()
                .any(|&n| n == 0 || n > MAX_SENSOR_DIMENSION)
            || self.binning_x > MAX_SENSOR_DIMENSION
            || self.binning_y > MAX_SENSOR_DIMENSION
            || self.distortion_model.len() > 64
            || self.distortion_model.chars().any(char::is_control)
            || self.d.len() > MAX_DISTORTION
            || self
                .k
                .iter()
                .chain(&self.r)
                .chain(&self.p)
                .chain(&self.d)
                .any(|n| !n.is_finite() || n.abs() > 1e12)
        {
            return Err("Invalid bounded CameraInfo metadata".into());
        }
        self.region()?;
        // ROS explicitly represents an uncalibrated camera by K[0] == 0.
        // It is source state, not permission to render a calibrated view.
        if self.k[0] == 0. {
            return Ok(());
        }
        if self.k[0] < 0.
            || self.k[4] <= 0.
            || [self.k[3], self.k[6], self.k[7]].iter().any(|&n| n != 0.)
            || self.k[8] != 1.
            || self.p[0] <= 0.
            || self.p[5] <= 0.
            || [self.p[1], self.p[4], self.p[8], self.p[9], self.p[11]]
                .iter()
                .any(|&n| n != 0.)
            || self.p[10] != 1.
            || !rotation(&self.r)
        {
            return Err("Malformed initialized CameraInfo K/R/P".into());
        }
        Ok(())
    }

    fn region(&self) -> Result<[u32; 4], String> {
        let q = self.roi;
        if [q.x_offset, q.y_offset, q.width, q.height] == [0; 4] {
            return Ok([0, 0, self.width, self.height]);
        }
        if q.width == 0
            || q.height == 0
            || q.x_offset
                .checked_add(q.width)
                .is_none_or(|n| n > self.width)
            || q.y_offset
                .checked_add(q.height)
                .is_none_or(|n| n > self.height)
        {
            return Err("CameraInfo ROI is outside the full sensor dimensions".into());
        }
        Ok([q.x_offset, q.y_offset, q.width, q.height])
    }

    pub fn effective(&self) -> Result<EffectiveCalibration, String> {
        self.validate()?;
        if self.k[0] == 0. {
            return Err("CameraInfo is uncalibrated (K[0] is zero)".into());
        }
        let [x, y, width, height] = self.region()?;
        let cropped = [x, y, width, height] != [0, 0, self.width, self.height];
        if cropped
            && self.roi.do_rectify
            && (self.d.iter().any(|&n| n != 0.) || self.r != identity())
        {
            return Err(
                "Rectified ROI calculation is unsupported; provide effective rectified calibration"
                    .into(),
            );
        }
        let bx = self.binning_x.max(1);
        let by = self.binning_y.max(1);
        let size = [width / bx, height / by];
        if size.contains(&0) {
            return Err("CameraInfo binning produces an empty image".into());
        }
        let mut p = self.p;
        p[2] -= f64::from(x);
        p[6] -= f64::from(y);
        for i in [0, 2, 3] {
            p[i] /= f64::from(bx);
        }
        for i in [5, 6, 7] {
            p[i] /= f64::from(by);
        }
        Ok(EffectiveCalibration { size, p })
    }

    pub fn matches_image(
        &self,
        image: &crate::image::ImageMetadata,
    ) -> Result<EffectiveCalibration, String> {
        image.raw_length()?;
        let effective = self.effective()?;
        if image.frame_id != self.frame_id || effective.size != [image.width, image.height] {
            return Err("Image and CameraInfo optical frame or effective dimensions differ".into());
        }
        Ok(effective)
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err("CameraInfo exceeds 8KiB".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err("CameraInfo exceeds 8KiB".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
}

fn identity() -> [f64; 9] {
    [1., 0., 0., 0., 1., 0., 0., 0., 1.]
}
fn rotation(r: &[f64; 9]) -> bool {
    for i in 0..3 {
        for j in 0..3 {
            let dot: f64 = (0..3).map(|k| r[i * 3 + k] * r[j * 3 + k]).sum();
            if (dot - if i == j { 1. } else { 0. }).abs() > 1e-6 {
                return false;
            }
        }
    }
    let det = r[0] * (r[4] * r[8] - r[5] * r[7]) - r[1] * (r[3] * r[8] - r[5] * r[6])
        + r[2] * (r[3] * r[7] - r[4] * r[6]);
    (det - 1.).abs() <= 1e-6
}

#[cfg(test)]
mod tests;
