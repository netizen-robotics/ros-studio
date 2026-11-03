use super::*;
#[test]
fn session21_large_images_are_bounded_coalesced_and_do_not_evict_peer_or_controls() {
    struct ImageBytes(usize, bool);
    impl Payload for ImageBytes {
        fn payload_bytes(&self) -> usize {
            self.0
        }
        fn is_image(&self) -> bool {
            self.1
        }
    }
    let generation = Generation(1);
    let mut q = FairQueue::with_payload(generation).with_image_budget();
    q.register(generation, SubscriptionId(1)).unwrap();
    q.register(generation, SubscriptionId(2)).unwrap();
    let size = crate::image::MAX_SAMPLE_BYTES;
    q.enqueue_sample(generation, SubscriptionId(2), 1, ImageBytes(100, false))
        .unwrap();
    for n in 1..=10 {
        q.enqueue_sample(generation, SubscriptionId(1), n, ImageBytes(size, true))
            .unwrap();
        assert_eq!(q.sample_bytes(), size + 100);
    }
    assert_eq!(
        q.stream_counters(SubscriptionId(1))
            .unwrap()
            .evicted_samples,
        9
    );
    assert_eq!(
        q.enqueue_sample(
            generation,
            SubscriptionId(1),
            11,
            ImageBytes(crate::session::MAX_EVENT_BYTES + 1, true)
        ),
        Err(QueueError::Oversized)
    );
    assert_eq!(
        q.enqueue_sample(generation, SubscriptionId(2), 2, ImageBytes(size, false)),
        Err(QueueError::Oversized)
    );
    let mut legacy = FairQueue::with_payload(generation);
    legacy.register(generation, SubscriptionId(1)).unwrap();
    assert_eq!(
        legacy.enqueue_sample(generation, SubscriptionId(1), 1, ImageBytes(size, true)),
        Err(QueueError::Oversized)
    );
}
fn config(topic: &str) -> Configuration {
    Configuration {
        map_updates: None,
        topic: topic.into(),
        message_type: "std_msgs/msg/String".into(),
        best_effort: false,
        transient_local: false,
        output: Output::Typed,
        spatial: None,
        robot: None,
    }
}
#[test]
fn snapshot_map_optional_reader_is_counted_with_shared_tf_through_stop() {
    let mut map = spatial_config();
    map.output = Output::MapSnapshot;
    map.topic = "/map".into();
    map.message_type = crate::mapping::MAP_MESSAGE_TYPE.into();
    map.best_effort = false;
    map.transient_local = true;
    assert!(map.valid());
    assert!(map.owns_tf());
    assert_eq!(resource_usage(&[&map]).unwrap().native_handles, 3);
    map.map_updates = Some(crate::map_grid::UpdateSource {
        topic: "/map_updates".into(),
        best_effort: false,
        transient_local: false,
    });
    assert!(map.valid());
    assert_eq!(resource_usage(&[&map]).unwrap().native_handles, 4);
    let peers: Vec<_> = (0..4).map(|i| config(&format!("/peer_{i}"))).collect();
    let mut all: Vec<_> = peers.iter().collect();
    all.push(&map);
    assert_eq!(resource_usage(&all).unwrap().native_handles, 8);
    let extra = config("/extra");
    all.push(&extra);
    assert!(resource_usage(&all).is_ok());
    let mut legacy = map.clone();
    legacy.output = Output::OccupancyGrid;
    legacy.map_updates = None;
    assert_eq!(resource_usage(&[&map, &legacy]).unwrap().native_handles, 5);
    let mut invalid = map.clone();
    invalid.map_updates.as_mut().unwrap().topic = invalid.topic.clone();
    assert!(!invalid.valid());
    let mut invalid = map.clone();
    invalid.output = Output::Pose;
    invalid.message_type = crate::mapping::POSE_MESSAGE_TYPE.into();
    assert!(!invalid.valid());
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let start = registry.start_explicit(ViewId(1), map).unwrap();
    registry.start_explicit(ViewId(2), legacy.clone()).unwrap();
    registry.start_explicit(ViewId(3), legacy.clone()).unwrap();
    let stop = registry.stop_explicit(start.subscription).unwrap();
    assert_eq!(registry.used_resources(), 6);
    assert_eq!(
        registry
            .start_explicit(ViewId(4), legacy.clone())
            .unwrap_err(),
        RegistryError::Capacity
    );
    registry
        .start_ack(start.generation, start.subscription, start.request)
        .unwrap();
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 4);
    assert!(registry.start_explicit(ViewId(4), legacy).is_ok());
}
#[test]
fn clearing_one_ordered_stream_preserves_peer_and_control_lanes() {
    let mut queue = FairQueue::new(Generation(1));
    queue.register(Generation(1), SubscriptionId(1)).unwrap();
    queue.register(Generation(1), SubscriptionId(2)).unwrap();
    queue
        .enqueue_sample(
            Generation(1),
            SubscriptionId(1),
            1,
            vec![1; 100].into_boxed_slice(),
        )
        .unwrap();
    queue
        .enqueue_control(Generation(1), vec![9].into_boxed_slice())
        .unwrap();
    queue
        .enqueue_sample(
            Generation(1),
            SubscriptionId(2),
            1,
            vec![2; 100].into_boxed_slice(),
        )
        .unwrap();
    assert!(queue
        .clear_samples(Generation(2), SubscriptionId(1))
        .is_err());
    assert_eq!(queue.queued_samples(), 2);
    queue
        .clear_samples(Generation(1), SubscriptionId(1))
        .unwrap();
    assert_eq!(queue.sample_bytes(), 100);
    assert_eq!(queue.stream_queued_samples(SubscriptionId(1)), Some(0));
    assert_eq!(queue.stream_queued_samples(SubscriptionId(2)), Some(1));
    assert_eq!(
        queue
            .stream_counters(SubscriptionId(1))
            .unwrap()
            .cleared_samples,
        1
    );
    assert_eq!(
        queue
            .stream_counters(SubscriptionId(2))
            .unwrap()
            .cleared_samples,
        0
    );
    let packets = [queue.pop().unwrap(), queue.pop().unwrap()];
    assert!(packets
        .iter()
        .any(|p| matches!(p,Packet::Control(bytes) if bytes.as_ref()==[9])));
    assert!(packets.iter().any(|p| matches!(
        p,
        Packet::Sample {
            subscription: SubscriptionId(2),
            ..
        }
    )));
}
#[test]
fn description_owns_one_native_reader_without_tf_and_reserves_through_stop() {
    let mut description = config("/robot_description");
    description.output = Output::RobotDescription;
    description.transient_local = true;
    assert!(description.valid());
    assert!(!description.owns_tf());
    assert!(description.scene_key().is_none());
    assert_eq!(resource_usage(&[&description]).unwrap().native_handles, 1);
    let mut wrong = description.clone();
    wrong.message_type = "example/msg/String".into();
    assert!(!wrong.valid());
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let first = registry
        .start_explicit(ViewId(1), description.clone())
        .unwrap();
    let peer = registry.start_explicit(ViewId(2), config("/peer")).unwrap();
    let stop = registry.stop_explicit(first.subscription).unwrap();
    assert!(registry
        .start_explicit(ViewId(3), description.clone())
        .is_err());
    assert_eq!(registry.used_resources(), 2);
    registry
        .start_ack(first.generation, first.subscription, first.request)
        .unwrap();
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 1);
    assert_eq!(registry.entries()[0].subscription, peer.subscription);
    assert!(registry.start_explicit(ViewId(3), description).is_ok());
}
fn spatial_config() -> Configuration {
    Configuration {
        map_updates: None,
        topic: "/scan".into(),
        message_type: crate::spatial_input::SCAN_MESSAGE_TYPE.into(),
        best_effort: true,
        transient_local: false,
        output: Output::SpatialScan,
        robot: None,
        spatial: Some(crate::spatial::Configuration {
            fixed_frame: "map".into(),
            clock_policy: crate::spatial_input::SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
    }
}
#[test]
fn shared_tf_reservation_survives_stopping_until_last_scene_owner_cleanup() {
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let peer = registry.start_explicit(ViewId(1), config("/peer")).unwrap();
    let first = registry
        .start_explicit(ViewId(2), spatial_config())
        .unwrap();
    let mut next = spatial_config();
    next.topic = "/second_scan".into();
    let second = registry.start_explicit(ViewId(3), next).unwrap();
    assert_eq!(registry.used_resources(), 5);
    assert_eq!(registry.available_resources(), MAX_NATIVE_HANDLES - 5);
    let stop = registry.stop_explicit(first.subscription).unwrap();
    assert_eq!(registry.used_resources(), 5);
    registry
        .start_ack(first.generation, first.subscription, first.request)
        .unwrap();
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 4);
    let stop = registry.stop_explicit(second.subscription).unwrap();
    assert_eq!(registry.used_resources(), 4);
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 1);
    assert_eq!(registry.entries()[0].subscription, peer.subscription);
    let fresh = registry
        .start_explicit(ViewId(2), spatial_config())
        .unwrap();
    registry.disconnect();
    registry.connect().unwrap();
    assert_eq!(registry.used_resources(), 0);
    assert!(registry.entries().is_empty());
    assert_eq!(
        registry.start_ack(fresh.generation, fresh.subscription, fresh.request),
        Err(RegistryError::StaleGeneration)
    );
}
#[test]
fn robot_multiple_scans_and_cloud_share_readers_but_keep_independent_caps() {
    let scan = spatial_config();
    let mut cloud = scan.clone();
    cloud.output = Output::PointCloud;
    cloud.topic = "/cloud".into();
    cloud.message_type = crate::cloud::MESSAGE_TYPE.into();
    let r = crate::robot::Configuration {
        fixed_frame: "map".into(),
        frames: vec!["base".into()],
        clock_policy: crate::spatial_input::SpatialClockPolicy::VerifiedSystem,
        dynamic_tf_topic: "/tf".into(),
        dynamic_tf_best_effort: false,
        static_tf_topic: "/tf_static".into(),
    };
    let robot = Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: crate::spatial_input::TF_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(Box::new(r)),
    };
    let initial = [&robot, &scan, &scan, &cloud];
    let usage = resource_usage(&initial).unwrap();
    assert_eq!(
        (
            usage.logical,
            usage.native_handles,
            usage.robots,
            usage.sensor_layers
        ),
        (4, 5, 1, 3)
    );
    let peer = config("/peer");
    assert_eq!(
        resource_usage(&[&robot, &scan, &scan, &cloud, &peer, &peer, &peer])
            .unwrap()
            .native_handles,
        8
    );
    assert!(resource_usage(&[&robot, &scan, &scan, &cloud, &peer, &peer, &peer, &peer]).is_ok());
    assert_eq!(
        resource_usage(&[&robot, &scan, &scan, &cloud, &cloud])
            .unwrap()
            .native_handles,
        6
    );
    assert!(resource_usage(&[&robot, &scan, &scan, &cloud, &cloud, &cloud]).is_err());
    let mut incompatible = cloud.clone();
    incompatible
        .spatial
        .as_mut()
        .unwrap()
        .dynamic_tf_best_effort = true;
    assert!(resource_usage(&[&robot, &incompatible]).is_err());
    assert!(resource_usage(&[&robot, &robot]).is_err());
}
#[test]
fn spatial_output_clock_types_frames_and_tf_role_combinations_are_explicit() {
    let good = spatial_config();
    assert!(good.valid());
    assert_eq!(good.resource_cost(), 1);
    assert_eq!(resource_usage(&[&good]).unwrap().native_handles, 3);
    for policy in [
        crate::spatial_input::SpatialClockPolicy::Unknown,
        crate::spatial_input::SpatialClockPolicy::Simulated,
    ] {
        let mut bad = good.clone();
        bad.spatial.as_mut().unwrap().clock_policy = policy;
        assert!(!bad.valid());
    }
    for mode in 0..6 {
        let mut bad = good.clone();
        match mode {
            0 => bad.spatial = None,
            1 => bad.output = Output::Typed,
            2 => bad.message_type = crate::image::MESSAGE_TYPE.into(),
            3 => bad.spatial.as_mut().unwrap().fixed_frame = "/map".into(),
            4 => bad.spatial.as_mut().unwrap().static_tf_topic = "/tf".into(),
            _ => bad.spatial.as_mut().unwrap().dynamic_tf_topic = "tf".into(),
        }
        assert!(!bad.valid());
    }
    let mut registry = Registry::default();
    registry.connect().unwrap();
    for id in 1..=MAX_SUBSCRIPTIONS as u64 {
        registry
            .start_explicit(ViewId(id), config("/peer"))
            .unwrap();
    }
    assert_eq!(
        registry
            .start_explicit(ViewId(MAX_SUBSCRIPTIONS as u64 + 1), good)
            .unwrap_err(),
        RegistryError::Capacity
    );
    assert_eq!(
        registry.entries().len(),
        MAX_SUBSCRIPTIONS,
        "No partial group reservation"
    );
}
#[test]
fn output_selection_is_explicit_and_compact_requires_exact_standard_image_type() {
    assert_eq!(Output::default(), Output::Typed);
    let mut configuration = config("/camera");
    configuration.message_type = crate::image::MESSAGE_TYPE.into();
    assert!(
        configuration.valid(),
        "Messages on an Image topic keep typed output"
    );
    configuration.output = Output::CompactImage;
    assert!(configuration.valid());
    assert_eq!(
        serde_json::to_value(&configuration).unwrap()["output"],
        "compact_image"
    );
    let mut registry = Registry::default();
    registry.connect().unwrap();
    for message_type in [
        "std_msgs/msg/String",
        "sensor_msgs/msg/CompressedImage",
        "custom_msgs/msg/Image",
    ] {
        configuration.message_type = message_type.into();
        assert!(!configuration.valid());
        assert_eq!(
            registry
                .start_explicit(ViewId(1), configuration.clone())
                .unwrap_err(),
            RegistryError::InvalidConfiguration
        );
        assert!(registry.entries().is_empty());
    }
    let mut wire = serde_json::to_value(config("/one")).unwrap();
    wire.as_object_mut().unwrap().remove("output");
    assert!(
        serde_json::from_value::<Configuration>(wire.clone()).is_err(),
        "Missing output cannot silently default on the wire"
    );
    wire["output"] = "image".into();
    assert!(serde_json::from_value::<Configuration>(wire).is_err());
}
fn running() -> (Registry, StartIntent) {
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let start = registry.start_explicit(ViewId(1), config("/one")).unwrap();
    registry
        .start_ack(start.generation, start.subscription, start.request)
        .unwrap();
    (registry, start)
}
fn bytes(len: usize) -> Box<[u8]> {
    vec![0; len].into_boxed_slice()
}
#[test]
fn sample_cannot_start_or_revive_a_view() {
    let mut r = Registry::default();
    let generation = r.connect().unwrap();
    let start = r.start_explicit(ViewId(1), config("/one")).unwrap();
    assert_eq!(
        r.sample(generation, start.subscription, 1),
        Err(RegistryError::WrongState)
    );
    let stop = r.stop_explicit(start.subscription).unwrap();
    assert_eq!(
        r.start_ack(generation, start.subscription, start.request),
        Ok(())
    );
    assert_eq!(r.entries()[0].state, State::Stopping);
    assert_eq!(
        r.sample(generation, start.subscription, 2),
        Err(RegistryError::WrongState)
    );
    assert_eq!(
        r.stop_ack(generation, start.subscription, stop.request),
        Ok(ViewId(1))
    );
    assert_eq!(
        r.start_ack(generation, start.subscription, start.request),
        Err(RegistryError::UnknownSubscription)
    );
    assert_eq!(
        r.sample(generation, start.subscription, 3),
        Err(RegistryError::UnknownSubscription)
    );
}
#[test]
fn cap_includes_starting_active_and_stopping_and_no_implicit_replacement() {
    let mut r = Registry::default();
    r.connect().unwrap();
    let mut starts = Vec::new();
    for view in 1..=MAX_SUBSCRIPTIONS as u64 {
        starts.push(r.start_explicit(ViewId(view), config("/topic")).unwrap());
    }
    assert_eq!(
        r.start_explicit(ViewId(MAX_SUBSCRIPTIONS as u64 + 1), config("/excess")),
        Err(RegistryError::Capacity)
    );
    let first = &starts[0];
    r.start_ack(first.generation, first.subscription, first.request)
        .unwrap();
    assert_eq!(
        r.start_explicit(ViewId(1), config("/replacement")),
        Err(RegistryError::ViewAlreadyPresent)
    );
    let stop = r.stop_explicit(first.subscription).unwrap();
    assert_eq!(
        r.start_explicit(ViewId(MAX_SUBSCRIPTIONS as u64 + 1), config("/excess")),
        Err(RegistryError::Capacity)
    );
    r.stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    let next = r.start_explicit(ViewId(1), config("/replacement")).unwrap();
    assert_ne!(next.subscription, first.subscription);
    assert!(next.request.0 > stop.request.0);
    assert_eq!(r.entries()[0].subscription, starts[1].subscription);
}
#[test]
fn mismatched_duplicate_and_reordered_events_preserve_state() {
    let (mut r, start) = running();
    assert_eq!(
        r.start_ack(start.generation, start.subscription, start.request),
        Err(RegistryError::WrongState)
    );
    assert_eq!(
        r.sample(start.generation, start.subscription, 8),
        Ok(start.view)
    );
    for sequence in [8, 7, 0] {
        assert_eq!(
            r.sample(start.generation, start.subscription, sequence),
            Err(RegistryError::OldSequence)
        );
    }
    assert_eq!(
        r.sample(start.generation, start.subscription, 9),
        Ok(start.view)
    );
    let stop = r.stop_explicit(start.subscription).unwrap();
    assert_eq!(
        r.stop_ack(stop.generation, stop.subscription, RequestId(900)),
        Err(RegistryError::WrongRequest)
    );
    assert_eq!(r.entries()[0].state, State::Stopping);
    r.stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(
        r.stop_ack(stop.generation, stop.subscription, stop.request),
        Err(RegistryError::UnknownSubscription)
    );
}
#[test]
fn disconnect_reconnect_invalidates_every_old_event_without_replay() {
    let (mut r, old) = running();
    let stop = r.stop_explicit(old.subscription).unwrap();
    r.disconnect();
    assert!(r.entries().is_empty());
    assert_eq!(
        r.sample(old.generation, old.subscription, 1),
        Err(RegistryError::Disconnected)
    );
    let generation = r.connect().unwrap();
    assert_ne!(generation, old.generation);
    assert!(r.entries().is_empty());
    assert_eq!(
        r.start_ack(old.generation, old.subscription, old.request),
        Err(RegistryError::StaleGeneration)
    );
    assert_eq!(
        r.stop_ack(old.generation, old.subscription, stop.request),
        Err(RegistryError::StaleGeneration)
    );
    let next = r.start_explicit(old.view, config("/one")).unwrap();
    assert_ne!(next.subscription, old.subscription);
    assert_ne!(next.request, old.request);
    assert_eq!(
        r.sample(generation, old.subscription, 2),
        Err(RegistryError::UnknownSubscription)
    );
    assert_eq!(r.entries()[0].state, State::Starting);
}
#[test]
fn scoped_start_error_and_failed_stop_need_explicit_retry() {
    let mut r = Registry::default();
    let generation = r.connect().unwrap();
    let bad = r.start_explicit(ViewId(1), config("/bad")).unwrap();
    let good = r.start_explicit(ViewId(2), config("/good")).unwrap();
    r.start_ack(generation, good.subscription, good.request)
        .unwrap();
    assert_eq!(
        r.request_error(generation, bad.subscription, RequestId(99)),
        Err(RegistryError::WrongRequest)
    );
    r.request_error(generation, bad.subscription, bad.request)
        .unwrap();
    assert_eq!(r.sample(generation, good.subscription, 1), Ok(good.view));
    let stop = r.stop_explicit(good.subscription).unwrap();
    r.request_error(generation, good.subscription, stop.request)
        .unwrap();
    assert_eq!(
        r.sample(generation, good.subscription, 2),
        Err(RegistryError::WrongState)
    );
    let retry = r.stop_explicit(good.subscription).unwrap();
    assert_ne!(retry.request, stop.request);
    assert_eq!(
        r.stop_ack(generation, good.subscription, stop.request),
        Err(RegistryError::WrongRequest)
    );
    r.stop_ack(generation, good.subscription, retry.request)
        .unwrap();
    assert!(r.entries().is_empty());
}
#[test]
fn rejects_invalid_configuration_and_ids_without_entries() {
    let mut r = Registry::default();
    assert_eq!(
        r.start_explicit(ViewId(1), config("/one")),
        Err(RegistryError::Disconnected)
    );
    r.connect().unwrap();
    assert_eq!(
        r.start_explicit(ViewId(0), config("/one")),
        Err(RegistryError::InvalidId)
    );
    for topic in ["", "relative", "/", "/a//b", "/9bad", "/bad\n"] {
        assert_eq!(
            r.start_explicit(ViewId(1), config(topic)),
            Err(RegistryError::InvalidConfiguration)
        );
    }
    let mut bad = config("/one");
    bad.message_type = "foo/srv/Bar".into();
    assert_eq!(
        r.start_explicit(ViewId(1), bad),
        Err(RegistryError::InvalidConfiguration)
    );
    let mut oversized = config("/one");
    oversized.message_type = "/".repeat(257);
    assert_eq!(
        r.start_explicit(ViewId(1), oversized),
        Err(RegistryError::InvalidConfiguration)
    );
    assert!(r.entries().is_empty());
    assert_eq!(
        r.stop_explicit(SubscriptionId(0)),
        Err(RegistryError::InvalidId)
    );
}
#[test]
fn four_independent_streams_route_only_to_their_view() {
    let mut r = Registry::default();
    let generation = r.connect().unwrap();
    let starts: Vec<_> = (1..=4)
        .map(|view| {
            r.start_explicit(ViewId(view), config(&format!("/topic{view}")))
                .unwrap()
        })
        .collect();
    for start in &starts {
        r.start_ack(generation, start.subscription, start.request)
            .unwrap();
    }
    for sequence in 0..100 {
        for start in &starts {
            assert_eq!(
                r.sample(generation, start.subscription, sequence),
                Ok(start.view)
            );
        }
    }
    let stop = r.stop_explicit(starts[1].subscription).unwrap();
    r.stop_ack(generation, stop.subscription, stop.request)
        .unwrap();
    for start in [&starts[0], &starts[2], &starts[3]] {
        assert_eq!(
            r.sample(generation, start.subscription, 100),
            Ok(start.view)
        );
    }
}
#[test]
fn queue_count_flood_evicts_only_its_own_oldest_samples() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    for id in [1, 2] {
        q.register(generation, SubscriptionId(id)).unwrap();
    }
    q.enqueue_sample(generation, SubscriptionId(2), 999, bytes(1))
        .unwrap();
    for sequence in 0..1000 {
        q.enqueue_sample(generation, SubscriptionId(1), sequence, bytes(2))
            .unwrap();
    }
    assert_eq!(q.queued_samples(), PER_STREAM_SAMPLES + 1);
    assert_eq!(
        q.counters().evicted_samples,
        1000 - PER_STREAM_SAMPLES as u64
    );
    assert_eq!(
        q.stream_counters(SubscriptionId(2))
            .unwrap()
            .evicted_samples,
        0
    );
    assert!(matches!(
        q.pop(),
        Some(Packet::Sample {
            subscription: SubscriptionId(1),
            sequence: 984,
            ..
        })
    ));
    assert!(matches!(
        q.pop(),
        Some(Packet::Sample {
            subscription: SubscriptionId(2),
            sequence: 999,
            ..
        })
    ));
}
#[test]
fn byte_flood_keeps_latest_and_budget_for_all32_late_low_rate_streams() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    q.register(generation, SubscriptionId(1)).unwrap();
    for sequence in 0..20 {
        q.enqueue_sample(
            generation,
            SubscriptionId(1),
            sequence,
            bytes(MAX_SAMPLE_BYTES),
        )
        .unwrap();
    }
    for id in 2..=MAX_SUBSCRIPTIONS as u64 {
        q.register(generation, SubscriptionId(id)).unwrap();
        q.enqueue_sample(generation, SubscriptionId(id), 0, bytes(MAX_SAMPLE_BYTES))
            .unwrap();
    }
    assert_eq!(q.sample_bytes(), AGGREGATE_SAMPLE_BYTES);
    assert_eq!(q.counters().evicted_samples, 19);
    assert_eq!(q.counters().rejected_samples, 0);
    for id in 1..=MAX_SUBSCRIPTIONS as u64 {
        assert!(
            matches!(q.pop(), Some(Packet::Sample {subscription, sequence, ..})
            if subscription == SubscriptionId(id) && sequence == if id == 1 {19} else {0})
        );
    }
}
#[test]
fn reserved_lanes_progress_during_flood_and_yield_to_samples() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    q.register(generation, SubscriptionId(1)).unwrap();
    for sequence in 0..PER_STREAM_SAMPLES as u64 {
        q.enqueue_sample(generation, SubscriptionId(1), sequence, bytes(1))
            .unwrap();
    }
    for _ in 0..RESERVED_ITEMS {
        q.enqueue_control(generation, bytes(1)).unwrap();
        q.enqueue_diagnostic(generation, bytes(1)).unwrap();
    }
    for sequence in 0..PER_STREAM_SAMPLES as u64 {
        assert!(matches!(q.pop(), Some(Packet::Control(_))));
        assert!(matches!(q.pop(), Some(Packet::Diagnostic(_))));
        assert!(
            matches!(q.pop(), Some(Packet::Sample { sequence: actual, .. }) if actual == sequence)
        );
    }
    for _ in PER_STREAM_SAMPLES..RESERVED_ITEMS {
        assert!(matches!(q.pop(), Some(Packet::Control(_))));
        assert!(matches!(q.pop(), Some(Packet::Diagnostic(_))));
    }
    assert!(q.pop().is_none());
    assert_eq!(q.sample_bytes(), 0);
}
#[test]
fn reserved_overflow_returns_payload_with_visible_counters() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    for _ in 0..RESERVED_ITEMS {
        q.enqueue_control(generation, bytes(1)).unwrap();
    }
    let original = vec![5, 6].into_boxed_slice();
    assert_eq!(
        q.enqueue_control(generation, original.clone()),
        Err((QueueError::Full, original))
    );
    assert_eq!(
        q.enqueue_diagnostic(generation, bytes(MAX_RESERVED_BYTES + 1))
            .unwrap_err()
            .0,
        QueueError::Oversized
    );
    assert_eq!(q.counters().rejected_controls, 1);
    assert_eq!(q.counters().rejected_diagnostics, 1);
}
#[test]
fn reset_unregister_and_stale_lanes_cannot_deliver_previous_session() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    q.register(generation, SubscriptionId(1)).unwrap();
    q.enqueue_sample(generation, SubscriptionId(1), 0, bytes(5))
        .unwrap();
    q.enqueue_control(generation, bytes(1)).unwrap();
    q.enqueue_diagnostic(generation, bytes(1)).unwrap();
    q.reset(Generation(2)).unwrap();
    assert!(q.pop().is_none());
    assert_eq!(q.sample_bytes(), 0);
    assert_eq!(q.counters().cleared_samples, 1);
    assert_eq!(
        q.enqueue_sample(generation, SubscriptionId(1), 1, bytes(1)),
        Err(QueueError::StaleGeneration)
    );
    assert_eq!(
        q.enqueue_control(generation, bytes(1)).unwrap_err().0,
        QueueError::StaleGeneration
    );
    assert_eq!(
        q.enqueue_diagnostic(generation, bytes(1)).unwrap_err().0,
        QueueError::StaleGeneration
    );
    q.register(Generation(2), SubscriptionId(2)).unwrap();
    q.enqueue_sample(Generation(2), SubscriptionId(2), 1, bytes(7))
        .unwrap();
    q.unregister(SubscriptionId(2)).unwrap();
    assert!(q.pop().is_none());
    assert_eq!(q.sample_bytes(), 0);
    assert_eq!(q.counters().cleared_samples, 2);
}
#[test]
fn queue_rejects_invalid_oversized_unknown_and_ninth_stream() {
    let generation = Generation(1);
    let mut q = FairQueue::new(generation);
    assert_eq!(
        q.register(generation, SubscriptionId(0)),
        Err(QueueError::InvalidId)
    );
    for id in 1..=MAX_SUBSCRIPTIONS as u64 {
        q.register(generation, SubscriptionId(id)).unwrap();
    }
    assert_eq!(
        q.register(generation, SubscriptionId(1)),
        Err(QueueError::DuplicateStream)
    );
    assert_eq!(
        q.register(generation, SubscriptionId(MAX_SUBSCRIPTIONS as u64 + 1)),
        Err(QueueError::Capacity)
    );
    assert_eq!(
        q.enqueue_sample(
            generation,
            SubscriptionId(MAX_SUBSCRIPTIONS as u64 + 1),
            1,
            bytes(1)
        ),
        Err(QueueError::UnknownStream)
    );
    assert_eq!(
        q.enqueue_sample(
            generation,
            SubscriptionId(1),
            1,
            bytes(MAX_SAMPLE_BYTES + 1)
        ),
        Err(QueueError::Oversized)
    );
    assert_eq!(
        q.counters().rejected_sample_bytes,
        (MAX_SAMPLE_BYTES + 2) as u64
    );
    assert_eq!(q.queued_samples(), 0);
}
#[test]
fn byte_limit_eviction_retains_latest_data_and_reports_drops() {
    let generation = Generation(1);
    let mut queue = FairQueue::new(generation);
    queue.register(generation, SubscriptionId(1)).unwrap();
    for sequence in 0..20 {
        queue
            .enqueue_sample(
                generation,
                SubscriptionId(1),
                sequence,
                bytes(MAX_SAMPLE_BYTES),
            )
            .unwrap();
    }
    assert_eq!(queue.sample_bytes(), MAX_SAMPLE_BYTES);
    assert_eq!(queue.queued_samples(), 1);
    assert_eq!(
        queue
            .stream_counters(SubscriptionId(1))
            .unwrap()
            .evicted_samples,
        19
    );
    for expected in 19..20 {
        assert!(
            matches!(queue.pop(), Some(Packet::Sample { sequence, .. }) if sequence == expected)
        );
    }
    assert_eq!(queue.sample_bytes(), 0);
}
#[test]
fn all_event_orderings_leave_stop_irreversible() {
    let mut cases = 0;
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    let mut ordering = [a, b, c, d];
                    ordering.sort();
                    if ordering != [0, 1, 2, 3] {
                        continue;
                    }
                    let mut registry = Registry::default();
                    let generation = registry.connect().unwrap();
                    let start = registry.start_explicit(ViewId(1), config("/one")).unwrap();
                    let mut stop = None;
                    let mut stopped = false;
                    for event in [a, b, c, d] {
                        match event {
                            0 => {
                                let _ = registry.start_ack(
                                    generation,
                                    start.subscription,
                                    start.request,
                                );
                            }
                            1 => {
                                stop = Some(registry.stop_explicit(start.subscription).unwrap());
                                stopped = true;
                            }
                            2 => {
                                let _ = registry.stop_ack(
                                    generation,
                                    start.subscription,
                                    stop.map(|s| s.request).unwrap_or(RequestId(999)),
                                );
                            }
                            3 => {
                                let result = registry.sample(generation, start.subscription, 1);
                                if stopped {
                                    assert!(result.is_err());
                                }
                            }
                            _ => unreachable!(),
                        }
                        if stopped {
                            assert!(registry
                                .entries()
                                .iter()
                                .all(|entry| entry.state != State::Active));
                        }
                    }
                    assert!(registry
                        .start_ack(generation, start.subscription, start.request)
                        .is_err());
                    assert!(registry.sample(generation, start.subscription, 2).is_err());
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 24);
}
#[test]
fn stale_reset_cannot_clear_or_reopen_queue_generation() {
    let generation = Generation(5);
    let mut queue = FairQueue::new(generation);
    queue.register(generation, SubscriptionId(1)).unwrap();
    queue
        .enqueue_sample(generation, SubscriptionId(1), 1, bytes(5))
        .unwrap();
    assert_eq!(queue.reset(Generation(4)), Err(QueueError::StaleGeneration));
    assert_eq!(queue.reset(generation), Err(QueueError::StaleGeneration));
    assert_eq!(queue.queued_samples(), 1);
    assert!(matches!(
        queue.pop(),
        Some(Packet::Sample {
            generation: Generation(5),
            ..
        })
    ));
}
#[test]
fn definitive_start_failure_after_cancellation_releases_only_its_slot() {
    let (mut registry, active) = running();
    let canceled = registry
        .start_explicit(ViewId(2), config("/canceled"))
        .unwrap();
    let stop = registry.stop_explicit(canceled.subscription).unwrap();
    assert_eq!(
        registry.request_error(canceled.generation, canceled.subscription, RequestId(999)),
        Err(RegistryError::WrongRequest)
    );
    assert_eq!(registry.entries().len(), 2);
    assert_eq!(
        registry.request_error(canceled.generation, canceled.subscription, canceled.request),
        Ok(canceled.view)
    );
    assert_eq!(registry.entries().len(), 1);
    assert_eq!(
        registry.sample(active.generation, active.subscription, 1),
        Ok(active.view)
    );
    assert_eq!(
        registry.stop_ack(stop.generation, stop.subscription, stop.request),
        Err(RegistryError::UnknownSubscription)
    );
    assert_eq!(
        registry.start_ack(canceled.generation, canceled.subscription, canceled.request),
        Err(RegistryError::UnknownSubscription)
    );
    let replacement = registry
        .start_explicit(canceled.view, config("/replacement"))
        .unwrap();
    assert_ne!(replacement.subscription, canceled.subscription);
    assert_eq!(
        registry.sample(replacement.generation, canceled.subscription, 2),
        Err(RegistryError::UnknownSubscription)
    );
}
#[test]
fn contradictory_start_failure_after_ack_does_not_remove_stopping_entry() {
    let (mut registry, active) = running();
    let stop = registry.stop_explicit(active.subscription).unwrap();
    assert_eq!(
        registry.request_error(active.generation, active.subscription, active.request),
        Err(RegistryError::WrongRequest)
    );
    assert_eq!(registry.entries()[0].state, State::Stopping);
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
}

