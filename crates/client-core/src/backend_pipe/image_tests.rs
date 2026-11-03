use super::*;
use studio_protocol::{
    image::{HeaderStamp, ImageMetadata, ImageSample},
    structured::{Sample, Value, CONTRACT_VERSION},
};

#[test]
fn production_reader_publishes_decoded_frames_and_requests_repaint_after_unlocking() {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct ScheduledReader {
        ready: Cursor<Vec<u8>>,
        rest: Option<Cursor<Vec<u8>>>,
        shared: Arc<Mutex<Shared>>,
        frames: images::LatestFrames,
    }
    impl Read for ScheduledReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.ready.position() < self.ready.get_ref().len() as u64 {
                return self.ready.read(buffer);
            }
            if self.rest.is_none() {
                // The real Ready has now been ingested. Simulate the explicit
                // UI allocation before the backend releases Started + a frame.
                let mut shared = self.shared.lock().unwrap();
                let intent = allocate(&mut shared, 1, Output::CompactImage);
                shared.bind_image(target(&intent, &self.frames)).unwrap();
                let mut bytes = Event::Started {
                    generation: intent.generation,
                    request: intent.request,
                    subscription: intent.subscription,
                }
                .encode()
                .unwrap();
                bytes.extend(image_event(&intent, 1, 7).encode().unwrap());
                self.rest = Some(Cursor::new(bytes));
            }
            self.rest.as_mut().unwrap().read(buffer)
        }
    }
    let shared = Arc::new(Mutex::new(Shared::new(0)));
    let frames = images::LatestFrames::default();
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_unlocked = Arc::new(AtomicBool::new(true));
    let context = eframe::egui::Context::default();
    let observer = Arc::downgrade(&shared);
    let count = callback_count.clone();
    let unlocked = callback_unlocked.clone();
    context.set_request_repaint_callback(move |_| {
        count.fetch_add(1, Ordering::Relaxed);
        if let Some(owner) = observer.upgrade() {
            let available = owner.try_lock().is_ok();
            if !available {
                unlocked.store(false, Ordering::Relaxed);
            }
        }
    });
    shared.lock().unwrap().repaint = Some(context);
    let reader = ScheduledReader {
        ready: Cursor::new(
            Event::ready("jazzy".into(), "fixture".into())
                .encode()
                .unwrap(),
        ),
        rest: None,
        shared: shared.clone(),
        frames: frames.clone(),
    };
    assert!(pump(reader, &shared).contains("output closed"));
    assert!(frames.take_with_metadata().is_some());
    assert!(callback_count.load(Ordering::Relaxed) > 0);
    assert!(
        callback_unlocked.load(Ordering::Relaxed),
        "Repaint callback must never run under the UI lifecycle mutex"
    );
}

fn owner() -> Shared {
    let mut shared = Shared::new(0);
    shared
        .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
        .unwrap();
    shared
}
fn allocate(shared: &mut Shared, view: u64, output: Output) -> StartIntent {
    let intent = shared
        .registry
        .start_explicit(
            ViewId(view),
            Configuration {
                map_updates: None,
                topic: format!("/camera_{view}"),
                message_type: images::IMAGE_MESSAGE_TYPE.into(),
                best_effort: false,
                transient_local: false,
                output,
                spatial: None,
                robot: None,
            },
        )
        .unwrap();
    shared
        .queue
        .register(intent.generation, intent.subscription)
        .unwrap();
    intent
}
fn target(intent: &StartIntent, frames: &images::LatestFrames) -> ImageTarget {
    let local_generation = frames.start();
    ImageTarget {
        generation: intent.generation,
        subscription: intent.subscription,
        topic: intent.configuration.topic.clone(),
        stream: images::ImageStream::new(frames.clone(), local_generation),
    }
}
fn bind(shared: &mut Shared, intent: &StartIntent) -> (ImageTarget, images::LatestFrames) {
    let frames = images::LatestFrames::default();
    let target = target(intent, &frames);
    shared.bind_image(target.clone()).unwrap();
    (target, frames)
}
fn activate(shared: &mut Shared, intent: &StartIntent) {
    shared
        .ingest_for_test(Event::Started {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
        })
        .unwrap();
}
fn sample(intent: &StartIntent, sequence: u64) -> ImageSample {
    ImageSample::from_raw(
        &intent.configuration.topic,
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: 42,
                nanoseconds: sequence as u32,
            },
            frame_id: format!("camera_{sequence}"),
            width: 1,
            height: 1,
            encoding: "mono8".into(),
            is_bigendian: false,
            step: 1,
        },
        &[sequence as u8],
    )
    .unwrap()
}
fn image_event(intent: &StartIntent, sequence: u64, backend_drops: u64) -> Event {
    Event::ImageObservation {
        generation: intent.generation,
        subscription: intent.subscription,
        sequence,
        sample: Box::new(sample(intent, sequence)),
        backend_drops,
    }
}
fn typed_event(intent: &StartIntent, sequence: u64) -> Event {
    Event::Observation {
        generation: intent.generation,
        subscription: intent.subscription,
        sequence,
        sample: Box::new(Sample {
            contract_version: CONTRACT_VERSION,
            topic: intent.configuration.topic.clone(),
            message_type: intent.configuration.message_type.clone(),
            source_time: None,
            value: Value::Unsigned {
                value: sequence.to_string(),
            },
        }),
        backend_drops: 0,
    }
}

