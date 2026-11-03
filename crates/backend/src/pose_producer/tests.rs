use super::*;
use crate::spatial_session::{ClockPolicy, StreamKind, Transform};
use studio_protocol::{
    pose_snapshot::{
        store::{RETRY_NS, TIMEOUT_NS},
        Kind, Snapshot, Source,
    },
    subscriptions::{Generation, SubscriptionId},
    uncertainty::Covariance,
};
fn producer(kind: Kind) -> PoseProducer {
    PoseProducer::new(
        Owner {
            generation: Generation(1),
            subscription: SubscriptionId(2),
            kind,
            topic: "/pose".into(),
            fixed_frame: "world".into(),
        },
        if kind == Kind::Odometry {
            Some(HistorySettings::default())
        } else {
            None
        },
    )
    .unwrap()
}
fn decoded(stamp: i64, x: f64) -> Decoded {
    Decoded {
        kind: Kind::Odometry,
        topic: "/pose".into(),
        source: Source {
            frame_id: "odom".into(),
            child_frame_id: Some("base".into()),
        },
        acquisition_stamp_ns: stamp,
        components: [x, 0., 0., 0., 0., 0., 1.],
        covariance: Covariance::new([0.; 36]).unwrap(),
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
            child: "odom".into(),
            stamp_ns: stamp,
            components: [x, 0., 0., 0., 0., 0., 1.],
        },
    )
    .unwrap();
}
fn emitted(p: &mut PoseProducer, tf: &mut SpatialSession, now: u64) -> Snapshot {
    Snapshot::decode(&p.poll(tf, now, true).unwrap().unwrap()).unwrap()
}
#[test]
fn native_cache_missing_recovers_at_exact_time_while_zero_uses_latest() {
    let mut tf = active();
    let mut exact = producer(Kind::Odometry);
    exact.input(1, decoded(2_000_000_000, 1.), 0).unwrap();
    assert_eq!(
        emitted(&mut exact, &mut tf, 0).latest.unwrap().transform,
        LatestPose::Missing
    );
    insert(&mut tf, 1_000_000_000, 1.);
    insert(&mut tf, 3_000_000_000, 3.);
    let ready = emitted(&mut exact, &mut tf, RETRY_NS);
    assert!(
        matches!(ready.latest.unwrap().transform,LatestPose::Ready {resolved_stamp_ns:2_000_000_000,components} if components[0]==2.)
    );
    assert_eq!(ready.trail.len(), 1);
    let mut latest = producer(Kind::Odometry);
    latest.input(1, decoded(0, 1.), 0).unwrap();
    assert!(
        matches!(emitted(&mut latest,&mut tf,0).latest.unwrap().transform,LatestPose::Ready {resolved_stamp_ns:3_000_000_000,components} if components[0]==3.)
    );
    insert(&mut tf, 4_000_000_000, 4.);
    assert!(exact.poll(&mut tf, 2 * RETRY_NS, true).unwrap().is_none());
    latest.input(2, decoded(0, 1.), RETRY_NS).unwrap();
    let newer = emitted(&mut latest, &mut tf, RETRY_NS);
    assert!(
        matches!(newer.latest.unwrap().transform,LatestPose::Ready {resolved_stamp_ns:4_000_000_000,components} if components[0]==4.)
    );
    assert!(
        matches!(newer.trail[0].transform,LatestPose::Ready {components,..} if components[0]==3.)
    );
}
#[test]
fn output_denial_does_not_block_input_processing_or_timeout_progress() {
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    p.input(1, decoded(10, 1.), 0).unwrap();
    assert!(p.poll(&mut tf, 0, false).unwrap().is_none());
    assert!(p.poll(&mut tf, TIMEOUT_NS, false).unwrap().is_none());
    assert_eq!(p.source().snapshot().unwrap().counters.lookup_timeouts, 1);
    let result = emitted(&mut p, &mut tf, TIMEOUT_NS);
    assert_eq!(result.latest.unwrap().transform, LatestPose::LookupTimeout);
    assert!(result.known_history_gap);
    assert!(p
        .poll(&mut tf, TIMEOUT_NS + RETRY_NS, true)
        .unwrap()
        .is_none());
}

#[test]
fn unchanged_missing_tf_retries_do_not_reemit_full_history() {
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    p.input(1, decoded(10, 1.), 0).unwrap();
    let first = emitted(&mut p, &mut tf, 0);
    assert_eq!(first.latest.unwrap().transform, LatestPose::Missing);
    assert!(p.poll(&mut tf, RETRY_NS, true).unwrap().is_none());
    assert!(p.poll(&mut tf, 2 * RETRY_NS, true).unwrap().is_none());
    assert_eq!(
        emitted(&mut p, &mut tf, TIMEOUT_NS)
            .counters
            .lookup_timeouts,
        1
    );
}

