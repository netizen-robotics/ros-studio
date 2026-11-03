use studio_protocol::{
    pose_snapshot::{
        self as pose,
        transport::{Configuration as PoseConfiguration, HistoryConfiguration},
    },
    session::{Command, Event, VERSION},
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::*,
};
fn configuration(kind: pose::Kind) -> Configuration {
    Configuration {
        topic: format!(
            "/{}",
            if kind == pose::Kind::Odometry {
                "odom"
            } else {
                "covariance"
            }
        ),
        message_type: kind.message_type().into(),
        best_effort: false,
        transient_local: false,
        output: Output::PoseSnapshot(PoseConfiguration {
            kind,
            history: (kind == pose::Kind::Odometry).then(HistoryConfiguration::default),
        }),
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
fn exact_current_session_capabilities_reject_old_version_missing_changed_and_extra_fields() {
    assert_eq!(VERSION, 21);
    let ready = serde_json::to_value(Event::ready("jazzy".into(), "fixture".into())).unwrap();
    assert!(Event::decode(&serde_json::to_vec(&ready).unwrap()).is_ok());
    let mut old = ready.clone();
    old["version"] = 14.into();
    assert!(Event::decode(&serde_json::to_vec(&old).unwrap()).is_err());
    for key in ready["pose_snapshot"].as_object().unwrap().keys() {
        let mut changed = ready.clone();
        changed["pose_snapshot"][key] = 0.into();
        assert!(Event::decode(&serde_json::to_vec(&changed).unwrap()).is_err());
        let mut missing = ready.clone();
        missing["pose_snapshot"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(Event::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
    let mut extra = ready;
    extra["pose_snapshot"]["unexpected"] = true.into();
    assert!(Event::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
}
#[test]
fn pose_kind_history_and_scene_admission_preserve_global_limits_until_cleanup() {
    let odom = configuration(pose::Kind::Odometry);
    let covariance = configuration(pose::Kind::PoseWithCovariance);
    assert!(odom.valid());
    assert_eq!(
        resource_usage(&[&odom, &covariance])
            .unwrap()
            .native_handles,
        4
    );
    let mut duplicate = odom.clone();
    duplicate.topic = "/other_odom".into();
    assert!(resource_usage(&[&odom, &duplicate]).is_err());
    let mut wrong = odom.clone();
    wrong.message_type = pose::POSE_MESSAGE_TYPE.into();
    assert!(!wrong.valid());
    wrong = odom.clone();
    wrong.spatial.as_mut().unwrap().clock_policy = SpatialClockPolicy::Unknown;
    assert!(!wrong.valid());
    let mut invalid = odom.clone();
    let Output::PoseSnapshot(ref mut p) = invalid.output else {
        unreachable!()
    };
    p.history = None;
    assert!(!invalid.valid());
    let mut peers = vec![
        Configuration {
            topic: "/peer".into(),
            message_type: "std_msgs/msg/String".into(),
            output: Output::Typed,
            spatial: None,
            robot: None,
            map_updates: None,
            best_effort: false,
            transient_local: false
        };
        5
    ];
    for (i, p) in peers.iter_mut().enumerate() {
        p.topic = format!("/peer_{i}");
    }
    let mut all = vec![&odom, &covariance];
    all.extend(peers[..4].iter());
    assert_eq!(resource_usage(&all).unwrap().native_handles, 8);
    all.push(&peers[4]);
    assert_eq!(resource_usage(&all).unwrap().native_handles, 9);
    let mut r = Registry::default();
    r.connect().unwrap();
    let first = r.start_explicit(ViewId(1), odom).unwrap();
    r.start_ack(first.generation, first.subscription, first.request)
        .unwrap();
    assert!(r.clear_markers_explicit(first.subscription).is_err());
    let clear = r.clear_pose_explicit(first.subscription).unwrap();
    assert_eq!(r.used_resources(), 3);
    assert!(r.clear_pose_explicit(first.subscription).is_err());
    assert!(r
        .clear_pose_ack(
            clear.generation,
            clear.subscription,
            RequestId(clear.request.0 + 1)
        )
        .is_err());
    r.clear_pose_ack(clear.generation, clear.subscription, clear.request)
        .unwrap();
    let stop = r.stop_explicit(first.subscription).unwrap();
    assert!(r.start_explicit(ViewId(2), duplicate.clone()).is_err());
    r.stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert!(r.start_explicit(ViewId(2), duplicate).is_ok());
}
#[test]
fn new_configuration_and_clear_command_roundtrip_without_changing_unit_output_strings() {
    assert_eq!(serde_json::to_string(&Output::Typed).unwrap(), "\"typed\"");
    let configuration = configuration(pose::Kind::Odometry);
    let c = Command::Start {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        request: RequestId(3),
        configuration,
    };
    assert_eq!(Command::decode(&c.encode().unwrap()).unwrap(), c);
    let clear = Command::ClearPose {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        request: RequestId(4),
    };
    assert_eq!(Command::decode(&clear.encode().unwrap()).unwrap(), clear);
    let owner = pose::Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        kind: pose::Kind::Odometry,
        topic: "/odom".into(),
        fixed_frame: "world".into(),
    };
    let mut event = Event::PoseCleared {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        request: RequestId(4),
        barrier: Box::new(pose::transport::Invalidation {
            contract_version: 1,
            owner,
            revision: 3,
            continuity_epoch: 2,
            reason: pose::ResetReason::LocalClear,
        }),
    };
    assert!(event.encode().is_ok());
    if let Event::PoseCleared {
        ref mut barrier, ..
    } = event
    {
        barrier.owner.subscription = SubscriptionId(8);
    }
    assert!(event.encode().is_err());
}
