use studio_protocol::{
    camera_snapshot::{
        self as camera,
        transport::{Capabilities, Message},
        Counters, InfoSource, Owner, Snapshot, Status,
    },
    session::{self, Command, Event},
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::*,
};
fn configuration() -> Configuration {
    Configuration {
        topic: "/camera/image_rect".into(),
        message_type: studio_protocol::image::MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: false,
        output: Output::CameraSnapshot(Box::new(InfoSource {
            topic: "/camera/camera_info".into(),
            best_effort: true,
            transient_local: false,
        })),
        spatial: Some(spatial::Configuration {
            fixed_frame: "world".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
        robot: None,
        map_updates: None,
    }
}
#[test]
fn current_camera_capabilities_reject_old_missing_extra_and_changed_contracts() {
    assert_eq!(session::VERSION, 21);
    let value = serde_json::to_value(Event::ready("jazzy".into(), "fixture".into())).unwrap();
    assert_eq!(
        value["camera_snapshot"],
        serde_json::to_value(Capabilities::default()).unwrap()
    );
    for version in [14, 15] {
        let mut old = value.clone();
        old["version"] = version.into();
        assert!(Event::decode(&serde_json::to_vec(&old).unwrap()).is_err());
    }
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("camera_snapshot");
    assert!(Event::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    for key in value["camera_snapshot"].as_object().unwrap().keys() {
        let mut bad = value.clone();
        bad["camera_snapshot"][key] = 0.into();
        assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = value.clone();
        bad["camera_snapshot"].as_object_mut().unwrap().remove(key);
        assert!(Event::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut extra = value;
    extra["camera_snapshot"]["unexpected"] = true.into();
    assert!(Event::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
}
#[test]
fn paired_camera_admission_keeps_both_readers_reserved_through_start_and_stop() {
    let config = configuration();
    assert!(config.valid());
    assert_eq!(config.resource_cost(), 2);
    assert_eq!(resource_usage(&[&config]).unwrap().native_handles, 4);
    let mut other = config.clone();
    other.topic = "/other/image_rect".into();
    assert!(resource_usage(&[&config, &other]).is_err());
    let mut peers = vec![
        Configuration {
            topic: "/peer".into(),
            message_type: "std_msgs/msg/String".into(),
            best_effort: false,
            transient_local: false,
            output: Output::Typed,
            spatial: None,
            robot: None,
            map_updates: None
        };
        5
    ];
    for (i, p) in peers.iter_mut().enumerate() {
        p.topic = format!("/peer_{i}");
    }
    let mut all = vec![&config];
    all.extend(peers[..4].iter());
    assert_eq!(resource_usage(&all).unwrap().native_handles, 8);
    all.push(&peers[4]);
    assert_eq!(resource_usage(&all).unwrap().native_handles, 9);
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let start = registry.start_explicit(ViewId(1), config).unwrap();
    assert_eq!(registry.used_resources(), 4);
    assert!(registry.start_explicit(ViewId(2), other.clone()).is_err());
    registry
        .start_ack(start.generation, start.subscription, start.request)
        .unwrap();
    assert!(registry.clear_pose_explicit(start.subscription).is_err());
    let stop = registry.stop_explicit(start.subscription).unwrap();
    assert_eq!(registry.used_resources(), 4);
    assert!(registry.start_explicit(ViewId(2), other.clone()).is_err());
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert!(registry.start_explicit(ViewId(2), other).is_ok());
}
#[test]
fn source_grammar_requires_exact_image_info_and_verified_scene_configuration() {
    let config = configuration();
    let command = Command::Start {
        generation: Generation(1),
        request: RequestId(2),
        subscription: SubscriptionId(3),
        configuration: config.clone(),
    };
    assert_eq!(
        Command::decode(&command.encode().unwrap()).unwrap(),
        command
    );
    assert_eq!(serde_json::to_string(&Output::Typed).unwrap(), "\"typed\"");
    let mut bad = config.clone();
    bad.message_type = studio_protocol::camera_info::MESSAGE_TYPE.into();
    assert!(!bad.valid());
    let mut bad = config.clone();
    bad.spatial = None;
    assert!(!bad.valid());
    let mut bad = config.clone();
    bad.spatial.as_mut().unwrap().clock_policy = SpatialClockPolicy::Unknown;
    assert!(!bad.valid());
    let mut bad = config.clone();
    if let Output::CameraSnapshot(info) = &mut bad.output {
        info.topic = bad.topic.clone();
    }
    assert!(!bad.valid());
    let mut wire = serde_json::to_value(command).unwrap();
    wire["configuration"]["output"]["camera_snapshot"]["extra"] = true.into();
    assert!(Command::decode(&serde_json::to_vec(&wire).unwrap()).is_err());
}
#[test]
fn camera_event_identity_and_wire_bounds_are_checked() {
    let config = configuration();
    let Output::CameraSnapshot(info) = config.output else {
        unreachable!()
    };
    let owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        topic: config.topic,
        info: *info,
        fixed_frame: "world".into(),
    };
    let snapshot = Snapshot {
        contract_version: camera::CONTRACT_VERSION,
        owner,
        revision: 1,
        continuity_epoch: 1,
        status: Status::WaitingInfo,
        diagnostic: None,
        counters: Counters::default(),
        frame: None,
    };
    let mut event = Event::CameraSnapshotObservation {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        sequence: 1,
        sample: Box::new(Message::Snapshot(Box::new(snapshot))),
        backend_drops: 0,
    };
    assert!(Event::decode(&event.encode().unwrap()).is_ok());
    if let Event::CameraSnapshotObservation { subscription, .. } = &mut event {
        *subscription = SubscriptionId(99);
    }
    assert!(event.encode().is_err());
}
