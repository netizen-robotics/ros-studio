//! Configuration-only v5 persistence; parsing these tests must never load assets.
use serde_json::{json, Value};
use studio_desktop::workspace::{
    self, ClockExpectation, PackageRoot, Preset, RobotAssets, SupportedRenderers, ViewKind,
    Workspace,
};

fn tf(topic: &str, retained: bool) -> Value {
    json!({"topic":topic,"message_type":"tf2_msgs/msg/TFMessage","qos":{"reliability":"reliable","durability":if retained {"transient_local"} else {"volatile"}}})
}
fn robot_workspace() -> Value {
    json!({
        "version":5,"name":"Inactive robot 工作区","panels":{"data_open":true,"inspector_open":true,"diagnostics_open":false},
        "layout":{"order":["robot"],"scene_percent":65},"connection_suggestion":null,
        "views":[{"id":"robot","kind":{"renderer":"robot","assets":{"urdf_path":"/not-created/机器人.urdf","package_roots":[{"package":"fixture_robot","path":"/not-created/本地资源"}]},"fixed_frame":"map","transforms":{"dynamic_tf":tf("/tf",false),"static_tf":tf("/tf_static",true)},"clock_expectation":"system"},"topic":"/tf","message_type":"tf2_msgs/msg/TFMessage","qos":{"reliability":"reliable","durability":"volatile"}}]
    })
}
fn parse(value: &Value) -> Result<Workspace, workspace::Error> {
    Workspace::parse(&serde_json::to_vec(value).unwrap())
}
fn legacy(version: u32) -> Value {
    let mut value = robot_workspace();
    value["version"] = json!(version);
    value["views"][0] = json!({"id":"robot","kind":if version==1 {json!("messages")} else {json!({"renderer":"messages"})},"topic":"/status","message_type":"std_msgs/msg/String","qos":{"reliability":"reliable","durability":"volatile"}});
    if version == 1 {
        value.as_object_mut().unwrap().remove("layout");
    } else if version < 4 {
        value["layout"] =
            json!({"order":["robot"],"columns":1,"panel_weights":[100],"collapsed":[]});
    }
    value
}

#[test]
fn robot_v5_roundtrip_retains_only_configuration_and_restore_requires_no_asset_io() {
    assert_eq!(workspace::FORMAT_VERSION, 7);
    let document = parse(&robot_workspace()).unwrap();
    let encoded = document.encode().unwrap();
    assert_eq!(Workspace::parse(&encoded).unwrap(), document);
    let restored = document
        .clone()
        .inactive_restore()
        .unwrap()
        .into_configuration();
    assert_eq!(restored, document);
    let ViewKind::Robot {
        assets,
        fixed_frame,
        clock_expectation,
        ..
    } = &restored.views[0].kind
    else {
        panic!("Robot kind lost")
    };
    assert_eq!(
        assets,
        &RobotAssets {
            urdf_path: "/not-created/机器人.urdf".into(),
            package_roots: vec![PackageRoot {
                package: "fixture_robot".into(),
                path: "/not-created/本地资源".into()
            }]
        }
    );
    assert_eq!(fixed_frame, "map");
    assert_eq!(
        *clock_expectation,
        ClockExpectation::System,
        "clock expectation survives only as intent"
    );
    SupportedRenderers::all()
        .validate_workspace(&restored)
        .unwrap();
    let mut unsupported = SupportedRenderers::all();
    unsupported.robot = false;
    assert!(unsupported.validate_workspace(&restored).is_err());
    let wire: Value = serde_json::from_slice(&encoded).unwrap();
    let fields = wire["views"][0]["kind"].as_object().unwrap();
    assert_eq!(fields.len(), 5);
    for forbidden in [
        "frames",
        "poses",
        "loaded_bytes",
        "model_id",
        "activation",
        "clock_verified",
    ] {
        assert!(!fields.contains_key(forbidden));
    }
}

