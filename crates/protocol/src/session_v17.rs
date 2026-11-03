//! Paired identified observation-session contract. Standalone CLI v2 is separate.
use crate::{cloud, image_v1 as image, mapping, robot, spatial, structured, subscriptions::*};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

pub const VERSION: u32 = 17;
pub const SHARED_SCENE_VERSION: u32 = 1;
pub const MAX_COMMAND_BYTES: usize = robot::REQUIRED_COMMAND_BYTES;
pub const MAX_ERROR_BYTES: usize = 512;
pub const MAX_GRAPH_TOPICS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Start {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
        configuration: Configuration,
    },
    Stop {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
    ClearMarkers {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
    ClearPose {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
}
impl Command {
    pub fn ids(&self) -> (Generation, RequestId, SubscriptionId) {
        match self {
            Self::Start {
                generation,
                request,
                subscription,
                ..
            }
            | Self::Stop {
                generation,
                request,
                subscription,
            }
            | Self::ClearMarkers {
                generation,
                request,
                subscription,
            }
            | Self::ClearPose {
                generation,
                request,
                subscription,
            } => (*generation, *request, *subscription),
        }
    }
    /// Configuration rejection is a correlated start failure, not a framing
    /// error that would tear down unrelated streams. The backend validates it
    /// before creating a native handle.
    pub fn validate(&self) -> Result<(), String> {
        let (generation, request, subscription) = self.ids();
        if generation.0 == 0 || request.0 == 0 || subscription.0 == 0 {
            return Err("Observation IDs must be nonzero".into());
        }
        Ok(())
    }
    pub fn decode(line: &[u8]) -> Result<Self, String> {
        if line.len() > MAX_COMMAND_BYTES {
            return Err("Observation command exceeds 128 KiB".into());
        }
        let value: Self = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode_line(self, MAX_COMMAND_BYTES)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
// Ready carries the bounded handshake once per connection. Its capability records
// are already boxed; another allocation for this 312-byte enum is unnecessary.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Ready {
        camera_snapshot: Box<crate::camera_snapshot::transport::Capabilities>,
        pose_snapshot: Box<crate::pose_snapshot::transport::Capabilities>,
        marker_snapshot: Box<crate::marker_snapshot::Capabilities>,
        map_snapshot: Box<crate::map_snapshot::Capabilities>,
        robot_description: Box<crate::robot_description::Capabilities>,
        tf_graph: Box<crate::tf_graph::Capabilities>,
        version: u32,
        distro: Box<str>,
        rmw: Box<str>,
        max_subscriptions: usize,
        max_sample_bytes: usize,
        max_command_bytes: usize,
        image_contract_version: u32,
        max_image_raw_bytes: usize,
        max_image_sample_bytes: usize,
        spatial_contract_version: u32,
        max_native_handles: usize,
        max_spatial_scan_rays: usize,
        max_tf_frames: usize,
        max_tf_edges: usize,
        max_tf_dynamic_inserts: usize,
        tf_cache_window_ns: i64,
        spatial_pending_timeout_ms: u64,
        spatial_retry_interval_ms: u64,
        robot_contract_version: u32,
        max_robot_links: usize,
        max_robot_hz: u32,
        max_robot_sample_bytes: usize,
        shared_scene_contract_version: u32,
        max_sensor_layers: usize,
        point_cloud_contract_version: u32,
        max_cloud_source_points: usize,
        max_cloud_raw_bytes: usize,
        max_cloud_sample_bytes: usize,
        mapping_contract_version: u32,
        max_map_dimension: u32,
        max_map_cells: usize,
        max_path_poses: usize,
        max_mapping_sample_bytes: usize,
        max_map_layers: usize,
        max_pose_layers: usize,
        max_path_layers: usize,
    },
    Graph {
        topics: Vec<(String, Vec<String>)>,
    },
    /// Coalesced names from the existing shared TF listener, never a browsing reader.
    SceneFrames {
        generation: Generation,
        frames: Vec<String>,
    },
    Started {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
    MarkersCleared {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
    PoseCleared {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
        barrier: Box<crate::pose_snapshot::transport::Invalidation>,
    },
    /// Emitted only after the owning worker has dropped its native handle.
    Stopped {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
    },
    Observation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<structured::Sample>,
        backend_drops: u64,
    },
    ImageObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<image::ImageSample>,
        backend_drops: u64,
    },
    SpatialObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<spatial::Sample>,
        backend_drops: u64,
    },
    RobotObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<robot::Sample>,
        backend_drops: u64,
    },
    PointCloudObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<cloud::Sample>,
        backend_drops: u64,
    },
    RobotDescriptionObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::robot_description::Chunk>,
        backend_drops: u64,
    },
    TfGraphObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::tf_graph::Sample>,
        backend_drops: u64,
    },
    MappingObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<mapping::Sample>,
        backend_drops: u64,
    },
    MapSnapshotObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::map_snapshot::Message>,
        backend_drops: u64,
    },
    MarkerSnapshotObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::marker_snapshot::Message>,
        backend_drops: u64,
    },
    PoseSnapshotObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::pose_snapshot::transport::Message>,
        backend_drops: u64,
    },
    CameraSnapshotObservation {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: Box<crate::camera_snapshot::transport::Message>,
        backend_drops: u64,
    },
    /// Final application queue-loss snapshot, emitted before identified native
    /// cleanup/error acknowledgment. Does not acknowledge start or stop itself.
    Drops {
        generation: Generation,
        subscription: SubscriptionId,
        backend_drops: u64,
        spatial_counters: Option<spatial::Counters>,
    },
    RequestError {
        generation: Generation,
        request: RequestId,
        subscription: SubscriptionId,
        message: String,
        /// Only true when the backend can guarantee that no handle remains.
        no_handle: bool,
    },
    /// Runtime failure, emitted after confirmed native-handle cleanup.
    StreamError {
        generation: Generation,
        subscription: SubscriptionId,
        message: String,
    },
    /// Session-wide faults require explicit disconnect/reconnect recovery.
    Error {
        message: String,
    },
}
impl Event {
    pub fn ready(distro: String, rmw: String) -> Self {
        Self::Ready {
            camera_snapshot: Box::default(),
            pose_snapshot: Box::default(),
            marker_snapshot: Box::default(),
            map_snapshot: Box::new(crate::map_snapshot::Capabilities {
                max_layers: 1,
                ..Default::default()
            }),
            robot_description: Box::default(),
            tf_graph: Box::default(),
            version: VERSION,
            distro: distro.into_boxed_str(),
            rmw: rmw.into_boxed_str(),
            max_subscriptions: 8,
            max_sample_bytes: structured::MAX_SAMPLE_BYTES,
            max_command_bytes: MAX_COMMAND_BYTES,
            image_contract_version: image::CONTRACT_VERSION,
            max_image_raw_bytes: image::MAX_RAW_BYTES,
            max_image_sample_bytes: image::MAX_SAMPLE_BYTES,
            spatial_contract_version: spatial::CONTRACT_VERSION,
            max_native_handles: 8,
            max_spatial_scan_rays: spatial::MAX_SCAN_RAYS,
            max_tf_frames: spatial::MAX_FRAMES,
            max_tf_edges: spatial::MAX_EDGES,
            max_tf_dynamic_inserts: spatial::DYNAMIC_INSERTS_PER_WINDOW,
            tf_cache_window_ns: spatial::CACHE_WINDOW_NS,
            spatial_pending_timeout_ms: spatial::PENDING_TIMEOUT_MS,
            spatial_retry_interval_ms: spatial::RETRY_INTERVAL_MS,
            robot_contract_version: robot::CONTRACT_VERSION,
            max_robot_links: robot::MAX_LINKS,
            max_robot_hz: robot::MAX_HZ,
            max_robot_sample_bytes: robot::MAX_SAMPLE_BYTES,
            shared_scene_contract_version: SHARED_SCENE_VERSION,
            max_sensor_layers: MAX_SENSOR_LAYERS,
            point_cloud_contract_version: cloud::CONTRACT_VERSION,
            max_cloud_source_points: cloud::MAX_SOURCE_POINTS,
            max_cloud_raw_bytes: cloud::MAX_RAW_BYTES,
            max_cloud_sample_bytes: cloud::MAX_SAMPLE_BYTES,
            mapping_contract_version: mapping::CONTRACT_VERSION,
            max_map_dimension: mapping::MAX_MAP_DIMENSION,
            max_map_cells: mapping::MAX_MAP_CELLS,
            max_path_poses: mapping::MAX_PATH_POSES,
            max_mapping_sample_bytes: mapping::MAX_SAMPLE_BYTES,
            max_map_layers: 1,
            max_pose_layers: mapping::MAX_POSE_LAYERS,
            max_path_layers: mapping::MAX_PATH_LAYERS,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        fn identity(g: Generation, s: SubscriptionId) -> Result<(), String> {
            if g.0 == 0 || s.0 == 0 {
                Err("Observation IDs must be nonzero".into())
            } else {
                Ok(())
            }
        }
        fn message(s: &str) -> Result<(), String> {
            if s.is_empty() || s.len() > MAX_ERROR_BYTES {
                Err("Invalid bounded observation error".into())
            } else {
                Ok(())
            }
        }
        match self {
            Self::Ready {
                camera_snapshot,
                pose_snapshot,
                marker_snapshot,
                map_snapshot,
                robot_description,
                tf_graph,
                version,
                distro,
                rmw,
                max_subscriptions,
                max_sample_bytes,
                max_command_bytes,
                image_contract_version,
                max_image_raw_bytes,
                max_image_sample_bytes,
                spatial_contract_version,
                max_native_handles,
                max_spatial_scan_rays,
                max_tf_frames,
                max_tf_edges,
                max_tf_dynamic_inserts,
                tf_cache_window_ns,
                spatial_pending_timeout_ms,
                spatial_retry_interval_ms,
                robot_contract_version,
                max_robot_links,
                max_robot_hz,
                max_robot_sample_bytes,
                shared_scene_contract_version,
                max_sensor_layers,
                point_cloud_contract_version,
                max_cloud_source_points,
                max_cloud_raw_bytes,
                max_cloud_sample_bytes,
                mapping_contract_version,
                max_map_dimension,
                max_map_cells,
                max_path_poses,
                max_mapping_sample_bytes,
                max_map_layers,
                max_pose_layers,
                max_path_layers,
            } => {
                if **camera_snapshot != crate::camera_snapshot::transport::Capabilities::default()
                    || pose_snapshot.validate().is_err()
                    || **marker_snapshot != crate::marker_snapshot::Capabilities::default()
                    || **map_snapshot
                        != (crate::map_snapshot::Capabilities {
                            max_layers: 1,
                            ..Default::default()
                        })
                    || **robot_description != crate::robot_description::Capabilities::default()
                    || **tf_graph != crate::tf_graph::Capabilities::default()
                    || *version != VERSION
                    || *shared_scene_contract_version != SHARED_SCENE_VERSION
                    || *max_sensor_layers != MAX_SENSOR_LAYERS
                    || *point_cloud_contract_version != cloud::CONTRACT_VERSION
                    || *max_cloud_source_points != cloud::MAX_SOURCE_POINTS
                    || *max_cloud_raw_bytes != cloud::MAX_RAW_BYTES
                    || *max_cloud_sample_bytes != cloud::MAX_SAMPLE_BYTES
                    || *mapping_contract_version != mapping::CONTRACT_VERSION
                    || *max_map_dimension != mapping::MAX_MAP_DIMENSION
                    || *max_map_cells != mapping::MAX_MAP_CELLS
                    || *max_path_poses != mapping::MAX_PATH_POSES
                    || *max_mapping_sample_bytes != mapping::MAX_SAMPLE_BYTES
                    || *max_map_layers != 1
                    || *max_pose_layers != mapping::MAX_POSE_LAYERS
                    || *max_path_layers != mapping::MAX_PATH_LAYERS
                    || *max_subscriptions != 8
                    || *max_sample_bytes != structured::MAX_SAMPLE_BYTES
                    || *max_command_bytes != MAX_COMMAND_BYTES
                    || *image_contract_version != image::CONTRACT_VERSION
                    || *max_image_raw_bytes != image::MAX_RAW_BYTES
                    || *max_image_sample_bytes != image::MAX_SAMPLE_BYTES
                    || *spatial_contract_version != spatial::CONTRACT_VERSION
                    || *max_native_handles != 8
                    || *max_spatial_scan_rays != spatial::MAX_SCAN_RAYS
                    || *max_tf_frames != spatial::MAX_FRAMES
                    || *max_tf_edges != spatial::MAX_EDGES
                    || *max_tf_dynamic_inserts != spatial::DYNAMIC_INSERTS_PER_WINDOW
                    || *tf_cache_window_ns != spatial::CACHE_WINDOW_NS
                    || *spatial_pending_timeout_ms != spatial::PENDING_TIMEOUT_MS
                    || *spatial_retry_interval_ms != spatial::RETRY_INTERVAL_MS
                    || *robot_contract_version != robot::CONTRACT_VERSION
                    || *max_robot_links != robot::MAX_LINKS
                    || *max_robot_hz != robot::MAX_HZ
                    || *max_robot_sample_bytes != robot::MAX_SAMPLE_BYTES
                    || distro.is_empty()
                    || distro.len() > 64
                    || rmw.is_empty()
                    || rmw.len() > 128
                {
                    return Err("Incompatible observation session or limits".into());
                }
            }
            Self::SceneFrames { generation, frames } => {
                if generation.0 == 0
                    || frames.len() > spatial::MAX_FRAMES
                    || frames.iter().any(|frame| !spatial::valid_frame(frame))
                    || frames
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != frames.len()
                {
                    return Err("Observed TF frames exceed supported identity or bounds".into());
                }
            }
            Self::Graph { topics } => {
                if topics.len() > MAX_GRAPH_TOPICS
                    || topics.iter().any(|(name, types)| {
                        name.is_empty()
                            || name.len() > 256
                            || types.len() > 16
                            || types.iter().any(|ty| ty.is_empty() || ty.len() > 256)
                    })
                {
                    return Err("Discovery graph exceeds supported bounds".into());
                }
            }
            Self::PoseCleared {
                generation,
                request,
                subscription,
                barrier,
            } => {
                identity(*generation, *subscription)?;
                barrier.validate()?;
                if request.0 == 0
                    || barrier.owner.generation != *generation
                    || barrier.owner.subscription != *subscription
                    || barrier.reason != crate::pose_snapshot::ResetReason::LocalClear
                {
                    return Err("Invalid correlated pose Clear acknowledgment".into());
                }
            }
            Self::CameraSnapshotObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0
                    || sample.owner().generation != *generation
                    || sample.owner().subscription != *subscription
                {
                    return Err("Camera envelope contradicts owner or sequence".into());
                }
                sample.encode()?;
            }
            Self::PoseSnapshotObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                sample.validate()?;
                if *sequence == 0
                    || sample.owner().generation != *generation
                    || sample.owner().subscription != *subscription
                {
                    return Err("Pose envelope contradicts owner or sequence".into());
                }
                if sample.encode()?.len() > crate::pose_snapshot::MAX_SAMPLE_BYTES {
                    return Err("Pose message exceeds budget".into());
                }
            }
            Self::MarkersCleared {
                generation,
                request,
                subscription,
            }
            | Self::Started {
                generation,
                request,
                subscription,
            }
            | Self::Stopped {
                generation,
                request,
                subscription,
            } => {
                identity(*generation, *subscription)?;
                if request.0 == 0 {
                    return Err("Request ID must be nonzero".into());
                }
            }
            Self::Observation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::Drops {
                generation,
                subscription,
                ..
            } => identity(*generation, *subscription)?,
            Self::RobotDescriptionObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::TfGraphObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::SpatialObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::ImageObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::RobotObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode_wire()?;
            }
            Self::PointCloudObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::MappingObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Sample sequence must be nonzero".into());
                }
                sample.encode()?;
            }
            Self::MarkerSnapshotObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Marker sample sequence must be positive".into());
                }
                sample.validate()?;
                let owner = match sample.as_ref() {
                    crate::marker_snapshot::Message::Chunk(c) => &c.header.owner,
                    crate::marker_snapshot::Message::Prune(p) => &p.owner,
                    crate::marker_snapshot::Message::Clear(c) => &c.owner,
                };
                if owner.generation != *generation || owner.subscription != *subscription {
                    return Err("Marker envelope contradicts owner".into());
                }
                sample.encode()?;
            }
            Self::MapSnapshotObservation {
                generation,
                subscription,
                sequence,
                sample,
                ..
            } => {
                identity(*generation, *subscription)?;
                if *sequence == 0 {
                    return Err("Map sample sequence must be positive".into());
                }
                sample.validate()?;
                let owner = match sample.as_ref() {
                    crate::map_snapshot::Message::Chunk(chunk) => &chunk.header.owner,
                    crate::map_snapshot::Message::Clear(clear) => &clear.owner,
                };
                if owner.generation != *generation || owner.subscription != *subscription {
                    return Err("Map envelope contradicts sample owner".into());
                }
                sample.encode()?;
            }
            Self::RequestError {
                generation,
                request,
                subscription,
                message: text,
                ..
            } => {
                identity(*generation, *subscription)?;
                if request.0 == 0 {
                    return Err("Request ID must be nonzero".into());
                }
                message(text)?;
            }
            Self::StreamError {
                generation,
                subscription,
                message: text,
            } => {
                identity(*generation, *subscription)?;
                message(text)?;
            }
            Self::Error { message: text } => message(text)?,
        }
        Ok(())
    }
    pub fn decode(line: &[u8]) -> Result<Self, String> {
        if line.len() > crate::MAX_LINE {
            return Err("Observation event exceeds line limit".into());
        }
        let value: Self = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode_line(self, crate::MAX_LINE)
    }
}