#[test]
fn late_start_success_during_cancel_cannot_be_contradicted_or_revive_stream() {
    let mut r = Registry::with_prior_generation(41);
    let g = r.connect().unwrap();
    assert_eq!(g, Generation(42));
    let start = r.start_explicit(ViewId(1), config("/one")).unwrap();
    let peer = r.start_explicit(ViewId(2), config("/two")).unwrap();
    let stop = r.stop_explicit(start.subscription).unwrap();
    r.start_ack(g, start.subscription, start.request).unwrap();
    assert_eq!(r.entries()[0].state, State::Stopping);
    assert_eq!(
        r.request_error(g, start.subscription, start.request),
        Err(RegistryError::WrongRequest)
    );
    assert_eq!(
        r.sample(g, start.subscription, 1),
        Err(RegistryError::WrongState)
    );
    assert_eq!(
        r.stop_ack(g, start.subscription, stop.request),
        Ok(ViewId(1))
    );
    r.start_ack(g, peer.subscription, peer.request).unwrap();
    assert_eq!(r.sample(g, peer.subscription, 1), Ok(ViewId(2)));
    assert_eq!(
        r.stream_failed(Generation(41), peer.subscription),
        Err(RegistryError::StaleGeneration)
    );
    assert_eq!(r.stream_failed(g, peer.subscription), Ok(ViewId(2)));
    assert!(r.entries().is_empty());
}

