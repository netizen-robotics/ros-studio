//! Persistent, observe-only session. Native handles never leave their worker.
use crate::native_topic::NativeTopic;
use rclrs::CreateBasicExecutor;
use std::{
    io::{self, BufRead, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use studio_protocol::{
    cloud, image, mapping, robot,
    session::{self, Command, Event},
    spatial,
    structured::{self, Sample, Value},
    subscriptions::*,
};

pub(crate) trait Reader {
    fn poll_camera_info(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::camera_info::Calibration, String>>, String> {
        Err("Reader does not support CameraInfo".into())
    }
    fn poll_camera_image(&mut self) -> Result<Option<Result<image::ImageSample, String>>, String> {
        Err("Reader does not support nonblocking Camera Image".into())
    }
    fn poll_pose_covariance(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::pose_snapshot::Decoded, String>>, String> {
        Err("Reader does not support complete pose/covariance".into())
    }
    fn poll_markers(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::marker::Batch, String>>, String> {
        Err("Reader does not support complete Markers".into())
    }
    fn poll_grid(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Grid, String>>, String> {
        Err("Reader does not support raw grids".into())
    }
    fn poll_grid_update(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Patch, String>>, String> {
        Err("Reader does not support grid updates".into())
    }
    fn publication(&self) -> Result<crate::native_topic::Publication, String> {
        Err("Reader has no publication metadata".into())
    }
    fn enable_message_loss_events(&mut self) -> Result<bool, String> {
        Ok(false)
    }
    fn message_loss_count(&mut self) -> Result<Option<u64>, String> {
        Ok(None)
    }
    fn poll_description(&mut self) -> Result<Option<String>, String> {
        Err("Reader does not support complete robot descriptions".into())
    }
    fn poll(&mut self) -> Result<Option<Value>, String>;
    fn poll_now(&mut self) -> Result<Option<Value>, String> {
        self.poll()
    }
    fn poll_spatial(&mut self) -> Result<Option<spatial::Sample>, String> {
        Err("Reader does not support Spatial output".into())
    }
    fn poll_robot(&mut self) -> Result<Option<robot::Sample>, String> {
        Err("Reader does not support Robot output".into())
    }
    fn poll_cloud(&mut self) -> Result<Option<cloud::DecodedCloud>, String> {
        Err("Reader does not support compact PointCloud2 output".into())
    }
    fn poll_mapping(&mut self) -> Result<Option<mapping::Decoded>, String> {
        Err("Reader does not support compact mapping output".into())
    }
    fn spatial_counters(&self) -> Option<spatial::Counters> {
        None
    }
    fn poll_image(&mut self) -> Result<Option<image::ImageSample>, String> {
        Err("This observation reader does not support compact Image output".into())
    }
}
impl Reader for NativeTopic {
    fn poll_camera_info(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::camera_info::Calibration, String>>, String> {
        NativeTopic::poll_camera_info(self)
    }
    fn poll_camera_image(&mut self) -> Result<Option<Result<image::ImageSample, String>>, String> {
        NativeTopic::poll_image_input(self)
    }
    fn poll_pose_covariance(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::pose_snapshot::Decoded, String>>, String> {
        NativeTopic::poll_pose_covariance(self)
    }
    fn poll_markers(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::marker::Batch, String>>, String> {
        NativeTopic::poll_markers(self)
    }
    fn poll_grid(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Grid, String>>, String> {
        NativeTopic::poll_grid_input(self)
    }
    fn poll_grid_update(
        &mut self,
    ) -> Result<Option<Result<studio_protocol::map_grid::Patch, String>>, String> {
        NativeTopic::poll_grid_update_input(self)
    }
    fn publication(&self) -> Result<crate::native_topic::Publication, String> {
        NativeTopic::publication(self)
    }
    fn enable_message_loss_events(&mut self) -> Result<bool, String> {
        NativeTopic::enable_message_loss_events(self)
    }
    fn message_loss_count(&mut self) -> Result<Option<u64>, String> {
        NativeTopic::message_loss_count(self)
    }
    fn poll_description(&mut self) -> Result<Option<String>, String> {
        NativeTopic::poll_description(self)
    }
    fn poll_mapping(&mut self) -> Result<Option<mapping::Decoded>, String> {
        NativeTopic::poll_mapping(self)
    }
    fn poll_cloud(&mut self) -> Result<Option<cloud::DecodedCloud>, String> {
        NativeTopic::poll_cloud(self)
    }
    fn poll_now(&mut self) -> Result<Option<Value>, String> {
        self.poll_typed_timeout(0)
    }
    fn poll(&mut self) -> Result<Option<Value>, String> {
        self.poll_typed()
    }
    fn poll_image(&mut self) -> Result<Option<image::ImageSample>, String> {
        NativeTopic::poll_image(self)
    }
}
#[derive(Clone)]
pub(crate) enum EncodedPayload {
    Typed(Box<[u8]>),
    CompactImage(Box<[u8]>),
    Spatial(Box<[u8]>),
    Robot(Box<[u8]>),
    Cloud(Box<[u8]>),
    Mapping(Box<[u8]>),
    MapSnapshot(Box<[u8]>),
    MarkerSnapshot(Box<[u8]>),
    PoseSnapshot(Box<[u8]>),
    CameraSnapshot(Box<[u8]>),
    TfGraph(Box<[u8]>),
    RobotDescription(Box<[u8]>),
}
impl Payload for EncodedPayload {
    fn payload_bytes(&self) -> usize {
        self.bytes().len()
    }
    fn is_image(&self) -> bool {
        matches!(self, Self::CompactImage(_) | Self::CameraSnapshot(_))
    }
}
impl EncodedPayload {
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Self::Typed(bytes)
            | Self::CompactImage(bytes)
            | Self::Spatial(bytes)
            | Self::Robot(bytes)
            | Self::Cloud(bytes)
            | Self::Mapping(bytes)
            | Self::MapSnapshot(bytes)
            | Self::MarkerSnapshot(bytes)
            | Self::PoseSnapshot(bytes)
            | Self::CameraSnapshot(bytes)
            | Self::RobotDescription(bytes)
            | Self::TfGraph(bytes) => bytes,
        }
    }
    pub(crate) fn event_name(&self) -> &'static str {
        match self {
            Self::Typed(_) => "observation",
            Self::CompactImage(_) => "image_observation",
            Self::Spatial(_) => "spatial_observation",
            Self::Robot(_) => "robot_observation",
            Self::Cloud(_) => "point_cloud_observation",
            Self::RobotDescription(_) => "robot_description_observation",
            Self::TfGraph(_) => "tf_graph_observation",
            Self::Mapping(_) => "mapping_observation",
            Self::MapSnapshot(_) => "map_snapshot_observation",
            Self::MarkerSnapshot(_) => "marker_snapshot_observation",
            Self::PoseSnapshot(_) => "pose_snapshot_observation",
            Self::CameraSnapshot(_) => "camera_snapshot_observation",
        }
    }
}
fn poll_encoded(
    reader: &mut dyn Reader,
    configuration: &Configuration,
) -> Result<Option<EncodedPayload>, String> {
    match configuration.output {
        Output::OccupancyGrid
        | Output::Pose
        | Output::Path
        | Output::MapSnapshot
        | Output::MarkerSnapshot => Err("Mapping requires the shared scene TF worker".into()),
        Output::CameraSnapshot(_) => {
            Err("Camera requires the shared paired-source scene worker".into())
        }
        Output::PoseSnapshot(_) => {
            Err("Pose covariance requires the shared scene TF worker".into())
        }
        Output::RobotDescription => Err("Description requires the bounded capture worker".into()),
        Output::TfGraph => Err("TF graph requires the shared scene TF worker".into()),
        Output::PointCloud => Err("PointCloud2 requires the shared scene TF worker".into()),
        Output::Robot => reader
            .poll_robot()?
            .map(|sample| {
                sample
                    .encode(
                        configuration
                            .robot
                            .as_ref()
                            .ok_or("Missing Robot configuration")?,
                    )
                    .map(|bytes| EncodedPayload::Robot(bytes.into_boxed_slice()))
            })
            .transpose(),
        Output::SpatialScan => reader
            .poll_spatial()?
            .map(|sample| {
                sample
                    .encode()
                    .map(|bytes| EncodedPayload::Spatial(bytes.into_boxed_slice()))
            })
            .transpose(),
        Output::Typed => reader
            .poll()?
            .map(|value| {
                Sample {
                    contract_version: structured::CONTRACT_VERSION,
                    topic: configuration.topic.clone(),
                    message_type: configuration.message_type.clone(),
                    source_time: None,
                    value,
                }
                .encode()
                .map(|bytes| EncodedPayload::Typed(bytes.into_boxed_slice()))
            })
            .transpose(),
        Output::CompactImage => reader
            .poll_image()?
            .map(|sample| {
                if sample.topic != configuration.topic
                    || sample.message_type != configuration.message_type
                {
                    return Err("Native compact Image identity contradicts its subscription".into());
                }
                sample
                    .encode()
                    .map(|bytes| EncodedPayload::CompactImage(bytes.into_boxed_slice()))
            })
            .transpose(),
    }
}
pub(crate) type Factory =
    Arc<dyn Fn(&Configuration) -> Result<Box<dyn Reader>, String> + Send + Sync>;
type PoseClearPending = (
    RequestId,
    mpsc::Receiver<Result<crate::scene_worker::PoseBarrier, String>>,
    Arc<AtomicBool>,
);
struct Worker {
    generation: Generation,
    request: RequestId,
    subscription: SubscriptionId,
    stop_request: Option<RequestId>,
    ready: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    acknowledged: bool,
    join: JoinHandle<Result<(), String>>,
    configuration: Configuration,
    spatial_counters: Option<Arc<Mutex<spatial::Counters>>>,
    retired_drops: Option<u64>,
    clear_pending: Option<(RequestId, mpsc::Receiver<Result<(), String>>)>,
    pose_clear_pending: Option<PoseClearPending>,
}
struct Manager {
    factory: Factory,
    scene: crate::scene_worker::SceneService,
    queue: Arc<Mutex<FairQueue<EncodedPayload>>>,
    workers: Vec<Worker>,
    generation: Option<Generation>,
    last_request: u64,
    last_subscription: u64,
}
impl Manager {
    #[cfg(test)]
    fn reserved_handles(&self) -> usize {
        resource_usage(
            &self
                .workers
                .iter()
                .map(|w| &w.configuration)
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .native_handles
    }
    fn new(factory: Factory) -> Self {
        Self {
            scene: crate::scene_worker::SceneService::new(factory.clone()),
            factory,
            queue: Arc::new(Mutex::new(
                FairQueue::with_payload(Generation(0)).with_image_budget(),
            )),
            workers: Vec::new(),
            generation: None,
            last_request: 0,
            last_subscription: 0,
        }
    }
    fn send(out: &mut impl Write, event: Event) -> Result<(), String> {
        out.write_all(&event.encode()?)
            .and_then(|_| out.flush())
            .map_err(|e| e.to_string())
    }
    fn reject(
        &self,
        out: &mut impl Write,
        command: &Command,
        message: &str,
        no_handle: bool,
    ) -> Result<(), String> {
        let (generation, request, subscription) = command.ids();
        Self::send(
            out,
            Event::RequestError {
                generation,
                request,
                subscription,
                message: session::bounded_error(message),
                no_handle,
            },
        )
    }
    /// Snapshot and retire atomically relative to worker admission. Writes must
    /// happen after releasing the queue lock, so sensor owners cannot block on
    /// stdout while holding the shared queue.
    fn retire_samples(&self, subscription: SubscriptionId) -> Option<u64> {
        let mut queue = self.queue.lock().unwrap();
        let counters = queue.stream_counters(subscription)?;
        let _ = queue.unregister(subscription);
        Some(
            counters
                .evicted_samples
                .saturating_add(counters.rejected_samples),
        )
    }
    fn final_drops(
        &self,
        out: &mut impl Write,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<(), String> {
        if let Some(backend_drops) = self.retire_samples(subscription) {
            Self::send(
                out,
                Event::Drops {
                    generation,
                    subscription,
                    backend_drops,
                    spatial_counters: None,
                },
            )?;
        }
        Ok(())
    }
    fn command(&mut self, command: Command, out: &mut impl Write) -> Result<(), String> {
        command.validate()?;
        let (generation, request, subscription) = command.ids();
        if let Some(current) = self.generation {
            if current != generation {
                return Err("Generation changed inside an observation process".into());
            }
        } else {
            self.generation = Some(generation);
            self.queue
                .lock()
                .unwrap()
                .reset(generation)
                .map_err(|e| format!("Queue reset: {e:?}"))?;
        }
        if request.0 <= self.last_request {
            // Reject without repeating a side effect or claiming an existing
            // handle disappeared. Fresh stop retries are required.
            return self.reject(
                out,
                &command,
                "Request ID is stale; retry with a fresh request ID",
                !self.workers.iter().any(|w| w.subscription == subscription),
            );
        }
        self.last_request = request.0;
        match &command {
            Command::Tool { .. } => {
                return Err("Tool command requires the interaction dispatcher".into())
            }
            Command::Start { configuration, .. } => {
                if subscription.0 <= self.last_subscription {
                    return self.reject(
                        out,
                        &command,
                        "Subscription ID was already allocated",
                        !self.workers.iter().any(|w| w.subscription == subscription),
                    );
                }
                self.last_subscription = subscription.0;
                if !configuration.valid() {
                    return self.reject(
                        out,
                        &command,
                        "Invalid topic or discovered message type",
                        true,
                    );
                }
                let mut reservations: Vec<_> = self
                    .workers
                    .iter()
                    .map(|worker| &worker.configuration)
                    .collect();
                reservations.push(configuration);
                if let Err(message) = resource_usage(&reservations) {
                    return self.reject(out, &command, message, true);
                }
                self.queue
                    .lock()
                    .unwrap()
                    .register(generation, subscription)
                    .map_err(|e| format!("Queue register: {e:?}"))?;
                let ready = Arc::new(AtomicBool::new(false));
                let released = Arc::new(AtomicBool::new(false));
                let cancel = Arc::new(AtomicBool::new(false));
                let (r, release, c) = (ready.clone(), released.clone(), cancel.clone());
                let configuration = configuration.clone();
                let queue = self.queue.clone();
                let factory = self.factory.clone();
                let reservation = configuration.clone();
                let scene = self.scene.clone();
                let spatial_counters = (configuration.output == Output::SpatialScan)
                    .then(|| Arc::new(Mutex::new(spatial::Counters::default())));
                let stats = spatial_counters.clone();
                let join = thread::spawn(move || {
                    if configuration.owns_tf() {
                        return scene.attach_and_wait(crate::scene_worker::Attach {
                            generation,
                            subscription,
                            configuration,
                            ready: r,
                            released: release,
                            cancel: c,
                            queue,
                            spatial_counters: stats,
                        });
                    }
                    // Construct and drop on this same worker, including errors,
                    // cancellation during open and panic unwinding.
                    let mut reader: Box<dyn Reader> = factory(&configuration)?;
                    r.store(true, Ordering::Release);
                    while !release.load(Ordering::Acquire) && !c.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    if configuration.output == Output::RobotDescription {
                        return crate::description_worker::run(
                            reader.as_mut(),
                            &configuration.topic,
                            generation,
                            subscription,
                            &queue,
                            &c,
                        );
                    }
                    let mut sequence = 0u64;
                    while !c.load(Ordering::Acquire) {
                        let result = poll_encoded(reader.as_mut(), &configuration);
                        if let (Some(stats), Some(snapshot)) = (&stats, reader.spatial_counters()) {
                            *stats.lock().unwrap() = snapshot;
                        }
                        let Some(bytes) = result? else {
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        };
                        if c.load(Ordering::Acquire) {
                            break;
                        }
                        sequence = sequence.checked_add(1).ok_or("Sample sequence exhausted")?;
                        // Cancellation unregisters the stream; a racing enqueue
                        // is rejected and cannot restore a stopped stream.
                        let _ = queue.lock().unwrap().enqueue_sample(
                            generation,
                            subscription,
                            sequence,
                            bytes,
                        );
                    }
                    Ok(())
                });
                self.workers.push(Worker {
                    generation,
                    request,
                    subscription,
                    stop_request: None,
                    ready,
                    released,
                    cancel,
                    acknowledged: false,
                    join,
                    configuration: reservation,
                    spatial_counters,
                    retired_drops: None,
                    clear_pending: None,
                    pose_clear_pending: None,
                });
            }
            Command::ClearPose { .. } => {
                let Some(index) = self
                    .workers
                    .iter()
                    .position(|w| w.subscription == subscription)
                else {
                    return self.reject(out, &command, "Unknown pose subscription", true);
                };
                let worker = &self.workers[index];
                if !worker.acknowledged
                    || worker.stop_request.is_some()
                    || worker.pose_clear_pending.is_some()
                    || !matches!(worker.configuration.output, Output::PoseSnapshot(_))
                {
                    return self.reject(
                        out,
                        &command,
                        "Clear requires an active pose owner without pending Clear",
                        false,
                    );
                }
                let gate = Arc::new(AtomicBool::new(false));
                match self
                    .scene
                    .clear_pose(generation, subscription, gate.clone())
                {
                    Ok(completion) => {
                        self.workers[index].pose_clear_pending = Some((request, completion, gate))
                    }
                    Err(message) => return self.reject(out, &command, &message, false),
                }
            }
            Command::ClearMarkers { .. } => {
                let Some(index) = self
                    .workers
                    .iter()
                    .position(|w| w.subscription == subscription)
                else {
                    return self.reject(out, &command, "Unknown Marker subscription", true);
                };
                let worker = &self.workers[index];
                if !worker.acknowledged
                    || worker.stop_request.is_some()
                    || worker.clear_pending.is_some()
                    || worker.configuration.output != Output::MarkerSnapshot
                {
                    return self.reject(
                        out,
                        &command,
                        "Clear requires one active Marker owner with no pending Clear",
                        false,
                    );
                }
                match self.scene.clear_markers(generation, subscription) {
                    Ok(completion) => {
                        self.workers[index].clear_pending = Some((request, completion))
                    }
                    Err(message) => return self.reject(out, &command, &message, false),
                }
            }
            Command::Stop { .. } => {
                if let Some(index) = self
                    .workers
                    .iter()
                    .position(|w| w.subscription == subscription)
                {
                    let worker = &mut self.workers[index];
                    worker.stop_request = Some(request);
                    worker.clear_pending = None;
                    worker.pose_clear_pending = None;
                    worker.cancel.store(true, Ordering::Release);
                    if worker.spatial_counters.is_some() {
                        // Retire samples now; retain one fixed loss snapshot
                        // until joined, when the final worker counters are stable.
                        if let Some(drops) = self.retire_samples(subscription) {
                            self.workers[index].retired_drops = Some(drops);
                        }
                    } else {
                        self.final_drops(out, generation, subscription)?;
                    }
                } else if subscription.0 <= self.last_subscription {
                    // No tombstone map is required: all IDs below the high-water
                    // mark can never create another handle in this process.
                    Self::send(
                        out,
                        Event::Stopped {
                            generation,
                            request,
                            subscription,
                        },
                    )?;
                } else {
                    self.reject(out, &command, "Unknown subscription ID", true)?;
                }
            }
        }
        Ok(())
    }
    fn progress(&mut self, out: &mut impl Write) -> Result<(), String> {
        for worker in &mut self.workers {
            let clear =
                worker
                    .pose_clear_pending
                    .as_ref()
                    .and_then(|(request, completion, gate)| match completion.try_recv() {
                        Ok(result) => Some((*request, result, gate.clone())),
                        Err(mpsc::TryRecvError::Disconnected) => Some((
                            *request,
                            Err("Scene worker closed before pose Clear".into()),
                            gate.clone(),
                        )),
                        Err(mpsc::TryRecvError::Empty) => None,
                    });
            if let Some((request, result, gate)) = clear {
                worker.pose_clear_pending = None;
                let event = match result {
                    Ok(barrier) => Event::PoseCleared {
                        generation: worker.generation,
                        request,
                        subscription: worker.subscription,
                        barrier: Box::new(barrier),
                    },
                    Err(message) => Event::RequestError {
                        generation: worker.generation,
                        request,
                        subscription: worker.subscription,
                        message: session::bounded_error(&message),
                        no_handle: false,
                    },
                };
                Self::send(out, event)?;
                gate.store(true, Ordering::Release);
            }
            let clear = worker
                .clear_pending
                .as_ref()
                .and_then(|(request, completion)| match completion.try_recv() {
                    Ok(result) => Some((*request, result)),
                    Err(mpsc::TryRecvError::Disconnected) => Some((
                        *request,
                        Err("Scene worker closed before Clear acknowledgment".into()),
                    )),
                    Err(mpsc::TryRecvError::Empty) => None,
                });
            if let Some((request, result)) = clear {
                worker.clear_pending = None;
                Self::send(
                    out,
                    match result {
                        Ok(()) => Event::MarkersCleared {
                            generation: worker.generation,
                            request,
                            subscription: worker.subscription,
                        },
                        Err(message) => Event::RequestError {
                            generation: worker.generation,
                            request,
                            subscription: worker.subscription,
                            message: session::bounded_error(&message),
                            no_handle: false,
                        },
                    },
                )?;
            }
            if !worker.acknowledged
                && worker.stop_request.is_none()
                && worker.ready.load(Ordering::Acquire)
            {
                Self::send(
                    out,
                    Event::Started {
                        generation: worker.generation,
                        request: worker.request,
                        subscription: worker.subscription,
                    },
                )?;
                worker.acknowledged = true;
                // Flush the acknowledgement before permitting any data.
                worker.released.store(true, Ordering::Release);
            }
        }
        let mut index = 0;
        while index < self.workers.len() {
            if !self.workers[index].join.is_finished() {
                index += 1;
                continue;
            }
            let worker = self.workers.remove(index);
            let result = worker
                .join
                .join()
                .unwrap_or_else(|_| Err("Observation worker panicked".into()));
            if worker.spatial_counters.is_some() {
                if let Some(backend_drops) = worker
                    .retired_drops
                    .or_else(|| self.retire_samples(worker.subscription))
                {
                    Self::send(
                        out,
                        Event::Drops {
                            generation: worker.generation,
                            subscription: worker.subscription,
                            backend_drops,
                            spatial_counters: worker
                                .spatial_counters
                                .as_ref()
                                .map(|c| *c.lock().unwrap()),
                        },
                    )?;
                }
            } else {
                self.final_drops(out, worker.generation, worker.subscription)?;
            }
            if let Some(request) = worker.stop_request {
                Self::send(
                    out,
                    Event::Stopped {
                        generation: worker.generation,
                        request,
                        subscription: worker.subscription,
                    },
                )?;
            } else if let Err(message) = result {
                let message = session::bounded_error(&message);
                let event = if worker.acknowledged {
                    Event::StreamError {
                        generation: worker.generation,
                        subscription: worker.subscription,
                        message,
                    }
                } else {
                    Event::RequestError {
                        generation: worker.generation,
                        request: worker.request,
                        subscription: worker.subscription,
                        message,
                        no_handle: true,
                    }
                };
                Self::send(out, event)?;
            }
        }
        let packet = self.queue.lock().unwrap().pop();
        if let Some(Packet::Sample {
            generation,
            subscription,
            sequence,
            bytes,
        }) = packet
        {
            let drops = self
                .queue
                .lock()
                .unwrap()
                .stream_counters(subscription)
                .unwrap_or_default();
            // Sample bytes were validated and bounded before queue admission.
            // Frame without a second decoded tree or changing numeric fidelity.
            write!(out, "{{\"event\":\"{}\",\"generation\":{},\"subscription\":{},\"sequence\":{},\"backend_drops\":{},\"sample\":", bytes.event_name(),generation.0, subscription.0, sequence, drops.evicted_samples.saturating_add(drops.rejected_samples)).map_err(|e| e.to_string())?;
            out.write_all(bytes.bytes())
                .and_then(|_| out.write_all(b"}\n"))
                .and_then(|_| out.flush())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.cancel.store(true, Ordering::Release);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join.join();
        }
    }
}

/// Bounded command framing. An incomplete final frame is a transport error.
fn read_command(input: &mut impl BufRead) -> Result<Option<Command>, String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = input.fill_buf().map_err(|e| e.to_string())?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("Incomplete observation command".into())
            };
        }
        let take = chunk
            .iter()
            .position(|b| *b == b'\n')
            .map_or(chunk.len(), |i| i + 1);
        if bytes.len() + take > session::MAX_COMMAND_BYTES {
            return Err(format!(
                "Observation command exceeds {} bytes",
                session::MAX_COMMAND_BYTES
            ));
        }
        let complete = chunk[take - 1] == b'\n';
        bytes.extend_from_slice(&chunk[..take]);
        input.consume(take);
        if complete {
            return Command::decode(&bytes).map(Some);
        }
    }
}

pub fn run(distro: &str) -> Result<(), String> {
    // Reserve the wire before creating any ROS objects: middleware diagnostics
    // can bypass rcutils and write directly to the process stdout descriptor.
    let mut out = crate::service_codec::NativeDiagnostics::stderr()?;
    let context = rclrs::Context::default_from_env().map_err(|e| e.to_string())?;
    let executor = context.create_basic_executor();
    let node = executor
        .create_node("studio_observe")
        .map_err(|e| e.to_string())?;
    let mut tools = crate::tools_worker::ToolsWorker::default();
    let imports = tools.imports.clone();
    let factory: Factory = Arc::new(move |c| {
        let bundle = imports
            .lock()
            .unwrap()
            .get(&c.message_type)
            .cloned()
            .unwrap_or_default();
        NativeTopic::open_with_bundle(
            &c.topic,
            &c.message_type,
            c.best_effort,
            c.transient_local,
            false,
            &bundle,
        )
        .map(|r| Box::new(r) as Box<dyn Reader>)
    });
    let mut manager = Manager::new(factory);
    let (tx, rx) = mpsc::sync_channel(studio_protocol::subscriptions::COMMAND_QUEUE_CAPACITY);
    // EOF/failure has its own reserved slot and cannot wait behind commands.
    let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut input = io::stdin().lock();
        let result = loop {
            match read_command(&mut input) {
                Ok(Some(command)) => match tx.try_send(command) {
                    Ok(()) => (),
                    Err(mpsc::TrySendError::Full(_)) => {
                        break Err("Observation command queue overflow".into())
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                },
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            }
        };
        let _ = terminal_tx.send(result);
    });
    Manager::send(
        &mut out,
        Event::ready(
            distro.into(),
            std::env::var("RMW_IMPLEMENTATION").map_err(|e| e.to_string())?,
        ),
    )?;
    let mut graph_at = Instant::now();
    let mut sent_frames = Vec::new();
    loop {
        if let Ok(terminal) = terminal_rx.try_recv() {
            return terminal;
        }
        if let Ok(command) = rx.try_recv() {
            if matches!(command, Command::Tool { .. }) {
                for event in tools.command(command, &node) {
                    Manager::send(&mut out, event)?;
                }
            } else {
                manager.command(command, &mut out)?;
            }
        }
        for event in tools.progress() {
            Manager::send(&mut out, event)?;
        }
        manager.progress(&mut out)?;
        if graph_at.elapsed() >= Duration::from_secs(1) {
            let topics = node
                .get_topic_names_and_types()
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect();
            Manager::send(&mut out, Event::Graph { topics })?;
            if let Some(generation) = manager.generation {
                let frames = manager.scene.observed_frames();
                if frames != sent_frames {
                    Manager::send(
                        &mut out,
                        Event::SceneFrames {
                            generation,
                            frames: frames.clone(),
                        },
                    )?;
                    sent_frames = frames;
                }
            }
            graph_at = Instant::now();
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, rc::Rc, sync::atomic::AtomicUsize};
    struct SpatialFixtureReader {
        owner: thread::ThreadId,
        values: std::collections::VecDeque<Value>,
        gate: Arc<AtomicBool>,
        destroyed: Arc<AtomicUsize>,
        _local: Rc<()>,
    }
    impl Reader for SpatialFixtureReader {
        fn poll(&mut self) -> Result<Option<Value>, String> {
            assert_eq!(self.owner, thread::current().id());
            if self.gate.load(Ordering::Acquire) {
                Ok(self.values.pop_front())
            } else {
                Ok(None)
            }
        }
    }
    impl Drop for SpatialFixtureReader {
        fn drop(&mut self) {
            assert_eq!(self.owner, thread::current().id());
            self.destroyed.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn spatial_start(request: u64, id: u64) -> Command {
        Command::Start {
            generation: Generation(7),
            request: RequestId(request),
            subscription: SubscriptionId(id),
            configuration: crate::spatial_worker::tests::configuration(),
        }
    }
    fn robot_start(request: u64, id: u64) -> Command {
        Command::Start {
            generation: Generation(7),
            request: RequestId(request),
            subscription: SubscriptionId(id),
            configuration: crate::robot_worker::tests::configuration(),
        }
    }
    #[test]
    fn robot_partial_latest_recovery_two_peers_and_stop_preserve_independent_owners() {
        let gate = Arc::new(AtomicBool::new(false));
        let destroyed = Arc::new(AtomicUsize::new(0));
        let (g, d) = (gate.clone(), destroyed.clone());
        let factory: Factory = Arc::new(move |c| {
            if c.topic.starts_with("/peer") {
                return Ok(Box::new(Mock {
                    owner: thread::current().id(),
                    drops: d.clone(),
                    sequence: 0,
                    fails: false,
                    _not_send: Rc::new(()),
                }));
            }
            let values = if c.topic == "/tf_static" {
                vec![crate::spatial_worker::tests::tf("base", "tool", 0, 2.)]
            } else {
                vec![crate::spatial_worker::tests::tf("map", "base", 1, 1.)]
            };
            Ok(Box::new(SpatialFixtureReader {
                owner: thread::current().id(),
                values: values.into(),
                gate: g.clone(),
                destroyed: d.clone(),
                _local: Rc::new(()),
            }))
        });
        let mut manager = Manager::new(factory);
        let mut out = Vec::new();
        manager.command(robot_start(1, 1), &mut out).unwrap();
        manager.command(start(2, 2, "/peer_a"), &mut out).unwrap();
        manager.command(start(3, 3, "/peer_b"), &mut out).unwrap();
        assert_eq!(manager.reserved_handles(), 4);
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e|matches!(e,Event::RobotObservation {sample,..} if matches!(sample.links[1].pose,robot::LinkPose::Missing)))
        });
        gate.store(true, Ordering::Release);
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e|matches!(e,Event::RobotObservation {sample,..} if matches!(sample.links[2].pose,robot::LinkPose::Ready {resolved_stamp_ns:1_000_000_000,components} if components[0]==3.)))
        });
        let samples: Vec<_> = events(&out)
            .into_iter()
            .filter_map(|e| {
                if let Event::RobotObservation {
                    sequence, sample, ..
                } = e
                {
                    Some((sequence, sample))
                } else {
                    None
                }
            })
            .collect();
        assert!(samples.last().unwrap().0 > samples[0].0);
        assert_eq!(samples.last().unwrap().1.links.len(), 4);
        assert!(matches!(
            samples.last().unwrap().1.links[3].pose,
            robot::LinkPose::Missing
        ));
        out.clear();
        manager.command(stop(5, 1), &mut out).unwrap();
        // Until manager progress reaps ownership, another Robot is forbidden even
        // when its native worker has already finished in the background.
        manager.command(robot_start(6, 5), &mut out).unwrap();
        assert!(matches!(
            events(&out).last(),
            Some(Event::RequestError {
                subscription: SubscriptionId(5),
                no_handle: true,
                ..
            })
        ));
        until(&mut manager, &mut out, |m, _| m.workers.len() == 2);
        assert_eq!(destroyed.load(Ordering::SeqCst), 2);
        let terminal = events(&out);
        let drops = terminal
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Drops {
                        subscription: SubscriptionId(1),
                        spatial_counters: None,
                        ..
                    }
                )
            })
            .unwrap();
        let stopped = terminal
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(drops < stopped);
        assert!(!terminal[stopped + 1..].iter().any(|e| matches!(
            e,
            Event::RobotObservation {
                subscription: SubscriptionId(1),
                ..
            }
        )));
        assert_eq!(manager.reserved_handles(), 2);
        out.clear();
        until(&mut manager, &mut out, |_, b| {
            [2, 3].iter().all(|id| {
                events(b).iter().any(
                    |e| matches!(e,Event::Observation {subscription,..} if subscription.0==*id),
                )
            })
        });
        manager.command(robot_start(7, 6), &mut out).unwrap();
        until(&mut manager, &mut out, |_, b| {
            events(b).iter().any(|e| {
                matches!(
                    e,
                    Event::RobotObservation {
                        subscription: SubscriptionId(6),
                        ..
                    }
                )
            })
        });
    }
    #[test]
    fn robot_and_scans_share_two_native_tf_readers_in_both_start_orders() {
        for robot_first in [true, false] {
            let destroyed = Arc::new(AtomicUsize::new(0));
            let d = destroyed.clone();
            let factory: Factory = Arc::new(move |_| {
                Ok(Box::new(SpatialFixtureReader {
                    owner: thread::current().id(),
                    values: Default::default(),
                    gate: Arc::new(AtomicBool::new(true)),
                    destroyed: d.clone(),
                    _local: Rc::new(()),
                }))
            });
            let mut manager = Manager::new(factory);
            let mut out = Vec::new();
            manager
                .command(
                    if robot_first {
                        robot_start(1, 1)
                    } else {
                        spatial_start(1, 1)
                    },
                    &mut out,
                )
                .unwrap();
            manager
                .command(
                    if robot_first {
                        spatial_start(2, 2)
                    } else {
                        robot_start(2, 2)
                    },
                    &mut out,
                )
                .unwrap();
            until(&mut manager, &mut out, |_, b| {
                events(b)
                    .iter()
                    .filter(|e| matches!(e, Event::Started { .. }))
                    .count()
                    == 2
            });
            assert_eq!(manager.reserved_handles(), 3);
            manager.command(stop(3, 1), &mut out).unwrap();
            until(&mut manager, &mut out, |m, _| m.workers.len() == 1);
            assert_eq!(
                destroyed.load(Ordering::SeqCst),
                if robot_first { 0 } else { 1 }
            );
            manager
                .command(
                    if robot_first {
                        robot_start(4, 3)
                    } else {
                        spatial_start(4, 3)
                    },
                    &mut out,
                )
                .unwrap();
            until(&mut manager, &mut out, |_, b| {
                events(b).iter().any(|e| {
                    matches!(
                        e,
                        Event::Started {
                            subscription: SubscriptionId(3),
                            ..
                        }
                    )
                })
            });
            manager.command(stop(5, 2), &mut out).unwrap();
            manager.command(stop(6, 3), &mut out).unwrap();
            until(&mut manager, &mut out, |m, _| m.workers.is_empty());
            assert_eq!(
                destroyed.load(Ordering::SeqCst),
                if robot_first { 3 } else { 4 }
            );
        }
    }
    #[test]
    fn robot_open_and_runtime_failure_cleanup_precedes_error_and_preserves_peer() {
        for open_failure in [true, false] {
            let destroyed = Arc::new(AtomicUsize::new(0));
            let d = destroyed.clone();
            let factory: Factory = Arc::new(move |c| {
                if c.topic == "/peer" {
                    return Ok(Box::new(Mock {
                        owner: thread::current().id(),
                        drops: d.clone(),
                        sequence: 0,
                        fails: false,
                        _not_send: Rc::new(()),
                    }));
                }
                if c.topic == "/tf" && open_failure {
                    return Err("controlled Robot dynamic open failure".into());
                }
                let values = if c.topic == "/tf" {
                    vec![Value::Unsigned { value: "1".into() }]
                } else {
                    vec![]
                };
                Ok(Box::new(SpatialFixtureReader {
                    owner: thread::current().id(),
                    values: values.into(),
                    gate: Arc::new(AtomicBool::new(true)),
                    destroyed: d.clone(),
                    _local: Rc::new(()),
                }))
            });
            let mut manager = Manager::new(factory);
            let mut out = Vec::new();
            manager.command(start(1, 1, "/peer"), &mut out).unwrap();
            manager.command(robot_start(2, 2), &mut out).unwrap();
            until(&mut manager, &mut out, |_, b| {
                events(b).iter().any(|e| {
                    matches!(
                        e,
                        Event::RequestError {
                            subscription: SubscriptionId(2),
                            no_handle: true,
                            ..
                        } | Event::StreamError {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
            });
            assert_eq!(manager.workers.len(), 1);
            assert_eq!(
                destroyed.load(Ordering::SeqCst),
                if open_failure { 1 } else { 2 }
            );
            let terminal = events(&out);
            let drops = terminal
                .iter()
                .position(|e| {
                    matches!(
                        e,
                        Event::Drops {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
                .unwrap();
            let error = terminal
                .iter()
                .position(|e| {
                    matches!(
                        e,
                        Event::RequestError {
                            subscription: SubscriptionId(2),
                            ..
                        } | Event::StreamError {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
                .unwrap();
            assert!(drops < error);
            out.clear();
            until(&mut manager, &mut out, |_, b| {
                events(b).iter().any(|e| {
                    matches!(
                        e,
                        Event::Observation {
                            subscription: SubscriptionId(1),
                            ..
                        }
                    )
                })
            });
        }
    }
    #[test]
    fn spatial_open_or_runtime_failure_releases_all_three_before_error_and_preserves_peer() {
        for open_failure in [true, false] {
            let destroyed = Arc::new(AtomicUsize::new(0));
            let d = destroyed.clone();
            let factory: Factory = Arc::new(move |c| {
                if c.topic == "/peer" {
                    return Ok(Box::new(Mock {
                        owner: thread::current().id(),
                        drops: d.clone(),
                        sequence: 0,
                        fails: false,
                        _not_send: Rc::new(()),
                    }));
                }
                if c.topic == "/tf" && open_failure {
                    return Err("controlled dynamic TF open failure".into());
                }
                let values = if c.topic == "/tf" {
                    vec![Value::Unsigned { value: "1".into() }]
                } else {
                    vec![]
                };
                Ok(Box::new(SpatialFixtureReader {
                    owner: thread::current().id(),
                    values: values.into(),
                    gate: Arc::new(AtomicBool::new(true)),
                    destroyed: d.clone(),
                    _local: Rc::new(()),
                }))
            });
            let mut m = Manager::new(factory);
            let mut out = Vec::new();
            m.command(start(1, 1, "/peer"), &mut out).unwrap();
            m.command(spatial_start(2, 2), &mut out).unwrap();
            until(&mut m, &mut out, |_, b| {
                events(b).iter().any(|e| {
                    matches!(
                        e,
                        Event::RequestError {
                            subscription: SubscriptionId(2),
                            no_handle: true,
                            ..
                        } | Event::StreamError {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
            });
            assert_eq!(
                destroyed.load(Ordering::SeqCst),
                if open_failure { 1 } else { 3 }
            );
            assert_eq!(m.workers.len(), 1);
            assert_eq!(m.workers[0].subscription, SubscriptionId(1));
            let terminal: Vec<_> = events(&out)
                .into_iter()
                .filter(|e| {
                    matches!(
                        e,
                        Event::Drops {
                            subscription: SubscriptionId(2),
                            ..
                        } | Event::RequestError {
                            subscription: SubscriptionId(2),
                            ..
                        } | Event::StreamError {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
                .collect();
            assert!(
                matches!(terminal.first(),Some(Event::Drops{spatial_counters:Some(c),..}) if c.tf_rejected==u64::from(!open_failure))
            );
            out.clear();
            until(&mut m, &mut out, |_, b| {
                events(b).iter().any(|e| {
                    matches!(
                        e,
                        Event::Observation {
                            subscription: SubscriptionId(1),
                            ..
                        }
                    )
                })
            });
        }
    }
    #[test]
    fn spatial_group_missing_then_ready_uses_new_sequence_same_acquisition_and_preserves_peer() {
        let gate = Arc::new(AtomicBool::new(false));
        let destroyed = Arc::new(AtomicUsize::new(0));
        let (g, d) = (gate.clone(), destroyed.clone());
        let factory: Factory = Arc::new(move |c| {
            let values = match c.topic.as_str() {
                "/tf_static" => vec![crate::spatial_worker::tests::tf("base", "laser", 0, 1.)],
                "/tf" => vec![
                    crate::spatial_worker::tests::tf("map", "base", 1, 1.),
                    crate::spatial_worker::tests::tf("map", "base", 3, 3.),
                ],
                "/scan" => vec![crate::spatial_worker::tests::scan(2)],
                _ => vec![Value::Unsigned { value: "1".into() }],
            };
            Ok(Box::new(SpatialFixtureReader {
                owner: thread::current().id(),
                values: values.into(),
                gate: if c.topic == "/scan" || c.topic == "/peer" {
                    Arc::new(AtomicBool::new(true))
                } else {
                    g.clone()
                },
                destroyed: d.clone(),
                _local: Rc::new(()),
            }))
        });
        let mut m = Manager::new(factory);
        let mut out = Vec::new();
        m.command(spatial_start(1, 1), &mut out).unwrap();
        m.command(start(2, 2, "/peer"), &mut out).unwrap();
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e|matches!(e,Event::SpatialObservation{sample,..} if matches!(sample.tf,spatial::TfOutcome::Missing)))
        });
        assert_eq!(m.reserved_handles(), 4);
        gate.store(true, Ordering::Release);
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e|matches!(e,Event::SpatialObservation{sample,..} if matches!(sample.tf,spatial::TfOutcome::Ready{..})))
        });
        let spatial: Vec<_> = events(&out)
            .into_iter()
            .filter_map(|e| {
                if let Event::SpatialObservation {
                    sequence, sample, ..
                } = e
                {
                    Some((sequence, sample))
                } else {
                    None
                }
            })
            .collect();
        assert!(spatial.last().unwrap().0 > spatial[0].0);
        assert_eq!(
            spatial[0].1.acquisition_stamp_ns,
            spatial.last().unwrap().1.acquisition_stamp_ns
        );
        let payload = EncodedPayload::Spatial(
            spatial
                .last()
                .unwrap()
                .1
                .encode()
                .unwrap()
                .into_boxed_slice(),
        );
        for sequence in 100..200 {
            m.queue
                .lock()
                .unwrap()
                .enqueue_sample(Generation(7), SubscriptionId(1), sequence, payload.clone())
                .unwrap();
        }
        out.clear();
        m.command(stop(4, 1), &mut out).unwrap();
        until(&mut m, &mut out, |m, _| m.workers.len() == 1);
        let final_events = events(&out);
        let drops_at=final_events.iter().position(|e|matches!(e,Event::Drops{subscription:SubscriptionId(1),backend_drops:84,spatial_counters:Some(c),..} if c.tf_accepted==3&&c.scans_received==1)).unwrap();
        let stopped_at = final_events
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        request: RequestId(4),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(drops_at < stopped_at);
        assert_eq!(destroyed.load(Ordering::SeqCst), 3);
        assert_eq!(m.workers[0].subscription, SubscriptionId(2));
        drop(m);
        assert_eq!(destroyed.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn spatial_cancel_during_second_open_reserves_three_until_join_and_has_no_partial_start() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let opened = Arc::new(AtomicUsize::new(0));
        let destroyed = Arc::new(AtomicUsize::new(0));
        let (e, r, o, d) = (
            entered.clone(),
            release.clone(),
            opened.clone(),
            destroyed.clone(),
        );
        let factory: Factory = Arc::new(move |_| {
            let n = o.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 2 {
                e.store(true, Ordering::Release);
                while !r.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            Ok(Box::new(SpatialFixtureReader {
                owner: thread::current().id(),
                values: Default::default(),
                gate: Arc::new(AtomicBool::new(true)),
                destroyed: d.clone(),
                _local: Rc::new(()),
            }))
        });
        let mut m = Manager::new(factory);
        let mut out = Vec::new();
        m.command(spatial_start(1, 1), &mut out).unwrap();
        until(&mut m, &mut out, |_, _| entered.load(Ordering::Acquire));
        m.command(stop(2, 1), &mut out).unwrap();
        m.command(start(3, 2, "/peer"), &mut out).unwrap();
        assert_eq!(m.reserved_handles(), 4);
        release.store(true, Ordering::Release);
        until(&mut m, &mut out, |m, out| {
            m.workers.len() == 1
                && events(out).iter().any(|event| {
                    matches!(
                        event,
                        Event::Started {
                            subscription: SubscriptionId(2),
                            ..
                        }
                    )
                })
        });
        assert_eq!(
            opened.load(Ordering::SeqCst),
            3,
            "Canceled group never opens its scan; ordinary peer opens once"
        );
        assert_eq!(destroyed.load(Ordering::SeqCst), 2);
        assert!(!events(&out).iter().any(|e| matches!(
            e,
            Event::Started {
                subscription: SubscriptionId(1),
                ..
            }
        )));
        let terminal: Vec<_> = events(&out)
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Drops {
                        subscription: SubscriptionId(1),
                        ..
                    } | Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
            .collect();
        assert!(matches!(
            &terminal[..],
            [
                Event::Drops {
                    spatial_counters: Some(_),
                    ..
                },
                Event::Stopped {
                    request: RequestId(2),
                    ..
                }
            ]
        ));
    }
    fn config(topic: &str) -> Configuration {
        Configuration {
            map_updates: None,
            topic: topic.into(),
            message_type: "std_msgs/msg/UInt64".into(),
            best_effort: false,
            transient_local: false,
            output: Output::Typed,
            spatial: None,
            robot: None,
        }
    }
    fn start(request: u64, id: u64, topic: &str) -> Command {
        Command::Start {
            generation: Generation(7),
            request: RequestId(request),
            subscription: SubscriptionId(id),
            configuration: config(topic),
        }
    }
    fn stop(request: u64, id: u64) -> Command {
        Command::Stop {
            generation: Generation(7),
            request: RequestId(request),
            subscription: SubscriptionId(id),
        }
    }
    fn events(bytes: &[u8]) -> Vec<Event> {
        bytes
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| Event::decode(b).unwrap())
            .collect()
    }
    fn until(manager: &mut Manager, out: &mut Vec<u8>, done: impl Fn(&Manager, &[u8]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done(manager, out) {
            assert!(
                Instant::now() < deadline,
                "worker deadline; events={:?}",
                events(out)
            );
            manager.progress(out).unwrap();
            thread::sleep(Duration::from_millis(1));
        }
    }
    struct Mock {
        owner: thread::ThreadId,
        drops: Arc<AtomicUsize>,
        sequence: u64,
        fails: bool,
        _not_send: Rc<()>,
    }
    impl Reader for Mock {
        fn poll(&mut self) -> Result<Option<Value>, String> {
            assert_eq!(thread::current().id(), self.owner);
            thread::sleep(Duration::from_millis(1));
            if self.fails {
                return Err("synthetic scoped failure".into());
            }
            self.sequence += 1;
            Ok(Some(Value::Unsigned {
                value: self.sequence.to_string(),
            }))
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            assert_eq!(thread::current().id(), self.owner);
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn manager() -> (Manager, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let opens = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let (o, d) = (opens.clone(), drops.clone());
        let factory: Factory = Arc::new(move |c| {
            o.fetch_add(1, Ordering::SeqCst);
            if c.topic == "/open_error" {
                return Err("synthetic open failure".into());
            }
            Ok(Box::new(Mock {
                owner: thread::current().id(),
                drops: d.clone(),
                sequence: 0,
                fails: c.topic == "/poll_error",
                _not_send: Rc::new(()),
            }))
        });
        (Manager::new(factory), opens, drops)
    }
    #[test]
    fn two_native_owners_progress_and_stop_does_not_reset_peer() {
        let (mut m, opens, drops) = manager();
        let mut out = Vec::new();
        m.command(start(1, 1, "/one"), &mut out).unwrap();
        m.command(start(2, 2, "/two"), &mut out).unwrap();
        until(&mut m, &mut out, |_, out| {
            events(out)
                .iter()
                .filter(|e| matches!(e, Event::Observation { .. }))
                .count()
                >= 8
        });
        let mut acknowledged = Vec::new();
        let mut sequences = HashMap::new();
        for e in events(&out) {
            match e {
                Event::Started { subscription, .. } => acknowledged.push(subscription),
                Event::Observation {
                    subscription,
                    sequence,
                    ..
                } => {
                    assert!(acknowledged.contains(&subscription));
                    assert!(sequence > *sequences.get(&subscription).unwrap_or(&0));
                    sequences.insert(subscription, sequence);
                }
                _ => (),
            }
        }
        assert_eq!(sequences.len(), 2);
        out.clear();
        m.command(stop(3, 1), &mut out).unwrap();
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
        });
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "stop ack follows Drop and join"
        );
        assert!(!events(&out).iter().any(|e| matches!(
            e,
            Event::Observation {
                subscription: SubscriptionId(1),
                ..
            }
        )));
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e| matches!(e, Event::Observation { subscription: SubscriptionId(2), sequence, .. } if *sequence > sequences[&SubscriptionId(2)]))
        });
        m.command(stop(4, 1), &mut out).unwrap();
        assert!(matches!(
            events(&out).last().unwrap(),
            Event::Stopped {
                request: RequestId(4),
                ..
            }
        ));
        m.command(start(5, 1, "/replayed"), &mut out).unwrap();
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert_eq!(m.workers.len(), 1);
        drop(m);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn capacity_invalid_configuration_and_scoped_failures_leave_peer_running() {
        let (mut m, opens, _) = manager();
        let mut out = Vec::new();
        for id in 1..=32 {
            m.command(start(id, id, "/live"), &mut out).unwrap();
        }
        m.command(start(33, 33, "/excess"), &mut out).unwrap();
        assert!(matches!(
            events(&out).last(),
            Some(Event::RequestError {
                subscription: SubscriptionId(33),
                no_handle: true,
                ..
            })
        ));
        until(&mut m, &mut out, |_, b| {
            events(b)
                .iter()
                .filter(|e| matches!(e, Event::Started { .. }))
                .count()
                == 32
        });
        assert_eq!(opens.load(Ordering::SeqCst), 32);
        m.command(stop(34, 1), &mut out).unwrap();
        until(&mut m, &mut out, |m, _| m.workers.len() == 31);
        m.command(start(35, 34, "/open_error"), &mut out).unwrap();
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e| {
                matches!(
                    e,
                    Event::RequestError {
                        subscription: SubscriptionId(34),
                        no_handle: true,
                        ..
                    }
                )
            })
        });
        m.command(start(36, 35, "/poll_error"), &mut out).unwrap();
        until(&mut m, &mut out, |_, b| {
            events(b).iter().any(|e| {
                matches!(
                    e,
                    Event::StreamError {
                        subscription: SubscriptionId(35),
                        ..
                    }
                )
            })
        });
        m.command(start(37, 36, "invalid"), &mut out).unwrap();
        assert!(matches!(
            events(&out).last(),
            Some(Event::RequestError {
                subscription: SubscriptionId(36),
                no_handle: true,
                ..
            })
        ));
        assert_eq!(m.workers.len(), 31);
        assert_eq!(opens.load(Ordering::SeqCst), 34);
    }
    #[test]
    fn cancellation_while_opening_waits_for_owner_cleanup_and_never_starts() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let (e, r, d) = (entered.clone(), release.clone(), drops.clone());
        let factory: Factory = Arc::new(move |_| {
            e.store(true, Ordering::Release);
            while !r.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(Box::new(Mock {
                owner: thread::current().id(),
                drops: d.clone(),
                sequence: 0,
                fails: false,
                _not_send: Rc::new(()),
            }))
        });
        let mut m = Manager::new(factory);
        let mut out = Vec::new();
        m.command(start(1, 1, "/one"), &mut out).unwrap();
        until(&mut m, &mut out, |_, _| entered.load(Ordering::Acquire));
        m.command(stop(2, 1), &mut out).unwrap();
        m.progress(&mut out).unwrap();
        assert!(matches!(
            &events(&out)[..],
            [Event::Drops {
                backend_drops: 0,
                ..
            }]
        ));
        assert_eq!(m.workers.len(), 1, "stopping owns capacity until cleanup");
        release.store(true, Ordering::Release);
        until(&mut m, &mut out, |m, _| m.workers.is_empty());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(matches!(
            &events(&out)[..],
            [
                Event::Drops {
                    backend_drops: 0,
                    ..
                },
                Event::Stopped {
                    request: RequestId(2),
                    ..
                }
            ]
        ));
    }
    #[test]
    fn ingress_rejects_unbounded_incomplete_and_non_observation_commands() {
        let command = stop(1, 1).encode().unwrap();
        assert_eq!(read_command(&mut &command[..]).unwrap(), Some(stop(1, 1)));
        assert_eq!(read_command(&mut &b""[..]).unwrap(), None);
        assert!(read_command(&mut &command[..command.len() - 1]).is_err());
        assert!(read_command(&mut &vec![b'x'; session::MAX_COMMAND_BYTES + 1][..]).is_err());
        assert!(read_command(&mut &b"{\"command\":\"publish\"}\n"[..]).is_err());
    }

    #[test]
    fn final_backend_drop_snapshot_precedes_stop_cleanup_ack() {
        let (mut manager, _, drops) = manager();
        let mut out = Vec::new();
        manager.command(start(1, 1, "/one"), &mut out).unwrap();
        // No progress means the native owner remains behind its release gate.
        // Inject bounded valid payloads into that registered stream directly.
        let bytes = Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/one".into(),
            message_type: "std_msgs/msg/UInt64".into(),
            source_time: None,
            value: Value::Unsigned { value: "1".into() },
        }
        .encode()
        .unwrap()
        .into_boxed_slice();
        for sequence in 1..=100 {
            manager
                .queue
                .lock()
                .unwrap()
                .enqueue_sample(
                    Generation(7),
                    SubscriptionId(1),
                    sequence,
                    EncodedPayload::Typed(bytes.clone()),
                )
                .unwrap();
        }
        assert_eq!(
            manager
                .queue
                .lock()
                .unwrap()
                .stream_counters(SubscriptionId(1))
                .unwrap()
                .evicted_samples,
            84
        );
        manager.command(stop(2, 1), &mut out).unwrap();
        until(&mut manager, &mut out, |m, _| m.workers.is_empty());
        let frames: Vec<serde_json::Value> = out
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect();
        assert_eq!(
            frames
                .first()
                .and_then(|f| f.get("event"))
                .and_then(|f| f.as_str()),
            Some("drops"),
            "final backend queue drops must be delivered before cleanup acknowledgment"
        );
        assert_eq!(frames[0]["backend_drops"], 84);
        assert_eq!(frames[1]["event"], "stopped");
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runtime_failure_preserves_final_backend_drops_and_peer_progress() {
        struct FailWhen(Arc<AtomicBool>);
        impl Reader for FailWhen {
            fn poll(&mut self) -> Result<Option<Value>, String> {
                thread::sleep(Duration::from_millis(1));
                if self.0.load(Ordering::Acquire) {
                    Err("controlled worker failure".into())
                } else {
                    Ok(None)
                }
            }
        }
        let fail = Arc::new(AtomicBool::new(false));
        let flag = fail.clone();
        let factory: Factory = Arc::new(move |config| {
            if config.topic == "/one" {
                Ok(Box::new(FailWhen(flag.clone())))
            } else {
                Ok(Box::new(Mock {
                    owner: thread::current().id(),
                    drops: Arc::new(AtomicUsize::new(0)),
                    sequence: 0,
                    fails: false,
                    _not_send: Rc::new(()),
                }))
            }
        });
        let mut manager = Manager::new(factory);
        let mut out = Vec::new();
        manager.command(start(1, 1, "/one"), &mut out).unwrap();
        manager.command(start(2, 2, "/peer"), &mut out).unwrap();
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes)
                .iter()
                .filter(|e| matches!(e, Event::Started { .. }))
                .count()
                == 2
        });
        out.clear();
        let bytes = Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/one".into(),
            message_type: "std_msgs/msg/UInt64".into(),
            source_time: None,
            value: Value::Unsigned { value: "1".into() },
        }
        .encode()
        .unwrap()
        .into_boxed_slice();
        for sequence in 1..=100 {
            manager
                .queue
                .lock()
                .unwrap()
                .enqueue_sample(
                    Generation(7),
                    SubscriptionId(1),
                    sequence,
                    EncodedPayload::Typed(bytes.clone()),
                )
                .unwrap();
        }
        fail.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager
            .workers
            .iter()
            .find(|w| w.subscription == SubscriptionId(1))
            .unwrap()
            .join
            .is_finished()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        manager.progress(&mut out).unwrap();
        assert!(matches!(
            &events(&out)[..2],
            [
                Event::Drops {
                    subscription: SubscriptionId(1),
                    backend_drops: 84,
                    ..
                },
                Event::StreamError {
                    subscription: SubscriptionId(1),
                    ..
                }
            ]
        ));
        assert_eq!(manager.workers.len(), 1);
        assert!(manager
            .queue
            .lock()
            .unwrap()
            .stream_counters(SubscriptionId(2))
            .is_some());
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::Observation {
                        subscription: SubscriptionId(2),
                        ..
                    }
                )
            })
        });
    }

    #[test]
    fn open_failure_has_zero_snapshot_before_definitive_error_and_retry_does_not_repeat_it() {
        let (mut manager, _, _) = manager();
        let mut out = Vec::new();
        manager
            .command(start(1, 1, "/open_error"), &mut out)
            .unwrap();
        until(&mut manager, &mut out, |m, _| m.workers.is_empty());
        assert!(matches!(
            &events(&out)[..],
            [
                Event::Drops {
                    backend_drops: 0,
                    ..
                },
                Event::RequestError {
                    no_handle: true,
                    ..
                }
            ]
        ));
        out.clear();
        manager.command(stop(2, 1), &mut out).unwrap();
        assert!(matches!(
            &events(&out)[..],
            [Event::Stopped {
                request: RequestId(2),
                ..
            }]
        ));
    }

    #[test]
    fn stale_requests_and_foreign_generations_cannot_change_live_owners() {
        let (mut m, opens, _) = manager();
        let mut out = Vec::new();
        m.command(start(1, 1, "/one"), &mut out).unwrap();
        until(&mut m, &mut out, |_, bytes| {
            events(bytes)
                .iter()
                .any(|e| matches!(e, Event::Started { .. }))
        });
        out.clear();
        m.command(start(1, 1, "/one"), &mut out).unwrap();
        assert!(matches!(
            &events(&out)[..],
            [Event::RequestError {
                no_handle: false,
                ..
            }]
        ));
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        let mut foreign = stop(2, 1);
        if let Command::Stop { generation, .. } = &mut foreign {
            *generation = Generation(6);
        }
        assert!(m.command(foreign, &mut out).is_err());
        assert!(m.workers[0].stop_request.is_none());
        until(&mut m, &mut out, |_, bytes| {
            events(bytes)
                .iter()
                .any(|e| matches!(e, Event::Observation { .. }))
        });
    }

    struct ImageMock {
        owner: thread::ThreadId,
        topic: String,
        sequence: u64,
        typed: Arc<AtomicUsize>,
        images: Arc<AtomicUsize>,
        destroyed: Arc<AtomicUsize>,
        malformed: bool,
    }
    impl Reader for ImageMock {
        fn poll(&mut self) -> Result<Option<Value>, String> {
            assert_eq!(self.owner, thread::current().id());
            self.typed.fetch_add(1, Ordering::SeqCst);
            self.sequence += 1;
            thread::sleep(Duration::from_millis(1));
            Ok(Some(Value::Unsigned {
                value: self.sequence.to_string(),
            }))
        }
        fn poll_image(&mut self) -> Result<Option<image::ImageSample>, String> {
            assert_eq!(self.owner, thread::current().id());
            self.images.fetch_add(1, Ordering::SeqCst);
            self.sequence += 1;
            thread::sleep(Duration::from_millis(1));
            let encodings = [
                ("mono8", 1),
                ("rgb8", 3),
                ("bgr8", 3),
                ("rgba8", 4),
                ("bgra8", 4),
            ];
            let (encoding, channels) = encodings[(self.sequence as usize - 1) % encodings.len()];
            let step = 2 * channels + 2;
            let raw: Vec<_> = (0..step * 2).map(|i| i as u8).collect();
            let mut sample = image::ImageSample::from_raw(
                &self.topic,
                image::ImageMetadata {
                    header_stamp: image::HeaderStamp {
                        seconds: 7,
                        nanoseconds: self.sequence as u32,
                    },
                    frame_id: "camera_optical".into(),
                    width: 2,
                    height: 2,
                    encoding: encoding.into(),
                    is_bigendian: self.sequence.is_multiple_of(2),
                    step,
                },
                &raw,
            )?;
            if self.malformed {
                sample.complete = false;
            }
            Ok(Some(sample))
        }
    }
    impl Drop for ImageMock {
        fn drop(&mut self) {
            assert_eq!(self.owner, thread::current().id());
            self.destroyed.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn image_manager() -> (
        Manager,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let typed = Arc::new(AtomicUsize::new(0));
        let images = Arc::new(AtomicUsize::new(0));
        let destroyed = Arc::new(AtomicUsize::new(0));
        let (t, i, d) = (typed.clone(), images.clone(), destroyed.clone());
        let factory: Factory = Arc::new(move |configuration| {
            Ok(Box::new(ImageMock {
                owner: thread::current().id(),
                topic: configuration.topic.clone(),
                sequence: 0,
                typed: t.clone(),
                images: i.clone(),
                destroyed: d.clone(),
                malformed: configuration.topic == "/bad_image",
            }))
        });
        (Manager::new(factory), typed, images, destroyed)
    }
    fn image_start(request: u64, id: u64, topic: &str, output: Output) -> Command {
        let mut configuration = config(topic);
        configuration.message_type = image::MESSAGE_TYPE.into();
        configuration.output = output;
        Command::Start {
            generation: Generation(7),
            request: RequestId(request),
            subscription: SubscriptionId(id),
            configuration,
        }
    }
    #[test]
    fn explicit_compact_polling_preserves_five_encodings_and_typed_image_peer_on_stop() {
        let (mut manager, typed, images, destroyed) = image_manager();
        let mut out = Vec::new();
        manager
            .command(image_start(1, 1, "/camera", Output::CompactImage), &mut out)
            .unwrap();
        manager
            .command(image_start(2, 2, "/camera", Output::Typed), &mut out)
            .unwrap();
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes)
                .iter()
                .filter(|e| matches!(e, Event::ImageObservation { .. }))
                .count()
                >= 5
        });
        let mut kinds = std::collections::HashSet::new();
        let mut last_image = 0;
        let mut peer_sequence = 0;
        let mut started = Vec::new();
        for event in events(&out) {
            match event {
                Event::Started { subscription, .. } => started.push(subscription),
                Event::ImageObservation {
                    subscription: SubscriptionId(1),
                    sequence,
                    sample,
                    ..
                } => {
                    assert!(started.contains(&SubscriptionId(1)));
                    assert!(sequence > last_image);
                    last_image = sequence;
                    assert_eq!(sample.metadata.header_stamp.nanoseconds, sequence as u32);
                    assert_eq!(sample.metadata.frame_id, "camera_optical");
                    assert_eq!(sample.topic, "/camera");
                    assert_eq!(
                        sample.raw_bytes().unwrap(),
                        (0..sample.original_bytes)
                            .map(|i| i as u8)
                            .collect::<Vec<_>>()
                    );
                    kinds.insert(sample.metadata.encoding);
                }
                Event::Observation {
                    subscription: SubscriptionId(2),
                    sequence,
                    sample,
                    ..
                } => {
                    assert_eq!(sample.message_type, image::MESSAGE_TYPE);
                    peer_sequence = sequence;
                }
                Event::Observation { .. } | Event::ImageObservation { .. } => {
                    panic!("Output kinds crossed subscription identities")
                }
                _ => {}
            }
        }
        assert_eq!(kinds.len(), 5);
        assert!(typed.load(Ordering::SeqCst) > 0);
        assert!(images.load(Ordering::SeqCst) >= 5);
        out.clear();
        manager.command(stop(3, 1), &mut out).unwrap();
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
        });
        let all = events(&out);
        let drops = all
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Drops {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
            .unwrap();
        let stopped = all
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(drops < stopped);
        assert_eq!(destroyed.load(Ordering::SeqCst), 1);
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e|matches!(e,Event::Observation{subscription:SubscriptionId(2),sequence,..} if *sequence>peer_sequence))
        });
        drop(manager);
        assert_eq!(destroyed.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn invalid_compact_type_never_opens_and_malformed_image_failure_is_scoped() {
        let (mut manager, typed, images, destroyed) = image_manager();
        let mut out = Vec::new();
        let mut invalid = config("/not_image");
        invalid.output = Output::CompactImage;
        manager
            .command(
                Command::Start {
                    generation: Generation(7),
                    request: RequestId(1),
                    subscription: SubscriptionId(1),
                    configuration: invalid,
                },
                &mut out,
            )
            .unwrap();
        assert!(matches!(
            &events(&out)[..],
            [Event::RequestError {
                no_handle: true,
                ..
            }]
        ));
        assert!(manager.workers.is_empty());
        assert_eq!(images.load(Ordering::SeqCst), 0);
        assert_eq!(typed.load(Ordering::SeqCst), 0);
        manager.command(start(2, 2, "/peer"), &mut out).unwrap();
        manager
            .command(
                image_start(3, 3, "/bad_image", Output::CompactImage),
                &mut out,
            )
            .unwrap();
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::StreamError {
                        subscription: SubscriptionId(3),
                        ..
                    }
                )
            })
        });
        let all = events(&out);
        let failed = all
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Event::StreamError {
                        subscription: SubscriptionId(3),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(matches!(
            all[failed - 1],
            Event::Drops {
                subscription: SubscriptionId(3),
                backend_drops: 0,
                ..
            }
        ));
        assert_eq!(destroyed.load(Ordering::SeqCst), 1);
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::Observation {
                        subscription: SubscriptionId(2),
                        ..
                    }
                )
            })
        });
        assert_eq!(manager.workers.len(), 1);
    }
    #[test]
    fn compact_payload_byte_flood_keeps_peer_capacity_and_final_drop_snapshot() {
        let (mut manager, _, _, _) = image_manager();
        let mut out = Vec::new();
        manager
            .command(image_start(1, 1, "/camera", Output::CompactImage), &mut out)
            .unwrap();
        manager.command(start(2, 2, "/peer"), &mut out).unwrap();
        let sample = image::ImageSample::from_raw(
            "/camera",
            image::ImageMetadata {
                header_stamp: image::HeaderStamp {
                    seconds: 1,
                    nanoseconds: 0,
                },
                frame_id: "optical".into(),
                width: 256,
                height: 256,
                encoding: "rgba8".into(),
                is_bigendian: false,
                step: 1024,
            },
            &vec![255; 256 * 256 * 4],
        )
        .unwrap();
        let payload = EncodedPayload::CompactImage(sample.encode().unwrap().into_boxed_slice());
        for sequence in 1..=100 {
            manager
                .queue
                .lock()
                .unwrap()
                .enqueue_sample(Generation(7), SubscriptionId(1), sequence, payload.clone())
                .unwrap();
        }
        let counters = manager
            .queue
            .lock()
            .unwrap()
            .stream_counters(SubscriptionId(1))
            .unwrap();
        assert!(counters.evicted_samples > 0);
        assert!(manager.queue.lock().unwrap().sample_bytes() <= AGGREGATE_SAMPLE_BYTES);
        manager.command(stop(3, 1), &mut out).unwrap();
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::Stopped {
                        subscription: SubscriptionId(1),
                        ..
                    }
                )
            })
        });
        assert!(
            matches!(events(&out)[0],Event::Drops{backend_drops,..} if backend_drops==counters.evicted_samples)
        );
        until(&mut manager, &mut out, |_, bytes| {
            events(bytes).iter().any(|e| {
                matches!(
                    e,
                    Event::Observation {
                        subscription: SubscriptionId(2),
                        ..
                    }
                )
            })
        });
    }
    #[test]
    fn compact_worker_preserves_unsupported_encoding_but_rejects_oversize_and_wrong_identity() {
        struct One(Option<image::ImageSample>);
        impl Reader for One {
            fn poll(&mut self) -> Result<Option<Value>, String> {
                panic!("Explicit compact output must never use typed polling")
            }
            fn poll_image(&mut self) -> Result<Option<image::ImageSample>, String> {
                Ok(self.0.take())
            }
        }
        let mut configuration = config("/camera");
        configuration.message_type = image::MESSAGE_TYPE.into();
        configuration.output = Output::CompactImage;
        let sample = image::ImageSample::from_raw(
            "/camera",
            image::ImageMetadata {
                header_stamp: image::HeaderStamp {
                    seconds: 0,
                    nanoseconds: 0,
                },
                frame_id: "optical".into(),
                width: 1,
                height: 1,
                encoding: "16UC1".into(),
                is_bigendian: true,
                step: 2,
            },
            &[1, 2],
        )
        .unwrap();
        let encoded = poll_encoded(&mut One(Some(sample.clone())), &configuration)
            .unwrap()
            .unwrap();
        assert_eq!(encoded.event_name(), "image_observation");
        let decoded = image::ImageSample::decode(encoded.bytes()).unwrap();
        assert_eq!(decoded.metadata.encoding, "16UC1");
        assert_eq!(decoded.raw_bytes().unwrap(), [1, 2]);
        let mut too_large = sample.clone();
        too_large.metadata.width = 2048;
        too_large.metadata.height = 2048;
        too_large.metadata.step = 8193;
        assert!(poll_encoded(&mut One(Some(too_large)), &configuration)
            .err()
            .expect("Oversize compact image must fail")
            .contains("budget"));
        let mut wrong = sample;
        wrong.topic = "/other".into();
        assert!(poll_encoded(&mut One(Some(wrong)), &configuration)
            .err()
            .expect("Wrong compact identity must fail")
            .contains("identity"));
    }
}
