use super::*;
use crate::workspace::{Document, Preset};
use std::time::{Duration, Instant};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "studio-workspace-io-{}-{}",
            std::process::id(),
            crate::workspace::storage_test_id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn wait(worker: &mut IoWorker) -> IoCompletion {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(completion) = worker.poll() {
            return completion;
        }
        assert!(
            Instant::now() < deadline,
            "workspace operation did not complete"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn application_worker_writes_exact_camera_snapshot_and_returns_it_despite_newer_edits() {
    use crate::{
        application::{self, Application},
        presentation::*,
    };
    let dir = Directory::new();
    let path = dir.path("visualizer-v10.json");
    let mut original = Preset::Inspect.workspace();
    original.views.clear();
    original.layout = super::super::Layout::for_views(&[]);
    original.presentation = Some(VisualizerPresentation {
        camera: CameraState {
            mode: CameraMode::TopDown,
            target_mm: [999_999_999, -3, 4],
            ..Default::default()
        },
        ..Default::default()
    });
    let mut document = Document::new(original.clone()).unwrap();
    let mut worker = IoWorker::for_application(Application::Visualizer);
    worker
        .submit(IoOperation::save(path.clone(), original.clone()))
        .unwrap();
    let mut newer = original.clone();
    newer.presentation.as_mut().unwrap().camera.vertical_span_mm = 54321;
    document.edit(newer.clone()).unwrap();
    let IoOutput::Saved(written) = wait(&mut worker).result.unwrap() else {
        panic!("Expected saved snapshot")
    };
    assert_eq!(*written, original);
    document.mark_saved(*written).unwrap();
    assert!(document.is_dirty());
    assert_eq!(document.current(), &newer);
    let loaded =
        application::parse(&std::fs::read(&path).unwrap(), Application::Visualizer).unwrap();
    assert_eq!(loaded.workspace, original);
    assert!(!loaded.legacy);
    worker.finish_shutdown();
}
#[test]
fn save_returns_exact_snapshot_and_load_never_mutates_document() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let original = Preset::Inspect.workspace();
    let mut document = Document::new(original.clone()).unwrap();
    let mut worker = IoWorker::new();
    let id = worker
        .submit(IoOperation::save(path.clone(), original.clone()))
        .unwrap();
    let mut edited = original.clone();
    edited.name = "Newer unsaved edit".into();
    document.edit(edited.clone()).unwrap();
    let completion = wait(&mut worker);
    assert_eq!(completion.id, id);
    assert!(!completion.cancelled);
    assert_eq!(completion.kind, IoKind::Save);
    let IoOutput::Saved(snapshot) = completion.result.unwrap() else {
        panic!("wrong result");
    };
    assert_eq!(*snapshot, original);
    document.mark_saved(*snapshot).unwrap();
    assert!(document.is_dirty());
    assert_eq!(document.current(), &edited);
    let load_id = worker.submit(IoOperation::load(path)).unwrap();
    assert!(load_id.0 > id.0);
    let completion = wait(&mut worker);
    let IoOutput::Loaded(loaded) = completion.result.unwrap() else {
        panic!("wrong result");
    };
    assert_eq!(*loaded, original);
    assert_eq!(document.current(), &edited); // Root owns preview/explicit application.
    worker.finish_shutdown();
}
#[test]
fn queue_pressure_cancellation_and_stale_ids_preserve_the_actual_write() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut worker = IoWorker::with_executor(move |operation| {
        entered_tx.send(thread::current().id()).unwrap();
        release_rx.recv().unwrap();
        execute(operation)
    });
    let snapshot = Preset::Analyze.workspace();
    let id = worker
        .submit(IoOperation::save(path.clone(), snapshot.clone()))
        .unwrap();
    assert_ne!(
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        thread::current().id()
    );
    for _ in 0..100 {
        assert_eq!(
            worker.submit(IoOperation::load(path.clone())).unwrap_err(),
            SubmitError::Busy
        );
    }
    assert!(!worker.cancel(OperationId(id.0 + 1)));
    assert!(worker.cancel(id));
    assert!(worker.is_busy());
    release_tx.send(()).unwrap();
    let completion = wait(&mut worker);
    assert!(completion.cancelled);
    assert_eq!(completion.id, id);
    assert!(matches!(completion.result, Ok(IoOutput::Saved(_))));
    assert_eq!(load(&path).unwrap(), snapshot); // Cancellation cannot undo rename.
    assert!(!worker.is_busy());
    assert!(!worker.cancel(id));
    let next = worker.submit(IoOperation::load(path)).unwrap();
    assert!(next.0 > id.0);
    assert!(!worker.cancel(id)); // Old UI completions cannot cancel the new operation.
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    release_tx.send(()).unwrap();
    assert!(!wait(&mut worker).cancelled);
    worker.finish_shutdown();
}
#[test]
fn cancellation_after_completed_write_is_truthful_at_poll_and_shutdown_reaps_worker() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let mut worker = IoWorker::with_executor(move |operation| {
        let result = execute(operation);
        done_tx.send(()).unwrap();
        result
    });
    let id = worker
        .submit(IoOperation::save(path.clone(), Preset::Inspect.workspace()))
        .unwrap();
    done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(path.is_file());
    assert!(worker.cancel(id));
    let completion = wait(&mut worker);
    assert!(completion.cancelled);
    assert!(matches!(completion.result, Ok(IoOutput::Saved(_))));
    worker.request_shutdown();
    assert_eq!(
        worker.submit(IoOperation::load(path)).unwrap_err(),
        SubmitError::Closed
    );
    worker.finish_shutdown();
    assert!(worker.join.is_none());
    worker.finish_shutdown(); // Idempotent after application close.
}
#[test]
fn load_save_failures_preserve_documents_and_existing_destination() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let snapshot = Preset::Inspect.workspace();
    let mut worker = IoWorker::new();
    worker
        .submit(IoOperation::save(path.clone(), snapshot.clone()))
        .unwrap();
    wait(&mut worker).result.unwrap();
    let original = std::fs::read(&path).unwrap();
    let document = Document::new(snapshot.clone()).unwrap();
    worker
        .submit(IoOperation::load(dir.path("missing")))
        .unwrap();
    assert!(wait(&mut worker).result.is_err());
    assert_eq!(document.current(), &snapshot);
    assert!(document.is_dirty());
    let mut invalid = snapshot.clone();
    invalid.version = 999;
    assert!(matches!(
        worker.submit(IoOperation::save(path.clone(), invalid)),
        Err(SubmitError::Invalid(_))
    ));
    assert!(!worker.is_busy());
    worker
        .submit(IoOperation::save(
            dir.path("missing/destination"),
            snapshot.clone(),
        ))
        .unwrap();
    assert!(wait(&mut worker).result.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&path, b"not JSON").unwrap();
    worker.submit(IoOperation::load(path)).unwrap();
    assert!(wait(&mut worker).result.is_err());
    assert_eq!(document.current(), &snapshot);
    worker.finish_shutdown();
}
#[test]
fn drop_does_not_wait_on_active_filesystem_work() {
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let mut worker = IoWorker::with_executor(move |_| {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        finished_tx.send(()).unwrap();
        Ok(IoOutput::CancelledBeforeStart)
    });
    worker
        .submit(IoOperation::load(PathBuf::from("pending")))
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
    let dropping = thread::spawn(move || {
        drop(worker);
        dropped_tx.send(()).unwrap();
    });
    let did_not_wait = dropped_rx.recv_timeout(Duration::from_secs(1)).is_ok();
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    dropping.join().unwrap();
    assert!(
        did_not_wait,
        "Drop waited on filesystem work instead of requesting cancellation"
    );
}

