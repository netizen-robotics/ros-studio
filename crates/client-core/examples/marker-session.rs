//! Actual ROS/backend/default-client Marker acceptance. No native UI/GPU proof.
use serde_json::{json, Value};
use std::{
    fs::File,
    io::Write,
    path::Path,
    process::Command as Process,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use studio_client_core::{
    backend_pipe::{self, MarkerStream, MarkerTarget, MarkerUpdate, Shared},
    map_fixture::{FixturePublisher, Managed},
};
use studio_protocol::{
    marker,
    marker_snapshot::{self as wire, Owner},
    session::{Command, Event},
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::*,
};
struct Session {
    process: Managed,
    shared: Arc<Mutex<Shared>>,
    terminal: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    images: u64,
    image_frames: studio_images::LatestFrames,
}
impl Session {
    fn new(root: &Path, folder: &Path, index: u32, prior: u64) -> Self {
        let mut command = Process::new(root.join("target/debug/studio-backend"));
        command
            .arg("--observe-session")
            .env("DYLD_LIBRARY_PATH", root.join(".pixi/envs/default/lib"))
            .stderr(File::create(folder.join(format!("backend-{index}.stderr.log"))).unwrap());
        let mut process = Managed::spawn(command);
        std::fs::write(
            folder.join(format!("backend-{index}.pid")),
            process.child.id().to_string(),
        )
        .unwrap();
        let shared = Arc::new(Mutex::new(Shared::new(prior)));
        let (tx, terminal) = mpsc::sync_channel(1);
        let reader = backend_pipe::spawn(process.child.stdout.take().unwrap(), shared.clone(), tx);
        let mut s = Self {
            process,
            shared,
            terminal,
            reader: Some(reader),
            images: 0,
            image_frames: studio_images::LatestFrames::default(),
        };
        s.wait("session14 Ready", |s| {
            s.shared.lock().unwrap().registry.generation().0 > prior
        });
        s
    }
    fn wait(&mut self, label: &str, mut f: impl FnMut(&mut Self) -> bool) {
        let until = Instant::now() + Duration::from_secs(25);
        loop {
            {
                let mut shared = self.shared.lock().unwrap();
                while let Some(packet) = shared.queue.pop() {
                    let item = match packet {
                        Packet::Control(i)
                        | Packet::Diagnostic(i)
                        | Packet::Sample { bytes: i, .. } => i,
                    };
                    if matches!(item.event, Event::ImageObservation { .. }) {
                        self.images += 1;
                    }
                }
            }
            if self.image_frames.take_with_metadata().is_some() {
                self.images += 1;
            }
            if f(self) {
                println!("Passed: {label}");
                return;
            }
            if let Ok(e) = self.terminal.try_recv() {
                panic!("{label}: terminal {e}");
            }
            assert!(Instant::now() < until, "Timed out: {label}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn send(&mut self, command: Command) {
        self.process
            .input
            .as_mut()
            .unwrap()
            .write_all(&command.encode().unwrap())
            .unwrap();
    }
    fn start(&mut self, view: u64, config: Configuration) -> (StartIntent, Option<MarkerStream>) {
        let intent = {
            let mut shared = self.shared.lock().unwrap();
            let intent = shared
                .registry
                .start_explicit(ViewId(view), config.clone())
                .unwrap();
            shared
                .queue
                .register(intent.generation, intent.subscription)
                .unwrap();
            intent
        };
        if config.output == Output::CompactImage {
            let generation = self.image_frames.start();
            self.shared
                .lock()
                .unwrap()
                .bind_image(backend_pipe::ImageTarget {
                    generation: intent.generation,
                    subscription: intent.subscription,
                    topic: config.topic.clone(),
                    stream: studio_images::ImageStream::new(self.image_frames.clone(), generation),
                })
                .unwrap();
        }
        let stream = (config.output == Output::MarkerSnapshot).then(|| {
            let owner = Owner {
                generation: intent.generation,
                subscription: intent.subscription,
                topic: config.topic.clone(),
                message_type: config.message_type.clone(),
                fixed_frame: config.spatial.as_ref().unwrap().fixed_frame.clone(),
            };
            let stream = MarkerStream::new(owner.clone()).unwrap();
            self.shared
                .lock()
                .unwrap()
                .bind_marker(MarkerTarget {
                    owner,
                    stream: stream.clone(),
                })
                .unwrap();
            stream
        });
        self.send(Command::Start {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
            configuration: config,
        });
        self.wait("Started", |s| {
            s.shared
                .lock()
                .unwrap()
                .registry
                .entries()
                .iter()
                .any(|e| e.subscription == intent.subscription && e.state == State::Active)
        });
        (intent, stream)
    }
    fn frame(
        &mut self,
        stream: &MarkerStream,
        label: &str,
        mut predicate: impl FnMut(&wire::Complete) -> bool,
    ) -> Arc<wire::Complete> {
        let mut result = None;
        self.wait(label, |_| match stream.take() {
            Some(Ok(MarkerUpdate::Complete {
                state, visibility, ..
            })) if predicate(&state) => {
                let now = stream.monotonic_ns();
                for entry in state.visible_entries(now) {
                    assert!(visibility.allows(&visibility, &entry.marker.key, entry.version, now));
                }
                result = Some(state);
                true
            }
            Some(Err(e)) => panic!("Marker fault: {e}"),
            _ => false,
        });
        result.unwrap()
    }
    fn stop(&mut self, intent: &StartIntent, stream: Option<&MarkerStream>) {
        let stop = self
            .shared
            .lock()
            .unwrap()
            .registry
            .stop_explicit(intent.subscription)
            .unwrap();
        if let Some(s) = stream {
            s.stop();
        }
        self.send(Command::Stop {
            generation: stop.generation,
            request: stop.request,
            subscription: stop.subscription,
        });
        self.wait("authoritative Stopped", |s| {
            !s.shared
                .lock()
                .unwrap()
                .registry
                .entries()
                .iter()
                .any(|e| e.subscription == intent.subscription)
        });
    }
    fn clear(&mut self, intent: &StartIntent, stream: &MarkerStream) {
        let command = self
            .shared
            .lock()
            .unwrap()
            .clear_markers_explicit(intent.subscription)
            .unwrap();
        assert!(stream.is_waiting_for_clear());
        assert!(stream.take().is_none());
        assert!(stream.visibility().is_none());
        self.send(command);
        self.wait("correlated Clear ack", |_| !stream.is_waiting_for_clear());
    }
    fn finish(&mut self) {
        self.process.finish();
        self.reader.take().unwrap().join().unwrap();
        self.shared.lock().unwrap().registry.disconnect();
    }
}
fn config(target: &str, array: bool) -> Configuration {
    Configuration {
        topic: format!("/studio_fixture/{target}"),
        message_type: if array {
            marker::ARRAY_MESSAGE_TYPE
        } else {
            marker::MESSAGE_TYPE
        }
        .into(),
        best_effort: false,
        transient_local: true,
        output: Output::MarkerSnapshot,
        robot: None,
        map_updates: None,
        spatial: Some(spatial::Configuration {
            fixed_frame: "world".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
    }
}
fn publish(p: &mut FixturePublisher, target: &str, markers: Value) {
    assert_eq!(
        p.send(&json!({"op":"publish","target":target,"markers":markers}).to_string())["sent"],
        "publish"
    );
}
fn counts(p: &mut FixturePublisher, predicate: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let c = p.send(r#"{"op":"counts"}"#);
        if predicate(&c) {
            return c;
        }
        assert!(Instant::now() < end, "DDS counts: {c}");
        thread::sleep(Duration::from_millis(50));
    }
}
fn main() {
    for (k, v) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(k).as_deref(), Ok(v));
    }
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap();
    assert!(matches!(
        rmw.as_str(),
        "rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp"
    ));
    for k in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(
            std::env::var(k).unwrap_or_default().is_empty(),
            "Remove {k}"
        );
    }
    let root = std::env::current_dir().unwrap();
    let folder = std::path::PathBuf::from(std::env::var("STUDIO_MARKER_EVIDENCE").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let mut command = Process::new(root.join(".pixi/envs/default/bin/python"));
    command
        .arg(root.join("crates/backend/tests/fixtures/marker_publisher.py"))
        .stderr(File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    publish(&mut publisher, "single", json!([{"id":1}]));
    publish(&mut publisher, "array", json!([{"id":10}]));
    let mut session = Session::new(&root, &folder, 1, 0);
    let mut cases = vec![];
    let (single, single_stream) = session.start(1, config("marker", false));
    let single_stream = single_stream.unwrap();
    let (array, array_stream) = session.start(2, config("marker_array", true));
    let array_stream = array_stream.unwrap();
    let first = session.frame(&single_stream, "retained single", |s| {
        s.entries().len() == 1 && s.visible_entries(single_stream.monotonic_ns()).count() == 1
    });
    assert_eq!(first.entries()[0].marker.key.id, 1);
    assert!(
        matches!(&first.entries()[0].transform, wire::Transform::Resolved { pose: studio_protocol::latest_pose::LatestPose::Ready { components, .. } } if components[0]==2.0)
    );
    session.frame(&array_stream, "retained array", |s| {
        s.entries().len() == 1 && s.visible_entries(array_stream.monotonic_ns()).count() == 1
    });
    let c = counts(&mut publisher, |c| {
        c["single"] == 1 && c["array"] == 1 && c["static_tf"] == 1
    });
    assert_eq!(c["sibling"], 0);
    assert_eq!(session.shared.lock().unwrap().registry.used_resources(), 4);
    cases.push("retained explicit Marker/Array, shared TF and no sibling subscription");
    let mut image_config = config("marker_image", false);
    image_config.output = Output::CompactImage;
    image_config.message_type = studio_protocol::image::MESSAGE_TYPE.into();
    image_config.spatial = None;
    image_config.transient_local = false;
    let (image, _) = session.start(3, image_config);
    counts(&mut publisher, |c| c["image"] == 1);
    publisher.send(r#"{"op":"image"}"#);
    session.wait("Image beside two Marker owners", |s| s.images > 0);
    publish(
        &mut publisher,
        "array",
        json!([{"id":10,"x":1.0},{"id":10,"x":2.0},{"id":11,"namespace":"other"}]),
    );
    session.frame(&array_stream, "ordered duplicate replacement", |s| {
        s.entries().len() == 2
            && s.entries()
                .iter()
                .any(|e| e.marker.key.id == 10 && e.marker.pose[0] == 2.0)
    });
    publish(&mut publisher, "array", json!([{"id":10,"type":12}]));
    session.frame(&array_stream, "unsupported ADD removes old key", |s| {
        s.entries().len() == 1 && s.entries()[0].marker.key.namespace == "other"
    });
    publish(
        &mut publisher,
        "array",
        json!([{"id":0,"action":3,"namespace":"other"}]),
    );
    session.frame(&array_stream, "namespace DELETEALL", |s| {
        s.entries().is_empty()
    });
    publish(&mut publisher, "array", json!([{"id":20},{"id":21}]));
    let before_state = session.frame(&array_stream, "two entries", |s| s.entries().len() == 2);
    let before_delete = array_stream.visibility().unwrap();
    publish(&mut publisher, "array", json!([{"id":20,"action":2}]));
    session.frame(&array_stream, "default-field DELETE", |s| {
        s.entries().len() == 1 && s.entries()[0].marker.key.id == 21
    });
    let deleted = before_state
        .entries()
        .iter()
        .find(|e| e.marker.key.id == 20)
        .unwrap();
    let peer_key = before_state
        .entries()
        .iter()
        .find(|e| e.marker.key.id == 21)
        .unwrap()
        .marker
        .key
        .clone();
    publish(&mut publisher, "array", json!([{"id":20}]));
    let readded = session.frame(&array_stream, "same key readded after DELETE", |s| {
        s.entries().len() == 2
    });
    let current = array_stream.visibility().unwrap();
    let peer = readded
        .entries()
        .iter()
        .find(|e| e.marker.key == peer_key)
        .unwrap();
    assert!(current.allows(
        &before_delete,
        &peer_key,
        peer.version,
        array_stream.monotonic_ns()
    ));
    assert!(!current.allows(
        &before_delete,
        &deleted.marker.key,
        deleted.version,
        array_stream.monotonic_ns()
    ));
    publish(
        &mut publisher,
        "array",
        json!([{"id":0,"action":3,"namespace":""}]),
    );
    session.frame(&array_stream, "global DELETEALL", |s| {
        s.entries().is_empty()
    });
    cases.push("ordered keys, duplicate ADD, rejected ADD, DELETE and namespace/global DELETEALL");
    publish(
        &mut publisher,
        "array",
        json!([{"id":29,"type":11,"points":[[0,0,0],[1,0,0],[0,1,0],[0,0,1],[1,0,1],[0,1,1]],"colors":[[1,0,0,0.25],[0,1,0,0.75]]}]),
    );
    let face = session.frame(&array_stream, "native TriangleList face colors", |s| {
        s.entries().len() == 1 && s.entries()[0].marker.key.id == 29
    });
    let colors = face.entries()[0].marker.geometry.colors();
    assert_eq!(colors.len(), 6);
    assert_eq!(&colors[..3], &[[1., 0., 0., 0.25]; 3]);
    assert_eq!(&colors[3..], &[[0., 1., 0., 0.75]; 3]);
    publish(&mut publisher, "array", json!([{"id":29,"action":2}]));
    session.frame(&array_stream, "face-color case cleanup", |s| {
        s.entries().is_empty()
    });
    cases.push("TriangleList per-face colors normalize to canonical per-vertex colors");

    publish(
        &mut publisher,
        "array",
        json!([{"id":30,"frame":"missing","locked":true}]),
    );
    session.frame(&array_stream, "missing frame hidden", |s| {
        s.entries().len() == 1 && s.visible_entries(array_stream.monotonic_ns()).count() == 0
    });
    assert_eq!(
        publisher.send(r#"{"op":"connect_frame"}"#)["sent"],
        "connect_frame"
    );
    session.frame(&array_stream, "TF-only recovery", |s| {
        s.visible_entries(array_stream.monotonic_ns()).count() == 1
    });
    cases.push("frame-locked missing TF and recovery without new Marker");
    session.clear(&array, &array_stream);
    let c = counts(&mut publisher, |c| c["array"] == 1);
    assert_eq!(c["single"], 1);
    assert_eq!(c["static_tf"], 1);
    publish(&mut publisher, "array", json!([{"id":40}]));
    session.frame(&array_stream, "fresh post-Clear owner state", |s| {
        s.entries().len() == 1 && s.entries()[0].marker.key.id == 40
    });
    cases.push("Clear acknowledgment hides old state, retains readers and accepts fresh state");
    publish(
        &mut publisher,
        "array",
        json!([{"id":41,"lifetime_ns":300000000}]),
    );
    session.frame(&array_stream, "finite lifetime arrives", |s| {
        s.entries().len() == 2
    });
    session.frame(&array_stream, "idle expiry keeps permanent peer", |s| {
        s.entries().len() == 1 && s.entries()[0].marker.key.id == 40
    });
    cases.push("idle finite expiry with permanent peer");
    session.clear(&array, &array_stream);
    let mut maximum = None;
    for id in 0..16 {
        publish(
            &mut publisher,
            "array",
            json!([{"id":100+id,"type":8,"points":4096}]),
        );
        maximum = Some(
            session.frame(&array_stream, "bounded aggregate source grows", |s| {
                s.entries().len() == (id + 1) as usize
            }),
        );
    }
    let maximum = maximum.unwrap();
    assert_eq!(maximum.entries().len(), 16);
    assert_eq!(
        maximum
            .entries()
            .iter()
            .map(|e| e.marker.geometry.points().len())
            .sum::<usize>(),
        65536
    );
    cases.push("65536 retained points reassembled atomically through bounded chunks");
    let command = session
        .shared
        .lock()
        .unwrap()
        .clear_markers_explicit(array.subscription)
        .unwrap();
    session.send(command);
    session.stop(&array, Some(&array_stream));
    assert!(array_stream.is_stopped());
    assert!(array_stream.take().is_none());
    counts(&mut publisher, |c| {
        c["array"] == 0 && c["single"] == 1 && c["static_tf"] == 1
    });
    publish(&mut publisher, "single", json!([{"id":2}]));
    session.frame(&single_stream, "peer survives owner Stop", |s| {
        s.entries().len() == 2
    });
    let prior_images = session.images;
    publisher.send(r#"{"op":"image"}"#);
    session.wait("Image survives Marker Clear/Stop", |s| {
        s.images > prior_images
    });
    session.stop(&image, None);
    cases.push("Image peer continues beside maximum Marker load and Clear/Stop");
    let mut mismatch = config("marker_best", false);
    mismatch.transient_local = false;
    let (reliable, reliable_stream) = session.start(4, mismatch.clone());
    let reliable_stream = reliable_stream.unwrap();
    counts(&mut publisher, |c| c["static_tf"] == 1);
    let mut mismatch_next = Instant::now();
    let until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < until {
        if Instant::now() >= mismatch_next {
            publish(&mut publisher, "best", json!([{"id":70}]));
            mismatch_next = Instant::now() + Duration::from_millis(80);
        }
        assert!(!matches!(
            reliable_stream.take(),
            Some(Ok(MarkerUpdate::Complete { .. }))
        ));
        thread::sleep(Duration::from_millis(5));
    }
    session.stop(&reliable, Some(&reliable_stream));
    mismatch.best_effort = true;
    let (best, best_stream) = session.start(5, mismatch);
    let best_stream = best_stream.unwrap();
    counts(&mut publisher, |c| c["best"] == 1);
    let mut next = Instant::now();
    let mut best_effort_sent = 0;
    session.wait("explicit QoS correction with bounded live source", |_| {
        if Instant::now() >= next {
            publish(&mut publisher, "best", json!([{"id":71}]));
            best_effort_sent += 1;
            next = Instant::now() + Duration::from_millis(100);
        }
        match best_stream.take() {
            Some(Ok(MarkerUpdate::Complete { state, .. })) => {
                state.entries().len() == 1 && state.entries()[0].marker.key.id == 71
            }
            Some(Err(e)) => panic!("Corrected Best effort stream fault: {e}"),
            _ => false,
        }
    });
    session.stop(&best, Some(&best_stream));
    cases.push(
        "Reliable/Best effort mismatch and explicit correction with live small Marker stream",
    );
    session.stop(&single, Some(&single_stream));
    counts(&mut publisher, |c| {
        c["array"] == 0 && c["single"] == 0 && c["static_tf"] == 0
    });
    cases.push("scoped Stop and final native reader cleanup");
    let (_eof, eof_stream) = session.start(6, config("marker", false));
    let eof_stream = eof_stream.unwrap();
    session.frame(&eof_stream, "retained data before EOF", |s| {
        !s.entries().is_empty()
    });
    let generation = session.shared.lock().unwrap().registry.generation().0;
    session.finish();
    assert!(eof_stream.is_stopped());
    counts(&mut publisher, |c| c["single"] == 0 && c["static_tf"] == 0);
    cases.push("stdin EOF releases an active Marker and shared TF");
    let mut second = Session::new(&root, &folder, 2, generation);
    assert!(second.shared.lock().unwrap().registry.entries().is_empty());
    counts(&mut publisher, |c| {
        c["single"] == 0 && c["array"] == 0 && c["static_tf"] == 0
    });
    second.finish();
    cases.push("new backend generation remains inactive without replay");
    let publisher_pid = publisher.process.child.id();
    publisher.finish();
    std::fs::write(folder.join("result.json"),serde_json::to_vec_pretty(&json!({"cases":cases,"rmw":rmw,"test_pid":std::process::id(),"publisher_pid":publisher_pid,"best_effort_sent":best_effort_sent,"session":14})).unwrap()).unwrap();
    println!("Marker actual ROS cases passed: {}", cases.len());
}
