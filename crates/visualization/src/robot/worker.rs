//! Bounded local-asset executor. Start/cancel/poll never wait for filesystem work.
use super::{
    load_model, load_model_from_text, LoadOptions, RobotError, RobotErrorKind, RobotModel,
};
use crate::spatial::robot_geometry::{prepare_robot, PreparedRobot};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};

static NEXT_LOAD: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadId(pub u64);
#[derive(Clone, Debug)]
pub struct LoadedRobot {
    pub model: Arc<RobotModel>,
    pub prepared: Arc<PreparedRobot>,
}
#[derive(Debug)]
pub struct LoadCompletion {
    pub id: LoadId,
    pub result: Result<LoadedRobot, RobotError>,
}
#[derive(Clone, Debug)]
pub enum LoadSource {
    File(PathBuf),
    Description {
        text: Arc<str>,
        asset_root: Option<PathBuf>,
    },
}
struct Job {
    id: LoadId,
    source: LoadSource,
    options: LoadOptions,
    cancel: Arc<AtomicBool>,
}
struct Pending {
    id: LoadId,
    cancel: Arc<AtomicBool>,
}
fn error(kind: RobotErrorKind, message: &str) -> RobotError {
    RobotError::new(kind, message)
}
fn load_prepare(
    source: &LoadSource,
    options: &LoadOptions,
    cancel: &AtomicBool,
) -> Result<LoadedRobot, RobotError> {
    let model = match source {
        LoadSource::File(path) => load_model(path, options, cancel)?,
        LoadSource::Description { text, asset_root } => {
            load_model_from_text(text, asset_root.as_deref(), options, cancel)?
        }
    };
    let prepared = prepare_robot(&model, cancel)?;
    Ok(LoadedRobot {
        model: Arc::new(model),
        prepared: Arc::new(prepared),
    })
}

