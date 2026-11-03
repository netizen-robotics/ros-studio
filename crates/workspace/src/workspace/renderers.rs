use super::{invalid, message_type_valid, topic_valid, valid_id, Error, Qos};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", from = "WireViewKind")]
pub enum ViewKind {
    /// Visualizer v17 only; raw rectified Image/CameraInfo and local intent.
    #[serde(skip)]
    Camera {
        info: studio_protocol::camera_snapshot::InfoSource,
        appearance: crate::camera::Appearance,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    /// Visualizer v16 only; source and local appearance intent, never live data.
    #[serde(skip)]
    Uncertainty {
        configuration: studio_protocol::pose_snapshot::transport::Configuration,
        appearance: crate::uncertainty::Appearance,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    /// Visualizer v15 only; frozen legacy grammars exclude this display.
    #[serde(skip)]
    Markers {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    /// Application-v14 only, excluded from all frozen legacy grammars.
    #[serde(skip)]
    Map {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
        updates: Option<crate::map::UpdateSource>,
        appearance: crate::map::Appearance,
    },
    /// Application-v11 only; frozen legacy decoders intentionally exclude it.
    #[serde(skip)]
    RobotTopic {
        description: super::RobotTopicSource,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    /// Application-v10 only: deliberately absent from frozen WireViewKind.
    #[serde(skip)]
    TfGraph {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Messages,
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<SpatialLayerConfig>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
    PointCloud {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    OccupancyGrid {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Pose {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Path {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Robot {
        assets: super::RobotAssets,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
// A unit variant silently ignores extra members in serde's internally tagged
// representation. Empty struct variant makes Messages reject them as well.
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum WireViewKind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<SpatialLayerConfig>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
    PointCloud {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    OccupancyGrid {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Pose {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Path {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Robot {
        assets: super::RobotAssets,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<WireViewKind> for ViewKind {
    fn from(value: WireViewKind) -> Self {
        match value {
            WireViewKind::PointCloud {
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::PointCloud {
                fixed_frame,
                transforms,
                clock_expectation,
            },
            WireViewKind::OccupancyGrid {
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::OccupancyGrid {
                fixed_frame,
                transforms,
                clock_expectation,
            },
            WireViewKind::Pose {
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::Pose {
                fixed_frame,
                transforms,
                clock_expectation,
            },
            WireViewKind::Path {
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::Path {
                fixed_frame,
                transforms,
                clock_expectation,
            },
            WireViewKind::Messages {} => Self::Messages,
            WireViewKind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            WireViewKind::Image { display_mode } => Self::Image { display_mode },
            WireViewKind::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            },
            WireViewKind::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            } => Self::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "segment",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PathSegment {
    Field(String),
    Index(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMode {
    Fit,
    ActualSize,
}
/// A saved expectation, never proof or authorization to use system-clock data.
/// Runtime verification must be obtained again after restore or reconnect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockExpectation {
    Unknown,
    System,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialTransforms {
    pub dynamic_tf: TransformInput,
    pub static_tf: TransformInput,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformInput {
    pub topic: String,
    pub message_type: String,
    pub qos: Qos,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialLayerConfig {
    pub id: String,
    pub kind: LayerKind,
    pub topic: String,
    pub message_type: String,
    pub qos: Qos,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    LaserScan,
}
impl ViewKind {
    pub(super) fn validate(&self, message_type: &str) -> Result<(), Error> {
        match self {
            Self::Camera {
                info,
                appearance,
                fixed_frame,
                transforms,
                ..
            } => {
                info.validate().map_err(invalid)?;
                appearance.validate().map_err(invalid)?;
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::image::MESSAGE_TYPE
                    || info.topic == transforms.dynamic_tf.topic
                    || info.topic == transforms.static_tf.topic
                {
                    return Err(invalid(
                        "Camera requires rectified Image and a distinct CameraInfo source",
                    ));
                }
            }
            Self::Uncertainty {
                configuration,
                appearance,
                fixed_frame,
                transforms,
                ..
            } => {
                let _ = configuration.settings().map_err(invalid)?;
                appearance.validate().map_err(invalid)?;
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != configuration.kind.message_type() {
                    return Err(invalid(
                        "Uncertainty source type must match its configured kind",
                    ));
                }
            }
            Self::Markers {
                fixed_frame,
                transforms,
                ..
            } => {
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if !matches!(
                    message_type,
                    studio_protocol::marker::MESSAGE_TYPE
                        | studio_protocol::marker::ARRAY_MESSAGE_TYPE
                ) {
                    return Err(invalid("Markers require exact Marker or MarkerArray type"));
                }
            }
            Self::Map {
                fixed_frame,
                transforms,
                updates,
                appearance,
                ..
            } => {
                Self::OccupancyGrid {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(message_type)?;
                appearance.validate()?;
                if let Some(updates) = updates {
                    updates.validate()?;
                    if updates.topic == transforms.dynamic_tf.topic
                        || updates.topic == transforms.static_tf.topic
                    {
                        return Err(invalid("Map update source must differ from TF inputs"));
                    }
                }
            }
            Self::RobotTopic {
                description,
                fixed_frame,
                transforms,
                ..
            } => {
                description.validate()?;
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::spatial_input::TF_MESSAGE_TYPE {
                    return Err(invalid("Robot pose source requires tf2_msgs/msg/TFMessage"));
                }
            }
            Self::TfGraph {
                fixed_frame,
                transforms,
                ..
            } => {
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::spatial_input::TF_MESSAGE_TYPE {
                    return Err(invalid("TF display requires tf2_msgs/msg/TFMessage"));
                }
            }
            Self::Messages => (),
            Self::PointCloud {
                fixed_frame,
                transforms,
                ..
            } => {
                let dynamic = &transforms.dynamic_tf;
                let statics = &transforms.static_tf;
                if message_type != studio_protocol::cloud::MESSAGE_TYPE
                    || !studio_protocol::spatial::valid_frame(fixed_frame)
                    || dynamic.message_type != studio_protocol::spatial_input::TF_MESSAGE_TYPE
                    || statics.message_type != studio_protocol::spatial_input::TF_MESSAGE_TYPE
                    || !topic_valid(&dynamic.topic)
                    || !topic_valid(&statics.topic)
                    || dynamic.topic == statics.topic
                    || dynamic.qos.durability != super::Durability::Volatile
                    || statics.qos.reliability != super::Reliability::Reliable
                    || statics.qos.durability != super::Durability::TransientLocal
                {
                    return Err(invalid("PointCloud2 requires its exact type, fixed frame and explicit compatible TF inputs"));
                }
            }
            Self::OccupancyGrid {
                fixed_frame,
                transforms,
                ..
            } => {
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::mapping::MAP_MESSAGE_TYPE {
                    return Err(invalid("Mapping display requires its exact message type"));
                }
            }
            Self::Pose {
                fixed_frame,
                transforms,
                ..
            } => {
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::mapping::POSE_MESSAGE_TYPE {
                    return Err(invalid("Mapping display requires its exact message type"));
                }
            }
            Self::Path {
                fixed_frame,
                transforms,
                ..
            } => {
                Self::PointCloud {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: ClockExpectation::Unknown,
                }
                .validate(studio_protocol::cloud::MESSAGE_TYPE)?;
                if message_type != studio_protocol::mapping::PATH_MESSAGE_TYPE {
                    return Err(invalid("Mapping display requires its exact message type"));
                }
            }
            Self::Robot {
                assets,
                fixed_frame,
                transforms,
                ..
            } => {
                assets.validate()?;
                let dynamic = &transforms.dynamic_tf;
                let statics = &transforms.static_tf;
                if message_type != "tf2_msgs/msg/TFMessage"
                    || !studio_protocol::spatial::valid_frame(fixed_frame)
                    || dynamic.message_type != message_type
                    || statics.message_type != message_type
                    || !topic_valid(&dynamic.topic)
                    || !topic_valid(&statics.topic)
                    || dynamic.topic == statics.topic
                    || dynamic.qos.durability != super::Durability::Volatile
                    || statics.qos.reliability != super::Reliability::Reliable
                    || statics.qos.durability != super::Durability::TransientLocal
                {
                    return Err(invalid("Robot requires an explicit fixed frame, distinct TF inputs, Volatile dynamic TF and Reliable/TransientLocal static TF"));
                }
            }
            Self::NumericPlot { field_path } => {
                if field_path.is_empty()
                    || field_path.len() > 16
                    || field_path.iter().any(|p| match p {
                        PathSegment::Field(name) => {
                            name.is_empty()
                                || name.len() > 256
                                || name.chars().any(char::is_control)
                        }
                        PathSegment::Index(index) => *index >= 4096,
                    })
                {
                    return Err(invalid(
                        "numeric plot requires an explicit bounded field path",
                    ));
                }
            }
            Self::Image { .. } if message_type != "sensor_msgs/msg/Image" => {
                return Err(invalid("Image requires sensor_msgs/msg/Image"))
            }
            Self::Image { .. } => (),
            Self::Spatial {
                fixed_frame,
                layers,
                transforms,
                ..
            } => {
                if fixed_frame.is_empty()
                    || fixed_frame.len() > 256
                    || fixed_frame
                        .chars()
                        .any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err(invalid(
                        "spatial view requires an explicit fixed frame, at most 256 bytes",
                    ));
                }
                if layers.is_empty() || layers.len() > 4 {
                    return Err(invalid("spatial view supports 1–4 layers"));
                }
                let mut ids = HashSet::new();
                for layer in layers {
                    if !valid_id(&layer.id)
                        || !ids.insert(&layer.id)
                        || !topic_valid(&layer.topic)
                        || !message_type_valid(&layer.message_type)
                        || layer.message_type != "sensor_msgs/msg/LaserScan"
                    {
                        return Err(invalid(
                            "invalid or duplicate LaserScan layer configuration",
                        ));
                    }
                }
                if message_type != "sensor_msgs/msg/LaserScan" {
                    return Err(invalid("Spatial requires sensor_msgs/msg/LaserScan"));
                }
                if let Some(transforms) = transforms {
                    for input in [&transforms.dynamic_tf, &transforms.static_tf] {
                        if !topic_valid(&input.topic) || !message_type_valid(&input.message_type) {
                            return Err(invalid("invalid bounded TF topic/type configuration"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn plot_path(&self) -> Option<crate::plots::FieldPath> {
        match self {
            Self::NumericPlot { field_path } => Some(crate::plots::FieldPath(
                field_path
                    .iter()
                    .map(|s| match s {
                        PathSegment::Field(name) => crate::plots::PathSegment::Field(name.clone()),
                        PathSegment::Index(index) => crate::plots::PathSegment::Index(*index),
                    })
                    .collect(),
            )),
            _ => None,
        }
    }
    pub fn numeric_plot(path: &crate::plots::FieldPath) -> Self {
        Self::NumericPlot {
            field_path: path
                .0
                .iter()
                .map(|s| match s {
                    crate::plots::PathSegment::Field(name) => PathSegment::Field(name.clone()),
                    crate::plots::PathSegment::Index(index) => PathSegment::Index(*index),
                })
                .collect(),
        }
    }
}
