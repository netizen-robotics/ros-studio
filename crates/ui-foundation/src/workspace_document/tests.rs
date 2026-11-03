use super::*;
use crate::workspace::{ClockExpectation, Preset, ViewKind};
use studio_protocol::subscriptions::{Generation, SubscriptionId};

fn controller() -> Controller {
    Controller::new(Preset::Inspect.workspace(), SupportedRenderers::all()).unwrap()
}
#[test]
fn actual_v11_save_open_restore_keeps_camera_and_does_not_clean_newer_camera_edits() {
    use std::time::{Duration, Instant};
    use studio_workspace::{
        application::{self, Application},
        presentation::*,
    };
    let path = std::env::temp_dir().join(format!(
        "studio-v11-camera-controller-{}.json",
        std::process::id()
    ));
    let mut initial = Preset::Inspect.workspace();
    initial.views.clear();
    initial.layout = crate::workspace::Layout::for_views(&[]);
    let mut c =
        Controller::for_application(initial, SupportedRenderers::all(), Application::Visualizer)
            .unwrap();
    let mut snapshot = c.current().clone();
    snapshot.presentation.as_mut().unwrap().camera.mode = CameraMode::TopDown;
    snapshot.presentation.as_mut().unwrap().scene = Some(SceneSettings {
        fixed_frame: "world".into(),
        dynamic_tf_topic: "/tf".into(),
        static_tf_topic: "/tf_static".into(),
        dynamic_tf_best_effort: false,
    });
    c.observe(snapshot.clone()).unwrap();
    c.save_path(path.clone()).unwrap();
    let mut newer = snapshot.clone();
    newer.presentation.as_mut().unwrap().camera.target_mm = [123456, 0, 0];
    c.observe(newer.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.is_busy() {
        c.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(c.error().is_none());
    assert!(c.dirty());
    assert_eq!(c.current(), &newer);
    assert_eq!(
        application::parse(&std::fs::read(&path).unwrap(), Application::Visualizer)
            .unwrap()
            .workspace,
        snapshot
    );
    c.load_path(path.clone()).unwrap();
    while c.is_busy() {
        c.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(c.candidate().unwrap().path.is_some());
    c.begin_apply(ApplyApproval::DiscardUnsaved, &[]).unwrap();
    let restored = c.take_ready(&[]).unwrap().unwrap();
    assert_eq!(restored.configuration(), &snapshot);
    assert!(!c.dirty());
    assert_eq!(c.current(), &snapshot);
    c.shutdown();
    std::fs::remove_file(path).unwrap();
}
fn owner(id: u64) -> HandleIdentity {
    HandleIdentity {
        generation: Generation(2),
        subscription: SubscriptionId(id),
    }
}
fn pending(controller: &mut Controller, id: u64, kind: IoKind) {
    controller.pending_io = Some(PendingIo {
        id: OperationId(id),
        kind,
        context: controller.context,
        cancelled: false,
    });
}
fn complete(id: u64, kind: IoKind, output: IoOutput) -> IoCompletion {
    IoCompletion {
        id: OperationId(id),
        path: PathBuf::from("/tmp/workspace-controller.json"),
        kind,
        cancelled: false,
        result: Ok(output),
    }
}

#[test]
fn save_records_exact_snapshot_newer_edits_stay_dirty_and_never_apply() {
    let mut c = controller();
    let snapshot = c.current().clone();
    pending(&mut c, 1, IoKind::Save);
    let mut newer = snapshot.clone();
    newer.name = "Newer edit".into();
    c.observe(newer.clone()).unwrap();
    c.io_completed(complete(
        1,
        IoKind::Save,
        IoOutput::Saved(Box::new(snapshot)),
    ));
    assert_eq!(c.current(), &newer);
    assert!(c.dirty());
    assert!(c.status().contains("newer edits"));
    assert!(c.candidate().is_none());
    assert!(!c.is_applying());
    assert!(c.take_ready(&[]).unwrap().is_none());
    c.shutdown();
}

#[test]
fn file_failure_leaves_busy_copy_and_preserves_dirty_document_after_acknowledgment() {
    for kind in [IoKind::Load, IoKind::Save] {
        let mut c = controller();
        let before = c.current().clone();
        c.path = Some(PathBuf::from("/tmp/original-workspace.json"));
        c.status = if kind == IoKind::Load {
            "Opening workspace"
        } else {
            "Saving workspace"
        }
        .into();
        pending(&mut c, 1, kind);
        c.io_completed(IoCompletion {
            id: OperationId(1),
            path: PathBuf::from("/tmp/invalid-parent/workspace.json"),
            kind,
            cancelled: false,
            result: Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic denied destination",
            )
            .into()),
        });
        assert!(!c.is_busy());
        assert!(!c.status().contains("Opening"));
        assert!(!c.status().contains("Saving"));
        assert!(c.status().contains("failed"));
        assert!(c.error().is_some());
        c.acknowledge_error();
        assert!(c.error().is_none());
        assert!(c.status().contains("failed"));
        assert_eq!(c.current(), &before);
        assert_eq!(c.path(), Some(Path::new("/tmp/original-workspace.json")));
        assert!(c.dirty());
        assert!(c.take_ready(&[]).unwrap().is_none());
        c.shutdown();
    }
}

#[test]
fn edits_disconnect_and_cancel_reject_late_loads_without_discarding_current() {
    for action in 0..3 {
        let mut c = controller();
        pending(&mut c, 1, IoKind::Load);
        match action {
            0 => {
                let mut edited = c.current().clone();
                edited.name = "Edited".into();
                c.observe(edited).unwrap();
            }
            1 => c.disconnect(),
            _ => c.cancel(),
        }
        let current = c.current().clone();
        c.io_completed(complete(
            1,
            IoKind::Load,
            IoOutput::Loaded(Box::new(Preset::Analyze.workspace())),
        ));
        assert_eq!(c.current(), &current);
        assert!(c.candidate().is_none());
        assert!(c.take_ready(&[]).unwrap().is_none());
        c.shutdown();
    }
}

#[test]
fn canceled_save_never_claims_rollback_or_cleans_document() {
    let mut c = controller();
    let snapshot = c.current().clone();
    pending(&mut c, 1, IoKind::Save);
    c.cancel();
    c.io_completed(complete(
        1,
        IoKind::Save,
        IoOutput::Saved(Box::new(snapshot)),
    ));
    assert!(c.dirty());
    assert!(c.path().is_none());
    assert!(c.status().contains("destination may have changed"));
    assert!(c.error().unwrap().contains("Check the file"));
    c.shutdown();
}

#[test]
fn disconnect_does_not_hide_a_completed_save_or_autoapply_loaded_configuration() {
    let mut c = controller();
    let snapshot = c.current().clone();
    pending(&mut c, 1, IoKind::Save);
    c.disconnect();
    c.io_completed(complete(
        1,
        IoKind::Save,
        IoOutput::Saved(Box::new(snapshot)),
    ));
    assert!(!c.dirty());
    assert!(c.path().is_some());
    assert!(c.candidate().is_none());
    assert!(c.take_ready(&[]).unwrap().is_none());
    c.shutdown();
}

#[test]
fn stale_completion_cannot_consume_new_operation_and_unsupported_load_stays_reviewable() {
    let mut c = controller();
    let current = c.current().clone();
    pending(&mut c, 2, IoKind::Load);
    c.io_completed(complete(
        1,
        IoKind::Load,
        IoOutput::Loaded(Box::new(Preset::Analyze.workspace())),
    ));
    assert_eq!(c.pending_io.as_ref().unwrap().id, OperationId(2));
    let mut draft = Preset::Visualize.workspace();
    let ViewKind::Spatial {
        transforms,
        clock_expectation,
        ..
    } = &mut draft.views[0].kind
    else {
        panic!()
    };
    *transforms = None;
    *clock_expectation = ClockExpectation::Unknown;
    c.io_completed(complete(
        2,
        IoKind::Load,
        IoOutput::Loaded(Box::new(draft.clone())),
    ));
    assert_eq!(c.current(), &current);
    assert_eq!(c.candidate().unwrap().workspace, draft);
    assert!(c
        .candidate()
        .unwrap()
        .issue
        .as_ref()
        .unwrap()
        .contains("TF inputs"));
    assert!(c
        .begin_apply(ApplyApproval::DiscardUnsaved, &[owner(1)])
        .is_err());
    assert!(!c.is_applying());
    c.shutdown();
}

#[test]
fn restore_requires_dirty_approval_exact_cleanup_and_explicit_apply_once() {
    let mut c = controller();
    let current = c.current().clone();
    let target = Preset::Analyze.workspace();
    pending(&mut c, 1, IoKind::Load);
    c.io_completed(complete(
        1,
        IoKind::Load,
        IoOutput::Loaded(Box::new(target.clone())),
    ));
    assert_eq!(c.current(), &current);
    assert!(c.take_ready(&[]).unwrap().is_none());
    assert!(c
        .begin_apply(ApplyApproval::CleanOrSaved, &[owner(1)])
        .is_err());
    let stops = c
        .begin_apply(ApplyApproval::DiscardUnsaved, &[owner(1)])
        .unwrap();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].owner, owner(1));
    assert!(!c.cleanup_confirmed(owner(9)).unwrap());
    assert!(c.take_ready(&[owner(1)]).unwrap().is_none());
    c.stop_failed(owner(1), "Cleanup not confirmed").unwrap();
    assert_eq!(c.retry_failed().unwrap().len(), 1);
    assert!(c.retry_failed().unwrap().is_empty());
    assert!(c.cleanup_confirmed(owner(1)).unwrap());
    let token = c.take_ready(&[]).unwrap().unwrap();
    assert_eq!(token.configuration(), &target);
    assert_eq!(c.current(), &target);
    assert!(!c.dirty());
    assert!(c.candidate().is_none());
    assert!(c.take_ready(&[]).unwrap().is_none());
    c.disconnect();
    assert_eq!(c.current(), &target);
    assert!(c.take_ready(&[]).unwrap().is_none());
    c.shutdown();
}