/// UTF-8-safe diagnostic bound; a marker is included in the same byte budget.
pub fn bounded_error(message: &str) -> String {
    if message.is_empty() {
        return "Observation failed without details".into();
    }
    if message.len() <= MAX_ERROR_BYTES {
        return message.into();
    }
    let suffix = " … [truncated]";
    let mut end = MAX_ERROR_BYTES - suffix.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &message[..end])
}

pub(crate) fn encode_line(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    struct Limited {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Observation frame exceeds byte budget",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Limited {
        bytes: Vec::new(),
        limit: limit.saturating_sub(1),
    };
    serde_json::to_writer(&mut out, value).map_err(|e| e.to_string())?;
    out.bytes.push(b'\n');
    Ok(out.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn description_capabilities_are_strict_and_full_chunks_fit_the_existing_envelope() {
        let value =
            serde_json::to_value(Event::ready("jazzy".into(), "fixture-no-ROS".into())).unwrap();
        for key in [
            "contract_version",
            "max_document_bytes",
            "max_chunk_bytes",
            "max_chunks",
            "max_owners",
        ] {
            let mut missing = value.clone();
            missing["robot_description"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(Event::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
            let mut changed = value.clone();
            changed["robot_description"][key] = 0.into();
            assert!(Event::decode(&serde_json::to_vec(&changed).unwrap()).is_err());
        }
        let mut extra = value;
        extra["robot_description"]["extra"] = true.into();
        assert!(Event::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
        let document = crate::robot_description::Document::new(
            "/robot_description",
            "x".repeat(crate::robot_description::MAX_DOCUMENT_BYTES),
        )
        .unwrap();
        let event = Event::RobotDescriptionObservation {
            generation: Generation(1),
            subscription: SubscriptionId(1),
            sequence: 1,
            sample: Box::new(document.chunk(0).unwrap()),
            backend_drops: 0,
        };
        assert!(event.encode().unwrap().len() < crate::MAX_LINE);
        assert!(Event::decode(&event.encode().unwrap()).is_ok());
    }
    #[test]
    fn v12_compact_start_is_explicit_and_capabilities_are_negotiated() {
        let wire = serde_json::json!({"command":"start","generation":1,"request":1,"subscription":1,
            "configuration":{"topic":"/camera","message_type":"sensor_msgs/msg/Image","best_effort":false,"transient_local":false,"output":"compact_image"}});
        let decoded = Command::decode(&serde_json::to_vec(&wire).unwrap())
            .expect("Explicit compact output must be supported by the paired v12 session");
        assert!(matches!(decoded,Command::Start{configuration,..} if configuration.valid()));
        let ready =
            serde_json::to_value(Event::ready("jazzy".into(), "fixture-no-ROS".into())).unwrap();
        assert_eq!(ready["version"], VERSION);
        assert_eq!(ready["image_contract_version"], 1);
        assert_eq!(ready["max_image_raw_bytes"], 262144);
        assert_eq!(ready["max_image_sample_bytes"], 524288);
        for (key, expected) in [
            ("spatial_contract_version", 1),
            ("shared_scene_contract_version", 1),
            ("max_sensor_layers", 4),
            ("point_cloud_contract_version", 2),
            ("max_cloud_source_points", 8192),
            ("max_cloud_raw_bytes", 262144),
            ("max_cloud_sample_bytes", 524288),
            ("max_native_handles", 8),
            ("max_spatial_scan_rays", 2048),
            ("max_tf_frames", 256),
            ("max_tf_edges", 128),
            ("max_tf_dynamic_inserts", 256),
            ("tf_cache_window_ns", 10_000_000_000u64),
            ("spatial_pending_timeout_ms", 1000),
            ("spatial_retry_interval_ms", 50),
        ] {
            assert_eq!(ready[key], expected);
            let mut bad = ready.clone();
            bad[key] = (expected + 1).into();
            assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
            let mut absent = ready.clone();
            absent.as_object_mut().unwrap().remove(key);
            assert!(Event::decode(&serde_json::to_vec(&absent).unwrap()).is_err());
        }
        for (key, wrong) in [
            ("image_contract_version", 2),
            ("max_image_raw_bytes", 262145),
            ("max_image_sample_bytes", 524289),
        ] {
            let mut bad = ready.clone();
            bad[key] = wrong.into();
            assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
            let mut absent = ready.clone();
            absent.as_object_mut().unwrap().remove(key);
            assert!(Event::decode(&serde_json::to_vec(&absent).unwrap()).is_err());
        }
        let mut missing = wire.clone();
        missing["configuration"]
            .as_object_mut()
            .unwrap()
            .remove("output");
        assert!(Command::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
    #[test]
    fn tf_graph_capabilities_must_be_exact_and_complete() {
        let ready =
            serde_json::to_value(Event::ready("jazzy".into(), "fixture-no-ROS".into())).unwrap();
        assert_eq!(
            ready["tf_graph"],
            serde_json::json!({"contract_version":1,"max_hz":5,"max_sample_bytes":524288,"max_owners":1})
        );
        for key in [
            "contract_version",
            "max_hz",
            "max_sample_bytes",
            "max_owners",
        ] {
            let mut missing = ready.clone();
            missing["tf_graph"].as_object_mut().unwrap().remove(key);
            assert!(Event::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
            let mut changed = ready.clone();
            changed["tf_graph"][key] = 0.into();
            assert!(Event::decode(&serde_json::to_vec(&changed).unwrap()).is_err());
        }
        let mut extra = ready;
        extra["tf_graph"]["extra"] = true.into();
        assert!(Event::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
    }
    #[test]
    fn observation_controls_are_explicit_bounded_and_not_robot_commands() {
        let start = Command::Start {
            generation: Generation(9),
            request: RequestId(1),
            subscription: SubscriptionId(2),
            configuration: Configuration {
                map_updates: None,
                topic: "/one".into(),
                message_type: "std_msgs/msg/String".into(),
                best_effort: true,
                transient_local: false,
                output: Output::Typed,
                spatial: None,
                robot: None,
            },
        };
        assert_eq!(Command::decode(&start.encode().unwrap()).unwrap(), start);
        for bad in [
            r#"{"command":"publish","topic":"/cmd_vel"}"#,
            r#"{"command":"stop","generation":0,"request":1,"subscription":2}"#,
            r#"{"command":"stop","generation":1,"request":1,"subscription":2,"execute":true}"#,
        ] {
            assert!(Command::decode(bad.as_bytes()).is_err());
        }
        assert!(Command::decode(&vec![b' '; MAX_COMMAND_BYTES + 1]).is_err());
    }
    #[test]
    fn ready_requires_paired_version_and_identical_limits() {
        let ready = Event::ready("jazzy".into(), "fixture-no-ros".into());
        assert!(Event::decode(&ready.encode().unwrap()).is_ok());
        let wire = String::from_utf8(ready.encode().unwrap()).unwrap();
        for version in 1..VERSION {
            assert!(Event::decode(
                wire.replace(
                    &format!("\"version\":{VERSION}"),
                    &format!("\"version\":{version}")
                )
                .as_bytes()
            )
            .is_err());
        }
        assert!(Event::decode(
            wire.replace("\"max_subscriptions\":8", "\"max_subscriptions\":9")
                .as_bytes()
        )
        .is_err());
    }
    #[test]
    fn typed_fidelity_and_identity_survive_session_framing() {
        let sample = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/one".into(),
            message_type: "std_msgs/msg/UInt64".into(),
            source_time: None,
            value: structured::Value::Unsigned {
                value: u64::MAX.to_string(),
            },
        };
        let event = Event::Observation {
            generation: Generation(8),
            subscription: SubscriptionId(9),
            sequence: 1,
            sample: Box::new(sample.clone()),
            backend_drops: 3,
        };
        match Event::decode(&event.encode().unwrap()).unwrap() {
            Event::Observation {
                sample: decoded,
                backend_drops: 3,
                ..
            } => assert_eq!(*decoded, sample),
            _ => panic!("wrong observation"),
        }
    }
    #[test]
    fn final_drop_snapshots_are_identified_bounded_and_preserve_u64_counts() {
        let event = Event::Drops {
            generation: Generation(9),
            subscription: SubscriptionId(4),
            backend_drops: u64::MAX,
            spatial_counters: None,
        };
        let bytes = event.encode().unwrap();
        assert!(bytes.len() < MAX_COMMAND_BYTES);
        assert!(matches!(
            Event::decode(&bytes).unwrap(),
            Event::Drops {
                backend_drops: u64::MAX,
                ..
            }
        ));
        assert!(Event::Drops {
            generation: Generation(0),
            subscription: SubscriptionId(4),
            backend_drops: 0,
            spatial_counters: None,
        }
        .encode()
        .is_err());
        assert!(Event::Drops {
            generation: Generation(9),
            subscription: SubscriptionId(0),
            backend_drops: 0,
            spatial_counters: None,
        }
        .encode()
        .is_err());
    }
    #[test]
    fn compact_image_observation_keeps_full_raw_budget_identity_and_metadata() {
        let raw: Vec<_> = (0..image::MAX_RAW_BYTES).map(|i| (i % 256) as u8).collect();
        let metadata = image::ImageMetadata {
            header_stamp: image::HeaderStamp {
                seconds: 7,
                nanoseconds: 123,
            },
            frame_id: "optical".into(),
            width: 256,
            height: 256,
            encoding: "rgba8".into(),
            is_bigendian: false,
            step: 1024,
        };
        let sample = image::ImageSample::from_raw("/camera", metadata.clone(), &raw).unwrap();
        let event = Event::ImageObservation {
            generation: Generation(8),
            subscription: SubscriptionId(9),
            sequence: 2,
            sample: Box::new(sample),
            backend_drops: 3,
        };
        let wire = event.encode().unwrap();
        assert!(wire.len() < crate::MAX_LINE);
        match Event::decode(&wire).unwrap() {
            Event::ImageObservation {
                generation: Generation(8),
                subscription: SubscriptionId(9),
                sequence: 2,
                sample,
                backend_drops: 3,
            } => {
                assert_eq!(sample.metadata, metadata);
                assert_eq!(sample.raw_bytes().unwrap(), raw);
            }
            _ => panic!("Compact observation identity changed"),
        }
        let valid: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        for (key, value) in [("sequence", 0), ("generation", 0), ("subscription", 0)] {
            let mut bad = valid.clone();
            bad[key] = value.into();
            assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        for field in ["contract_version", "original_bytes"] {
            let mut bad = valid.clone();
            bad["sample"][field] = 0.into();
            assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
    }
    #[test]
    fn native_diagnostics_and_graph_serialization_are_bounded() {
        let message = bounded_error(&"🤖\n".repeat(2000));
        assert!(message.len() <= MAX_ERROR_BYTES && message.ends_with("[truncated]"));
        assert!(Event::Error { message }.encode().unwrap().len() < MAX_COMMAND_BYTES);
        // Valid individual metadata limits still cannot exceed the hard frame
        // budget through aggregate escaping/entry growth.
        let graph = Event::Graph {
            topics: vec![("x".repeat(256), vec!["y".repeat(256); 16]); MAX_GRAPH_TOPICS],
        };
        assert!(graph.encode().is_err());
        assert!(Event::Started {
            generation: Generation(1),
            request: RequestId(0),
            subscription: SubscriptionId(1)
        }
        .encode()
        .is_err());
    }
}

#[cfg(test)]
mod frame_metadata_tests {
    use super::*;
    #[test]
    fn frames_are_bounded_and_session16_is_frozen() {
        assert_eq!(crate::session_v16::VERSION, 16);
        let frames = Event::SceneFrames {
            generation: Generation(1),
            frames: vec!["map".into(), "laser".into()],
        };
        let bytes = frames.encode().unwrap();
        assert!(Event::decode(&bytes).is_ok());
        assert!(crate::session_v16::Event::decode(&bytes).is_err());
        assert!(crate::session_v16::Event::decode(
            &Event::ready("jazzy".into(), "fixture".into())
                .encode()
                .unwrap()
        )
        .is_err());
        for (generation, frames) in [
            (0, vec!["map".into()]),
            (1, vec!["map".into(), "map".into()]),
            (1, vec!["/bad".into()]),
            (1, vec!["x".repeat(257)]),
            (1, (0..257).map(|n| format!("f{n}")).collect()),
        ] {
            assert!(Event::SceneFrames {
                generation: Generation(generation),
                frames
            }
            .encode()
            .is_err());
        }
    }
}
