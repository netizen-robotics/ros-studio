//! Independent v6 persistence checks; no asset I/O or live subscriptions.
use serde_json::{json, Value};
use studio_desktop::workspace::{
    self, ClockExpectation, Preset, SupportedRenderers, ViewKind, Workspace,
};
fn tf(topic: &str, statics: bool) -> Value {
    json!({"topic":topic,"message_type":"tf2_msgs/msg/TFMessage","qos":{"reliability":"reliable","durability":if statics {"transient_local"}else{"volatile"}}})
}
fn cloud(id: &str) -> Value {
    json!({"id":id,"kind":{"renderer":"point_cloud","fixed_frame":"map","transforms":{"dynamic_tf":tf("/tf",false),"static_tf":tf("/tf_static",true)},"clock_expectation":"system"},"topic":"/sensor_scene/cloud","message_type":"sensor_msgs/msg/PointCloud2","qos":{"reliability":"best_effort","durability":"volatile"}})
}
fn document(version: u32, views: Vec<Value>) -> Value {
    let order = views.iter().map(|v| v["id"].clone()).collect::<Vec<_>>();
    json!({"version":version,"name":"Configuration only","panels":{"data_open":true,"inspector_open":true,"diagnostics_open":false},"layout":{"order":order,"scene_percent":65},"views":views,"connection_suggestion":null})
}
fn parse(value: &Value) -> Result<Workspace, workspace::Error> {
    Workspace::parse(&serde_json::to_vec(value).unwrap())
}
fn ordinary(id: &str, version: u32) -> Value {
    json!({"id":id,"kind":if version==1{json!("messages")}else{json!({"renderer":"messages"})},"topic":"/status","message_type":"std_msgs/msg/String","qos":{"reliability":"reliable","durability":"volatile"}})
}
fn legacy(version: u32, count: usize) -> Value {
    let mut doc = document(
        version,
        (0..count)
            .map(|i| ordinary(&format!("view{i}"), version))
            .collect(),
    );
    if version == 1 {
        doc.as_object_mut().unwrap().remove("layout");
    } else if version < 4 {
        let order = doc["layout"]["order"].clone();
        doc["layout"] =
            json!({"order":order,"columns":1,"panel_weights":vec![100;count],"collapsed":[]});
    }
    doc
}
fn robot(id: &str) -> Value {
    let mut v = cloud(id);
    v["topic"] = json!("/tf");
    v["message_type"] = json!("tf2_msgs/msg/TFMessage");
    v["qos"]["reliability"] = json!("reliable");
    v["kind"]["renderer"] = json!("robot");
    v["kind"]["assets"] = json!({"urdf_path":"/not-created/机器人.urdf","package_roots":[{"package":"fixture_robot","path":"/not-created/assets"}]});
    v
}

#[test]
fn v6_eight_cloud_drafts_roundtrip_inactive_and_ninth_is_rejected() {
    assert_eq!(workspace::FORMAT_VERSION, 7);
    assert_eq!(workspace::MAX_VIEWS, 8);
    let mut wire = document(6, (0..8).map(|i| cloud(&format!("cloud{i}"))).collect());
    let doc = parse(&wire).unwrap();
    let encoded = doc.encode().unwrap();
    assert_eq!(Workspace::parse(&encoded).unwrap(), doc);
    let restored = doc.clone().inactive_restore().unwrap().into_configuration();
    assert_eq!(restored, doc);
    assert_eq!(restored.views.len(), 8);
    assert!(restored.views.iter().all(|v| matches!(
        v.kind,
        ViewKind::PointCloud {
            clock_expectation: ClockExpectation::System,
            ..
        }
    )));
    SupportedRenderers::all()
        .validate_workspace(&restored)
        .unwrap();
    let mut unsupported = SupportedRenderers::all();
    unsupported.point_cloud = false;
    assert!(unsupported.validate_workspace(&restored).is_err());
    wire["views"].as_array_mut().unwrap().push(cloud("cloud8"));
    wire["layout"]["order"]
        .as_array_mut()
        .unwrap()
        .push(json!("cloud8"));
    assert!(parse(&wire).is_err());
}

#[test]
fn legacy_versions_keep_original_view_caps_and_migrate_order_without_new_renderer_admission() {
    for version in 1..=5 {
        let count = if version == 1 { 1 } else { 4 };
        let old = legacy(version, count);
        let migrated = parse(&old).unwrap();
        assert_eq!(migrated.version, 7);
        assert_eq!(migrated.views.len(), count);
        assert_eq!(
            migrated.layout.order,
            (0..count).map(|i| format!("view{i}")).collect::<Vec<_>>()
        );
        assert!(
            parse(&legacy(version, 5)).is_err(),
            "format{version} cannot gain v6capacity"
        );
        if version == 1 {
            assert!(
                parse(&legacy(version, 2)).is_err(),
                "v1 remains stricter than later four-view versions"
            );
        }
        let mut smuggled = legacy(version, 1);
        smuggled["views"][0] = cloud("view0");
        assert!(
            parse(&smuggled).is_err(),
            "PointCloud cannot appear in declared format{version}"
        );
    }
}

