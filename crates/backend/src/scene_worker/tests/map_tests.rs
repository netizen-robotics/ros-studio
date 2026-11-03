use super::*;
use crate::native_topic::Publication;
use studio_protocol::{
    map_grid::{Grid, Metadata, Patch, UpdateSource},
    map_snapshot::{self, Received},
};
type FullInputs = Arc<Mutex<VecDeque<Result<Grid, String>>>>;
type UpdateInputs = Arc<Mutex<VecDeque<(Result<Patch, String>, Publication)>>>;
#[derive(Default)]
struct Fixture {
    full: FullInputs,
    updates: UpdateInputs,
    inputs: Inputs,
    opens: Arc<Mutex<Vec<String>>>,
    drops: Arc<Mutex<Vec<String>>>,
    fail_updates: Arc<AtomicBool>,
}
struct MapFake {
    full: FullInputs,
    updates: UpdateInputs,
    publication: Option<Publication>,
    topic: String,
    owner: thread::ThreadId,
    drops: Arc<Mutex<Vec<String>>>,
}
impl Drop for MapFake {
    fn drop(&mut self) {
        assert_eq!(self.owner, thread::current().id());
        self.drops.lock().unwrap().push(self.topic.clone());
    }
}
impl Reader for MapFake {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Err("Map used generic preview".into())
    }
    fn poll_grid(&mut self) -> Result<Option<Result<Grid, String>>, String> {
        Ok(self.full.lock().unwrap().pop_front())
    }
    fn poll_grid_update(&mut self) -> Result<Option<Result<Patch, String>>, String> {
        Ok(self
            .updates
            .lock()
            .unwrap()
            .pop_front()
            .map(|(input, pubinfo)| {
                self.publication = Some(pubinfo);
                input
            }))
    }
    fn publication(&self) -> Result<Publication, String> {
        self.publication.clone().ok_or("No publication".into())
    }
    fn enable_message_loss_events(&mut self) -> Result<bool, String> {
        Ok(true)
    }
    fn message_loss_count(&mut self) -> Result<Option<u64>, String> {
        Ok(Some(0))
    }
}
impl Fixture {
    fn factory(&self) -> Factory {
        let (full, updates, inputs, opens, drops, fail) = (
            self.full.clone(),
            self.updates.clone(),
            self.inputs.clone(),
            self.opens.clone(),
            self.drops.clone(),
            self.fail_updates.clone(),
        );
        Arc::new(move |config| {
            opens.lock().unwrap().push(config.topic.clone());
            if config.topic == "/map_updates" && fail.load(Ordering::Acquire) {
                return Err("Injected update open failure".into());
            }
            if config.topic == "/map" || config.topic == "/map_updates" {
                Ok(Box::new(MapFake {
                    full: full.clone(),
                    updates: updates.clone(),
                    publication: None,
                    topic: config.topic.clone(),
                    owner: thread::current().id(),
                    drops: drops.clone(),
                }))
            } else {
                Ok(Box::new(Fake {
                    topic: config.topic.clone(),
                    owner: thread::current().id(),
                    inputs: inputs.clone(),
                    drops: drops.clone(),
                }))
            }
        })
    }
}
fn configuration() -> Configuration {
    let mut c = crate::mapping_worker::tests::configuration();
    c.topic = "/map".into();
    c.output = Output::MapSnapshot;
    c.map_updates = Some(UpdateSource {
        topic: "/map_updates".into(),
        best_effort: false,
        transient_local: false,
    });
    c
}
fn grid(size: u32, stamp: i64) -> Grid {
    Grid {
        metadata: Metadata {
            frame_id: "map".into(),
            stamp_ns: stamp,
            width: size,
            height: size,
            resolution: 0.05,
            origin: [0., 0., 0., 0., 0., 0., 1.],
            map_load_time_ns: 0,
        },
        cells: vec![0; size as usize * size as usize].into_boxed_slice(),
    }
}
fn pop(queue: &Arc<Mutex<FairQueue<EncodedPayload>>>) -> Option<Event> {
    let packet = queue.lock().unwrap().pop()?;
    let Packet::Sample {
        generation,
        subscription,
        sequence,
        bytes,
    } = packet
    else {
        panic!("Unexpected control");
    };
    let wire=format!("{{\"event\":\"{}\",\"generation\":{},\"subscription\":{},\"sequence\":{},\"backend_drops\":0,\"sample\":{}}}",bytes.event_name(),generation.0,subscription.0,sequence,std::str::from_utf8(bytes.bytes()).unwrap());
    Some(Event::decode(wire.as_bytes()).unwrap())
}
#[test]
fn slow_writer_keeps_one_chunk_applies_every_patch_and_allows_peer_and_scoped_stop() {
    let fixture = Fixture::default();
    fixture.full.lock().unwrap().push_back(Ok(grid(2048, 10)));
    let service = SceneService::new(fixture.factory());
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    let mut map = start(&service, &queue, 1, configuration());
    wait(|| {
        queue
            .lock()
            .unwrap()
            .stream_queued_samples(SubscriptionId(1))
            == Some(1)
    });
    let mut receiver = map_snapshot::Receiver::new(map_snapshot::Owner {
        generation: Generation(7),
        subscription: SubscriptionId(1),
        topic: "/map".into(),
        fixed_frame: "map".into(),
    })
    .unwrap();
    match pop(&queue).unwrap() {
        Event::MapSnapshotObservation { sample, .. } => {
            assert_eq!(receiver.receive(&sample).unwrap(), Received::Pending)
        }
        other => panic!("{other:?}"),
    }
    for index in 0..32 {
        fixture.updates.lock().unwrap().push_back((
            Ok(Patch {
                frame_id: "map".into(),
                stamp_ns: 11 + index as i64,
                x: index,
                y: 0,
                width: 1,
                height: 1,
                cells: vec![(index + 1) as i8].into_boxed_slice(),
            }),
            Publication {
                publisher_id: "aa".repeat(16),
                sequence: None,
            },
        ));
    }
    wait(|| fixture.updates.lock().unwrap().is_empty());
    assert_eq!(
        queue
            .lock()
            .unwrap()
            .stream_queued_samples(SubscriptionId(1)),
        Some(1)
    );
    let mut pose = crate::mapping_worker::tests::configuration();
    pose.topic = "/pose".into();
    pose.output = Output::Pose;
    pose.message_type = mapping::POSE_MESSAGE_TYPE.into();
    fixture.inputs.lock().unwrap().insert(
        "/pose".into(),
        VecDeque::from([Input::Mapping(mapping::Decoded {
            topic: "/pose".into(),
            message_type: mapping::POSE_MESSAGE_TYPE.into(),
            source_frame: "map".into(),
            acquisition_stamp_ns: 10,
            geometry: mapping::Geometry::Pose {
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        })]),
    );
    let mut peer = start(&service, &queue, 2, pose);
    let mut peer_seen = false;
    let mut complete = None;
    wait(|| {
        assert!(
            queue
                .lock()
                .unwrap()
                .stream_queued_samples(SubscriptionId(1))
                .unwrap()
                <= 1
        );
        while let Some(event) = pop(&queue) {
            match event {
                Event::MapSnapshotObservation { sample, .. } => {
                    if let Received::Complete(map) = receiver.receive(&sample).unwrap() {
                        if map.header.metadata.stamp_ns == 42 {
                            complete = Some(map);
                        }
                    }
                }
                Event::MappingObservation {
                    subscription: SubscriptionId(2),
                    ..
                } => peer_seen = true,
                other => panic!("{other:?}"),
            }
        }
        peer_seen && complete.is_some()
    });
    let complete = complete.unwrap();
    assert_eq!(complete.header.assembly_counters.patches, 32);
    assert_eq!(
        complete.header.source_status.continuity,
        studio_protocol::map_grid::Continuity::Unknown
    );
    for index in 0..32 {
        assert_eq!(complete.grid.cells[index], (index + 1) as i8);
    }
    assert!(fixture.updates.lock().unwrap().is_empty());
    assert_eq!(
        queue
            .lock()
            .unwrap()
            .stream_counters(SubscriptionId(1))
            .unwrap()
            .evicted_samples,
        0
    );
    map.stop().unwrap();
    let mut closed = fixture.drops.lock().unwrap().clone();
    closed.sort();
    assert_eq!(closed, ["/map", "/map_updates"]);
    peer.stop().unwrap();
    let mut closed = fixture.drops.lock().unwrap().clone();
    closed.sort();
    assert_eq!(
        closed,
        ["/map", "/map_updates", "/pose", "/tf", "/tf_static"]
    );
}
#[test]
fn update_open_failure_releases_full_and_tf_before_error_completion() {
    let fixture = Fixture::default();
    fixture.fail_updates.store(true, Ordering::Release);
    let service = SceneService::new(fixture.factory());
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    queue
        .lock()
        .unwrap()
        .register(Generation(7), SubscriptionId(1))
        .unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    let result = service.attach_and_wait(Attach {
        generation: Generation(7),
        subscription: SubscriptionId(1),
        configuration: configuration(),
        ready: ready.clone(),
        released: Arc::new(AtomicBool::new(true)),
        cancel: Arc::new(AtomicBool::new(false)),
        queue,
        spatial_counters: None,
    });
    assert!(result.unwrap_err().contains("update open failure"));
    assert!(!ready.load(Ordering::Acquire));
    let mut closed = fixture.drops.lock().unwrap().clone();
    closed.sort();
    assert_eq!(closed, ["/map", "/tf", "/tf_static"]);
}

#[test]
fn cancellation_while_update_reader_opens_drops_all_sources_before_completion() {
    let fixture = Fixture::default();
    let base = fixture.factory();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let release = Mutex::new(release_rx);
    let factory: Factory = Arc::new(move |config| {
        if config.topic == "/map_updates" {
            entered_tx.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }
        base(config)
    });
    let service = SceneService::new(factory);
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    queue
        .lock()
        .unwrap()
        .register(Generation(7), SubscriptionId(1))
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let attach = Attach {
        generation: Generation(7),
        subscription: SubscriptionId(1),
        configuration: configuration(),
        ready: ready.clone(),
        released: Arc::new(AtomicBool::new(true)),
        cancel: cancel.clone(),
        queue,
        spatial_counters: None,
    };
    let join = thread::spawn(move || service.attach_and_wait(attach));
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cancel.store(true, Ordering::Release);
    release_tx.send(()).unwrap();
    assert!(join.join().unwrap().unwrap_err().contains("canceled"));
    assert!(!ready.load(Ordering::Acquire));
    let mut closed = fixture.drops.lock().unwrap().clone();
    closed.sort();
    assert_eq!(closed, ["/map", "/map_updates", "/tf", "/tf_static"]);
}
