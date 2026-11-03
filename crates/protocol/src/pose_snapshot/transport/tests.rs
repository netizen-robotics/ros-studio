use super::*;
use crate::latest_pose::LatestPose;
use serde_json::json;
fn owner() -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        kind: Kind::Odometry,
        topic: "/odom".into(),
        fixed_frame: "world".into(),
    }
}
fn configuration() -> Configuration {
    Configuration {
        kind: Kind::Odometry,
        history: Some(HistoryConfiguration::default()),
    }
}
fn sample(revision: u64, epoch: u64) -> Snapshot {
    Snapshot {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision,
        continuity_epoch: epoch,
        source: Some(Source {
            frame_id: "odom".into(),
            child_frame_id: Some("base".into()),
        }),
        history: configuration().settings().unwrap(),
        latest: Some(Entry {
            sequence: revision,
            acquisition_stamp_ns: 10,
            components: [0., 0., 0., 0., 0., 0., 1.],
            covariance: Covariance::new([0.; 36]).unwrap(),
            transform: LatestPose::Missing,
        }),
        trail: vec![],
        last_reset: None,
        known_history_gap: false,
        counters: Counters::default(),
    }
}
fn barrier(revision: u64, epoch: u64) -> Invalidation {
    Invalidation {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision,
        continuity_epoch: epoch,
        reason: ResetReason::LocalClear,
    }
}
fn receiver() -> Receiver {
    Receiver::new(owner(), configuration()).unwrap()
}

