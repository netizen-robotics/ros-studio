use super::*;
use crate::spatial_session::{ClockPolicy, StreamKind, Transform};
use studio_protocol::{
    map_snapshot::{Received, Receiver},
    subscriptions::{Generation, SubscriptionId},
};
fn owner() -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        topic: "/map".into(),
        fixed_frame: "world".into(),
    }
}
fn grid(size: u32, frame: &str, stamp: i64) -> Grid {
    Grid {
        metadata: map_grid::Metadata {
            frame_id: frame.into(),
            stamp_ns: stamp,
            width: size,
            height: size,
            resolution: 0.05,
            origin: [0., 0., 0., 0., 0., 0., 1.],
            map_load_time_ns: 0,
        },
        cells: (0..size as usize * size as usize)
            .map(|i| i as u8 as i8)
            .collect(),
    }
}
fn active() -> SpatialSession {
    let mut tf = SpatialSession::new().unwrap();
    tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
    tf
}
fn insert(tf: &mut SpatialSession, stamp: i64, x: f64) {
    tf.insert(
        StreamKind::Dynamic,
        &Transform {
            parent: "world".into(),
            child: "source".into(),
            stamp_ns: stamp,
            components: [x, 0., 0., 0., 0., 0., 1.],
        },
    )
    .unwrap();
}
fn poll(producer: &mut MapProducer, tf: &mut SpatialSession, now: Instant) -> Message {
    producer
        .poll(tf, now, &spatial::Counters::default())
        .unwrap()
        .unwrap()
}
#[test]
fn maximum_atomic_delivery_releases_copy_and_never_queues_multiple_chunks() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [Some(true), None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    producer
        .full(token, 1, grid(2048, "world", 10), now)
        .unwrap();
    let mut receiver = Receiver::new(owner()).unwrap();
    for index in 0..map_snapshot::MAX_CHUNKS {
        let message = poll(&mut producer, &mut tf, now);
        assert!(message.encode().unwrap().len() < map_snapshot::MAX_SAMPLE_BYTES);
        match receiver.receive(&message).unwrap() {
            Received::Pending => assert!(index + 1 < map_snapshot::MAX_CHUNKS),
            Received::Complete(complete) => {
                assert_eq!(index + 1, map_snapshot::MAX_CHUNKS);
                assert!(complete
                    .grid
                    .cells
                    .iter()
                    .enumerate()
                    .all(|(i, v)| *v == i as u8 as i8));
                assert_eq!(complete.header.source_status.full_loss_events, Some(true));
            }
            other => panic!("Unexpected {other:?}"),
        }
        assert_eq!(
            producer.cell_bytes(),
            (
                map_grid::MAX_CELLS,
                if index + 1 == map_snapshot::MAX_CHUNKS {
                    0
                } else {
                    map_grid::MAX_CELLS
                }
            )
        );
    }
    assert!(producer
        .poll(&mut tf, now, &spatial::Counters::default())
        .unwrap()
        .is_none());
    producer.stop();
    assert_eq!(producer.cell_bytes(), (0, 0));
}
#[test]
fn replacement_cancels_partial_and_fault_clear_precedes_further_chunks() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    let mut receiver = Receiver::new(owner()).unwrap();
    producer
        .full(token, 1, grid(2048, "world", 10), now)
        .unwrap();
    let old = poll(&mut producer, &mut tf, now);
    assert_eq!(receiver.receive(&old).unwrap(), Received::Pending);
    let replacement = grid(2048, "world", 20);
    assert_eq!(
        producer.cell_bytes().0 + producer.cell_bytes().1 + replacement.cell_bytes(),
        3 * map_grid::MAX_CELLS
    );
    producer.full(token, 2, replacement, now).unwrap();
    assert_eq!(producer.cell_bytes(), (map_grid::MAX_CELLS, 0));
    let new = poll(&mut producer, &mut tf, now);
    assert_eq!(receiver.receive(&new).unwrap(), Received::Pending);
    assert_eq!(receiver.receive(&old).unwrap(), Received::Stale);
    producer.fault(token, 3, Invalidation::QueueOverflow, now);
    let clear = poll(&mut producer, &mut tf, now);
    assert!(
        matches!(&clear,Message::Clear(c) if c.reason==ClearReason::QueueOverflow && c.source_status.recovery_floor_ns==Some(20))
    );
    assert!(matches!(
        receiver.receive(&clear).unwrap(),
        Received::Cleared(_)
    ));
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
    assert_eq!(producer.cell_bytes(), (0, 0));
    assert!(producer
        .poll(&mut tf, now, &spatial::Counters::default())
        .unwrap()
        .is_none());
    assert_eq!(receiver.receive(&new).unwrap(), Received::Stale);
}
#[test]
fn missing_timeout_recovers_at_acquisition_without_new_source_or_latest_fallback() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    producer
        .full(token, 1, grid(2, "source", 2_000_000_000), now)
        .unwrap();
    assert!(
        matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.reason==ClearReason::MissingTransform && c.metadata.as_ref().unwrap().stamp_ns==2_000_000_000)
    );
    let later = now + Duration::from_millis(1000);
    assert!(
        matches!(poll(&mut producer,&mut tf,later),Message::Clear(c) if c.reason==ClearReason::LookupTimeout)
    );
    assert_eq!(producer.cell_bytes(), (4, 0));
    insert(&mut tf, 1_000_000_000, 1.);
    insert(&mut tf, 3_000_000_000, 3.);
    assert!(
        matches!(poll(&mut producer,&mut tf,later+Duration::from_millis(50)),Message::Chunk(c) if c.header.transform[0]==2. && c.header.metadata.stamp_ns==2_000_000_000 && c.header.source_counters.lookup_timeouts==1)
    );
    assert_eq!(producer.source().assembly().counters().full_maps, 1);
}

