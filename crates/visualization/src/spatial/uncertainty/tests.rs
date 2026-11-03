use super::*;
use studio_protocol::{
    pose_snapshot::{
        transport::{Configuration, HistoryConfiguration, Receiver},
        Kind, Owner, Source,
    },
    subscriptions::{Generation, RequestId, SubscriptionId},
    uncertainty::Covariance,
};
fn entry(sequence: u64) -> Entry {
    let values = std::array::from_fn(|i| if i / 6 == i % 6 { 0.04 } else { 0. });
    Entry {
        sequence,
        acquisition_stamp_ns: sequence as i64,
        components: [0., 0., 0., 0., 0., 0., 1.],
        covariance: Covariance::new(values).unwrap(),
        transform: LatestPose::Ready {
            resolved_stamp_ns: sequence as i64,
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        contract_version: 1,
        owner: Owner {
            generation: Generation(1),
            subscription: SubscriptionId(2),
            kind: Kind::Odometry,
            topic: "/odom".into(),
            fixed_frame: "world".into(),
        },
        revision: 1,
        continuity_epoch: 1,
        source: Some(Source {
            frame_id: "source".into(),
            child_frame_id: Some("base".into()),
        }),
        history: Some(HistoryConfiguration::default().settings().unwrap()),
        latest: Some(entry(1)),
        trail: vec![],
        last_reset: None,
        known_history_gap: false,
        counters: Default::default(),
    }
}
#[test]
fn covariance_axes_follow_captured_tf_without_body_rotation() {
    let mut s = snapshot();
    let e = s.latest.as_mut().unwrap();
    let q = std::f64::consts::FRAC_1_SQRT_2;
    e.components[5] = q;
    e.components[6] = q;
    e.transform = LatestPose::Ready {
        resolved_stamp_ns: 1,
        components: [0., 0., 0., 0., 0., q, q],
    };
    let mut values = [0.; 36];
    values[0] = 4.;
    values[7] = 1.;
    e.covariance = Covariance::new(values).unwrap();
    let p = Prepared::new(Arc::new(s)).unwrap();
    let g = Geometry::new(
        &p,
        Appearance {
            orientation: false,
            ..Default::default()
        },
    )
    .unwrap();
    let yellow: Vec<_> = g
        .segments
        .iter()
        .filter(|s| s.color == [0.96, 0.73, 0.16, 1.])
        .flat_map(|s| [s.a, s.b])
        .collect();
    let max_x = yellow.iter().map(|p| p.0[0].abs()).fold(0_f64, f64::max);
    let max_y = yellow.iter().map(|p| p.0[1].abs()).fold(0_f64, f64::max);
    assert!((max_x - 1.).abs() < 1e-10);
    assert!((max_y - 2.).abs() < 1e-10);
    let nominal = p.head.as_ref().unwrap().pose.apply(Point3([1., 0., 0.]));
    assert!((nominal.0[0] + 1.).abs() < 1e-10);
}
#[test]
fn maximum_history_and_head_are_bounded_and_zero_covariance_draws_only_nominal_pose() {
    let mut s = snapshot();
    s.history.as_mut().unwrap().keep = MAX_HISTORY;
    s.trail = (1..=MAX_HISTORY as u64).map(entry).collect();
    s.latest = Some(entry(MAX_HISTORY as u64 + 1));
    let p = Prepared::new(Arc::new(s.clone())).unwrap();
    let a = Appearance {
        history_covariance: true,
        ..Default::default()
    };
    let all = Geometry::new(&p, a).unwrap();
    assert!(all.segments.len() <= MAX_SEGMENTS / 2);
    let camera = CameraPose::orbit(0.5, 0.8, 10., [0.; 3]);
    let vertices = all.vertices(camera, [800, 600], 2).unwrap();
    assert!(vertices.len() <= MAX_VERTICES / 2);
    assert!(vertices
        .iter()
        .all(|v| v.clip.iter().all(|n| n.is_finite())));
    s.latest = s.trail.last().cloned();
    let duplicate = Geometry::new(&Prepared::new(Arc::new(s)).unwrap(), a).unwrap();
    assert!(duplicate.segments.len() < all.segments.len());
    let mut zero = snapshot();
    zero.latest.as_mut().unwrap().covariance = Covariance::new([0.; 36]).unwrap();
    let g = Geometry::new(&Prepared::new(Arc::new(zero)).unwrap(), Default::default()).unwrap();
    assert_eq!(g.segments.len(), 3);
}
#[test]
fn angular_caps_and_world_bounds_are_finite_and_missing_head_keeps_explicit_history() {
    let mut s = snapshot();
    let mut values = [0.; 36];
    values[21] = 1e12;
    values[28] = 1e12;
    values[35] = 1e12;
    s.latest.as_mut().unwrap().covariance = Covariance::new(values).unwrap();
    let g = Geometry::new(&Prepared::new(Arc::new(s)).unwrap(), Default::default()).unwrap();
    assert_eq!(g.capped_angles, 6);
    assert!(g
        .segments
        .iter()
        .all(|s| bounded_point(s.a) && bounded_point(s.b)));
    let mut missing = snapshot();
    missing.trail = vec![entry(1)];
    let mut latest = entry(2);
    latest.transform = LatestPose::Missing;
    missing.latest = Some(latest);
    let p = Prepared::new(Arc::new(missing)).unwrap();
    assert!(p.head.is_none());
    let g = Geometry::new(&p, Default::default()).unwrap();
    assert_eq!(g.segments.len(), 3);
    let mut outside = snapshot();
    outside.latest.as_mut().unwrap().components[0] = 999_999.9;
    let g = Geometry::new(
        &Prepared::new(Arc::new(outside)).unwrap(),
        Default::default(),
    )
    .unwrap();
    assert!(g.hidden_shapes > 0);
    assert!(g
        .segments
        .iter()
        .all(|s| bounded_point(s.a) && bounded_point(s.b)));
}
#[test]
fn cached_geometry_requires_its_exact_snapshot_permission_for_drawing_and_fit() {
    let s = snapshot();
    let mut r = Receiver::new(
        s.owner.clone(),
        Configuration {
            kind: Kind::Odometry,
            history: Some(Default::default()),
        },
    )
    .unwrap();
    r.apply(s).unwrap();
    let p = Prepared::new(r.snapshot().unwrap().clone()).unwrap();
    let g = Geometry::new(&p, Default::default()).unwrap();
    let permission = r.permission().unwrap();
    let layer = Layer {
        id: 1,
        prepared: &p,
        geometry: &g,
        permission: &permission,
        width: 2,
    };
    assert!(layer.is_current());
    assert!(layer.bounds().is_some());
    let copied = Prepared::new(Arc::new((*p.snapshot).clone())).unwrap();
    let wrong = Geometry::new(&copied, Default::default()).unwrap();
    assert!(!Layer {
        geometry: &wrong,
        ..layer
    }
    .is_current());
    r.begin_clear(RequestId(1)).unwrap();
    assert!(!layer.is_current());
    assert!(layer.bounds().is_none());
}
