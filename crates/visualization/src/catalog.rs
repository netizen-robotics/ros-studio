//! Closed catalog of implemented observe-only displays. No subscription actions.
use studio_protocol::subscriptions::Output;

pub struct DisplayDescriptor {
    pub output: Output,
    pub label: &'static str,
    pub description: &'static str,
    pub message_type: Option<&'static str>,
}

pub static DISPLAYS: std::sync::LazyLock<Vec<DisplayDescriptor>> = std::sync::LazyLock::new(|| {
    vec![
        DisplayDescriptor {
            output: Output::PoseSnapshot(
                studio_protocol::pose_snapshot::transport::Configuration::for_kind(
                    studio_protocol::pose_snapshot::Kind::PoseWithCovariance,
                ),
            ),
            label: "Pose covariance",
            description: "A stamped pose with position and angular uncertainty.",
            message_type: Some(studio_protocol::pose_snapshot::POSE_MESSAGE_TYPE),
        },
        DisplayDescriptor {
            output: Output::PoseSnapshot(
                studio_protocol::pose_snapshot::transport::Configuration::for_kind(
                    studio_protocol::pose_snapshot::Kind::Odometry,
                ),
            ),
            label: "Odometry",
            description: "A fresh pose with independently thinned history and covariance.",
            message_type: Some(studio_protocol::pose_snapshot::ODOMETRY_MESSAGE_TYPE),
        },
        DisplayDescriptor {
            output: Output::TfGraph,
            label: "TF frames",
            description: "Inspect the shared transform tree, frame axes and latest pose status.",
            message_type: None,
        },
        DisplayDescriptor {
            output: Output::Robot,
            label: "Robot model",
            description: "Load and review a local URDF, then follow link transforms.",
            message_type: None,
        },
        DisplayDescriptor {
            output: Output::SpatialScan,
            label: "Laser scan",
            description: "Acquisition-time scan points in the shared scene.",
            message_type: Some("sensor_msgs/msg/LaserScan"),
        },
        DisplayDescriptor {
            output: Output::PointCloud,
            label: "Point cloud",
            description:
                "PointCloud2 with available source color and intensity in the shared scene.",
            message_type: Some("sensor_msgs/msg/PointCloud2"),
        },
        DisplayDescriptor {
            output: Output::MarkerSnapshot,
            label: "Markers",
            description: "Inspect Marker or MarkerArray shapes, text and reviewed local meshes.",
            message_type: Some(studio_protocol::marker::MESSAGE_TYPE),
        },
        DisplayDescriptor {
            output: Output::MapSnapshot,
            label: "Map",
            description: "Large occupancy maps, optional costmap updates and local palettes.",
            message_type: Some("nav_msgs/msg/OccupancyGrid"),
        },
        DisplayDescriptor {
            output: Output::Pose,
            label: "Pose",
            description: "A stamped position and orientation.",
            message_type: Some("geometry_msgs/msg/PoseStamped"),
        },
        DisplayDescriptor {
            output: Output::Path,
            label: "Path",
            description: "An ordered, bounded stamped path.",
            message_type: Some("nav_msgs/msg/Path"),
        },
        DisplayDescriptor {
            output: Output::CompactImage,
            label: "Image",
            description: "Raw mono8/RGB/BGR/RGBA/BGRA image companion.",
            message_type: Some("sensor_msgs/msg/Image"),
        },
        DisplayDescriptor {
            output: Output::CameraSnapshot(Box::new(
                studio_protocol::camera_snapshot::InfoSource {
                    topic: "/camera_info".into(),
                    best_effort: false,
                    transient_local: false,
                },
            )),
            label: "Calibrated Camera",
            description:
                "Rectified Image aligned with scene geometry using CameraInfo and optical TF.",
            message_type: Some("sensor_msgs/msg/Image"),
        },
    ]
});

pub fn for_type(message_type: &str) -> Option<&'static DisplayDescriptor> {
    if message_type == studio_protocol::marker::ARRAY_MESSAGE_TYPE {
        return DISPLAYS.iter().find(|d| d.output == Output::MarkerSnapshot);
    }
    DISPLAYS
        .iter()
        .find(|d| d.message_type == Some(message_type))
}

/// Every implemented choice for an exact advertised type; browsing never activates it.
pub fn for_type_all(message_type: &str) -> Vec<&'static DisplayDescriptor> {
    DISPLAYS
        .iter()
        .filter(|d| {
            d.message_type == Some(message_type)
                || (d.output == Output::MarkerSnapshot
                    && message_type == studio_protocol::marker::ARRAY_MESSAGE_TYPE)
                || (d.output == Output::TfGraph && message_type == "tf2_msgs/msg/TFMessage")
                || (d.output == Output::Robot && message_type == "std_msgs/msg/String")
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_has_both_choices_and_auxiliary_info_is_not_a_display() {
        let choices = for_type_all("sensor_msgs/msg/Image");
        assert_eq!(choices.len(), 2);
        assert_eq!(
            for_type("sensor_msgs/msg/Image").unwrap().output,
            Output::CompactImage
        );
        assert!(for_type_all("sensor_msgs/msg/CameraInfo").is_empty());
        assert_eq!(
            for_type_all(studio_protocol::marker::ARRAY_MESSAGE_TYPE).len(),
            1
        );
    }
}
