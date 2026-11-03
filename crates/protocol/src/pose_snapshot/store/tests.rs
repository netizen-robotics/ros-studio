use super::*;
fn owner(kind: Kind) -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        kind,
        topic: "/pose".into(),
        fixed_frame: "world".into(),
    }
}
fn decoded(kind: Kind, stamp: i64, x: f64) -> Decoded {
    Decoded {
        kind,
        topic: "/pose".into(),
        source: Source {
            frame_id: "odom".into(),
            child_frame_id: if kind == Kind::Odometry {
                Some("base".into())
            } else {
                None
            },
        },
        acquisition_stamp_ns: stamp,
        components: [x, 0., 0., 0., 0., 0., 1.],
        covariance: crate::uncertainty::Covariance::new([0.; 36]).unwrap(),
    }
}
fn start(history: HistorySettings) -> (Store, Token) {
    let mut s = Store::default();
    let t = s.start(owner(Kind::Odometry), Some(history)).unwrap();
    (s, t)
}
fn ready(r: &TransformRequest, x: f64) -> LatestPose {
    LatestPose::Ready {
        resolved_stamp_ns: match r.time() {
            LookupTime::At(n) => n,
            LookupTime::Latest => 777,
        },
        components: [x, 0., 0., 0., 0., 0., 1.],
    }
}
fn snap(s: &Store) -> Snapshot {
    let v = s.snapshot().unwrap();
    v.validate().unwrap();
    v
}