#[test]
fn strict_v5_rejects_runtime_and_unknown_fields_at_each_configuration_boundary() {
    for path in [
        vec!["enabled"],
        vec!["kind", "model_id"],
        vec!["kind", "frames"],
        vec!["kind", "poses"],
        vec!["kind", "clock_verified"],
        vec!["kind", "assets", "loaded_bytes"],
        vec!["kind", "assets", "activation"],
        vec!["kind", "assets", "package_roots", "session_id"],
    ] {
        let mut value = robot_workspace();
        if path == ["kind", "assets", "package_roots", "session_id"] {
            value["views"][0]["kind"]["assets"]["package_roots"][0]["session_id"] = json!(7);
        } else {
            let mut target = &mut value["views"][0];
            for key in &path[..path.len() - 1] {
                target = &mut target[*key];
            }
            target[path[path.len() - 1]] = json!(7);
        }
        assert!(
            parse(&value).is_err(),
            "runtime/unknown field accepted at {path:?}"
        );
    }
}

#[test]
fn frozen_legacy_grammars_migrate_ordinary_views_and_reject_robot_even_with_old_version_number() {
    for version in 1..=4 {
        let migrated = parse(&legacy(version)).unwrap();
        assert_eq!(migrated.version, 7);
        assert!(matches!(migrated.views[0].kind, ViewKind::Messages));
        assert_eq!(migrated.layout.order, ["robot"]);
        let mut value = legacy(version);
        value["views"][0] = robot_workspace()["views"][0].clone();
        assert!(
            parse(&value).is_err(),
            "Robot cannot be smuggled into format{version}"
        );
        let mut value = legacy(version);
        value["views"][0]["enabled"] = json!(true);
        assert!(
            parse(&value).is_err(),
            "legacy format{version} must remain strict"
        );
    }
}

#[test]
fn v4_spatial_migration_preserves_tf_clock_order_and_rejects_bad_layout_before_conversion() {
    let mut value = legacy(4);
    let view = &mut value["views"][0];
    view["topic"] = json!("/scan");
    view["message_type"] = json!("sensor_msgs/msg/LaserScan");
    view["kind"] = json!({"renderer":"spatial","fixed_frame":"map","layers":[{"id":"scan","kind":"laser_scan","topic":"/scan","message_type":"sensor_msgs/msg/LaserScan","qos":{"reliability":"reliable","durability":"volatile"}}],"transforms":{"dynamic_tf":tf("/tf",false),"static_tf":tf("/tf_static",true)},"clock_expectation":"system"});
    let migrated = parse(&value).unwrap();
    assert_eq!(migrated.version, 7);
    assert!(matches!(
        migrated.views[0].kind,
        ViewKind::Spatial {
            clock_expectation: ClockExpectation::System,
            ..
        }
    ));
    assert_eq!(migrated.layout.order, ["robot"]);
    let mut duplicate = value.clone();
    duplicate["layout"]["order"] = json!(["robot", "robot"]);
    assert!(parse(&duplicate).is_err());
    let mut unknown = value;
    unknown["layout"]["order"] = json!(["unknown"]);
    assert!(parse(&unknown).is_err());
}

#[test]
fn utf8_asset_paths_package_tokens_and_mapping_counts_are_bounded_without_filesystem_lookup() {
    let mut value = robot_workspace();
    let exact = format!("/{}", "界".repeat(1365));
    assert_eq!(exact.len(), 4096);
    value["views"][0]["kind"]["assets"]["urdf_path"] = json!(exact);
    assert!(parse(&value).is_ok());
    value["views"][0]["kind"]["assets"]["urdf_path"] = json!(format!("/{}a", "界".repeat(1365)));
    assert!(parse(&value).is_err());
    for invalid in ["", "bad\npath", "bad\0path"] {
        let mut value = robot_workspace();
        value["views"][0]["kind"]["assets"]["urdf_path"] = json!(invalid);
        assert!(parse(&value).is_err());
        let mut value = robot_workspace();
        value["views"][0]["kind"]["assets"]["package_roots"][0]["path"] = json!(invalid);
        assert!(parse(&value).is_err());
    }
    for name in ["", "1bad", "has/slash", "has space", "package-name"] {
        let mut value = robot_workspace();
        value["views"][0]["kind"]["assets"]["package_roots"][0]["package"] = json!(name);
        assert!(parse(&value).is_err());
    }
    let mut value = robot_workspace();
    value["views"][0]["kind"]["assets"]["package_roots"] = json!((0..16)
        .map(|i| json!({"package":format!("pkg{i}"),"path":format!("/absent/{i}")}))
        .collect::<Vec<_>>());
    assert!(parse(&value).is_ok());
    value["views"][0]["kind"]["assets"]["package_roots"]
        .as_array_mut()
        .unwrap()
        .push(json!({"package":"pkg16","path":"/absent/16"}));
    assert!(parse(&value).is_err());
    let mut value = robot_workspace();
    let root = value["views"][0]["kind"]["assets"]["package_roots"][0].clone();
    value["views"][0]["kind"]["assets"]["package_roots"]
        .as_array_mut()
        .unwrap()
        .push(root);
    assert!(parse(&value).is_err());
}

