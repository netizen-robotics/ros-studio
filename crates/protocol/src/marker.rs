//! Bounded Marker/MarkerArray semantics shared by observation and presentation.
//! This pure state contract is carried by negotiated session14 marker_snapshot1.
//! Native extraction, TF lookup, asset I/O and rendering belong to owning workers.
pub use crate::latest_pose::LookupTime;
use crate::{latest_pose::LatestPose, mapping, spatial};
use serde::{Deserialize, Serialize};

pub mod store;
pub const CONTRACT_VERSION: u32 = 1;
pub const MESSAGE_TYPE: &str = "visualization_msgs/msg/Marker";
pub const ARRAY_MESSAGE_TYPE: &str = "visualization_msgs/msg/MarkerArray";
pub const MAX_MARKERS: usize = 256;
pub const MAX_OPERATIONS: usize = 256;
pub const MAX_POINTS: usize = 4096;
pub const MAX_BATCH_POINTS: usize = 16384;
pub const MAX_RETAINED_POINTS: usize = 65536;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_RETAINED_TEXT_BYTES: usize = 65536;
pub const MAX_RESOURCE_BYTES: usize = 4096;
pub const MAX_RETAINED_RESOURCE_BYTES: usize = 65536;
pub const MAX_NAMESPACE_BYTES: usize = 256;
pub const MAX_DIAGNOSTICS: usize = 64;
pub const MAX_BATCH_BYTES: usize = 512 * 1024;
pub const MAX_LIFETIME_NS: u64 = i32::MAX as u64 * 1_000_000_000 + 999_999_999;
pub const RETRY_NS: u64 = 50_000_000;
pub const LOOKUP_TIMEOUT_NS: u64 = 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub namespace: String,
    pub id: i32,
}
fn valid_namespace(namespace: &str) -> bool {
    namespace.len() <= MAX_NAMESPACE_BYTES && !namespace.chars().any(char::is_control)
}
impl Key {
    pub fn validate(&self) -> Result<(), Error> {
        valid_namespace(&self.namespace)
            .then_some(())
            .ok_or(Error::InvalidKey)
    }
}

