use super::*;
use crate::{
    presentation::SensorPresentation,
    sensor::{ColorMode, SensorAppearance, SizeMode},
};
fn fixture() -> Workspace {
    let mut w = Preset::Visualize.workspace();
    w.views.truncate(1);
    w.layout = Layout::for_views(&w.views);
    let mut p = VisualizerPresentation::legacy(&w);
    p.sensors.push(SensorPresentation {
        view: w.views[0].id.clone(),
        appearance: SensorAppearance {
            history_ms: 1500,
            color: ColorMode::Intensity,
            size_mode: SizeMode::World,
            diameter_mm: 50,
            opacity: 128,
            automatic_range: false,
            intensity_milli: [-1000, 8000],
            ..Default::default()
        },
    });
    w.presentation = Some(p);
    w
}
#[test]
fn sensor_v13_roundtrips_configuration_and_frozen_versions_reject_new_fields() {
    let w = fixture();
    let bytes = encode(&w, Application::Visualizer).unwrap();
    let imported = parse(&bytes, Application::Visualizer).unwrap();
    assert!(!imported.legacy);
    assert_eq!(imported.workspace, w);
    assert!(parse(&bytes, Application::Tools).is_err());
    assert!(w.encode().is_err());
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["version"], VISUALIZER_VERSION);
    let text = String::from_utf8(bytes).unwrap();
    for forbidden in [
        "points",
        "received",
        "clock_verified",
        "subscription",
        "source_frame",
    ] {
        assert!(!text.contains(forbidden));
    }
    for version in [9, 10, 11, 12] {
        let mut old = wire.clone();
        old["version"] = version.into();
        old.as_object_mut().unwrap().remove("shell");
        assert!(parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err());
        old["presentation"]
            .as_object_mut()
            .unwrap()
            .remove("sensors");
        if version < 12 {
            old["presentation"]
                .as_object_mut()
                .unwrap()
                .remove("robots");
        }
        let original = serde_json::to_vec(&old).unwrap();
        let imported = parse(&original, Application::Visualizer).unwrap();
        assert!(imported.legacy);
        assert!(imported.workspace.presentation.unwrap().sensors.is_empty());
        assert_eq!(original, serde_json::to_vec(&old).unwrap());
    }
}
#[test]
fn sensor_settings_reject_orphans_duplicates_wrong_views_invalid_ranges_and_runtime_state() {
    let wire: serde_json::Value =
        serde_json::from_slice(&encode(&fixture(), Application::Visualizer).unwrap()).unwrap();
    for case in 0..9 {
        let mut bad = wire.clone();
        let setting = &mut bad["presentation"]["sensors"][0];
        match case {
            0 => setting["view"] = "missing".into(),
            1 => setting["appearance"]["history_ms"] = 10001.into(),
            2 => setting["appearance"]["pixels"] = 0.into(),
            3 => setting["appearance"]["height_mm"] = serde_json::json!([0, 0]),
            4 => setting["appearance"]["intensity_milli"] = serde_json::json!([i64::MIN, 1]),
            5 => setting["appearance"]["enabled"] = true.into(),
            6 => setting["samples"] = serde_json::json!([]),
            7 => {
                let duplicate = setting.clone();
                bad["presentation"]["sensors"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            _ => bad["views"][0]["kind"] = serde_json::json!({"renderer":"image"}),
        }
        assert!(
            parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err(),
            "case{case}"
        );
    }
}
