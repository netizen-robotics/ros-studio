//! Real backend-session/client-core qualification with localhost synthetic ROS.
//! No UI/GPU, robot commands or runtime Python dependency is established here.
use serde_json::json;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use studio_client_core::backend_pipe::{self, MapStream, MapTarget, Shared};
use studio_client_core::map_fixture::{Managed, MapPublisher as Publisher};
use studio_protocol::{
    map_grid::UpdateSource,
    map_snapshot::{ClearReason, Complete, Owner},
    mapping,
    session::{Command, Event},
    spatial,
    spatial_input::SpatialClockPolicy,
    structured,
    subscriptions::*,
};

#[cfg(not(feature = "spatial"))]
use studio_client_core::backend_pipe::MapUpdate as IncomingUpdate;
#[cfg(feature = "spatial")]
use studio_client_core::backend_pipe::PreparedMapUpdate as IncomingUpdate;
#[cfg(not(feature = "spatial"))]
type PreparedOrRaw = Arc<Complete>;
#[cfg(feature = "spatial")]
type PreparedOrRaw = Arc<studio_client_core::spatial::map_texture::TextureMap>;
struct QualifiedComplete(PreparedOrRaw);
impl std::ops::Deref for QualifiedComplete {
    type Target = Complete;
    fn deref(&self) -> &Complete {
        #[cfg(feature = "spatial")]
        {
            self.0.complete()
        }
        #[cfg(not(feature = "spatial"))]
        {
            &self.0
        }
    }
}
enum QualifiedUpdate {
    Complete(QualifiedComplete),
    Cleared(Arc<studio_protocol::map_snapshot::Clear>),
}
fn take_map(stream: &MapStream) -> Option<Result<QualifiedUpdate, String>> {
    #[cfg(feature = "spatial")]
    let incoming = stream.take_prepared();
    #[cfg(not(feature = "spatial"))]
    let incoming = stream.take();
    incoming.map(|result| {
        result.map(|update| match update {
            IncomingUpdate::Complete(map) => QualifiedUpdate::Complete(QualifiedComplete(map)),
            IncomingUpdate::Cleared(clear) => QualifiedUpdate::Cleared(clear),
        })
    })
}

