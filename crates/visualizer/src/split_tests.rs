use super::*;
#[test]
fn application_capability_boundary() {
    assert!(!allowed_output(&Output::Typed));
    assert!(allowed_output(&Output::CompactImage));
    assert!(allowed_output(&Output::Robot));
    assert!(allowed_output(&Output::PointCloud));
}
#[test]
fn root_starts_disconnected_and_empty() {
    let app = App::default();
    assert_eq!(app.connection.phase, Phase::Disconnected);
    assert_eq!(app.connection.domain_id(), Ok(0));
    assert!(app.views.is_empty());
    assert!(app.session.is_none());
}
#[test]
#[ignore = "Requires scripts/test-split-apps-ros.py localhost publishers and explicit middleware"]
fn split_real_ros_visualizer_workflow() {
    assert_eq!(std::env::var("ROS_DOMAIN_ID").unwrap(), "187");
    assert_eq!(std::env::var("ROS_LOCALHOST_ONLY").unwrap(), "1");
    assert_eq!(
        std::env::var("ROS_AUTOMATIC_DISCOVERY_RANGE").unwrap(),
        "LOCALHOST"
    );
    struct Finished;
    impl Drop for Finished {
        fn drop(&mut self) {
            session::finish_shutdown();
        }
    }
    let _finished = Finished;
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect();
    let wait = |app: &mut App, test: &dyn Fn(&App) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(35);
        while !test(app) {
            app.poll_backend();
            app.poll_scene_settings();
            app.check_deadlines(Instant::now());
            assert!(
                Instant::now() < deadline,
                "Timed out: {}",
                app.connection.status
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(&mut app, &|a| {
        a.connection.phase == Phase::Connected
            && a.discovery
                .topics
                .iter()
                .any(|(t, _)| t == "/studio_fixture/split/scan")
    });
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "split_world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.begin_source(
        Output::SpatialScan,
        "/studio_fixture/split/scan_best",
        spatial::SCAN_MESSAGE_TYPE,
    );
    // This probe intentionally requests incompatible Reliable QoS, independent
    // of the product's Best effort scan default.
    app.draft_best_effort = false;
    app.subscribe();
    let id = app.selected.unwrap();
    assert!(!app.views[0].enabled);
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
    app.set_display_enabled(id, true);
    wait(&mut app, &|a| a.views[0].status == ViewStatus::Waiting);
    let mismatch_until = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < mismatch_until {
        app.poll_backend();
        assert_eq!(
            app.views[0].spatial_counters.scans_received, 0,
            "Reliable request must not consume a Best effort-only publisher"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    app.configure_view(id);
    app.draft_best_effort = true;
    app.subscribe();
    wait(&mut app, &|a| {
        a.views[0].spatial_counters.scans_received > 0
            && !a.views[0].sensor_history.points().is_empty()
    });
    let scan_subscription = app.views[0].subscription;
    let scan_token = app.views[0].sensor_token;
    let scan_style = studio_workspace::sensor::SensorAppearance {
        history_ms: 1000,
        color: studio_workspace::sensor::ColorMode::Intensity,
        pixels: 8,
        opacity: 128,
        ..app.views[0].sensor_appearance
    };
    app.views[0].set_sensor_appearance(scan_style).unwrap();
    wait(&mut app, &|a| a.views[0].sensor_history.frame_count() >= 2);
    assert!(app.views[0]
        .sensor_history
        .points()
        .iter()
        .any(|p| p.intensity == Some(42.)));
    assert!(app.views[0]
        .sensor_history
        .points()
        .iter()
        .any(|p| p.intensity.is_none()));
    let retained_scan_points = app.views[0].sensor_history.points().len();
    app.views[0].clear_sensor().unwrap();
    assert!(app.views[0].sensor_history.points().is_empty());
    wait(&mut app, &|a| {
        !a.views[0].sensor_history.points().is_empty()
    });
    assert_eq!(app.views[0].subscription, scan_subscription);
    assert_eq!(app.views[0].sensor_token, scan_token);
    app.begin_source(
        Output::CompactImage,
        "/studio_fixture/split/image",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.draft_best_effort = true;
    app.subscribe();
    let image = app.selected.unwrap();
    assert!(!app.views[1].enabled);
    app.set_display_enabled(image, true);
    wait(&mut app, &|a| a.views[1].image_sequence > 0);
    assert!(app.views[0].enabled);
    app.begin_source(Output::TfGraph, "/tf", spatial::TF_MESSAGE_TYPE);
    app.subscribe();
    let tf_id = app.selected.unwrap();
    assert!(!app.views[2].enabled);
    app.set_display_enabled(tf_id, true);
    wait(&mut app, &|a| a.views[2].tf_graph.ready_count() == 3);
    assert!(app.views[2].tf_graph.select("split_laser"));
    wait(&mut app, &|a| {
        a.discovery
            .topics
            .iter()
            .any(|(t, _)| t == "/studio_fixture/split/cloud")
    });
    app.begin_source(
        Output::PointCloud,
        "/studio_fixture/split/cloud",
        studio_protocol::cloud::MESSAGE_TYPE,
    );
    app.subscribe();
    let cloud_id = app.selected.unwrap();
    assert!(!app.views[3].enabled);
    app.set_display_enabled(cloud_id, true);
    wait(&mut app, &|a| {
        !a.views[3].sensor_history.points().is_empty()
    });
    let cloud_subscription = app.views[3].subscription;
    let cloud_token = app.views[3].sensor_token;
    let cloud_style = studio_workspace::sensor::SensorAppearance {
        history_ms: 1500,
        color: studio_workspace::sensor::ColorMode::Rgb,
        size_mode: studio_workspace::sensor::SizeMode::World,
        diameter_mm: 500,
        opacity: 128,
        ..app.views[3].sensor_appearance
    };
    app.views[3].set_sensor_appearance(cloud_style).unwrap();
    wait(&mut app, &|a| a.views[3].sensor_history.frame_count() >= 2);
    assert_eq!(
        app.views[3].sensor_history.points()[0].rgba,
        Some([240, 70, 50, 255])
    );
    assert_eq!(app.views[3].sensor_history.points()[4].intensity, None);
    assert_eq!(app.views[3].subscription, cloud_subscription);
    assert_eq!(app.views[3].sensor_token, cloud_token);
    let cloud_sequence = app.views[3].spatial_sequence;
    app.views[0].clear_sensor().unwrap();
    assert!(!app.views[3].sensor_history.points().is_empty());
    wait(&mut app, &|a| a.views[3].spatial_sequence > cloud_sequence);
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert!(!imported.legacy);
    let restored = app.workspace_prepare(&imported.workspace).unwrap();
    assert!(restored
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    assert!(restored.views[2].tf_graph.sample().is_none());
    assert!(!restored.views[2].spatial_clock_verified);
    assert!(restored
        .views
        .iter()
        .all(|v| v.sensor_token.is_none() && v.sensor_history.points().is_empty()));
    assert_eq!(restored.views[0].sensor_appearance, scan_style);
    assert_eq!(restored.views[3].sensor_appearance, cloud_style);
    if let Ok(folder) = std::env::var("STUDIO_UI_ROS_EVIDENCE") {
        let report = serde_json::json!({"test":"actual-root sensor appearance/history","outcome":"passed","rmw":std::env::var("RMW_IMPLEMENTATION").unwrap(),"domain":187,"scope":"Actual app state plus standard backend/local synthetic ROS; no native-window claim","scan_intensity_alignment":true,"scan_retained_points":retained_scan_points,"clear_without_reader_replacement":scan_subscription==app.views[0].subscription,"peer_clear_isolation":true,"cloud_rgba_and_missing_intensity":true,"inactive_v13_restore":true});
        std::fs::write(
            std::path::Path::new(&folder).join(format!(
                "{}-sensor-appearance.json",
                std::env::var("RMW_IMPLEMENTATION").unwrap()
            )),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    // Draft selection does not replace the active source or a peer.
    app.configure_view(id);
    app.draft_best_effort = false;
    app.discovery.topic = "/studio_fixture/split/scan".into();
    assert!(app.views[0].configuration.best_effort);
    assert_eq!(
        app.views[0].configuration.topic,
        "/studio_fixture/split/scan_best"
    );
    assert!(app.views[1].enabled);
    app.subscribe();
    wait(&mut app, &|a| {
        a.views[0].subscription.is_some()
            && !a.views[0].configuration.best_effort
            && a.views[0].status == ViewStatus::Receiving
    });
    let image_sequence = app.views[1].image_sequence;
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "split_base".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    wait(&mut app, &|a| a.scene_settings.pending.is_none());
    assert!(!app.views[0].enabled);
    assert!(app.views[0].subscription.is_none());
    assert_eq!(app.scene_settings.configuration.fixed_frame, "split_base");
    assert!(!app.views[2].enabled);
    assert!(app.views[2].tf_graph.sample().is_none());
    assert!(app.views[2].subscription.is_none());
    assert!(app.views[1].enabled);
    wait(&mut app, &|a| a.views[1].image_sequence > image_sequence);
    app.set_display_enabled(id, true);
    wait(&mut app, &|a| a.views[0].status == ViewStatus::Receiving);
    app.set_display_enabled(tf_id, true);
    wait(&mut app, &|a| a.views[2].tf_graph.ready_count() == 3);
    app.set_display_enabled(tf_id, false);
    wait(&mut app, &|a| a.views[2].subscription.is_none());
    app.set_display_enabled(id, false);
    wait(&mut app, &|a| a.views[0].subscription.is_none());
    assert!(app.views[1].enabled);
    app.stop();
    app.connect();
    wait(&mut app, &|a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    assert!(app.views.iter().all(|v| !v.enabled));
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
    app.stop();
}
