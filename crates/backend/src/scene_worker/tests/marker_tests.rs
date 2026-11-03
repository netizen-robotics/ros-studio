use super::*;
use crate::native_topic::Publication;
use studio_protocol::{
    marker::{self, Batch, Geometry, Key, Marker, Operation},
    marker_snapshot as wire,
};
type MarkerInputs = Arc<Mutex<HashMap<String, VecDeque<(Batch, Publication)>>>>;
#[derive(Default)]
struct Fixture {
    inputs: MarkerInputs,
    typed: Inputs,
    opens: Arc<Mutex<Vec<String>>>,
    drops: Arc<Mutex<Vec<String>>>,
    block: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
}
struct MarkerFake {
    topic: String,
    inputs: MarkerInputs,
    publication: Option<Publication>,
    owner: thread::ThreadId,
    drops: Arc<Mutex<Vec<String>>>,
}
impl Drop for MarkerFake {
    fn drop(&mut self) {
        assert_eq!(self.owner, thread::current().id());
        self.drops.lock().unwrap().push(self.topic.clone());
    }
}
impl Reader for MarkerFake {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Err("Marker used generic preview".into())
    }
    fn poll_markers(&mut self) -> Result<Option<Result<Batch, String>>, String> {
        assert_eq!(self.owner, thread::current().id());
        Ok(self
            .inputs
            .lock()
            .unwrap()
            .entry(self.topic.clone())
            .or_default()
            .pop_front()
            .map(|(batch, p)| {
                self.publication = Some(p);
                Ok(batch)
            }))
    }
    fn publication(&self) -> Result<Publication, String> {
        self.publication
            .clone()
            .ok_or("Missing test publisher".into())
    }
    fn enable_message_loss_events(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}
