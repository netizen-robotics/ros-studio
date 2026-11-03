use super::*;
use crate::native_topic::Publication;
use studio_protocol::pose_snapshot::{
    self as pose,
    transport::{Configuration as PoseConfiguration, HistoryConfiguration, Message},
    Kind,
};
type PoseInputs = Arc<Mutex<HashMap<String, VecDeque<Result<pose::Decoded, String>>>>>;
struct PoseFake {
    topic: String,
    inputs: PoseInputs,
    publication: Option<Publication>,
}
impl Reader for PoseFake {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Ok(None)
    }
    fn poll_pose_covariance(&mut self) -> Result<Option<Result<pose::Decoded, String>>, String> {
        let result = self
            .inputs
            .lock()
            .unwrap()
            .entry(self.topic.clone())
            .or_default()
            .pop_front();
        if result.is_some() {
            let sequence = self
                .publication
                .as_ref()
                .and_then(|p| p.sequence)
                .unwrap_or(0)
                + 1;
            self.publication = Some(Publication {
                publisher_id: "ab".repeat(16),
                sequence: Some(sequence),
            });
        }
        Ok(result)
    }
    fn publication(&self) -> Result<Publication, String> {
        Ok(self.publication.clone().unwrap_or(Publication {
            publisher_id: "ab".repeat(16),
            sequence: Some(1),
        }))
    }
}
fn config(kind: Kind) -> Configuration {
    let mut c = crate::mapping_worker::tests::configuration();
    c.topic = if kind == Kind::Odometry {
        "/odom"
    } else {
        "/covariance"
    }
    .into();
    c.message_type = kind.message_type().into();
    c.output = Output::PoseSnapshot(PoseConfiguration {
        kind,
        history: (kind == Kind::Odometry).then(HistoryConfiguration::default),
    });
    c
}
fn decoded(kind: Kind, stamp: i64) -> pose::Decoded {
    pose::Decoded {
        kind,
        topic: config(kind).topic,
        source: pose::Source {
            frame_id: "map".into(),
            child_frame_id: (kind == Kind::Odometry).then(|| "base".into()),
        },
        acquisition_stamp_ns: stamp,
        components: [stamp as f64, 0., 0., 0., 0., 0., 1.],
        covariance: studio_protocol::uncertainty::Covariance::new([0.; 36]).unwrap(),
    }
}
fn full(
    queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
    id: u64,
    sequence: u64,
    stash: &mut HashMap<u64, Event>,
) -> pose::Snapshot {
    for _ in 0..20 {
        let Event::PoseSnapshotObservation {
            sample: message, ..
        } = sample(queue, id, stash)
        else {
            panic!("Wrong pose scene output")
        };
        if let Message::Snapshot(s) = *message {
            if s.latest.as_ref().is_some_and(|e| e.sequence == sequence) {
                return *s;
            }
        }
    }
    panic!("Pose head not received")
}
#[test]
fn pose_clear_waits_for_ack_gate_while_input_and_peer_progress_then_resets_retire_old_data() {
    let inputs = PoseInputs::default();
    let factory_inputs = inputs.clone();
    let service = SceneService::new(Arc::new(move |c| {
        Ok(Box::new(PoseFake {
            topic: c.topic.clone(),
            inputs: factory_inputs.clone(),
            publication: None,
        }))
    }));
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    let mut stash = HashMap::new();
    let mut odom = start(&service, &queue, 1, config(Kind::Odometry));
    let mut peer = start(&service, &queue, 2, config(Kind::PoseWithCovariance));
    inputs
        .lock()
        .unwrap()
        .entry("/odom".into())
        .or_default()
        .push_back(Ok(decoded(Kind::Odometry, 10)));
    full(&queue, 1, 1, &mut stash);
    let gate = Arc::new(AtomicBool::new(false));
    let barrier = service
        .clear_pose(Generation(7), SubscriptionId(1), gate.clone())
        .unwrap()
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(barrier.reason, pose::ResetReason::LocalClear);
    inputs
        .lock()
        .unwrap()
        .entry("/odom".into())
        .or_default()
        .push_back(Ok(decoded(Kind::Odometry, 20)));
    inputs
        .lock()
        .unwrap()
        .entry("/covariance".into())
        .or_default()
        .push_back(Ok(decoded(Kind::PoseWithCovariance, 10)));
    full(&queue, 2, 1, &mut stash);
    wait(|| inputs.lock().unwrap()["/odom"].is_empty());
    assert_eq!(
        queue
            .lock()
            .unwrap()
            .stream_queued_samples(SubscriptionId(1)),
        Some(0)
    );
    gate.store(true, Ordering::Release);
    let fresh = full(&queue, 1, 2, &mut stash);
    assert!(fresh.revision > barrier.revision);
    assert_eq!(fresh.continuity_epoch, barrier.continuity_epoch);
    assert_eq!(fresh.trail.len(), 1);
    inputs
        .lock()
        .unwrap()
        .entry("/odom".into())
        .or_default()
        .push_back(Ok(decoded(Kind::Odometry, 30)));
    wait(|| {
        queue
            .lock()
            .unwrap()
            .stream_queued_samples(SubscriptionId(1))
            == Some(1)
    });
    let cleared_before = queue
        .lock()
        .unwrap()
        .stream_counters(SubscriptionId(1))
        .unwrap()
        .cleared_samples;
    let mut changed = decoded(Kind::Odometry, 40);
    changed.source.child_frame_id = Some("base2".into());
    inputs
        .lock()
        .unwrap()
        .entry("/odom".into())
        .or_default()
        .push_back(Ok(changed));
    wait(|| {
        queue
            .lock()
            .unwrap()
            .stream_counters(SubscriptionId(1))
            .unwrap()
            .cleared_samples
            > cleared_before
    });
    let Event::PoseSnapshotObservation {
        sample: message, ..
    } = sample(&queue, 1, &mut stash)
    else {
        panic!("Missing pose barrier")
    };
    assert!(
        matches!(*message,Message::Invalidated(ref b) if b.reason==pose::ResetReason::FrameChanged)
    );
    let changed = full(&queue, 1, 4, &mut stash);
    assert_eq!(changed.trail.len(), 1);
    assert_eq!(
        changed.source.unwrap().child_frame_id.as_deref(),
        Some("base2")
    );
    inputs
        .lock()
        .unwrap()
        .entry("/odom".into())
        .or_default()
        .push_back(Err("injected covariance failure".into()));
    wait(|| odom.join.as_ref().unwrap().is_finished());
    assert!(odom.join.take().unwrap().join().unwrap().is_err());
    inputs
        .lock()
        .unwrap()
        .entry("/covariance".into())
        .or_default()
        .push_back(Ok(decoded(Kind::PoseWithCovariance, 20)));
    full(&queue, 2, 2, &mut stash);
    peer.stop().unwrap();
}
