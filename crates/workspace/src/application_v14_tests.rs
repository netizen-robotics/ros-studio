use super::*;
fn fixture(updates: bool) -> Workspace {
    let mut w = Preset::Visualize.workspace();
    let ViewKind::Spatial {
        fixed_frame,
        transforms: Some(tf),
        ..
    } = &w.views[0].kind
    else {
        panic!("fixture")
    };
    w.views = vec![ViewConfig {
        id: "map".into(),
        topic: "/map".into(),
        message_type: studio_protocol::mapping::MAP_MESSAGE_TYPE.into(),
        qos: Qos {
            reliability: Reliability::Reliable,
            durability: Durability::TransientLocal,
        },
        kind: ViewKind::Map {
            fixed_frame: fixed_frame.clone(),
            transforms: tf.clone(),
            clock_expectation: ClockExpectation::System,
            updates: updates.then(|| crate::map::UpdateSource {
                topic: "/costmap_updates".into(),
                message_type: studio_protocol::map_grid::UPDATE_MESSAGE_TYPE.into(),
                qos: Qos {
                    reliability: Reliability::BestEffort,
                    durability: Durability::Volatile,
                },
            }),
            appearance: crate::map::Appearance {
                palette: crate::map::Palette::Costmap,
                opacity_per_mille: 735,
                draw_behind: true,
            },
        },
    }];
    w.layout = Layout::for_views(&w.views);
    w.presentation = Some(VisualizerPresentation::legacy(&w));
    w
}
#[test]
fn map_v14_roundtrip_is_configuration_only_and_requires_capability() {
    for updates in [false, true] {
        let w = fixture(updates);
        let bytes = encode(&w, Application::Visualizer).unwrap();
        let parsed = parse(&bytes, Application::Visualizer).unwrap();
        assert!(!parsed.legacy);
        assert_eq!(parsed.workspace, w);
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["version"], VISUALIZER_VERSION);
        let text = String::from_utf8(bytes.clone()).unwrap();
        for forbidden in [
            "cells",
            "subscription",
            "generation",
            "clock_verified",
            "revision",
            "enabled",
        ] {
            assert!(!text.contains(forbidden));
        }
        assert!(parse(&bytes, Application::Tools).is_err());
        assert!(Workspace::parse(&bytes).is_err());
        assert!(w.encode().is_err());
        let caps = SupportedRenderers {
            map: false,
            ..SupportedRenderers::all()
        };
        assert!(caps.validate_workspace(&w).is_err());
        for version in 8..14 {
            let mut old = wire.clone();
            old["version"] = version.into();
            assert!(
                parse(&serde_json::to_vec(&old).unwrap(), Application::Visualizer).is_err(),
                "{version}"
            );
        }
    }
}
#[test]
fn map_rejects_runtime_members_missing_nullable_source_bad_qos_and_aliases() {
    let wire: serde_json::Value =
        serde_json::from_slice(&encode(&fixture(true), Application::Visualizer).unwrap()).unwrap();
    for case in 0..14 {
        let mut bad = wire.clone();
        let kind = &mut bad["views"][0]["kind"];
        match case {
            0 => kind["cells"] = serde_json::json!([0]),
            1 => kind["clock_verified"] = true.into(),
            2 => {
                kind.as_object_mut().unwrap().remove("updates");
            }
            3 => kind["updates"]["message_type"] = "std_msgs/msg/String".into(),
            4 => kind["updates"]["topic"] = "relative".into(),
            5 => kind["updates"]["topic"] = "/map".into(),
            6 => kind["updates"]["topic"] = "/tf".into(),
            7 => kind["updates"]["topic"] = "/tf_static".into(),
            8 => kind["appearance"]["opacity_per_mille"] = 1001.into(),
            9 => kind["appearance"]["palette"] = "binary".into(),
            10 => kind["updates"]["qos"]["durability"] = "unknown".into(),
            11 => kind["updates"]["revision"] = 1.into(),
            12 => bad["views"][0]["topic"] = "/tf".into(),
            _ => bad["views"][0]["message_type"] = "map_msgs/msg/OccupancyGridUpdate".into(),
        }
        assert!(
            parse(&serde_json::to_vec(&bad).unwrap(), Application::Visualizer).is_err(),
            "case {case}"
        );
    }
    let text = serde_json::to_string(&wire).unwrap().replace(
        "\"opacity_per_mille\":735",
        "\"opacity_per_mille\":735,\"opacity_per_mille\":500",
    );
    assert!(parse(text.as_bytes(), Application::Visualizer).is_err());
}
#[test]
fn prior_map_grammars_import_as_inactive_defaults_without_changing_original() {
    let mut w = fixture(false);
    let ViewKind::Map {
        fixed_frame,
        transforms,
        clock_expectation,
        ..
    } = w.views[0].kind.clone()
    else {
        panic!()
    };
    w.views[0].kind = ViewKind::OccupancyGrid {
        fixed_frame,
        transforms,
        clock_expectation,
    };
    w.presentation = None;
    let legacy: serde_json::Value = serde_json::from_slice(&w.encode().unwrap()).unwrap();
    for version in 7..14 {
        let mut old = legacy.clone();
        old["version"] = version.into();
        if version >= 8 {
            old["application"] = "visualizer".into();
        }
        if version >= 9 {
            old["presentation"] = serde_json::to_value(VisualizerPresentation::legacy(&w)).unwrap();
            if version < 13 {
                old["presentation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("sensors");
            }
            if version < 12 {
                old["presentation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("robots");
            }
        }
        let original = serde_json::to_vec(&old).unwrap();
        let parsed = parse(&original, Application::Visualizer).unwrap();
        assert!(parsed.legacy, "{version}");
        assert!(
            matches!(&parsed.workspace.views[0].kind,ViewKind::Map {updates:None,appearance,..} if *appearance==crate::map::Appearance::default())
        );
        assert!(parsed.workspace.clone().inactive_restore().is_ok());
        assert_eq!(original, serde_json::to_vec(&old).unwrap());
        let current = encode(&parsed.workspace, Application::Visualizer).unwrap();
        assert!(!parse(&current, Application::Visualizer).unwrap().legacy);
    }
}
