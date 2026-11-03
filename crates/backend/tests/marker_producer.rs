use studio_backend::marker_producer::MarkerProducer;
use studio_protocol::{
    latest_pose::LatestPose,
    marker::{self, Batch, Geometry, Key, Marker, Operation},
    marker_snapshot::{
        self as wire, ClearReason, Clock, LossSupport, Message, Owner, Received, Receiver,
    },
    spatial,
    subscriptions::{Generation, SubscriptionId},
};
fn owner() -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        topic: "/markers".into(),
        message_type: marker::ARRAY_MESSAGE_TYPE.into(),
        fixed_frame: "world".into(),
    }
}
fn clock(mono: u64) -> Clock {
    Clock {
        unix_ns: 1_000_000_000 + mono,
        monotonic_ns: mono,
    }
}
fn key(id: i32) -> Key {
    Key {
        namespace: "objects".into(),
        id,
    }
}
fn marker(id: i32, large: bool) -> Marker {
    Marker {
        key: key(id),
        frame_id: "world".into(),
        stamp_ns: 10,
        pose: [0., 0., 0., 0., 0., 0., 1.],
        scale: [1.; 3],
        color: [1.; 4],
        lifetime_ns: 0,
        frame_locked: false,
        geometry: if large {
            Geometry::Points {
                points: vec![[0.123456789, 0.234567891, 0.345678912]; marker::MAX_POINTS],
                colors: vec![[0.1, 0.2, 0.3, 0.4]; marker::MAX_POINTS],
            }
        } else {
            Geometry::Cube
        },
    }
}
fn add(m: Marker) -> Operation {
    Operation::Add {
        marker: Box::new(m),
    }
}
fn batch(operations: Vec<Operation>) -> Batch {
    Batch {
        contract_version: marker::CONTRACT_VERSION,
        operations,
    }
}
fn poll(p: &mut MarkerProducer, mono: u64) -> Option<Message> {
    p.poll(clock(mono), spatial::Counters::default()).unwrap()
}
fn send(p: &mut MarkerProducer, r: &mut Receiver, mono: u64) -> Option<Received> {
    poll(p, mono).map(|m| {
        m.validate().unwrap();
        r.receive(&Message::decode(&m.encode().unwrap()).unwrap(), clock(mono))
            .unwrap()
    })
}
fn finish(
    p: &mut MarkerProducer,
    r: &mut Receiver,
    mut mono: u64,
) -> std::sync::Arc<wire::Complete> {
    for _ in 0..wire::MAX_CHUNKS + 4 {
        if let Some(Received::Complete { state, .. }) = send(p, r, mono) {
            return state;
        }
        mono += 1;
    }
    panic!("Marker transfer did not complete")
}

#[test]
fn waiting_and_blocked_writer_apply_every_action_without_copy_or_replay() {
    let mut p = MarkerProducer::new(owner(), LossSupport::Unknown).unwrap();
    assert!(matches!(poll(&mut p,0),Some(Message::Clear(c)) if c.reason==ClearReason::Waiting));
    assert!(poll(&mut p, 1).is_none());
    p.input(
        1,
        batch(vec![add(marker(0, false)), add(marker(1, false))]),
        2,
    )
    .unwrap();
    p.input(2, batch(vec![Operation::Delete { key: key(0) }]), 3)
        .unwrap();
    for sequence in 3..103 {
        let mut m = marker(1, false);
        m.pose[0] = sequence as f64;
        p.input(sequence, batch(vec![add(m)]), sequence + 1)
            .unwrap();
    }
    assert_eq!(p.in_flight_bytes(), 0);
    assert_eq!(p.pending_prunes(), 0);
    assert_eq!(p.source().entries().len(), 1);
    assert_eq!(p.source().counters().batches, 102);
    let mut r = Receiver::new(owner()).unwrap();
    let state = finish(&mut p, &mut r, 200);
    assert_eq!(state.entries().len(), 1);
    assert_eq!(state.entries()[0].marker.pose[0], 102.);
    assert!(poll(&mut p, 201).is_none());
}