#[test]
fn source_gap_does_not_bypass_full_snapshot_rate_limit() {
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    let mut input = decoded(10, 1.);
    input.source.frame_id = "world".into();
    p.input(1, input.clone(), 0).unwrap();
    emitted(&mut p, &mut tf, 0);
    p.source_gap().unwrap();
    input.acquisition_stamp_ns = 20;
    p.input(2, input, 1).unwrap();
    assert!(p.poll(&mut tf, 1, true).unwrap().is_none());
    let next = emitted(&mut p, &mut tf, RETRY_NS);
    assert!(next.known_history_gap);
    assert_eq!(next.latest.unwrap().sequence, 2);
}

#[test]
fn clear_barrier_stays_empty_even_if_new_input_arrives_before_output_permit() {
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    let mut input = decoded(10, 1.);
    input.source.frame_id = "world".into();
    p.input(1, input.clone(), 0).unwrap();
    emitted(&mut p, &mut tf, 0);
    p.clear().unwrap();
    input.acquisition_stamp_ns = 20;
    p.input(2, input, 1).unwrap();
    assert!(p.poll(&mut tf, 1, false).unwrap().is_none());
    let clear = emitted(&mut p, &mut tf, 1);
    assert!(clear.latest.is_none());
    assert!(clear.trail.is_empty());
    assert!(p.poll(&mut tf, 2, true).unwrap().is_none());
    assert_eq!(
        emitted(&mut p, &mut tf, RETRY_NS + 1)
            .latest
            .unwrap()
            .sequence,
        2
    );
}
#[test]
fn pending_clear_is_retired_on_stop_or_source_fault() {
    for fault in [false, true] {
        let mut tf = active();
        let mut p = producer(Kind::Odometry);
        p.clear().unwrap();
        assert!(p.pending_clear.is_some());
        if fault {
            p.source_fault();
        } else {
            p.stop();
        }
        assert!(p.pending_clear.is_none());
        assert!(p.poll(&mut tf, 0, true).unwrap().is_none());
    }
}
#[test]
fn full_snapshot_coalescing_throttle_and_clear_preserve_current_state() {
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    let mut input = decoded(10, 1.);
    input.source.frame_id = "world".into();
    p.input(1, input.clone(), 0).unwrap();
    let first = emitted(&mut p, &mut tf, 0);
    assert_eq!(first.trail.len(), 1);
    input.acquisition_stamp_ns = 20;
    input.components[0] = 2.;
    p.input(2, input.clone(), 1).unwrap();
    assert!(p.poll(&mut tf, 1, true).unwrap().is_none());
    input.acquisition_stamp_ns = 30;
    input.components[0] = 3.;
    p.input(3, input.clone(), 2).unwrap();
    assert!(p.poll(&mut tf, 2, false).unwrap().is_none());
    let latest = emitted(&mut p, &mut tf, RETRY_NS);
    assert_eq!(latest.trail.len(), 3);
    assert_eq!(latest.latest.unwrap().sequence, 3);
    p.clear().unwrap();
    let cleared = emitted(&mut p, &mut tf, RETRY_NS);
    assert!(cleared.latest.is_none());
    assert!(cleared.trail.is_empty());
    assert!(cleared.continuity_epoch > first.continuity_epoch);
    assert_eq!(p.input(4, input, RETRY_NS).unwrap(), Input::Duplicate);
    assert!(p.source().snapshot().unwrap().latest.is_none());
    p.stop();
    assert!(p.poll(&mut tf, RETRY_NS, true).unwrap().is_none());
    assert!(p.source().snapshot().is_none());
}
#[test]
fn identity_cannot_bypass_native_verified_system_context_and_reset_retires_source() {
    let mut tf = SpatialSession::new().unwrap();
    let mut p = producer(Kind::Odometry);
    let mut input = decoded(10, 1.);
    input.source.frame_id = "world".into();
    p.input(1, input.clone(), 0).unwrap();
    assert!(p.poll(&mut tf, 0, true).is_err());
    assert!(p.source().token().is_none());
    assert!(p.source().snapshot().unwrap().latest.is_none());
    let mut tf = active();
    let mut p = producer(Kind::Odometry);
    p.input(1, input, 0).unwrap();
    assert_eq!(emitted(&mut p, &mut tf, 0).trail.len(), 1);
    assert_eq!(tf.frame_count(), 0);
    tf.reset().unwrap();
    assert!(p.poll(&mut tf, RETRY_NS, true).is_err());
    assert!(p.source().token().is_none());
    assert!(p.source().snapshot().unwrap().trail.is_empty());
}
