//! Complete bounded PointCloud2 extraction and acquisition-time geometry.
//! Native introspection stays in the backend; this module is pure and testable.
use crate::{image::HeaderStamp, spatial::TfOutcome};
use serde::{Deserialize, Serialize};
#[path = "cloud_attributes.rs"]
mod attributes;
#[path = "cloud_points.rs"]
mod packed_xyz;
pub use attributes::{Channel, Channels};

pub const CONTRACT_VERSION: u32 = 2;
pub const MESSAGE_TYPE: &str = "sensor_msgs/msg/PointCloud2";
pub const MAX_SOURCE_POINTS: usize = 8192;
pub const MAX_RAW_BYTES: usize = 256 * 1024;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_FIELDS: usize = 32;
pub const MAX_FIELD_NAME_BYTES: usize = 64;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_POINT_STEP: u32 = 256;
pub const MAX_COORDINATE: f64 = 1_000_000.;
pub const MAX_STAMP_NS: i64 = i32::MAX as i64 * 1_000_000_000 + 999_999_999;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub datatype: u8,
    pub count: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub header_stamp: HeaderStamp,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub fields: Vec<Field>,
    pub is_bigendian: bool,
    pub point_step: u32,
    pub row_step: u32,
    pub is_dense: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedCloud {
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub source_points: usize,
    pub skipped_points: usize,
    pub points: Vec<[f32; 3]>,
    pub channels: Channels,
}
fn field_width(datatype: u8) -> Option<usize> {
    match datatype {
        1 | 2 => Some(1),
        3 | 4 => Some(2),
        5..=7 => Some(4),
        8 => Some(8),
        _ => None,
    }
}
impl Metadata {
    pub fn stamp_ns(&self) -> Result<i64, String> {
        if self.header_stamp.seconds < 0 || self.header_stamp.nanoseconds >= 1_000_000_000 {
            return Err("PointCloud2 requires a positive System-clock header stamp".into());
        }
        let stamp = i64::from(self.header_stamp.seconds) * 1_000_000_000
            + i64::from(self.header_stamp.nanoseconds);
        (stamp > 0)
            .then_some(stamp)
            .ok_or_else(|| "PointCloud2 requires a positive System-clock header stamp".into())
    }
    /// Validate all lengths and field ranges before allocating or reading bytes.
    pub fn raw_length(&self) -> Result<usize, String> {
        self.stamp_ns()?;
        if !crate::spatial::valid_frame(&self.frame_id) {
            return Err(
                "PointCloud2 header.frame_id is invalid; publish a valid TF frame name".into(),
            );
        }
        if self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
            || self.point_step == 0
            || self.point_step > MAX_POINT_STEP
            || self.fields.is_empty()
            || self.fields.len() > MAX_FIELDS
        {
            return Err(
                "Invalid PointCloud2 dimensions, fields or point stride; inspect the source layout"
                    .into(),
            );
        }
        let points = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or("PointCloud2 point count overflow")?;
        if points > MAX_SOURCE_POINTS {
            return Err(format!("PointCloud2 has {points} source points; the limit is {MAX_SOURCE_POINTS}. Publish a smaller or downsampled cloud"));
        }
        if u64::from(self.row_step) < u64::from(self.width) * u64::from(self.point_step) {
            return Err("PointCloud2 row_step is shorter than width × point_step".into());
        }
        let bytes = (self.row_step as usize)
            .checked_mul(self.height as usize)
            .ok_or("PointCloud2 byte count overflow")?;
        if bytes > MAX_RAW_BYTES {
            return Err(format!("PointCloud2 requires {bytes} raw bytes; the limit is {MAX_RAW_BYTES}. Publish a smaller or downsampled cloud"));
        }
        if points == 0 && bytes != 0 {
            return Err("PointCloud2 empty dimensions require an empty data buffer".into());
        }
        let mut ranges = Vec::with_capacity(self.fields.len());
        let mut names = std::collections::HashSet::new();
        for field in &self.fields {
            if field.name.is_empty()
                || field.name.len() > MAX_FIELD_NAME_BYTES
                || field.name.chars().any(char::is_control)
                || !names.insert(field.name.as_str())
                || field.count == 0
            {
                return Err("Invalid or duplicate PointCloud2 field name/count".into());
            }
            let width =
                field_width(field.datatype).ok_or("Unsupported PointCloud2 field datatype")?;
            let end = width
                .checked_mul(field.count as usize)
                .and_then(|length| (field.offset as usize).checked_add(length))
                .ok_or("PointCloud2 field range overflow")?;
            let start = field.offset as usize;
            if end > self.point_step as usize || ranges.iter().any(|&(a, b)| start < b && a < end) {
                return Err("PointCloud2 field ranges overlap or exceed the point stride".into());
            }
            ranges.push((start, end));
        }
        for name in ["x", "y", "z"] {
            let field = self
                .fields
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| format!("PointCloud2 is missing {name}"))?;
            if field.count != 1 || !matches!(field.datatype, 7 | 8) {
                return Err(format!("PointCloud2 {name} must be one FLOAT32 or FLOAT64"));
            }
        }
        Ok(bytes)
    }
    pub fn decode(&self, raw: &[u8]) -> Result<DecodedCloud, String> {
        if raw.len() != self.raw_length()? {
            return Err("PointCloud2 data length must exactly equal row_step × height".into());
        }
        let xyz = ["x", "y", "z"].map(|name| {
            self.fields
                .iter()
                .find(|f| f.name == name)
                .expect("validated XYZ")
        });
        let count = self.width as usize * self.height as usize;
        let mut points = Vec::with_capacity(count);
        let plan = attributes::Plan::new(self);
        let mut channels = plan.channels();
        let mut skipped_points = 0;
        for row in 0..self.height as usize {
            for col in 0..self.width as usize {
                let start = row * self.row_step as usize + col * self.point_step as usize;
                let point = xyz.map(|field| {
                    let offset = start + field.offset as usize;
                    if field.datatype == 7 {
                        let bytes: [u8; 4] = raw[offset..offset + 4].try_into().unwrap();
                        f64::from(if self.is_bigendian {
                            f32::from_be_bytes(bytes)
                        } else {
                            f32::from_le_bytes(bytes)
                        })
                    } else {
                        let bytes: [u8; 8] = raw[offset..offset + 8].try_into().unwrap();
                        if self.is_bigendian {
                            f64::from_be_bytes(bytes)
                        } else {
                            f64::from_le_bytes(bytes)
                        }
                    }
                });
                if point
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
                {
                    skipped_points += 1;
                } else {
                    points.push(point.map(|v| v as f32));
                    plan.push(&mut channels, &raw[start..], self.is_bigendian);
                }
            }
        }
        Ok(DecodedCloud {
            source_frame: self.frame_id.clone(),
            acquisition_stamp_ns: self.stamp_ns()?,
            source_points: count,
            skipped_points,
            points,
            channels,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub clouds_received: u64,
    pub pending_replaced: u64,
    pub lookup_timeouts: u64,
    pub tf_dynamic_batches: u64,
    pub tf_static_batches: u64,
    pub tf_accepted: u64,
    pub tf_rejected: u64,
    pub duplicate_statics: u64,
    pub native_errors: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub contract_version: u32,
    pub topic: String,
    pub message_type: String,
    pub fixed_frame: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub source_points: usize,
    pub skipped_points: usize,
    #[serde(with = "packed_xyz")]
    pub points: Vec<[f32; 3]>,
    pub channels: Channels,
    pub tf: TfOutcome,
    pub counters: Counters,
}
impl Sample {
    pub fn validate(&self) -> Result<(), String> {
        self.channels.validate(self.points.len())?;
        if self.contract_version != CONTRACT_VERSION
            || self.message_type != MESSAGE_TYPE
            || self.topic.is_empty()
            || self.topic.len() > 256
            || !self.topic.starts_with('/')
            || self
                .topic
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || !crate::spatial::valid_frame(&self.fixed_frame)
            || !crate::spatial::valid_frame(&self.source_frame)
            || self.acquisition_stamp_ns <= 0
            || self.acquisition_stamp_ns > MAX_STAMP_NS
            || self.source_points > MAX_SOURCE_POINTS
            || self.skipped_points > self.source_points
            || self.points.len() != self.source_points - self.skipped_points
            || self
                .points
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || f64::from(v.abs()) > MAX_COORDINATE)
        {
            return Err(
                "Invalid bounded PointCloud2 sample identity, counts or coordinates".into(),
            );
        }
        if let TfOutcome::Ready { components } = &self.tf {
            let norm = components[3..].iter().map(|v| v * v).sum::<f64>();
            if components.iter().any(|v| !v.is_finite())
                || components[..3].iter().any(|v| v.abs() > MAX_COORDINATE)
                || (norm - 1.).abs() > 1e-6
            {
                return Err("Invalid PointCloud2 acquisition-time rigid transform".into());
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
}
