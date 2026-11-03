use super::*;
use studio_protocol::{robot, spatial_input::SpatialClockPolicy};

fn config() -> Configuration {
    Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: "tf2_msgs/msg/TFMessage".into(),
        best_effort: false,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(
            robot::Configuration {
                fixed_frame: "map".into(),
                frames: vec!["base".into(), "arm".into()],
                clock_policy: SpatialClockPolicy::VerifiedSystem,
                dynamic_tf_topic: "/tf".into(),
                dynamic_tf_best_effort: false,
                static_tf_topic: "/tf_static".into(),
            }
            .into(),
        ),
    }
}
fn start(shared: &mut Shared, id: u64) -> StartIntent {
    let intent = shared
        .registry
        .start_explicit(ViewId(id), config())
        .unwrap();
    shared
        .queue
        .register(intent.generation, intent.subscription)
        .unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            subscription: intent.subscription,
            request: intent.request,
        })
        .unwrap();
    intent
}
fn sample(intent: &StartIntent, sequence: u64) -> Event {
    Event::RobotObservation {
        generation: intent.generation,
        subscription: intent.subscription,
        sequence,
        sample: Box::new(robot::Sample {
            contract_version: 1,
            fixed_frame: "map".into(),
            policy: robot::PosePolicy::LatestAvailable,
            links: ["base", "arm"]
                .into_iter()
                .map(|frame| robot::LinkResult {
                    frame: frame.into(),
                    pose: robot::LinkPose::Missing,
                })
                .collect(),
            counters: robot::Counters::default(),
        }),
        backend_drops: 0,
    }
}
fn drain(shared: &mut Shared) -> Vec<Event> {
    let mut result = Vec::new();
    while let Some(packet) = shared.queue.pop() {
        result.push(match packet {
            Packet::Control(t) | Packet::Diagnostic(t) | Packet::Sample { bytes: t, .. } => t.event,
        });
    }
    result
}
#[test]
fn robot_ingress_validates_exact_request_before_advancing_sequence_and_rejects_retired_owners() {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    let first = start(&mut shared, 1);
    let mut wrong = sample(&first, 1);
    if let Event::RobotObservation { sample, .. } = &mut wrong {
        sample.links.swap(0, 1);
    }
    shared.ingest_for_test(wrong).unwrap();
    assert_eq!(shared.registry.entries()[0].last_sequence, None);
    shared.ingest_for_test(sample(&first, 1)).unwrap();
    shared.ingest_for_test(sample(&first, 1)).unwrap();
    let packets = drain(&mut shared);
    assert_eq!(
        packets
            .iter()
            .filter(|e| matches!(e, Event::RobotObservation { .. }))
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
    let second = start(&mut shared, 1);
    shared.ingest_for_test(sample(&first, 3)).unwrap();
    shared.ingest_for_test(sample(&second, 1)).unwrap();
    let packets = drain(&mut shared);
    assert_eq!(packets.iter().filter(|e| matches!(e, Event::RobotObservation { subscription, .. } if *subscription==second.subscription)).count(), 1);
    assert!(!packets.iter().any(|e| matches!(e, Event::RobotObservation { subscription, .. } if *subscription==first.subscription)));
    assert_eq!(shared.ingress_rejections, 4);
}
#[test]
fn robot_flood_is_accounted_as_bounded_samples_and_preserves_reserved_controls() {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    let intent = start(&mut shared, 1);
    for sequence in 1..=100 {
        shared.ingest_for_test(sample(&intent, sequence)).unwrap();
    }
    let counters = shared.queue.stream_counters(intent.subscription).unwrap();
    assert_eq!(counters.evicted_samples, 84);
    let packets = drain(&mut shared);
    assert_eq!(
        packets
            .iter()
            .filter(|e| matches!(e, Event::RobotObservation { .. }))
            .count(),
        16
    );
    assert!(packets.iter().any(|e| matches!(e, Event::Started { .. })));
}