#[test]
fn three_maps_with_updates_share_tf_and_report_map_and_reader_limits_separately() {
    let maps: Vec<_> = (0..4)
        .map(|i| {
            let mut map = spatial_config();
            map.output = Output::MapSnapshot;
            map.topic = format!("/map_{i}");
            map.message_type = crate::mapping::MAP_MESSAGE_TYPE.into();
            map.map_updates = Some(crate::map_grid::UpdateSource {
                topic: format!("/map_{i}_updates"),
                best_effort: false,
                transient_local: false,
            });
            map
        })
        .collect();
    let usage = resource_usage(&maps[..3].iter().collect::<Vec<_>>()).unwrap();
    assert_eq!((usage.maps, usage.logical, usage.native_handles), (3, 3, 8));
    assert!(resource_usage(&maps.iter().collect::<Vec<_>>())
        .unwrap_err()
        .contains("Map display limit"));
    let peer = config("/peer");
    assert_eq!(
        resource_usage(&[&maps[0], &maps[1], &maps[2], &peer])
            .unwrap()
            .native_handles,
        9
    );
    let mut registry = Registry::default();
    registry.connect().unwrap();
    let starts: Vec<_> = maps[..3]
        .iter()
        .enumerate()
        .map(|(i, map)| {
            registry
                .start_explicit(ViewId(i as u64 + 1), map.clone())
                .unwrap()
        })
        .collect();
    let stop = registry.stop_explicit(starts[1].subscription).unwrap();
    assert_eq!(registry.used_resources(), 8);
    assert_eq!(
        registry
            .start_explicit(ViewId(4), maps[3].clone())
            .unwrap_err(),
        RegistryError::Capacity
    );
    registry
        .start_ack(
            starts[1].generation,
            starts[1].subscription,
            starts[1].request,
        )
        .unwrap();
    registry
        .stop_ack(stop.generation, stop.subscription, stop.request)
        .unwrap();
    assert_eq!(registry.used_resources(), 6);
    assert!(registry.start_explicit(ViewId(4), maps[3].clone()).is_ok());
    registry.disconnect();
    registry.connect().unwrap();
    assert!(registry.entries().is_empty());
}
