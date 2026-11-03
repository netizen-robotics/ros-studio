//! V11 extends only the current Visualizer grammar; older adapters stay frozen.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Topic(TopicView),
    Previous(v10::View),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TopicView {
    id: String,
    topic: String,
    message_type: String,
    qos: Qos,
    kind: TopicKind,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum TopicKind {
    RobotTopic {
        description: RobotTopicSource,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::RobotTopic {
            description,
            fixed_frame,
            transforms,
            clock_expectation,
        } = &view.kind
        {
            Self::Topic(TopicView {
                id: view.id.clone(),
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
                kind: TopicKind::RobotTopic {
                    description: description.clone(),
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                },
            })
        } else {
            Self::Previous(v10::View::from(view))
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Previous(previous) => previous.into(),
            View::Topic(view) => {
                let TopicKind::RobotTopic {
                    description,
                    fixed_frame,
                    transforms,
                    clock_expectation,
                } = view.kind;
                Self {
                    id: view.id,
                    topic: view.topic,
                    message_type: view.message_type,
                    qos: view.qos,
                    kind: ViewKind::RobotTopic {
                        description,
                        fixed_frame,
                        transforms,
                        clock_expectation,
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Workspace {
        let mut workspace = Preset::Visualize.workspace();
        let ViewKind::Spatial {
            transforms: Some(tf),
            ..
        } = &workspace.views[0].kind
        else {
            panic!("TF fixture");
        };
        let transforms = tf.clone();
        let description = RobotTopicSource {
            topic: "/robot_description".into(),
            asset_root: Some("/reviewed/assets".into()),
            package_roots: vec![PackageRoot {
                package: "robot_description".into(),
                path: "/reviewed/package".into(),
            }],
            ..Default::default()
        };
        workspace.views = vec![ViewConfig {
            id: "robot".into(),
            topic: transforms.dynamic_tf.topic.clone(),
            message_type: "tf2_msgs/msg/TFMessage".into(),
            qos: transforms.dynamic_tf.qos.clone(),
            kind: ViewKind::RobotTopic {
                description,
                fixed_frame: "map".into(),
                transforms,
                clock_expectation: ClockExpectation::System,
            },
        }];
        workspace.layout = Layout::for_views(&workspace.views);
        workspace.presentation = Some(VisualizerPresentation::legacy(&workspace));
        workspace
    }
    #[test]
    fn topic_roundtrip_preserves_only_configuration_and_old_grammars_stay_frozen() {
        let workspace = fixture();
        let bytes = encode(&workspace, Application::Visualizer).unwrap();
        let parsed = parse(&bytes, Application::Visualizer).unwrap();
        assert!(!parsed.legacy);
        assert_eq!(parsed.workspace, workspace);
        assert_eq!(
            parsed.workspace.views[0].source_label(),
            "/robot_description"
        );
        assert!(parse(&bytes, Application::Tools).is_err());
        assert!(workspace.encode().is_err());
        let mut wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["version"], VISUALIZER_VERSION);
        assert_eq!(
            wire["views"][0]["kind"]["description"]["qos"]["durability"],
            "transient_local"
        );
        for version in [8, 9, 10, 11, 12] {
            wire["version"] = version.into();
            wire.as_object_mut().unwrap().remove("shell");
            assert!(
                parse(&serde_json::to_vec(&wire).unwrap(), Application::Visualizer).is_err(),
                "version {version}"
            );
        }
        wire["version"] = 7.into();
        wire.as_object_mut().unwrap().remove("application");
        wire.as_object_mut().unwrap().remove("presentation");
        assert!(Workspace::parse(&serde_json::to_vec(&wire).unwrap()).is_err());
        let mut capabilities = SupportedRenderers::all();
        capabilities.robot_topic = false;
        assert!(capabilities.validate_workspace(&workspace).is_err());
    }
    #[test]
    fn topic_schema_rejects_runtime_members_bad_type_roots_and_duplicate_fields() {
        let workspace = fixture();
        let wire: serde_json::Value =
            serde_json::from_slice(&encode(&workspace, Application::Visualizer).unwrap()).unwrap();
        for mode in 0..8 {
            let mut invalid = wire.clone();
            let source = &mut invalid["views"][0]["kind"]["description"];
            match mode {
                0 => source["xml"] = "<robot/>".into(),
                1 => source["subscription"] = 1.into(),
                2 => source["message_type"] = "example/msg/String".into(),
                3 => source["asset_root"] = "".into(),
                4 => source["topic"] = "relative".into(),
                5 => {
                    source["package_roots"] =
                        serde_json::json!([{"package":"a","path":"/a"},{"package":"a","path":"/b"}])
                }
                6 => source["qos"]["durability"] = "unknown".into(),
                _ => invalid["views"][0]["kind"]["clock_verified"] = true.into(),
            }
            assert!(
                parse(
                    &serde_json::to_vec(&invalid).unwrap(),
                    Application::Visualizer
                )
                .is_err(),
                "mode {mode}"
            );
        }
        let compact = serde_json::to_string(&wire).unwrap();
        assert!(parse(
            compact
                .replace(
                    "\"topic\":\"/robot_description\"",
                    "\"topic\":\"/robot_description\",\"topic\":\"/other\""
                )
                .as_bytes(),
            Application::Visualizer
        )
        .is_err());
    }
    #[test]
    fn v10_tf_camera_import_is_an_unsaved_copy_preserving_original_bytes() {
        let mut workspace = fixture();
        let ViewKind::RobotTopic {
            fixed_frame,
            transforms,
            clock_expectation,
            ..
        } = workspace.views[0].kind.clone()
        else {
            unreachable!();
        };
        workspace.views[0].kind = ViewKind::TfGraph {
            fixed_frame,
            transforms,
            clock_expectation,
        };
        let mut value: serde_json::Value =
            serde_json::from_slice(&encode(&workspace, Application::Visualizer).unwrap()).unwrap();
        value["version"] = 10.into();
        value.as_object_mut().unwrap().remove("shell");
        value["presentation"]
            .as_object_mut()
            .unwrap()
            .remove("sensors");
        value["presentation"]
            .as_object_mut()
            .unwrap()
            .remove("robots");
        let original = serde_json::to_vec(&value).unwrap();
        let imported = parse(&original, Application::Visualizer).unwrap();
        assert!(imported.legacy);
        assert_eq!(imported.workspace, workspace);
        assert_eq!(serde_json::to_vec(&value).unwrap(), original);
    }
}