#[test]
fn robot_workspace_rejects_contradictory_topic_type_and_tf_qos() {
    for (path, replacement) in [
        (vec!["topic"], json!("/other_tf")),
        (vec!["message_type"], json!("std_msgs/msg/String")),
        (vec!["qos", "reliability"], json!("best_effort")),
        (vec!["qos", "durability"], json!("transient_local")),
        (vec!["kind", "fixed_frame"], json!("/map")),
        (
            vec!["kind", "transforms", "dynamic_tf", "message_type"],
            json!("std_msgs/msg/String"),
        ),
        (
            vec!["kind", "transforms", "dynamic_tf", "qos", "durability"],
            json!("transient_local"),
        ),
        (
            vec!["kind", "transforms", "static_tf", "topic"],
            json!("/tf"),
        ),
        (
            vec!["kind", "transforms", "static_tf", "message_type"],
            json!("std_msgs/msg/String"),
        ),
        (
            vec!["kind", "transforms", "static_tf", "qos", "reliability"],
            json!("best_effort"),
        ),
        (
            vec!["kind", "transforms", "static_tf", "qos", "durability"],
            json!("volatile"),
        ),
    ] {
        let mut value = robot_workspace();
        let mut target = &mut value["views"][0];
        for key in &path[..path.len() - 1] {
            target = &mut target[*key];
        }
        target[path[path.len() - 1]] = replacement;
        assert!(
            parse(&value).is_err(),
            "Robot configuration contradiction accepted at {path:?}"
        );
    }
}

#[test]
fn legal_individual_escaped_paths_cannot_bypass_aggregate_file_budget() {
    let mut document = parse(&robot_workspace()).unwrap();
    let ViewKind::Robot { assets, .. } = &mut document.views[0].kind else {
        panic!("Robot")
    };
    assets.package_roots = (0..16)
        .map(|i| PackageRoot {
            package: format!("pkg{i}"),
            path: "\\".repeat(4096),
        })
        .collect();
    document.validate().unwrap();
    assert!(serde_json::to_vec(&document).unwrap().len() > workspace::MAX_FILE_BYTES);
    assert!(
        document.encode().is_err(),
        "format cap applies to the whole encoded document"
    );
}

#[test]
fn four_robot_configurations_remain_inactive_drafts_but_fifth_is_rejected() {
    let mut value = robot_workspace();
    let template = value["views"][0].clone();
    value["views"] = json!((0..4)
        .map(|i| {
            let mut view = template.clone();
            view["id"] = json!(format!("robot{i}"));
            view
        })
        .collect::<Vec<_>>());
    value["layout"]["order"] = json!(["robot0", "robot1", "robot2", "robot3"]);
    let document = parse(&value).unwrap();
    assert_eq!(
        document
            .inactive_restore()
            .unwrap()
            .configuration()
            .views
            .len(),
        4
    );
    let mut fifth = template;
    fifth["id"] = json!("robot4");
    value["views"].as_array_mut().unwrap().push(fifth);
    value["layout"]["order"]
        .as_array_mut()
        .unwrap()
        .push(json!("robot4"));
    assert!(parse(&value).is_err());
}

#[test]
fn presets_remain_inactive_current_format_without_inventing_robot_inputs() {
    for preset in [Preset::Inspect, Preset::Visualize, Preset::Analyze] {
        let document = preset.workspace();
        assert_eq!(document.version, 7);
        let restored = Workspace::parse(&document.encode().unwrap())
            .unwrap()
            .inactive_restore()
            .unwrap()
            .into_configuration();
        assert_eq!(restored, document);
        assert!(
            document
                .views
                .iter()
                .all(|view| !matches!(view.kind, ViewKind::Robot { .. })),
            "existing presets cannot invent a local model path"
        );
    }
}
