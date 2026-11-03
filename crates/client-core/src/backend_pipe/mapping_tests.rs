use super::*;
use studio_protocol::{
    mapping,
    spatial::{Configuration as TfConfiguration, TfOutcome},
    spatial_input::SpatialClockPolicy,
};
use studio_visualization::spatial::mapping_live::{decode_mapping_sample, MappingStream};

fn owner() -> Shared {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    shared
}
fn start(shared: &mut Shared, id: u64) -> (StartIntent, MappingTarget) {
    let intent = shared
        .registry
        .start_explicit(
            ViewId(id),
            Configuration {
                map_updates: None,
                topic: format!("/mapping_{id}"),
                message_type: if id == 3 {
                    mapping::MAP_MESSAGE_TYPE
                } else if id == 1 {
                    mapping::POSE_MESSAGE_TYPE
                } else {
                    mapping::PATH_MESSAGE_TYPE
                }
                .into(),
                best_effort: false,
                transient_local: false,
                output: if id == 3 {
                    Output::OccupancyGrid
                } else if id == 1 {
                    Output::Pose
                } else {
                    Output::Path
                },
                spatial: Some(TfConfiguration {
                    fixed_frame: "map".into(),
                    clock_policy: SpatialClockPolicy::VerifiedSystem,
                    dynamic_tf_topic: "/tf".into(),
                    dynamic_tf_best_effort: false,
                    static_tf_topic: "/tf_static".into(),
                }),
                robot: None,
            },
        )
        .unwrap();
    shared
        .queue
        .register(intent.generation, intent.subscription)
        .unwrap();
    let stream = MappingStream::default();
    let target = MappingTarget {
        generation: intent.generation,
        subscription: intent.subscription,
        topic: intent.configuration.topic.clone(),
        message_type: intent.configuration.message_type.clone(),
        fixed_frame: "map".into(),
        local_generation: stream.start(),
        stream,
    };
    shared.bind_mapping(target.clone()).unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            subscription: intent.subscription,
            request: intent.request,
        })
        .unwrap();
    (intent, target)
}
fn sample(target: &MappingTarget) -> mapping::Sample {
    mapping::Sample {
        contract_version: 1,
        topic: target.topic.clone(),
        message_type: target.message_type.clone(),
        fixed_frame: "map".into(),
        source_frame: "sensor".into(),
        acquisition_stamp_ns: 123,
        geometry: if target.message_type == mapping::MAP_MESSAGE_TYPE {
            mapping::Geometry::OccupancyGrid {
                width: 1,
                height: 1,
                resolution: 1.,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
                cells: vec![0],
            }
        } else if target.message_type == mapping::POSE_MESSAGE_TYPE {
            mapping::Geometry::Pose {
                components: [1., 2., 3., 0., 0., 0., 1.],
            }
        } else {
            mapping::Geometry::Path {
                poses: vec![mapping::PathPose {
                    frame_id: "sensor".into(),
                    stamp_ns: 123,
                    components: [1., 2., 3., 0., 0., 0., 1.],
                }],
            }
        },
        tf: TfOutcome::Ready {
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
        counters: mapping::Counters::default(),
    }
}
#[test]
fn mapping_bad_map_cell_count_clears_only_map_and_retains_pose_path_peers() {
    let mut shared = owner();
    let (_, pose) = start(&mut shared, 1);
    let (_, path) = start(&mut shared, 2);
    let (_, map) = start(&mut shared, 3);
    for target in [&pose, &path, &map] {
        shared
            .ingest_for_test(event(target, 1, sample(target)))
            .unwrap();
    }
    let mut invalid = sample(&map);
    if let mapping::Geometry::OccupancyGrid { cells, .. } = &mut invalid.geometry {
        cells.push(100);
    }
    shared.ingest_for_test(event(&map, 2, invalid)).unwrap();
    assert!(map.stream.take(map.local_generation).is_none());
    assert!(map.stream.snapshot().unwrap().error.is_some());
    assert_eq!(shared.mapping_faults.len(), 1);
    assert_eq!(shared.mapping_faults[0].subscription, map.subscription);
    assert!(pose.stream.take(pose.local_generation).is_some());
    assert!(path.stream.take(path.local_generation).is_some());
    assert_eq!(shared.registry.entries().len(), 3);
}
fn event(target: &MappingTarget, sequence: u64, sample: mapping::Sample) -> Event {
    Event::MappingObservation {
        generation: target.generation,
        subscription: target.subscription,
        sequence,
        sample: Box::new(sample),
        backend_drops: 2,
    }
}
#[test]
fn mapping_wrong_identity_clears_only_fresh_owner_and_latches_bounded_stop_fault() {
    for field in 0..3 {
        let mut shared = owner();
        let (_, a) = start(&mut shared, 1);
        let (_, b) = start(&mut shared, 2);
        shared.ingest_for_test(event(&a, 1, sample(&a))).unwrap();
        shared.ingest_for_test(event(&b, 1, sample(&b))).unwrap();
        let mut wrong = sample(&a);
        match field {
            0 => wrong.topic = "/other".into(),
            1 => wrong.message_type = "sensor_msgs/msg/Image".into(),
            _ => wrong.fixed_frame = "odom".into(),
        }
        shared.ingest_for_test(event(&a, 2, wrong.clone())).unwrap();
        for n in 3..100 {
            shared.ingest_for_test(event(&a, n, wrong.clone())).unwrap();
        }
        assert!(a.stream.take(a.local_generation).is_none());
        assert!(a.stream.snapshot().unwrap().error.is_some());
        assert_eq!(shared.mapping_faults.len(), 1);
        assert_eq!(shared.mapping_faults[0].subscription, a.subscription);
        assert!(b.stream.take(b.local_generation).is_some());
        assert!(b.stream.snapshot().unwrap().error.is_none());
        assert_eq!(
            shared.registry.entries().len(),
            2,
            "Fault retains native cleanup reservation"
        );
    }
}
#[test]
fn mapping_stale_duplicate_and_stopping_identity_errors_do_not_clear_geometry() {
    let mut shared = owner();
    let (_, target) = start(&mut shared, 1);
    shared
        .ingest_for_test(event(&target, 2, sample(&target)))
        .unwrap();
    let mut wrong = sample(&target);
    wrong.fixed_frame = "odom".into();
    shared
        .ingest_for_test(event(&target, 2, wrong.clone()))
        .unwrap();
    let mut old = target.clone();
    old.generation = Generation(target.generation.0 + 1);
    shared
        .ingest_for_test(event(&old, 3, wrong.clone()))
        .unwrap();
    shared.registry.stop_explicit(target.subscription).unwrap();
    shared.ingest_for_test(event(&target, 3, wrong)).unwrap();
    assert!(target.stream.take(target.local_generation).is_some());
    assert!(target.stream.snapshot().unwrap().error.is_none());
    assert!(shared.mapping_faults.is_empty());
    assert_eq!(shared.ingress_rejections, 3);
}
#[test]
fn mapping_decoder_failure_is_scoped_and_cleanup_releases_target_and_fault() {
    let mut shared = owner();
    let (_, a) = start(&mut shared, 1);
    let (_, b) = start(&mut shared, 2);
    shared.ingest_for_test(event(&a, 1, sample(&a))).unwrap();
    let mut invalid = sample(&a);
    invalid.acquisition_stamp_ns = 0;
    // Deliberately malformed content passes the bounded envelope parser and is
    // handled by the same scoped worker decoder as the production reader.
    shared.ingest_for_test(event(&a, 2, invalid)).unwrap();
    assert!(a.stream.take(a.local_generation).is_none());
    assert!(a.stream.snapshot().unwrap().error.is_some());
    assert_eq!(shared.mapping_faults.len(), 1);
    shared.ingest_for_test(event(&b, 1, sample(&b))).unwrap();
    assert!(b.stream.take(b.local_generation).is_some());
    let stop = shared.registry.stop_explicit(a.subscription).unwrap();
    shared
        .ingest_for_test(Event::Stopped {
            generation: stop.generation,
            subscription: stop.subscription,
            request: stop.request,
        })
        .unwrap();
    assert!(shared.mapping_faults.is_empty());
    assert_eq!(shared.mappings.len(), 1);
    assert_eq!(shared.mappings[0].subscription, b.subscription);
}
#[test]
fn mapping_decode_racing_stop_or_reconnect_cannot_publish_frames_or_errors() {
    for reconnect in [false, true] {
        let mut shared = owner();
        let (_, target) = start(&mut shared, 1);
        let data = sample(&target);
        let prepared = mapping_pipe::prepare(
            &mut shared,
            target.generation,
            target.subscription,
            1,
            0,
            &data,
        )
        .unwrap();
        let decoded = decode_mapping_sample(&data);
        if reconnect {
            shared.registry.disconnect();
            target.stream.stop();
            shared
                .ingest_for_test(Event::ready("jazzy".into(), "fixture".into()))
                .unwrap();
        } else {
            shared.registry.stop_explicit(target.subscription).unwrap();
        }
        assert!(!mapping_pipe::publish(
            &mut shared,
            &prepared,
            1,
            0,
            decoded
        ));
        assert!(!mapping_pipe::publish(
            &mut shared,
            &prepared,
            2,
            0,
            Err(
                studio_visualization::spatial::mapping_live::MappingLiveError(
                    "late decoder error".into()
                )
            )
        ));
        assert!(target.stream.take(target.local_generation).is_none());
        assert!(target.stream.snapshot().is_none());
        assert!(shared.mapping_faults.is_empty());
    }
}
#[test]
fn mapping_raw_decoder_rejects_zero_envelope_but_scopes_invalid_sample_content() {
    let mut shared = owner();
    let (_, target) = start(&mut shared, 1);
    let mut invalid = sample(&target);
    invalid.tf = TfOutcome::Ready {
        components: [0.; 7],
    };
    let bytes = serde_json::to_vec(&event(&target, 1, invalid)).unwrap();
    assert!(decode_event(&bytes).is_ok());
    assert!(Event::decode(&bytes).is_err());
    let zero = serde_json::to_vec(&event(&target, 0, sample(&target))).unwrap();
    assert!(decode_event(&zero).is_err());
    assert!(decode_event(&vec![b' '; MAX_LINE + 1]).is_err());
}

#[test]
fn mapping_production_reader_survives_scoped_decode_failure_and_repaints_without_shared_lock() {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Reader {
        ready: Cursor<Vec<u8>>,
        rest: Option<Cursor<Vec<u8>>>,
        shared: Arc<Mutex<Shared>>,
        targets: Arc<Mutex<Vec<MappingTarget>>>,
    }
    impl Read for Reader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.ready.position() < self.ready.get_ref().len() as u64 {
                return self.ready.read(buffer);
            }
            if self.rest.is_none() {
                let mut shared = self.shared.lock().unwrap();
                let (_, a) = start(&mut shared, 1);
                let (_, b) = start(&mut shared, 2);
                self.targets.lock().unwrap().extend([a.clone(), b.clone()]);
                let mut invalid = sample(&a);
                invalid.acquisition_stamp_ns = 0;
                let mut bytes = serde_json::to_vec(&event(&a, 1, invalid)).unwrap();
                bytes.push(b'\n');
                bytes.extend(event(&b, 1, sample(&b)).encode().unwrap());
                bytes.extend(
                    Event::Graph {
                        topics: vec![("/still_alive".into(), vec![])],
                    }
                    .encode()
                    .unwrap(),
                );
                self.rest = Some(Cursor::new(bytes));
            }
            self.rest.as_mut().unwrap().read(buffer)
        }
    }
    let shared = Arc::new(Mutex::new(Shared::new(0)));
    let targets = Arc::new(Mutex::new(Vec::new()));
    let unlocked = Arc::new(AtomicBool::new(true));
    let observed = Arc::new(AtomicBool::new(false));
    let context = eframe::egui::Context::default();
    let observer = Arc::downgrade(&shared);
    let flag = unlocked.clone();
    let repaint = observed.clone();
    context.set_request_repaint_callback(move |_| {
        repaint.store(true, Ordering::Relaxed);
        if let Some(shared) = observer.upgrade() {
            if shared.try_lock().is_err() {
                flag.store(false, Ordering::Relaxed);
            }
        }
    });
    shared.lock().unwrap().repaint = Some(context);
    let terminal = pump(
        Reader {
            ready: Cursor::new(
                Event::ready("jazzy".into(), "fixture".into())
                    .encode()
                    .unwrap(),
            ),
            rest: None,
            shared: shared.clone(),
            targets: targets.clone(),
        },
        &shared,
    );
    assert!(terminal.contains("output closed"), "{terminal}");
    let targets = targets.lock().unwrap();
    assert!(targets[0].stream.snapshot().unwrap().error.is_some());
    assert!(targets[0]
        .stream
        .take(targets[0].local_generation)
        .is_none());
    assert!(targets[1]
        .stream
        .take(targets[1].local_generation)
        .is_some());
    let shared = shared.lock().unwrap();
    assert_eq!(shared.mapping_faults.len(), 1);
    assert_eq!(shared.graph.as_ref().unwrap()[0].0, "/still_alive");
    assert!(observed.load(Ordering::Relaxed));
    assert!(unlocked.load(Ordering::Relaxed));
}
