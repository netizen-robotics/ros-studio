use super::*;
use studio_protocol::cloud::{self, Channel, Channels};
fn cloud_event(command: &Command, sequence: u64, tf: studio_protocol::spatial::TfOutcome) -> Event {
    let (generation, _, subscription) = command.ids();
    Event::PointCloudObservation {
        generation,
        subscription,
        sequence,
        backend_drops: 0,
        sample: Box::new(cloud::Sample {
            contract_version: cloud::CONTRACT_VERSION,
            topic: match command {
                Command::Start { configuration, .. } => configuration.topic.clone(),
                _ => unreachable!(),
            },
            message_type: cloud::MESSAGE_TYPE.into(),
            fixed_frame: "world".into(),
            source_frame: "sensor".into(),
            acquisition_stamp_ns: sequence as i64 * 1_000_000_000,
            source_points: 1,
            skipped_points: 0,
            points: vec![[1., 2., 3.]],
            channels: Channels {
                color: Channel::Values(vec![[10, 20, 30, 128]]),
                intensity: Channel::Values(vec![Some(42.)]),
            },
            tf,
            counters: Default::default(),
        }),
    }
}
fn ready() -> studio_protocol::spatial::TfOutcome {
    studio_protocol::spatial::TfOutcome::Ready {
        components: [0., 0., 0., 0., 0., 0., 1.],
    }
}
#[test]
fn malformed_sensor_data_and_invalid_appearance_preserve_peer_history_and_reception() {
    let (mut app, outgoing) = connected();
    app.discovery
        .topics
        .push(("/cloud".into(), vec![cloud::MESSAGE_TYPE.into()]));
    let first = add(&mut app, Output::PointCloud, "/cloud", cloud::MESSAGE_TYPE);
    app.set_display_enabled(first, true);
    let a = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&a));
    app.discovery
        .topics
        .push(("/cloud_peer".into(), vec![cloud::MESSAGE_TYPE.into()]));
    let second = add(
        &mut app,
        Output::PointCloud,
        "/cloud_peer",
        cloud::MESSAGE_TYPE,
    );
    app.set_display_enabled(second, true);
    let b = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&b));
    ingest(&mut app, cloud_event(&a, 1, ready()));
    ingest(&mut app, cloud_event(&b, 1, ready()));
    let old = app.views[0].sensor_appearance;
    let mut bad = old;
    bad.intensity_milli = [1, 1];
    assert_eq!(
        app.views[0].set_sensor_appearance(bad),
        Err("Maximum must be greater than minimum".into())
    );
    assert_eq!(app.views[0].sensor_appearance, old);
    assert_eq!(app.views[0].sensor_history.frame_count(), 1);
    let mut packet = cloud_event(&a, 2, ready());
    let Event::PointCloudObservation { sample, .. } = &mut packet else {
        unreachable!()
    };
    sample.channels.color = Channel::Values(vec![]);
    ingest(&mut app, packet);
    assert!(app.views[0].sensor_history.points().is_empty());
    assert!(app.views[0].sensor_token.is_none());
    let stop = outgoing.try_recv().unwrap();
    assert_eq!(stop.ids().2, a.ids().2);
    assert_eq!(app.views[1].sensor_history.frame_count(), 1);
    assert_eq!(app.views[1].status, ViewStatus::Receiving);
    ingest(&mut app, cloud_event(&b, 2, ready()));
    assert_eq!(app.views[1].sensor_history.frame_count(), 1);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn history_appearance_clear_idle_expiry_and_missing_tf_never_replace_the_reader() {
    let (mut app, outgoing) = connected();
    app.discovery
        .topics
        .push(("/cloud".into(), vec![cloud::MESSAGE_TYPE.into()]));
    let id = add(&mut app, Output::PointCloud, "/cloud", cloud::MESSAGE_TYPE);
    app.set_display_enabled(id, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    let settings = studio_workspace::sensor::SensorAppearance {
        history_ms: 1000,
        color: studio_workspace::sensor::ColorMode::Intensity,
        ..app.views[0].sensor_appearance
    };
    app.views[0].set_sensor_appearance(settings).unwrap();
    ingest(&mut app, cloud_event(&start, 1, ready()));
    assert_eq!(app.views[0].sensor_history.points()[0].intensity, Some(42.));
    assert_eq!(
        app.views[0].sensor_history.points()[0].rgba,
        Some([10, 20, 30, 128])
    );
    let before = app.workspace_snapshot().unwrap();
    let token = app.views[0].sensor_token;
    app.views[0].clear_sensor().unwrap();
    app.poll_backend();
    assert!(app.views[0].sensor_history.points().is_empty());
    assert_eq!(app.workspace_snapshot().unwrap(), before);
    assert_eq!(app.views[0].sensor_token, token);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    ingest(&mut app, cloud_event(&start, 2, ready()));
    assert_eq!(app.views[0].sensor_history.frame_count(), 1);
    app.views[0].origin = Instant::now() - Duration::from_millis(1200);
    app.poll_sensor_history(&egui::Context::default());
    assert!(app.views[0].sensor_history.points().is_empty());
    assert_eq!(app.views[0].sensor_history.counters().expired_frames, 1);
    ingest(&mut app, cloud_event(&start, 3, ready()));
    assert_eq!(app.views[0].sensor_history.frame_count(), 1);
    ingest(
        &mut app,
        cloud_event(&start, 4, studio_protocol::spatial::TfOutcome::Missing),
    );
    assert!(app.views[0].sensor_history.points().is_empty());
    assert_eq!(app.views[0].status, ViewStatus::Waiting);
    ingest(&mut app, cloud_event(&start, 5, ready()));
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    let mut appearance = settings;
    appearance.opacity = 64;
    app.views[0].set_sensor_appearance(appearance).unwrap();
    assert_eq!(app.views[0].sensor_history.frame_count(), 1);
    assert_eq!(app.views[0].sensor_token, token);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.set_display_enabled(id, false);
    let _stop = outgoing.try_recv().unwrap();
    assert_eq!(app.views[0].sensor_history.owned_sample_bytes(), 0);
    assert!(app.views[0].sensor_token.is_none());
}
#[test]
fn sensor_workspace_restore_preserves_intent_and_never_restores_points_or_permission() {
    let (mut app, outgoing) = connected();
    app.discovery
        .topics
        .push(("/cloud".into(), vec![cloud::MESSAGE_TYPE.into()]));
    let id = add(&mut app, Output::PointCloud, "/cloud", cloud::MESSAGE_TYPE);
    app.set_display_enabled(id, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    ingest(&mut app, cloud_event(&start, 1, ready()));
    let settings = studio_workspace::sensor::SensorAppearance {
        history_ms: 1500,
        pixels: 12,
        opacity: 128,
        color: studio_workspace::sensor::ColorMode::Rgb,
        ..app.views[0].sensor_appearance
    };
    app.views[0].set_sensor_appearance(settings).unwrap();
    let snapshot = app.workspace_snapshot().unwrap();
    assert_eq!(snapshot.presentation.as_ref().unwrap().sensors.len(), 1);
    let bytes = studio_workspace::application::encode(
        &snapshot,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert!(!imported.legacy);
    let mut restored = App::default();
    restored
        .workspace_review_configuration(imported.workspace)
        .unwrap();
    restored
        .workspace_begin_apply(crate::components::workspace::ApplyApproval::DiscardUnsaved)
        .unwrap();
    restored.workspace_poll(&egui::Context::default());
    assert_eq!(restored.workspace_snapshot().unwrap(), snapshot);
    let view = &restored.views[0];
    assert!(!view.enabled);
    assert!(view.subscription.is_none() && view.sensor_token.is_none());
    assert!(view.sensor_history.points().is_empty());
    assert!(!view.spatial_clock_verified);
    assert!(restored.scene_settings.verified);
    assert_eq!(view.sensor_appearance, settings);
}
