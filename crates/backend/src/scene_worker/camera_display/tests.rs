use super::*;
use studio_protocol::{
    camera_info::Calibration,
    camera_snapshot::{
        transport::{Message, Receiver as CameraReceiver},
        Status,
    },
    image::{HeaderStamp, ImageMetadata, ImageSample},
};
#[derive(Default)]
struct Inputs {
    info: Option<Result<Calibration, String>>,
    image: Option<Result<ImageSample, String>>,
    opens: Vec<String>,
    drops: Vec<String>,
    fail_info: bool,
    cancel_image: Option<Arc<AtomicBool>>,
}
struct Fake {
    topic: String,
    created: thread::ThreadId,
    inputs: Arc<Mutex<Inputs>>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        assert_eq!(self.created, thread::current().id());
        self.inputs.lock().unwrap().drops.push(self.topic.clone());
    }
}
impl Reader for Fake {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Ok(None)
    }
    fn poll_camera_info(&mut self) -> Result<Option<Result<Calibration, String>>, String> {
        Ok(self.inputs.lock().unwrap().info.take())
    }
    fn poll_camera_image(&mut self) -> Result<Option<Result<ImageSample, String>>, String> {
        Ok(self.inputs.lock().unwrap().image.take())
    }
}
fn configuration() -> Configuration {
    Configuration {
        topic: "/camera/image_rect".into(),
        message_type: "sensor_msgs/msg/Image".into(),
        best_effort: false,
        transient_local: false,
        output: Output::CameraSnapshot(Box::new(studio_protocol::camera_snapshot::InfoSource {
            topic: "/camera/camera_info".into(),
            best_effort: true,
            transient_local: false,
        })),
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
fn calibration() -> Calibration {
    serde_json::from_value(serde_json::json!({"stamp_ns":1,"frame_id":"world","width":2,"height":2,
        "distortion_model":"plumb_bob","d":[],"k":[100.,0.,1.,0.,100.,1.,0.,0.,1.],
        "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],"p":[100.,0.,1.,0.,0.,100.,1.,0.,0.,0.,1.,0.],
        "binning_x":0,"binning_y":0,"roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}})).unwrap()
}
fn image() -> ImageSample {
    ImageSample::from_raw(
        "/camera/image_rect",
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: 1,
                nanoseconds: 0,
            },
            frame_id: "world".into(),
            width: 2,
            height: 2,
            encoding: "rgba8".into(),
            is_bigendian: false,
            step: 8,
        },
        &[255; 16],
    )
    .unwrap()
}
fn service(inputs: &Arc<Mutex<Inputs>>) -> SceneService {
    let shared = inputs.clone();
    let factory: Factory = Arc::new(move |c| {
        let mut state = shared.lock().unwrap();
        state.opens.push(c.topic.clone());
        if c.topic == "/camera/camera_info" && state.fail_info {
            return Err("injected CameraInfo open failure".into());
        }
        if c.topic == "/camera/image_rect" {
            if let Some(cancel) = &state.cancel_image {
                cancel.store(true, Ordering::Release);
            }
        }
        drop(state);
        Ok(Box::new(Fake {
            topic: c.topic.clone(),
            created: thread::current().id(),
            inputs: shared.clone(),
        }))
    });
    SceneService::new(factory)
}
struct Ticket {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<(), String>>>,
    ready: Arc<AtomicBool>,
}
impl Ticket {
    fn stop(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        self.join.take().unwrap().join().unwrap()
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.join.is_some() {
            let _ = self.stop();
        }
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < until, "Camera actor timeout");
        thread::sleep(Duration::from_millis(1));
    }
}
fn start(
    service: &SceneService,
    queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
    id: u64,
    configuration: Configuration,
    cancel: Arc<AtomicBool>,
) -> Ticket {
    queue
        .lock()
        .unwrap()
        .register(Generation(7), SubscriptionId(id))
        .unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    let attach = Attach {
        generation: Generation(7),
        subscription: SubscriptionId(id),
        configuration,
        cancel: cancel.clone(),
        ready: ready.clone(),
        released: Arc::new(AtomicBool::new(true)),
        queue: queue.clone(),
        spatial_counters: None,
    };
    let clone = service.clone();
    let join = thread::spawn(move || clone.attach_and_wait(attach));
    wait(|| ready.load(Ordering::Acquire) || join.is_finished());
    Ticket {
        cancel,
        join: Some(join),
        ready,
    }
}
fn peer(service: &SceneService, queue: &Arc<Mutex<FairQueue<EncodedPayload>>>) -> Ticket {
    let mut c = configuration();
    c.output = Output::TfGraph;
    c.topic = "/tf".into();
    c.message_type = spatial_input::TF_MESSAGE_TYPE.into();
    let ticket = start(service, queue, 1, c, Arc::new(AtomicBool::new(false)));
    assert!(ticket.ready.load(Ordering::Acquire));
    ticket
}
fn sample(
    queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
    mut wanted: impl FnMut(&Message) -> bool,
) -> Message {
    let mut found = None;
    wait(|| {
        while let Some(packet) = queue.lock().unwrap().pop() {
            if let Packet::Sample {
                subscription,
                bytes: payload,
                ..
            } = packet
            {
                if subscription == SubscriptionId(2) {
                    let message = Message::decode(payload.bytes()).unwrap();
                    if wanted(&message) {
                        found = Some(message);
                        return true;
                    }
                }
            }
        }
        false
    });
    found.unwrap()
}
#[test]
fn paired_actor_orders_calibration_barriers_and_keeps_tf_peer_after_camera_stop() {
    let inputs = Arc::new(Mutex::new(Inputs {
        info: Some(Ok(calibration())),
        image: Some(Ok(image())),
        ..Default::default()
    }));
    let service = service(&inputs);
    let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
    let mut peer = peer(&service, &queue);
    let mut camera = start(
        &service,
        &queue,
        2,
        configuration(),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(camera.ready.load(Ordering::Acquire));
    let Message::Snapshot(first) = sample(
        &queue,
        |m| matches!(m,Message::Snapshot(s) if s.status==Status::Receiving),
    ) else {
        unreachable!()
    };
    assert_eq!(inputs.lock().unwrap().opens.len(), 4);
    let mut receiver = CameraReceiver::new(first.owner.clone()).unwrap();
    let accepted = receiver.apply(*first).unwrap().unwrap();
    let mut cal = calibration();
    cal.p[2] = 0.5;
    inputs.lock().unwrap().info = Some(Ok(cal));
    let Message::Invalidated(barrier) = sample(
        &queue,
        |m| matches!(m,Message::Invalidated(b) if b.continuity_epoch>accepted.snapshot.continuity_epoch),
    ) else {
        unreachable!()
    };
    receiver.invalidate(&barrier).unwrap();
    assert!(!accepted.permission.is_valid());
    inputs.lock().unwrap().image = Some(Ok(image()));
    let Message::Snapshot(next) = sample(
        &queue,
        |m| matches!(m,Message::Snapshot(s) if s.status==Status::Receiving && s.continuity_epoch==barrier.continuity_epoch),
    ) else {
        unreachable!()
    };
    assert_eq!(next.frame.as_ref().unwrap().calibration.p[2], 0.5);
    assert_eq!(next.frame.as_ref().unwrap().image_sequence, 2);
    camera.stop().unwrap();
    let drops = inputs.lock().unwrap().drops.clone();
    assert!(drops.contains(&"/camera/image_rect".into()));
    assert!(drops.contains(&"/camera/camera_info".into()));
    assert!(!drops.contains(&"/tf".into()));
    queue
        .lock()
        .unwrap()
        .clear_samples(Generation(7), SubscriptionId(1))
        .unwrap();
    let mut progressed = false;
    wait(|| {
        while let Some(p) = queue.lock().unwrap().pop() {
            if let Packet::Sample { subscription, .. } = p {
                if subscription == SubscriptionId(1) {
                    progressed = true;
                    return true;
                }
            }
        }
        false
    });
    assert!(progressed);
    peer.stop().unwrap();
    drop(service);
    assert_eq!(inputs.lock().unwrap().drops.len(), 4);
}
#[test]
fn paired_open_failure_and_startup_cancel_cleanup_on_actor_thread_preserve_peer() {
    for fail in [true, false] {
        let inputs = Arc::new(Mutex::new(Inputs::default()));
        let service = service(&inputs);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let mut peer = peer(&service, &queue);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut s = inputs.lock().unwrap();
            s.fail_info = fail;
            if !fail {
                s.cancel_image = Some(cancel.clone());
            }
        }
        let mut camera = start(&service, &queue, 2, configuration(), cancel);
        wait(|| camera.join.as_ref().unwrap().is_finished());
        assert!(!camera.ready.load(Ordering::Acquire));
        assert!(camera.stop().is_err());
        let state = inputs.lock().unwrap();
        assert_eq!(state.drops, vec!["/camera/image_rect"]);
        assert_eq!(
            state
                .opens
                .iter()
                .filter(|s| s.as_str() == "/camera/camera_info")
                .count(),
            usize::from(fail)
        );
        drop(state);
        peer.stop().unwrap();
        drop(service);
        assert_eq!(inputs.lock().unwrap().drops.len(), 3);
    }
}
