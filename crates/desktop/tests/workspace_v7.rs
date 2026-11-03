//! Configuration-only mapping migration; deliberately nonexistent asset paths.
use serde_json::{json, Value};
use studio_desktop::workspace::{self, SupportedRenderers, Workspace};
fn view(renderer: &str, id: &str) -> Value {
    let ty = match renderer {
        "occupancy_grid" => "nav_msgs/msg/OccupancyGrid",
        "pose" => "geometry_msgs/msg/PoseStamped",
        _ => "nav_msgs/msg/Path",
    };
    json!({"id":id,"kind":{"renderer":renderer,"fixed_frame":"map","clock_expectation":"system","transforms":{"dynamic_tf":{"topic":"/tf","message_type":"tf2_msgs/msg/TFMessage","qos":{"reliability":"reliable","durability":"volatile"}},"static_tf":{"topic":"/tf_static","message_type":"tf2_msgs/msg/TFMessage","qos":{"reliability":"reliable","durability":"transient_local"}}}},"topic":format!("/mapping/{id}"),"message_type":ty,"qos":{"reliability":"reliable","durability":if renderer=="occupancy_grid"{"transient_local"}else{"volatile"}}})
}
fn doc(version: u32, views: Vec<Value>) -> Value {
    let order = views.iter().map(|v| v["id"].clone()).collect::<Vec<_>>();
    json!({"version":version,"name":"Inactive overlays","views":views,"panels":{"data_open":true,"inspector_open":true,"diagnostics_open":false},"layout":{"order":order,"scene_percent":65},"connection_suggestion":null})
}
fn parse(value: &Value) -> Result<Workspace, workspace::Error> {
    Workspace::parse(&serde_json::to_vec(value).unwrap())
}
#[test]
fn mapping_v7_roundtrip_is_config_only_inactive_and_renderer_capabilities_are_explicit() {
    assert_eq!(workspace::FORMAT_VERSION, 7);
    let value = doc(
        7,
        vec![
            view("occupancy_grid", "map"),
            view("pose", "pose"),
            view("path", "path"),
        ],
    );
    let parsed = parse(&value).unwrap();
    let encoded = parsed.encode().unwrap();
    assert_eq!(Workspace::parse(&encoded).unwrap(), parsed);
    assert_eq!(
        parsed
            .clone()
            .inactive_restore()
            .unwrap()
            .into_configuration(),
        parsed
    );
    for renderer in ["occupancy_grid", "pose", "path"] {
        let one = parse(&doc(7, vec![view(renderer, "one")])).unwrap();
        let mut supported = SupportedRenderers::all();
        match renderer {
            "occupancy_grid" => supported.occupancy_grid = false,
            "pose" => supported.pose = false,
            _ => supported.path = false,
        }
        assert!(supported.validate_workspace(&one).is_err());
    }
    let text = String::from_utf8(encoded).unwrap();
    for forbidden in [
        "cells",
        "poses",
        "acquisition_stamp_ns",
        "clock_verified",
        "generation",
        "activation",
        "loaded_bytes",
    ] {
        assert!(!text.contains(forbidden));
    }
}
#[test]
fn frozen_v1_through_v6_reject_mapping_and_preserve_original_capacity() {
    for version in 1..=6 {
        let cap = if version == 1 {
            1
        } else if version < 6 {
            4
        } else {
            8
        };
        let mut old=doc(version,(0..cap).map(|i|json!({"id":format!("v{i}"),"kind":if version==1{json!("messages")}else{json!({"renderer":"messages"})},"topic":"/status","message_type":"std_msgs/msg/String","qos":{"reliability":"reliable","durability":"volatile"}})).collect());
        if version == 1 {
            old.as_object_mut().unwrap().remove("layout");
        } else if version < 4 {
            let order = old["layout"]["order"].clone();
            old["layout"] =
                json!({"order":order,"columns":1,"panel_weights":vec![100;cap],"collapsed":[]});
        }
        let parsed = parse(&old).unwrap();
        assert_eq!(parsed.version, 7);
        assert_eq!(parsed.views.len(), cap);
        let mut over = old.clone();
        let mut extra = over["views"][0].clone();
        extra["id"] = json!("extra");
        over["views"].as_array_mut().unwrap().push(extra);
        if version != 1 {
            over["layout"]["order"]
                .as_array_mut()
                .unwrap()
                .push(json!("extra"));
            if version < 4 {
                over["layout"]["panel_weights"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(100));
            }
        }
        assert!(parse(&over).is_err(), "legacy cap {version}");
        for renderer in ["occupancy_grid", "pose", "path"] {
            let mut invalid = old.clone();
            invalid["views"][0] = view(renderer, "v0");
            assert!(
                parse(&invalid).is_err(),
                "renderer {renderer} smuggled into{version}"
            );
        }
    }
}
#[test]
fn mapping_configuration_rejects_runtime_geometry_wrong_types_and_ambiguous_tf() {
    for renderer in ["occupancy_grid", "pose", "path"] {
        for field in [
            "cells",
            "poses",
            "origin",
            "resolution",
            "source_frame",
            "acquisition_stamp_ns",
            "clock_verified",
        ] {
            let mut value = doc(7, vec![view(renderer, "one")]);
            value["views"][0]["kind"][field] = json!(0);
            assert!(parse(&value).is_err());
        }
        for case in 0..5 {
            let mut value = doc(7, vec![view(renderer, "one")]);
            let v = &mut value["views"][0];
            match case {
                0 => v["message_type"] = json!("std_msgs/msg/String"),
                1 => v["kind"]["fixed_frame"] = json!("bad frame"),
                2 => v["kind"]["transforms"]["static_tf"]["topic"] = json!("/tf"),
                3 => v["kind"]["transforms"]["static_tf"]["qos"]["durability"] = json!("volatile"),
                _ => {
                    v["kind"]["transforms"]["dynamic_tf"]["qos"]["durability"] =
                        json!("transient_local")
                }
            }
            assert!(parse(&value).is_err(), "{renderer} invalid{case}");
        }
    }
}
#[test]
fn eight_mapping_drafts_are_storable_without_granting_active_scene_capacity() {
    let mut value = doc(7, (0..8).map(|i| view("path", &format!("p{i}"))).collect());
    let parsed = parse(&value).unwrap();
    assert_eq!(
        parsed
            .inactive_restore()
            .unwrap()
            .configuration()
            .views
            .len(),
        8
    );
    value["views"]
        .as_array_mut()
        .unwrap()
        .push(view("path", "p8"));
    value["layout"]["order"]
        .as_array_mut()
        .unwrap()
        .push(json!("p8"));
    assert!(parse(&value).is_err());
}