pub struct AssetWorker {
    jobs: Option<SyncSender<Job>>,
    results: Receiver<LoadCompletion>,
    pending: Option<Pending>,
    join: Option<JoinHandle<()>>,
}
impl AssetWorker {
    pub fn new() -> Result<Self, RobotError> {
        Self::with_source_loader(load_prepare)
    }
    #[cfg(test)]
    fn with_loader(
        loader: impl Fn(&std::path::Path, &LoadOptions, &AtomicBool) -> Result<LoadedRobot, RobotError>
            + Send
            + 'static,
    ) -> Result<Self, RobotError> {
        Self::with_source_loader(move |source, options, cancel| match source {
            LoadSource::File(path) => loader(path, options, cancel),
            _ => panic!("File-only injected test loader"),
        })
    }
    fn with_source_loader(
        loader: impl Fn(&LoadSource, &LoadOptions, &AtomicBool) -> Result<LoadedRobot, RobotError>
            + Send
            + 'static,
    ) -> Result<Self, RobotError> {
        let (jobs, requests) = mpsc::sync_channel::<Job>(1);
        let (responses, results) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("robot-assets".into())
            .spawn(move || {
                while let Ok(job) = requests.recv() {
                    let result = if job.cancel.load(Ordering::Acquire) {
                        Err(error(RobotErrorKind::Cancelled, "Robot load cancelled"))
                    } else {
                        // A failed decoder must not leave an active UI permanently Busy.
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            loader(&job.source, &job.options, &job.cancel)
                        }))
                        .unwrap_or_else(|_| {
                            Err(error(
                                RobotErrorKind::Io,
                                "Robot asset worker failed; retry loading explicitly",
                            ))
                        })
                    };
                    if responses
                        .try_send(LoadCompletion { id: job.id, result })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| {
                RobotError::new(
                    RobotErrorKind::Io,
                    format!("Cannot start robot asset worker: {e}"),
                )
            })?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            pending: None,
            join: Some(join),
        })
    }
    /// Exactly one in-flight operation, including an unpolled completion. A
    /// replacement must cancel, observe completion, then start a new operation.
    pub fn start(&mut self, path: PathBuf, options: LoadOptions) -> Result<LoadId, RobotError> {
        self.start_source(LoadSource::File(path), options)
    }
    pub fn start_source(
        &mut self,
        source: LoadSource,
        options: LoadOptions,
    ) -> Result<LoadId, RobotError> {
        // Reject oversize topic jobs before retaining them in the bounded mailbox.
        if matches!(&source, LoadSource::Description { text, .. } if text.len() > super::MAX_URDF_BYTES)
        {
            return Err(error(
                RobotErrorKind::Limit,
                "Robot description exceeds 1 MiB",
            ));
        }
        let jobs = self
            .jobs
            .as_ref()
            .ok_or_else(|| error(RobotErrorKind::Io, "Robot asset worker is stopped"))?;
        if self.pending.is_some() {
            return Err(error(
                RobotErrorKind::Busy,
                "Wait for the current robot load to finish cancelling before replacing it",
            ));
        }
        let id = NEXT_LOAD
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(LoadId)
            .map_err(|_| {
                error(
                    RobotErrorKind::Limit,
                    "Robot load identifiers exhausted; restart the app",
                )
            })?;
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.try_send(Job {
            id,
            source,
            options,
            cancel: cancel.clone(),
        })
        .map_err(|_| {
            error(
                RobotErrorKind::Io,
                "Robot asset worker is unavailable; retry explicitly",
            )
        })?;
        self.pending = Some(Pending { id, cancel });
        Ok(id)
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn cancel(&mut self, id: LoadId) -> bool {
        if let Some(pending) = &self.pending {
            if pending.id == id {
                pending.cancel.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }
    /// Nonblocking; cancellation also suppresses an already queued success.
    pub fn poll(&mut self) -> Option<LoadCompletion> {
        let mut completion = match self.results.try_recv() {
            Ok(completion) => completion,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                self.jobs = None;
                let pending = self.pending.take()?;
                return Some(LoadCompletion {
                    id: pending.id,
                    result: Err(error(
                        if pending.cancel.load(Ordering::Acquire) {
                            RobotErrorKind::Cancelled
                        } else {
                            RobotErrorKind::Io
                        },
                        "Robot asset worker ended before returning its result; retry explicitly",
                    )),
                });
            }
        };
        let pending = self.pending.as_ref()?;
        if pending.id != completion.id {
            return None; // Never clear a new pending operation for stale work.
        }
        let pending = self.pending.take().unwrap();
        if pending.cancel.load(Ordering::Acquire) {
            completion.result = Err(error(
                RobotErrorKind::Cancelled,
                "Robot load cancelled; late result discarded",
            ));
        }
        Some(completion)
    }
    pub fn request_shutdown(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.store(true, Ordering::Release);
        }
        self.jobs = None;
    }
    /// Off active UI frames only: joining may wait for an OS filesystem call
    /// that cooperative cancellation cannot interrupt. Native exit owns this.
    pub fn finish_shutdown(&mut self) -> Result<(), RobotError> {
        self.request_shutdown();
        let outcome = self.join.take().map(|join| join.join());
        self.pending = None;
        while self.results.try_recv().is_ok() {}
        match outcome {
            Some(Err(_)) => Err(error(
                RobotErrorKind::Io,
                "Robot asset worker failed during shutdown",
            )),
            _ => Ok(()),
        }
    }
}
impl Drop for AssetWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn loaded() -> LoadedRobot {
        LoadedRobot {
            model: Arc::new(RobotModel {
                name: "queued-success".into(),
                links: vec![super::super::RobotLink {
                    name: "base".into(),
                    visuals: vec![],
                }],
                joints: vec![],
                root_link: 0,
            }),
            prepared: Arc::new(PreparedRobot {
                link_names: vec!["base".into()],
                visuals: vec![],
                triangle_count: 0,
            }),
        }
    }
    fn wait(worker: &mut AssetWorker) -> LoadCompletion {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(completion) = worker.poll() {
                return completion;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not complete"
            );
            thread::yield_now();
        }
    }
    fn denied(_: &Path, _: &LoadOptions, _: &AtomicBool) -> Result<LoadedRobot, RobotError> {
        Err(error(RobotErrorKind::Asset, "test asset unavailable"))
    }
    #[test]
    fn gated_job_keeps_single_owner_until_completion_and_cancellation_is_correlated() {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let mut worker = AssetWorker::with_loader(move |path, options, cancel| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            denied(path, options, cancel)
        })
        .unwrap();
        let id = worker
            .start("first.urdf".into(), LoadOptions::default())
            .unwrap();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            worker
                .start("second.urdf".into(), LoadOptions::default())
                .unwrap_err()
                .kind,
            RobotErrorKind::Busy
        );
        assert!(!worker.cancel(LoadId(id.0 + 1)));
        assert!(worker.cancel(id));
        assert!(worker.is_busy());
        release_tx.send(()).unwrap();
        let completion = worker
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(completion.id, id);
        // Requeue the observed result to exercise cancellation at the poll
        // boundary deterministically, without relying on a timing-only sleep.
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(completion).unwrap();
        worker.results = rx;
        let completion = worker.poll().unwrap();
        assert_eq!(
            completion.result.unwrap_err().kind,
            RobotErrorKind::Cancelled
        );
        assert!(!worker.is_busy());
        worker.finish_shutdown().unwrap();
    }
    #[test]
    fn topic_text_uses_the_same_cancellable_asset_preparation_worker() {
        let mut worker = AssetWorker::new().unwrap();
        let text: Arc<str> = Arc::from(
            r#"<robot name="topic"><link name="base"><visual><geometry><box size="1 2 3"/></geometry></visual></link></robot>"#,
        );
        let id = worker
            .start_source(
                LoadSource::Description {
                    text: text.clone(),
                    asset_root: None,
                },
                LoadOptions::default(),
            )
            .unwrap();
        let completion = wait(&mut worker);
        assert_eq!(completion.id, id);
        let loaded = completion.result.unwrap();
        assert_eq!(loaded.model.name, "topic");
        assert_eq!(loaded.prepared.triangle_count, 12);
        assert_eq!(loaded.prepared.link_names, ["base"]);
        let id = worker
            .start_source(
                LoadSource::Description {
                    text,
                    asset_root: None,
                },
                LoadOptions::default(),
            )
            .unwrap();
        assert!(worker.cancel(id));
        assert_eq!(
            wait(&mut worker).result.unwrap_err().kind,
            RobotErrorKind::Cancelled
        );
        assert!(worker
            .start_source(
                LoadSource::Description {
                    text: Arc::from("x".repeat(super::super::MAX_URDF_BYTES + 1)),
                    asset_root: None
                },
                LoadOptions::default()
            )
            .is_err());
        assert!(!worker.is_busy());
        worker.finish_shutdown().unwrap();
    }
    #[test]
    fn shutdown_is_idempotent_and_rejects_future_jobs() {
        let mut worker = AssetWorker::with_loader(denied).unwrap();
        worker.request_shutdown();
        assert!(worker
            .start("late.urdf".into(), LoadOptions::default())
            .is_err());
        worker.finish_shutdown().unwrap();
        worker.finish_shutdown().unwrap();
        assert!(!worker.is_busy());
    }
    #[test]
    fn cancellation_after_success_was_queued_cannot_publish_the_old_model() {
        let mut worker = AssetWorker::with_loader(|_, _, _| Ok(loaded())).unwrap();
        let id = worker
            .start("queued.urdf".into(), LoadOptions::default())
            .unwrap();
        let completion = worker
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(completion.id, id);
        assert!(
            completion.result.is_ok(),
            "the real worker already produced success"
        );
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(completion).unwrap();
        worker.results = rx;
        assert!(worker.cancel(id));
        assert_eq!(
            worker.poll().unwrap().result.unwrap_err().kind,
            RobotErrorKind::Cancelled
        );
        assert!(worker.poll().is_none());
        worker.finish_shutdown().unwrap();
    }
    #[test]
    fn decoder_panic_returns_a_correlated_failure_then_explicit_retry_can_succeed() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let mut worker = AssetWorker::with_loader(move |_, _, _| {
            if count.fetch_add(1, Ordering::Relaxed) == 0 {
                panic!("injected decoder failure");
            }
            Ok(loaded())
        })
        .unwrap();
        let first = worker
            .start("panic.urdf".into(), LoadOptions::default())
            .unwrap();
        let completion = wait(&mut worker);
        assert_eq!(completion.id, first);
        assert_eq!(completion.result.unwrap_err().kind, RobotErrorKind::Io);
        let retry = worker
            .start("retry.urdf".into(), LoadOptions::default())
            .unwrap();
        assert_ne!(retry, first);
        let completion = wait(&mut worker);
        assert_eq!(completion.id, retry);
        assert_eq!(completion.result.unwrap().model.name, "queued-success");
        worker.finish_shutdown().unwrap();
    }
}
