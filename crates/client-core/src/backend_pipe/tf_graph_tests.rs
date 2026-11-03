use super::*;
use studio_protocol::{spatial, spatial_input::SpatialClockPolicy, tf_graph};
fn config() -> Configuration {
    Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        best_effort: false,
        transient_local: false,
        output: Output::TfGraph,
        robot: None,
        spatial: Some(spatial::Configuration {
            fixed_frame: "map".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            static_tf_topic: "/tf_static".into(),
            dynamic_tf_best_effort: false,
        }),
    }
}
fn start(shared: &mut Shared) -> StartIntent {
    let intent = shared.registry.start_explicit(ViewId(1), config()).unwrap();
    shared
        .queue
        .register(intent.generation, intent.subscription)
        .unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
        })
        .unwrap();
    intent
}
fn sample(intent: &StartIntent, sequence: u64) -> Event {
    Event::TfGraphObservation {
        generation: intent.generation,
        subscription: intent.subscription,
        sequence,
        sample: Box::new(tf_graph::Sample {
            contract_version: tf_graph::CONTRACT_VERSION,
            fixed_frame: "map".into(),
            frames: vec![],
            counters: spatial::Counters::default(),
        }),
        backend_drops: 0,
    }
}
fn drain(shared: &mut Shared) -> Vec<Event> {
    let mut events = vec![];
    while let Some(packet) = shared.queue.pop() {
        events.push(match packet {
            Packet::Sample { bytes, .. } | Packet::Control(bytes) | Packet::Diagnostic(bytes) => {
                bytes.event
            }
        });
    }
    events
}
#[test]
fn graph_owner_frame_sequence_stop_and_reconnect_are_checked_before_delivery() {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    let first = start(&mut shared);
    let mut wrong = sample(&first, 1);
    if let Event::TfGraphObservation { sample, .. } = &mut wrong {
        sample.fixed_frame = "other".into();
    }
    shared.ingest_for_test(wrong).unwrap();
    assert_eq!(shared.registry.entries()[0].last_sequence, None);
    shared.ingest_for_test(sample(&first, 1)).unwrap();
    shared.ingest_for_test(sample(&first, 1)).unwrap();
    assert_eq!(
        drain(&mut shared)
            .iter()
            .filter(|e| matches!(e, Event::TfGraphObservation { .. }))
            .count(),
        1
    );
    let stop = shared.registry.stop_explicit(first.subscription).unwrap();
    shared.ingest_for_test(sample(&first, 2)).unwrap();
    shared
        .ingest_for_test(Event::Stopped {
            generation: stop.generation,
            request: stop.request,
            subscription: stop.subscription,
        })
        .unwrap();
    let second = start(&mut shared);
    shared.ingest_for_test(sample(&first, 3)).unwrap();
    shared.ingest_for_test(sample(&second, 1)).unwrap();
    let events = drain(&mut shared);
    assert_eq!(events.iter().filter(|e|matches!(e,Event::TfGraphObservation{subscription,..} if *subscription==second.subscription)).count(),1);
    assert_eq!(shared.ingress_rejections, 4);
    shared.registry.disconnect();
    shared.ingest_for_test(sample(&second, 2)).unwrap();
    assert!(!drain(&mut shared)
        .iter()
        .any(|e| matches!(e, Event::TfGraphObservation { .. })));
}
#[test]
fn graph_flood_is_bounded_and_cannot_replace_cleanup_controls() {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    let intent = start(&mut shared);
    for sequence in 1..=100 {
        shared.ingest_for_test(sample(&intent, sequence)).unwrap();
    }
    assert_eq!(
        shared
            .queue
            .stream_counters(intent.subscription)
            .unwrap()
            .evicted_samples,
        84
    );
    let stop = shared.registry.stop_explicit(intent.subscription).unwrap();
    shared
        .ingest_for_test(Event::Stopped {
            generation: stop.generation,
            request: stop.request,
            subscription: stop.subscription,
        })
        .unwrap();
    let events = drain(&mut shared);
    assert!(events.iter().any(|e| matches!(e, Event::Stopped { .. })));
    assert!(shared.registry.entries().is_empty());
}
