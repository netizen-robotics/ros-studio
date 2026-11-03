use super::*;
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{
        self as pose,
        transport::{Configuration as PoseConfiguration, Invalidation, Message},
        Kind,
    },
    uncertainty::Covariance,
};
fn app() -> (App, Receiver<Command>) {
    let (mut app, outgoing) = connected();
    let mut topics = app.discovery.topics.clone();
    topics.extend([
        ("/covariance".into(), vec![pose::POSE_MESSAGE_TYPE.into()]),
        ("/odom".into(), vec![pose::ODOMETRY_MESSAGE_TYPE.into()]),
    ]);
    ingest(&mut app, Event::Graph { topics });
    (app, outgoing)
}
fn activate(app: &mut App, outgoing: &Receiver<Command>, kind: Kind) -> (ViewId, Command) {
    let id = add(
        app,
        Output::PoseSnapshot(PoseConfiguration::for_kind(kind)),
        if kind == Kind::Odometry {
            "/odom"
        } else {
            "/covariance"
        },
        kind.message_type(),
    );
    assert!(!app.views.iter().find(|v| v.id == id).unwrap().enabled);
    assert!(outgoing.try_recv().is_err());
    app.set_display_enabled(id, true);
    let command = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    ingest(app, started(&command));
    (id, command)
}
fn sample(command: &Command, revision: u64, epoch: u64) -> pose::Snapshot {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!()
    };
    let Output::PoseSnapshot(c) = configuration.output else {
        panic!()
    };
    pose::Snapshot {
        contract_version: 1,
        owner: pose::Owner {
            generation: *generation,
            subscription: *subscription,
            kind: c.kind,
            topic: configuration.topic.clone(),
            fixed_frame: "world".into(),
        },
        revision,
        continuity_epoch: epoch,
        source: Some(pose::Source {
            frame_id: "world".into(),
            child_frame_id: (c.kind == Kind::Odometry).then(|| "base".into()),
        }),
        history: c.settings().unwrap(),
        latest: Some(pose::Entry {
            sequence: revision,
            acquisition_stamp_ns: revision as i64,
            components: [1., 2., 0., 0., 0., 0., 1.],
            covariance: Covariance::new(std::array::from_fn(|i| {
                if i / 6 == i % 6 {
                    0.04
                } else {
                    0.
                }
            }))
            .unwrap(),
            transform: LatestPose::Ready {
                resolved_stamp_ns: revision as i64,
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        }),
        trail: vec![],
        last_reset: None,
        known_history_gap: false,
        counters: pose::Counters {
            received: revision,
            ..Default::default()
        },
    }
}
fn deliver(app: &mut App, command: &Command, sequence: u64, snapshot: pose::Snapshot) {
    let (g, _, s) = command.ids();
    ingest(
        app,
        Event::PoseSnapshotObservation {
            generation: g,
            subscription: s,
            sequence,
            sample: Box::new(Message::Snapshot(Box::new(snapshot))),
            backend_drops: 0,
        },
    );
    app.poll_uncertainty();
}
#[test]
fn explicit_activation_clear_and_stale_epoch_preserve_peer_geometry() {
    let (mut app, outgoing) = app();
    let (cov_id, cov) = activate(&mut app, &outgoing, Kind::PoseWithCovariance);
    let (odom_id, odom) = activate(&mut app, &outgoing, Kind::Odometry);
    deliver(&mut app, &cov, 1, sample(&cov, 1, 1));
    deliver(&mut app, &odom, 1, sample(&odom, 1, 1));
    assert!(app
        .views
        .iter()
        .all(|v| v.uncertainty.layer(v.id.0).is_some()));
    let index = app.views.iter().position(|v| v.id == odom_id).unwrap();
    let old = app.views[index].uncertainty.permission.clone().unwrap();
    app.clear_uncertainty(index);
    let clear = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(clear, Command::ClearPose { .. }));
    assert!(!old.is_valid());
    assert!(app.views[index].uncertainty.layer(odom_id.0).is_none());
    assert!(app
        .views
        .iter()
        .find(|v| v.id == cov_id)
        .unwrap()
        .uncertainty
        .layer(cov_id.0)
        .is_some());
    let (g, r, s) = clear.ids();
    let mut barrier = Invalidation {
        contract_version: 1,
        owner: sample(&odom, 1, 1).owner,
        revision: 3,
        continuity_epoch: 2,
        reason: pose::ResetReason::LocalClear,
    };
    ingest(
        &mut app,
        Event::PoseCleared {
            generation: g,
            request: r,
            subscription: s,
            barrier: Box::new(barrier.clone()),
        },
    );
    app.poll_uncertainty();
    deliver(&mut app, &odom, 2, sample(&odom, 2, 1));
    assert!(app.views[index].uncertainty.layer(odom_id.0).is_none());
    deliver(&mut app, &odom, 3, sample(&odom, 4, 2));
    assert!(app.views[index].uncertainty.layer(odom_id.0).is_some());
    app.stop_view(odom_id);
    let stop = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(app.views[index].uncertainty.layer(odom_id.0).is_none());
    barrier.revision = 5;
    ingest(
        &mut app,
        Event::PoseCleared {
            generation: g,
            request: r,
            subscription: s,
            barrier: Box::new(barrier),
        },
    );
    assert!(app.views[index].uncertainty.layer(odom_id.0).is_none());
    ingest(&mut app, stopped(&stop));
    assert!(app
        .views
        .iter()
        .find(|v| v.id == cov_id)
        .unwrap()
        .uncertainty
        .layer(cov_id.0)
        .is_some());
}
#[test]
fn history_apply_waits_for_exact_stop_and_inactive_restore_has_no_live_authority() {
    let (mut app, outgoing) = app();
    let (id, start) = activate(&mut app, &outgoing, Kind::Odometry);
    let index = app.views.iter().position(|v| v.id == id).unwrap();
    deliver(&mut app, &start, 1, sample(&start, 1, 1));
    app.views[index].uncertainty.history_draft.keep = 2;
    app.apply_uncertainty_history(index);
    let stop = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert!(outgoing.try_recv().is_err());
    ingest(&mut app, stopped(&stop));
    let replacement = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        matches!(&replacement,Command::Start {configuration,..} if matches!(configuration.output,Output::PoseSnapshot(c) if c.history.unwrap().keep==2))
    );
    ingest(&mut app, started(&replacement));
    deliver(&mut app, &replacement, 1, sample(&replacement, 1, 1));
    app.views[index].uncertainty.appearance.sigma_milli = 2000;
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        studio_workspace::application::VISUALIZER_VERSION
    );
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&imported.workspace).unwrap();
    assert!(prepared.views.iter().all(|v| !v.enabled
        && v.subscription.is_none()
        && !v.spatial_clock_verified
        && v.uncertainty.stream.is_none()
        && v.uncertainty.prepared.is_none()));
    assert_eq!(prepared.views[0].uncertainty.appearance.sigma_milli, 2000);
    assert_eq!(prepared.views[0].uncertainty.history_draft.keep, 2);
}

