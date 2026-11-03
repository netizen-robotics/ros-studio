//! Main-thread document orchestration. Selection never performs file I/O or activates ROS.
use crate::workspace::{
    ApplyApproval, Document, HandleIdentity, InactiveRestore, IoCompletion, IoKind, IoOperation,
    IoOutput, IoWorker, OperationId, PreviewId, RestoreCoordinator, RestoreState, StopIntent,
    SupportedRenderers, Workspace,
};
use crate::workspace_dialog::{Dialog, DialogCompletion, DialogError, DialogId, DialogKind};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Candidate {
    pub workspace: Workspace,
    pub path: Option<PathBuf>,
    /// Unsupported/changed configuration remains inspectable instead of being filtered.
    pub issue: Option<String>,
    pub preview: Option<PreviewId>,
    pub excluded: Vec<String>,
}
struct PendingDialog {
    id: DialogId,
    kind: DialogKind,
    context: u64,
    snapshot: Option<Workspace>,
}
struct PendingIo {
    id: OperationId,
    kind: IoKind,
    context: u64,
    cancelled: bool,
}

pub struct Controller {
    dialog: Dialog,
    worker: IoWorker,
    document: Document,
    supported: SupportedRenderers,
    restore: RestoreCoordinator,
    revision: u64,
    context: u64,
    pending_dialog: Option<PendingDialog>,
    pending_io: Option<PendingIo>,
    path: Option<PathBuf>,
    candidate: Option<Candidate>,
    error: Option<String>,
    status: String,
    fallback: Option<DialogKind>,
    closed: bool,
    application: Option<studio_workspace::application::Application>,
}
impl Controller {
    pub fn new(initial: Workspace, supported: SupportedRenderers) -> Result<Self, String> {
        Ok(Self {
            dialog: Dialog::new(),
            worker: IoWorker::new(),
            document: Document::new(initial).map_err(|e| e.to_string())?,
            supported,
            restore: RestoreCoordinator::new(),
            revision: 0,
            context: 0,
            pending_dialog: None,
            pending_io: None,
            path: None,
            candidate: None,
            error: None,
            status: "Unsaved workspace".into(),
            fallback: None,
            closed: false,
            application: None,
        })
    }
    pub fn for_application(
        mut initial: Workspace,
        supported: SupportedRenderers,
        app: studio_workspace::application::Application,
    ) -> Result<Self, String> {
        if app == studio_workspace::application::Application::Visualizer
            && initial.presentation.is_none()
        {
            initial.presentation =
                Some(studio_workspace::presentation::VisualizerPresentation::legacy(&initial));
        }
        let mut controller = Self::new(initial, supported)?;
        controller.worker.request_shutdown();
        controller.worker = IoWorker::for_application(app);
        controller.application = Some(app);
        Ok(controller)
    }
    pub fn current(&self) -> &Workspace {
        self.document.current()
    }
    pub fn dirty(&self) -> bool {
        self.document.is_dirty()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn candidate(&self) -> Option<&Candidate> {
        self.candidate.as_ref()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn fallback_kind(&self) -> Option<DialogKind> {
        self.fallback
    }
    pub fn restore_state(&self) -> &RestoreState {
        self.restore.state()
    }
    pub fn is_busy(&self) -> bool {
        self.dialog.is_busy() || self.worker.is_busy()
    }
    pub fn is_applying(&self) -> bool {
        matches!(
            self.restore.state(),
            RestoreState::Stopping { .. } | RestoreState::Ready { .. }
        )
    }
    pub fn acknowledge_error(&mut self) {
        self.error = None;
    }
    /// Explicit recovery after a file worker failure; never retries the operation.
    pub fn restart_file_worker(&mut self) -> Result<(), String> {
        self.available()?;
        self.worker.request_shutdown();
        self.worker = self
            .application
            .map(IoWorker::for_application)
            .unwrap_or_default();
        self.status = "File worker restarted; retry the intended operation explicitly".into();
        Ok(())
    }
    fn fail<T>(&mut self, error: impl ToString) -> Result<T, String> {
        let error: String = error.to_string().chars().take(2048).collect();
        self.error = Some(error.clone());
        Err(error)
    }
    fn available(&mut self) -> Result<(), String> {
        if self.closed {
            return self.fail("Workspace controller is shut down");
        }
        if self.is_busy() || self.is_applying() {
            return self.fail("Finish or cancel the current workspace operation first");
        }
        Ok(())
    }
    fn invalidate_context(&mut self) -> Result<(), String> {
        self.context = self
            .context
            .checked_add(1)
            .ok_or("Workspace context IDs exhausted")?;
        self.restore.disconnected();
        if let Some(candidate) = &mut self.candidate {
            candidate.preview = None;
            candidate.issue =
                Some("Review this candidate again before replacing the workspace".into());
        }
        if self
            .pending_dialog
            .as_ref()
            .is_some_and(|p| p.kind == DialogKind::Open)
        {
            self.dialog.cancel();
        }
        if let Some(pending) = &mut self.pending_io {
            if pending.kind == IoKind::Load {
                pending.cancelled = true;
                self.worker.cancel(pending.id);
            }
        }
        Ok(())
    }
    /// Record root's configuration snapshot. Runtime samples/status do not belong here.
    pub fn observe(&mut self, mut workspace: Workspace) -> Result<bool, String> {
        if self.application == Some(studio_workspace::application::Application::Visualizer)
            && workspace.presentation.is_none()
        {
            workspace.presentation =
                Some(studio_workspace::presentation::VisualizerPresentation::legacy(&workspace));
        }
        if self.closed {
            return self.fail("Workspace controller is shut down");
        }
        if &workspace == self.current() {
            return Ok(false);
        }
        workspace.validate().map_err(|e| e.to_string())?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("Workspace revision IDs exhausted")?;
        self.invalidate_context()?;
        self.document.edit(workspace).map_err(|e| e.to_string())?;
        self.revision = revision;
        Ok(true)
    }
    pub fn open(&mut self) -> Result<(), String> {
        self.available()?;
        self.invalidate_context()?;
        match self.dialog.open() {
            Ok(id) => {
                self.pending_dialog = Some(PendingDialog {
                    id,
                    kind: DialogKind::Open,
                    context: self.context,
                    snapshot: None,
                });
                self.status = "Choose a workspace to open".into();
                self.fallback = None;
                Ok(())
            }
            Err(error) => {
                if error == DialogError::Unsupported {
                    self.fallback = Some(DialogKind::Open);
                }
                self.fail(error)
            }
        }
    }
    pub fn save(&mut self) -> Result<(), String> {
        if let Some(path) = self.path.clone() {
            self.save_path(path)
        } else {
            self.save_as()
        }
    }
    pub fn save_as(&mut self) -> Result<(), String> {
        self.available()?;
        let snapshot = self.current().clone();
        match self.dialog.save("workspace.json") {
            Ok(id) => {
                self.pending_dialog = Some(PendingDialog {
                    id,
                    kind: DialogKind::Save,
                    context: self.context,
                    snapshot: Some(snapshot),
                });
                self.status = "Choose a workspace save location".into();
                self.fallback = None;
                Ok(())
            }
            Err(error) => {
                if error == DialogError::Unsupported {
                    self.fallback = Some(DialogKind::Save);
                }
                self.fail(error)
            }
        }
    }
    pub fn load_path(&mut self, path: impl Into<PathBuf>) -> Result<(), String> {
        self.available()?;
        self.invalidate_context()?;
        self.submit(IoOperation::Load { path: path.into() }, IoKind::Load)
    }
    pub fn save_path(&mut self, path: impl Into<PathBuf>) -> Result<(), String> {
        self.available()?;
        self.submit(
            IoOperation::save(path.into(), self.current().clone()),
            IoKind::Save,
        )
    }
    /// Review an inactive in-memory preset through the same dirty/cleanup gate.
    /// It has no source file; replacing configuration requires a later explicit action.
    pub fn preview_configuration(&mut self, workspace: Workspace) -> Result<(), String> {
        self.available()?;
        workspace.validate().map_err(|error| error.to_string())?;
        self.invalidate_context()?;
        self.candidate = Some(Candidate {
            workspace,
            path: None,
            issue: None,
            preview: None,
            excluded: Vec::new(),
        });
        self.review_candidate()
    }
    fn submit(&mut self, operation: IoOperation, kind: IoKind) -> Result<(), String> {
        match self.worker.submit(operation) {
            Ok(id) => {
                self.pending_io = Some(PendingIo {
                    id,
                    kind,
                    context: self.context,
                    cancelled: false,
                });
                self.status = match kind {
                    IoKind::Load => "Opening workspace",
                    IoKind::Save => "Saving workspace",
                }
                .into();
                self.fallback = None;
                Ok(())
            }
            Err(error) => self.fail(error),
        }
    }
    /// Nonblocking; root should request periodic repaint while is_busy().
    pub fn poll(&mut self) {
        if let Some(completion) = self.dialog.poll() {
            self.dialog_completed(completion);
        }
        if let Some(completion) = self.worker.poll() {
            self.io_completed(completion);
        }
    }
    /// Root routes this to egui's normal viewport Close command and on_exit cleanup.
    pub fn take_exit_request(&mut self) -> bool {
        self.dialog.take_exit_request()
    }
    fn dialog_completed(&mut self, completion: DialogCompletion) {
        let Some(pending) = self.pending_dialog.take() else {
            return;
        };
        if pending.id != completion.id || pending.kind != completion.kind {
            self.pending_dialog = Some(pending);
            return;
        }
        if self.closed || (pending.kind == DialogKind::Open && pending.context != self.context) {
            return;
        }
        match completion.result {
            Ok(Some(path)) => {
                let operation = match pending.kind {
                    DialogKind::Open => IoOperation::Load { path },
                    DialogKind::Save => {
                        IoOperation::save(path, pending.snapshot.expect("save snapshot"))
                    }
                };
                let kind = match pending.kind {
                    DialogKind::Open => IoKind::Load,
                    DialogKind::Save => IoKind::Save,
                };
                let _ = self.submit(operation, kind);
            }
            Ok(None) => self.status = "File selection canceled".into(),
            Err(error) => {
                let _: Result<(), _> = self.fail(error);
            }
        }
    }
    fn io_completed(&mut self, completion: IoCompletion) {
        let Some(pending) = self.pending_io.take() else {
            return;
        };
        if pending.id != completion.id || pending.kind != completion.kind {
            self.pending_io = Some(pending);
            return;
        }
        let cancelled = pending.cancelled || completion.cancelled || self.closed;
        if pending.kind == IoKind::Load && (cancelled || pending.context != self.context) {
            self.status = "Open canceled; current workspace retained".into();
            return;
        }
        if pending.kind == IoKind::Save && cancelled {
            self.status =
                "Save canceled; destination may have changed. Check the file before retrying."
                    .into();
            self.error = Some(self.status.clone());
            return;
        }
        match completion.result {
            Ok(IoOutput::Imported {
                workspace,
                excluded,
                legacy,
            }) if pending.kind == IoKind::Load => {
                self.candidate = Some(Candidate {
                    workspace: *workspace,
                    path: if legacy { None } else { Some(completion.path) },
                    issue: None,
                    preview: None,
                    excluded,
                });
                let _ = self.review_candidate();
            }
            Ok(IoOutput::Loaded(workspace)) if pending.kind == IoKind::Load => {
                self.candidate = Some(Candidate {
                    workspace: *workspace,
                    path: Some(completion.path),
                    issue: None,
                    preview: None,
                    excluded: Vec::new(),
                });
                let _ = self.review_candidate();
            }
            Ok(IoOutput::Saved(snapshot)) if pending.kind == IoKind::Save => {
                match self.document.mark_saved(*snapshot) {
                    Ok(()) => {
                        self.path = Some(completion.path);
                        self.status = if self.dirty() {
                            "Snapshot saved; newer edits are unsaved"
                        } else {
                            "Workspace saved"
                        }
                        .into();
                    }
                    Err(error) => {
                        let _: Result<(), _> = self.fail(error);
                    }
                }
            }
            Ok(IoOutput::CancelledBeforeStart) => {
                self.status = "File operation canceled before it started".into()
            }
            Ok(_) => {
                let _: Result<(), _> = self.fail("Workspace worker returned an unexpected result");
            }
            Err(error) => {
                self.status = match pending.kind {
                    IoKind::Load => "Workspace open failed; current workspace retained",
                    IoKind::Save => {
                        "Workspace save failed; review the destination and retry explicitly"
                    }
                }
                .into();
                let _: Result<(), _> = self.fail(error);
            }
        }
    }
    /// Explicit review after configuration edits; never emits Stop or activation.
    pub fn review_candidate(&mut self) -> Result<(), String> {
        if self.closed {
            return self.fail("Workspace controller is shut down");
        }
        let Some(candidate) = &mut self.candidate else {
            return self.fail("Open a workspace before reviewing it");
        };
        match self
            .restore
            .preview(candidate.workspace.clone(), self.revision, self.supported)
        {
            Ok(id) => {
                candidate.preview = Some(id);
                candidate.issue = None;
                self.status = "Review the inactive workspace before replacing configuration".into();
                Ok(())
            }
            Err(error) => {
                let error = error.to_string();
                candidate.preview = None;
                candidate.issue = Some(error.clone());
                self.fail(error)
            }
        }
    }
    pub fn begin_apply(
        &mut self,
        approval: ApplyApproval,
        owners: &[HandleIdentity],
    ) -> Result<Vec<StopIntent>, String> {
        if self.closed || self.is_busy() {
            return self.fail("Finish the current file operation before replacing configuration");
        }
        let id = self
            .candidate
            .as_ref()
            .and_then(|c| c.preview)
            .ok_or("Review a supported workspace before replacing configuration")?;
        match self
            .restore
            .apply(id, self.revision, self.dirty(), approval, owners)
        {
            Ok(stops) => Ok(stops),
            Err(error) => self.fail(error),
        }
    }
    pub fn cleanup_confirmed(&mut self, owner: HandleIdentity) -> Result<bool, String> {
        self.restore
            .cleanup_confirmed(owner, self.revision)
            .map_err(|e| e.to_string())
    }
    pub fn stop_failed(&mut self, owner: HandleIdentity, message: &str) -> Result<bool, String> {
        self.restore
            .stop_failed(owner, self.revision, message)
            .map_err(|e| e.to_string())
    }
    pub fn retry_failed(&mut self) -> Result<Vec<StopIntent>, String> {
        let id = self
            .candidate
            .as_ref()
            .and_then(|c| c.preview)
            .ok_or("Review the workspace before retrying")?;
        self.restore
            .retry_failed(id, self.revision)
            .map_err(|e| e.to_string())
    }
    /// Root must synchronously apply this inactive token to panels in the same frame.
    pub fn take_ready(
        &mut self,
        owners: &[HandleIdentity],
    ) -> Result<Option<InactiveRestore>, String> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("Workspace revision IDs exhausted")?;
        let context = self
            .context
            .checked_add(1)
            .ok_or("Workspace context IDs exhausted")?;
        let Some(restore) = self
            .restore
            .take_ready(self.revision, owners)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        self.path = self.candidate.take().and_then(|c| c.path);
        if self.path.is_some() {
            self.document.apply_inactive(restore.clone());
        } else {
            // A preset is a new unsaved document, not a successful disk save.
            self.document = Document::new(restore.configuration().clone())
                .expect("restore token contains validated configuration");
        }
        self.revision = revision;
        self.context = context;
        self.status = if self.application == Some(studio_workspace::application::Application::Tools) {
            "Workspace restored inactive; reconnect and activate tasks explicitly"
        } else {
            "Workspace restored inactive; enable displays explicitly and verify Spatial clocks again"
        }
        .into();
        Ok(Some(restore))
    }
    pub fn cancel(&mut self) {
        self.dialog.cancel();
        if let Some(pending) = &mut self.pending_io {
            pending.cancelled = true;
            self.worker.cancel(pending.id);
        }
        let _ = self.invalidate_context();
        self.status = "Workspace operation canceled".into();
    }
    pub fn disconnect(&mut self) {
        self.dialog.cancel();
        let _ = self.invalidate_context();
        self.candidate = None;
        self.status = "Workspace configuration retained after connection change".into();
    }
    /// Nonblocking cancellation for ordinary lifecycle handling.
    pub fn request_shutdown(&mut self) {
        self.closed = true;
        self.dialog.close();
        self.pending_dialog = None;
        if let Some(pending) = &mut self.pending_io {
            pending.cancelled = true;
        }
        self.restore.disconnected();
        self.worker.request_shutdown();
    }
    /// May wait on filesystem work: call only from the native exit callback.
    pub fn shutdown(&mut self) {
        self.request_shutdown();
        self.worker.finish_shutdown();
    }
}

#[cfg(test)]
mod tests;
