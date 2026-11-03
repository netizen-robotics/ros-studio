use super::*;
fn markers() -> Workspace {
    let mut w = Preset::Visualize.workspace();
    let ViewKind::Spatial {
        fixed_frame,
        transforms: Some(transforms),
        ..
    } = &w.views[0].kind
    else {
        panic!()
    };
    w.views = vec![ViewConfig {
        id: "markers".into(),
        topic: "/objects".into(),
        message_type: studio_protocol::marker::ARRAY_MESSAGE_TYPE.into(),
        qos: Qos {
            reliability: Reliability::Reliable,
            durability: Durability::TransientLocal,
        },
        kind: ViewKind::Markers {
            fixed_frame: fixed_frame.clone(),
            transforms: transforms.clone(),
            clock_expectation: ClockExpectation::System,
        },
    }];
    w.layout = Layout::for_views(&w.views);
    w.presentation = Some(VisualizerPresentation::legacy(&w));
    w
}
#[test]
fn markers_v15_strict_roundtrip_and_app_legacy_boundaries() {
    for ty in [
        studio_protocol::marker::MESSAGE_TYPE,
        studio_protocol::marker::ARRAY_MESSAGE_TYPE,
    ] {
        let mut w = markers();
        w.views[0].message_type = ty.into();
        let bytes = encode(&w, Application::Visualizer).unwrap();
        let parsed = parse(&bytes, Application::Visualizer).unwrap();
        assert!(!parsed.legacy);
        assert_eq!(parsed.workspace, w);
        assert!(parse(&bytes, Application::Tools).is_err());
        assert!(Workspace::parse(&bytes).is_err());
        assert!(w.encode().is_err());
        assert!(encode(&w, Application::Tools).is_err());
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["version"], VISUALIZER_VERSION);
        for version in 8..15 {
            let mut old = wire.clone();
            old["version"] = version.into();
            old.as_object_mut().unwrap().remove("shell");
            assert!(
                parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err(),
                "old version{version} accepted Markers"
            );
        }
        for field in ["fixed_frame", "transforms", "clock_expectation"] {
            let mut missing = wire.clone();
            missing["views"][0]["kind"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(parse(
                &serde_json::to_vec(&missing).unwrap(),
                Application::Visualizer
            )
            .is_err());
        }
        for (field, value) in [
            ("asset_approved", serde_json::json!(true)),
            ("source_epoch", serde_json::json!(1)),
            ("resource_root", serde_json::json!("/tmp")),
        ] {
            let mut extra = wire.clone();
            extra["views"][0]["kind"][field] = value;
            assert!(parse(
                &serde_json::to_vec(&extra).unwrap(),
                Application::Visualizer
            )
            .is_err());
        }
    }
}
#[test]
fn marker_validation_and_v14_migration_remain_explicit() {
    let mut w = markers();
    w.views[0].message_type = "nav_msgs/msg/Path".into();
    assert!(encode(&w, Application::Visualizer).is_err());
    let mut w = markers();
    let topic = w.views[0].topic.clone();
    let ViewKind::Markers { transforms, .. } = &mut w.views[0].kind else {
        panic!()
    };
    transforms.dynamic_tf.topic = topic;
    assert!(encode(&w, Application::Visualizer).is_err());
    let old = Preset::Visualize.workspace();
    let mut wire: serde_json::Value =
        serde_json::from_slice(&encode(&old, Application::Visualizer).unwrap()).unwrap();
    wire["version"] = 14.into();
    wire.as_object_mut().unwrap().remove("shell");
    let imported = parse(&serde_json::to_vec(&wire).unwrap(), Application::Visualizer).unwrap();
    assert!(imported.legacy);
    assert_eq!(imported.workspace.views, old.views);
    let tools = Preset::Analyze.workspace();
    let wire: serde_json::Value =
        serde_json::from_slice(&encode(&tools, Application::Tools).unwrap()).unwrap();
    assert_eq!(wire["version"], crate::tools::VERSION);
}