#[test]
fn presentation_keeps_exact_resolved_head_without_changing_source_snapshot_or_history() {
    for kind in [Kind::Odometry, Kind::PoseWithCovariance] {
        let mut s = Store::default();
        let t = s
            .start(
                owner(kind),
                (kind == Kind::Odometry).then_some(HistorySettings {
                    position_tolerance_m: 10.,
                    ..Default::default()
                }),
            )
            .unwrap();
        s.input(t, 1, decoded(kind, 10, 0.), 0).unwrap();
        capture(&mut s, t, 0);
        s.input(t, 2, decoded(kind, 20, 0.2), 1).unwrap();
        assert_eq!(
            snap(&s).latest.unwrap().sequence,
            2,
            "source contract unchanged"
        );
        let visible = s.presentation_snapshot(2).unwrap();
        visible.validate().unwrap();
        assert_eq!(visible.latest.unwrap().sequence, 1);
        capture(&mut s, t, 3);
        s.input(t, 3, decoded(kind, 30, 0.4), 4).unwrap();
        let visible = s.presentation_snapshot(5).unwrap();
        visible.validate().unwrap();
        assert_eq!(
            visible.latest.unwrap().sequence,
            2,
            "resolved head must not depend on decimated history"
        );
        if kind == Kind::Odometry {
            assert_eq!(visible.trail.last().unwrap().sequence, 1);
        }
        assert_eq!(
            s.presentation_snapshot(TIMEOUT_NS + 3)
                .unwrap()
                .latest
                .unwrap()
                .sequence,
            3,
            "continuous pending input cannot retain old head forever"
        );
        s.source_gap(t).unwrap();
        assert!(s.presentation_snapshot(6).unwrap().latest.is_none());
        s.stop();
        assert!(s.presentation_snapshot(7).is_none());
    }
}
fn capture(s: &mut Store, t: Token, now: u64) {
    for r in s.requests(t, now).unwrap() {
        s.complete(&r, ready(&r, 0.), now).unwrap();
    }
    snap(s);
}
#[test]
fn out_of_order_results_update_head_immediately_but_commit_anchors_in_order() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    s.input(t, 2, decoded(Kind::Odometry, 20, 2.), 1).unwrap();
    let r = s.requests(t, 2).unwrap();
    assert_eq!(r.len(), 2);
    assert!(s.complete(&r[1], ready(&r[1], 0.), 3).unwrap());
    let v = snap(&s);
    assert!(v.trail.is_empty());
    assert!(matches!(
        v.latest.unwrap().transform,
        LatestPose::Ready { .. }
    ));
    assert_eq!(s.budget().pending, 2);
    s.complete(&r[0], ready(&r[0], 0.), 4).unwrap();
    let v = snap(&s);
    assert_eq!(
        v.trail.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(v.latest.as_ref(), v.trail.last());
}
#[test]
fn head_covariance_is_fresh_while_thinning_uses_the_last_committed_anchor() {
    let (mut s, t) = start(HistorySettings {
        keep: 100,
        position_tolerance_m: 1.,
        angle_tolerance_rad: 0.2,
    });
    s.input(t, 1, decoded(Kind::Odometry, 10, 0.), 0).unwrap();
    capture(&mut s, t, 0);
    let mut next = decoded(Kind::Odometry, 20, 0.2);
    next.covariance = crate::uncertainty::Covariance::new(std::array::from_fn(|i| {
        if i / 6 == i % 6 {
            4.
        } else {
            0.
        }
    }))
    .unwrap();
    s.input(t, 2, next, 1).unwrap();
    capture(&mut s, t, 1);
    let v = snap(&s);
    assert_eq!(v.trail.len(), 1);
    assert_eq!(v.latest.unwrap().covariance.row_major()[0], 4.);
    assert_eq!(v.trail[0].covariance.row_major()[0], 0.);
    s.input(t, 3, decoded(Kind::Odometry, 30, 0.8), 2).unwrap();
    capture(&mut s, t, 2);
    assert_eq!(snap(&s).trail.len(), 1);
    s.input(t, 4, decoded(Kind::Odometry, 40, 1.), 3).unwrap();
    capture(&mut s, t, 3);
    assert_eq!(snap(&s).trail.len(), 2);
    let mut sign = decoded(Kind::Odometry, 50, 1.);
    sign.components[6] = -0.9999999;
    s.input(t, 5, sign, 4).unwrap();
    capture(&mut s, t, 4);
    assert_eq!(snap(&s).trail.len(), 2);
    let mut turned = decoded(Kind::Odometry, 60, 1.);
    turned.components[5] = 0.15f64.sin();
    turned.components[6] = 0.15f64.cos();
    s.input(t, 6, turned, 5).unwrap();
    capture(&mut s, t, 5);
    assert_eq!(snap(&s).trail.len(), 3);
    assert_eq!(snap(&s).counters.filtered, 3);
}
#[test]
fn clear_preserves_duplicate_guard_and_changed_equal_stamp_requires_restart() {
    let (mut s, t) = start(HistorySettings::default());
    let input = decoded(Kind::Odometry, 10, 1.);
    s.input(t, 1, input.clone(), 0).unwrap();
    let r = s.requests(t, 0).unwrap().remove(0);
    let epoch = snap(&s).continuity_epoch;
    s.clear(t).unwrap();
    assert!(snap(&s).latest.is_none());
    assert!(s.budget().duplicate_guard);
    assert_eq!(s.input(t, 2, input.clone(), 1).unwrap(), Input::Duplicate);
    assert!(snap(&s).latest.is_none());
    assert!(!s.complete(&r, ready(&r, 0.), 0).unwrap());
    assert!(snap(&s).continuity_epoch > epoch);
    let mut changed = input;
    changed.components[0] = 2.;
    assert_eq!(s.input(t, 3, changed, 2), Err(Fault::TimestampConflict));
    assert!(s.token().is_none());
    assert!(snap(&s).latest.is_none());
    assert_eq!(s.fault(), Some(Fault::TimestampConflict));
    let fresh = s
        .start(owner(Kind::Odometry), Some(HistorySettings::default()))
        .unwrap();
    assert_ne!(fresh, t);
    assert_eq!(
        s.input(t, 99, decoded(Kind::Odometry, 999, 999.), u64::MAX)
            .unwrap(),
        Input::Stale
    );
    s.input(fresh, 1, decoded(Kind::Odometry, 1, 1.), 0)
        .unwrap();
    capture(&mut s, fresh, 0);
    assert_eq!(snap(&s).trail.len(), 1);
}
#[test]
fn zero_stamp_is_a_new_latest_capture_per_receipt_and_old_captures_stay_frozen() {
    let (mut s, t) = start(HistorySettings {
        position_tolerance_m: 0.,
        angle_tolerance_rad: 0.,
        ..Default::default()
    });
    let input = decoded(Kind::Odometry, 0, 1.);
    s.input(t, 1, input.clone(), 0).unwrap();
    let r = s.requests(t, 0).unwrap().remove(0);
    assert_eq!(r.time(), LookupTime::Latest);
    s.complete(&r, ready(&r, 1.), 1).unwrap();
    s.input(t, 2, input, 2).unwrap();
    let r2 = s.requests(t, 2).unwrap().remove(0);
    s.complete(&r2, ready(&r2, 2.), 3).unwrap();
    let v = snap(&s);
    assert_eq!(v.trail.len(), 2);
    assert_eq!(v.trail[0].transform, ready(&r, 1.));
    assert_eq!(v.trail[1].transform, ready(&r2, 2.));
    assert!(!s.complete(&r, ready(&r, 9.), 0).unwrap());
}
#[test]
fn frame_child_and_time_policy_changes_cancel_old_epoch_requests() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    let old = s.requests(t, 0).unwrap().remove(0);
    let mut next = decoded(Kind::Odometry, 5, 2.);
    next.source.frame_id = "map".into();
    s.input(t, 2, next.clone(), 1).unwrap();
    assert_eq!(snap(&s).last_reset, Some(ResetReason::FrameChanged));
    assert!(!s.complete(&old, ready(&old, 0.), 0).unwrap());
    capture(&mut s, t, 1);
    assert_eq!(snap(&s).trail.len(), 1);
    next.acquisition_stamp_ns = 0;
    s.input(t, 3, next.clone(), 2).unwrap();
    assert_eq!(snap(&s).last_reset, Some(ResetReason::TimePolicyChanged));
    next.source.child_frame_id = Some("new_body".into());
    s.input(t, 4, next, 3).unwrap();
    assert_eq!(snap(&s).last_reset, Some(ResetReason::FrameChanged));
    assert!(snap(&s).trail.is_empty());
}
#[test]
fn known_loss_is_scoped_latched_through_clear_and_reset_only_by_activation() {
    let (mut a, ta) = start(HistorySettings::default());
    let (mut b, tb) = start(HistorySettings::default());
    for (s, t) in [(&mut a, ta), (&mut b, tb)] {
        s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
        capture(s, t, 0);
    }
    a.source_gap(ta).unwrap();
    assert!(snap(&a).trail.is_empty());
    assert!(snap(&a).known_history_gap);
    assert_eq!(snap(&b).trail.len(), 1);
    a.clear(ta).unwrap();
    assert!(snap(&a).known_history_gap);
    a.input(ta, 2, decoded(Kind::Odometry, 20, 2.), 1).unwrap();
    capture(&mut a, ta, 1);
    assert!(snap(&a).known_history_gap);
    a.start(owner(Kind::Odometry), Some(HistorySettings::default()))
        .unwrap();
    assert!(!snap(&a).known_history_gap);
}
#[test]
fn retries_do_not_extend_receipt_deadline_and_stale_callbacks_cannot_regress_clock() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    let first = s.requests(t, 0).unwrap().remove(0);
    assert!(s.requests(t, 1).unwrap().is_empty());
    s.complete(&first, LatestPose::Missing, 5).unwrap();
    assert!(s.requests(t, RETRY_NS).unwrap().is_empty());
    let second = s.requests(t, RETRY_NS + 5).unwrap().remove(0);
    assert_ne!(first, second);
    assert!(!s.complete(&first, ready(&first, 0.), 0).unwrap());
    s.complete(&second, LatestPose::Extrapolated, 900_000_000)
        .unwrap();
    let third = s.requests(t, 950_000_000).unwrap().remove(0);
    assert!(!s.complete(&third, ready(&third, 0.), TIMEOUT_NS).unwrap());
    let v = snap(&s);
    assert_eq!(v.counters.lookup_timeouts, 1);
    assert_eq!(v.latest.unwrap().transform, LatestPose::LookupTimeout);
    assert!(v.known_history_gap);
    assert_eq!(s.budget().pending, 0);
}
#[test]
fn timeout_of_older_input_releases_already_ready_newer_anchor() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    s.input(t, 2, decoded(Kind::Odometry, 20, 2.), 1).unwrap();
    let r = s.requests(t, 2).unwrap();
    s.complete(&r[1], ready(&r[1], 0.), 3).unwrap();
    s.tick(t, TIMEOUT_NS).unwrap();
    let v = snap(&s);
    assert_eq!(v.trail.iter().map(|e| e.sequence).collect::<Vec<_>>(), [2]);
    assert!(matches!(
        v.latest.unwrap().transform,
        LatestPose::Ready { .. }
    ));
    assert!(v.known_history_gap);
}
#[test]
fn pending_overflow_and_receipt_gaps_clear_history_before_new_capture() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    let old = s.requests(t, 0).unwrap().remove(0);
    for i in 2..=MAX_PENDING {
        s.input(
            t,
            i as u64,
            decoded(Kind::Odometry, i as i64 * 10, i as f64),
            0,
        )
        .unwrap();
    }
    assert_eq!(s.budget().pending, MAX_PENDING);
    s.input(t, 17, decoded(Kind::Odometry, 170, 17.), 0)
        .unwrap();
    let v = snap(&s);
    assert!(v.known_history_gap);
    assert_eq!(v.counters.pending_dropped, 16);
    assert_eq!(s.budget().pending, 1);
    assert!(!s.complete(&old, ready(&old, 0.), 0).unwrap());
    capture(&mut s, t, 0);
    s.input(t, 19, decoded(Kind::Odometry, 190, 19.), 1)
        .unwrap();
    assert!(snap(&s).trail.is_empty());
    assert_eq!(snap(&s).counters.known_source_gaps, 2);
}
#[test]
fn covariance_pose_replaces_pending_without_allocating_a_trail() {
    let mut s = Store::default();
    let t = s.start(owner(Kind::PoseWithCovariance), None).unwrap();
    s.input(t, 1, decoded(Kind::PoseWithCovariance, 10, 1.), 0)
        .unwrap();
    let old = s.requests(t, 0).unwrap().remove(0);
    s.input(t, 2, decoded(Kind::PoseWithCovariance, 20, 2.), 1)
        .unwrap();
    assert_eq!(s.budget().pending, 1);
    assert!(!s.complete(&old, ready(&old, 0.), 0).unwrap());
    capture(&mut s, t, 1);
    let v = snap(&s);
    assert!(v.trail.is_empty());
    assert_eq!(v.latest.unwrap().sequence, 2);
    assert!(!v.known_history_gap);
}
#[test]
fn maximum_history_and_pending_storage_stay_bounded_and_stop_releases_all() {
    let (mut s, t) = start(HistorySettings {
        keep: MAX_HISTORY,
        ..Default::default()
    });
    for i in 1..=MAX_HISTORY + 4 {
        s.input(
            t,
            i as u64,
            decoded(Kind::Odometry, i as i64 * 10, i as f64),
            i as u64,
        )
        .unwrap();
        capture(&mut s, t, i as u64);
    }
    let v = snap(&s);
    assert_eq!(v.trail.len(), MAX_HISTORY);
    assert_eq!(v.trail[0].sequence, 5);
    assert_eq!(v.counters.evicted, 4);
    for i in 261..=276 {
        s.input(t, i, decoded(Kind::Odometry, i as i64 * 10, i as f64), i)
            .unwrap();
    }
    assert_eq!(
        s.budget(),
        Budget {
            anchors: 256,
            pending: 16,
            head: true,
            duplicate_guard: true,
            resolved_head: true,
        }
    );
    assert!(snap(&s).encode().unwrap().len() <= MAX_SAMPLE_BYTES);
    let old = s.requests(t, 276).unwrap().remove(0);
    s.stop();
    assert_eq!(s.budget(), Budget::default());
    assert!(s.snapshot().is_none());
    assert!(!s.complete(&old, ready(&old, 0.), 0).unwrap());
}
#[test]
fn identity_uses_no_requests_and_invalid_draft_does_not_replace_active_state() {
    let mut o = owner(Kind::Odometry);
    o.fixed_frame = "odom".into();
    let mut s = Store::default();
    let t = s.start(o, Some(HistorySettings::default())).unwrap();
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    assert!(s.requests(t, 0).unwrap().is_empty());
    assert_eq!(snap(&s).trail.len(), 1);
    assert!(s
        .start(
            owner(Kind::PoseWithCovariance),
            Some(HistorySettings::default())
        )
        .is_err());
    assert_eq!(s.token(), Some(t));
    assert_eq!(snap(&s).trail.len(), 1);
}
#[test]
fn invalid_input_decreasing_stamp_and_bad_result_fault_and_retire_state() {
    let (mut s, t) = start(HistorySettings::default());
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    let r = s.requests(t, 0).unwrap().remove(0);
    let mut bad = ready(&r, 0.);
    if let LatestPose::Ready {
        ref mut resolved_stamp_ns,
        ..
    } = bad
    {
        *resolved_stamp_ns = 11;
    }
    assert_eq!(s.complete(&r, bad, 1), Err(Fault::InvalidTransform));
    assert!(snap(&s).latest.is_none());
    assert!(s.token().is_none());
    let t = s
        .start(owner(Kind::Odometry), Some(HistorySettings::default()))
        .unwrap();
    let mut input = decoded(Kind::Odometry, 10, 1.);
    input.components[6] = 0.;
    assert_eq!(s.input(t, 1, input, 0), Err(Fault::InvalidInput));
    let t = s
        .start(owner(Kind::Odometry), Some(HistorySettings::default()))
        .unwrap();
    s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 0).unwrap();
    assert_eq!(
        s.input(t, 2, decoded(Kind::Odometry, 9, 1.), 1),
        Err(Fault::TimestampConflict)
    );
    assert_eq!(s.budget(), Budget::default());
}
#[test]
fn clock_and_identity_exhaustion_never_leave_a_reusable_activation() {
    for which in 0..4 {
        let (mut s, t) = start(HistorySettings::default());
        s.input(t, 1, decoded(Kind::Odometry, 10, 1.), 10).unwrap();
        let result = match which {
            0 => s.tick(t, 9).map(|_| ()),
            1 => {
                s.request = u64::MAX;
                s.requests(t, 10).map(|_| ())
            }
            2 => {
                s.epoch = u64::MAX;
                s.clear(t).map(|_| ())
            }
            _ => {
                s.revision = u64::MAX;
                s.clear(t).map(|_| ())
            }
        };
        assert!(result.is_err());
        assert!(s.token().is_none());
        assert_eq!(s.budget(), Budget::default());
    }
}
