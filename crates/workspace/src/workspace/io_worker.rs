//! Bounded local file I/O off UI frames. Cancellation never rolls back a write.
use super::{load, save_atomic, Error, Workspace};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoKind {
    Load,
    Save,
}
#[derive(Debug)]
pub enum IoOperation {
    Load {
        path: PathBuf,
    },
    Save {
        path: PathBuf,
        snapshot: Box<Workspace>,
    },
}
impl IoOperation {
    pub fn save(path: PathBuf, snapshot: Workspace) -> Self {
        Self::Save {
            path,
            snapshot: Box::new(snapshot),
        }
    }
    pub fn load(path: PathBuf) -> Self {
        Self::Load { path }
    }
    fn path(&self) -> &PathBuf {
        match self {
            Self::Load { path } | Self::Save { path, .. } => path,
        }
    }
    fn kind(&self) -> IoKind {
        match self {
            Self::Load { .. } => IoKind::Load,
            Self::Save { .. } => IoKind::Save,
        }
    }
}
#[derive(Debug)]
pub enum IoOutput {
    Loaded(Box<Workspace>),
    Imported {
        workspace: Box<Workspace>,
        excluded: Vec<String>,
        legacy: bool,
    },
    /// Exact successfully written snapshot, including when edits happened meanwhile.
    Saved(Box<Workspace>),
    /// Cancellation was observed before any storage operation began.
    CancelledBeforeStart,
}
#[derive(Debug)]
pub struct IoCompletion {
    pub id: OperationId,
    pub path: PathBuf,
    pub kind: IoKind,
    /// Cancellation suppresses applying this result; it does NOT mean a save was undone.
    pub cancelled: bool,
    pub result: Result<IoOutput, Error>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    Closed,
    Invalid(String),
    IdExhausted,
}
impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("A workspace file operation is still pending"),
            Self::Closed => f.write_str("Workspace file worker is closed"),
            Self::Invalid(message) => f.write_str(message),
            Self::IdExhausted => f.write_str("Workspace operation IDs exhausted"),
        }
    }
}
impl std::error::Error for SubmitError {}
struct Job {
    id: OperationId,
    operation: IoOperation,
    cancel: Arc<AtomicBool>,
}
struct Pending {
    id: OperationId,
    path: PathBuf,
    kind: IoKind,
    cancel: Arc<AtomicBool>,
}

