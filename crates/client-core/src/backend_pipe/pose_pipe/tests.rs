use super::*;
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{Entry, HistorySettings, Kind, ResetReason, Source},
    session::Event,
    spatial,
    spatial_input::SpatialClockPolicy,
    uncertainty::Covariance,
};
fn configuration(kind: Kind) -> studio_protocol::subscriptions::Configuration {
    studio_protocol::subscriptions::Configuration {
        topic: format!(
            "/{}",
            if kind == Kind::Odometry {
                "odom"
            } else {
                "covariance"
            }
        ),
        message_type: kind.message_type().into(),
        best_effort: false,
        transient_local: true,
        output: Output::PoseSnapshot(Configuration {
            kind,
            history: (kind == Kind::Odometry).then(Default::default),
        }),
        spatial: Some(spatial::Configuration {
            fixed_frame: "world".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
        robot: None,
        map_updates: None,
    }
}
fn bind(shared: &mut Shared, kind: Kind, id: u64) -> PoseTarget {
    let config = configuration(kind);
    let Output::PoseSnapshot(pose) = config.output else {
        unreachable!()
    };
    let start = shared
        .registry
        .start_explicit(ViewId(id), config.clone())
        .unwrap();
    shared
        .queue
        .register(start.generation, start.subscription)
        .unwrap();
    let owner = Owner {
        generation: start.generation,
        subscription: start.subscription,
        kind,
        topic: config.topic,
        fixed_frame: "world".into(),
    };
    let target = PoseTarget {
        stream: PoseStream::new(owner.clone(), pose).unwrap(),
        owner,
        configuration: pose,
    };
    shared.bind_pose(target.clone()).unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: start.generation,
            request: start.request,
            subscription: start.subscription,
        })
        .unwrap();
    target
}
fn setup() -> (Shared, PoseTarget) {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture".into()))
        .unwrap();
    let target = bind(&mut shared, Kind::Odometry, 1);
    (shared, target)
}
fn snapshot(target: &PoseTarget, revision: u64, epoch: u64) -> Snapshot {
    Snapshot {
        contract_version: wire::CONTRACT_VERSION,
        owner: target.owner.clone(),
        revision,
        continuity_epoch: epoch,
        source: Some(Source {
            frame_id: "world".into(),
            child_frame_id: (target.owner.kind == Kind::Odometry).then(|| "base".into()),
        }),
        history: (target.owner.kind == Kind::Odometry).then(HistorySettings::default),
        latest: Some(Entry {
            sequence: revision,
            acquisition_stamp_ns: 10,
            components: [1., 0., 0., 0., 0., 0., 1.],
            covariance: Covariance::new([0.; 36]).unwrap(),
            transform: LatestPose::Ready {
                resolved_stamp_ns: 10,
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        }),
        trail: vec![],
        last_reset: None,
        known_history_gap: false,
        counters: Default::default(),
    }
}
fn event(target: &PoseTarget, sequence: u64, snapshot: Snapshot) -> Event {
    Event::PoseSnapshotObservation {
        generation: target.owner.generation,
        subscription: target.owner.subscription,
        sequence,
        sample: Box::new(Message::Snapshot(Box::new(snapshot))),
        backend_drops: 0,
    }
}
fn permission(target: &PoseTarget) -> Permission {
    match target.stream.take().unwrap().unwrap() {
        PoseUpdate::Snapshot { permission, .. } => permission,
        _ => panic!("Missing pose snapshot"),
    }
}
fn ack(target: &PoseTarget, request: RequestId) -> Event {
    Event::PoseCleared {
        generation: target.owner.generation,
        subscription: target.owner.subscription,
        request,
        barrier: Box::new(Invalidation {
            contract_version: wire::CONTRACT_VERSION,
            owner: target.owner.clone(),
            revision: 3,
            continuity_epoch: 2,
            reason: ResetReason::LocalClear,
        }),
    }
}
#[test]
fn clear_blocks_queued_data_until_exact_ack_then_rejects_prior_epoch() {
    let (mut shared, target) = setup();
    shared
        .ingest_for_test(event(&target, 1, snapshot(&target, 1, 1)))
        .unwrap();
    let old = permission(&target);
    let command = shared
        .clear_pose_explicit(target.owner.subscription)
        .unwrap();
    let (_, request, _) = command.ids();
    assert!(!old.is_valid());
    shared
        .ingest_for_test(event(&target, 2, snapshot(&target, 2, 1)))
        .unwrap();
    assert!(target.stream.take().is_none());
    shared
        .ingest_for_test(ack(&target, RequestId(request.0 + 1)))
        .unwrap();
    assert!(target.stream.is_waiting_for_clear());
    shared.ingest_for_test(ack(&target, request)).unwrap();
    assert!(!target.stream.is_waiting_for_clear());
    shared
        .ingest_for_test(event(&target, 3, snapshot(&target, 2, 1)))
        .unwrap();
    assert!(target.stream.take().is_none());
    shared
        .ingest_for_test(event(&target, 4, snapshot(&target, 4, 2)))
        .unwrap();
    assert!(permission(&target).is_valid());
}
#[test]
fn scoped_malformed_owner_and_stop_revoke_cached_results_without_touching_peer() {
    let (mut shared, target) = setup();
    let peer = bind(&mut shared, Kind::PoseWithCovariance, 2);
    shared
        .ingest_for_test(event(&target, 1, snapshot(&target, 1, 1)))
        .unwrap();
    let old = permission(&target);
    shared
        .ingest_for_test(event(&peer, 1, snapshot(&peer, 1, 1)))
        .unwrap();
    let peer_permission = permission(&peer);
    let mut bad = snapshot(&target, 2, 1);
    bad.history = None;
    shared.ingest_for_test(event(&target, 2, bad)).unwrap();
    assert!(target.stream.take().unwrap().is_err());
    assert!(!old.is_valid());
    assert!(peer_permission.is_valid());
    assert!(shared
        .clear_pose_explicit(target.owner.subscription)
        .is_err());
    shared.stop_pose_explicit(peer.owner.subscription).unwrap();
    assert!(!peer_permission.is_valid());
}
#[test]
fn sequence_and_stale_decode_checks_prevent_mailbox_republication_after_stop() {
    let (mut shared, target) = setup();
    let message = Message::Snapshot(Box::new(snapshot(&target, 1, 1)));
    let prepared = prepare(
        &mut shared,
        target.owner.generation,
        target.owner.subscription,
        1,
    )
    .unwrap();
    let result = decode(&prepared, &message, 0).unwrap();
    assert!(prepare(
        &mut shared,
        target.owner.generation,
        target.owner.subscription,
        1
    )
    .is_none());
    shared
        .stop_pose_explicit(target.owner.subscription)
        .unwrap();
    publish(&mut shared, &prepared, result);
    assert!(target.stream.take().is_none());
    assert!(target
        .stream
        .0
        .receiver
        .lock()
        .unwrap()
        .permission()
        .is_none());
}
#[test]
fn failed_clear_remains_hidden_and_needs_stop_restart() {
    let (mut shared, target) = setup();
    shared
        .ingest_for_test(event(&target, 1, snapshot(&target, 1, 1)))
        .unwrap();
    let old = permission(&target);
    let c = shared
        .clear_pose_explicit(target.owner.subscription)
        .unwrap();
    let (g, r, s) = c.ids();
    shared.pose_clear_send_failed(g, s, r);
    assert!(!old.is_valid());
    assert!(target.stream.take().unwrap().is_err());
    shared
        .ingest_for_test(event(&target, 2, snapshot(&target, 9, 3)))
        .unwrap();
    assert!(target.stream.take().is_none());
    assert!(shared.clear_pose_explicit(s).is_err());
    shared.stop_pose_explicit(s).unwrap();
}
#[cfg(unix)]
#[test]
fn production_reader_does_not_hold_shared_during_decode_and_clear_is_nonblocking() {
    use std::{io::Write, os::unix::net::UnixStream, thread, time::Duration};
    let shared = Arc::new(Mutex::new(Shared::new(0)));
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let (tx, _) = mpsc::sync_channel(1);
    let worker = super::super::spawn(reader, shared.clone(), tx);
    writer
        .write_all(
            &Event::ready("jazzy".into(), "fixture".into())
                .encode()
                .unwrap(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while shared.lock().unwrap().registry.generation().0 == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let target = bind(&mut shared.lock().unwrap(), Kind::Odometry, 1);
    shared
        .lock()
        .unwrap()
        .ingest_for_test(event(&target, 1, snapshot(&target, 1, 1)))
        .unwrap();
    let old = permission(&target);
    let held = target.stream.0.receiver.lock().unwrap();
    writer
        .write_all(&event(&target, 2, snapshot(&target, 2, 1)).encode().unwrap())
        .unwrap();
    while shared.lock().unwrap().registry.entries()[0].last_sequence != Some(2) {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let command = shared
        .try_lock()
        .expect("Decode held UI registry lock")
        .clear_pose_explicit(target.owner.subscription)
        .unwrap();
    assert!(!old.is_valid());
    drop(held);
    writer
        .write_all(&ack(&target, command.ids().1).encode().unwrap())
        .unwrap();
    while target.stream.is_waiting_for_clear() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    drop(writer);
    worker.join().unwrap();
    assert!(target.stream.is_stopped());
    assert!(target.stream.take().is_none());
}
