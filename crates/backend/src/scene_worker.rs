//! One worker owns the shared native TF readers/cache and every sensor reader.
//! Logical worker proxies wait only for bounded authoritative completion slots.
use crate::{
    cloud_worker::CloudState,
    mapping_worker::MappingState,
    observation::{EncodedPayload, Factory, Reader},
    robot_worker::RobotState,
    spatial_session::{ClockPolicy, SpatialSession, StreamKind, Transform},
    spatial_worker::ScanState,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    sync::{
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use studio_protocol::{
    spatial,
    spatial_input::{self, SpatialClockPolicy, TfStreamKind},
    structured,
    subscriptions::*,
};
const CONTROL_CAPACITY: usize = 16;
mod camera_display;
mod map_display;
mod marker_display;
mod pose_display;
pub(crate) type PoseBarrier = studio_protocol::pose_snapshot::transport::Invalidation;
type PoseClearReply = SyncSender<Result<PoseBarrier, String>>;
struct Completion {
    result: Mutex<Option<Result<(), String>>>,
    wake: Condvar,
}
impl Completion {
    fn finish(&self, result: Result<(), String>) {
        let mut slot = self.result.lock().unwrap();
        if slot.is_none() {
            *slot = Some(result);
            self.wake.notify_all();
        }
    }
}
#[derive(Clone)]
pub(crate) struct SceneService {
    inner: Arc<ServiceInner>,
}
struct ServiceInner {
    tx: SyncSender<Command>,
    shutdown: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
    frames: Arc<Mutex<Vec<String>>>,
}
/// Declared before Actor on its thread: Rust drops Actor and every native
/// owner, including its receiver, before publishing terminal completion.
struct ClosedOnExit(Arc<AtomicBool>);
impl Drop for ClosedOnExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Drop for ServiceInner {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.tx.try_send(Command::Shutdown);
        if let Some(join) = self.join.lock().unwrap().take() {
            let _ = join.join();
        }
    }
}
pub(crate) struct Attach {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub configuration: Configuration,
    pub ready: Arc<AtomicBool>,
    pub released: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub queue: Arc<Mutex<FairQueue<EncodedPayload>>>,
    pub spatial_counters: Option<Arc<Mutex<spatial::Counters>>>,
}
struct Request {
    attach: Attach,
    completion: Arc<Completion>,
}
enum Command {
    Attach(Box<Request>),
    Detach(Generation, SubscriptionId),
    ClearMarkers(Generation, SubscriptionId, SyncSender<Result<(), String>>),
    ClearPose(Generation, SubscriptionId, Arc<AtomicBool>, PoseClearReply),
    Shutdown,
}
impl SceneService {
    pub(crate) fn new(factory: Factory) -> Self {
        Self::spawn(factory, || {})
    }
    fn spawn(factory: Factory, after_drain: impl FnOnce() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel(CONTROL_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        let terminal = closed.clone();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let observed_frames = frames.clone();
        let join = thread::spawn(move || {
            let _closed_on_exit = ClosedOnExit(terminal);
            let mut actor = Actor {
                factory,
                rx,
                shutdown: flag,
                context: None,
                owners: Vec::new(),
                turn: 0,
                inflight: None,
                frames: observed_frames,
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| actor.run()));
            if result.is_err() {
                actor.fail_all("Shared TF scene worker panicked".into());
            }
            actor.context = None;
            if let Some(completion) = actor.inflight.take() {
                completion.finish(Err("Scene attach interrupted by worker failure".into()));
            }
            while let Ok(command) = actor.rx.try_recv() {
                if let Command::ClearMarkers(_, _, reply) = &command {
                    let _ = reply.try_send(Err("Scene worker stopped before Clear".into()));
                }
                if let Command::ClearPose(_, _, _, reply) = &command {
                    let _ = reply.try_send(Err("Scene worker stopped before pose Clear".into()));
                }
                if let Command::Attach(request) = command {
                    request
                        .completion
                        .finish(Err("Scene worker stopped before attach".into()));
                }
            }
            // A producer can still enqueue after the last Empty result.
            // The exit flag covers that request only after Actor is dropped.
            after_drain();
        });
        Self {
            inner: Arc::new(ServiceInner {
                tx,
                shutdown,
                closed,
                join: Mutex::new(Some(join)),
                frames,
            }),
        }
    }
    pub(crate) fn observed_frames(&self) -> Vec<String> {
        self.inner.frames.lock().unwrap().clone()
    }
    pub(crate) fn attach_and_wait(&self, attach: Attach) -> Result<(), String> {
        let completion = Arc::new(Completion {
            result: Mutex::new(None),
            wake: Condvar::new(),
        });
        let cancel = attach.cancel.clone();
        let generation = attach.generation;
        let subscription = attach.subscription;
        self.inner
            .tx
            .try_send(Command::Attach(Box::new(Request {
                attach,
                completion: completion.clone(),
            })))
            .map_err(|_| "Bounded scene attach lane unavailable; retry explicitly".to_string())?;
        self.wait_for_completion(completion, cancel, generation, subscription)
    }
    pub(crate) fn clear_markers(
        &self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<mpsc::Receiver<Result<(), String>>, String> {
        let (reply, completion) = mpsc::sync_channel(1);
        self.inner
            .tx
            .try_send(Command::ClearMarkers(generation, subscription, reply))
            .map_err(|_| "Bounded scene Clear lane unavailable; retry explicitly".to_string())?;
        Ok(completion)
    }
    pub(crate) fn clear_pose(
        &self,
        generation: Generation,
        subscription: SubscriptionId,
        gate: Arc<AtomicBool>,
    ) -> Result<mpsc::Receiver<Result<PoseBarrier, String>>, String> {
        let (reply, completion) = mpsc::sync_channel(1);
        self.inner
            .tx
            .try_send(Command::ClearPose(generation, subscription, gate, reply))
            .map_err(|_| "Bounded pose Clear lane unavailable; retry explicitly".to_string())?;
        Ok(completion)
    }
    fn wait_for_completion(
        &self,
        completion: Arc<Completion>,
        cancel: Arc<AtomicBool>,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<(), String> {
        let mut detached = false;
        let mut slot = completion.result.lock().unwrap();
        loop {
            if let Some(result) = slot.take() {
                return result;
            }
            if self.inner.closed.load(Ordering::Acquire) {
                return Err("Scene worker exited before attach completion".into());
            }
            if cancel.load(Ordering::Acquire) && !detached {
                // Cancellation is also directly visible to the actor; a full
                // control lane cannot lose this owner's stop request.
                detached = self
                    .inner
                    .tx
                    .try_send(Command::Detach(generation, subscription))
                    .is_ok();
            }
            slot = completion
                .wake
                .wait_timeout(slot, Duration::from_millis(5))
                .unwrap()
                .0;
        }
    }
}
struct Context {
    key: SceneKey,
    inputs: Vec<(Configuration, Box<dyn Reader>)>,
    tf: SpatialSession,
    counters: spatial::Counters,
}
impl Context {
    fn open(key: SceneKey, factory: &Factory, cancel: &AtomicBool) -> Result<Self, String> {
        let mut tf = SpatialSession::new().map_err(|e| e.message)?;
        tf.activate_explicit(ClockPolicy::VerifiedSystem)
            .map_err(|e| e.message)?;
        let configs = [
            (&key.static_tf_topic, false, true),
            (&key.dynamic_tf_topic, key.dynamic_tf_best_effort, false),
        ];
        let mut inputs = Vec::with_capacity(2);
        for (topic, best_effort, transient_local) in configs {
            if cancel.load(Ordering::Acquire) {
                return Err("Shared TF startup canceled".into());
            }
            let configuration = Configuration {
                map_updates: None,
                topic: topic.clone(),
                message_type: spatial_input::TF_MESSAGE_TYPE.into(),
                best_effort,
                transient_local,
                output: Output::Typed,
                spatial: None,
                robot: None,
            };
            let reader = factory(&configuration)?;
            inputs.push((configuration, reader));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Shared TF startup canceled".into());
        }
        Ok(Self {
            key,
            inputs,
            tf,
            counters: spatial::Counters::default(),
        })
    }
    fn poll(&mut self) -> Result<(), String> {
        for index in 0..2 {
            let Some(value) = self.inputs[index].1.poll_now()? else {
                continue;
            };
            if index == 0 {
                self.counters.tf_static_batches = self.counters.tf_static_batches.saturating_add(1);
            } else {
                self.counters.tf_dynamic_batches =
                    self.counters.tf_dynamic_batches.saturating_add(1);
            }
            let config = &self.inputs[index].0;
            let sample = structured::Sample {
                contract_version: structured::CONTRACT_VERSION,
                topic: config.topic.clone(),
                message_type: config.message_type.clone(),
                source_time: None,
                value,
            };
            let batch = spatial_input::decode_tf_message(
                &sample,
                if index == 0 {
                    TfStreamKind::Static
                } else {
                    TfStreamKind::Dynamic
                },
                SpatialClockPolicy::VerifiedSystem,
            )
            .map_err(|e| {
                self.counters.tf_rejected = self.counters.tf_rejected.saturating_add(1);
                e.to_string()
            })?;
            let transforms: Vec<_> = batch
                .transforms
                .into_iter()
                .map(|t| Transform {
                    parent: t.parent,
                    child: t.child,
                    stamp_ns: t.stamp_ns,
                    components: t.components,
                })
                .collect();
            let result = self.tf.insert_live_batch(
                if index == 0 {
                    StreamKind::Static
                } else {
                    StreamKind::Dynamic
                },
                &transforms,
            );
            let c = self.tf.counters();
            self.counters.tf_accepted = c.accepted;
            self.counters.tf_rejected = c.rejected;
            self.counters.duplicate_statics = c.duplicate_statics;
            self.counters.native_errors = c.native_errors;
            result.map_err(|e| format!("Shared TF {:?}: {}", e.status, e.message))?;
        }
        Ok(())
    }
}
enum Display {
    Camera(Box<camera_display::CameraDisplay>),
    MarkerSnapshot(Box<marker_display::MarkerDisplay>),
    PoseSnapshot(Box<pose_display::PoseDisplay>),
    MapSnapshot(Box<map_display::MapDisplay>),
    Robot(Box<RobotState>),
    TfGraph {
        next: Option<Instant>,
    },
    Scan {
        reader: Box<dyn Reader>,
        state: Box<ScanState>,
    },
    Cloud {
        reader: Box<dyn Reader>,
        state: Box<CloudState>,
    },
    Mapping {
        reader: Box<dyn Reader>,
        state: Box<MappingState>,
    },
}
struct Owner {
    request: Request,
    display: Display,
    sequence: u64,
}
struct Actor {
    factory: Factory,
    rx: Receiver<Command>,
    shutdown: Arc<AtomicBool>,
    context: Option<Context>,
    owners: Vec<Owner>,
    turn: usize,
    inflight: Option<Arc<Completion>>,
    frames: Arc<Mutex<Vec<String>>>,
}
impl Actor {
    fn attach(&mut self, request: Request) {
        let completion = request.completion.clone();
        let result = (|| {
            let a = &request.attach;
            if a.cancel.load(Ordering::Acquire) {
                return Err("Scene attach canceled".into());
            }
            let mut configurations: Vec<_> = self
                .owners
                .iter()
                .map(|o| &o.request.attach.configuration)
                .collect();
            configurations.push(&a.configuration);
            resource_usage(&configurations).map_err(str::to_string)?;
            let key = a
                .configuration
                .scene_key()
                .ok_or("Scene attach requires TF configuration")?;
            if self.context.as_ref().is_some_and(|c| c.key != key) {
                return Err("Incompatible shared scene TF source".into());
            }
            if self.context.is_none() {
                self.context = Some(Context::open(key, &self.factory, &a.cancel)?);
            }
            let display = match a.configuration.output {
                Output::CameraSnapshot(_) => Display::Camera(Box::new(
                    camera_display::CameraDisplay::new(a, &self.factory)?,
                )),
                Output::PoseSnapshot(_) => Display::PoseSnapshot(Box::new(
                    pose_display::PoseDisplay::new(a, &self.factory)?,
                )),
                Output::MarkerSnapshot => Display::MarkerSnapshot(Box::new(
                    marker_display::MarkerDisplay::new(a, &self.factory)?,
                )),
                Output::MapSnapshot => {
                    Display::MapSnapshot(Box::new(map_display::MapDisplay::new(a, &self.factory)?))
                }
                Output::TfGraph => Display::TfGraph { next: None },
                Output::Robot => Display::Robot(Box::new(RobotState::new(&a.configuration)?)),
                Output::SpatialScan => Display::Scan {
                    reader: (self.factory)(&a.configuration)?,
                    state: Box::new(ScanState::new(&a.configuration)?),
                },
                Output::PointCloud => Display::Cloud {
                    reader: (self.factory)(&a.configuration)?,
                    state: Box::new(CloudState::new(&a.configuration)?),
                },
                Output::OccupancyGrid | Output::Pose | Output::Path => Display::Mapping {
                    reader: (self.factory)(&a.configuration)?,
                    state: Box::new(MappingState::new(&a.configuration)?),
                },
                _ => return Err("Unsupported scene display".into()),
            };
            if a.cancel.load(Ordering::Acquire) {
                drop(display);
                return Err("Scene attach canceled after native open".into());
            }
            self.owners.push(Owner {
                request,
                display,
                sequence: 0,
            });
            self.owners
                .last()
                .unwrap()
                .request
                .attach
                .ready
                .store(true, Ordering::Release);
            Ok(())
        })();
        if let Err(error) = result {
            if self.owners.is_empty() {
                self.context = None;
                self.frames.lock().unwrap().clear();
            }
            completion.finish(Err(error));
        }
    }
    fn finish_owner(&mut self, index: usize, result: Result<(), String>) {
        let owner = self.owners.remove(index);
        let completion = owner.request.completion.clone();
        drop(owner);
        if self.owners.is_empty() {
            self.context = None;
            self.frames.lock().unwrap().clear();
        }
        completion.finish(result);
    }
    fn fail_all(&mut self, error: String) {
        if let Some(context) = &self.context {
            for owner in &self.owners {
                if let (Display::Scan { state, .. }, Some(counters)) =
                    (&owner.display, &owner.request.attach.spatial_counters)
                {
                    *counters.lock().unwrap() = state.counters(&context.counters);
                }
            }
        }
        let owners = std::mem::take(&mut self.owners);
        let completions: Vec<_> = owners
            .iter()
            .map(|o| o.request.completion.clone())
            .collect();
        drop(owners);
        self.context = None;
        self.frames.lock().unwrap().clear();
        for completion in completions {
            completion.finish(Err(format!("Shared scene TF failure: {error}")));
        }
    }
    fn run(&mut self) {
        while !self.shutdown.load(Ordering::Acquire) {
            for _ in 0..CONTROL_CAPACITY {
                match self.rx.try_recv() {
                    Ok(Command::Attach(request)) => {
                        self.inflight = Some(request.completion.clone());
                        self.attach(*request);
                        self.inflight = None;
                    }
                    Ok(Command::ClearPose(g, s, gate, reply)) => {
                        let result = (|| {
                            let owner = self
                                .owners
                                .iter_mut()
                                .find(|o| {
                                    o.request.attach.generation == g
                                        && o.request.attach.subscription == s
                                })
                                .ok_or("Pose owner no longer active")?;
                            let attach = &owner.request.attach;
                            if attach.cancel.load(Ordering::Acquire)
                                || !attach.released.load(Ordering::Acquire)
                            {
                                return Err("Pose Clear canceled or not started".into());
                            }
                            match &mut owner.display {
                                Display::PoseSnapshot(state) => state.local_clear(attach, gate),
                                _ => Err("Clear requires pose output".into()),
                            }
                        })();
                        let _ = reply.try_send(result);
                    }
                    Ok(Command::ClearMarkers(g, s, reply)) => {
                        let result = (|| {
                            let owner = self
                                .owners
                                .iter_mut()
                                .find(|o| {
                                    o.request.attach.generation == g
                                        && o.request.attach.subscription == s
                                })
                                .ok_or("Marker owner no longer active")?;
                            let attach = &owner.request.attach;
                            if attach.cancel.load(Ordering::Acquire)
                                || !attach.released.load(Ordering::Acquire)
                            {
                                return Err("Marker Clear canceled or not started".to_string());
                            }
                            match &mut owner.display {
                                Display::MarkerSnapshot(state) => state.local_clear(attach),
                                _ => Err("Clear requires Marker output".into()),
                            }
                        })();
                        let _ = reply.try_send(result);
                    }
                    Ok(Command::Detach(g, s)) => {
                        for owner in &self.owners {
                            if owner.request.attach.generation == g
                                && owner.request.attach.subscription == s
                            {
                                owner.request.attach.cancel.store(true, Ordering::Release);
                            }
                        }
                    }
                    Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                        self.shutdown.store(true, Ordering::Release);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            let mut index = 0;
            while index < self.owners.len() {
                if self.owners[index]
                    .request
                    .attach
                    .cancel
                    .load(Ordering::Acquire)
                {
                    self.finish_owner(index, Ok(()));
                } else {
                    index += 1;
                }
            }
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }
            if let Some(context) = &mut self.context {
                if let Err(error) = context.poll() {
                    self.fail_all(error);
                    continue;
                }
            }
            {
                let mut observed = self.frames.lock().unwrap();
                if let Some(context) = &self.context {
                    let current = context.tf.frame_names();
                    if *observed != current {
                        *observed = current;
                    }
                } else {
                    observed.clear();
                }
            }
            let count = self.owners.len();
            if count > 0 {
                self.turn = (self.turn + 1) % count;
                self.owners.rotate_left(self.turn);
            }
            let mut index = 0;
            while index < self.owners.len() {
                let owner = &mut self.owners[index];
                let a = &owner.request.attach;
                if a.cancel.load(Ordering::Acquire) || !a.released.load(Ordering::Acquire) {
                    index += 1;
                    continue;
                }
                let context = self
                    .context
                    .as_mut()
                    .expect("Scene owners require shared TF");
                let now = Instant::now();
                let result = (|| -> Result<Option<EncodedPayload>, String> {
                    match &mut owner.display {
                        Display::Camera(state) => state.poll(a, context),
                        Display::MarkerSnapshot(state) => state.poll(a, context),
                        Display::PoseSnapshot(state) => state.poll(a, context),
                        Display::MapSnapshot(state) => state.poll(a, context, now),
                        Display::TfGraph { next } => {
                            if next.is_some_and(|n| now < n) {
                                return Ok(None);
                            }
                            let system_ns = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_err(|e| e.to_string())?
                                .as_nanos()
                                .try_into()
                                .map_err(|_| "System clock exceeds ROS Time range")?;
                            let config = a
                                .configuration
                                .spatial
                                .as_ref()
                                .ok_or("Missing TF graph configuration")?;
                            let sample = context
                                .tf
                                .graph_snapshot(&config.fixed_frame, system_ns, &a.cancel)
                                .map_err(|e| {
                                    format!("Shared TF graph {:?}: {}", e.status, e.message)
                                })?;
                            *next = Some(
                                Instant::now()
                                    + Duration::from_millis(
                                        1000 / u64::from(studio_protocol::tf_graph::MAX_HZ),
                                    ),
                            );
                            sample
                                .map(|mut sample| {
                                    sample.counters = context.counters;
                                    sample.encode().map(|bytes| {
                                        EncodedPayload::TfGraph(bytes.into_boxed_slice())
                                    })
                                })
                                .transpose()
                        }
                        Display::Robot(state) => state
                            .poll(&mut context.tf, &a.cancel, &context.counters)?
                            .map(|sample| {
                                sample
                                    .encode(
                                        a.configuration
                                            .robot
                                            .as_ref()
                                            .ok_or("Missing Robot configuration")?,
                                    )
                                    .map(|bytes| EncodedPayload::Robot(bytes.into_boxed_slice()))
                            })
                            .transpose(),
                        Display::Scan { reader, state } => {
                            if let Some(value) = reader.poll_now()? {
                                state.input(value, now)?;
                            }
                            let sample = state.poll(&mut context.tf, now, &context.counters)?;
                            if let Some(counters) = &a.spatial_counters {
                                *counters.lock().unwrap() = state.counters(&context.counters);
                            }
                            sample
                                .map(|sample| {
                                    sample.encode().map(|bytes| {
                                        EncodedPayload::Spatial(bytes.into_boxed_slice())
                                    })
                                })
                                .transpose()
                        }
                        Display::Cloud { reader, state } => {
                            if let Some(cloud) = reader.poll_cloud()? {
                                state.input(cloud, now)?;
                            }
                            state
                                .poll(&mut context.tf, now, &context.counters)?
                                .map(|sample| {
                                    sample.encode().map(|bytes| {
                                        EncodedPayload::Cloud(bytes.into_boxed_slice())
                                    })
                                })
                                .transpose()
                        }
                        Display::Mapping { reader, state } => {
                            if let Some(decoded) = reader.poll_mapping()? {
                                state.input(decoded, now)?;
                            }
                            state
                                .poll(&mut context.tf, now, &context.counters)?
                                .map(|sample| {
                                    sample.encode().map(|bytes| {
                                        EncodedPayload::Mapping(bytes.into_boxed_slice())
                                    })
                                })
                                .transpose()
                        }
                    }
                })();
                match result {
                    Ok(Some(payload)) => {
                        if !a.cancel.load(Ordering::Acquire) {
                            owner.sequence = match owner.sequence.checked_add(1) {
                                Some(s) => s,
                                None => {
                                    self.finish_owner(
                                        index,
                                        Err("Scene sequence exhausted".into()),
                                    );
                                    continue;
                                }
                            };
                            let ordered = matches!(
                                payload,
                                EncodedPayload::MapSnapshot(_)
                                    | EncodedPayload::MarkerSnapshot(_)
                                    | EncodedPayload::PoseSnapshot(_)
                            );
                            let result = a.queue.lock().unwrap().enqueue_sample(
                                a.generation,
                                a.subscription,
                                owner.sequence,
                                payload,
                            );
                            if ordered {
                                if let Err(error) = result {
                                    self.finish_owner(
                                        index,
                                        Err(format!(
                                            "Ordered scene queue rejected transfer: {error:?}"
                                        )),
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.finish_owner(index, Err(error));
                        continue;
                    }
                }
                index += 1;
            }
            thread::sleep(Duration::from_millis(1));
        }
        while !self.owners.is_empty() {
            self.finish_owner(0, Ok(()));
        }
    }
}

#[cfg(test)]
mod tests {
    mod map_tests;
    mod marker_tests;
    mod pose_tests;
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use studio_protocol::{cloud, mapping, session::Event};
    enum Input {
        Typed(structured::Value),
        Cloud(cloud::DecodedCloud),
        Mapping(mapping::Decoded),
        Error,
    }
    type Inputs = Arc<Mutex<HashMap<String, VecDeque<Input>>>>;
    struct Fake {
        topic: String,
        owner: thread::ThreadId,
        inputs: Inputs,
        drops: Arc<Mutex<Vec<String>>>,
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            assert_eq!(self.owner, thread::current().id());
            self.drops.lock().unwrap().push(self.topic.clone());
        }
    }
    impl Reader for Fake {
        fn poll(&mut self) -> Result<Option<structured::Value>, String> {
            assert_eq!(self.owner, thread::current().id());
            match self
                .inputs
                .lock()
                .unwrap()
                .entry(self.topic.clone())
                .or_default()
                .pop_front()
            {
                Some(Input::Typed(v)) => Ok(Some(v)),
                Some(Input::Error) => Err("injected typed source failure".into()),
                Some(Input::Cloud(_)) => Err("Cloud used generic truncated decoder".into()),
                Some(Input::Mapping(_)) => Err("Mapping used generic truncated decoder".into()),
                None => Ok(None),
            }
        }
        fn poll_cloud(&mut self) -> Result<Option<cloud::DecodedCloud>, String> {
            assert_eq!(self.owner, thread::current().id());
            match self
                .inputs
                .lock()
                .unwrap()
                .entry(self.topic.clone())
                .or_default()
                .pop_front()
            {
                Some(Input::Cloud(v)) => Ok(Some(v)),
                Some(Input::Error) => Err("injected compact cloud failure".into()),
                Some(Input::Typed(_)) => Err("Wrong cloud fixture input".into()),
                Some(Input::Mapping(_)) => Err("Mapping used cloud decoder".into()),
                None => Ok(None),
            }
        }
        fn poll_mapping(&mut self) -> Result<Option<mapping::Decoded>, String> {
            assert_eq!(self.owner, thread::current().id());
            match self
                .inputs
                .lock()
                .unwrap()
                .entry(self.topic.clone())
                .or_default()
                .pop_front()
            {
                Some(Input::Mapping(decoded)) => Ok(Some(decoded)),
                Some(Input::Error) => Err("injected compact mapping failure".into()),
                Some(_) => Err("Wrong mapping fixture input".into()),
                None => Ok(None),
            }
        }
    }
    struct Ticket {
        cancel: Arc<AtomicBool>,
        join: Option<JoinHandle<Result<(), String>>>,
    }
    impl Ticket {
        fn stop(&mut self) -> Result<(), String> {
            self.cancel.store(true, Ordering::Release);
            self.join.take().unwrap().join().unwrap()
        }
    }
    impl Drop for Ticket {
        fn drop(&mut self) {
            if self.join.is_some() {
                let _ = self.stop();
            }
        }
    }
    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < deadline, "Shared scene fixture timed out");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn start(
        service: &SceneService,
        queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
        id: u64,
        configuration: Configuration,
    ) -> Ticket {
        queue
            .lock()
            .unwrap()
            .register(Generation(7), SubscriptionId(id))
            .unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let attach = Attach {
            generation: Generation(7),
            subscription: SubscriptionId(id),
            configuration,
            ready: ready.clone(),
            released: Arc::new(AtomicBool::new(true)),
            cancel: cancel.clone(),
            queue: queue.clone(),
            spatial_counters: None,
        };
        let clone = service.clone();
        let join = thread::spawn(move || clone.attach_and_wait(attach));
        wait(|| ready.load(Ordering::Acquire) || join.is_finished());
        assert!(!join.is_finished(), "Scene fixture attach failed");
        Ticket {
            cancel,
            join: Some(join),
        }
    }
    fn sample(
        queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
        id: u64,
        stash: &mut HashMap<u64, Event>,
    ) -> Event {
        if let Some(event) = stash.remove(&id) {
            return event;
        }
        let mut found = None;
        wait(|| {
            while let Some(packet) = queue.lock().unwrap().pop() {
                if let Packet::Sample {
                    generation,
                    subscription,
                    sequence,
                    bytes,
                } = packet
                {
                    let wire=format!("{{\"event\":\"{}\",\"generation\":{},\"subscription\":{},\"sequence\":{},\"backend_drops\":0,\"sample\":{}}}",bytes.event_name(),generation.0,subscription.0,sequence,std::str::from_utf8(bytes.bytes()).unwrap());
                    let event = Event::decode(wire.as_bytes()).unwrap();
                    if subscription.0 == id {
                        found = Some(event);
                        return true;
                    }
                    stash.insert(subscription.0, event);
                }
            }
            false
        });
        found.unwrap()
    }
    #[test]
    fn graph_waits_for_start_release_and_duplicate_owner_opens_no_extra_reader() {
        let opens = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (o, d) = (opens.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            o.lock().unwrap().push(c.topic.clone());
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: Arc::new(Mutex::new(HashMap::new())),
                drops: d.clone(),
            }))
        });
        let service = SceneService::new(factory);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        queue
            .lock()
            .unwrap()
            .register(Generation(7), SubscriptionId(1))
            .unwrap();
        let mut config = crate::spatial_worker::tests::configuration();
        config.output = Output::TfGraph;
        config.best_effort = false;
        config.topic = "/tf".into();
        config.message_type = "tf2_msgs/msg/TFMessage".into();
        let ready = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let first = Attach {
            generation: Generation(7),
            subscription: SubscriptionId(1),
            configuration: config.clone(),
            ready: ready.clone(),
            released: released.clone(),
            cancel: cancel.clone(),
            queue: queue.clone(),
            spatial_counters: None,
        };
        let clone = service.clone();
        let join = thread::spawn(move || clone.attach_and_wait(first));
        let mut ticket = Ticket {
            cancel,
            join: Some(join),
        };
        wait(|| ready.load(Ordering::Acquire));
        let duplicate = Attach {
            generation: Generation(7),
            subscription: SubscriptionId(2),
            configuration: config,
            ready: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(true)),
            cancel: Arc::new(AtomicBool::new(false)),
            queue: queue.clone(),
            spatial_counters: None,
        };
        assert!(service
            .attach_and_wait(duplicate)
            .unwrap_err()
            .contains("capacity"));
        assert_eq!(queue.lock().unwrap().queued_samples(), 0);
        assert_eq!(opens.lock().unwrap().len(), 2);
        released.store(true, Ordering::Release);
        assert!(
            matches!(sample(&queue,1,&mut HashMap::new()),Event::TfGraphObservation{sample,..} if sample.frames.is_empty())
        );
        ticket.stop().unwrap();
        assert_eq!(drops.lock().unwrap().len(), 2);
    }
    #[test]
    fn graph_and_robot_share_exactly_one_tf_pair_and_stop_independently() {
        let inputs: Inputs = Arc::new(Mutex::new(HashMap::new()));
        inputs.lock().unwrap().insert(
            "/tf_static".into(),
            vec![Input::Typed(crate::spatial_worker::tests::tf(
                "map", "base", 0, 1.,
            ))]
            .into(),
        );
        let opens = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (i, o, d) = (inputs, opens.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            o.lock().unwrap().push(c.topic.clone());
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: i.clone(),
                drops: d.clone(),
            }))
        });
        let service = SceneService::new(factory);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let mut config = crate::spatial_worker::tests::configuration();
        config.best_effort = false;
        config.output = Output::TfGraph;
        config.topic = "/tf".into();
        config.message_type = "tf2_msgs/msg/TFMessage".into();
        let mut graph = start(&service, &queue, 1, config);
        let mut robot = start(
            &service,
            &queue,
            2,
            crate::robot_worker::tests::configuration(),
        );
        let mut stash = HashMap::new();
        match sample(&queue, 1, &mut stash) {
            Event::TfGraphObservation { sample, .. } => {
                assert_eq!(sample.frames.len(), 2);
                assert!(
                    matches!(sample.frames[0].pose,studio_protocol::tf_graph::LatestPose::Ready{resolved_stamp_ns:0,components} if components[0]==1.)
                );
                assert_eq!(sample.counters.tf_accepted, 1);
            }
            other => panic!("Expected graph: {other:?}"),
        }
        assert_eq!(opens.lock().unwrap().as_slice(), ["/tf_static", "/tf"]);
        graph.stop().unwrap();
        assert!(drops.lock().unwrap().is_empty());
        assert!(matches!(
            sample(&queue, 2, &mut stash),
            Event::RobotObservation { .. }
        ));
        robot.stop().unwrap();
        let mut closed = drops.lock().unwrap().clone();
        closed.sort();
        assert_eq!(closed, ["/tf", "/tf_static"]);
    }
    #[test]
    fn one_tf_pair_serves_robot_two_scans_cloud_and_scoped_sensor_failure() {
        let inputs: Inputs = Arc::new(Mutex::new(HashMap::new()));
        let opens = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        {
            let mut q = inputs.lock().unwrap();
            q.insert(
                "/tf_static".into(),
                vec![
                    Input::Typed(crate::spatial_worker::tests::tf("base", "laser", 0, 1.)),
                    Input::Typed(crate::spatial_worker::tests::tf("base", "tool", 0, 2.)),
                ]
                .into(),
            );
            q.insert(
                "/tf".into(),
                vec![
                    Input::Typed(crate::spatial_worker::tests::tf("map", "base", 1, 1.)),
                    Input::Typed(crate::spatial_worker::tests::tf("map", "base", 3, 3.)),
                ]
                .into(),
            );
        }
        let (i, o, d) = (inputs.clone(), opens.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            o.lock()
                .unwrap()
                .push((c.topic.clone(), thread::current().id()));
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: i.clone(),
                drops: d.clone(),
            }))
        });
        let service = SceneService::new(factory);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let mut stash = HashMap::new();
        let mut robot = start(
            &service,
            &queue,
            1,
            crate::robot_worker::tests::configuration(),
        );
        let mut scan_config = crate::spatial_worker::tests::configuration();
        scan_config.topic = "/scan_a".into();
        let mut scan_a = start(&service, &queue, 2, scan_config.clone());
        scan_config.topic = "/scan_b".into();
        let mut scan_b = start(&service, &queue, 3, scan_config.clone());
        let mut cloud_config = scan_config;
        cloud_config.topic = "/cloud".into();
        cloud_config.output = Output::PointCloud;
        cloud_config.message_type = cloud::MESSAGE_TYPE.into();
        let mut pointcloud = start(&service, &queue, 4, cloud_config);
        {
            let mut q = inputs.lock().unwrap();
            for topic in ["/scan_a", "/scan_b"] {
                q.entry(topic.into())
                    .or_default()
                    .push_back(Input::Typed(crate::spatial_worker::tests::scan(2)));
            }
            q.entry("/cloud".into())
                .or_default()
                .push_back(Input::Cloud(cloud::DecodedCloud {
                    source_frame: "laser".into(),
                    acquisition_stamp_ns: 2_000_000_000,
                    source_points: 1,
                    skipped_points: 0,
                    points: vec![[1., 2., 3.]],
                    channels: cloud::Channels {
                        color: cloud::Channel::Values(vec![[1, 2, 3, 4]]),
                        intensity: cloud::Channel::Values(vec![Some(42.)]),
                    },
                }));
        }
        let mut ready_scan = false;
        while !ready_scan {
            if let Event::SpatialObservation { sample, .. } = sample(&queue, 2, &mut stash) {
                if let spatial::TfOutcome::Ready { components } = sample.tf {
                    assert_eq!(sample.acquisition_stamp_ns, 2_000_000_000);
                    assert_eq!(components[0], 3.);
                    ready_scan = true;
                }
            }
        }
        let mut ready_cloud = false;
        while !ready_cloud {
            if let Event::PointCloudObservation { sample, .. } = sample(&queue, 4, &mut stash) {
                if let spatial::TfOutcome::Ready { components } = sample.tf {
                    assert_eq!(sample.points, vec![[1., 2., 3.]]);
                    assert_eq!(
                        sample.channels,
                        cloud::Channels {
                            color: cloud::Channel::Values(vec![[1, 2, 3, 4]]),
                            intensity: cloud::Channel::Values(vec![Some(42.)])
                        }
                    );
                    assert_eq!(sample.acquisition_stamp_ns, 2_000_000_000);
                    assert_eq!(components[0], 3.);
                    ready_cloud = true;
                }
            }
        }
        let opened = opens.lock().unwrap();
        assert_eq!(opened.len(), 5);
        assert_eq!(opened.iter().filter(|(t, _)| t == "/tf").count(), 1);
        assert_eq!(opened.iter().filter(|(t, _)| t == "/tf_static").count(), 1);
        assert!(opened.iter().all(|(_, id)| *id == opened[0].1));
        drop(opened);
        inputs
            .lock()
            .unwrap()
            .entry("/cloud".into())
            .or_default()
            .push_back(Input::Error);
        wait(|| pointcloud.join.as_ref().unwrap().is_finished());
        assert!(pointcloud.stop().is_err());
        assert_eq!(drops.lock().unwrap().as_slice(), ["/cloud"]);
        assert!(matches!(
            sample(&queue, 1, &mut stash),
            Event::RobotObservation { .. }
        ));
        scan_a.stop().unwrap();
        assert!(!drops
            .lock()
            .unwrap()
            .iter()
            .any(|s| s == "/tf" || s == "/tf_static"));
        inputs
            .lock()
            .unwrap()
            .entry("/scan_b".into())
            .or_default()
            .push_back(Input::Typed(crate::spatial_worker::tests::scan(3)));
        assert!(matches!(
            sample(&queue, 3, &mut stash),
            Event::SpatialObservation { .. }
        ));
        scan_b.stop().unwrap();
        assert!(!drops
            .lock()
            .unwrap()
            .iter()
            .any(|s| s == "/tf" || s == "/tf_static"));
        robot.stop().unwrap();
        assert_eq!(drops.lock().unwrap().len(), 5);
    }
    #[test]
    fn malformed_shared_tf_closes_all_dependents_before_completion() {
        let inputs: Inputs = Arc::new(Mutex::new(HashMap::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (i, d) = (inputs.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: i.clone(),
                drops: d.clone(),
            }))
        });
        let service = SceneService::new(factory);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let mut robot = start(
            &service,
            &queue,
            1,
            crate::robot_worker::tests::configuration(),
        );
        let mut scan = start(
            &service,
            &queue,
            2,
            crate::spatial_worker::tests::configuration(),
        );
        inputs.lock().unwrap().insert(
            "/tf".into(),
            vec![Input::Typed(structured::Value::Unsigned {
                value: "1".into(),
            })]
            .into(),
        );
        wait(|| {
            robot.join.as_ref().unwrap().is_finished() && scan.join.as_ref().unwrap().is_finished()
        });
        assert!(robot
            .stop()
            .unwrap_err()
            .contains("Shared scene TF failure"));
        assert!(scan.stop().is_err());
        assert_eq!(drops.lock().unwrap().len(), 3);
    }
    #[test]
    fn mapping_identity_missing_recovery_and_scoped_failure_preserve_other_owners() {
        let inputs: Inputs = Arc::new(Mutex::new(HashMap::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        let opens = Arc::new(Mutex::new(Vec::new()));
        let (i, d, o) = (inputs.clone(), drops.clone(), opens.clone());
        let factory: Factory = Arc::new(move |c| {
            o.lock().unwrap().push(c.topic.clone());
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: i.clone(),
                drops: d.clone(),
            }))
        });
        let service = SceneService::new(factory);
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let mut stash = HashMap::new();
        let map_config = crate::mapping_worker::tests::configuration();
        let mut pose_config = map_config.clone();
        pose_config.topic = "/pose".into();
        pose_config.output = Output::Pose;
        pose_config.message_type = mapping::POSE_MESSAGE_TYPE.into();
        let mut path_config = map_config.clone();
        path_config.topic = "/path".into();
        path_config.output = Output::Path;
        path_config.message_type = mapping::PATH_MESSAGE_TYPE.into();
        let mut map_ticket = start(&service, &queue, 1, map_config.clone());
        let mut pose_ticket = start(&service, &queue, 2, pose_config.clone());
        let mut path_ticket = start(&service, &queue, 3, path_config.clone());
        assert_eq!(opens.lock().unwrap().len(), 5);
        let decoded =
            |configuration: &Configuration, source: &str, stamp, geometry| mapping::Decoded {
                topic: configuration.topic.clone(),
                message_type: configuration.message_type.clone(),
                source_frame: source.into(),
                acquisition_stamp_ns: stamp,
                geometry,
            };
        let map = decoded(
            &map_config,
            "unresolved",
            1,
            mapping::Geometry::OccupancyGrid {
                width: 1,
                height: 1,
                resolution: 0.5,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
                cells: vec![100],
            },
        );
        let pose = decoded(
            &pose_config,
            "map",
            1,
            mapping::Geometry::Pose {
                components: [1., 2., 3., 0., 0., 0., 1.],
            },
        );
        let path = decoded(
            &path_config,
            "map",
            1,
            mapping::Geometry::Path { poses: Vec::new() },
        );
        for value in [map, pose, path] {
            inputs
                .lock()
                .unwrap()
                .entry(value.topic.clone())
                .or_default()
                .push_back(Input::Mapping(value));
        }
        assert!(
            matches!(sample(&queue,1,&mut stash),Event::MappingObservation{sample,..} if matches!(sample.tf,spatial::TfOutcome::Missing))
        );
        for id in [2, 3] {
            assert!(
                matches!(sample(&queue,id,&mut stash),Event::MappingObservation{sample,..} if matches!(sample.tf,spatial::TfOutcome::Ready{components} if components == [0.,0.,0.,0.,0.,0.,1.]))
            );
        }
        inputs
            .lock()
            .unwrap()
            .entry("/tf_static".into())
            .or_default()
            .push_back(Input::Typed(crate::spatial_worker::tests::tf(
                "map",
                "unresolved",
                0,
                2.,
            )));
        assert!(
            matches!(sample(&queue,1,&mut stash),Event::MappingObservation{sample,..} if matches!(sample.tf,spatial::TfOutcome::Ready{components} if components[0]==2.))
        );
        inputs
            .lock()
            .unwrap()
            .entry("/map".into())
            .or_default()
            .push_back(Input::Error);
        wait(|| map_ticket.join.as_ref().unwrap().is_finished());
        assert!(map_ticket.stop().is_err());
        assert_eq!(drops.lock().unwrap().as_slice(), ["/map"]);
        for (c, geometry) in [
            (
                &pose_config,
                mapping::Geometry::Pose {
                    components: [2., 3., 4., 0., 0., 0., 1.],
                },
            ),
            (&path_config, mapping::Geometry::Path { poses: Vec::new() }),
        ] {
            let value = decoded(c, "map", 2, geometry);
            inputs
                .lock()
                .unwrap()
                .entry(c.topic.clone())
                .or_default()
                .push_back(Input::Mapping(value));
        }
        for id in [2, 3] {
            assert!(
                matches!(sample(&queue,id,&mut stash),Event::MappingObservation{sample,..} if sample.acquisition_stamp_ns==2)
            );
        }
        pose_ticket.stop().unwrap();
        assert!(!drops
            .lock()
            .unwrap()
            .iter()
            .any(|t| t == "/tf" || t == "/tf_static"));
        path_ticket.stop().unwrap();
        assert_eq!(drops.lock().unwrap().len(), 5);
    }
    #[test]
    fn attach_after_panic_final_drain_finishes_only_after_actor_exit() {
        let inputs: Inputs = Arc::new(Mutex::new(HashMap::new()));
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (i, d) = (inputs.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            if c.topic == "/panic" {
                panic!("injected scene attach panic after native TF readers opened");
            }
            Ok(Box::new(Fake {
                topic: c.topic.clone(),
                owner: thread::current().id(),
                inputs: i.clone(),
                drops: d.clone(),
            }))
        });
        let (drained_tx, drained_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let service = SceneService::spawn(factory, move || {
            drained_tx.send(()).unwrap();
            exit_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(7))));
        let attach = |id, configuration| Attach {
            generation: Generation(7),
            subscription: SubscriptionId(id),
            configuration,
            ready: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(true)),
            cancel: Arc::new(AtomicBool::new(false)),
            queue: queue.clone(),
            spatial_counters: None,
        };
        let mut configuration = crate::spatial_worker::tests::configuration();
        configuration.topic = "/panic".into();
        let first = attach(1, configuration.clone());
        let clone = service.clone();
        let first = thread::spawn(move || clone.attach_and_wait(first));
        drained_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        wait(|| first.is_finished());
        assert!(first.join().unwrap().is_err());
        assert_eq!(drops.lock().unwrap().len(), 2);
        assert!(!service.inner.closed.load(Ordering::Acquire));

        // Deterministically enqueue into the exact former race window: the
        // actor has observed Empty in its final drain, but still owns rx.
        let completion = Arc::new(Completion {
            result: Mutex::new(None),
            wake: Condvar::new(),
        });
        let late = attach(2, configuration);
        let cancel = late.cancel.clone();
        assert!(service
            .inner
            .tx
            .try_send(Command::Attach(Box::new(Request {
                attach: late,
                completion: completion.clone(),
            })))
            .is_ok());
        let clone = service.clone();
        let waiter = thread::spawn(move || {
            clone.wait_for_completion(completion, cancel, Generation(7), SubscriptionId(2))
        });
        assert!(!service.inner.closed.load(Ordering::Acquire));
        assert!(!waiter.is_finished(), "Actor has not exited yet");
        exit_tx.send(()).unwrap();
        wait(|| waiter.is_finished());
        assert!(waiter
            .join()
            .unwrap()
            .unwrap_err()
            .contains("Scene worker exited"));
        assert!(service.inner.closed.load(Ordering::Acquire));
        assert_eq!(drops.lock().unwrap().len(), 2);
        assert!(service.inner.tx.try_send(Command::Shutdown).is_err());
    }
}