#[test]
fn shutdown_with_unpolled_completion_is_bounded_and_preserves_saved_snapshot() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let mut worker = IoWorker::with_executor(move |operation| {
        let result = execute(operation);
        done_tx.send(()).unwrap();
        result
    });
    worker
        .submit(IoOperation::save(path.clone(), Preset::Inspect.workspace()))
        .unwrap();
    done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    // The single result slot must fit the single outstanding job during shutdown,
    // even when the UI stopped polling before the file operation completed.
    worker.finish_shutdown();
    let completion = worker.poll().unwrap();
    assert!(completion.cancelled);
    let IoOutput::Saved(snapshot) = completion.result.unwrap() else {
        panic!("wrong result");
    };
    assert_eq!(load(&path).unwrap(), *snapshot);
    assert!(worker.poll().is_none());
}

#[test]
fn worker_exit_reports_one_correlated_failure_without_claiming_write_rollback() {
    let dir = Directory::new();
    let path = dir.path("workspace.json");
    let expected = Preset::Inspect.workspace();
    let document = Document::new(expected.clone()).unwrap();
    let mut worker = IoWorker::with_executor(|operation| {
        execute(operation).unwrap();
        panic!("injected worker failure after an atomic write");
    });
    let id = worker
        .submit(IoOperation::save(path.clone(), expected.clone()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !worker.join.as_ref().unwrap().is_finished() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let completion = worker
        .poll()
        .expect("worker exit must produce a correlated failure");
    assert_eq!(completion.id, id);
    assert_eq!(completion.kind, IoKind::Save);
    assert_eq!(completion.path, path);
    assert!(completion
        .result
        .unwrap_err()
        .to_string()
        .contains("destination may have changed"));
    assert!(!worker.is_busy());
    assert!(worker.poll().is_none());
    assert_eq!(
        worker.submit(IoOperation::load(path.clone())).unwrap_err(),
        SubmitError::Closed
    );
    assert_eq!(load(&path).unwrap(), expected);
    assert_eq!(document.current(), &expected);
    assert!(document.is_dirty());
    worker.finish_shutdown();
}

#[test]
fn cancelled_late_load_after_disconnect_cannot_supply_automatic_restore() {
    use crate::workspace::{RestoreCoordinator, SupportedRenderers};
    let dir = Directory::new();
    let path = dir.path("candidate.json");
    let current = Preset::Inspect.workspace();
    let target = Preset::Analyze.workspace();
    save_atomic(&path, &target).unwrap();
    let document = Document::new(current.clone()).unwrap();
    let mut restore = RestoreCoordinator::new();
    let preview = restore
        .preview(target.clone(), 10, SupportedRenderers::all())
        .unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut worker = IoWorker::with_executor(move |operation| {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        execute(operation)
    });
    let id = worker.submit(IoOperation::load(path)).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    restore.disconnected();
    assert!(worker.cancel(id)); // Root cancels related I/O on disconnect/abandoning preview.
    release_tx.send(()).unwrap();
    let completion = wait(&mut worker);
    assert_eq!(completion.id, id);
    assert!(completion.cancelled);
    let IoOutput::Loaded(candidate) = completion.result.unwrap() else {
        panic!("wrong result");
    };
    assert_eq!(*candidate, target); // A truthful late result is available but must not be applied.
    assert!(restore.take_ready(10, &[]).unwrap().is_none());
    assert!(restore.configuration().is_none());
    assert_eq!(document.current(), &current);
    assert!(restore
        .apply(
            preview,
            10,
            false,
            crate::workspace::ApplyApproval::CleanOrSaved,
            &[]
        )
        .is_err());
    worker.finish_shutdown();
}
