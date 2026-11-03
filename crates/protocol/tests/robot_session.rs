//! Independent paired session8/robot1 contract and admission regressions.
use studio_protocol::{
    robot::{self, Counters, LinkPose, LinkResult, PosePolicy, Sample},
    session::{Command, Event},
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::{
        Configuration, Generation, Output, Registry, RegistryError, RequestId, SubscriptionId,
        ViewId,
    },
};

fn robot_configuration() -> robot::Configuration {
    robot::Configuration {
        fixed_frame: "map".into(),
        frames: vec!["base".into(), "arm".into()],
        clock_policy: SpatialClockPolicy::VerifiedSystem,
        dynamic_tf_topic: "/tf".into(),
        dynamic_tf_best_effort: true,
        static_tf_topic: "/tf_static".into(),
    }
}
fn robot_output(robot: robot::Configuration) -> Configuration {
    Configuration {
        map_updates: None,
        topic: robot.dynamic_tf_topic.clone(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        best_effort: robot.dynamic_tf_best_effort,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(Box::new(robot)),
    }
}
fn typed() -> Configuration {
    Configuration {
        map_updates: None,
        topic: "/status".into(),
        message_type: "std_msgs/msg/String".into(),
        best_effort: false,
        transient_local: false,
        output: Output::Typed,
        spatial: None,
        robot: None,
    }
}
fn scan() -> Configuration {
    Configuration {
        map_updates: None,
        topic: "/scan".into(),
        message_type: "sensor_msgs/msg/LaserScan".into(),
        best_effort: true,
        transient_local: false,
        output: Output::SpatialScan,
        spatial: Some(spatial::Configuration {
            fixed_frame: "map".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: true,
            static_tf_topic: "/tf_static".into(),
        }),
        robot: None,
    }
}
fn sample(config: &robot::Configuration) -> Sample {
    Sample {
        contract_version: 1,
        fixed_frame: config.fixed_frame.clone(),
        policy: PosePolicy::LatestAvailable,
        links: config
            .frames
            .iter()
            .enumerate()
            .map(|(i, frame)| LinkResult {
                frame: frame.clone(),
                pose: if i == 0 {
                    LinkPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                    }
                } else {
                    LinkPose::Missing
                },
            })
            .collect(),
        counters: Counters::default(),
    }
}

#[test]
fn maximum_escaped_frame_start_roundtrips_with_explicit_128kib_command_contract() {
    let mut config = robot_configuration();
    config.frames = (0..128)
        .map(|i| format!("f{i:03}_{}q", "\\\"".repeat(125)))
        .collect();
    assert!(config.frames.iter().all(|frame| frame.len() == 256));
    config.validate().unwrap();
    let command = Command::Start {
        generation: Generation(7),
        request: RequestId(11),
        subscription: SubscriptionId(13),
        configuration: robot_output(config.clone()),
    };
    let bytes = command.encode().unwrap();
    assert!(
        bytes.len() > 4096,
        "maximum legal Robot requests exceed the retired session6 command bound"
    );
    assert!(bytes.len() <= 128 * 1024);
    assert_eq!(studio_protocol::session::VERSION, 21);
    assert_eq!(studio_protocol::session::MAX_COMMAND_BYTES, 128 * 1024);
    assert_eq!(Command::decode(&bytes).unwrap(), command);
    let event = Event::RobotObservation {
        generation: Generation(7),
        subscription: SubscriptionId(13),
        sequence: 1,
        sample: Box::new(sample(&config)),
        backend_drops: 0,
    };
    let decoded = Event::decode(&event.encode().unwrap()).unwrap();
    match decoded {
        Event::RobotObservation { sample, .. } => {
            sample.validate(&config).unwrap();
            assert_eq!(sample.links.len(), 128);
        }
        _ => panic!("Robot event changed kind"),
    }
}

