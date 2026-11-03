//! Actual localhost ROS proof for the corrected first-scan defaults and TF rate bound.
use super::*;
use serde_json::json;
use std::{path::PathBuf, process::Command as Process};
use studio_client_core::map_fixture::FixturePublisher;
fn wait(app: &mut App, label: &str, seconds: u64, mut ready: impl FnMut(&App) -> bool) {
    let until = Instant::now() + Duration::from_secs(seconds);
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.check_deadlines(Instant::now());
        if ready(app) {
            return;
        }
        assert!(
            Instant::now() < until,
            "{label}: {} {:?}",
            app.connection.status,
            app.views
                .iter()
                .map(|v| (&v.error, &v.spatial_status, v.status))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "Requires guarded localhost domain187, explicit matching RMW and STUDIO_UI_ROS_EVIDENCE"]
fn scene_defaults_real_ros_scan_high_rate_tf() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
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
        .arg(root.join("crates/backend/tests/fixtures/scene_scan_publisher.py"))
        .env("DYLD_LIBRARY_PATH", root.join(".pixi/envs/default/lib"))
        .stderr(std::fs::File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovered scan", 30, |a| {
        a.connection.phase == Phase::Connected
            && a.discovery
                .topics
                .iter()
                .any(|(t, _)| t == "/studio_fixture/scan")
    });
    assert_eq!(app.scene_settings.configuration.fixed_frame, "map");
    app.show_topic(
        Output::SpatialScan,
        "/studio_fixture/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    wait(&mut app, "receiving scans while map missing", 3, |a| {
        a.views[0].spatial_counters.scans_received > 0
    });
    assert!(app.views[0].enabled && app.views[0].error.is_none());
    assert!(app.views[0].sensor_history.points().is_empty());
    let subscription = app.views[0].subscription;
    publisher.send("map");
    wait(
        &mut app,
        "missing-map recovery without a new Start",
        3,
        |a| !a.views[0].sensor_history.points().is_empty(),
    );
    assert_eq!(app.views[0].subscription, subscription);
    let id = app.views[0].id;
    app.set_display_enabled(id, false);
    wait(&mut app, "first Stop cleanup", 10, |a| {
        a.views[0].subscription.is_none()
    });
    app.remove_view(id);
    let shown = Instant::now();
    app.show_topic(
        Output::SpatialScan,
        "/studio_fixture/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    wait(&mut app, "one Show ready geometry", 3, |a| {
        !a.views[0].sensor_history.points().is_empty()
    });
    let first_scan_ms = shown.elapsed().as_millis();
    let until = Instant::now() + Duration::from_secs(15);
    let mut received = 0;
    let mut observed_ticks = 0;
    let mut last_success_stamp = app.views[0].sensor_history.oldest_acquisition_stamp();
    let mut last_success_at = Instant::now();
    let mut successful_updates = 0;
    let mut maximum_success_gap = Duration::ZERO;
    while Instant::now() < until {
        app.poll_backend();
        app.check_deadlines(Instant::now());
        assert!(
            app.views[0].enabled && app.views[0].error.is_none(),
            "{:?}",
            app.views[0].error
        );
        let stamp = app.views[0].sensor_history.oldest_acquisition_stamp();
        if let Some(stamp) = stamp.filter(|stamp| Some(*stamp) != last_success_stamp) {
            if let Some(previous) = last_success_stamp {
                assert!(stamp > previous);
            }
            maximum_success_gap = maximum_success_gap.max(last_success_at.elapsed());
            last_success_at = Instant::now();
            last_success_stamp = Some(stamp);
            successful_updates += 1;
        }
        assert!(
            last_success_at.elapsed() < Duration::from_millis(500),
            "Visible frame stopped advancing"
        );
        observed_ticks += 1;
        received += u64::from(!app.views[0].sensor_history.points().is_empty());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        received > 100 && received * 100 >= observed_ticks * 95,
        "Scan must remain visible during transient TF retries: {received}/{observed_ticks}"
    );
    assert!(
        successful_updates >= 100,
        "Insufficient completed scan updates: {successful_updates}"
    );
    assert!(app.views[0].spatial_counters.tf_accepted > 256);
    assert!(app.views[0].spatial_counters.tf_rejected > 0);
    assert!(
        app.observed_tf_frames.iter().any(|f| f == "map")
            && app.observed_tf_frames.iter().any(|f| f == "laser")
            && app.observed_tf_frames.iter().any(|f| f == "odom")
    );
    let active_subscription = app.views[0].subscription;
    app.apply_scene_settings().unwrap();
    assert_eq!(app.views[0].subscription, active_subscription);
    let counts = publisher.send("counts");
    assert_eq!(counts["scan"], 1);
    assert_eq!(counts["dynamic_tf"], 1);
    assert_eq!(counts["static_tf"], 1);
    let counters = app.views[0].spatial_counters;
    let frames = app.observed_tf_frames.clone();
    app.scene_settings.draft.fixed_frame = "odom".into();
    app.apply_scene_settings().unwrap();
    wait(&mut app, "changed-frame cleanup", 10, |a| {
        a.scene_settings.pending.is_none()
    });
    app.stop();
    session::finish_shutdown();
    app.connect();
    wait(&mut app, "inactive reconnect", 30, |a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    assert_eq!(app.scene_settings.configuration.fixed_frame, "odom");
    assert!(
        app.scene_settings.verified && !app.views[0].enabled && app.views[0].subscription.is_none()
    );
    app.stop();
    session::finish_shutdown();
    let until = Instant::now() + Duration::from_secs(10);
    let final_counts = loop {
        let c = publisher.send("counts");
        if c["scan"] == 0 && c["dynamic_tf"] == 0 && c["static_tf"] == 0 {
            break c;
        }
        assert!(Instant::now() < until, "{c}");
        std::thread::sleep(Duration::from_millis(50));
    };
    publisher.finish();
    std::fs::write(folder.join("result.json"),serde_json::to_vec_pretty(&json!({"outcome":"passed","middleware":std::env::var("RMW_IMPLEMENTATION").unwrap(),"first_scan_ms":first_scan_ms,"visible_ticks":received,"successful_updates":successful_updates,"maximum_success_gap_ms":maximum_success_gap.as_millis(),"observed_ticks":observed_ticks,"tf_counters":counters,"observed_frames":frames,"active_counts":counts,"final_counts":final_counts,"high_rate_seconds":15,"tf_hz":100,"scan_hz":10})).unwrap()).unwrap();
}
