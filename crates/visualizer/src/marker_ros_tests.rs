//! Actual Visualizer root, backend and synthetic localhost ROS; no native UI claim.
use super::*;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command as Process,
};
use studio_client_core::map_fixture::FixturePublisher;
use studio_protocol::marker;
fn wait(app: &mut App, label: &str, mut predicate: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(35);
    let ctx = egui::Context::default();
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.poll_markers(&ctx);
        app.check_deadlines(Instant::now());
        if predicate(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {} {:?}",
            app.connection.status,
            app.views
                .iter()
                .map(|v| (
                    &v.configuration.topic,
                    v.status,
                    &v.error,
                    &v.spatial_status
                ))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn publish(p: &mut FixturePublisher, target: &str, markers: Value) {
    assert_eq!(
        p.send(&json!({"op":"publish","target":target,"markers":markers}).to_string())["sent"],
        "publish"
    );
}
fn add(app: &mut App, topic: &str, ty: &str) -> ViewId {
    app.begin_source(Output::MarkerSnapshot, topic, ty);
    // Explicit retained late-join intent; the product default remains Volatile.
    app.draft_transient_local = topic != "/studio_fixture/marker_best";
    app.subscribe();
    app.selected.unwrap()
}
fn visible(app: &App) -> usize {
    app.markers
        .projected
        .as_ref()
        .zip(app.marker_permit())
        .map_or(0, |(o, p)| {
            o.frame.opaque_visible(Some(&p)).map(|(_, r)| r.len()).sum()
        })
}
fn has_key(app: &App, id: ViewId, key: i32) -> bool {
    app.views
        .iter()
        .find(|v| v.id == id)
        .and_then(|v| v.marker.state.as_ref())
        .is_some_and(|s| s.entries().iter().any(|e| e.marker.key.id == key))
}
#[test]
#[ignore = "Requires isolated localhost/domain187 ROS and unique STUDIO_UI_ROS_EVIDENCE"]
fn marker_real_ros_visualizer_workflow() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    for key in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(std::env::var(key).unwrap_or_default().is_empty());
    }
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap();
    assert!(matches!(
        rmw.as_str(),
        "rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp"
    ));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let folder = PathBuf::from(std::env::var("STUDIO_UI_ROS_EVIDENCE").unwrap());
    assert!(
        folder.is_absolute() && folder.starts_with(root.join("artifacts/feasibility/evidence"))
    );
    struct Finish;
    impl Drop for Finish {
        fn drop(&mut self) {
            session::finish_shutdown();
        }
    }
    let _finish = Finish;
    let mut command = Process::new(root.join(".pixi/envs/default/bin/python"));
    command
        .arg(root.join("crates/backend/tests/fixtures/marker_publisher.py"))
        .stderr(std::fs::File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    let publisher_pid = publisher.process.child.id();
    publish(&mut publisher, "single", json!([{"id":1}]));
    publish(&mut publisher, "array", json!([{"id":10}]));
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = rmw == "rmw_cyclonedds_cpp";
    app.shared_scene.marker_request = Some((spatial::Camera::default().pose(), [800, 600]));
    app.connect();
    wait(&mut app, "discovery", |a| {
        a.connection.phase == Phase::Connected
            && [
                "/studio_fixture/marker",
                "/studio_fixture/marker_array",
                "/studio_fixture/marker_best",
                "/studio_fixture/marker_image",
                "/tf_static",
            ]
            .iter()
            .all(|name| a.discovery.topics.iter().any(|(t, _)| t == name))
    });
    let mut cases = vec!["exact Marker/Array/Image/TF discovery without activation"];
    assert!(app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .registry
        .entries()
        .is_empty());
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    let single = add(&mut app, "/studio_fixture/marker", marker::MESSAGE_TYPE);
    let array = add(
        &mut app,
        "/studio_fixture/marker_array",
        marker::ARRAY_MESSAGE_TYPE,
    );
    assert!(app.views.iter().all(|v| !v.enabled));
    let c = publisher.send(r#"{"op":"counts"}"#);
    assert_eq!(c["single"], 0);
    assert_eq!(c["array"], 0);
    app.set_display_enabled(single, true);
    app.set_display_enabled(array, true);
    wait(
        &mut app,
        "retained Markers through both worker stages",
        |a| visible(a) == 2,
    );
    let c = publisher.send(r#"{"op":"counts"}"#);
    assert_eq!(c["single"], 1);
    assert_eq!(c["array"], 1);
    assert_eq!(c["sibling"], 0);
    assert_eq!(c["static_tf"], 1);
    cases.push("Add disabled, explicit Enable, retained data, shared TF and no sibling reader");
    publish(
        &mut publisher,
        "array",
        json!([{"id":10,"x":1.0},{"id":10,"x":2.0},{"id":11}]),
    );
    wait(&mut app, "ordered replacement", |a| {
        has_key(a, array, 11) && visible(a) == 3
    });
    publish(&mut publisher, "array", json!([{"id":10,"action":2}]));
    wait(&mut app, "key DELETE", |a| {
        !has_key(a, array, 10) && visible(a) == 2
    });
    publish(&mut publisher, "array", json!([{"id":10}]));
    wait(&mut app, "deleted key readded", |a| {
        has_key(a, array, 10) && visible(a) == 3
    });
    cases.push("ordered ADD/duplicate/DELETE/readd with cached peers");
    publish(&mut publisher, "array", json!([{"id":10,"type":12}]));
    wait(&mut app, "unsupported ADD removes old key", |a| {
        !has_key(a, array, 10) && visible(a) == 2
    });
    cases.push("unsupported kind removes its obsolete key without hiding peers");
    publish(
        &mut publisher,
        "array",
        json!([{"id":11,"action":2},{"id":30,"frame":"missing","locked":true}]),
    );
    wait(&mut app, "missing TF hides geometry", |a| {
        has_key(a, array, 30) && visible(a) == 1
    });
    publisher.send(r#"{"op":"connect_frame"}"#);
    wait(&mut app, "TF-only recovery", |a| visible(a) == 2);
    cases.push("frame-locked missing TF and recovery without a new Marker");
    app.clear_marker(array);
    assert_eq!(visible(&app), 1);
    wait(&mut app, "correlated Clear", |a| {
        a.views
            .iter()
            .find(|v| v.id == array)
            .unwrap()
            .marker
            .clear_request
            .is_none()
    });
    publish(
        &mut publisher,
        "array",
        json!([{"id":40},{"id":41,"lifetime_ns":300000000}]),
    );
    wait(&mut app, "post-Clear entries", |a| {
        has_key(a, array, 41) && visible(a) == 3
    });
    wait(&mut app, "idle expiry", |a| {
        !has_key(a, array, 41) && visible(a) == 2
    });
    cases.push("Clear and exact idle lifetime expiry preserve permanent/other-owner peers");
    app.begin_source(
        Output::CompactImage,
        "/studio_fixture/marker_image",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.subscribe();
    let image = app.selected.unwrap();
    app.set_display_enabled(image, true);
    wait(&mut app, "Image subscription", |a| {
        a.views
            .iter()
            .find(|v| v.id == image)
            .unwrap()
            .status
            .active()
    });
    // Reader startup is earlier than DDS endpoint matching. This Volatile
    // one-shot sample is deliberately sent only after the publisher sees it.
    wait(&mut app, "Image publisher match", |_| {
        publisher.send(r#"{"op":"counts"}"#)["image"] == 1
    });
    publisher.send(r#"{"op":"image"}"#);
    wait(&mut app, "Image reception", |a| {
        a.views
            .iter()
            .find(|v| v.id == image)
            .unwrap()
            .image_sequence
            > 0
    });
    app.stop_view(array);
    wait(&mut app, "owner Stop", |a| {
        a.views
            .iter()
            .find(|v| v.id == array)
            .unwrap()
            .subscription
            .is_none()
    });
    assert_eq!(visible(&app), 1);
    cases.push("Image and other Marker owner survive scoped Stop");
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        studio_workspace::application::VISUALIZER_VERSION
    );
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let restored = app.workspace_prepare(&imported.workspace).unwrap();
    assert!(restored
        .views
        .iter()
        .all(|v| !v.enabled && v.marker.stream.is_none() && !v.spatial_clock_verified));
    cases.push("v15 source/QoS/scene intent prepares inactive without runtime authority");
    let best = add(
        &mut app,
        "/studio_fixture/marker_best",
        marker::MESSAGE_TYPE,
    );
    app.set_display_enabled(best, true);
    wait(&mut app, "Reliable reader started", |a| {
        a.views
            .iter()
            .find(|v| v.id == best)
            .unwrap()
            .status
            .active()
    });
    publish(&mut publisher, "best", json!([{"id":71}]));
    let until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < until {
        app.poll_backend();
        app.poll_markers(&egui::Context::default());
        assert!(!has_key(&app, best, 71));
        std::thread::sleep(Duration::from_millis(5));
    }
    app.stop_view(best);
    wait(&mut app, "mismatch reader cleanup", |a| {
        a.views
            .iter()
            .find(|v| v.id == best)
            .unwrap()
            .subscription
            .is_none()
    });
    app.views
        .iter_mut()
        .find(|v| v.id == best)
        .unwrap()
        .configuration
        .best_effort = true;
    app.set_display_enabled(best, true);
    let mut next = Instant::now();
    wait(&mut app, "explicit Best effort correction", |a| {
        if Instant::now() >= next {
            publish(&mut publisher, "best", json!([{"id":72}]));
            next = Instant::now() + Duration::from_millis(100);
        }
        has_key(a, best, 72)
    });
    cases.push("Reliable/Best effort mismatch and explicit correction");
    app.stop_view(best);
    app.stop_view(single);
    app.stop_view(image);
    wait(&mut app, "all reader cleanup", |a| {
        a.views.iter().all(|v| v.subscription.is_none())
    });
    cases.push("all explicit Stop requests await validated cleanup");
    app.set_display_enabled(single, true);
    wait(&mut app, "retained before disconnect", |a| {
        has_key(a, single, 1)
    });
    app.stop();
    session::finish_shutdown();
    assert!(app.views.iter().all(|v| v.marker.stream.is_none()));
    cases.push("disconnect hides Marker data and releases backend");
    app.connect();
    wait(&mut app, "inactive reconnect", |a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    app.stop();
    app.markers.finish_shutdown().unwrap();
    session::finish_shutdown();
    publisher.finish();
    cases.push("reconnect never replays commands; workers and publisher finish");
    assert_eq!(cases.len(), 12);
    let records: Vec<Value> = std::fs::read_to_string(folder.join("backend-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    std::fs::write(folder.join("result.json"),serde_json::to_vec_pretty(&json!({"outcome":"passed","cases":cases,"rmw":rmw,"domain":187,"publisher_pid":publisher_pid,"test_pid":std::process::id(),"backend_pids":records.iter().map(|v|v["pid"].clone()).collect::<Vec<_>>(),"scope":"Actual Visualizer root/backend/ROS and preparation/projection workers; no native-window, GPU, sustained performance, LAN or hardware acceptance"})).unwrap()).unwrap();
}
