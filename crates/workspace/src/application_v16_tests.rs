use super::*;
use studio_protocol::pose_snapshot::{
    transport::{Configuration, HistoryConfiguration},
    Kind,
};
pub(super) fn encode(workspace: &Workspace, application: Application) -> Result<Vec<u8>, Error> {
    if application != Application::Visualizer {
        return super::encode(workspace, application);
    }
    workspace.validate()?;
    serde_json::to_vec_pretty(&v10::Wire::<v16::View, VisualizerPresentation> {
        version: 16,
        application,
        name: workspace.name.clone(),
        panels: workspace.panels.clone(),
        layout: workspace.layout.clone(),
        views: workspace.views.iter().map(v16::View::from).collect(),
        connection_suggestion: workspace.connection_suggestion.clone(),
        presentation: workspace
            .presentation
            .clone()
            .unwrap_or_else(|| VisualizerPresentation::legacy(workspace)),
    })
    .map_err(|e| Error::Invalid(e.to_string()))
}
pub(super) fn uncertainty() -> Workspace {
    let mut w = Preset::Visualize.workspace();
    let ViewKind::Spatial {
        fixed_frame,
        transforms: Some(tf),
        ..
    } = &w.views[0].kind
    else {
        panic!()
    };
    let (fixed_frame, tf) = (fixed_frame.clone(), tf.clone());
    w.views = [Kind::PoseWithCovariance, Kind::Odometry]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| ViewConfig {
            id: format!("pose_{i}"),
            topic: format!("/pose_{i}"),
            message_type: kind.message_type().into(),
            qos: Qos {
                reliability: Reliability::Reliable,
                durability: Durability::TransientLocal,
            },
            kind: ViewKind::Uncertainty {
                configuration: Configuration::for_kind(kind),
                appearance: crate::uncertainty::Appearance {
                    history_covariance: true,
                    ..Default::default()
                },
                fixed_frame: fixed_frame.clone(),
                transforms: tf.clone(),
                clock_expectation: ClockExpectation::System,
            },
        })
        .collect();
    w.layout = Layout::for_views(&w.views);
    w.presentation = Some(VisualizerPresentation::legacy(&w));
    w
}
#[test]
fn v16_source_history_appearance_roundtrip_excludes_live_data_and_other_apps() {
    let mut w = uncertainty();
    let ViewKind::Uncertainty {
        configuration,
        appearance,
        ..
    } = &mut w.views[1].kind
    else {
        panic!()
    };
    configuration.history = Some(HistoryConfiguration {
        keep: 256,
        position_tolerance_um: 1,
        angle_tolerance_urad: 3141592,
    });
    appearance.sigma_milli = 6000;
    let encoded = encode(&w, Application::Visualizer).unwrap();
    let parsed = parse(&encoded, Application::Visualizer).unwrap();
    assert_eq!(parsed.workspace, w);
    assert!(parsed.legacy);
    assert!(parse(&encoded, Application::Tools).is_err());
    assert!(w.encode().is_err());
    let text = String::from_utf8(encoded.clone()).unwrap();
    for key in [
        "\"generation\"",
        "\"subscription\"",
        "\"latest\"",
        "\"trail\"",
        "\"permission\"",
        "\"acquisition_stamp_ns\"",
    ] {
        assert!(!text.contains(key), "Unexpected live field {key}");
    }
    let wire: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(wire["version"], 16);
    for version in 8..16 {
        let mut old = wire.clone();
        old["version"] = version.into();
        assert!(
            parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err(),
            "old grammar{version} accepted uncertainty"
        );
    }
    let mut wrong = wire;
    wrong["application"] = "tools".into();
    assert!(parse(&serde_json::to_vec(&wrong).unwrap(), Application::Tools).is_err());
}
#[test]
fn v16_rejects_bad_type_history_appearance_unknown_and_missing_fields() {
    let wire: serde_json::Value =
        serde_json::from_slice(&encode(&uncertainty(), Application::Visualizer).unwrap()).unwrap();
    for key in [
        "configuration",
        "appearance",
        "fixed_frame",
        "transforms",
        "clock_expectation",
    ] {
        let mut bad = wire.clone();
        bad["views"][0]["kind"].as_object_mut().unwrap().remove(key);
        assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    }
    for (key, value) in [
        ("keep", serde_json::json!(257)),
        ("position_tolerance_um", serde_json::json!(0.5)),
        ("angle_tolerance_urad", serde_json::json!(3141593)),
    ] {
        let mut bad = wire.clone();
        bad["views"][1]["kind"]["configuration"]["history"][key] = value;
        assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    }
    for (key, value) in [
        ("sigma_milli", serde_json::json!(0)),
        ("line_width_px", serde_json::json!(9)),
        ("pose_length_mm", serde_json::json!(5001)),
        ("extra", serde_json::json!(true)),
    ] {
        let mut bad = wire.clone();
        bad["views"][0]["kind"]["appearance"][key] = value;
        assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    }
    let mut bad = wire.clone();
    bad["views"][0]["message_type"] = "nav_msgs/msg/Odometry".into();
    assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
    let mut bad = wire;
    bad["views"][0]["kind"]["live_snapshot"] = serde_json::json!({});
    assert!(parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err());
}
#[test]
fn frozen_v15_ordinary_displays_migrate_and_unsupported_restore_is_explicit() {
    let old = Preset::Visualize.workspace();
    let mut wire: serde_json::Value =
        serde_json::from_slice(&encode(&old, Application::Visualizer).unwrap()).unwrap();
    wire["version"] = 15.into();
    let parsed = parse(&serde_json::to_vec(&wire).unwrap(), Application::Visualizer).unwrap();
    assert!(parsed.legacy);
    assert_eq!(parsed.workspace.views, old.views);
    let saved = uncertainty();
    let unsupported = SupportedRenderers {
        uncertainty: false,
        ..SupportedRenderers::all()
    };
    assert!(unsupported.validate_workspace(&saved).is_err());
    SupportedRenderers::all()
        .validate_workspace(&saved)
        .unwrap();
}
