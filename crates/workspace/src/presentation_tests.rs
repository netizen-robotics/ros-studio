use crate::{
    application::{self, Application},
    presentation::*,
    workspace::*,
};

fn fixture() -> Workspace {
    let mut workspace = Preset::Inspect.workspace();
    workspace.views.clear();
    workspace.layout = Layout::for_views(&[]);
    workspace.presentation = Some(VisualizerPresentation {
        shell: Default::default(),
        scene: Some(SceneSettings {
            fixed_frame: "map".into(),
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
            dynamic_tf_best_effort: true,
        }),
        camera: CameraState {
            mode: CameraMode::TopDown,
            target_mm: [1234, -5678, 90],
            vertical_span_mm: 23456,
            ..Default::default()
        },
        saved_views: vec![SavedView {
            name: "Workshop top".into(),
            camera: CameraState::default(),
        }],
        robots: Vec::new(),
        sensors: Vec::new(),
    });
    workspace
}
#[test]
fn v13_roundtrips_empty_and_image_workspaces_without_clock_authorization() {
    for image in [false, true] {
        let mut workspace = fixture();
        if image {
            workspace.views = Preset::Visualize
                .workspace()
                .views
                .into_iter()
                .filter(|v| matches!(v.kind, ViewKind::Image { .. }))
                .collect();
            workspace.layout = Layout::for_views(&workspace.views);
        }
        let bytes = application::encode(&workspace, Application::Visualizer).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
            application::VISUALIZER_VERSION
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("verified"));
        let imported = application::parse(&bytes, Application::Visualizer).unwrap();
        assert!(!imported.legacy);
        assert_eq!(imported.workspace, workspace);
        assert!(application::parse(&bytes, Application::Tools).is_err());
        assert!(workspace.encode().is_err());
    }
}
#[test]
fn strict_legacy_grammars_reject_presentation_and_v8_migrates_without_mutation() {
    let mut workspace = fixture();
    workspace.presentation = None;
    let old = workspace.encode().unwrap();
    let mut v7: serde_json::Value = serde_json::from_slice(&old).unwrap();
    v7["presentation"] = serde_json::to_value(VisualizerPresentation::default()).unwrap();
    assert!(Workspace::parse(&serde_json::to_vec(&v7).unwrap()).is_err());
    let mut v8: serde_json::Value = serde_json::from_slice(&old).unwrap();
    v8["version"] = 8.into();
    v8["application"] = "visualizer".into();
    let bytes = serde_json::to_vec(&v8).unwrap();
    let imported = application::parse(&bytes, Application::Visualizer).unwrap();
    assert!(imported.legacy);
    assert_eq!(
        imported.workspace.presentation,
        Some(VisualizerPresentation::default())
    );
    assert_eq!(old, workspace.encode().unwrap());
    v8["presentation"] = serde_json::to_value(VisualizerPresentation::default()).unwrap();
    assert!(
        application::parse(&serde_json::to_vec(&v8).unwrap(), Application::Visualizer).is_err()
    );
}
#[test]
fn v11_rejects_unknown_runtime_members_bounds_duplicate_views_and_invalid_numbers() {
    let original: serde_json::Value =
        serde_json::from_slice(&application::encode(&fixture(), Application::Visualizer).unwrap())
            .unwrap();
    for (pointer, replacement) in [
        ("/presentation/camera/distance_mm", serde_json::json!(0)),
        (
            "/presentation/camera/target_mm",
            serde_json::json!([1_000_000_001i64, 0, 0]),
        ),
        (
            "/presentation/camera/azimuth_microrad",
            serde_json::json!(-1),
        ),
        (
            "/presentation/camera/vertical_span_mm",
            serde_json::json!(2_000_001),
        ),
        (
            "/presentation/camera/distance_mm",
            serde_json::json!(12000.5),
        ),
        (
            "/presentation/scene/static_tf_topic",
            serde_json::json!("/tf"),
        ),
        (
            "/presentation/saved_views",
            serde_json::json!([{"name":"same","camera":CameraState::default()},{"name":"same","camera":CameraState::default()}]),
        ),
        (
            "/presentation/saved_views",
            serde_json::json!((0..9)
                .map(|i| SavedView {
                    name: i.to_string(),
                    camera: CameraState::default()
                })
                .collect::<Vec<_>>()),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            application::parse(
                &serde_json::to_vec(&value).unwrap(),
                Application::Visualizer
            )
            .is_err(),
            "Accepted invalid {pointer}"
        );
    }
    for pointer in [
        "/presentation",
        "/presentation/scene",
        "/presentation/camera",
    ] {
        let mut value = original.clone();
        value.pointer_mut(pointer).unwrap()["clock_verified"] = true.into();
        assert!(application::parse(
            &serde_json::to_vec(&value).unwrap(),
            Application::Visualizer
        )
        .is_err());
    }
    let encoded = String::from_utf8(serde_json::to_vec(&original).unwrap()).unwrap();
    for invalid in ["NaN", "Infinity", "-Infinity"] {
        let bytes = encoded.replace(
            "\"distance_mm\":12000",
            &format!("\"distance_mm\":{invalid}"),
        );
        assert!(application::parse(bytes.as_bytes(), Application::Visualizer).is_err());
    }
    let version = original["version"].as_u64().unwrap();
    let member = format!("\"version\":{version}");
    assert!(encoded.contains(&member));
    let duplicate = encoded.replacen(&member, &format!("{member},{member}"), 1);
    assert!(application::parse(duplicate.as_bytes(), Application::Visualizer).is_err());
    assert!(application::parse(&vec![b' '; MAX_FILE_BYTES + 1], Application::Visualizer).is_err());
}