#[test]
fn startup_cancel_rejects_late_started_and_pose_data_without_replaying_or_hiding_peer() {
    for kind in [Kind::PoseWithCovariance, Kind::Odometry] {
        let (mut app, outgoing) = app();
        let peer_kind = if kind == Kind::Odometry {
            Kind::PoseWithCovariance
        } else {
            Kind::Odometry
        };
        let (peer_id, peer_start) = activate(&mut app, &outgoing, peer_kind);
        deliver(&mut app, &peer_start, 1, sample(&peer_start, 1, 1));
        let id = add(
            &mut app,
            Output::PoseSnapshot(PoseConfiguration::for_kind(kind)),
            if kind == Kind::Odometry {
                "/odom"
            } else {
                "/covariance"
            },
            kind.message_type(),
        );
        app.set_display_enabled(id, true);
        let start = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
        let index = app.views.iter().position(|v| v.id == id).unwrap();
        assert_eq!(app.views[index].status, ViewStatus::Starting);
        app.stop_view(id);
        let stop = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(stop, Command::Stop { .. }));
        assert!(app.views[index].uncertainty.stream.is_none());
        ingest(&mut app, started(&start));
        deliver(&mut app, &start, 1, sample(&start, 1, 1));
        assert_eq!(app.views[index].status, ViewStatus::Stopping);
        assert!(!app.views[index].enabled);
        assert!(app.views[index].uncertainty.layer(id.0).is_none());
        assert!(app
            .views
            .iter()
            .find(|v| v.id == peer_id)
            .unwrap()
            .uncertainty
            .layer(peer_id.0)
            .is_some());
        ingest(&mut app, stopped(&stop));
        assert!(app.views[index].subscription.is_none());
        assert!(
            outgoing.try_recv().is_err(),
            "Cancellation must not replay Start"
        );
    }
}