/// All installed Jazzy kinds, including ArrowStrip. Irrelevant ROS fields are
/// omitted by canonical extraction. Extended embedded/remote assets are rejected
/// explicitly upstream; these local references grant no filesystem permission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Geometry {
    Arrow {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
    },
    Cube,
    Sphere,
    Cylinder,
    LineStrip {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    LineList {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    CubeList {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    SphereList {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    Points {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    Text {
        text: String,
    },
    Mesh {
        resource: String,
        use_embedded_materials: bool,
    },
    Triangles {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
    ArrowStrip {
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        points: Vec<[f64; 3]>,
        #[serde(deserialize_with = "crate::serde_bounded::points")]
        colors: Vec<[f32; 4]>,
    },
}
fn valid_color(color: &[f32; 4]) -> bool {
    color
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
impl Geometry {
    pub fn points(&self) -> &[[f64; 3]] {
        match self {
            Self::Arrow { points }
            | Self::LineStrip { points, .. }
            | Self::LineList { points, .. }
            | Self::CubeList { points, .. }
            | Self::SphereList { points, .. }
            | Self::Points { points, .. }
            | Self::Triangles { points, .. }
            | Self::ArrowStrip { points, .. } => points,
            _ => &[],
        }
    }
    pub fn colors(&self) -> &[[f32; 4]] {
        match self {
            Self::LineStrip { colors, .. }
            | Self::LineList { colors, .. }
            | Self::CubeList { colors, .. }
            | Self::SphereList { colors, .. }
            | Self::Points { colors, .. }
            | Self::Triangles { colors, .. }
            | Self::ArrowStrip { colors, .. } => colors,
            _ => &[],
        }
    }
    pub fn text_bytes(&self) -> usize {
        if let Self::Text { text } = self {
            text.len()
        } else {
            0
        }
    }
    pub fn resource_bytes(&self) -> usize {
        if let Self::Mesh { resource, .. } = self {
            resource.len()
        } else {
            0
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        let points = self.points();
        let colors = self.colors();
        if points.len() > MAX_POINTS || colors.len() > MAX_POINTS {
            return Err(Error::PointLimit);
        }
        if points
            .iter()
            .flatten()
            .any(|x| !x.is_finite() || x.abs() > mapping::MAX_COORDINATE)
            || colors.iter().any(|color| !valid_color(color))
            || (!colors.is_empty() && colors.len() != points.len())
        {
            return Err(Error::InvalidGeometry);
        }
        let count_ok = match self {
            Self::Arrow { .. } => points.is_empty() || points.len() == 2,
            Self::LineStrip { .. } | Self::ArrowStrip { .. } => points.len() != 1,
            Self::LineList { .. } => points.len().is_multiple_of(2),
            Self::Triangles { .. } => points.len().is_multiple_of(3),
            _ => true,
        };
        if !count_ok {
            return Err(Error::InvalidGeometry);
        }
        if let Self::Text { text } = self {
            if text.len() > MAX_TEXT_BYTES {
                return Err(Error::TextLimit);
            }
            if text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            {
                return Err(Error::InvalidGeometry);
            }
        }
        if let Self::Mesh { resource, .. } = self {
            if resource.len() > MAX_RESOURCE_BYTES {
                return Err(Error::ResourceLimit);
            }
            if resource.is_empty() || resource.chars().any(char::is_control) {
                return Err(Error::InvalidGeometry);
            }
            if resource.contains("://")
                && !resource.starts_with("file://")
                && !resource.starts_with("package://")
            {
                return Err(Error::UnsupportedResource);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub key: Key,
    pub frame_id: String,
    /// Zero explicitly requests latest-available TF, not acquisition alignment.
    pub stamp_ns: i64,
    pub pose: [f64; 7],
    pub scale: [f64; 3],
    pub color: [f32; 4],
    /// Zero persists. Positive durations use monotonic receipt time.
    pub lifetime_ns: u64,
    pub frame_locked: bool,
    pub geometry: Geometry,
}
impl Marker {
    pub fn validate(&self) -> Result<(), Error> {
        self.key.validate()?;
        if !spatial::valid_frame(&self.frame_id)
            || !(0..=mapping::MAX_STAMP_NS).contains(&self.stamp_ns)
            || !mapping::valid_pose(&self.pose)
            || self
                .scale
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=mapping::MAX_COORDINATE).contains(v))
            || !valid_color(&self.color)
            || self.lifetime_ns > MAX_LIFETIME_NS
        {
            return Err(Error::InvalidMarker);
        }
        self.geometry.validate()
    }
    pub fn lookup_time(&self) -> LookupTime {
        if self.frame_locked || self.stamp_ns == 0 {
            LookupTime::Latest
        } else {
            LookupTime::At(self.stamp_ns)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InvalidKey,
    InvalidMarker,
    InvalidGeometry,
    UnsupportedKind,
    UnsupportedResource,
    PointLimit,
    TextLimit,
    ResourceLimit,
    EntryLimit,
    BatchLimit,
    InvalidTransform,
    ClockRegressed,
    CounterExhausted,
    InvalidOwner,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Add {
        marker: Box<Marker>,
    },
    Delete {
        key: Key,
    },
    /// Empty namespace clears this owning display, never another display.
    DeleteAll {
        namespace: String,
    },
    /// A malformed/unsupported native ADD removes stale geometry at its key.
    Rejected {
        key: Key,
        reason: Error,
    },
}
impl Operation {
    pub fn key(&self) -> Option<&Key> {
        match self {
            Self::Add { marker } => Some(&marker.key),
            Self::Delete { key } | Self::Rejected { key, .. } => Some(key),
            Self::DeleteAll { .. } => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub contract_version: u32,
    #[serde(deserialize_with = "crate::serde_bounded::operations")]
    pub operations: Vec<Operation>,
}
impl Batch {
    pub fn validate(&self) -> Result<(), Error> {
        if self.contract_version != CONTRACT_VERSION || self.operations.len() > MAX_OPERATIONS {
            return Err(Error::BatchLimit);
        }
        let mut points = 0;
        for op in &self.operations {
            match op {
                Operation::Add { marker } => {
                    marker.validate()?;
                    points += marker.geometry.points().len();
                }
                Operation::DeleteAll { namespace } => {
                    if !valid_namespace(namespace) {
                        return Err(Error::InvalidKey);
                    }
                }
                Operation::Delete { key } | Operation::Rejected { key, .. } => key.validate()?,
            }
        }
        if points > MAX_BATCH_POINTS {
            return Err(Error::PointLimit);
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate().map_err(|e| e.to_string())?;
        crate::session::encode_line(self, MAX_BATCH_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err("Marker batch exceeds512KiB".into());
        }
        let batch: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        batch.validate().map_err(|e| e.to_string())?;
        Ok(batch)
    }
}

/// Validation shared by the worker-owned state and future snapshot receiver.
pub fn validate_transform(time: LookupTime, transform: &LatestPose) -> Result<(), Error> {
    time.validate_result(transform)
        .map_err(|_| Error::InvalidTransform)
}