#[test]
fn mixed_outputs_and_wrong_image_identities_do_not_consume_sequence_or_change_pixels() {
    let mut shared = owner();
    let compact = allocate(&mut shared, 1, Output::CompactImage);
    let typed = allocate(&mut shared, 2, Output::Typed);
    let (target, frames) = bind(&mut shared, &compact);
    activate(&mut shared, &compact);
    activate(&mut shared, &typed);
    shared.ingest_for_test(typed_event(&compact, 1)).unwrap();
    shared.ingest_for_test(image_event(&typed, 1, 99)).unwrap();
    let mut wrong_topic = image_event(&compact, 1, 99);
    if let Event::ImageObservation { sample, .. } = &mut wrong_topic {
        sample.topic = "/different_camera".into();
    }
    shared.ingest_for_test(wrong_topic).unwrap();
    let mut wrong_generation = image_event(&compact, 1, 99);
    if let Event::ImageObservation { generation, .. } = &mut wrong_generation {
        *generation = Generation(99);
    }
    shared.ingest_for_test(wrong_generation).unwrap();
    let mut unknown_subscription = image_event(&compact, 1, 99);
    if let Event::ImageObservation { subscription, .. } = &mut unknown_subscription {
        *subscription = SubscriptionId(99);
    }
    shared.ingest_for_test(unknown_subscription).unwrap();
    assert_eq!(shared.ingress_rejections, 5);
    assert!(shared
        .registry
        .entries()
        .iter()
        .all(|e| e.last_sequence.is_none()));
    assert!(target.stream.snapshot().is_none());
    assert!(frames.take().is_none());
    shared.ingest_for_test(image_event(&compact, 1, 7)).unwrap();
    shared.ingest_for_test(typed_event(&typed, 1)).unwrap();
    shared.ingest_for_test(image_event(&compact, 2, 7)).unwrap();
    shared
        .ingest_for_test(image_event(&compact, 2, 99))
        .unwrap();
    shared
        .ingest_for_test(image_event(&compact, 1, 99))
        .unwrap();
    assert_eq!(shared.ingress_rejections, 7);
    let update = target.stream.snapshot().unwrap();
    assert_eq!(update.sequence, 2);
    assert_eq!(
        update.received_frames, 2,
        "Registry validates each sequence once before publishing"
    );
    assert_eq!(update.backend_drops, 7);
    let (frame, metadata) = frames.take_with_metadata().unwrap();
    assert_eq!(frame.pixels()[0].r(), 2);
    let metadata = metadata.unwrap();
    assert_eq!(metadata.frame_id, "camera_2");
    assert_eq!(metadata.header_stamp.nanoseconds, 2);
    assert_eq!(metadata.source_time, None);
    assert_eq!(
        shared.queue.queued_samples(),
        1,
        "Compact base64 must not enter retained typed history"
    );
}