#[test]
fn replacement_tf_lag_keeps_completed_map_but_input_cannot_extend_deadline() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    insert(&mut tf, 1_000_000_000, 1.);
    insert(&mut tf, 3_000_000_000, 3.);
    producer
        .full(token, 1, grid(2, "source", 2_000_000_000), now)
        .unwrap();
    assert!(
        matches!(poll(&mut producer, &mut tf, now), Message::Chunk(c) if c.header.transform[0] == 2.)
    );

    let replacement_at = now + Duration::from_millis(100);
    producer
        .full(token, 2, grid(2, "source", 4_000_000_000), replacement_at)
        .unwrap();
    assert!(
        producer
            .poll(&mut tf, replacement_at, &spatial::Counters::default())
            .unwrap()
            .is_none(),
        "transient future TF must not clear the completed map"
    );
    insert(&mut tf, 5_000_000_000, 5.);
    let delivered_at = now + Duration::from_millis(150);
    assert!(
        matches!(poll(&mut producer, &mut tf, delivered_at), Message::Chunk(c) if c.header.metadata.stamp_ns == 4_000_000_000 && c.header.transform[0] == 4.)
    );

    // Even a newer full map arriving immediately before the deadline cannot
    // retain old geometry forever while no matching TF becomes available.
    for (sequence, millis) in [(3, 200), (4, 700), (5, 1100)] {
        let at = now + Duration::from_millis(millis);
        producer
            .full(token, sequence, grid(2, "source", 6_000_000_000), at)
            .unwrap();
        assert!(producer
            .poll(&mut tf, at, &spatial::Counters::default())
            .unwrap()
            .is_none());
    }
    assert!(
        matches!(poll(&mut producer, &mut tf, delivered_at + Duration::from_millis(spatial::PENDING_TIMEOUT_MS)), Message::Clear(c) if c.reason == ClearReason::LookupTimeout)
    );
    insert(&mut tf, 7_000_000_000, 7.);
    assert!(
        matches!(poll(&mut producer, &mut tf, now + Duration::from_millis(1200)), Message::Chunk(c) if c.header.transform[0] == 6.)
    );
    producer
        .full(
            token,
            6,
            grid(2, "source", 8_000_000_000),
            now + Duration::from_millis(1250),
        )
        .unwrap();
    producer.fault(
        token,
        7,
        Invalidation::QueueOverflow,
        now + Duration::from_millis(1251),
    );
    assert!(
        matches!(poll(&mut producer, &mut tf, now + Duration::from_millis(1251)), Message::Clear(c) if c.reason == ClearReason::QueueOverflow)
    );
    producer.stop();
    assert!(producer
        .poll(
            &mut tf,
            now + Duration::from_millis(1300),
            &spatial::Counters::default()
        )
        .unwrap()
        .is_none());
}
#[test]
fn extrapolation_hides_until_historical_transform_exists_and_invalid_world_bounds_preserve_source()
{
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    insert(&mut tf, 1_000_000_000, 1.);
    insert(&mut tf, 3_000_000_000, 3.);
    producer
        .full(token, 1, grid(2, "source", 4_000_000_000), now)
        .unwrap();
    assert!(
        matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.reason==ClearReason::Extrapolated)
    );
    insert(&mut tf, 5_000_000_000, 5.);
    assert!(
        matches!(poll(&mut producer,&mut tf,now+Duration::from_millis(50)),Message::Chunk(c) if c.header.transform[0]==4.)
    );
    let mut big = grid(1, "source", 5_000_000_000);
    big.metadata.origin[0] = mapping::MAX_COORDINATE - 1.;
    producer.full(token, 2, big, now).unwrap();
    assert!(
        matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.reason==ClearReason::InvalidTransform && c.metadata.is_some())
    );
    assert_eq!(producer.cell_bytes(), (1, 0));
}
#[test]
fn stale_owner_duplicate_input_and_stop_do_not_restart_or_replay_delivery() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    producer.full(token, 1, grid(2, "world", 10), now).unwrap();
    poll(&mut producer, &mut tf, now);
    assert_eq!(
        producer.full(token, 2, grid(2, "world", 10), now),
        Ok(Apply::Unchanged)
    );
    assert!(producer
        .poll(&mut tf, now, &spatial::Counters::default())
        .unwrap()
        .is_none());
    let mut foreign = MapIngress::default();
    let other = foreign.start(false).unwrap();
    assert_eq!(
        producer.full(other, 100, grid(2, "world", 100), now),
        Ok(Apply::Stale)
    );
    producer.fault(other, 101, Invalidation::Clear, now);
    assert_eq!(producer.cell_bytes(), (4, 0));
    producer.stop();
    producer
        .full(token, 1000, grid(2, "world", 1000), now)
        .unwrap();
    assert_eq!(producer.cell_bytes(), (0, 0));
    assert!(producer
        .poll(&mut tf, now, &spatial::Counters::default())
        .unwrap()
        .is_none());
}
#[test]
fn ordered_patches_carry_unknown_diagnostics_and_loss_clears_without_replay() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), true, [Some(true), Some(true)], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    producer.full(token, 1, grid(2, "world", 10), now).unwrap();
    for seq in 2..4 {
        producer
            .patch(
                token,
                seq,
                Patch {
                    frame_id: "world".into(),
                    stamp_ns: 10 + seq as i64,
                    x: (seq - 2) as i32,
                    y: 0,
                    width: 1,
                    height: 2,
                    cells: vec![seq as i8; 2].into_boxed_slice(),
                },
                Publication {
                    publisher_id: "aa".repeat(16),
                    sequence: None,
                },
                now,
            )
            .unwrap();
    }
    let mut receiver = Receiver::new(owner()).unwrap();
    let result = receiver
        .receive(&poll(&mut producer, &mut tf, now))
        .unwrap();
    assert!(
        matches!(result,Received::Complete(c) if *c.grid.cells==[2,3,2,3] && c.header.source_status.continuity==map_grid::Continuity::Unknown)
    );
    producer.loss(token, 4, Reader::Updates, Some(3), now);
    assert!(
        matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.source_status.ingress_counters.reported_lost==3 && c.source_status.recovery_floor_ns==Some(13))
    );
    assert_eq!(producer.cell_bytes(), (0, 0));
}

#[test]
fn data_fault_during_tf_wait_cannot_resurrect_on_late_transform() {
    let now = Instant::now();
    let mut producer = MapProducer::new(owner(), false, [None, None], now).unwrap();
    let token = producer.token().unwrap();
    let mut tf = active();
    producer
        .full(token, 1, grid(2, "source", 2_000_000_000), now)
        .unwrap();
    assert!(matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.metadata.is_some()));
    producer.fault(token, 2, Invalidation::SourceFailure, now);
    assert!(
        matches!(poll(&mut producer,&mut tf,now),Message::Clear(c) if c.reason==ClearReason::SourceFailure && c.metadata.is_none())
    );
    insert(&mut tf, 1_000_000_000, 1.);
    insert(&mut tf, 3_000_000_000, 3.);
    assert!(producer
        .poll(
            &mut tf,
            now + Duration::from_secs(2),
            &spatial::Counters::default()
        )
        .unwrap()
        .is_none());
    assert_eq!(producer.cell_bytes(), (0, 0));
}