struct Session {
    process: Managed,
    shared: Arc<Mutex<Shared>>,
    terminal: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    counter: u64,
}
impl Session {
    fn new(root: &Path, prefix: &Path, folder: &Path, index: u32, prior: u64) -> Self {
        let mut command = Process::new(root.join("target/debug/studio-backend"));
        command
            .arg("--observe-session")
            .env("DYLD_LIBRARY_PATH", prefix.join("lib"))
            .stderr(File::create(folder.join(format!("backend-{index}.stderr.log"))).unwrap());
        let mut process = Managed::spawn(command);
        let shared = Arc::new(Mutex::new(Shared::new(prior)));
        let (tx, terminal) = mpsc::sync_channel(1);
        let reader = backend_pipe::spawn(process.child.stdout.take().unwrap(), shared.clone(), tx);
        let mut session = Self {
            process,
            shared,
            terminal,
            reader: Some(reader),
            counter: 0,
        };
        session.wait("session14 Ready", |session| {
            session.shared.lock().unwrap().registry.generation().0 > prior
        });
        session
    }
    fn drain(&mut self) {
        let mut shared = self.shared.lock().unwrap();
        while let Some(packet) = shared.queue.pop() {
            let item = match packet {
                Packet::Control(item)
                | Packet::Diagnostic(item)
                | Packet::Sample { bytes: item, .. } => item,
            };
            if let Event::Observation { sample, .. } = item.event {
                if sample.topic == "/studio_fixture/map_counter" {
                    if let structured::Value::Object { fields, .. } = &sample.value {
                        if let Some(field) = fields.iter().find(|f| f.name == "data") {
                            if let structured::Value::Unsigned { value } = &field.value {
                                self.counter = value.parse().unwrap();
                            }
                        }
                    }
                }
            }
        }
    }
    fn wait(&mut self, label: &str, mut predicate: impl FnMut(&mut Self) -> bool) {
        let until = Instant::now() + Duration::from_secs(25);
        loop {
            self.drain();
            if predicate(self) {
                return;
            }
            if let Ok(message) = self.terminal.try_recv() {
                panic!("{label}: backend ended: {message}");
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
    fn allocate(&mut self, view: u64, configuration: Configuration) -> StartIntent {
        let mut shared = self.shared.lock().unwrap();
        let intent = shared
            .registry
            .start_explicit(ViewId(view), configuration)
            .unwrap();
        shared
            .queue
            .register(intent.generation, intent.subscription)
            .unwrap();
        intent
    }
    fn start(&mut self, intent: &StartIntent, configuration: Configuration) {
        self.send(Command::Start {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
            configuration,
        });
        self.wait("identified Started", |s| {
            s.shared
                .lock()
                .unwrap()
                .registry
                .entries()
                .iter()
                .any(|e| e.subscription == intent.subscription && e.state == State::Active)
        });
    }
    fn map(&mut self, view: u64, updates: bool) -> (StartIntent, MapStream) {
        let configuration = map_config(updates);
        let intent = self.allocate(view, configuration.clone());
        let owner = Owner {
            generation: intent.generation,
            subscription: intent.subscription,
            topic: configuration.topic.clone(),
            fixed_frame: "world".into(),
        };
        let stream = MapStream::new(owner.clone(), updates).unwrap();
        self.shared
            .lock()
            .unwrap()
            .bind_map(MapTarget {
                owner,
                stream: stream.clone(),
            })
            .unwrap();
        self.start(&intent, configuration);
        (intent, stream)
    }
    fn frame(&mut self, stream: &MapStream, stamp: i64) -> QualifiedComplete {
        let mut found = None;
        self.wait("atomic complete map", |_| match take_map(stream) {
            Some(Ok(QualifiedUpdate::Complete(c))) if c.header.metadata.stamp_ns == stamp => {
                found = Some(c);
                true
            }
            Some(Err(e)) => panic!("Map worker fault: {e}"),
            _ => false,
        });
        found.unwrap()
    }
    fn stop(&mut self, intent: &StartIntent, stream: Option<&MapStream>) {
        let stop = self
            .shared
            .lock()
            .unwrap()
            .registry
            .stop_explicit(intent.subscription)
            .unwrap();
        if let Some(stream) = stream {
            stream.stop();
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
    fn finish(&mut self) {
        self.process.finish();
        self.reader.take().unwrap().join().unwrap();
        self.shared.lock().unwrap().registry.disconnect();
    }
}
fn map_config(updates: bool) -> Configuration {
    Configuration {
        topic: "/studio_fixture/raw_grid".into(),
        message_type: mapping::MAP_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: true,
        output: Output::MapSnapshot,
        robot: None,
        spatial: Some(spatial::Configuration {
            fixed_frame: "world".into(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
        map_updates: updates.then(|| UpdateSource {
            topic: "/studio_fixture/raw_grid_updates".into(),
            best_effort: false,
            transient_local: false,
        }),
    }
}
fn main() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap();
    assert!(matches!(
        rmw.as_str(),
        "rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp"
    ));
    for key in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(
            std::env::var(key).unwrap_or_default().is_empty(),
            "Remove {key}"
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let prefix = std::env::split_paths(&std::env::var_os("AMENT_PREFIX_PATH").unwrap())
        .next()
        .unwrap();
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Evidence directory required"),
    );
    assert!(
        folder.is_absolute() && folder.starts_with(root.join("artifacts/feasibility/evidence"))
    );
    std::fs::create_dir_all(&folder).unwrap();
    let mut publisher = Publisher::new(root, &prefix, &folder);
    let publisher_pid = publisher.process.child.id();
    let mut rows = Vec::new();
    publisher.send("retained");
    let mut session = Session::new(root, &prefix, &folder, 1, 0);
    let first_pid = session.process.child.id();
    let counter_config = Configuration {
        topic: "/studio_fixture/map_counter".into(),
        message_type: "std_msgs/msg/UInt64".into(),
        best_effort: false,
        transient_local: false,
        output: Output::Typed,
        spatial: None,
        robot: None,
        map_updates: None,
    };
    let peer = session.allocate(1, counter_config.clone());
    session.start(&peer, counter_config);
    let config = map_config(true);
    let bad = session.allocate(2, config.clone());
    let mut invalid = config;
    invalid.map_updates.as_mut().unwrap().topic = invalid.topic.clone();
    session.send(Command::Start {
        generation: bad.generation,
        request: bad.request,
        subscription: bad.subscription,
        configuration: invalid,
    });
    session.wait("correlated invalid start cleanup", |s| {
        !s.shared
            .lock()
            .unwrap()
            .registry
            .entries()
            .iter()
            .any(|e| e.subscription == bad.subscription)
    });
    assert_eq!(session.shared.lock().unwrap().registry.used_resources(), 1);
    rows.push(json!({"case":"invalid-update-source-no-native-handle","outcome":"passed"}));
    let (map, stream) = session.map(3, true);
    let frame = session.frame(&stream, 2_000_000_003);
    assert_eq!(frame.grid.cell_bytes(), 4 * 1024 * 1024);
    assert_eq!(frame.header.transform[0], 2.);
    drop(frame);
    assert_eq!(session.shared.lock().unwrap().registry.used_resources(), 5);
    rows.push(json!({"case":"maximum-retained-full-with-actual-static-TF-and-shared-peer","outcome":"passed"}));
    publisher.send("counter");
    session.wait("typed peer progress", |s| s.counter == 1);
    publisher.send("patch");
    let frame = session.frame(&stream, 2_000_000_004);
    assert_eq!(frame.header.assembly_counters.patches, 1);
    drop(frame);
    rows.push(json!({"case":"maximum-ordered-update-snapshot2-client","outcome":"passed"}));
    publisher.send("burst");
    let mut burst = None;
    session.wait("burst final source state",|_|match take_map(&stream){
        Some(Ok(QualifiedUpdate::Complete(c))) if c.header.metadata.stamp_ns==3_000_000_031=>{burst=Some(json!({"state":"complete","continuity":c.header.source_status.continuity,"ingress":c.header.source_status.ingress_counters,"patches":c.header.assembly_counters.patches}));true},
        Some(Ok(QualifiedUpdate::Cleared(c))) if c.source_status.recovery_floor_ns==Some(3_000_000_031)=>{burst=Some(json!({"state":"cleared","reason":c.reason,"ingress":c.source_status.ingress_counters}));true},
        Some(Err(e))=>panic!("Burst worker fault: {e}"),_=>false});
    rows.push(json!({"case":"burst-continuity-not-inferred-from-zero-loss","outcome":"passed","actual":burst}));
    publisher.send("recovery");
    drop(session.frame(&stream, 4_000_000_000));
    publisher.send("malformed_patch");
    session.wait("scoped malformed source Clear", |_| {
        match take_map(&stream) {
            Some(Ok(QualifiedUpdate::Cleared(c))) if c.reason == ClearReason::InvalidInput => true,
            Some(Err(e)) => panic!("Malformed data incorrectly killed client: {e}"),
            _ => false,
        }
    });
    publisher.send("recovery");
    drop(session.frame(&stream, 4_000_000_000));
    rows.push(json!({"case":"malformed-native-update-clear-and-full-recovery","outcome":"passed"}));
    session.stop(&map, Some(&stream));
    assert_eq!(session.shared.lock().unwrap().registry.used_resources(), 1);
    publisher.send("counter");
    session.wait("peer survives map Stop", |s| s.counter == 2);
    assert!(take_map(&stream).is_none());
    rows.push(json!({"case":"scoped-stop-peer-progress-and-no-map-replay","outcome":"passed"}));
    let (full_only, full_stream) = session.map(4, false);
    let frame = session.frame(&full_stream, 4_000_000_000);
    assert_eq!(
        frame.header.source_status.continuity,
        studio_protocol::map_grid::Continuity::Disabled
    );
    drop(frame);
    assert_eq!(session.shared.lock().unwrap().registry.used_resources(), 4);
    session.stop(&full_only, Some(&full_stream));
    session.stop(&peer, None);
    let counts = publisher.no_sources();
    rows.push(json!({"case":"full-only-retained-reader-cost-and-final-cleanup","outcome":"passed","counts":counts}));
    session.finish();
    let mut reconnected = Session::new(root, &prefix, &folder, 2, 1);
    let second_pid = reconnected.process.child.id();
    assert!(reconnected
        .shared
        .lock()
        .unwrap()
        .registry
        .entries()
        .is_empty());
    let counts = publisher.no_sources();
    assert!(take_map(&stream).is_none());
    assert!(take_map(&full_stream).is_none());
    let (_active_disconnect, disconnect_stream) = reconnected.map(5, false);
    drop(reconnected.frame(&disconnect_stream, 4_000_000_000));
    assert_eq!(
        reconnected.shared.lock().unwrap().registry.used_resources(),
        3
    );
    reconnected.finish();
    assert!(disconnect_stream.is_stopped());
    let disconnect_counts = publisher.no_sources();
    rows.push(json!({"case":"active-session-EOF-releases-sources-and-cancels-client-map","outcome":"passed","counts":disconnect_counts}));
    let mut third = Session::new(root, &prefix, &folder, 3, 2);
    let third_pid = third.process.child.id();
    assert!(third.shared.lock().unwrap().registry.entries().is_empty());
    publisher.no_sources();
    third.finish();
    publisher.finish();
    rows.push(json!({"case":"new-generation-reconnect-without-automatic-start","outcome":"passed","counts":counts}));
    let result = json!({"outcome":"passed","scope":"Local backend session/client-core; no native UI/GPU or release acceptance","rmw":rmw,"worker_prepared":cfg!(feature="spatial"),"rows":rows,"pids":[publisher_pid,first_pid,second_pid,third_pid],"reader_pid":std::process::id()});
    std::fs::write(
        folder.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("{result}");
}
