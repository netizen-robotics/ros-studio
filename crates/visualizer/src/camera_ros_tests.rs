//! Actual application/backend/ROS acceptance; no native-window claim.
use super::*;
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command as Process};
use studio_client_core::map_fixture::FixturePublisher;
use studio_protocol::camera_snapshot::{InfoSource, Status};

fn index(app: &App, id: ViewId) -> usize {
    app.views.iter().position(|v| v.id == id).unwrap()
}
fn wait(app: &mut App, label: &str, mut predicate: impl FnMut(&App) -> bool) {
    let until = Instant::now() + Duration::from_secs(35);
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.poll_cameras();
        app.check_deadlines(Instant::now());
        if predicate(app) {
            eprintln!("Camera ROS case passed: {label}");
            return;
        }
        assert!(
            Instant::now() < until,
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
fn current(app: &App, id: ViewId) -> bool {
    app.views[index(app, id)].camera.is_current()
}
fn status(app: &App, id: ViewId, expected: Status) -> bool {
    app.views[index(app, id)]
        .camera
        .snapshot
        .as_ref()
        .is_some_and(|s| s.status == expected)
}
fn configure_info(app: &mut App, id: ViewId, topic: &str, best: bool, retained: bool) {
    app.configure_view(id);
    let Output::CameraSnapshot(info) = &mut app.draft_output else {
        panic!("Camera configuration lost its type");
    };
    info.topic = topic.into();
    info.best_effort = best;
    info.transient_local = retained;
}

// Independent DDS lanes have no cross-topic arrival order. Offer new Images
// while awaiting readiness; the product must never replay a pre-calibration
// Image. Keep the same deadline and require prepared, authorized output.
fn wait_with_fresh_images(
    app: &mut App,
    publisher: &mut FixturePublisher,
    camera: ViewId,
    label: &str,
    expected: Status,
) {
    let mut offered = Instant::now() - Duration::from_millis(100);
    wait(app, label, |a| {
        if status(a, camera, expected) && (expected != Status::Receiving || current(a, camera)) {
            return true;
        }
        if offered.elapsed() >= Duration::from_millis(100) {
            publisher.send(r#"{"op":"publish","target":"image","stamp":2}"#);
            offered = Instant::now();
        }
        false
    });
}

#[test]
#[ignore = "Requires guarded localhost domain187 and unique STUDIO_UI_ROS_EVIDENCE"]
fn camera_real_ros_visualizer_workflow() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
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
        .arg(root.join("crates/backend/tests/fixtures/camera_publisher.py"))
        .stderr(std::fs::File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovery without activation", |a| {
        a.connection.phase == Phase::Connected
            && [
                "/studio_fixture/image_rect",
                "/studio_fixture/camera_info",
                "/studio_fixture/best_info",
            ]
            .iter()
            .all(|t| a.discovery.topics.iter().any(|(name, _)| name == t))
    });
    let counts = publisher.send(r#"{"op":"counts"}"#);
    assert!(["image", "info", "other", "best", "peer", "static_tf"]
        .iter()
        .all(|k| counts[*k] == 0));
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.begin_source(
        Output::CameraSnapshot(Box::new(InfoSource {
            topic: "/studio_fixture/camera_info".into(),
            best_effort: false,
            transient_local: true,
        })),
        "/studio_fixture/image_rect",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.draft_transient_local = true;
    app.subscribe();
    let camera = app.selected.unwrap();
    app.begin_source(
        Output::CompactImage,
        "/studio_fixture/peer_image",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.draft_transient_local = true;
    app.subscribe();
    let peer = app.selected.unwrap();
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    app.set_display_enabled(peer, true);
    app.set_display_enabled(camera, true);
    wait(&mut app, "source and shared TF matches", |_| {
        let c = publisher.send(r#"{"op":"counts"}"#);
        c["image"] == 1 && c["info"] == 1 && c["static_tf"] == 1 && c["peer"] == 1
    });
    wait(
        &mut app,
        "both retained Camera inputs and ordinary Image delivered",
        |a| {
            a.views[index(a, camera)]
                .camera
                .snapshot
                .as_ref()
                .is_some_and(|s| s.counters.images > 0 && s.counters.calibrations > 0)
                && a.views[index(a, peer)].status == ViewStatus::Receiving
        },
    );
    wait_with_fresh_images(
        &mut app,
        &mut publisher,
        camera,
        "fresh Image after independently arriving retained calibration",
        Status::Receiving,
    );
    let ci = index(&app, camera);
    assert_eq!(
        app.views[ci]
            .camera
            .projection
            .as_ref()
            .unwrap()
            .pixel([1.0, 0.0, 1.0]),
        Some([90.0, 23.0])
    );
    let held = app.views[ci].camera.permission.clone().unwrap();
    for sequence in 0..5 {
        let previous = app.views[ci]
            .camera
            .snapshot
            .as_ref()
            .unwrap()
            .counters
            .images;
        publisher.send(r#"{"op":"publish","target":"image","stamp":2}"#);
        wait(&mut app, "same-continuity image replacement", |a| {
            a.views[ci]
                .camera
                .snapshot
                .as_ref()
                .is_some_and(|s| s.counters.images > previous)
                && current(a, camera)
        });
        assert!(
            held.is_valid(),
            "replacement {sequence} revoked captured completed alignment"
        );
    }
    let old = app.views[ci].camera.permission.clone().unwrap();
    let first = app.views[ci].subscription;
    configure_info(&mut app, camera, "/studio_fixture/other_info", false, true);
    assert!(old.is_valid() && current(&app, camera));
    assert_eq!(app.views[ci].subscription, first);
    app.subscribe();
    assert!(!old.is_valid() && !current(&app, camera));
    wait_with_fresh_images(
        &mut app,
        &mut publisher,
        camera,
        "incompatible pair after explicit replacement",
        Status::Incompatible,
    );
    assert_ne!(app.views[ci].subscription, first);
    assert_eq!(app.views[index(&app, peer)].status, ViewStatus::Receiving);
    configure_info(&mut app, camera, "/studio_fixture/camera_info", false, true);
    app.subscribe();
    wait_with_fresh_images(
        &mut app,
        &mut publisher,
        camera,
        "explicit pair correction",
        Status::Receiving,
    );
    let old = app.views[ci].camera.permission.clone().unwrap();
    publisher.send(r#"{"op":"publish","target":"info","focal":70.0}"#);
    wait(
        &mut app,
        "calibration change invalidates without replay",
        |a| status(a, camera, Status::WaitingImage),
    );
    assert!(!old.is_valid() && !current(&app, camera));
    publisher.send(r#"{"op":"publish","target":"image","stamp":3}"#);
    wait(&mut app, "fresh Image uses changed calibration", |a| {
        current(a, camera)
    });
    assert_eq!(
        app.views[ci]
            .camera
            .projection
            .as_ref()
            .unwrap()
            .pixel([1.0, 0.0, 1.0]),
        Some([100.0, 23.0])
    );
    publisher.send(r#"{"op":"publish","target":"info","invalid":true}"#);
    wait(&mut app, "invalid CameraInfo hides aligned output", |a| {
        status(a, camera, Status::InvalidInfo)
    });
    assert!(!current(&app, camera));
    publisher.send(r#"{"op":"publish","target":"info","focal":80.0}"#);
    wait(
        &mut app,
        "corrected calibration waits for fresh Image",
        |a| status(a, camera, Status::WaitingImage),
    );
    publisher.send(r#"{"op":"publish","target":"image","stamp":4}"#);
    wait(&mut app, "valid calibration recovery", |a| {
        current(a, camera)
    });
    publisher.send(r#"{"op":"publish","target":"image","stamp":5,"width":32}"#);
    wait(&mut app, "dimension mismatch hides aligned output", |a| {
        status(a, camera, Status::Incompatible)
    });
    publisher.send(r#"{"op":"publish","target":"image","stamp":6}"#);
    wait(&mut app, "dimension correction", |a| current(a, camera));
    configure_info(&mut app, camera, "/studio_fixture/best_info", false, false);
    app.subscribe();
    wait(&mut app, "independent CameraInfo QoS mismatch", |a| {
        let c = publisher.send(r#"{"op":"counts"}"#);
        status(a, camera, Status::WaitingInfo) && c["image"] == 1 && c["best"] == 0
    });
    configure_info(&mut app, camera, "/studio_fixture/best_info", true, false);
    app.subscribe();
    wait(
        &mut app,
        "explicit CameraInfo QoS correction matches",
        |_| publisher.send(r#"{"op":"counts"}"#)["best"] == 1,
    );
    // A matched volatile best-effort source cannot promise one-shot delivery.
    // Re-offer only after the explicit QoS correction, at a bounded 10 Hz.
    let mut offered = Instant::now() - Duration::from_millis(100);
    wait(&mut app, "volatile calibration received", |a| {
        if (status(a, camera, Status::WaitingImage)
            || (status(a, camera, Status::Receiving) && current(a, camera)))
            && a.views[ci]
                .camera
                .snapshot
                .as_ref()
                .is_some_and(|s| s.counters.calibrations > 0)
        {
            return true;
        }
        if offered.elapsed() >= Duration::from_millis(100) {
            publisher.send(r#"{"op":"publish","target":"best","focal":90.0}"#);
            offered = Instant::now();
        }
        false
    });
    wait_with_fresh_images(
        &mut app,
        &mut publisher,
        camera,
        "QoS corrected receiving",
        Status::Receiving,
    );
    assert_eq!(
        app.views[ci]
            .camera
            .projection
            .as_ref()
            .unwrap()
            .pixel([1.0, 0.0, 1.0]),
        Some([120.0, 23.0])
    );
    let old = app.views[ci].camera.permission.clone().unwrap();
    app.stop_view(camera);
    assert!(!old.is_valid() && !current(&app, camera));
    wait(
        &mut app,
        "two-source Stop releases shared TF, retains peer",
        |a| {
            let c = publisher.send(r#"{"op":"counts"}"#);
            a.views[ci].subscription.is_none()
                && c["image"] == 0
                && c["best"] == 0
                && c["static_tf"] == 0
                && c["peer"] == 1
        },
    );
    assert_eq!(app.views[index(&app, peer)].status, ViewStatus::Receiving);
    app.set_display_enabled(camera, true);
    app.set_display_enabled(camera, false);
    wait(&mut app, "startup cancellation cleanup", |a| {
        a.views[ci].subscription.is_none() && a.views[ci].camera.stream.is_none()
    });
    app.stop();
    session::finish_shutdown();
    app.connect();
    wait(&mut app, "inactive reconnect", |a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none() && v.camera.stream.is_none()));
    wait(&mut app, "no replayed readers", |_| {
        let c = publisher.send(r#"{"op":"counts"}"#);
        ["image", "info", "other", "best", "peer", "static_tf"]
            .iter()
            .all(|k| c[*k] == 0)
    });
    app.stop();
    session::finish_shutdown();
    publisher.finish();
    let launches: Vec<Value> = std::fs::read_to_string(folder.join("backend-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    std::fs::write(folder.join("result.json"), serde_json::to_vec_pretty(&json!({
        "outcome":"passed", "test_pid":std::process::id(),
        "backend_pids": launches.iter().map(|v| &v["pid"]).collect::<Vec<_>>(),
        "cases":["discovery without readers", "Add disabled and explicit Enable", "retained Image/CameraInfo/shared TF with Image peer",
            "draft does not replace; Apply incompatible source; correction", "semantic calibration invalidates and requires fresh Image",
            "invalid CameraInfo recovery", "dimension mismatch recovery", "independent CameraInfo QoS mismatch/correction",
            "two-reader Stop preserves Image peer", "startup cancellation", "inactive reconnect without replay"],
        "scope":"Actual App/backend/ROS and prepared calibrated frame; no native window, GPU, performance, LAN or hardware acceptance"
    })).unwrap()).unwrap();
}
