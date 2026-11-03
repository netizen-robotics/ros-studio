//! Offline cross-boundary checks. Actor ownership/fault tests live alongside the
//! private scene worker; these tests exercise public admission + native TF APIs.
use studio_backend::spatial_session::{ClockPolicy, SpatialSession, Status, StreamKind, Transform};
use studio_protocol::{
    robot, spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::{resource_usage, Configuration, Output},
};
fn scan(topic: &str) -> Configuration {
    Configuration {
        map_updates: None,
        topic: topic.into(),
        message_type: "sensor_msgs/msg/LaserScan".into(),
        best_effort: false,
        transient_local: false,
        output: Output::SpatialScan,
        robot: None,
        spatial: Some(spatial::Configuration {
            fixed_frame: "map".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
    }
}
#[test]
fn robot_two_scans_and_cloud_admit_five_shared_physical_readers() {
    let first = scan("/scan_a");
    let second = scan("/scan_b");
    let mut cloud = scan("/cloud");
    cloud.output = Output::PointCloud;
    cloud.message_type = "sensor_msgs/msg/PointCloud2".into();
    let robot = Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        best_effort: false,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(Box::new(robot::Configuration {
            fixed_frame: "map".into(),
            frames: vec!["base".into()],
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        })),
    };
    let usage = resource_usage(&[&robot, &first, &second, &cloud]).unwrap();
    assert_eq!(
        (
            usage.logical,
            usage.native_handles,
            usage.robots,
            usage.sensor_layers
        ),
        (4, 5, 1, 3)
    );
    let mut incompatible = second.clone();
    incompatible.spatial.as_mut().unwrap().dynamic_tf_topic = "/other_tf".into();
    assert!(resource_usage(&[&robot, &first, &incompatible]).is_err());
}
#[test]
fn one_native_cache_preserves_acquisition_lookup_beside_robot_latest() {
    let mut tf = SpatialSession::new().unwrap();
    tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
    for (stamp, x) in [(1_000_000_000, 1.), (3_000_000_000, 3.)] {
        tf.insert(
            StreamKind::Dynamic,
            &Transform {
                parent: "map".into(),
                child: "base".into(),
                stamp_ns: stamp,
                components: [x, 0., 0., 0., 0., 0., 1.],
            },
        )
        .unwrap();
    }
    let latest = tf.lookup_latest("map", "base").unwrap();
    let acquired = tf.lookup_at("map", "base", 2_000_000_000).unwrap();
    assert_eq!((latest.stamp_ns, latest.components[0]), (3_000_000_000, 3.));
    assert_eq!(
        (acquired.stamp_ns, acquired.components[0]),
        (2_000_000_000, 2.)
    );
    assert_eq!(
        tf.lookup_at("map", "base", 0).unwrap_err().status,
        Status::Invalid
    );
    assert_eq!((tf.edge_count(), tf.frame_count()), (1, 2));
}

#[test]
fn tf_graph_reserves_only_shared_readers_and_rejects_duplicate_or_mismatched_owners() {
    let first = scan("/scan");
    let mut graph = scan("/tf");
    graph.output = Output::TfGraph;
    graph.message_type = "tf2_msgs/msg/TFMessage".into();
    let usage = resource_usage(&[&first, &graph]).unwrap();
    assert_eq!(
        (usage.logical, usage.native_handles, usage.tf_graphs),
        (2, 3, 1)
    );
    assert!(resource_usage(&[&graph, &graph]).is_err());
    let mut other = graph.clone();
    other.spatial.as_mut().unwrap().fixed_frame = "other".into();
    assert!(resource_usage(&[&first, &other]).is_err());
    let mut bad = graph.clone();
    bad.best_effort = true;
    assert!(!bad.valid());
    bad = graph.clone();
    bad.topic = "/another".into();
    assert!(!bad.valid());
    bad = graph.clone();
    bad.transient_local = true;
    assert!(!bad.valid());
    bad = graph.clone();
    bad.spatial.as_mut().unwrap().clock_policy = SpatialClockPolicy::Unknown;
    assert!(!bad.valid());
}
