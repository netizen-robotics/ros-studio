//! Actual App composition with synthetic localhost ROS sources; no window claim.
use super::*;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command as Process,
};
use studio_client_core::map_fixture::FixturePublisher;
use studio_protocol::pose_snapshot::{self as pose, transport::Configuration};

fn wait(app: &mut App, label: &str, mut predicate: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.poll_uncertainty();
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
fn index(app: &App, id: ViewId) -> usize {
    app.views.iter().position(|v| v.id == id).unwrap()
}
fn visible(app: &App, id: ViewId) -> bool {
    app.views[index(app, id)]
        .uncertainty
        .layer(id.0)
        .and_then(|layer| layer.bounds())
        .is_some()
}
fn add(app: &mut App, kind: pose::Kind, topic: &str, ty: &str) -> ViewId {
    app.begin_source(
        Output::PoseSnapshot(Configuration::for_kind(kind)),
        topic,
        ty,
    );
    app.draft_transient_local = true;
    app.subscribe();
    app.selected.unwrap()
}
#[test]
#[ignore = "Requires guarded localhost domain187 and unique STUDIO_UI_ROS_EVIDENCE"]
fn uncertainty_real_ros_visualizer_workflow() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
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
        .arg(root.join("crates/backend/tests/fixtures/pose_publisher.py"))
        .stderr(std::fs::File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovery", |a| {
        a.connection.phase == Phase::Connected
            && ["/studio_fixture/raw_pose", "/studio_fixture/raw_odom"]
                .iter()
                .all(|t| a.discovery.topics.iter().any(|(name, _)| name == t))
    });
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
    let cov = add(
        &mut app,
        pose::Kind::PoseWithCovariance,
        "/studio_fixture/raw_pose",
        "geometry_msgs/msg/PoseWithCovarianceStamped",
    );
    let odom = add(
        &mut app,
        pose::Kind::Odometry,
        "/studio_fixture/raw_odom",
        "nav_msgs/msg/Odometry",
    );
    assert!(app.views.iter().all(|v| !v.enabled));
    let counts = publisher.send(r#"{"op":"counts"}"#);
    assert_eq!(counts["pose"], 0);
    assert_eq!(counts["odom"], 0);
    app.set_display_enabled(cov, true);
    app.set_display_enabled(odom, true);
    wait(
        &mut app,
        "publisher matches both sources and shared TF",
        |_| {
            let counts = publisher.send(r#"{"op":"counts"}"#);
            counts["pose"] == 1 && counts["odom"] == 1 && counts["static_tf"] == 1
        },
    );
    wait(&mut app, "retained source geometry", |a| {
        visible(a, cov) && visible(a, odom)
    });
    let counts = publisher.send(r#"{"op":"counts"}"#);
    assert_eq!(counts["pose"], 1);
    assert_eq!(counts["odom"], 1);
    assert_eq!(counts["static_tf"], 1);
    let oi = index(&app, odom);
    let old = app.views[oi].uncertainty.permission.clone().unwrap();
    app.clear_uncertainty(oi);
    assert!(!old.is_valid() && !visible(&app, odom) && visible(&app, cov));
    wait(&mut app, "Clear ack without new samples", |a| {
        !a.views[oi]
            .uncertainty
            .stream
            .as_ref()
            .unwrap()
            .is_waiting_for_clear()
    });
    assert!(!visible(&app, odom));
    publisher.send(r#"{"op":"publish","target":"odom","stamp":3}"#);
    wait(&mut app, "fresh post-Clear", |a| visible(a, odom));
    let prior = app.views[oi].subscription;
    app.views[oi].uncertainty.history_draft.keep = 8;
    app.apply_uncertainty_history(oi);
    wait(&mut app, "history replacement cleanup", |a| {
        a.views[oi].subscription.is_some() && a.views[oi].subscription != prior && visible(a, odom)
    });
    assert!(visible(&app, cov));
    publisher.send(r#"{"op":"publish","target":"pose","invalid":true}"#);
    wait(&mut app, "malformed source scoped cleanup", |a| {
        a.views[index(a, cov)].subscription.is_none()
    });
    assert!(!visible(&app, cov) && visible(&app, odom));
    app.stop_view(odom);
    assert!(!visible(&app, odom));
    wait(&mut app, "Stop cleanup", |a| {
        a.views.iter().all(|v| v.subscription.is_none())
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
        .all(|v| !v.enabled && v.subscription.is_none() && v.uncertainty.stream.is_none()));
    app.stop();
    session::finish_shutdown();
    publisher.finish();
    let launches: Vec<Value> = std::fs::read_to_string(folder.join("backend-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    std::fs::write(folder.join("result.json"), serde_json::to_vec_pretty(&json!({
        "outcome":"passed", "test_pid":std::process::id(), "backend_pids": launches.iter().map(|v| &v["pid"]).collect::<Vec<_>>(),
        "cases":["discovery without activation", "Add disabled and explicit Enable", "retained sources and shared TF",
            "immediate Clear and fresh recovery", "history replacement preserves peer", "malformed source scoped cleanup",
            "Stop and inactive reconnect"],
        "scope":"Actual App/backend/ROS and prepared geometry; no native window, GPU, performance, LAN or hardware acceptance"
    })).unwrap()).unwrap();
}
