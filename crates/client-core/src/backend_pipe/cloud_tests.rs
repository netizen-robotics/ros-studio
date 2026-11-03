use super::*;
use studio_protocol::{
    cloud,
    spatial::{Configuration as TfConfiguration, TfOutcome},
    spatial_input::SpatialClockPolicy,
};
use studio_visualization::spatial::cloud_live::{decode_cloud_sample, CloudStream};

fn owner() -> Shared {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    shared
}
fn start(shared: &mut Shared, id: u64) -> (StartIntent, CloudTarget) {
    let intent = shared
        .registry
        .start_explicit(
            ViewId(id),
            Configuration {
                map_updates: None,
                topic: format!("/cloud_{id}"),
                message_type: cloud::MESSAGE_TYPE.into(),
                best_effort: false,
                transient_local: false,
                output: Output::PointCloud,
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
    let stream = CloudStream::default();
    let target = CloudTarget {
        generation: intent.generation,
        subscription: intent.subscription,
        topic: intent.configuration.topic.clone(),
        fixed_frame: "map".into(),
        local_generation: stream.start(),
        stream,
    };
    shared.bind_cloud(target.clone()).unwrap();
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            subscription: intent.subscription,
            request: intent.request,
        })
        .unwrap();
    (intent, target)
}
fn sample(target: &CloudTarget) -> cloud::Sample {
    cloud::Sample {
        contract_version: cloud::CONTRACT_VERSION,
        topic: target.topic.clone(),
        message_type: cloud::MESSAGE_TYPE.into(),
        fixed_frame: "map".into(),
        source_frame: "sensor".into(),
        acquisition_stamp_ns: 123,
        source_points: 1,
        skipped_points: 0,
        points: vec![[1., 2., 3.]],
        channels: Default::default(),
        tf: TfOutcome::Ready {
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
        counters: cloud::Counters::default(),
    }
}
fn event(target: &CloudTarget, sequence: u64, sample: cloud::Sample) -> Event {
    Event::PointCloudObservation {
        generation: target.generation,
        subscription: target.subscription,
        sequence,
        sample: Box::new(sample),
        backend_drops: 2,
    }
}
#[test]
fn cloud_wrong_identity_clears_only_fresh_owner_and_latches_bounded_stop_fault() {
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
        assert_eq!(shared.cloud_faults.len(), 1);
        assert_eq!(shared.cloud_faults[0].subscription, a.subscription);
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
fn cloud_stale_duplicate_and_stopping_identity_errors_do_not_clear_geometry() {
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
    assert!(shared.cloud_faults.is_empty());
    assert_eq!(shared.ingress_rejections, 3);
}
#[test]
fn cloud_attributes_reach_the_bound_owner_and_malformed_alignment_is_scoped() {
    let mut shared = owner();
    let (_, a) = start(&mut shared, 1);
    let (_, b) = start(&mut shared, 2);
    let mut attributed = sample(&a);
    attributed.channels = cloud::Channels {
        color: cloud::Channel::Values(vec![[1, 2, 3, 4]]),
        intensity: cloud::Channel::Values(vec![None]),
    };
    shared
        .ingest_for_test(event(&a, 1, attributed.clone()))
        .unwrap();
    assert_eq!(
        a.stream.take(a.local_generation).unwrap().channels,
        attributed.channels
    );
    attributed.channels.color = cloud::Channel::Values(vec![]);
    shared.ingest_for_test(event(&a, 2, attributed)).unwrap();
    assert!(a.stream.take(a.local_generation).is_none());
    assert!(a.stream.snapshot().unwrap().error.is_some());
    shared.ingest_for_test(event(&b, 1, sample(&b))).unwrap();
    assert!(b.stream.take(b.local_generation).is_some());
    assert_eq!(shared.cloud_faults.len(), 1);
    assert_eq!(shared.cloud_faults[0].subscription, a.subscription);
}
#[test]
fn cloud_decoder_failure_is_scoped_and_cleanup_releases_target_and_fault() {
    let mut shared = owner();
    let (_, a) = start(&mut shared, 1);
    let (_, b) = start(&mut shared, 2);
    shared.ingest_for_test(event(&a, 1, sample(&a))).unwrap();
    let mut invalid = sample(&a);
    invalid.source_points = 2;
    // Deliberately malformed content passes the bounded envelope parser and is
    // handled by the same scoped worker decoder as the production reader.
    shared.ingest_for_test(event(&a, 2, invalid)).unwrap();
    assert!(a.stream.take(a.local_generation).is_none());
    assert!(a.stream.snapshot().unwrap().error.is_some());
    assert_eq!(shared.cloud_faults.len(), 1);
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
    assert!(shared.cloud_faults.is_empty());
    assert_eq!(shared.clouds.len(), 1);
    assert_eq!(shared.clouds[0].subscription, b.subscription);
}
#[test]
fn cloud_decode_racing_stop_or_reconnect_cannot_publish_frames_or_errors() {
    for reconnect in [false, true] {
        let mut shared = owner();
        let (_, target) = start(&mut shared, 1);
        let data = sample(&target);
        let prepared = cloud_pipe::prepare(
            &mut shared,
            target.generation,
            target.subscription,
            1,
            0,
            &data,
        )
        .unwrap();
        let decoded = decode_cloud_sample(&data);
        if reconnect {
            shared.registry.disconnect();
            target.stream.stop();
            shared
                .ingest_for_test(Event::ready("jazzy".into(), "fixture".into()))
                .unwrap();
        } else {
            shared.registry.stop_explicit(target.subscription).unwrap();
        }
        assert!(!cloud_pipe::publish(&mut shared, &prepared, 1, 0, decoded));
        assert!(!cloud_pipe::publish(
            &mut shared,
            &prepared,
            2,
            0,
            Err(studio_visualization::spatial::cloud_live::CloudLiveError(
                "late decoder error".into()
            ))
        ));
        assert!(target.stream.take(target.local_generation).is_none());
        assert!(target.stream.snapshot().is_none());
        assert!(shared.cloud_faults.is_empty());
    }
}
#[test]
fn cloud_raw_decoder_rejects_zero_envelope_but_scopes_invalid_sample_content() {
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
fn cloud_production_reader_survives_scoped_decode_failure_and_repaints_without_shared_lock() {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Reader {
        ready: Cursor<Vec<u8>>,
        rest: Option<Cursor<Vec<u8>>>,
        shared: Arc<Mutex<Shared>>,
        targets: Arc<Mutex<Vec<CloudTarget>>>,
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
                invalid.source_points = 2;
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
    assert_eq!(shared.cloud_faults.len(), 1);
    assert_eq!(shared.graph.as_ref().unwrap()[0].0, "/still_alive");
    assert!(observed.load(Ordering::Relaxed));
    assert!(unlocked.load(Ordering::Relaxed));
}