#[test]
fn changed_configuration_requires_explicit_rereview_and_old_acks_cannot_apply() {
    let mut c = controller();
    pending(&mut c, 1, IoKind::Load);
    c.io_completed(complete(
        1,
        IoKind::Load,
        IoOutput::Loaded(Box::new(Preset::Analyze.workspace())),
    ));
    c.begin_apply(ApplyApproval::DiscardUnsaved, &[owner(1)])
        .unwrap();
    let mut edited = c.current().clone();
    edited.name = "Changed during restore".into();
    c.observe(edited.clone()).unwrap();
    assert!(!c.cleanup_confirmed(owner(1)).unwrap());
    assert!(c.begin_apply(ApplyApproval::DiscardUnsaved, &[]).is_err());
    assert!(c.take_ready(&[]).unwrap().is_none());
    assert_eq!(c.current(), &edited);
    c.review_candidate().unwrap();
    assert!(c
        .begin_apply(ApplyApproval::DiscardUnsaved, &[])
        .unwrap()
        .is_empty());
    assert!(c.take_ready(&[]).unwrap().is_some());
    c.shutdown();
}

#[test]
fn real_worker_path_fallback_save_and_load_only_preview_without_native_chooser() {
    let mut c = controller();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "studio-document-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    c.save_path(path.clone()).unwrap();
    while c.is_busy() {
        c.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!c.dirty());
    assert_eq!(c.path(), Some(path.as_path()));
    let saved = c.current().clone();
    let mut edited = saved.clone();
    edited.name = "Current unsaved".into();
    c.observe(edited.clone()).unwrap();
    c.load_path(path.clone()).unwrap();
    while c.is_busy() {
        c.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(c.current(), &edited);
    assert!(c.dirty());
    assert_eq!(&c.candidate().unwrap().workspace, &saved);
    assert!(!c.is_applying());
    c.shutdown();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn stale_open_dialog_selection_cannot_submit_load_after_edits() {
    let mut c = controller();
    c.pending_dialog = Some(PendingDialog {
        id: DialogId(1),
        kind: DialogKind::Open,
        context: c.context,
        snapshot: None,
    });
    let mut edited = c.current().clone();
    edited.name = "Changed while choosing".into();
    c.observe(edited.clone()).unwrap();
    c.dialog_completed(DialogCompletion {
        id: DialogId(1),
        kind: DialogKind::Open,
        result: Ok(Some(PathBuf::from("/tmp/stale-selection.json"))),
    });
    assert!(!c.worker.is_busy());
    assert!(c.pending_io.is_none());
    assert_eq!(c.current(), &edited);
    assert!(c.candidate().is_none());
    c.shutdown();
}

#[test]
fn save_dialog_snapshot_is_from_click_not_later_selection_and_recovery_never_replays() {
    let mut c = controller();
    let snapshot = c.current().clone();
    c.pending_dialog = Some(PendingDialog {
        id: DialogId(1),
        kind: DialogKind::Save,
        context: c.context,
        snapshot: Some(snapshot.clone()),
    });
    let mut newer = snapshot.clone();
    newer.name = "Edited while choosing".into();
    c.observe(newer.clone()).unwrap();
    let path = std::env::temp_dir().join(format!(
        "studio-document-dialog-{}.json",
        std::process::id()
    ));
    c.dialog_completed(DialogCompletion {
        id: DialogId(1),
        kind: DialogKind::Save,
        result: Ok(Some(path.clone())),
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while c.is_busy() {
        c.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(crate::workspace::load(&path).unwrap(), snapshot);
    assert_eq!(c.current(), &newer);
    assert!(c.dirty());
    c.restart_file_worker().unwrap();
    assert!(!c.is_busy());
    assert!(c.pending_io.is_none());
    assert!(c.candidate().is_none());
    assert!(c.take_ready(&[]).unwrap().is_none());
    c.shutdown();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn preset_review_never_activates_or_reuses_old_save_path_and_requires_dirty_approval() {
    let mut c = controller();
    let original = c.current().clone();
    pending(&mut c, 1, IoKind::Save);
    c.io_completed(complete(
        1,
        IoKind::Save,
        IoOutput::Saved(Box::new(original.clone())),
    ));
    assert!(c.path().is_some());
    c.preview_configuration(Preset::Analyze.workspace())
        .unwrap();
    assert_eq!(c.current(), &original);
    assert!(c.candidate().unwrap().path.is_none());
    assert!(!c.is_applying());
    assert!(c.take_ready(&[]).unwrap().is_none());
    let mut edit = original.clone();
    edit.name = "Unsaved before preset".into();
    c.observe(edit).unwrap();
    c.review_candidate().unwrap();
    assert!(c.begin_apply(ApplyApproval::CleanOrSaved, &[]).is_err());
    c.begin_apply(ApplyApproval::DiscardUnsaved, &[]).unwrap();
    assert!(c.take_ready(&[]).unwrap().is_some());
    assert!(c.path().is_none());
    assert_eq!(c.current(), &Preset::Analyze.workspace());
    assert!(c.dirty());
    c.shutdown();
}

#[test]
fn application_import_preserves_legacy_source_and_saves_current_tools_to_new_destination() {
    use std::time::{Duration, Instant};
    use studio_workspace::application::{self, Application};
    let root = std::env::temp_dir().join(format!("studio-v8-controller-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("legacy.json");
    let destination = root.join("tools.json");
    let original = Preset::Visualize.workspace().encode().unwrap();
    std::fs::write(&source, &original).unwrap();
    let imported = application::parse(&original, Application::Tools).unwrap();
    let mut c = Controller::for_application(
        imported.workspace.clone(),
        SupportedRenderers::all(),
        Application::Tools,
    )
    .unwrap();
    c.load_path(source.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while c.is_busy() {
        c.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!c.candidate().unwrap().excluded.is_empty());
    assert!(c.candidate().unwrap().path.is_none());
    c.begin_apply(ApplyApproval::DiscardUnsaved, &[]).unwrap();
    assert!(c.take_ready(&[]).unwrap().is_some());
    assert!(c.path().is_none());
    c.save_path(destination.clone()).unwrap();
    while c.is_busy() {
        c.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(c.error().is_none());
    assert_eq!(std::fs::read(source).unwrap(), original);
    let bytes = std::fs::read(destination).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        studio_workspace::tools::VERSION
    );
    assert!(application::parse(&bytes, Application::Visualizer).is_err());
    c.shutdown();
    std::fs::remove_dir_all(root).unwrap();
}
