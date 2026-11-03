//! Public worker lifecycle checks. No UI, ROS or timing-based race claims.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use studio_desktop::robot::{AssetWorker, LoadCompletion, LoadOptions, RobotErrorKind};

/// Tests run off UI frames; every path, including assertion unwind, reaps its worker.
struct OwnedWorker(AssetWorker);
impl OwnedWorker {
    fn new() -> Self {
        Self(AssetWorker::new().unwrap())
    }
}
impl Drop for OwnedWorker {
    fn drop(&mut self) {
        self.0.request_shutdown();
        let _ = self.0.finish_shutdown();
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "studio-robot-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (PathBuf, LoadOptions) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    (
        root.join("articulated.urdf"),
        LoadOptions {
            package_roots: BTreeMap::from([("fixture_robot".into(), root)]),
        },
    )
}
fn completion(worker: &mut AssetWorker) -> LoadCompletion {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = worker.poll() {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "asset worker must produce a correlated completion"
        );
        std::thread::yield_now();
    }
}

#[test]
fn cancellation_keeps_slot_busy_until_polled_and_retry_uses_fresh_identity() {
    let mut owner = OwnedWorker::new();
    let worker = &mut owner.0;
    assert!(!worker.is_busy());
    assert!(worker.poll().is_none());
    let (path, options) = fixture();
    let id = worker.start(path.clone(), options.clone()).unwrap();
    assert!(worker.is_busy());
    assert!(
        worker.start(path.clone(), options.clone()).is_err(),
        "an unpolled completion still owns its slot"
    );
    assert!(worker.cancel(id));
    assert!(worker.is_busy(), "cancel is not cleanup/completion proof");
    assert!(worker.start(path.clone(), options.clone()).is_err());
    let cancelled = completion(worker);
    assert!(cancelled.id == id);
    match cancelled.result {
        Err(error) => assert_eq!(error.kind, RobotErrorKind::Cancelled),
        Ok(_) => panic!(
            "cancelled result must not deliver prepared geometry even if loading finished first"
        ),
    }
    assert!(!worker.is_busy());
    assert!(
        !worker.cancel(id),
        "an old completed ID cannot cancel future work"
    );
    let retry = worker.start(path, options).unwrap();
    assert!(retry != id);
    assert!(!worker.cancel(id));
    let loaded = completion(worker);
    assert!(loaded.id == retry);
    let loaded = loaded.result.unwrap();
    assert_eq!(loaded.model.links.len(), 5);
    assert_eq!(loaded.prepared.link_names.len(), 5);
    assert!(loaded.prepared.triangle_count > 0);
    assert!(!worker.is_busy());
    assert!(
        worker.poll().is_none(),
        "one job must not replay its completion"
    );
}

#[test]
fn file_and_parse_failures_are_correlated_and_do_not_poison_retry() {
    let scratch = Scratch::new();
    let invalid = scratch.write("invalid.urdf", "<robot name='bad'><link></robot>");
    let absent_asset = scratch.write("missing-mesh.urdf", "<robot name='missing'><link name='base'><visual><geometry><mesh filename='missing.stl'/></geometry></visual></link></robot>");
    let cases = [
        (scratch.0.join("absent.urdf"), RobotErrorKind::Io),
        (invalid, RobotErrorKind::Xml),
        (absent_asset, RobotErrorKind::Asset),
    ];
    let mut owner = OwnedWorker::new();
    let worker = &mut owner.0;
    for (path, expected) in cases {
        let id = worker
            .start(
                path,
                LoadOptions {
                    package_roots: BTreeMap::new(),
                },
            )
            .unwrap();
        let result = completion(worker);
        assert!(result.id == id);
        match result.result {
            Err(error) => {
                assert_eq!(error.kind, expected);
                assert!(!error.message.is_empty());
            }
            Ok(_) => panic!("invalid source unexpectedly supplied a robot model"),
        }
        assert!(!worker.is_busy());
    }
    let (path, options) = fixture();
    let id = worker.start(path, options).unwrap();
    let result = completion(worker);
    assert!(result.id == id);
    assert_eq!(result.result.unwrap().model.name, "original_fixture_robot");
}

#[test]
fn shutdown_reaps_an_accepted_job_without_requiring_a_completion_poll() {
    let mut owner = OwnedWorker::new();
    let worker = &mut owner.0;
    let (path, options) = fixture();
    worker.start(path.clone(), options.clone()).unwrap();
    // Either the work is still running or its completion is already in the
    // reserved slot. Both paths must shut down without UI polling/draining.
    worker.request_shutdown();
    assert!(worker.start(path.clone(), options.clone()).is_err());
    worker.finish_shutdown().unwrap();
    assert!(
        worker.start(path, options).is_err(),
        "shutdown must never silently restart a worker"
    );
}

#[test]
fn shutdown_of_idle_worker_closes_admission_without_synthesizing_a_load() {
    let mut owner = OwnedWorker::new();
    let worker = &mut owner.0;
    worker.request_shutdown();
    worker.finish_shutdown().unwrap();
    let (path, options) = fixture();
    assert!(worker.start(path, options).is_err());
    assert!(worker.poll().is_none());
}
