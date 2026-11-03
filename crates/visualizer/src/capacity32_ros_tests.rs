//! Actual32-reader localhost ROS workflow; no native window or hardware traffic.
use super::*;
use serde_json::json;
use std::{path::PathBuf, process::Command as Process};
use studio_client_core::map_fixture::FixturePublisher;
fn wait(app: &mut App, label: &str, mut ready: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
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
                .map(|v| (&v.configuration.topic, v.status, &v.error, v.image_sequence))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "Requires isolated localhost domain187 and matching DDS;32Image App/backend acceptance"]
fn capacity32_real_ros_reception_replacement_stop_and_reconnect() {
    for (k, v) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(k).as_deref(), Ok(v));
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
        .arg(root.join("crates/backend/tests/fixtures/capacity32_publisher.py"))
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
                .filter(|(t, _)| t.starts_with("/images/"))
                .count()
                == 33
    });
    assert_eq!(publisher.send("counts")["readers"], json!(vec![0; 33]));
    let started = Instant::now();
    for i in 0..32 {
        app.begin_source(
            Output::CompactImage,
            &format!("/images/image{i}"),
            images::IMAGE_MESSAGE_TYPE,
        );
        app.draft_transient_local = true;
        app.subscribe();
        app.set_display_enabled(app.selected.unwrap(), true);
    }
    wait(&mut app, "all32 retained Images", |a| {
        a.views
            .iter()
            .all(|v| v.status == ViewStatus::Receiving && v.image_sequence > 0)
    });
    let first_delivery_ms = started.elapsed().as_millis();
    let counts = publisher.send("counts");
    assert_eq!(counts["readers"], json!([vec![1; 32], vec![0]].concat()));
    for (i, view) in app.views.iter().enumerate() {
        let frame = view.image.frames().take().unwrap();
        assert_eq!(frame.pixels()[0].r(), i as u8);
    }
    app.show_topic(
        Output::CompactImage,
        "/images/image32",
        images::IMAGE_MESSAGE_TYPE,
    );
    assert_eq!(app.views.len(), 32);
    assert!(app
        .topic_action_error
        .as_deref()
        .unwrap()
        .contains("32 displays"));
    let start_counts: Vec<_> = app.views.iter().map(|v| v.image_sequence).collect();
    let until = Instant::now() + Duration::from_secs(10);
    let mut sequence = 2;
    let mut max_poll_ms = 0f64;
    while Instant::now() < until {
        publisher.send(&format!("publish {sequence}"));
        sequence += 1;
        let slice = Instant::now() + Duration::from_millis(100);
        while Instant::now() < slice {
            let before = Instant::now();
            app.poll_backend();
            max_poll_ms = max_poll_ms.max(before.elapsed().as_secs_f64() * 1000.);
            assert!(app.views.iter().all(|v| v.error.is_none() && v.enabled));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(app
        .views
        .iter()
        .zip(&start_counts)
        .all(|(v, n)| v.image_sequence >= n + 20));
    let received: Vec<_> = app.views.iter().map(|v| v.image_sequence).collect();
    let queue_drops: Vec<_> = app.views.iter().map(|v| v.queue_drops).collect();
    let middle = app.views[15].id;
    app.set_display_enabled(middle, false);
    wait(&mut app, "scoped Stop", |a| {
        a.views[15].subscription.is_none()
    });
    assert_eq!(app.views.iter().filter(|v| v.enabled).count(), 31);
    let first = app.views[0].id;
    app.configure_view(first);
    app.discovery.select("/images/image32");
    app.subscribe();
    wait(&mut app, "source replacement", |a| {
        a.views[0].configuration.topic == "/images/image32"
            && a.views[0].status == ViewStatus::Receiving
    });
    let new_frame = app.views[0].image.frames().take().unwrap();
    assert_eq!(new_frame.pixels()[0].r(), 32);
    assert!(app.views[31].enabled && app.views[31].status == ViewStatus::Receiving);
    app.set_display_enabled(middle, true);
    wait(&mut app, "retained restart", |a| {
        a.views.iter().all(|v| v.status == ViewStatus::Receiving)
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
        .all(|v| !v.enabled && v.subscription.is_none()));
    let deadline = Instant::now() + Duration::from_secs(15);
    let final_counts = loop {
        let c = publisher.send("counts");
        if c["readers"] == json!(vec![0; 33]) {
            break c;
        }
        assert!(Instant::now() < deadline, "{c}");
        std::thread::sleep(Duration::from_millis(50));
    };
    app.stop();
    session::finish_shutdown();
    publisher.finish();
    std::fs::write(folder.join("result.json"),serde_json::to_vec_pretty(&json!({"outcome":"passed","test_pid":std::process::id(),"first32_delivery_ms":first_delivery_ms,"load_seconds":10,"image_dimensions":[64,48],"offered_batches":sequence-2,"received_sequences":received,"application_queue_drops":queue_drops,"maximum_poll_ms":max_poll_ms,"active_counts":counts,"final_counts":final_counts,"cases":["discovery passive","32 exact retainedImages","33 rejection preserves peers","10second reception","scoped Stop","replacement preserves peers","retained restart","inactive reconnect","owned process cleanup"]})).unwrap()).unwrap();
}
