//! Pure configuration conversion shared by the App adapter and its tests.
use crate::components::workspace as file;
use studio_protocol::{
    spatial_input::SpatialClockPolicy,
    subscriptions::{Configuration, Output},
};

pub(super) fn empty_workspace() -> file::Workspace {
    let views = Vec::new();
    file::Workspace {
        tools: None,
        presentation: Some(studio_workspace::presentation::VisualizerPresentation::default()),
        version: file::FORMAT_VERSION,
        name: "Untitled workspace".into(),
        panels: file::Panels {
            data_open: true,
            inspector_open: true,
            diagnostics_open: false,
        },
        layout: file::Layout::for_views(&views),
        views,
        connection_suggestion: None,
    }
}
pub(super) fn qos(configuration: &Configuration) -> file::Qos {
    file::Qos {
        reliability: if configuration.best_effort {
            file::Reliability::BestEffort
        } else {
            file::Reliability::Reliable
        },
        durability: if configuration.transient_local {
            file::Durability::TransientLocal
        } else {
            file::Durability::Volatile
        },
    }
}
pub(super) fn configuration(view: &file::ViewConfig) -> Result<Configuration, String> {
    let robot = match &view.kind {
        file::ViewKind::RobotTopic {
            fixed_frame,
            transforms,
            ..
        }
        | file::ViewKind::Robot {
            fixed_frame,
            transforms,
            ..
        } => Some(Box::new(studio_protocol::robot::Configuration {
            fixed_frame: fixed_frame.clone(),
            frames: Vec::new(),
            clock_policy: SpatialClockPolicy::Unknown,
            dynamic_tf_topic: transforms.dynamic_tf.topic.clone(),
            dynamic_tf_best_effort: transforms.dynamic_tf.qos.reliability
                == file::Reliability::BestEffort,
            static_tf_topic: transforms.static_tf.topic.clone(),
        })),
        _ => None,
    };
    let spatial = match &view.kind {
        file::ViewKind::Spatial {
            fixed_frame,
            transforms,
            ..
        } => {
            let tf = transforms
                .as_ref()
                .ok_or("Spatial TF settings are incomplete")?;
            Some(studio_protocol::spatial::Configuration {
                fixed_frame: fixed_frame.clone(),
                // The stored expectation is deliberately not read as authorization.
                clock_policy: SpatialClockPolicy::Unknown,
                dynamic_tf_topic: tf.dynamic_tf.topic.clone(),
                dynamic_tf_best_effort: tf.dynamic_tf.qos.reliability
                    == file::Reliability::BestEffort,
                static_tf_topic: tf.static_tf.topic.clone(),
            })
        }
        file::ViewKind::TfGraph {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::PointCloud {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Camera {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Uncertainty {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Markers {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Map {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::OccupancyGrid {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Pose {
            fixed_frame,
            transforms: tf,
            ..
        }
        | file::ViewKind::Path {
            fixed_frame,
            transforms: tf,
            ..
        } => Some(studio_protocol::spatial::Configuration {
            fixed_frame: fixed_frame.clone(),
            clock_policy: SpatialClockPolicy::Unknown,
            dynamic_tf_topic: tf.dynamic_tf.topic.clone(),
            dynamic_tf_best_effort: tf.dynamic_tf.qos.reliability == file::Reliability::BestEffort,
            static_tf_topic: tf.static_tf.topic.clone(),
        }),
        _ => None,
    };
    Ok(Configuration {
        map_updates: match &view.kind {
            file::ViewKind::Map {
                updates: Some(source),
                ..
            } => Some(studio_protocol::map_grid::UpdateSource {
                topic: source.topic.clone(),
                best_effort: source.qos.reliability == file::Reliability::BestEffort,
                transient_local: source.qos.durability == file::Durability::TransientLocal,
            }),
            _ => None,
        },
        robot,
        topic: view.topic.clone(),
        message_type: view.message_type.clone(),
        best_effort: view.qos.reliability == file::Reliability::BestEffort,
        transient_local: view.qos.durability == file::Durability::TransientLocal,
        output: match &view.kind {
            file::ViewKind::TfGraph { .. } => Output::TfGraph,
            file::ViewKind::Camera { info, .. } => Output::CameraSnapshot(Box::new(info.clone())),
            file::ViewKind::Markers { .. } => Output::MarkerSnapshot,
            file::ViewKind::Uncertainty { configuration, .. } => {
                Output::PoseSnapshot(*configuration)
            }
            file::ViewKind::Image { .. } => Output::CompactImage,
            file::ViewKind::Spatial { .. } => Output::SpatialScan,
            file::ViewKind::Robot { .. } | file::ViewKind::RobotTopic { .. } => Output::Robot,
            file::ViewKind::PointCloud { .. } => Output::PointCloud,
            file::ViewKind::Map { .. } | file::ViewKind::OccupancyGrid { .. } => {
                Output::MapSnapshot
            }
            file::ViewKind::Pose { .. } => Output::Pose,
            file::ViewKind::Path { .. } => Output::Path,
            _ => Output::Typed,
        },
        spatial,
    })
}
/// Preserve stable display order, append new IDs, and retain the scene split.
pub(super) fn reconcile_layout(layout: &file::Layout, ids: &[String]) -> file::Layout {
    let mut order = Vec::new();
    for id in layout.order.iter().chain(ids.iter()) {
        if ids.contains(id) && !order.contains(id) {
            order.push(id.clone());
        }
    }
    file::Layout {
        order,
        scene_percent: layout.scene_percent.clamp(35, 80),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconcile_keeps_custom_order_and_split_without_removed_ids() {
        let old = file::Layout {
            order: vec!["custom".into(), "removed".into()],
            scene_percent: 73,
        };
        let ids = vec!["new".into(), "custom".into()];
        let next = reconcile_layout(&old, &ids);
        assert_eq!(next.order, ["custom", "new"]);
        assert_eq!(next.scene_percent, 73);
        assert_eq!(reconcile_layout(&next, &ids), next);
    }
    #[test]
    fn saved_spatial_expectation_never_authorizes_runtime_clock() {
        let config = configuration(&file::Preset::Visualize.workspace().views[0]).unwrap();
        assert_eq!(
            config.spatial.unwrap().clock_policy,
            SpatialClockPolicy::Unknown
        );
    }
    #[test]
    fn mapping_restore_preserves_exact_sources_and_qos_but_requires_fresh_clock_verification() {
        let visual = file::Preset::Visualize.workspace();
        let file::ViewKind::Spatial {
            transforms: Some(transforms),
            ..
        } = &visual.views[0].kind
        else {
            panic!("fixture TF")
        };
        for output in [Output::OccupancyGrid, Output::Pose, Output::Path] {
            let mut view = visual.views[0].clone();
            let fixed_frame = "mapping_world".to_owned();
            let transforms = transforms.clone();
            let clock_expectation = file::ClockExpectation::System;
            view.kind = match output {
                Output::OccupancyGrid => file::ViewKind::OccupancyGrid {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                },
                Output::Pose => file::ViewKind::Pose {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                },
                _ => file::ViewKind::Path {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                },
            };
            view.topic = "/mapping/source".into();
            view.message_type = match output {
                Output::OccupancyGrid => studio_protocol::mapping::MAP_MESSAGE_TYPE,
                Output::Pose => studio_protocol::mapping::POSE_MESSAGE_TYPE,
                _ => studio_protocol::mapping::PATH_MESSAGE_TYPE,
            }
            .into();
            view.qos = file::Qos {
                reliability: file::Reliability::Reliable,
                durability: if output == Output::OccupancyGrid {
                    file::Durability::TransientLocal
                } else {
                    file::Durability::Volatile
                },
            };
            let restored = configuration(&view).unwrap();
            assert_eq!(
                restored.output,
                if output == Output::OccupancyGrid {
                    Output::MapSnapshot
                } else {
                    output
                }
            );
            assert_eq!(restored.topic, view.topic);
            assert_eq!(restored.message_type, view.message_type);
            assert_eq!(qos(&restored), view.qos);
            assert!(restored.robot.is_none());
            let tf = restored.spatial.unwrap();
            assert_eq!(tf.fixed_frame, "mapping_world");
            assert_eq!(tf.clock_policy, SpatialClockPolicy::Unknown);
            assert_eq!(tf.dynamic_tf_topic, "/tf");
            assert_eq!(tf.static_tf_topic, "/tf_static");
        }
    }
    #[test]
    fn image_and_plot_use_distinct_outputs_and_exact_subscription_qos() {
        let visual = file::Preset::Visualize.workspace();
        assert_eq!(
            configuration(&visual.views[1]).unwrap().output,
            Output::CompactImage
        );
        let analyze = file::Preset::Analyze.workspace();
        let mut plot = analyze.views[0].clone();
        plot.qos.reliability = file::Reliability::Reliable;
        plot.qos.durability = file::Durability::TransientLocal;
        let config = configuration(&plot).unwrap();
        assert_eq!(config.output, Output::Typed);
        assert_eq!(qos(&config), plot.qos);
    }

    #[test]
    fn empty_document_is_valid_configuration_without_connection_suggestions() {
        let empty = empty_workspace();
        empty.validate().unwrap();
        assert!(empty.views.is_empty());
        assert!(empty.connection_suggestion.is_none());
    }
}
