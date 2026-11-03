//! Offline standard-native-tf2 acceptance for Robot's latest-available seam.
//! No ROS endpoints or robot commands. Scan acquisition semantics stay separate.
use studio_backend::spatial_session::{ClockPolicy, SpatialSession, Status, StreamKind, Transform};

const SECOND: i64 = 1_000_000_000;
fn edge(parent: &str, child: &str, stamp_ns: i64, x: f64) -> Transform {
    Transform {
        parent: parent.into(),
        child: child.into(),
        stamp_ns,
        components: [x, 0., 0., 0., 0., 0., 1.],
    }
}
fn active() -> SpatialSession {
    let mut session = SpatialSession::new().unwrap();
    session
        .activate_explicit(ClockPolicy::VerifiedSystem)
        .unwrap();
    session
}
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
}
#[test]
fn static_only_latest_returns_native_zero_stamp_without_receipt_time() {
    let mut session = active();
    session
        .insert(StreamKind::Static, &edge("world", "base", 42 * SECOND, 5.))
        .unwrap();
    session
        .insert(StreamKind::Static, &edge("base", "tool", 0, 2.))
        .unwrap();
    let latest = session.lookup_latest("world", "tool").unwrap();
    close(latest.components[0], 7.);
    assert_eq!(latest.stamp_ns, 0);
    assert_eq!((&*latest.target, &*latest.source), ("world", "tool"));
    // Explicit acquisition retains its requested positive stamp and same geometry.
    let acquisition = session.lookup_at("world", "tool", 17 * SECOND).unwrap();
    assert_eq!(acquisition.stamp_ns, 17 * SECOND);
    assert_eq!(acquisition.components, latest.components);
}
#[test]
fn native_identity_requires_activation_but_not_received_frame_names() {
    let mut session = active();
    let identity = session
        .lookup_latest("description_root", "description_root")
        .unwrap();
    assert_eq!(identity.components, [0., 0., 0., 0., 0., 0., 1.]);
    assert_eq!(identity.stamp_ns, 0);
    assert_eq!(session.frame_count(), 0);
    assert_eq!(session.edge_count(), 0);
    assert_eq!(
        session
            .lookup_at("description_root", "description_root", 0)
            .unwrap_err()
            .status,
        Status::Invalid
    );
    assert_eq!(
        session
            .lookup_at("description_root", "description_root", SECOND)
            .unwrap_err()
            .status,
        Status::Missing
    );
    for frame in ["", "/bad", "with space", "with\0nul"] {
        assert_eq!(
            session.lookup_latest(frame, frame).unwrap_err().status,
            Status::Invalid
        );
    }
}
#[test]
fn dynamic_latest_and_mixed_chain_return_native_per_link_common_stamps() {
    let mut session = active();
    session
        .insert(StreamKind::Static, &edge("world", "map", 0, 5.))
        .unwrap();
    for (stamp, x) in [(SECOND, 1.), (3 * SECOND, 3.)] {
        session
            .insert(StreamKind::Dynamic, &edge("map", "base", stamp, x))
            .unwrap();
    }
    for (stamp, x) in [(SECOND, 10.), (2 * SECOND, 20.)] {
        session
            .insert(StreamKind::Dynamic, &edge("base", "tool", stamp, x))
            .unwrap();
    }
    let base = session.lookup_latest("world", "base").unwrap();
    let tool = session.lookup_latest("world", "tool").unwrap();
    assert_eq!(base.stamp_ns, 3 * SECOND);
    assert_eq!(tool.stamp_ns, 2 * SECOND);
    close(base.components[0], 8.);
    close(tool.components[0], 27.);
    let inverse = session.lookup_latest("tool", "world").unwrap();
    assert_eq!(inverse.stamp_ns, 2 * SECOND);
    close(inverse.components[0], -27.);
    assert_eq!(
        session
            .lookup_at("world", "tool", 3 * SECOND)
            .unwrap_err()
            .status,
        Status::Extrapolated
    );
    assert_eq!(
        session.lookup_at("world", "tool", 0).unwrap_err().status,
        Status::Invalid
    );
}
#[test]
fn absent_and_disconnected_links_fail_without_allocating_new_frames() {
    let mut session = active();
    session
        .insert(StreamKind::Static, &edge("world", "base", 0, 1.))
        .unwrap();
    session
        .insert(StreamKind::Static, &edge("elsewhere", "tool", 0, 2.))
        .unwrap();
    let before = (session.frame_count(), session.edge_count());
    assert_eq!(
        session.lookup_latest("world", "absent").unwrap_err().status,
        Status::Missing
    );
    assert_eq!(
        session.lookup_latest("absent", "base").unwrap_err().status,
        Status::Missing
    );
    assert_eq!(
        session.lookup_latest("world", "tool").unwrap_err().status,
        Status::Missing
    );
    assert_eq!((session.frame_count(), session.edge_count()), before);
}
#[test]
fn explicit_reset_disables_latest_and_retains_only_static_paths() {
    let mut session = SpatialSession::new().unwrap();
    assert_eq!(
        session.lookup_latest("base", "base").unwrap_err().status,
        Status::Inactive
    );
    assert_eq!(
        session
            .activate_explicit(ClockPolicy::Unverified)
            .unwrap_err()
            .status,
        Status::ClockUnverified
    );
    assert_eq!(
        session.lookup_latest("base", "base").unwrap_err().status,
        Status::Inactive
    );
    assert_eq!(
        session
            .activate_explicit(ClockPolicy::Simulated)
            .unwrap_err()
            .status,
        Status::UnsupportedClock
    );
    assert_eq!(
        session.lookup_latest("base", "base").unwrap_err().status,
        Status::Inactive
    );
    session
        .activate_explicit(ClockPolicy::VerifiedSystem)
        .unwrap();
    session
        .insert(StreamKind::Static, &edge("world", "base", 0, 1.))
        .unwrap();
    session
        .insert(StreamKind::Dynamic, &edge("base", "tool", SECOND, 2.))
        .unwrap();
    assert!(session.lookup_latest("world", "tool").is_ok());
    session.reset().unwrap();
    assert_eq!(
        session.lookup_latest("world", "base").unwrap_err().status,
        Status::Inactive
    );
    assert_eq!(
        session.lookup_latest("base", "base").unwrap_err().status,
        Status::Inactive
    );
    session
        .activate_explicit(ClockPolicy::VerifiedSystem)
        .unwrap();
    assert_eq!(session.lookup_latest("world", "base").unwrap().stamp_ns, 0);
    assert!(session.lookup_latest("world", "tool").is_err());
    session
        .insert(StreamKind::Dynamic, &edge("base", "tool", SECOND / 2, 3.))
        .unwrap();
    assert_eq!(
        session.lookup_latest("world", "tool").unwrap().stamp_ns,
        SECOND / 2
    );
}