#[test]
fn ready_rejects_old_session_and_missing_or_contradictory_robot_limits() {
    let ready = Event::ready("jazzy".into(), "synthetic-no-ROS".into());
    let wire = serde_json::to_value(&ready).unwrap();
    assert_eq!(wire["version"], studio_protocol::session::VERSION);
    for (key, expected) in [
        ("robot_contract_version", 1),
        ("max_robot_links", 128),
        ("max_robot_hz", 10),
        ("max_robot_sample_bytes", 512 * 1024),
    ] {
        assert_eq!(wire[key], expected);
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            Event::decode(&serde_json::to_vec(&missing).unwrap()).is_err(),
            "missing capability {key} cannot negotiate Robot"
        );
        let mut wrong = wire.clone();
        wrong[key] = serde_json::json!(0);
        assert!(
            Event::decode(&serde_json::to_vec(&wrong).unwrap()).is_err(),
            "contradictory capability {key} cannot negotiate Robot"
        );
    }
    let mut old = wire;
    old["version"] = serde_json::json!(6);
    assert!(Event::decode(&serde_json::to_vec(&old).unwrap()).is_err());
}

#[test]
fn robot_configuration_cannot_smuggle_conflicting_source_qos_or_other_output_settings() {
    let valid = robot_output(robot_configuration());
    assert!(valid.valid());
    assert_eq!(valid.resource_cost(), 0);
    assert_eq!(
        studio_protocol::subscriptions::resource_usage(&[&valid])
            .unwrap()
            .native_handles,
        2
    );
    let mut bad = valid.clone();
    bad.topic = "/other_tf".into();
    assert!(!bad.valid());
    let mut bad = valid.clone();
    bad.best_effort = !bad.best_effort;
    assert!(!bad.valid());
    let mut bad = valid.clone();
    bad.transient_local = true;
    assert!(!bad.valid());
    let mut bad = valid.clone();
    bad.message_type = "std_msgs/msg/String".into();
    assert!(!bad.valid());
    let mut bad = valid.clone();
    bad.robot = None;
    assert!(!bad.valid());
    let mut bad = valid.clone();
    bad.spatial = scan().spatial;
    assert!(!bad.valid());
    for mut other in [
        typed(),
        scan(),
        Configuration {
            topic: "/image".into(),
            message_type: "sensor_msgs/msg/Image".into(),
            output: Output::CompactImage,
            ..typed()
        },
    ] {
        other.robot = valid.robot.clone();
        assert!(
            !other.valid(),
            "non-Robot output cannot carry Robot settings"
        );
    }
}

#[test]
fn batch_shape_validation_is_distinct_from_requested_identity_matching() {
    let config = robot_configuration();
    let valid = sample(&config);
    valid.validate_structure().unwrap();
    valid.validate(&config).unwrap();
    let mut reordered = valid.clone();
    reordered.links.reverse();
    reordered.validate_structure().unwrap();
    assert!(reordered.validate(&config).is_err());
    assert!(reordered.encode(&config).is_err());
    assert!(
        reordered.encode_wire().is_ok(),
        "wire shape does not infer the active request"
    );
    let mut duplicate = valid.clone();
    duplicate.links[1].frame = duplicate.links[0].frame.clone();
    assert!(duplicate.validate_structure().is_err());
    let mut missing = valid.clone();
    missing.links.pop();
    assert!(
        missing.validate(&config).is_err(),
        "partial poses still require complete named result entries"
    );
    let mut wrong_fixed = valid.clone();
    wrong_fixed.fixed_frame = "other_map".into();
    wrong_fixed.validate_structure().unwrap();
    assert!(wrong_fixed.validate(&config).is_err());
    let event = Event::RobotObservation {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        sequence: 3,
        sample: Box::new(valid),
        backend_drops: 0,
    };
    let wire = serde_json::to_value(event).unwrap();
    for key in ["generation", "subscription", "sequence"] {
        let mut zero = wire.clone();
        zero[key] = serde_json::json!(0);
        assert!(
            Event::decode(&serde_json::to_vec(&zero).unwrap()).is_err(),
            "zero envelope identity {key} must fail"
        );
    }
}

