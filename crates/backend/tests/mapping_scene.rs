//! Public admission/native-TF boundaries; actor/codec/pending tests live in modules.
use studio_backend::spatial_session::{ClockPolicy, SpatialSession, Status, StreamKind, Transform};
use studio_protocol::{
    mapping, robot, spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::{resource_usage, Configuration, Output},
};
fn configuration(output: Output, topic: &str) -> Configuration {
    Configuration {
        map_updates: None,
        topic: topic.into(),
        message_type: output
            .mapping_message_type()
            .unwrap_or("sensor_msgs/msg/LaserScan")
            .into(),
        best_effort: false,
        transient_local: output == Output::OccupancyGrid,
        output,
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
fn robot_mapping_baseline_and_current_capacity_boundaries_are_preserved() {
    let map = configuration(Output::OccupancyGrid, "/map");
    let pose = configuration(Output::Pose, "/pose");
    let path = configuration(Output::Path, "/path");
    let a = configuration(Output::SpatialScan, "/scan_a");
    let b = configuration(Output::SpatialScan, "/scan_b");
    let mut cloud = configuration(Output::PointCloud, "/cloud");
    cloud.message_type = studio_protocol::cloud::MESSAGE_TYPE.into();
    let robot = Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        output: Output::Robot,
        best_effort: false,
        transient_local: false,
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
    let all = [&robot, &a, &b, &cloud, &map, &pose, &path];
    let usage = resource_usage(&all).unwrap();
    assert_eq!(
        (
            usage.native_handles,
            usage.logical,
            usage.sensor_layers,
            usage.robots
        ),
        (8, 7, 3, 1)
    );
    let extra = Configuration {
        map_updates: None,
        topic: "/peer".into(),
        message_type: "std_msgs/msg/Float64".into(),
        output: Output::Typed,
        best_effort: false,
        transient_local: false,
        spatial: None,
        robot: None,
    };
    let mut over = all.to_vec();
    over.push(&extra);
    assert_eq!(resource_usage(&over).unwrap().native_handles, 9);
    let duplicate = configuration(Output::OccupancyGrid, "/other_map");
    let third = configuration(Output::OccupancyGrid, "/local_costmap");
    let fourth = configuration(Output::OccupancyGrid, "/extra_map");
    assert!(resource_usage(&[&map, &duplicate, &third]).is_ok());
    assert!(resource_usage(&[&map, &duplicate, &third, &fourth]).is_err());
    let typed = (0..studio_protocol::subscriptions::MAX_SUBSCRIPTIONS)
        .map(|i| Configuration {
            topic: format!("/typed_{i}"),
            ..extra.clone()
        })
        .collect::<Vec<_>>();
    assert!(resource_usage(&typed.iter().collect::<Vec<_>>()).is_ok());
    let mut over_logical = typed.iter().collect::<Vec<_>>();
    over_logical.push(&extra);
    assert!(resource_usage(&over_logical).is_err());
    let mut wrong = pose.clone();
    wrong.spatial.as_mut().unwrap().fixed_frame = "odom".into();
    assert!(resource_usage(&[&map, &wrong]).is_err());
}
#[test]
fn historical_mapping_lookup_cannot_substitute_latest_and_path_child_stamps_remain_metadata() {
    let mut tf = SpatialSession::new().unwrap();
    tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
    for (stamp, x) in [(20_000_000_000, 2.), (22_000_000_000, 4.)] {
        tf.insert(
            StreamKind::Dynamic,
            &Transform {
                parent: "map".into(),
                child: "source".into(),
                stamp_ns: stamp,
                components: [x, 0., 0., 0., 0., 0., 1.],
            },
        )
        .unwrap();
    }
    let path = mapping::Decoded {
        topic: "/path".into(),
        message_type: mapping::PATH_MESSAGE_TYPE.into(),
        source_frame: "source".into(),
        acquisition_stamp_ns: 21_000_000_000,
        geometry: mapping::Geometry::Path {
            poses: vec![
                mapping::PathPose {
                    frame_id: String::new(),
                    stamp_ns: 0,
                    components: [1., 2., 3., 0., 0., 0., 1.],
                },
                mapping::PathPose {
                    frame_id: "source".into(),
                    stamp_ns: 100_000_000_000,
                    components: [2., 3., 4., 0., 0., 0., 1.],
                },
            ],
        },
    };
    path.validate().unwrap();
    let acquired = tf
        .lookup_at("map", &path.source_frame, path.acquisition_stamp_ns)
        .unwrap();
    assert_eq!(acquired.components[0], 3.);
    assert_eq!(acquired.stamp_ns, path.acquisition_stamp_ns);
    assert_eq!(tf.lookup_latest("map", "source").unwrap().components[0], 4.);
    assert_eq!(
        tf.lookup_at("map", "source", 1).unwrap_err().status,
        Status::Extrapolated
    );
    assert_eq!(
        tf.lookup_at("map", "source", 0).unwrap_err().status,
        Status::Invalid
    );
}
