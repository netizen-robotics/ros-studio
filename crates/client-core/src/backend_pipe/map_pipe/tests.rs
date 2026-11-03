use super::*;
use studio_protocol::{
    map_grid::{self, Grid, Metadata},
    map_snapshot::{ClearReason, Snapshot, SourceStatus, CONTRACT_VERSION},
    mapping, session, spatial,
    spatial_input::SpatialClockPolicy,
};
fn setup() -> (Shared, StartIntent, MapTarget) {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "synthetic".into()))
        .unwrap();
    let config = Configuration {
        topic: "/map".into(),
        message_type: mapping::MAP_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: true,
        output: Output::MapSnapshot,
        spatial: Some(spatial::Configuration {
            fixed_frame: "world".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
        robot: None,
        map_updates: None,
    };
    let intent = shared.registry.start_explicit(ViewId(1), config).unwrap();
    shared
        .queue
        .register(intent.generation, intent.subscription)
        .unwrap();
    let owner = Owner {
        generation: intent.generation,
        subscription: intent.subscription,
        topic: "/map".into(),
        fixed_frame: "world".into(),
    };
    let target = MapTarget {
        stream: MapStream::new(owner.clone(), false).unwrap(),
        owner,
    };
    shared.bind_map(target.clone()).unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            subscription: intent.subscription,
            request: intent.request,
        })
        .unwrap();
    (shared, intent, target)
}
fn snapshot(target: &MapTarget, size: u32, revision: u64) -> Snapshot {
    Snapshot::new(
        target.owner.clone(),
        revision,
        Grid {
            metadata: Metadata {
                frame_id: "world".into(),
                stamp_ns: 10,
                width: size,
                height: size,
                resolution: 0.05,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
            },
            cells: vec![42; size as usize * size as usize].into_boxed_slice(),
        },
        [0., 0., 0., 0., 0., 0., 1.],
        mapping::Counters::default(),
        map_grid::Counters::default(),
        SourceStatus::default(),
    )
    .unwrap()
}
fn clear(target: &MapTarget, revision: u64) -> Message {
    Message::Clear(Box::new(Clear {
        contract_version: CONTRACT_VERSION,
        owner: target.owner.clone(),
        revision,
        reason: ClearReason::QueueOverflow,
        metadata: None,
        source_status: SourceStatus::default(),
    }))
}
fn ingest(shared: &mut Shared, target: &MapTarget, sequence: u64, message: Message) {
    shared
        .ingest_for_test(Event::MapSnapshotObservation {
            generation: target.owner.generation,
            subscription: target.owner.subscription,
            sequence,
            sample: Box::new(message),
            backend_drops: 0,
        })
        .unwrap();
}
#[test]
fn maximum_map_is_atomic_and_clear_releases_partial_without_replaying_old_completion() {
    let (mut shared, _, target) = setup();
    let snapshot = snapshot(&target, 2048, 1);
    for index in 0..snapshot.chunk_count() {
        ingest(
            &mut shared,
            &target,
            index as u64 + 1,
            Message::Chunk(Box::new(snapshot.chunk(index).unwrap())),
        );
        if index + 1 < snapshot.chunk_count() {
            assert!(target.stream.take().is_none());
        }
    }
    assert!(
        matches!(target.stream.take(),Some(Ok(MapUpdate::Complete(c))) if c.grid.cell_bytes()==map_grid::MAX_CELLS && c.grid.cells.iter().all(|v|*v==42))
    );
    let next = snapshot_for_revision(&target, 2);
    ingest(
        &mut shared,
        &target,
        23,
        Message::Chunk(Box::new(next.chunk(0).unwrap())),
    );
    assert!(
        target
            .stream
            .0
            .receiver
            .lock()
            .unwrap()
            .in_flight_cell_bytes()
            > 0
    );
    ingest(&mut shared, &target, 24, clear(&target, 3));
    assert_eq!(
        target
            .stream
            .0
            .receiver
            .lock()
            .unwrap()
            .in_flight_cell_bytes(),
        0
    );
    assert!(matches!(target.stream.take(),Some(Ok(MapUpdate::Cleared(c))) if c.revision==3));
    ingest(
        &mut shared,
        &target,
        25,
        Message::Chunk(Box::new(snapshot.chunk(0).unwrap())),
    );
    assert!(target.stream.take().is_none());
}
fn snapshot_for_revision(target: &MapTarget, revision: u64) -> Snapshot {
    snapshot(target, 2048, revision)
}
#[test]
fn late_decoded_completion_cannot_overwrite_newer_clear_or_stopping_owner() {
    let (mut shared, intent, target) = setup();
    let sample = Message::Chunk(Box::new(snapshot(&target, 1, 1).chunk(0).unwrap()));
    let late = decode(&target, &sample, 0).unwrap();
    let newer = decode(&target, &clear(&target, 2), 0).unwrap();
    publish(&mut shared, &target, newer);
    publish(&mut shared, &target, late);
    assert!(matches!(target.stream.take(),Some(Ok(MapUpdate::Cleared(c))) if c.revision==2));
    let later = decode(
        &target,
        &Message::Chunk(Box::new(snapshot(&target, 1, 3).chunk(0).unwrap())),
        0,
    )
    .unwrap();
    shared.registry.stop_explicit(intent.subscription).unwrap();
    publish(&mut shared, &target, later);
    assert!(target.stream.take().is_none());
    assert_eq!(shared.registry.used_resources(), 3);
}
#[test]
fn wrong_identity_or_transport_loss_latches_scoped_fault_and_keeps_native_reservation() {
    for loss in [false, true] {
        let (mut shared, _, target) = setup();
        let mut message = Message::Chunk(Box::new(snapshot(&target, 1, 1).chunk(0).unwrap()));
        if !loss {
            if let Message::Chunk(c) = &mut message {
                c.header.owner.topic = "/wrong".into();
            }
        }
        let result = decode(&target, &message, u64::from(loss)).unwrap();
        publish(&mut shared, &target, result);
        assert!(matches!(target.stream.take(), Some(Err(_))));
        assert_eq!(shared.map_faults.len(), 1);
        assert_eq!(shared.registry.used_resources(), 3);
        assert!(prepare(
            &mut shared,
            target.owner.generation,
            target.owner.subscription,
            2
        )
        .is_none());
        assert!(target.stream.take().is_none());
    }
}
#[test]
fn stop_does_not_wait_for_decoder_lock_and_suppresses_inflight_publication() {
    let (_, _, target) = setup();
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let stream = target.stream.clone();
    let worker = std::thread::spawn(move || {
        let mut receiver = stream.0.receiver.lock().unwrap();
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        if stream.0.cancelled.load(Ordering::Acquire) {
            receiver.stop();
        }
    });
    locked_rx.recv().unwrap();
    let stream = target.stream.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let stop = std::thread::spawn(move || {
        stream.stop();
        done_tx.send(()).unwrap();
    });
    let stopped = done_rx.recv_timeout(std::time::Duration::from_secs(1));
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    stop.join().unwrap();
    assert!(stopped.is_ok());
    assert!(!target
        .stream
        .publish(Delivery::new(Ok(MapUpdate::Cleared(Arc::new(
            match clear(&target, 1) {
                Message::Clear(c) => *c,
                _ => unreachable!(),
            }
        ))))));
    assert!(target.stream.take().is_none());
}
#[test]
fn strict_capability_mismatch_is_rejected_before_map_binding() {
    let mut ready = session::Event::ready("jazzy".into(), "synthetic".into());
    if let Event::Ready { map_snapshot, .. } = &mut ready {
        map_snapshot.max_cells = 65536;
    }
    assert!(ready.validate().is_err());
    let mut value = serde_json::to_value(Event::ready("jazzy".into(), "synthetic".into())).unwrap();
    value.as_object_mut().unwrap().remove("map_snapshot");
    assert!(Event::decode(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[cfg(feature = "spatial")]
#[test]
fn prepared_delivery_shares_cells_and_either_consumer_releases_the_other_view() {
    let (mut shared, _, target) = setup();
    let message = Message::Chunk(Box::new(snapshot(&target, 1, 1).chunk(0).unwrap()));
    let delivery = decode(&target, &message, 0).unwrap();
    let complete = match &delivery.result {
        Ok(MapUpdate::Complete(c)) => Arc::downgrade(c),
        _ => panic!("complete"),
    };
    let raw = complete.upgrade().unwrap();
    assert_eq!(
        raw.grid.cells.as_ptr().cast::<u8>(),
        delivery.prepared.as_ref().unwrap().bytes().as_ptr()
    );
    drop(raw);
    publish(&mut shared, &target, delivery);
    let prepared = match target.stream.take_prepared().unwrap().unwrap() {
        PreparedMapUpdate::Complete(map) => map,
        _ => panic!("prepared"),
    };
    assert_eq!(prepared.dimensions(), [1, 1]);
    assert!(target.stream.take().is_none());
    assert_eq!(Arc::strong_count(&complete.upgrade().unwrap()), 2);
    drop(prepared);
    assert!(complete.upgrade().is_none());
    let message = Message::Chunk(Box::new(snapshot(&target, 1, 2).chunk(0).unwrap()));
    let delivery = decode(&target, &message, 0).unwrap();
    let prepared = Arc::downgrade(delivery.prepared.as_ref().unwrap());
    publish(&mut shared, &target, delivery);
    let raw = target.stream.take().unwrap().unwrap();
    assert!(prepared.upgrade().is_none());
    assert!(target.stream.take_prepared().is_none());
    drop(raw);
}

#[cfg(feature = "spatial")]
#[test]
fn clear_and_stop_drop_staged_prepared_maps_without_late_revival() {
    let (mut shared, _, target) = setup();
    let message = Message::Chunk(Box::new(snapshot(&target, 1, 1).chunk(0).unwrap()));
    let late = decode(&target, &message, 0).unwrap();
    let weak = Arc::downgrade(late.prepared.as_ref().unwrap());
    let clear = decode(&target, &clear(&target, 2), 0).unwrap();
    publish(&mut shared, &target, clear);
    publish(&mut shared, &target, late);
    assert!(weak.upgrade().is_none());
    assert!(
        matches!(target.stream.take_prepared(),Some(Ok(PreparedMapUpdate::Cleared(c))) if c.revision==2)
    );
    let message = Message::Chunk(Box::new(snapshot(&target, 1, 3).chunk(0).unwrap()));
    let late = decode(&target, &message, 0).unwrap();
    let weak = Arc::downgrade(late.prepared.as_ref().unwrap());
    target.stream.stop();
    publish(&mut shared, &target, late);
    assert!(weak.upgrade().is_none());
    assert!(target.stream.take_prepared().is_none());
    assert!(shared.map_faults.is_empty());
}

#[cfg(feature = "spatial")]
#[test]
fn maximum_reader_preparation_runs_without_the_shared_registry_lock() {
    let (shared, _, target) = setup();
    let shared = Arc::new(Mutex::new(shared));
    let map = snapshot(&target, 2048, 1);
    let held = shared.lock().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker_target = target.clone();
    let worker = std::thread::spawn(move || {
        let mut complete = None;
        for i in 0..map.chunk_count() {
            complete = decode(
                &worker_target,
                &Message::Chunk(Box::new(map.chunk(i).unwrap())),
                0,
            );
        }
        tx.send(complete.unwrap()).unwrap();
    });
    let result = rx.recv_timeout(std::time::Duration::from_secs(5));
    drop(held);
    worker.join().unwrap();
    let delivery = result.expect("reader preparation must not wait on Shared");
    assert_eq!(
        delivery.prepared.as_ref().unwrap().bytes().len(),
        map_grid::MAX_CELLS
    );
    publish(&mut shared.lock().unwrap(), &target, delivery);
    assert!(
        matches!(target.stream.take_prepared(),Some(Ok(PreparedMapUpdate::Complete(map))) if map.dimensions()==[2048,2048])
    );
}
