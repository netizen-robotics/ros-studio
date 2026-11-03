use super::*;
use studio_protocol::{
    camera_info::Calibration,
    camera_snapshot::{self as camera, transport::Message, InfoSource, Owner, Snapshot, Status},
    image::{HeaderStamp, ImageMetadata, ImageSample},
    latest_pose::LatestPose,
};
fn output() -> Output {
    Output::CameraSnapshot(Box::new(InfoSource {
        topic: "/camera_info".into(),
        best_effort: false,
        transient_local: false,
    }))
}
fn snapshot(start: &Command, revision: u64, receiving: bool) -> Snapshot {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = start
    else {
        panic!()
    };
    let Output::CameraSnapshot(info) = &configuration.output else {
        panic!()
    };
    let mut snapshot = Snapshot {
        contract_version: camera::CONTRACT_VERSION,
        owner: Owner {
            generation: *generation,
            subscription: *subscription,
            topic: configuration.topic.clone(),
            info: (**info).clone(),
            fixed_frame: "world".into(),
        },
        revision,
        continuity_epoch: 1,
        status: Status::WaitingInfo,
        diagnostic: None,
        counters: Default::default(),
        frame: None,
    };
    if receiving {
        let calibration:Calibration=serde_json::from_value(serde_json::json!({
            "stamp_ns":1,"frame_id":"optical","width":2,"height":2,
            "distortion_model":"plumb_bob","d":[],"k":[1.,0.,1.,0.,1.,1.,0.,0.,1.],
            "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],"p":[1.,0.,1.,0.,0.,1.,1.,0.,0.,0.,1.,0.],
            "binning_x":0,"binning_y":0,"roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}
        })).unwrap();
        let image = ImageSample::from_raw(
            &configuration.topic,
            ImageMetadata {
                header_stamp: HeaderStamp {
                    seconds: 1,
                    nanoseconds: 0,
                },
                frame_id: "optical".into(),
                width: 2,
                height: 2,
                encoding: "rgba8".into(),
                is_bigendian: false,
                step: 8,
            },
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
        .unwrap();
        snapshot.status = Status::Receiving;
        snapshot.counters.images = 1;
        snapshot.counters.calibrations = 1;
        snapshot.frame = Some(Box::new(camera::Frame {
            image_sequence: 1,
            calibration_revision: 1,
            image,
            calibration,
            transform: LatestPose::Ready {
                resolved_stamp_ns: 1_000_000_000,
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        }));
    }
    snapshot
}
fn deliver(app: &mut App, start: &Command, sequence: u64, sample: Snapshot) {
    let (generation, _, subscription) = start.ids();
    ingest(
        app,
        Event::CameraSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample: Box::new(Message::Snapshot(Box::new(sample))),
            backend_drops: 0,
        },
    );
    app.poll_cameras();
}
#[test]
fn camera_draft_stays_camera_and_add_enable_waiting_receiving_are_explicit() {
    let (mut app, outgoing) = connected();
    app.begin_source(output(), "/camera", images::IMAGE_MESSAGE_TYPE);
    let context = egui::Context::default();
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            app.setup_ui(ui);
        });
    });
    assert!(matches!(app.draft_output, Output::CameraSnapshot(_)));
    assert_eq!(
        crate::camera_display::inferred_info("/namespace/image_rect"),
        "/namespace/camera_info"
    );
    app.subscribe();
    let id = app.selected.unwrap();
    assert!(!app.views[0].enabled);
    assert!(outgoing.try_recv().is_err());
    app.set_display_enabled(id, true);
    let start = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(&start,Command::Start{configuration,..} if configuration.owns_tf() && matches!(&configuration.output,Output::CameraSnapshot(info) if info.topic=="/camera_info"))
    );
    assert!(app.calibrated_scene);
    ingest(&mut app, started(&start));
    deliver(&mut app, &start, 1, snapshot(&start, 1, false));
    assert_eq!(app.views[0].status, ViewStatus::Waiting);
    assert!(!app.views[0].camera.is_current());
    deliver(&mut app, &start, 2, snapshot(&start, 2, true));
    assert!(app.views[0].camera.is_current());
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    let permission = app.views[0].camera.permission.clone().unwrap();
    app.configure_view(id);
    if let Output::CameraSnapshot(info) = &mut app.draft_output {
        info.topic = "/changed_info".into();
    }
    assert!(permission.is_valid());
    assert!(
        matches!(&app.views[0].configuration.output,Output::CameraSnapshot(info) if info.topic=="/camera_info")
    );
    app.subscribe();
    let stop = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert!(!permission.is_valid());
    ingest(&mut app, stopped(&stop));
    let replacement = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(&replacement,Command::Start{configuration,..} if matches!(&configuration.output,Output::CameraSnapshot(info) if info.topic=="/changed_info"))
    );
    assert!(!app.views[0].camera.is_current());
}
#[test]
fn camera_start_cancellation_and_disconnect_do_not_replay_or_stop_image_peer() {
    let (mut app, outgoing) = connected();
    let peer = add(
        &mut app,
        Output::CompactImage,
        "/camera_two",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.set_display_enabled(peer, true);
    let peer_start = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    ingest(&mut app, started(&peer_start));
    let id = add(&mut app, output(), "/camera", images::IMAGE_MESSAGE_TYPE);
    app.set_display_enabled(id, true);
    let start = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    let index = app.views.iter().position(|v| v.id == id).unwrap();
    let stream = app.views[index].camera.stream.clone().unwrap();
    app.set_display_enabled(id, false);
    let stop = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(stream.is_stopped());
    ingest(&mut app, started(&start));
    deliver(&mut app, &start, 1, snapshot(&start, 1, true));
    assert!(!app.views[index].enabled && !app.views[index].camera.is_current());
    assert!(app.views.iter().find(|v| v.id == peer).unwrap().enabled);
    ingest(&mut app, stopped(&stop));
    assert!(outgoing.try_recv().is_err());
    app.stop();
    assert!(app.views.iter().all(|v| !v.enabled));
    assert!(app.views[index].camera.stream.is_none());
}
#[test]
fn camera_workspace17_preparation_restores_inactive_and_never_grants_clock_or_pixels() {
    let (mut app, outgoing) = connected();
    let id = add(&mut app, output(), "/camera", images::IMAGE_MESSAGE_TYPE);
    app.views[0].camera.appearance.zoom_milli = 2000;
    app.views[0].camera.appearance.placement = studio_workspace::camera::ImagePlacement::Both;
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let parsed = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&parsed.workspace).unwrap();
    let view = &prepared.views[0];
    assert!(matches!(view.kind, ViewKind::Camera));
    assert!(!view.enabled && !view.spatial_clock_verified);
    assert!(
        view.subscription.is_none() && view.camera.stream.is_none() && !view.camera.is_current()
    );
    assert_eq!(view.camera.appearance, app.views[0].camera.appearance);
    assert_eq!(
        view.configuration.spatial.as_ref().unwrap().clock_policy,
        SpatialClockPolicy::Unknown
    );
    assert!(outgoing.try_recv().is_err());
    assert_eq!(app.selected, Some(id));
}
