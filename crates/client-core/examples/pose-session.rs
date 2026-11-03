//! Actual paired source/backend/default-client workflow, no App or GPU claim.
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
    backend_pipe::{self, PoseStream, PoseTarget, PoseUpdate, Shared},
    map_fixture::{FixturePublisher, Managed},
};
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{
        self as pose,
        transport::{Configuration as PoseConfiguration, HistoryConfiguration, Permission},
        Kind, Owner, Snapshot,
    },
    session::Command,
    spatial,
    spatial_input::SpatialClockPolicy,
    subscriptions::*,
};
struct Session {
    process: Managed,
    shared: Arc<Mutex<Shared>>,
    terminal: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
}
impl Session {
    fn open(root: &Path, folder: &Path, prior: u64, index: u32) -> Self {
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
        println!("backend_pid={}", process.child.id());
        let shared = Arc::new(Mutex::new(Shared::new(prior)));
        let (tx, terminal) = mpsc::sync_channel(1);
        let reader = backend_pipe::spawn(process.child.stdout.take().unwrap(), shared.clone(), tx);
        let mut s = Self {
            process,
            shared,
            terminal,
            reader: Some(reader),
        };
        s.wait("session15 Ready", |s| {
            s.shared.lock().unwrap().registry.generation().0 > prior
        });
        s
    }
    fn wait(&mut self, label: &str, mut predicate: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            {
                let mut state = self.shared.lock().unwrap();
                while state.queue.pop().is_some() {}
            }
            if predicate(self) {
                println!("Passed: {label}");
                return;
            }
            if let Ok(error) = self.terminal.try_recv() {
                panic!("{label}: {error}");
            }
            assert!(Instant::now() < deadline, "Timeout: {label}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn send(&mut self, c: Command) {
        self.process
            .input
            .as_mut()
            .unwrap()
            .write_all(&c.encode().unwrap())
            .unwrap();
    }
    fn start(&mut self, id: u64, c: Configuration) -> (StartIntent, PoseStream) {
        let Output::PoseSnapshot(configuration) = c.output else {
            unreachable!()
        };
        let (intent, stream) = {
            let mut shared = self.shared.lock().unwrap();
            let intent = shared
                .registry
                .start_explicit(ViewId(id), c.clone())
                .unwrap();
            shared
                .queue
                .register(intent.generation, intent.subscription)
                .unwrap();
            let owner = Owner {
                generation: intent.generation,
                subscription: intent.subscription,
                kind: configuration.kind,
                topic: c.topic.clone(),
                fixed_frame: c.spatial.as_ref().unwrap().fixed_frame.clone(),
            };
            let stream = PoseStream::new(owner.clone(), configuration).unwrap();
            shared
                .bind_pose(PoseTarget {
                    owner,
                    configuration,
                    stream: stream.clone(),
                })
                .unwrap();
            (intent, stream)
        };
        self.send(Command::Start {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
            configuration: c,
        });
        self.wait("explicit Started", |s| {
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
        stream: &PoseStream,
        label: &str,
        mut predicate: impl FnMut(&Snapshot) -> bool,
    ) -> (Arc<Snapshot>, Permission) {
        let mut value = None;
        self.wait(label, |_| match stream.take() {
            Some(Ok(PoseUpdate::Snapshot {
                snapshot,
                permission,
                ..
            })) => {
                if predicate(&snapshot) {
                    assert!(permission.is_valid());
                    value = Some((snapshot, permission));
                    true
                } else {
                    println!(
                        "Waiting: {label}: revision={} epoch={} head={:?} reset={:?}",
                        snapshot.revision,
                        snapshot.continuity_epoch,
                        snapshot
                            .latest
                            .as_ref()
                            .map(|e| (&e.transform, e.acquisition_stamp_ns)),
                        snapshot.last_reset
                    );
                    false
                }
            }
            Some(Err(error)) => panic!("Pose stream: {error}"),
            _ => false,
        });
        value.unwrap()
    }
    fn stop(&mut self, intent: &StartIntent, stream: &PoseStream) {
        let command = self
            .shared
            .lock()
            .unwrap()
            .stop_pose_explicit(intent.subscription)
            .unwrap();
        assert!(stream.is_stopped());
        self.send(command);
        self.wait("authoritative Stop", |s| {
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
fn config(target: &str, kind: Kind, best: bool) -> Configuration {
    Configuration {
        topic: format!("/studio_fixture/raw_{target}"),
        message_type: kind.message_type().into(),
        best_effort: best,
        transient_local: target != "odom_best",
        output: Output::PoseSnapshot(PoseConfiguration {
            kind,
            history: (kind == Kind::Odometry).then(HistoryConfiguration::default),
        }),
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
fn publish(p: &mut FixturePublisher, target: &str, stamp: i64, invalid: bool) {
    assert_eq!(
        p.send(
            &json!({"op":"publish","target":target,"stamp":stamp,"invalid":invalid}).to_string()
        )["sent"],
        target
    );
}
fn counts(p: &mut FixturePublisher, mut predicate: impl FnMut(&Value) -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let value = p.send(r#"{"op":"counts"}"#);
        if predicate(&value) {
            return;
        }
        assert!(Instant::now() < until, "Readers still allocated: {value}");
        thread::sleep(Duration::from_millis(20));
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
    assert!(matches!(
        std::env::var("RMW_IMPLEMENTATION").as_deref(),
        Ok("rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp")
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
        assert!(std::env::var(k).unwrap_or_default().is_empty());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let folder = std::path::PathBuf::from(std::env::var("STUDIO_POSE_EVIDENCE").unwrap());
    assert!(folder.starts_with(root.join("artifacts/feasibility/evidence")));
    let mut command = Process::new(root.join(".pixi/envs/default/bin/python"));
    command
        .arg(root.join("crates/backend/tests/fixtures/pose_publisher.py"))
        .stderr(File::create(folder.join("publisher.stderr.log")).unwrap());
    let mut publisher = FixturePublisher::from_command(command, &folder);
    println!(
        "test_pid={} publisher_pid={}",
        std::process::id(),
        publisher.process.child.id()
    );
    let mut session = Session::open(root, &folder, 0, 0);
    session.wait("discovery without subscription", |s| {
        s.shared.lock().unwrap().graph.as_ref().is_some_and(|g| {
            g.iter().any(|(t, types)| {
                t == "/studio_fixture/raw_odom"
                    && types.iter().any(|ty| ty == pose::ODOMETRY_MESSAGE_TYPE)
            })
        })
    });
    counts(&mut publisher, |c| {
        c["pose"] == 0 && c["odom"] == 0 && c["static_tf"] == 0
    });
    let (cov_intent, cov) = session.start(1, config("pose", Kind::PoseWithCovariance, false));
    let (cov_frame, cov_permission) = session.frame(
        &cov,
        "retained complete covariance and acquisition TF",
        |s| {
            s.latest.as_ref().is_some_and(|e| {
                matches!(
                    e.transform,
                    LatestPose::Ready {
                        resolved_stamp_ns: 2_000_000_003,
                        ..
                    }
                )
            })
        },
    );
    assert!(cov_frame.trail.is_empty());
    assert_eq!(
        cov_frame.latest.as_ref().unwrap().covariance.row_major()[35],
        0.09
    );
    let (best_intent, best) = session.start(2, config("odom_best", Kind::Odometry, false));
    publish(&mut publisher, "odom_best", 5, false);
    thread::sleep(Duration::from_millis(300));
    assert!(
        !matches!(best.take(),Some(Ok(PoseUpdate::Snapshot {snapshot,..})) if snapshot.latest.is_some())
    );
    session.stop(&best_intent, &best);
    assert!(cov_permission.is_valid());
    let (best_intent, best) = session.start(3, config("odom_best", Kind::Odometry, true));
    let mut next = Instant::now();
    session.wait("explicit Best effort correction",|_|{if Instant::now()>=next {publish(&mut publisher,"odom_best",6,false);next=Instant::now()+Duration::from_millis(100);}matches!(best.take(),Some(Ok(PoseUpdate::Snapshot {snapshot,..})) if snapshot.latest.as_ref().is_some_and(|e|matches!(e.transform,LatestPose::Ready {..})))});
    session.stop(&best_intent, &best);
    assert!(cov_permission.is_valid());
    let (odom_intent, odom) = session.start(4, config("odom", Kind::Odometry, false));
    let (initial, initial_permission) =
        session.frame(&odom, "retained Odometry with shared TF", |s| {
            s.latest
                .as_ref()
                .is_some_and(|e| matches!(e.transform, LatestPose::Ready { .. }))
        });
    assert_eq!(initial.trail.len(), 1);
    assert_eq!(
        initial.source.as_ref().unwrap().child_frame_id.as_deref(),
        Some("base")
    );
    counts(&mut publisher, |c| {
        c["pose"] == 1 && c["odom"] == 1 && c["static_tf"] == 1
    });
    let command = session
        .shared
        .lock()
        .unwrap()
        .clear_pose_explicit(odom_intent.subscription)
        .unwrap();
    assert!(!initial_permission.is_valid());
    session.send(command);
    session.wait("correlated Clear ack before fresh data", |_| {
        !odom.is_waiting_for_clear()
    });
    assert!(cov_permission.is_valid());
    publish(&mut publisher, "odom", 3, false);
    let (after_clear, after_clear_permission) =
        session.frame(&odom, "fresh source after Clear", |s| {
            s.latest
                .as_ref()
                .is_some_and(|e| e.acquisition_stamp_ns == 3_000_000_003)
        });
    assert_eq!(after_clear.trail.len(), 1);
    publish(&mut publisher, "odom", 0, false);
    let (zero, zero_permission) =
        session.frame(&odom, "zero-stamp latest capture and history reset", |s| {
            s.latest.as_ref().is_some_and(|e| {
                e.acquisition_stamp_ns == 0
                    && matches!(
                        e.transform,
                        LatestPose::Ready {
                            resolved_stamp_ns: 0,
                            ..
                        }
                    )
            })
        });
    assert_eq!(zero.trail.len(), 1);
    assert!(!after_clear_permission.is_valid());
    publish(&mut publisher, "pose", 4, true);
    session.wait("malformed source scoped cleanup", |s| {
        !s.shared
            .lock()
            .unwrap()
            .registry
            .entries()
            .iter()
            .any(|e| e.subscription == cov_intent.subscription)
    });
    assert!(!cov_permission.is_valid());
    assert!(zero_permission.is_valid());
    session.stop(&odom_intent, &odom);
    assert!(!zero_permission.is_valid());
    counts(&mut publisher, |c| {
        c["pose"] == 0 && c["odom"] == 0 && c["static_tf"] == 0
    });
    publish(&mut publisher, "pose", 4, false);
    let prior = session.shared.lock().unwrap().registry.generation().0;
    session.finish();
    let mut session = Session::open(root, &folder, prior, 1);
    counts(&mut publisher, |c| {
        c["pose"] == 0 && c["odom"] == 0 && c["static_tf"] == 0
    });
    let (_, fresh) = session.start(1, config("pose", Kind::PoseWithCovariance, false));
    let (_, permission) = session.frame(&fresh, "reconnect inactive until explicit Enable", |s| {
        s.latest
            .as_ref()
            .is_some_and(|e| matches!(e.transform, LatestPose::Ready { .. }))
    });
    session.finish();
    assert!(!permission.is_valid());
    counts(&mut publisher, |c| {
        c["pose"] == 0 && c["odom"] == 0 && c["static_tf"] == 0
    });
    publisher.finish();
    println!("paired pose session passed; backend/client/publisher cleanup complete");
}
