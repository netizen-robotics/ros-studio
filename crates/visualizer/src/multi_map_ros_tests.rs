//! Real backend and local synthetic publishers, separate from native/GPU acceptance.
use super::*;
use serde_json::json;
use std::{path::PathBuf, process::Command as Process};
use studio_client_core::map_fixture::FixturePublisher;
fn wait(app: &mut App, label: &str, mut ready: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.check_deadlines(Instant::now());
        if ready(app) {
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
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn cells(app: &App, index: usize) -> Option<&[i8]> {
    app.views[index]
        .map
        .frame
        .as_ref()
        .map(|m| m.complete().grid.cells.as_ref())
}
#[test]
#[ignore = "Requires isolated localhost domain187 and matching RMW; records multi-map ROS evidence"]
fn three_maps_real_ros_isolated_updates_and_lifecycle() {
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
    assert!(folder.starts_with(root.join("artifacts/feasibility/evidence")));
    struct Finish;
    impl Drop for Finish {
        fn drop(&mut self) {
            session::finish_shutdown();
        }
    }
    let _finish = Finish;
    let mut command = Process::new(root.join(".pixi/envs/default/bin/python"));
    command
        .arg(root.join("crates/backend/tests/fixtures/multi_map_publisher.py"))
        .env("DYLD_LIBRARY_PATH", root.join(".pixi/envs/default/lib"))
        .stderr(std::fs::File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovery", |a| {
        a.connection.phase == Phase::Connected
            && a.discovery
                .topics
                .iter()
                .any(|(t, _)| t == "/local_costmap/costmap_updates")
    });
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "world".into();
    app.apply_scene_settings().unwrap();
    let topics = ["/map", "/global_costmap/costmap", "/local_costmap/costmap"];
    for topic in topics {
        app.begin_source(
            Output::MapSnapshot,
            topic,
            studio_protocol::mapping::MAP_MESSAGE_TYPE,
        );
        app.draft_best_effort = false;
        app.draft_transient_local = true;
        app.draft_map_updates = Some(studio_protocol::map_grid::UpdateSource {
            topic: format!("{topic}_updates"),
            best_effort: false,
            transient_local: false,
        });
        app.subscribe();
        let id = app.selected.unwrap();
        app.set_display_enabled(id, true);
    }
    wait(&mut app, "three retained full maps", |a| {
        (0..3).all(|i| cells(a, i).is_some_and(|c| c == [i as i8 * 40; 64]))
    });
    let active_counts = publisher.send("counts");
    assert_eq!(active_counts["full"], json!([1, 1, 1]));
    assert_eq!(active_counts["updates"], json!([1, 1, 1]));
    assert_eq!(active_counts["dynamic_tf"], 1);
    assert_eq!(active_counts["static_tf"], 1);
    for i in 0..3 {
        publisher.send(&format!("patch {i} {}", 10 + i));
        wait(&mut app, "independent patch", |a| {
            cells(a, i).is_some_and(|c| c[0] == (10 + i) as i8)
        });
    }
    let peer = app.views[0].map.frame.as_ref().unwrap().clone();
    publisher.send("invalid 1 50");
    wait(&mut app, "scoped invalid update", |a| {
        a.views[1].map.frame.is_none()
    });
    assert!(std::sync::Arc::ptr_eq(
        app.views[0].map.frame.as_ref().unwrap(),
        &peer
    ));
    assert!(cells(&app, 2).is_some_and(|c| c[0] == 12));
    publisher.send("full 1 60");
    wait(&mut app, "fresh full recovery", |a| {
        cells(a, 1).is_some_and(|c| c == [60; 64])
    });
    let second = app.views[1].id;
    app.set_display_enabled(second, false);
    wait(&mut app, "scoped Stop", |a| {
        a.views[1].subscription.is_none()
    });
    assert!(app.views[1].map.frame.is_none());
    assert!(std::sync::Arc::ptr_eq(
        app.views[0].map.frame.as_ref().unwrap(),
        &peer
    ));
    app.set_display_enabled(second, true);
    wait(&mut app, "retained restart", |a| {
        cells(a, 1).is_some_and(|c| c == [60; 64])
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
        .all(|v| !v.enabled && v.subscription.is_none() && v.map.frame.is_none()));
    let deadline = Instant::now() + Duration::from_secs(15);
    let final_counts = loop {
        let counts = publisher.send("counts");
        if counts["full"] == json!([0, 0, 0])
            && counts["updates"] == json!([0, 0, 0])
            && counts["dynamic_tf"] == 0
            && counts["static_tf"] == 0
        {
            break counts;
        }
        assert!(Instant::now() < deadline, "{counts}");
        std::thread::sleep(Duration::from_millis(50));
    };
    app.stop();
    session::finish_shutdown();
    publisher.finish();
    std::fs::write(folder.join("result.json"),serde_json::to_vec_pretty(&json!({"outcome":"passed","test_pid":std::process::id(),"active_counts":active_counts,"final_counts":final_counts,"cases":["three retained maps","eight-reader budget with shared TF","independent ordered patches","scoped invalidation and recovery","scoped Stop and retained restart","inactive reconnect","publisher and backend cleanup"]})).unwrap()).unwrap();
}