impl Fixture {
    fn factory(&self) -> Factory {
        let (inputs, typed, opens, drops, block, entered) = (
            self.inputs.clone(),
            self.typed.clone(),
            self.opens.clone(),
            self.drops.clone(),
            self.block.clone(),
            self.entered.clone(),
        );
        Arc::new(move |c| {
            opens.lock().unwrap().push(c.topic.clone());
            if c.output == Output::MarkerSnapshot {
                let reader = MarkerFake {
                    topic: c.topic.clone(),
                    inputs: inputs.clone(),
                    publication: None,
                    owner: thread::current().id(),
                    drops: drops.clone(),
                };
                entered.store(true, Ordering::Release);
                while block.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(Box::new(reader))
            } else {
                Ok(Box::new(Fake {
                    topic: c.topic.clone(),
                    owner: thread::current().id(),
                    inputs: typed.clone(),
                    drops: drops.clone(),
                }))
            }
        })
    }
    fn push(&self, topic: &str, id: i32, writer: &str, sequence: u64) {
        self.inputs
            .lock()
            .unwrap()
            .entry(topic.into())
            .or_default()
            .push_back((
                Batch {
                    contract_version: marker::CONTRACT_VERSION,
                    operations: vec![Operation::Add {
                        marker: Box::new(Marker {
                            key: Key {
                                namespace: "objects".into(),
                                id,
                            },
                            frame_id: "map".into(),
                            stamp_ns: 10,
                            pose: [0., 0., 0., 0., 0., 0., 1.],
                            scale: [1.; 3],
                            color: [1.; 4],
                            lifetime_ns: 0,
                            frame_locked: false,
                            geometry: Geometry::Cube,
                        }),
                    }],
                },
                Publication {
                    publisher_id: writer.repeat(16),
                    sequence: Some(sequence),
                },
            ));
    }
}
fn config(topic: &str) -> Configuration {
    let mut config = crate::mapping_worker::tests::configuration();
    config.topic = topic.into();
    config.message_type = marker::ARRAY_MESSAGE_TYPE.into();
    config.output = Output::MarkerSnapshot;
    config
}
fn receiver(id: u64, topic: &str) -> wire::Receiver {
    wire::Receiver::new(wire::Owner {
        generation: Generation(7),
        subscription: SubscriptionId(id),
        topic: topic.into(),
        message_type: marker::ARRAY_MESSAGE_TYPE.into(),
        fixed_frame: "map".into(),
    })
    .unwrap()
}
fn complete(
    queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
    id: u64,
    r: &mut wire::Receiver,
    stash: &mut HashMap<u64, Event>,
) -> Arc<wire::Complete> {
    for _ in 0..100 {
        let Event::MarkerSnapshotObservation {
            sample: message, ..
        } = sample(queue, id, stash)
        else {
            panic!("Wrong scene output")
        };
        let now = wire::Clock {
            unix_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            monotonic_ns: 0,
        };
        if let wire::Received::Complete { state, .. } = r.receive(&message, now).unwrap() {
            return state;
        }
    }
    panic!("No Marker complete state")
}
#[test]
fn two_marker_owners_share_tf_and_clear_one_without_reopening_or_changing_peer() {
    let fixture = Fixture::default();
    fixture.push("/markers", 1, "aa", 1);
    fixture.push("/peer", 9, "bb", 1);
    let service = SceneService::new(fixture.factory());
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    let mut first = start(&service, &queue, 1, config("/markers"));
    let mut peer = start(&service, &queue, 2, config("/peer"));
    let mut stash = HashMap::new();
    let mut rx = receiver(1, "/markers");
    let mut other = receiver(2, "/peer");
    assert_eq!(complete(&queue, 1, &mut rx, &mut stash).entries().len(), 1);
    assert_eq!(
        complete(&queue, 2, &mut other, &mut stash).entries()[0]
            .marker
            .key
            .id,
        9
    );
    assert_eq!(fixture.opens.lock().unwrap().len(), 4);
    service
        .clear_markers(Generation(7), SubscriptionId(1))
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let Event::MarkerSnapshotObservation { sample: clear, .. } = sample(&queue, 1, &mut stash)
    else {
        panic!("Missing clear")
    };
    assert!(
        matches!(clear.as_ref(),wire::Message::Clear(c) if c.reason==wire::ClearReason::LocalClear)
    );
    assert!(fixture.drops.lock().unwrap().is_empty());
    assert_eq!(fixture.opens.lock().unwrap().len(), 4);
    fixture.push("/peer", 10, "bb", 2);
    let next = complete(&queue, 2, &mut other, &mut stash);
    assert_eq!(next.entries().len(), 2);
    first.stop().unwrap();
    assert!(fixture.drops.lock().unwrap().contains(&"/markers".into()));
    assert_eq!(fixture.drops.lock().unwrap().len(), 1);
    assert!(service
        .clear_markers(Generation(7), SubscriptionId(1))
        .unwrap()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .is_err());
    peer.stop().unwrap();
    assert_eq!(fixture.drops.lock().unwrap().len(), 4);
}
#[test]
fn multiple_publishers_are_accepted_but_per_writer_gap_and_stale_samples_clear_only_their_owner() {
    let fixture = Fixture::default();
    fixture.push("/markers", 1, "aa", 1);
    fixture.push("/markers", 2, "bb", 1);
    let service = SceneService::new(fixture.factory());
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    let mut ticket = start(&service, &queue, 1, config("/markers"));
    let mut stash = HashMap::new();
    let mut rx = receiver(1, "/markers");
    let state = complete(&queue, 1, &mut rx, &mut stash);
    assert_eq!(state.entries().len(), 2);
    assert!(!state.known_history_gap);
    fixture.push("/markers", 3, "aa", 3);
    let state = complete(&queue, 1, &mut rx, &mut stash);
    assert!(state.known_history_gap);
    assert_eq!(state.entries().len(), 1);
    assert_eq!(state.entries()[0].marker.key.id, 3);
    fixture.push("/markers", 2, "aa", 2);
    fixture.push("/markers", 3, "aa", 3);
    fixture.push("/markers", 4, "aa", 4);
    let state = complete(&queue, 1, &mut rx, &mut stash);
    assert_eq!(state.entries().len(), 1);
    assert_eq!(state.entries()[0].marker.key.id, 4);
    ticket.stop().unwrap();
}
#[test]
fn marker_start_cancellation_drops_native_owners_on_actor_thread_and_cannot_ack_late_clear() {
    let fixture = Fixture::default();
    fixture.block.store(true, Ordering::Release);
    let service = SceneService::new(fixture.factory());
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _release = Release(fixture.block.clone());
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    queue
        .lock()
        .unwrap()
        .register(Generation(7), SubscriptionId(1))
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let attach = Attach {
        generation: Generation(7),
        subscription: SubscriptionId(1),
        configuration: config("/markers"),
        ready: Arc::new(AtomicBool::new(false)),
        released: Arc::new(AtomicBool::new(true)),
        cancel: cancel.clone(),
        queue,
        spatial_counters: None,
    };
    let clone = service.clone();
    let join = thread::spawn(move || clone.attach_and_wait(attach));
    wait(|| fixture.entered.load(Ordering::Acquire));
    let completion = service
        .clear_markers(Generation(7), SubscriptionId(1))
        .unwrap();
    cancel.store(true, Ordering::Release);
    fixture.block.store(false, Ordering::Release);
    assert!(join.join().unwrap().is_err());
    assert!(completion
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .is_err());
    assert_eq!(fixture.drops.lock().unwrap().len(), 3);
}