/// At most one outstanding operation, one job slot and one completion slot.
/// No filesystem operations or joins happen in submit/poll/cancel/Drop.
/// Keep one worker for the application lifetime. Request shutdown on close;
/// call finish_shutdown only after active UI frames have ended.
pub struct IoWorker {
    jobs: Option<mpsc::SyncSender<Job>>,
    results: mpsc::Receiver<IoCompletion>,
    pending: Option<Pending>,
    join: Option<JoinHandle<()>>,
}
impl Default for IoWorker {
    fn default() -> Self {
        Self::new()
    }
}
impl IoWorker {
    pub fn new() -> Self {
        Self::with_executor(execute)
    }
    pub fn for_application(application: crate::application::Application) -> Self {
        Self::with_executor(move |operation| match operation {
            IoOperation::Load { path } => {
                let imported = crate::application::parse(
                    &super::storage::read_bytes_limit(
                        &path,
                        if application == crate::application::Application::Tools {
                            crate::tools::MAX_FILE_BYTES
                        } else {
                            super::MAX_FILE_BYTES
                        },
                    )?,
                    application,
                )?;
                Ok(IoOutput::Imported {
                    workspace: Box::new(imported.workspace),
                    excluded: imported.excluded,
                    legacy: imported.legacy,
                })
            }
            IoOperation::Save { path, mut snapshot } => {
                if application == crate::application::Application::Visualizer
                    && snapshot.presentation.is_none()
                {
                    snapshot.presentation = Some(
                        crate::presentation::VisualizerPresentation::legacy(&snapshot),
                    );
                }
                let bytes = crate::application::encode(&snapshot, application)?;
                super::storage::save_bytes(&path, &bytes)?;
                Ok(IoOutput::Saved(snapshot))
            }
        })
    }
    fn with_executor(
        mut execute: impl FnMut(IoOperation) -> Result<IoOutput, Error> + Send + 'static,
    ) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::sync_channel::<Job>(1);
        let (results_tx, results) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            while let Ok(job) = jobs_rx.recv() {
                let path = job.operation.path().clone();
                let kind = job.operation.kind();
                let result = if job.cancel.load(Ordering::Acquire) {
                    Ok(IoOutput::CancelledBeforeStart)
                } else {
                    execute(job.operation)
                };
                let completion = IoCompletion {
                    id: job.id,
                    path,
                    kind,
                    cancelled: job.cancel.load(Ordering::Acquire),
                    result,
                };
                if results_tx.send(completion).is_err() {
                    break;
                }
            }
        });
        Self {
            jobs: Some(jobs_tx),
            results,
            pending: None,
            join: Some(join),
        }
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn submit(&mut self, operation: IoOperation) -> Result<OperationId, SubmitError> {
        let sender = self.jobs.as_ref().ok_or(SubmitError::Closed)?;
        if self.is_busy() {
            return Err(SubmitError::Busy);
        }
        // Metadata/configuration only: storage validation and all reads/writes run on worker.
        if operation.path().as_os_str().is_empty() || operation.path().as_os_str().len() > 4096 {
            return Err(SubmitError::Invalid(
                "Workspace path must contain 1–4096 bytes".into(),
            ));
        }
        if let IoOperation::Save { snapshot, .. } = &operation {
            snapshot
                .validate()
                .map_err(|error| SubmitError::Invalid(error.to_string()))?;
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let raw = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| SubmitError::IdExhausted)?;
        let id = OperationId(raw);
        let cancel = Arc::new(AtomicBool::new(false));
        let path = operation.path().clone();
        let kind = operation.kind();
        match sender.try_send(Job {
            id,
            operation,
            cancel: cancel.clone(),
        }) {
            Ok(()) => {
                self.pending = Some(Pending {
                    id,
                    path,
                    kind,
                    cancel,
                });
                Ok(id)
            }
            Err(mpsc::TrySendError::Full(_)) => Err(SubmitError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(SubmitError::Closed),
        }
    }
    pub fn cancel(&mut self, id: OperationId) -> bool {
        if let Some(pending) = &self.pending {
            if pending.id == id {
                pending.cancel.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }
    /// Nonblocking. A cancelled completion remains correlated and truthful: even
    /// cancellation after worker completion is reflected when the UI polls it.
    pub fn poll(&mut self) -> Option<IoCompletion> {
        let mut completion = match self.results.try_recv() {
            Ok(completion) => completion,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                // An exited worker must not leave the editor permanently Busy.
                // It may have exited after rename, so report an uncertain save
                // outcome rather than marking a snapshot saved or claiming rollback.
                self.jobs = None;
                let pending = self.pending.take()?;
                let message = match pending.kind {
                    IoKind::Save => "Workspace file worker ended before returning a result; the destination may have changed. Check it before retrying.",
                    IoKind::Load => "Workspace file worker ended before returning a result; retry explicitly.",
                };
                return Some(IoCompletion {
                    id: pending.id,
                    path: pending.path,
                    kind: pending.kind,
                    cancelled: pending.cancel.load(Ordering::Acquire),
                    result: Err(Error::Io(std::io::Error::other(message))),
                });
            }
        };
        if let Some(pending) = &self.pending {
            if pending.id == completion.id {
                completion.cancelled |= pending.cancel.load(Ordering::Acquire);
                self.pending = None;
            }
        }
        Some(completion)
    }
    pub fn request_shutdown(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        self.jobs = None;
    }
    /// May wait for an in-progress OS filesystem operation; use only off active UI frames.
    /// Cancellation is cooperative and cannot interrupt/undo rename or a blocked disk call.
    pub fn finish_shutdown(&mut self) {
        self.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
impl Drop for IoWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}
fn execute(operation: IoOperation) -> Result<IoOutput, Error> {
    match operation {
        IoOperation::Load { path } => {
            load(&path).map(|workspace| IoOutput::Loaded(Box::new(workspace)))
        }
        IoOperation::Save { path, snapshot } => {
            save_atomic(&path, &snapshot)?;
            Ok(IoOutput::Saved(snapshot))
        }
    }
}
#[cfg(test)]
mod tests;