#[test]
fn image_bindings_require_exact_allocated_identity_and_are_bounded_to32() {
    let mut shared = owner();
    let first = allocate(&mut shared, 1, Output::CompactImage);
    let frames = images::LatestFrames::default();
    let valid = target(&first, &frames);
    for (case, mut wrong) in [valid.clone(), valid.clone(), valid.clone()]
        .into_iter()
        .enumerate()
    {
        if case == 0 {
            wrong.generation = Generation(99);
        } else if case == 1 {
            wrong.subscription = SubscriptionId(99);
        } else {
            wrong.topic = "/other".into();
        }
        assert!(shared.bind_image(wrong).is_err());
        assert!(shared.images.is_empty());
    }
    shared.bind_image(valid.clone()).unwrap();
    assert!(shared.bind_image(valid).is_err());
    for view in 2..=MAX_SUBSCRIPTIONS as u64 {
        let intent = allocate(&mut shared, view, Output::CompactImage);
        bind(&mut shared, &intent);
    }
    assert_eq!(shared.images.len(), MAX_SUBSCRIPTIONS);
    let mut ninth = shared.images[0].clone();
    ninth.subscription = SubscriptionId(999);
    assert!(shared.bind_image(ninth).is_err());
    assert_eq!(shared.images.len(), MAX_SUBSCRIPTIONS);
    let mut typed_owner = owner();
    let typed = allocate(&mut typed_owner, 1, Output::Typed);
    assert!(typed_owner
        .bind_image(target(&typed, &images::LatestFrames::default()))
        .is_err());
}

#[test]
fn prepared_success_and_error_after_stop_cannot_repopulate_or_damage_replacement() {
    let mut shared = owner();
    let old = allocate(&mut shared, 1, Output::CompactImage);
    let (old_binding, frames) = bind(&mut shared, &old);
    activate(&mut shared, &old);
    let original = sample(&old, 1);
    let prepared =
        prepare_image(&mut shared, old.generation, old.subscription, 1, &original).unwrap();
    let decoded = images::decode_compact_image(&original);
    let stop = shared.registry.stop_explicit(old.subscription).unwrap();
    shared.queue.unregister(old.subscription).unwrap();
    shared
        .images
        .retain(|binding| binding.subscription != old.subscription);
    // Keep the local mailbox active here: registry Stop alone must reject this
    // already-decoded work before the UI can stop/reset its local renderer.
    assert!(!publish_image(&mut shared, &prepared, 1, 99, decoded));
    assert!(!publish_image(
        &mut shared,
        &prepared,
        2,
        99,
        Err(images::ImageSampleError::Incomplete)
    ));
    assert!(old_binding.stream.snapshot().is_none());
    assert!(frames.take().is_none());
    shared
        .ingest_for_test(Event::Stopped {
            generation: stop.generation,
            request: stop.request,
            subscription: stop.subscription,
        })
        .unwrap();
    let replacement = allocate(&mut shared, 1, Output::CompactImage);
    let current = target(&replacement, &frames);
    shared.bind_image(current.clone()).unwrap();
    activate(&mut shared, &replacement);
    shared
        .ingest_for_test(image_event(&replacement, 1, 3))
        .unwrap();
    assert!(!publish_image(
        &mut shared,
        &prepared,
        3,
        99,
        images::decode_compact_image(&sample(&old, 3))
    ));
    assert!(!publish_image(
        &mut shared,
        &prepared,
        4,
        99,
        Err(images::ImageSampleError::Incomplete)
    ));
    let update = current.stream.snapshot().unwrap();
    assert_eq!(update.sequence, 1);
    assert_eq!(update.backend_drops, 3);
    assert_eq!(update.error, None);
    assert_eq!(frames.take().unwrap().pixels()[0].r(), 1);
    assert_eq!(
        shared.registry.entries()[0].subscription,
        replacement.subscription
    );
    assert_eq!(shared.registry.entries()[0].state, State::Active);
}

#[test]
fn authoritative_cleanup_removes_only_its_binding_but_uncertain_start_error_does_not() {
    for cleanup in 0..3 {
        let mut shared = owner();
        let retiring = allocate(&mut shared, 1, Output::CompactImage);
        let peer = allocate(&mut shared, 2, Output::CompactImage);
        bind(&mut shared, &retiring);
        let (peer_target, _) = bind(&mut shared, &peer);
        activate(&mut shared, &peer);
        let event = match cleanup {
            0 => {
                activate(&mut shared, &retiring);
                let stop = shared
                    .registry
                    .stop_explicit(retiring.subscription)
                    .unwrap();
                Event::Stopped {
                    generation: stop.generation,
                    request: stop.request,
                    subscription: stop.subscription,
                }
            }
            1 => {
                activate(&mut shared, &retiring);
                Event::StreamError {
                    generation: retiring.generation,
                    subscription: retiring.subscription,
                    message: "native cleanup confirmed".into(),
                }
            }
            _ => Event::RequestError {
                generation: retiring.generation,
                request: retiring.request,
                subscription: retiring.subscription,
                message: "open failed without a handle".into(),
                no_handle: true,
            },
        };
        shared.ingest_for_test(event).unwrap();
        assert_eq!(shared.images.len(), 1);
        assert_eq!(shared.images[0].subscription, peer.subscription);
        shared.ingest_for_test(image_event(&peer, 1, 0)).unwrap();
        assert_eq!(peer_target.stream.snapshot().unwrap().received_frames, 1);
    }
    let mut shared = owner();
    let uncertain = allocate(&mut shared, 1, Output::CompactImage);
    bind(&mut shared, &uncertain);
    shared
        .ingest_for_test(Event::RequestError {
            generation: uncertain.generation,
            request: uncertain.request,
            subscription: uncertain.subscription,
            message: "handle state unknown".into(),
            no_handle: false,
        })
        .unwrap();
    assert_eq!(shared.images.len(), 1);
    assert_eq!(shared.registry.entries()[0].state, State::Starting);
}

