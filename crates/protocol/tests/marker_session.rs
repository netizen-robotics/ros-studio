use studio_protocol::{
    marker, marker_snapshot as wire,
    session::{Command, Event, VERSION},
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::*,
};
fn config(topic: &str) -> Configuration {
    Configuration {
        topic: topic.into(),
        message_type: marker::ARRAY_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: true,
        output: Output::MarkerSnapshot,
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
fn marker_admission_requires_explicit_type_verified_scene_and_two_owner_shared_tf_capacity() {
    let mut r = Registry::default();
    let g = r.connect().unwrap();
    let a = r.start_explicit(ViewId(1), config("/a")).unwrap();
    let b = r.start_explicit(ViewId(2), config("/b")).unwrap();
    assert_eq!(r.used_resources(), 4);
    assert_eq!(
        r.start_explicit(ViewId(3), config("/c")),
        Err(RegistryError::Capacity)
    );
    assert_eq!(
        r.clear_markers_explicit(a.subscription),
        Err(RegistryError::WrongState)
    );
    r.start_ack(g, a.subscription, a.request).unwrap();
    r.start_ack(g, b.subscription, b.request).unwrap();
    let mut c = config("/single");
    c.message_type = marker::MESSAGE_TYPE.into();
    assert!(c.valid());
    c.message_type = "std_msgs/msg/String".into();
    assert!(!c.valid());
    c = config("/a");
    c.spatial = None;
    assert!(!c.valid());
    for clock in [SpatialClockPolicy::Unknown, SpatialClockPolicy::Simulated] {
        let mut c = config("/a");
        c.spatial.as_mut().unwrap().clock_policy = clock;
        assert!(!c.valid());
    }
}
#[test]
fn clear_correlations_do_not_release_handles_or_replay_after_stop_and_reconnect() {
    let mut r = Registry::default();
    let g = r.connect().unwrap();
    let a = r.start_explicit(ViewId(1), config("/a")).unwrap();
    r.start_ack(g, a.subscription, a.request).unwrap();
    let clear = r.clear_markers_explicit(a.subscription).unwrap();
    assert!(clear.request.0 > a.request.0);
    assert_eq!(r.used_resources(), 3);
    assert!(r
        .clear_markers_ack(g, a.subscription, RequestId(clear.request.0 + 1))
        .is_err());
    assert!(r.clear_markers_explicit(a.subscription).is_err());
    r.clear_markers_ack(g, a.subscription, clear.request)
        .unwrap();
    assert_eq!(r.used_resources(), 3);
    assert_eq!(r.entries()[0].state, State::Active);
    let retry = r.clear_markers_explicit(a.subscription).unwrap();
    let stop = r.stop_explicit(a.subscription).unwrap();
    assert!(stop.request.0 > retry.request.0);
    assert!(r
        .clear_markers_ack(g, a.subscription, retry.request)
        .is_err());
    assert_eq!(r.used_resources(), 3);
    r.stop_ack(g, a.subscription, stop.request).unwrap();
    assert_eq!(r.used_resources(), 0);
    r.disconnect();
    r.connect().unwrap();
    assert!(r.entries().is_empty());
    assert!(r
        .clear_markers_ack(g, a.subscription, retry.request)
        .is_err());
}
#[test]
fn clear_on_other_outputs_and_malformed_clear_wire_never_becomes_a_native_release() {
    let mut r = Registry::default();
    let g = r.connect().unwrap();
    let mut image = config("/image");
    image.output = Output::CompactImage;
    image.message_type = studio_protocol::image::MESSAGE_TYPE.into();
    image.spatial = None;
    let a = r.start_explicit(ViewId(1), image).unwrap();
    r.start_ack(g, a.subscription, a.request).unwrap();
    assert_eq!(
        r.clear_markers_explicit(a.subscription),
        Err(RegistryError::WrongState)
    );
    assert_eq!(r.used_resources(), 1);
    let clear = Command::ClearMarkers {
        generation: g,
        request: RequestId(2),
        subscription: a.subscription,
    };
    assert_eq!(Command::decode(&clear.encode().unwrap()).unwrap(), clear);
    assert!(Command::decode(
        br#"{"command":"clear_markers","generation":1,"request":0,"subscription":1}"#
    )
    .is_err());
    let ack = Event::MarkersCleared {
        generation: g,
        request: RequestId(2),
        subscription: a.subscription,
    };
    assert!(Event::decode(&ack.encode().unwrap()).is_ok());
    assert_eq!(r.used_resources(), 1);
}
#[test]
fn strict_current_session_marker_capabilities_and_envelope_identity_are_checked() {
    assert_eq!(VERSION, 21);
    let ready = Event::ready("jazzy".into(), "synthetic".into());
    let json = serde_json::to_value(&ready).unwrap();
    assert_eq!(json["marker_snapshot"]["max_owners"], 2);
    for field in ["marker_snapshot", "version"] {
        let mut bad = json.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Event>(bad).is_err());
    }
    let mut old = json.clone();
    old["version"] = 13.into();
    assert!(serde_json::from_value::<Event>(old)
        .unwrap()
        .validate()
        .is_err());
    let mut bad = json;
    bad["marker_snapshot"]["max_markers"] = 257.into();
    assert!(serde_json::from_value::<Event>(bad)
        .unwrap()
        .validate()
        .is_err());
    let owner = wire::Owner {
        generation: Generation(999),
        subscription: SubscriptionId(1),
        topic: "/a".into(),
        message_type: marker::MESSAGE_TYPE.into(),
        fixed_frame: "world".into(),
    };
    let sample = wire::Message::Clear(Box::new(wire::Clear {
        contract_version: 1,
        owner,
        revision: 1,
        reason: wire::ClearReason::Waiting,
        known_history_gap: false,
    }));
    assert!(Event::MarkerSnapshotObservation {
        generation: Generation(1),
        subscription: SubscriptionId(1),
        sequence: 1,
        sample: Box::new(sample),
        backend_drops: 0
    }
    .validate()
    .is_err());
}
