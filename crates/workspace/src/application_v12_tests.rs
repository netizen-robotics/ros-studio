use super::*;
use crate::presentation::RobotPresentation;

fn fixture() -> Workspace {
    let mut workspace = Preset::Visualize.workspace();
    workspace.views.truncate(1);
    let ViewKind::Spatial {
        transforms: Some(tf),
        ..
    } = &workspace.views[0].kind
    else {
        panic!("spatial preset")
    };
    let tf = tf.clone();
    let view = &mut workspace.views[0];
    view.topic = tf.dynamic_tf.topic.clone();
    view.message_type = tf.dynamic_tf.message_type.clone();
    view.qos = tf.dynamic_tf.qos.clone();
    view.kind = ViewKind::Robot {
        assets: RobotAssets {
            urdf_path: "/reviewed/robot.urdf".into(),
            package_roots: vec![],
        },
        fixed_frame: "map".into(),
        transforms: tf,
        clock_expectation: ClockExpectation::System,
    };
    workspace.layout = Layout::for_views(&workspace.views);
    let mut presentation = VisualizerPresentation::legacy(&workspace);
    presentation.robots = vec![RobotPresentation {
        view: workspace.views[0].id.clone(),
        appearance: RobotAppearance {
            visual: false,
            collision: true,
            opacity_permille: 500,
            links: vec![LinkAppearance {
                name: "base".into(),
                visible: false,
                opacity_permille: 250,
            }],
        },
    }];
    workspace.presentation = Some(presentation);
    workspace
}
#[test]
fn v12_roundtrip_preserves_only_appearance_configuration_and_frozen_imports_default_it() {
    let workspace = fixture();
    let bytes = encode(&workspace, Application::Visualizer).unwrap();
    let parsed = parse(&bytes, Application::Visualizer).unwrap();
    assert!(!parsed.legacy);
    assert_eq!(parsed.workspace, workspace);
    assert!(parse(&bytes, Application::Tools).is_err());
    assert!(workspace.encode().is_err());
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["version"], VISUALIZER_VERSION);
    let mut v12 = wire.clone();
    v12["version"] = 12.into();
    v12.as_object_mut().unwrap().remove("shell");
    assert!(parse(&serde_json::to_vec(&v12).unwrap(), Application::Visualizer).is_err());
    v12["presentation"]
        .as_object_mut()
        .unwrap()
        .remove("sensors");
    let migrated = parse(&serde_json::to_vec(&v12).unwrap(), Application::Visualizer).unwrap();
    assert!(migrated.legacy);
    assert_eq!(migrated.workspace, workspace);
    for version in [9, 10, 11] {
        let mut old = v12.clone();
        old["version"] = version.into();
        old.as_object_mut().unwrap().remove("shell");
        assert!(parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err());
        old["presentation"]
            .as_object_mut()
            .unwrap()
            .remove("robots");
        let original = serde_json::to_vec(&old).unwrap();
        let imported = parse(&original, Application::Visualizer).unwrap();
        assert!(imported.legacy);
        assert!(imported.workspace.presentation.unwrap().robots.is_empty());
        assert_eq!(original, serde_json::to_vec(&old).unwrap());
    }
}
#[test]
fn v12_rejects_invalid_cross_references_names_opacity_duplicate_fields_and_runtime_state() {
    let wire: serde_json::Value =
        serde_json::from_slice(&encode(&fixture(), Application::Visualizer).unwrap()).unwrap();
    for case in 0..9 {
        let mut bad = wire.clone();
        let robot = &mut bad["presentation"]["robots"][0];
        match case {
            0 => robot["view"] = "missing".into(),
            1 => robot["appearance"]["opacity_permille"] = 1001.into(),
            2 => robot["appearance"]["links"][0]["name"] = "/invalid".into(),
            3 => robot["appearance"]["links"][0]["opacity_permille"] = (-1).into(),
            4 => robot["appearance"]["enabled"] = true.into(),
            5 => robot["appearance"]["links"][0]["pose"] = serde_json::json!([0, 0, 0]),
            6 => {
                let duplicate = robot.clone();
                bad["presentation"]["robots"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            7 => {
                let duplicate = robot["appearance"]["links"][0].clone();
                robot["appearance"]["links"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            _ => {
                bad["views"][0]["kind"] =
                    serde_json::json!({"renderer":"image","display_mode":"fit"});
                bad["views"][0]["topic"] = "/image".into();
                bad["views"][0]["message_type"] = "sensor_msgs/msg/Image".into();
            }
        }
        assert!(
            parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err(),
            "case {case}"
        );
    }
    let compact = serde_json::to_string(&wire).unwrap();
    let duplicate = compact.replace(
        "\"opacity_permille\":500",
        "\"opacity_permille\":500,\"opacity_permille\":900",
    );
    assert_ne!(compact, duplicate);
    assert!(parse(duplicate.as_bytes(), Application::Visualizer).is_err());
}