#[test]
fn image_coalescing_and_pause_counters_are_distinct_from_transport_and_backend_drops() {
    let mut shared = owner();
    let image = allocate(&mut shared, 1, Output::CompactImage);
    let typed = allocate(&mut shared, 2, Output::Typed);
    let (target, frames) = bind(&mut shared, &image);
    activate(&mut shared, &image);
    activate(&mut shared, &typed);
    for sequence in 1..=1000 {
        shared
            .ingest_for_test(image_event(&image, sequence, 7))
            .unwrap();
    }
    assert_eq!(shared.queue.queued_samples(), 0);
    assert_eq!(shared.queue.sample_bytes(), 0);
    assert_eq!(frames.counters().superseded, 999);
    let update = target.stream.snapshot().unwrap();
    assert_eq!(update.received_frames, 1000);
    assert_eq!(update.backend_drops, 7);
    assert_eq!(
        shared
            .queue
            .stream_counters(image.subscription)
            .unwrap()
            .evicted_samples,
        0
    );
    let (latest, metadata) = frames.take_with_metadata().unwrap();
    assert_eq!(latest.pixels()[0].r(), 1000u64 as u8);
    assert_eq!(metadata.unwrap().frame_id, "camera_1000");
    frames.set_paused(true);
    shared
        .ingest_for_test(image_event(&image, 1001, 7))
        .unwrap();
    assert_eq!(frames.counters().paused_skips, 1);
    assert!(frames.take().is_none());
    for sequence in 1..=100 {
        shared
            .ingest_for_test(typed_event(&typed, sequence))
            .unwrap();
    }
    assert_eq!(shared.queue.queued_samples(), PER_STREAM_SAMPLES);
    assert_eq!(
        shared
            .queue
            .stream_counters(typed.subscription)
            .unwrap()
            .evicted_samples,
        84
    );
    assert_eq!(frames.counters().superseded, 999);
    assert_eq!(target.stream.snapshot().unwrap().backend_drops, 7);
}

#[test]
fn local_unsupported_decode_clears_pixels_but_never_acknowledges_native_cleanup() {
    let mut shared = owner();
    let intent = allocate(&mut shared, 1, Output::CompactImage);
    let (target, frames) = bind(&mut shared, &intent);
    activate(&mut shared, &intent);
    shared.ingest_for_test(image_event(&intent, 1, 2)).unwrap();
    let unsupported = ImageSample::from_raw(
        &intent.configuration.topic,
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: 42,
                nanoseconds: 2,
            },
            frame_id: "optical".into(),
            width: 1,
            height: 1,
            encoding: "16SC1".into(),
            is_bigendian: false,
            step: 2,
        },
        &[1, 2],
    )
    .unwrap();
    shared
        .ingest_for_test(Event::ImageObservation {
            generation: intent.generation,
            subscription: intent.subscription,
            sequence: 2,
            sample: Box::new(unsupported),
            backend_drops: 3,
        })
        .unwrap();
    assert!(frames.take().is_none());
    assert!(target.stream.snapshot().unwrap().error.is_some());
    assert_eq!(shared.registry.entries()[0].state, State::Active);
    assert_eq!(shared.images.len(), 1);
    shared.ingest_for_test(image_event(&intent, 3, 3)).unwrap();
    assert!(
        frames.take().is_none(),
        "Later frames cannot silently resolve a latched local display error"
    );
    assert_eq!(target.stream.snapshot().unwrap().received_frames, 3);
    assert_eq!(shared.registry.entries()[0].last_sequence, Some(3));
}