#[test]
fn ready_replacement_keeps_exact_cached_geometry_until_handoff_but_waiting_and_clear_revoke() {
    let mut r = receiver();
    let make_ready = |revision| {
        let mut s = sample(revision, 1);
        s.latest.as_mut().unwrap().transform = LatestPose::Ready {
            resolved_stamp_ns: 10,
            components: [0., 0., 0., 0., 0., 0., 1.],
        };
        s
    };
    r.apply(make_ready(1)).unwrap();
    let old = r.snapshot().unwrap().clone();
    let permission = r.permission().unwrap();
    r.apply(make_ready(2)).unwrap();
    assert!(
        permission.authorizes(&old),
        "worker preparation must not blank the current frame"
    );
    assert!(
        !permission.authorizes(r.snapshot().unwrap()),
        "permission must stay snapshot-specific"
    );
    r.apply(sample(3, 1)).unwrap();
    assert!(
        !permission.is_valid(),
        "genuine unavailable transform revokes old geometry"
    );
    r.apply(make_ready(4)).unwrap();
    let permission = r.permission().unwrap();
    r.invalidate(&barrier(5, 2)).unwrap();
    assert!(!permission.is_valid());
    assert!(r.snapshot().is_none());
}
#[test]
fn exact_capabilities_and_integer_history_are_strict_and_bounded() {
    let c = Capabilities::default();
    c.validate().unwrap();
    let mut changed = c.clone();
    changed.max_sample_bytes += 1;
    assert!(changed.validate().is_err());
    assert_eq!(
        serde_json::from_value::<Capabilities>(serde_json::to_value(&c).unwrap()).unwrap(),
        c
    );
    let mut extra = serde_json::to_value(c).unwrap();
    extra["unknown"] = json!(1);
    assert!(serde_json::from_value::<Capabilities>(extra).is_err());
    let mut h = HistoryConfiguration {
        keep: 256,
        position_tolerance_um: 1_000_000_000,
        angle_tolerance_urad: 3_141_592,
    };
    h.settings().unwrap();
    h.angle_tolerance_urad += 1;
    assert!(h.settings().is_err());
    h = HistoryConfiguration::default();
    h.position_tolerance_um = 1_000_000_001;
    assert!(h.settings().is_err());
    h = HistoryConfiguration::default();
    h.keep = 0;
    assert!(h.settings().is_err());
    h.keep = 257;
    assert!(h.settings().is_err());
    assert_eq!(
        HistoryConfiguration::default().settings().unwrap(),
        HistorySettings::default()
    );
    assert!(Configuration {
        kind: Kind::PoseWithCovariance,
        history: configuration().history
    }
    .settings()
    .is_err());
    assert!(Configuration {
        kind: Kind::Odometry,
        history: None
    }
    .settings()
    .is_err());
    assert!(Configuration {
        kind: Kind::PoseWithCovariance,
        history: None
    }
    .settings()
    .unwrap()
    .is_none());
    assert!(serde_json::from_value::<HistoryConfiguration>(
        json!({"keep":1,"position_tolerance_um":0.5,"angle_tolerance_urad":0})
    )
    .is_err());
}
#[test]
fn invalidation_roundtrip_rejects_unknown_zero_and_oversized_controls() {
    let b = barrier(2, 2);
    assert_eq!(Invalidation::decode(&b.encode().unwrap()).unwrap(), b);
    let mut bad = b.clone();
    bad.revision = 0;
    assert!(bad.encode().is_err());
    let mut extra = serde_json::to_value(b).unwrap();
    extra["unknown"] = json!(true);
    assert!(Invalidation::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
    assert!(Invalidation::decode(&vec![b' '; MAX_CONTROL_BYTES + 1]).is_err());
}
#[test]
fn continuity_control_retires_old_geometry_and_late_full_snapshots() {
    let mut r = receiver();
    assert_eq!(r.apply(sample(1, 1)).unwrap(), Applied::Updated);
    let retained = r.snapshot().unwrap().clone();
    assert_eq!(Arc::strong_count(&retained), 2);
    assert_eq!(r.invalidate(&barrier(3, 2)).unwrap(), Applied::Updated);
    assert!(r.snapshot().is_none());
    assert_eq!(Arc::strong_count(&retained), 1);
    assert_eq!(r.apply(sample(2, 1)).unwrap(), Applied::Stale);
    assert_eq!(r.apply(sample(99, 1)).unwrap(), Applied::Stale);
    assert_eq!(r.invalidate(&barrier(3, 2)).unwrap(), Applied::Stale);
    assert_eq!(r.apply(sample(4, 2)).unwrap(), Applied::Updated);
    assert_eq!(r.apply(sample(3, 3)).unwrap(), Applied::Stale);
    assert_eq!(r.snapshot().unwrap().revision, 4);
}
#[test]
fn local_clear_hides_immediately_and_blocks_queued_data_until_correlated_ack() {
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    r.begin_clear(RequestId(7)).unwrap();
    assert!(r.snapshot().is_none());
    assert!(r.begin_clear(RequestId(8)).is_err());
    assert_eq!(r.apply(sample(2, 1)).unwrap(), Applied::WaitingForClear);
    assert!(!r.acknowledge_clear(RequestId(6), &barrier(3, 2)).unwrap());
    assert_eq!(r.pending_clear(), Some(RequestId(7)));
    r.invalidate(&barrier(3, 2)).unwrap();
    assert_eq!(r.apply(sample(4, 2)).unwrap(), Applied::WaitingForClear);
    assert!(r.acknowledge_clear(RequestId(7), &barrier(3, 2)).unwrap());
    assert!(!r.acknowledge_clear(RequestId(7), &barrier(3, 2)).unwrap());
    assert_eq!(r.apply(sample(2, 1)).unwrap(), Applied::Stale);
    r.apply(sample(4, 2)).unwrap();
    assert!(r.begin_clear(RequestId(7)).is_err());
    assert!(r.snapshot().is_some());
    r.begin_clear(RequestId(8)).unwrap();
    assert!(!r.acknowledge_clear(RequestId(8), &barrier(4, 2)).unwrap());
    assert!(r.acknowledge_clear(RequestId(8), &barrier(5, 3)).unwrap());
}
#[test]
fn wrong_owner_cannot_release_clear_or_affect_peers_and_current_metadata_faults_scope() {
    let mut a = receiver();
    let mut peer = receiver();
    peer.apply(sample(1, 1)).unwrap();
    a.apply(sample(1, 1)).unwrap();
    a.begin_clear(RequestId(2)).unwrap();
    let mut old = barrier(3, 2);
    old.owner.generation = Generation(9);
    assert!(!a.acknowledge_clear(RequestId(2), &old).unwrap());
    assert_eq!(a.pending_clear(), Some(RequestId(2)));
    let mut unrelated = sample(50, 9);
    unrelated.owner.subscription = SubscriptionId(8);
    assert_eq!(a.apply(unrelated).unwrap(), Applied::Stale);
    let mut bad = barrier(3, 2);
    bad.owner.fixed_frame = "other".into();
    assert!(a.acknowledge_clear(RequestId(2), &bad).is_err());
    assert!(a.fault().is_some());
    assert!(a.snapshot().is_none());
    assert!(peer.snapshot().is_some());
    assert_eq!(a.apply(sample(100, 100)).unwrap(), Applied::Stale);
}
#[test]
fn malformed_current_history_and_source_fault_require_fresh_owner_activation() {
    for scenario in 0..4 {
        let mut r = receiver();
        r.apply(sample(1, 1)).unwrap();
        let mut next = sample(2, 1);
        match scenario {
            0 => next.history.as_mut().unwrap().keep = 2,
            1 => next.owner.topic = "/different".into(),
            2 => next.latest.as_mut().unwrap().components[6] = 0.,
            _ => next.last_reset = Some(ResetReason::SourceFault),
        }
        assert!(r.apply(next).is_err());
        assert!(r.snapshot().is_none());
        assert!(r.fault().is_some());
        assert_eq!(r.apply(sample(3, 2)).unwrap(), Applied::Stale);
    }
    let mut r = receiver();
    let mut b = barrier(2, 2);
    b.reason = ResetReason::SourceFault;
    assert!(r.invalidate(&b).is_err());
    assert!(r.begin_clear(RequestId(1)).is_err());
}
#[test]
fn stop_retires_pending_ack_and_releases_only_owned_snapshot_reference() {
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    let held = r.snapshot().unwrap().clone();
    r.begin_clear(RequestId(2)).unwrap();
    r.stop();
    assert_eq!(Arc::strong_count(&held), 1);
    assert_eq!(r.pending_clear(), None);
    assert!(!r.acknowledge_clear(RequestId(2), &barrier(2, 2)).unwrap());
    assert_eq!(r.apply(sample(9, 9)).unwrap(), Applied::Stale);
    assert!(r.snapshot().is_none());
}
#[test]
fn clear_ack_cannot_relabel_a_delivered_gap_control_or_reuse_its_prior_epoch() {
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    r.begin_clear(RequestId(2)).unwrap();
    assert!(!r.acknowledge_clear(RequestId(2), &barrier(1, 1)).unwrap());
    let mut gap = barrier(3, 2);
    gap.reason = ResetReason::SourceGap;
    r.invalidate(&gap).unwrap();
    assert!(r.acknowledge_clear(RequestId(2), &barrier(3, 2)).is_err());
    assert!(r.fault().is_some());
    assert!(r.snapshot().is_none());
}
#[test]
fn cached_permissions_revoke_on_replacement_clear_control_fault_stop_and_drop() {
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    let first = r.permission().unwrap();
    assert!(first.is_valid());
    r.apply(sample(2, 1)).unwrap();
    assert!(!first.is_valid());
    let second = r.permission().unwrap();
    r.begin_clear(RequestId(1)).unwrap();
    assert!(!second.is_valid());
    r.acknowledge_clear(RequestId(1), &barrier(3, 2)).unwrap();
    r.apply(sample(4, 2)).unwrap();
    let third = r.permission().unwrap();
    r.invalidate(&barrier(5, 3)).unwrap();
    assert!(!third.is_valid());
    r.apply(sample(6, 3)).unwrap();
    let fourth = r.permission().unwrap();
    let mut bad = sample(7, 3);
    bad.history = None;
    assert!(r.apply(bad).is_err());
    assert!(!fourth.is_valid());
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    let stopped = r.permission().unwrap();
    r.stop();
    assert!(!stopped.is_valid());
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    let dropped = r.permission().unwrap();
    drop(r);
    assert!(!dropped.is_valid());
    let mut r = receiver();
    r.serial = u64::MAX;
    assert!(r.apply(sample(1, 1)).is_err());
    assert!(r.permission().is_none());
}
#[test]
fn reset_control_allows_its_same_revision_full_state_once_without_allowing_replay() {
    let mut r = receiver();
    r.apply(sample(1, 1)).unwrap();
    let mut reset = barrier(3, 2);
    reset.reason = ResetReason::TimePolicyChanged;
    r.invalidate(&reset).unwrap();
    let mut full = sample(3, 2);
    full.last_reset = Some(ResetReason::TimePolicyChanged);
    assert_eq!(r.apply(full.clone()).unwrap(), Applied::Updated);
    assert_eq!(r.apply(full).unwrap(), Applied::Stale);
    assert!(r.permission().unwrap().is_valid());
}
