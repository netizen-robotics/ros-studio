use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Inspect,
    Visualize,
    Analyze,
}
impl Preset {
    /// Configuration templates only. UI must gate choices on supported capabilities;
    /// all configured topics/types require discovery reconciliation before activation.
    pub fn workspace(self) -> Workspace {
        let qos = || Qos {
            reliability: Reliability::BestEffort,
            durability: Durability::Volatile,
        };
        let view = |id: &str, kind, topic: &str, message_type: &str| ViewConfig {
            id: id.into(),
            kind,
            topic: topic.into(),
            message_type: message_type.into(),
            qos: qos(),
        };
        let (name, views) = match self {
            Self::Inspect => (
                "Inspect",
                vec![view(
                    "messages",
                    ViewKind::Messages,
                    "/demo/status",
                    "std_msgs/msg/String",
                )],
            ),
            Self::Visualize => (
                "Visualize",
                vec![
                    view(
                        "spatial",
                        ViewKind::Spatial {
                            fixed_frame: "map".into(),
                            // Template intent only; applying it never verifies any clock.
                            clock_expectation: ClockExpectation::System,
                            transforms: Some(SpatialTransforms {
                                dynamic_tf: TransformInput {
                                    topic: "/tf".into(),
                                    message_type: "tf2_msgs/msg/TFMessage".into(),
                                    qos: qos(),
                                },
                                static_tf: TransformInput {
                                    topic: "/tf_static".into(),
                                    message_type: "tf2_msgs/msg/TFMessage".into(),
                                    qos: Qos {
                                        reliability: Reliability::Reliable,
                                        durability: Durability::TransientLocal,
                                    },
                                },
                            }),
                            layers: vec![SpatialLayerConfig {
                                id: "scan".into(),
                                kind: LayerKind::LaserScan,
                                topic: "/scan".into(),
                                message_type: "sensor_msgs/msg/LaserScan".into(),
                                qos: qos(),
                            }],
                        },
                        "/scan",
                        "sensor_msgs/msg/LaserScan",
                    ),
                    view(
                        "image",
                        ViewKind::Image {
                            display_mode: ImageMode::Fit,
                        },
                        "/camera/image_raw",
                        "sensor_msgs/msg/Image",
                    ),
                ],
            ),
            Self::Analyze => (
                "Analyze",
                vec![
                    view(
                        "plot",
                        ViewKind::NumericPlot {
                            field_path: vec![PathSegment::Field("data".into())],
                        },
                        "/demo/number",
                        "std_msgs/msg/Float64",
                    ),
                    view(
                        "messages",
                        ViewKind::Messages,
                        "/demo/status",
                        "std_msgs/msg/String",
                    ),
                ],
            ),
        };
        let layout = Layout::for_views(&views);
        Workspace {
            tools: None,
            presentation: None,
            version: FORMAT_VERSION,
            name: name.into(),
            panels: Panels {
                data_open: true,
                inspector_open: true,
                diagnostics_open: false,
            },
            layout,
            views,
            connection_suggestion: None,
        }
    }
}
