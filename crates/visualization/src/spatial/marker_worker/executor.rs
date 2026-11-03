//! One active, one latest pending and one latest result; never join on polling.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
};
pub type Wake = Arc<dyn Fn() + Send + Sync>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Replacements {
    pub requests: u64,
    pub results: u64,
}
pub struct Completion<T> {
    pub id: u64,
    pub result: Result<T, String>,
}
struct Request<I> {
    id: u64,
    input: I,
    cancel: Arc<AtomicBool>,
}
struct State<I, O> {
    next: u64,
    closed: bool,
    active: Option<Arc<AtomicBool>>,
    pending: Option<Request<I>>,
    ready: Option<Completion<O>>,
    replacements: Replacements,
}
struct Shared<I, O> {
    state: Mutex<State<I, O>>,
    changed: Condvar,
}
pub(super) struct LatestWorker<I, O> {
    shared: Arc<Shared<I, O>>,
    join: Option<JoinHandle<()>>,
}
impl<I: Send + 'static, O: Send + 'static> LatestWorker<I, O> {
    pub fn new(
        name: &str,
        wake: Wake,
        mut run: impl FnMut(I, &AtomicBool) -> Result<O, String> + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next: 1,
                closed: false,
                active: None,
                pending: None,
                ready: None,
                replacements: Replacements::default(),
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        let join = thread::Builder::new()
            .name(name.into())
            .spawn(move || loop {
                let request = {
                    let mut state = worker.state.lock().unwrap();
                    while !state.closed && state.pending.is_none() {
                        state = worker.changed.wait(state).unwrap();
                    }
                    if state.closed {
                        break;
                    }
                    let request = state.pending.take().unwrap();
                    state.active = Some(request.cancel.clone());
                    request
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if request.cancel.load(Ordering::Acquire) {
                        Err("Marker worker cancelled".into())
                    } else {
                        run(request.input, &request.cancel)
                    }
                }))
                .unwrap_or_else(|_| Err("Marker worker failed; retry explicitly".into()));
                let (retired, notify) = {
                    let mut state = worker.state.lock().unwrap();
                    state.active = None;
                    if state.closed || request.cancel.load(Ordering::Acquire) {
                        (
                            Some(Completion {
                                id: request.id,
                                result,
                            }),
                            false,
                        )
                    } else {
                        let retired = state.ready.replace(Completion {
                            id: request.id,
                            result,
                        });
                        if retired.is_some() {
                            state.replacements.results =
                                state.replacements.results.saturating_add(1);
                        }
                        (retired, true)
                    }
                };
                // Large retired output is released off the UI and outside its lock.
                drop(retired);
                if notify {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake()));
                }
            })
            .map_err(|e| format!("Cannot start Marker worker: {e}"))?;
        Ok(Self {
            shared,
            join: Some(join),
        })
    }
    pub fn submit(&mut self, input: I) -> Result<u64, String> {
        let (id, retired) = {
            let mut state = self.shared.state.lock().unwrap();
            if state.closed {
                return Err("Marker worker is stopped".into());
            }
            let id = state.next;
            state.next = id
                .checked_add(1)
                .ok_or("Marker worker identifiers exhausted")?;
            let retired = state.pending.replace(Request {
                id,
                input,
                cancel: Arc::new(AtomicBool::new(false)),
            });
            if retired.is_some() {
                state.replacements.requests = state.replacements.requests.saturating_add(1);
            }
            (id, retired)
        };
        drop(retired);
        self.shared.changed.notify_one();
        Ok(id)
    }
    pub fn poll(&mut self) -> Option<Completion<O>> {
        self.shared.state.lock().unwrap().ready.take()
    }
    pub fn replacements(&self) -> Replacements {
        self.shared.state.lock().unwrap().replacements
    }
    pub fn cancel(&mut self) {
        let retired = {
            let mut state = self.shared.state.lock().unwrap();
            if let Some(active) = &state.active {
                active.store(true, Ordering::Release);
            }
            (state.pending.take(), state.ready.take())
        };
        drop(retired);
    }
    pub fn request_shutdown(&mut self) {
        self.shared.state.lock().unwrap().closed = true;
        self.cancel();
        self.shared.changed.notify_one();
    }
    pub fn finish_shutdown(&mut self) -> Result<(), String> {
        self.request_shutdown();
        self.join
            .take()
            .map(|j| j.join().map_err(|_| "Marker worker shutdown failed".into()))
            .unwrap_or(Ok(()))
    }
}
impl<I, O> Drop for LatestWorker<I, O> {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.closed = true;
        if let Some(active) = &state.active {
            active.store(true, Ordering::Release);
        }
        state.pending = None;
        state.ready = None;
        self.shared.changed.notify_one();
    }
}