#[test]
fn removal_interleaves_with_one_frozen_transfer_and_coalesces_new_state_without_starving_peers() {
    let mut p = MarkerProducer::new(owner(), LossSupport::Unsupported).unwrap();
    let mut r = Receiver::new(owner()).unwrap();
    send(&mut p, &mut r, 0);
    for id in 0..4 {
        p.input(id as u64 + 1, batch(vec![add(marker(id, true))]), 1)
            .unwrap();
    }
    assert!(matches!(send(&mut p, &mut r, 2), Some(Received::Pending)));
    let frozen = p.in_flight_bytes();
    assert!(frozen > wire::MAX_CHUNK_BYTES);
    p.input(
        5,
        batch(vec![
            Operation::Delete { key: key(0) },
            add(marker(100, false)),
        ]),
        3,
    )
    .unwrap();
    // Changes to keys absent from both exported manifests need no removal log.
    for i in 0..100 {
        p.input(
            6 + i,
            batch(vec![
                add(marker(1000 + i as i32, false)),
                Operation::Delete {
                    key: key(1000 + i as i32),
                },
            ]),
            4 + i,
        )
        .unwrap();
    }
    assert_eq!(p.in_flight_bytes(), frozen);
    assert_eq!(p.pending_prunes(), 1);
    let removal = poll(&mut p, 200).unwrap();
    assert!(matches!(&removal,Message::Prune(prune) if prune.keys==vec![key(0)]));
    r.receive(
        &Message::decode(&removal.encode().unwrap()).unwrap(),
        clock(200),
    )
    .unwrap();
    let state = finish(&mut p, &mut r, 201);
    assert_eq!(
        state
            .entries()
            .iter()
            .map(|e| e.marker.key.id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(p.in_flight_bytes(), 0);
    let latest = finish(&mut p, &mut r, 100_000_000);
    assert_eq!(
        latest
            .entries()
            .iter()
            .map(|e| e.marker.key.id)
            .collect::<Vec<_>>(),
        [1, 2, 3, 100]
    );
}

#[test]
fn lifetime_expires_on_idle_client_and_producer_without_new_source_messages() {
    let mut p = MarkerProducer::new(owner(), LossSupport::Unknown).unwrap();
    let mut r = Receiver::new(owner()).unwrap();
    send(&mut p, &mut r, 0);
    let mut m = marker(0, false);
    m.lifetime_ns = 100;
    p.input(1, batch(vec![add(m), add(marker(1, false))]), 1)
        .unwrap();
    let state = finish(&mut p, &mut r, 2);
    assert_eq!(state.visible_entries(100).count(), 2);
    assert_eq!(state.visible_entries(101).count(), 1);
    p.tick(101).unwrap();
    assert_eq!(p.source().counters().expired, 1);
    let removal = poll(&mut p, 102).unwrap();
    assert!(matches!(&removal,Message::Prune(prune) if prune.keys==vec![key(0)]));
    let Received::Complete {
        state: filtered, ..
    } = r.receive(&removal, clock(102)).unwrap()
    else {
        panic!("Expected filtered peer state")
    };
    assert_eq!(filtered.entries().len(), 1);
    assert_eq!(filtered.entries()[0].marker.key.id, 1);
}

#[test]
fn locked_tf_failure_hides_only_its_key_and_recovery_reuses_current_source() {
    let mut p = MarkerProducer::new(owner(), LossSupport::Unknown).unwrap();
    let mut r = Receiver::new(owner()).unwrap();
    send(&mut p, &mut r, 0);
    let mut moving = marker(0, false);
    moving.frame_id = "moving".into();
    moving.frame_locked = true;
    p.input(1, batch(vec![add(moving), add(marker(1, false))]), 1)
        .unwrap();
    let request = p.requests(1).unwrap().pop().unwrap();
    p.complete(
        &request,
        LatestPose::Ready {
            resolved_stamp_ns: 10,
            components: [2., 0., 0., 0., 0., 0., 1.],
        },
        2,
    )
    .unwrap();
    let state = finish(&mut p, &mut r, 3);
    assert_eq!(state.visible_entries(3).count(), 2);
    let req = p.requests(marker::RETRY_NS + 1).unwrap().pop().unwrap();
    p.complete(&req, LatestPose::Missing, marker::RETRY_NS + 2)
        .unwrap();
    assert!(
        send(&mut p, &mut r, marker::RETRY_NS + 3).is_none(),
        "short TF gap keeps previous marker"
    );
    let deadline = marker::RETRY_NS + 2 + marker::LOOKUP_TIMEOUT_NS;
    let Received::Complete { state, .. } = send(&mut p, &mut r, deadline).unwrap() else {
        panic!("Missing TF did not prune")
    };
    assert_eq!(state.visible_entries(deadline).count(), 1);
    assert_eq!(state.entries()[0].marker.key.id, 1);
    let req = p
        .requests(deadline + marker::RETRY_NS)
        .unwrap()
        .pop()
        .unwrap();
    p.complete(
        &req,
        LatestPose::Ready {
            resolved_stamp_ns: 20,
            components: [3., 0., 0., 0., 0., 0., 1.],
        },
        deadline + marker::RETRY_NS + 1,
    )
    .unwrap();
    let recovered = finish(&mut p, &mut r, deadline + marker::RETRY_NS + 2);
    assert_eq!(
        recovered
            .visible_entries(deadline + marker::RETRY_NS + 2)
            .count(),
        2
    );
}

#[test]
fn replacing_marker_waits_for_its_tf_but_deleting_peer_is_immediate() {
    let mut p = MarkerProducer::new(owner(), LossSupport::Unknown).unwrap();
    let mut r = Receiver::new(owner()).unwrap();
    send(&mut p, &mut r, 0);
    p.input(
        1,
        batch(vec![add(marker(0, false)), add(marker(1, false))]),
        1,
    )
    .unwrap();
    let old = finish(&mut p, &mut r, 2);
    let mut moving = marker(0, false);
    moving.frame_id = "source".into();
    p.input(2, batch(vec![add(moving)]), 100_000_000).unwrap();
    let req = p.requests(100_000_000).unwrap().pop().unwrap();
    p.complete(&req, LatestPose::Extrapolated, 100_000_001)
        .unwrap();
    assert!(poll(&mut p, 100_000_002).is_none());
    assert_eq!(old.visible_entries(100_000_002).count(), 2);
    p.input(
        3,
        batch(vec![Operation::Delete { key: key(1) }]),
        100_000_003,
    )
    .unwrap();
    let removed = poll(&mut p, 100_000_004).unwrap();
    assert!(matches!(&removed, Message::Prune(prune) if prune.keys == [key(1)]));
    r.receive(&removed, clock(100_000_004)).unwrap();
    let req = p.requests(200_000_000).unwrap().pop().unwrap();
    p.complete(
        &req,
        LatestPose::Ready {
            resolved_stamp_ns: 10,
            components: [3., 0., 0., 0., 0., 0., 1.],
        },
        200_000_001,
    )
    .unwrap();
    let recovered = finish(&mut p, &mut r, 200_000_002);
    assert_eq!(recovered.visible_entries(200_000_002).count(), 1);
    assert_eq!(recovered.entries()[0].marker.frame_id, "source");
    p.local_clear().unwrap();
    assert!(
        matches!(poll(&mut p, 200_000_003), Some(Message::Clear(c)) if c.reason == ClearReason::LocalClear)
    );
}

#[test]
fn local_clear_source_fault_sequence_gap_stop_and_clock_failure_release_transfer_state() {
    for mode in 0..5 {
        let mut p = MarkerProducer::new(owner(), LossSupport::Unknown).unwrap();
        let mut r = Receiver::new(owner()).unwrap();
        send(&mut p, &mut r, 0);
        p.input(
            1,
            batch(vec![add(marker(0, true)), add(marker(1, true))]),
            1,
        )
        .unwrap();
        send(&mut p, &mut r, 2);
        assert!(p.in_flight_bytes() > 0);
        match mode {
            0 => p.local_clear().unwrap(),
            1 => p.source_fault().unwrap(),
            2 => {
                p.input(3, batch(vec![add(marker(2, false))]), 3).unwrap();
            }
            3 => p.stop(),
            _ => assert!(p.tick(0).is_err()),
        }
        assert_eq!(p.in_flight_bytes(), 0);
        assert_eq!(p.pending_prunes(), 0);
        if mode >= 3 {
            assert!(poll(&mut p, 4).is_none());
            assert!(!p.input(4, batch(vec![add(marker(9, false))]), 5).unwrap());
        } else {
            let clear = poll(&mut p, 4).unwrap();
            assert!(matches!(&clear,Message::Clear(c) if c.known_history_gap==(mode!=0)));
            assert!(matches!(
                r.receive(&clear, clock(4)).unwrap(),
                Received::Cleared(_)
            ));
            let current = finish(&mut p, &mut r, 100_000_000);
            assert_eq!(current.entries().len(), usize::from(mode == 2));
        }
    }
}
