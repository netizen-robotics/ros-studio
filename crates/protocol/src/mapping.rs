//! Bounded read-only map, pose and path geometry; native introspection stays upstream.
use crate::spatial::TfOutcome;
use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: u32 = 1;
pub const MAP_MESSAGE_TYPE: &str = "nav_msgs/msg/OccupancyGrid";
pub const POSE_MESSAGE_TYPE: &str = "geometry_msgs/msg/PoseStamped";
pub const PATH_MESSAGE_TYPE: &str = "nav_msgs/msg/Path";
pub const MAX_MAP_DIMENSION: u32 = 256;
pub const MAX_MAP_CELLS: usize = 65536;
pub const MAX_PATH_POSES: usize = 1024;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_COORDINATE: f64 = 1_000_000.;
pub const MAX_STAMP_NS: i64 = crate::cloud::MAX_STAMP_NS;
pub const MIN_RESOLUTION: f64 = 0.000001;
pub const MAX_RESOLUTION: f64 = 1000.;
pub const MAX_MAP_LAYERS: usize = 3;
pub const MAX_POSE_LAYERS: usize = 1;
pub const MAX_PATH_LAYERS: usize = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathPose {
    pub frame_id: String,
    pub stamp_ns: i64,
    /// XYZ then quaternion XYZW, in the Path header's source frame.
    pub components: [f64; 7],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Geometry {
    OccupancyGrid {
        width: u32,
        height: u32,
        resolution: f64,
        origin: [f64; 7],
        map_load_time_ns: i64,
        cells: Vec<i8>,
    },
    Pose {
        components: [f64; 7],
    },
    Path {
        poses: Vec<PathPose>,
    },
}
pub fn valid_pose(pose: &[f64; 7]) -> bool {
    pose.iter().all(|x| x.is_finite())
        && pose[..3].iter().all(|x| x.abs() <= MAX_COORDINATE)
        && (pose[3..].iter().map(|x| x * x).sum::<f64>() - 1.).abs() <= 0.000001
}
/// Validate source-space affine corners without per-cell allocation. Dimension
/// caps belong to the caller, so legacy mapping1 and large map assembly share
/// identical finite/world-bound rules without changing their admitted sizes.
pub fn valid_map_bounds(width: u32, height: u32, resolution: f64, origin: &[f64; 7]) -> bool {
    map_corners(width, height, resolution, origin).is_some()
}
/// Apply a validated XYZ/quaternion pose to one point. Callers validate pose and
/// resulting bounds separately; this does not grant clock or TF permission.
pub fn transform_point(pose: &[f64; 7], point: [f64; 3]) -> [f64; 3] {
    let [tx, ty, tz, qx, qy, qz, qw] = *pose;
    let [x, y, z] = point;
    [
        tx + (1. - 2. * (qy * qy + qz * qz)) * x
            + 2. * (qx * qy - qz * qw) * y
            + 2. * (qx * qz + qy * qw) * z,
        ty + 2. * (qx * qy + qz * qw) * x
            + (1. - 2. * (qx * qx + qz * qz)) * y
            + 2. * (qy * qz - qx * qw) * z,
        tz + 2. * (qx * qz - qy * qw) * x
            + 2. * (qy * qz + qx * qw) * y
            + (1. - 2. * (qx * qx + qy * qy)) * z,
    ]
}
/// Four row-major rectangle corners, already transformed by the source origin.
/// No per-cell geometry is allocated.
pub fn map_corners(
    width: u32,
    height: u32,
    resolution: f64,
    origin: &[f64; 7],
) -> Option<[[f64; 3]; 4]> {
    if !resolution.is_finite()
        || !(MIN_RESOLUTION..=MAX_RESOLUTION).contains(&resolution)
        || !valid_pose(origin)
    {
        return None;
    }
    let corners = [
        [0., 0., 0.],
        [width as f64 * resolution, 0., 0.],
        [0., height as f64 * resolution, 0.],
        [width as f64 * resolution, height as f64 * resolution, 0.],
    ]
    .map(|point| transform_point(origin, point));
    corners
        .iter()
        .flatten()
        .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
        .then_some(corners)
}

impl Geometry {
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::OccupancyGrid { .. } => MAP_MESSAGE_TYPE,
            Self::Pose { .. } => POSE_MESSAGE_TYPE,
            Self::Path { .. } => PATH_MESSAGE_TYPE,
        }
    }
    pub fn validate(&self, source_frame: &str) -> Result<(), String> {
        let invalid = || "Invalid bounded mapping geometry".to_string();
        match self {
            Self::OccupancyGrid {
                width,
                height,
                resolution,
                origin,
                map_load_time_ns,
                cells,
            } => {
                let count = (*width as usize)
                    .checked_mul(*height as usize)
                    .ok_or_else(invalid)?;
                if *width > MAX_MAP_DIMENSION
                    || *height > MAX_MAP_DIMENSION
                    || count > MAX_MAP_CELLS
                    || cells.len() != count
                    || ((*width == 0 || *height == 0) && (*width != 0 || *height != 0))
                    || !resolution.is_finite()
                    || !(MIN_RESOLUTION..=MAX_RESOLUTION).contains(resolution)
                    || !valid_pose(origin)
                    || !(0..=MAX_STAMP_NS).contains(map_load_time_ns)
                    || cells.iter().any(|v| !(-1..=100).contains(v))
                {
                    return Err(invalid());
                }
                if !valid_map_bounds(*width, *height, *resolution, origin) {
                    return Err(invalid());
                }
            }
            Self::Pose { components } if !valid_pose(components) => return Err(invalid()),
            Self::Path { poses } => {
                if poses.len() > MAX_PATH_POSES
                    || poses.iter().any(|p| {
                        !valid_pose(&p.components)
                            || !(0..=MAX_STAMP_NS).contains(&p.stamp_ns)
                            || (!p.frame_id.is_empty() && p.frame_id != source_frame)
                    })
                {
                    return Err(invalid());
                }
            }
            Self::Pose { .. } => {}
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub messages_received: u64,
    pub pending_replaced: u64,
    pub lookup_timeouts: u64,
    pub tf_dynamic_batches: u64,
    pub tf_static_batches: u64,
    pub tf_accepted: u64,
    pub tf_rejected: u64,
    pub duplicate_statics: u64,
    pub native_errors: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub topic: String,
    pub message_type: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub geometry: Geometry,
}
fn validate_source(
    topic: &str,
    ty: &str,
    frame: &str,
    stamp: i64,
    geometry: &Geometry,
) -> Result<(), String> {
    let valid_token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    };
    if !topic.starts_with('/')
        || topic.len() > 256
        || !topic[1..].split('/').all(valid_token)
        || ty != geometry.message_type()
        || !crate::spatial::valid_frame(frame)
        || !(1..=MAX_STAMP_NS).contains(&stamp)
    {
        return Err("Invalid mapping source, type, frame or positive acquisition stamp".into());
    }
    geometry.validate(frame)
}
impl Decoded {
    pub fn validate(&self) -> Result<(), String> {
        validate_source(
            &self.topic,
            &self.message_type,
            &self.source_frame,
            self.acquisition_stamp_ns,
            &self.geometry,
        )
    }
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
    pub geometry: Geometry,
    pub tf: TfOutcome,
    pub counters: Counters,
}
impl Sample {
    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != CONTRACT_VERSION
            || !crate::spatial::valid_frame(&self.fixed_frame)
        {
            return Err("Unsupported mapping contract or invalid fixed frame".into());
        }
        validate_source(
            &self.topic,
            &self.message_type,
            &self.source_frame,
            self.acquisition_stamp_ns,
            &self.geometry,
        )?;
        if let TfOutcome::Ready { components } = &self.tf {
            if !valid_pose(components) {
                return Err("Invalid mapping acquisition transform".into());
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