#[test]
fn frozen_v5_robot_migrates_without_loading_assets_or_granting_clock_authorization() {
    let doc = parse(&document(5, vec![robot("robot")])).unwrap();
    assert_eq!(doc.version, 7);
    let restored = doc.inactive_restore().unwrap().into_configuration();
    let ViewKind::Robot {
        assets,
        clock_expectation,
        ..
    } = &restored.views[0].kind
    else {
        panic!("Robot migration")
    };
    assert_eq!(assets.urdf_path, "/not-created/机器人.urdf");
    assert_eq!(*clock_expectation, ClockExpectation::System);
    let encoded = String::from_utf8(restored.encode().unwrap()).unwrap();
    for runtime in [
        "clock_verified",
        "subscription",
        "activation",
        "model_id",
        "loaded_bytes",
        "poses",
    ] {
        assert!(!encoded.contains(runtime));
    }
    let mut old = document(4, vec![robot("robot")]);
    assert!(
        parse(&old).is_err(),
        "v4 cannot gain Robot via current enum"
    );
    old["version"] = json!(5);
    old["views"][0]["kind"]["clock_verified"] = json!(true);
    assert!(parse(&old).is_err());
}

#[test]
fn current_and_legacy_files_reject_unknown_runtime_data_instead_of_silently_dropping_it() {
    for version in 1..=6 {
        let mut value = if version < 6 {
            legacy(version, 1)
        } else {
            document(6, vec![cloud("view0")])
        };
        value["views"][0]["enabled"] = json!(true);
        assert!(parse(&value).is_err());
    }
    for field in [
        "points",
        "raw_bytes",
        "fields",
        "source_frame",
        "acquisition_stamp_ns",
        "clock_verified",
        "generation",
        "sequence",
    ] {
        let mut value = document(6, vec![cloud("cloud")]);
        value["views"][0]["kind"][field] = json!(1);
        assert!(
            parse(&value).is_err(),
            "PointCloud runtime field{field} cannot persist"
        );
    }
    let value = document(6, vec![cloud("cloud")]);
    let duplicate = serde_json::to_string(&value).unwrap().replacen(
        "\"renderer\":\"point_cloud\"",
        "\"renderer\":\"point_cloud\",\"renderer\":\"point_cloud\"",
        1,
    );
    assert!(Workspace::parse(duplicate.as_bytes()).is_err());
}

#[test]
fn point_cloud_type_and_tf_settings_must_be_explicit_bounded_and_unambiguous() {
    for (path, replacement) in [
        (vec!["message_type"], json!("sensor_msgs/msg/LaserScan")),
        (vec!["kind", "fixed_frame"], json!("/map")),
        (vec!["kind", "fixed_frame"], json!("bad frame")),
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
            vec!["kind", "transforms", "static_tf", "qos", "reliability"],
            json!("best_effort"),
        ),
        (
            vec!["kind", "transforms", "static_tf", "qos", "durability"],
            json!("volatile"),
        ),
        (vec!["kind", "transforms"], Value::Null),
    ] {
        let mut value = document(6, vec![cloud("cloud")]);
        let mut target = &mut value["views"][0];
        for key in &path[..path.len() - 1] {
            target = &mut target[*key];
        }
        target[path[path.len() - 1]] = replacement;
        assert!(parse(&value).is_err(), "invalid PointCloud setting{path:?}");
    }
    let mut bounded = document(6, vec![cloud("cloud")]);
    bounded["views"][0]["kind"]["fixed_frame"] = json!("f".repeat(256));
    assert!(parse(&bounded).is_ok());
    bounded["views"][0]["kind"]["fixed_frame"] = json!("f".repeat(257));
    assert!(parse(&bounded).is_err());
}

#[test]
fn eight_configurations_do_not_bypass_global_file_or_exact_layout_bounds() {
    let mut value = document(6, vec![robot("robot0"), robot("robot1")]);
    let roots = (0..16)
        .map(|i| json!({"package":format!("pkg{i}"),"path":"\\".repeat(2000)}))
        .collect::<Vec<_>>();
    for index in 0..2 {
        value["views"][index]["kind"]["assets"]["package_roots"] = json!(roots);
    }
    value["version"] = json!(workspace::FORMAT_VERSION);
    let doc: Workspace = serde_json::from_value(value).unwrap();
    doc.validate().unwrap();
    assert!(
        doc.encode().is_err(),
        "64KiBaggregatefilecap remains even with8views"
    );
    let mut invalid = document(6, vec![cloud("one"), cloud("two")]);
    invalid["layout"]["order"] = json!(["one", "one"]);
    assert!(parse(&invalid).is_err());
    invalid["layout"]["order"] = json!(["one", "unknown"]);
    assert!(parse(&invalid).is_err());
}

#[test]
fn existing_presets_emit_current_config_only_and_never_invent_cloud_inputs() {
    for preset in [Preset::Inspect, Preset::Visualize, Preset::Analyze] {
        let doc = preset.workspace();
        assert_eq!(doc.version, 7);
        let decoded = Workspace::parse(&doc.encode().unwrap()).unwrap();
        assert_eq!(decoded, doc);
        assert_eq!(
            decoded
                .clone()
                .inactive_restore()
                .unwrap()
                .into_configuration(),
            doc
        );
        assert!(doc
            .views
            .iter()
            .all(|v| !matches!(v.kind, ViewKind::PointCloud { .. })));
    }
}