#[test]
fn tf_group_conflict_retains_resources_through_stop_and_exact_retirement() {
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let first = registry
        .start_explicit(ViewId(1), robot_output(robot_configuration()))
        .unwrap();
    assert_eq!(registry.used_resources(), 2);
    assert_eq!(
        registry
            .start_explicit(ViewId(2), robot_output(robot_configuration()))
            .unwrap_err(),
        RegistryError::Capacity,
        "two Robot TF groups must not fit even though cost2+2 fits four handles"
    );
    registry
        .start_ack(first.generation, first.subscription, first.request)
        .unwrap();
    for id in 3..=33 {
        registry.start_explicit(ViewId(id), typed()).unwrap();
    }
    assert_eq!(registry.used_resources(), 33);
    assert_eq!(
        registry.start_explicit(ViewId(34), typed()).unwrap_err(),
        RegistryError::Capacity
    );
    let stop = registry.stop_explicit(first.subscription).unwrap();
    assert_eq!(registry.used_resources(), 33);
    assert_eq!(
        registry.sample(first.generation, first.subscription, 1),
        Err(RegistryError::WrongState)
    );
    let retry = registry.stop_explicit(first.subscription).unwrap();
    assert_eq!(
        registry.stop_ack(stop.generation, stop.subscription, stop.request),
        Err(RegistryError::WrongRequest)
    );
    assert_eq!(registry.used_resources(), 33);
    registry
        .stop_ack(retry.generation, retry.subscription, retry.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 31);
    registry
        .start_explicit(ViewId(35), robot_output(robot_configuration()))
        .unwrap();
}

#[test]
fn stopping_robot_blocks_tf_replacement_and_reconnect_never_replays_old_identity() {
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let old = registry
        .start_explicit(ViewId(1), robot_output(robot_configuration()))
        .unwrap();
    let stop = registry.stop_explicit(old.subscription).unwrap();
    assert_eq!(
        registry
            .start_explicit(ViewId(2), robot_output(robot_configuration()))
            .unwrap_err(),
        RegistryError::Capacity
    );
    let mut incompatible = scan();
    incompatible.spatial.as_mut().unwrap().fixed_frame = "other".into();
    assert_eq!(
        registry
            .start_explicit(ViewId(3), incompatible)
            .unwrap_err(),
        RegistryError::Capacity
    );
    registry.start_explicit(ViewId(4), scan()).unwrap();
    assert_eq!(
        registry.used_resources(),
        3,
        "Compatible scan shares TF even while Robot is stopping"
    );
    registry.disconnect();
    let new_generation = registry.connect().unwrap();
    assert_ne!(new_generation, old.generation);
    assert!(registry.entries().is_empty());
    assert_eq!(registry.used_resources(), 0);
    assert_eq!(
        registry.start_ack(old.generation, old.subscription, old.request),
        Err(RegistryError::StaleGeneration)
    );
    assert_eq!(
        registry.stop_ack(stop.generation, stop.subscription, stop.request),
        Err(RegistryError::StaleGeneration)
    );
    assert_eq!(
        registry.sample(old.generation, old.subscription, 1),
        Err(RegistryError::StaleGeneration)
    );
    let new = registry
        .start_explicit(ViewId(1), robot_output(robot_configuration()))
        .unwrap();
    assert_ne!(new.subscription, old.subscription);
    assert_eq!(
        registry.sample(new.generation, new.subscription, 1),
        Err(RegistryError::WrongState)
    );
    registry
        .start_ack(new.generation, new.subscription, new.request)
        .unwrap();
    assert_eq!(
        registry.sample(new.generation, new.subscription, 1),
        Ok(ViewId(1))
    );
    assert_eq!(
        registry.sample(new.generation, new.subscription, 1),
        Err(RegistryError::OldSequence)
    );
}
