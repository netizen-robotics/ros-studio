use super::*;
fn frozen_encode(workspace: &Workspace, application: Application) -> Result<Vec<u8>, Error> {
    if application != Application::Visualizer {
        return encode(workspace, application);
    }
    serde_json::to_vec_pretty(&v10::Wire::<v17::View, VisualizerPresentation> {
        version: 17,
        application,
        name: workspace.name.clone(),
        panels: workspace.panels.clone(),
        layout: workspace.layout.clone(),
        views: workspace.views.iter().map(v17::View::from).collect(),
        connection_suggestion: workspace.connection_suggestion.clone(),
        presentation: workspace.presentation.clone().unwrap_or_default(),
    })
    .map_err(|e| Error::Invalid(e.to_string()))
}
fn camera() -> Workspace {
    let mut w = Preset::Visualize.workspace();
    let ViewKind::Spatial {
        fixed_frame,
        transforms: Some(tf),
        ..
    } = &w.views[0].kind
    else {
        panic!()
    };
    w.views = vec![ViewConfig {
        id: "camera".into(),
        topic: "/camera/image_rect".into(),
        message_type: studio_protocol::image::MESSAGE_TYPE.into(),
        qos: Qos {
            reliability: Reliability::BestEffort,
            durability: Durability::Volatile,
        },
        kind: ViewKind::Camera {
            info: studio_protocol::camera_snapshot::InfoSource {
                topic: "/camera/camera_info".into(),
                best_effort: false,
                transient_local: true,
            },
            appearance: crate::camera::Appearance {
                placement: crate::camera::ImagePlacement::Both,
                overlay_alpha_milli: 731,
                zoom_milli: 2000,
            },
            fixed_frame: fixed_frame.clone(),
            transforms: tf.clone(),
            clock_expectation: ClockExpectation::System,
        },
    }];
    w.layout = Layout::for_views(&w.views);
    w.presentation = Some(VisualizerPresentation::legacy(&w));
    w
}
#[test]
fn camera_v17_roundtrip_keeps_only_intent_and_rejects_every_older_or_other_app_grammar() {
    let w = camera();
    let bytes = frozen_encode(&w, Application::Visualizer).unwrap();
    let parsed = parse(&bytes, Application::Visualizer).unwrap();
    assert_eq!(parsed.workspace, w);
    assert!(parsed.legacy);
    assert!(parse(&bytes, Application::Tools).is_err());
    assert!(w.encode().is_err());
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["version"], 17);
    for version in 8..17 {
        let mut old = wire.clone();
        old["version"] = version.into();
        assert!(
            parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err(),
            "old grammar{version}"
        );
    }
    let text = String::from_utf8(bytes).unwrap();
    for key in [
        "subscription",
        "generation",
        "permission",
        "pixels",
        "snapshot",
        "clock_verified",
        "enabled",
    ] {
        assert!(!text.contains(key), "{key}");
    }
    let unsupported = SupportedRenderers {
        camera: false,
        ..SupportedRenderers::all()
    };
    assert!(unsupported.validate_workspace(&w).is_err());
}
#[test]
fn camera_v17_rejects_bad_source_pair_tf_type_bounds_and_extra_live_state() {
    let wire: serde_json::Value =
        serde_json::from_slice(&frozen_encode(&camera(), Application::Visualizer).unwrap())
            .unwrap();
    for key in [
        "info",
        "appearance",
        "fixed_frame",
        "transforms",
        "clock_expectation",
    ] {
        let mut bad = wire.clone();
        bad["views"][0]["kind"].as_object_mut().unwrap().remove(key);
        assert!(
            parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err(),
            "missing{key}"
        );
    }
    for topic in ["/camera/image_rect", "/tf", "/tf_static", "invalid", ""] {
        let mut bad = wire.clone();
        bad["views"][0]["kind"]["info"]["topic"] = topic.into();
        assert!(
            parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err(),
            "{topic}"
        );
    }
    for (key, value) in [
        ("zoom_milli", 0),
        ("zoom_milli", 10001),
        ("overlay_alpha_milli", 1001),
    ] {
        let mut bad = wire.clone();
        bad["views"][0]["kind"]["appearance"][key] = value.into();
        assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    }
    let mut bad = wire.clone();
    bad["views"][0]["message_type"] = "sensor_msgs/msg/CameraInfo".into();
    assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    let mut bad = wire.clone();
    bad["views"][0]["kind"]["live_snapshot"] = serde_json::json!({});
    assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    let mut bad = wire;
    bad["views"][0]["kind"]["info"]["extra"] = true.into();
    assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
}
#[test]
fn frozen_v16_uncertainty_still_migrates_without_accepting_camera() {
    let w = super::v16_tests::uncertainty();
    let bytes = super::v16_tests::encode(&w, Application::Visualizer).unwrap();
    let parsed = parse(&bytes, Application::Visualizer).unwrap();
    assert!(parsed.legacy);
    assert_eq!(parsed.workspace, w);
    let current = encode(&parsed.workspace, Application::Visualizer).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&current).unwrap()["version"],
        VISUALIZER_VERSION
    );
}