#[test]
fn v11_tf_roundtrip_frozen_legacy_rejection_and_v9_copy_migration() {
    let mut workspace = fixture();
    let tf = SpatialTransforms {
        dynamic_tf: TransformInput {
            topic: "/tf".into(),
            message_type: "tf2_msgs/msg/TFMessage".into(),
            qos: Qos {
                reliability: Reliability::Reliable,
                durability: Durability::Volatile,
            },
        },
        static_tf: TransformInput {
            topic: "/tf_static".into(),
            message_type: "tf2_msgs/msg/TFMessage".into(),
            qos: Qos {
                reliability: Reliability::Reliable,
                durability: Durability::TransientLocal,
            },
        },
    };
    workspace.views.push(ViewConfig {
        id: "frames".into(),
        kind: ViewKind::TfGraph {
            fixed_frame: "map".into(),
            transforms: tf,
            clock_expectation: ClockExpectation::System,
        },
        topic: "/tf".into(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        qos: Qos {
            reliability: Reliability::Reliable,
            durability: Durability::Volatile,
        },
    });
    workspace.layout = Layout::for_views(&workspace.views);
    let bytes = application::encode(&workspace, Application::Visualizer).unwrap();
    let imported = application::parse(&bytes, Application::Visualizer).unwrap();
    assert!(!imported.legacy);
    assert_eq!(imported.workspace, workspace);
    assert!(application::parse(&bytes, Application::Tools).is_err());
    assert!(workspace.encode().is_err());
    let mut raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    raw["presentation"]
        .as_object_mut()
        .unwrap()
        .remove("sensors");
    for version in [7, 8, 9] {
        raw["version"] = version.into();
        if version == 7 {
            let mut old = raw.clone();
            old.as_object_mut().unwrap().remove("application");
            old.as_object_mut().unwrap().remove("presentation");
            assert!(Workspace::parse(&serde_json::to_vec(&old).unwrap()).is_err());
        } else {
            assert!(application::parse(
                &serde_json::to_vec(&raw).unwrap(),
                Application::Visualizer
            )
            .is_err());
        }
    }
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&application::encode(&fixture(), Application::Visualizer).unwrap())
            .unwrap();
    legacy["version"] = 9.into();
    legacy.as_object_mut().unwrap().remove("shell");
    legacy["presentation"]
        .as_object_mut()
        .unwrap()
        .remove("sensors");
    legacy["presentation"]
        .as_object_mut()
        .unwrap()
        .remove("robots");
    let original = serde_json::to_vec(&legacy).unwrap();
    let migrated = application::parse(&original, Application::Visualizer).unwrap();
    assert!(migrated.legacy);
    assert_eq!(migrated.workspace, fixture());
    assert_eq!(serde_json::to_vec(&legacy).unwrap(), original);
    let mut raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    raw["presentation"]
        .as_object_mut()
        .unwrap()
        .remove("sensors");
    raw["views"][0]["kind"]["clock_verified"] = true.into();
    assert!(
        application::parse(&serde_json::to_vec(&raw).unwrap(), Application::Visualizer).is_err()
    );
    let mut invalid = workspace.clone();
    invalid.views[0].qos.reliability = Reliability::BestEffort;
    assert!(application::encode(&invalid, Application::Visualizer).is_err());
    let mut capabilities = SupportedRenderers::all();
    capabilities.tf_graph = false;
    assert!(capabilities.validate_workspace(&workspace).is_err());
}
