//! Session-v17 ingress. Decoding, identity validation and queue pressure stay off UI frames.
use std::{
    io::{BufRead, BufReader, Read},
    sync::{mpsc, Arc, Mutex},
    time::Instant,
};
use studio_images::images;
use studio_protocol::{
    session::{Event, MAX_EVENT_BYTES as MAX_LINE},
    subscriptions::*,
};
#[cfg(feature = "spatial")]
mod spatial_pipe;
#[cfg(feature = "spatial")]
pub use spatial_pipe::SpatialTarget;
#[cfg(feature = "spatial")]
mod cloud_pipe;
#[cfg(feature = "spatial")]
pub use cloud_pipe::CloudTarget;
#[cfg(feature = "spatial")]
mod mapping_pipe;
#[cfg(feature = "spatial")]
pub use mapping_pipe::MappingTarget;

mod description_pipe;
pub use description_pipe::{CapturedDescription, DescriptionStream, DescriptionTarget};
mod marker_pipe;
pub use marker_pipe::{MarkerResult, MarkerStream, MarkerTarget, MarkerUpdate, MarkerVisibility};
mod map_pipe;
#[cfg(feature = "spatial")]
pub use map_pipe::PreparedMapUpdate;
pub use map_pipe::{MapResult, MapStream, MapTarget, MapUpdate};
mod camera_pipe;
pub use camera_pipe::CameraTarget;
mod pose_pipe;
pub use pose_pipe::{PoseResult, PoseStream, PoseTarget, PoseUpdate};

#[derive(Clone)]
pub struct ImageTarget {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub stream: images::ImageStream,
}

pub struct TimedEvent {
    pub event: Event,
    pub received_at: Instant,
    /// Bounded terminal snapshot; no retired-subscription map is retained.
    pub final_counters: Option<(SubscriptionId, Counters)>,
    payload_bytes: usize,
}
impl Payload for TimedEvent {
    fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
    fn is_image(&self) -> bool {
        matches!(
            self.event,
            Event::ImageObservation { .. } | Event::CameraSnapshotObservation { .. }
        )
    }
}
/// A new owner for every launch prevents a late old reader from touching a reconnect.
pub struct Shared {
    pub registry: Registry,
    pub queue: FairQueue<TimedEvent>,
    /// Discovery has its own coalesced, one-frame slot; it is not a 4 KiB control.
    pub graph: Option<Vec<(String, Vec<String>)>>,
    pub tool_events: std::collections::VecDeque<Event>,
    pub scene_frames: Option<Vec<String>>,
    pub ingress_rejections: u64,
    /// One latched fault per allocated Robot owner, separate from data pressure.
    pub robot_faults: Vec<RobotIngressFault>,
    pub cloud_faults: Vec<CloudIngressFault>,
    pub mapping_faults: Vec<MappingIngressFault>,
    /// At most one generation-bound decoded mailbox for each native owner.
    pub images: Vec<ImageTarget>,
    pub descriptions: Vec<DescriptionTarget>,
    pub maps: Vec<MapTarget>,
    pub markers: Vec<MarkerTarget>,
    pub poses: Vec<PoseTarget>,
    pub cameras: Vec<CameraTarget>,
    pub map_faults: Vec<RobotIngressFault>,
    #[cfg(feature = "spatial")]
    pub spatials: Vec<SpatialTarget>,
    #[cfg(feature = "spatial")]
    pub clouds: Vec<CloudTarget>,
    #[cfg(feature = "spatial")]
    pub mappings: Vec<MappingTarget>,
    pub repaint: Option<eframe::egui::Context>,
}
pub struct RobotIngressFault {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub message: String,
}
pub type CloudIngressFault = RobotIngressFault;
pub type MappingIngressFault = RobotIngressFault;

// Cloud/mapping geometry is validated by its worker after owner/sequence checks. Invalid
// sample content is a scoped fatal error, never a disconnect of unrelated owners.
fn decode_event(frame: &[u8]) -> Result<Event, String> {
    if frame.len() > MAX_LINE {
        return Err("Backend output exceeded protocol line limit".into());
    }
    let event: Event = serde_json::from_slice(frame).map_err(|error| error.to_string())?;
    if let Event::PoseCleared {
        generation,
        subscription,
        request,
        ..
    } = &event
    {
        if generation.0 == 0 || subscription.0 == 0 || request.0 == 0 {
            return Err("Pose Clear requires nonzero owner/request".into());
        }
        return Ok(event);
    }
    if let Event::CameraSnapshotObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::PoseSnapshotObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::PointCloudObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::RobotDescriptionObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::MarkerSnapshotObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::MapSnapshotObservation {
        generation,
        subscription,
        sequence,
        ..
    }
    | Event::MappingObservation {
        generation,
        subscription,
        sequence,
        ..
    } = &event
    {
        if generation.0 == 0 || subscription.0 == 0 || *sequence == 0 {
            return Err("Geometry observation requires nonzero owner and sequence".into());
        }
    } else {
        event.validate()?;
    }
    Ok(event)
}
impl Shared {
    /// Use the production frame validator and ingress mutation in App tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn ingest_for_test(&mut self, event: Event) -> Result<(), String> {
        let mut frame = serde_json::to_vec(&event).map_err(|error| error.to_string())?;
        frame.push(b'\n');
        let event = decode_event(&frame)?;
        if let Event::CameraSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = camera_pipe::prepare(self, generation, subscription, sequence) {
                if let Some(result) = camera_pipe::decode(&target, *sample, backend_drops) {
                    camera_pipe::publish(self, &target, result);
                }
            }
            return Ok(());
        }
        if let Event::PoseSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = pose_pipe::prepare(self, generation, subscription, sequence) {
                if let Some(result) = pose_pipe::decode(&target, &sample, backend_drops) {
                    pose_pipe::publish(self, &target, result);
                }
            }
            return Ok(());
        }
        if let Event::MarkerSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = marker_pipe::prepare(self, generation, subscription, sequence) {
                if let Some(result) = marker_pipe::decode(&target, &sample, backend_drops) {
                    marker_pipe::publish(self, &target, result);
                }
            }
            return Ok(());
        }
        if let Event::MapSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = map_pipe::prepare(self, generation, subscription, sequence) {
                if let Some(result) = map_pipe::decode(&target, &sample, backend_drops) {
                    map_pipe::publish(self, &target, result);
                }
            }
            return Ok(());
        }
        if let Event::RobotDescriptionObservation {
            generation,
            subscription,
            sequence,
            sample,
            ..
        } = event
        {
            if let Some(target) =
                description_pipe::prepare(self, generation, subscription, sequence)
            {
                description_pipe::accept(&target, &sample, sequence);
            }
            return Ok(());
        }
        #[cfg(feature = "spatial")]
        if let Event::MappingObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = mapping_pipe::prepare(
                self,
                generation,
                subscription,
                sequence,
                backend_drops,
                &sample,
            ) {
                let decoded =
                    studio_visualization::spatial::mapping_live::decode_mapping_sample(&sample);
                mapping_pipe::publish(self, &target, sequence, backend_drops, decoded);
            }
            return Ok(());
        }
        #[cfg(feature = "spatial")]
        if let Event::PointCloudObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = cloud_pipe::prepare(
                self,
                generation,
                subscription,
                sequence,
                backend_drops,
                &sample,
            ) {
                let decoded =
                    studio_visualization::spatial::cloud_live::decode_cloud_sample(&sample);
                cloud_pipe::publish(self, &target, sequence, backend_drops, decoded);
            }
            return Ok(());
        }
        #[cfg(feature = "spatial")]
        if let Event::SpatialObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) =
                spatial_pipe::prepare(self, generation, subscription, sequence, &sample)
            {
                let decoded = studio_visualization::spatial::decode_spatial_sample(&sample);
                spatial_pipe::publish(self, &target, sequence, backend_drops, decoded);
            }
            return Ok(());
        }
        if let Event::ImageObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            if let Some(target) = prepare_image(self, generation, subscription, sequence, &sample) {
                let decoded = images::decode_compact_image(&sample);
                publish_image(self, &target, sequence, backend_drops, decoded);
            }
            return Ok(());
        }
        ingest(self, event, Instant::now(), frame.len())
    }
    pub fn new(prior_generation: u64) -> Self {
        Self {
            registry: Registry::with_prior_generation(prior_generation),
            queue: FairQueue::with_payload(Generation(prior_generation)).with_image_budget(),
            graph: None,
            tool_events: std::collections::VecDeque::new(),
            scene_frames: None,
            ingress_rejections: 0,
            robot_faults: Vec::new(),
            cloud_faults: Vec::new(),
            mapping_faults: Vec::new(),
            images: Vec::new(),
            descriptions: Vec::new(),
            maps: Vec::new(),
            markers: Vec::new(),
            poses: Vec::new(),
            cameras: Vec::new(),
            map_faults: Vec::new(),
            #[cfg(feature = "spatial")]
            spatials: Vec::new(),
            #[cfg(feature = "spatial")]
            clouds: Vec::new(),
            #[cfg(feature = "spatial")]
            mappings: Vec::new(),
            repaint: None,
        }
    }
    pub fn bind_image(&mut self, target: ImageTarget) -> Result<(), String> {
        if self.images.len() >= MAX_SUBSCRIPTIONS
            || self
                .images
                .iter()
                .any(|current| current.subscription == target.subscription)
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.generation
                    && entry.subscription == target.subscription
                    && entry.configuration.topic == target.topic
                    && entry.configuration.output == Output::CompactImage
            })
        {
            return Err("Image binding does not match an allocated native owner".into());
        }
        self.images.push(target);
        Ok(())
    }
}
pub fn spawn(
    reader: impl Read + Send + 'static,
    shared: Arc<Mutex<Shared>>,
    terminal: mpsc::SyncSender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let message = pump(reader, &shared);
        // EOF/protocol failure invalidates map transfer state before notifying
        // the UI. Native registry reservations still await normal cleanup.
        for target in &shared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .maps
        {
            target.stream.stop();
        }
        for target in &shared.lock().unwrap_or_else(|e| e.into_inner()).markers {
            target.stream.stop();
        }
        for target in &shared.lock().unwrap_or_else(|e| e.into_inner()).cameras {
            target.stream.stop();
        }
        for target in &shared.lock().unwrap_or_else(|e| e.into_inner()).poses {
            target.stream.stop();
        }
        // Independent of all sample, control and diagnostic lanes, including EOF.
        let _ = terminal.try_send(message);
    })
}
fn pump(reader: impl Read, shared: &Mutex<Shared>) -> String {
    let mut reader = BufReader::new(reader);
    let mut greeted = false;
    loop {
        let mut line = Vec::new();
        match reader
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => return "Backend output closed; reconnect explicitly".into(),
            Err(e) => return format!("Backend read failed: {e}; reconnect explicitly"),
            _ => {}
        }
        let received_at = Instant::now();
        if line.len() > MAX_LINE {
            return "Backend exceeded protocol line limit; reconnect explicitly".into();
        }
        if !line.ends_with(b"\n") {
            return "Backend closed with an incomplete protocol line; reconnect explicitly".into();
        }
        let event = match decode_event(&line) {
            Ok(event) => event,
            Err(e) => {
                return format!(
                    "Invalid or incompatible backend protocol: {e}; reconnect explicitly"
                )
            }
        };
        // A validated startup failure can precede Ready (for example native
        // middleware initialization). Preserve its useful bounded diagnostic.
        if let Event::Error { message } = event {
            return format!("Backend error: {message}");
        }
        match &event {
            Event::Ready { .. } if !greeted => greeted = true,
            Event::Ready { .. } => {
                return "Repeated backend handshake; reconnect explicitly".into()
            }
            _ if !greeted => return "Backend event before handshake; reconnect explicitly".into(),
            _ => {}
        }
        if let Event::CameraSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                (
                    camera_pipe::prepare(&mut state, generation, subscription, sequence),
                    state.repaint.clone(),
                )
            };
            if let Some(target) = target {
                let result = camera_pipe::decode(&target, *sample, backend_drops);
                drop(line);
                if let Some(result) = result {
                    camera_pipe::publish(
                        &mut shared.lock().unwrap_or_else(|e| e.into_inner()),
                        &target,
                        result,
                    );
                }
            }
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
            continue;
        }
        if let Event::PoseSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                (
                    pose_pipe::prepare(&mut state, generation, subscription, sequence),
                    state.repaint.clone(),
                )
            };
            if let Some(target) = target {
                let result = pose_pipe::decode(&target, &sample, backend_drops);
                drop(sample);
                drop(line);
                if let Some(result) = result {
                    pose_pipe::publish(
                        &mut shared.lock().unwrap_or_else(|e| e.into_inner()),
                        &target,
                        result,
                    );
                }
            }
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
            continue;
        }
        if let Event::MarkerSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                (
                    marker_pipe::prepare(&mut state, generation, subscription, sequence),
                    state.repaint.clone(),
                )
            };
            if let Some(target) = target {
                let result = marker_pipe::decode(&target, &sample, backend_drops);
                drop(sample);
                drop(line);
                if let Some(result) = result {
                    marker_pipe::publish(
                        &mut shared.lock().unwrap_or_else(|error| error.into_inner()),
                        &target,
                        result,
                    );
                }
            }
            if let Some(context) = repaint {
                context.request_repaint();
            }
            continue;
        }
        if let Event::MapSnapshotObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                (
                    map_pipe::prepare(&mut state, generation, subscription, sequence),
                    state.repaint.clone(),
                )
            };
            if let Some(target) = target {
                let result = map_pipe::decode(&target, &sample, backend_drops);
                drop(sample);
                drop(line);
                if let Some(result) = result {
                    map_pipe::publish(
                        &mut shared.lock().unwrap_or_else(|error| error.into_inner()),
                        &target,
                        result,
                    );
                }
            }
            if let Some(context) = repaint {
                context.request_repaint();
            }
            continue;
        }
        if let Event::RobotDescriptionObservation {
            generation,
            subscription,
            sequence,
            sample,
            ..
        } = event
        {
            let (target, repaint) = {
                let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                (
                    description_pipe::prepare(&mut state, generation, subscription, sequence),
                    state.repaint.clone(),
                )
            };
            if let Some(target) = target {
                description_pipe::accept(&target, &sample, sequence);
            }
            if let Some(context) = repaint {
                context.request_repaint();
            }
            continue;
        }
        #[cfg(feature = "spatial")]
        if let Event::MappingObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut shared = shared.lock().unwrap_or_else(|error| error.into_inner());
                let target = mapping_pipe::prepare(
                    &mut shared,
                    generation,
                    subscription,
                    sequence,
                    backend_drops,
                    &sample,
                );
                (target, shared.repaint.clone())
            };
            if let Some(target) = target {
                let decoded =
                    studio_visualization::spatial::mapping_live::decode_mapping_sample(&sample);
                drop(sample);
                drop(line);
                let repaint = {
                    let mut shared = shared.lock().unwrap_or_else(|error| error.into_inner());
                    mapping_pipe::publish(&mut shared, &target, sequence, backend_drops, decoded)
                        .then(|| shared.repaint.clone())
                        .flatten()
                };
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            } else if let Some(context) = repaint {
                context.request_repaint();
            }
            continue;
        }
        #[cfg(feature = "spatial")]
        if let Event::PointCloudObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let (target, repaint) = {
                let mut shared = shared.lock().unwrap_or_else(|error| error.into_inner());
                let target = cloud_pipe::prepare(
                    &mut shared,
                    generation,
                    subscription,
                    sequence,
                    backend_drops,
                    &sample,
                );
                (target, shared.repaint.clone())
            };
            if let Some(target) = target {
                let decoded =
                    studio_visualization::spatial::cloud_live::decode_cloud_sample(&sample);
                drop(sample);
                drop(line);
                let repaint = {
                    let mut shared = shared.lock().unwrap_or_else(|error| error.into_inner());
                    cloud_pipe::publish(&mut shared, &target, sequence, backend_drops, decoded)
                        .then(|| shared.repaint.clone())
                        .flatten()
                };
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            } else if let Some(context) = repaint {
                // Includes a newly scoped identity fault; wake its cleanup owner.
                context.request_repaint();
            }
            continue;
        }
        #[cfg(feature = "spatial")]
        if let Event::SpatialObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let target = {
                let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                spatial_pipe::prepare(&mut state, generation, subscription, sequence, &sample)
            };
            if let Some(target) = target {
                let decoded = studio_visualization::spatial::decode_spatial_sample(&sample);
                drop(sample);
                drop(line);
                let repaint = {
                    let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                    spatial_pipe::publish(&mut state, &target, sequence, backend_drops, decoded)
                        .then(|| state.repaint.clone())
                        .flatten()
                };
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            }
            continue;
        }
        if let Event::ImageObservation {
            generation,
            subscription,
            sequence,
            sample,
            backend_drops,
        } = event
        {
            let target = {
                let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                prepare_image(&mut state, generation, subscription, sequence, &sample)
            };
            if let Some(target) = target {
                // Bounded base64/pixel conversion never holds the mutex the UI
                // uses for lifecycle and controls. Do not retain encoded history.
                let decoded = images::decode_compact_image(&sample);
                drop(sample);
                drop(line);
                let repaint = {
                    let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                    publish_image(&mut state, &target, sequence, backend_drops, decoded)
                        .then(|| state.repaint.clone())
                        .flatten()
                };
                // Repaint callbacks may lock egui state. Never call them while
                // holding the registry mutex the UI uses for its controls.
                if let Some(context) = repaint {
                    context.request_repaint();
                }
            }
            continue;
        }
        let mut shared = shared.lock().unwrap_or_else(|error| error.into_inner());
        if let Err(error) = ingest(&mut shared, event, received_at, line.len()) {
            return error;
        }
    }
}
fn prepare_image(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
    sample: &studio_protocol::image::ImageSample,
) -> Option<ImageTarget> {
    let identity = shared.registry.entries().iter().any(|entry| {
        entry.generation == generation
            && entry.subscription == subscription
            && entry.configuration.topic == sample.topic
            && entry.configuration.output == Output::CompactImage
            && entry.configuration.message_type == images::IMAGE_MESSAGE_TYPE
    });
    let target = shared
        .images
        .iter()
        .find(|target| {
            target.generation == generation
                && target.subscription == subscription
                && target.topic == sample.topic
        })
        .cloned();
    if identity
        && target.is_some()
        && shared
            .registry
            .sample(generation, subscription, sequence)
            .is_ok()
    {
        return target;
    }
    shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    None
}
fn publish_image(
    shared: &mut Shared,
    target: &ImageTarget,
    sequence: u64,
    backend_drops: u64,
    decoded: Result<images::TypedImage, images::ImageSampleError>,
) -> bool {
    // Stop/replacement may have happened during decoding. Neither a late frame
    // nor a late local decode error may change its replacement's state.
    let active = shared.registry.entries().iter().any(|entry| {
        entry.generation == target.generation
            && entry.subscription == target.subscription
            && entry.state == State::Active
            && entry.configuration.output == Output::CompactImage
    });
    if active {
        return target.stream.publish(sequence, backend_drops, decoded);
    }
    shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    false
}
fn ingest(
    shared: &mut Shared,
    event: Event,
    received_at: Instant,
    wire_bytes: usize,
) -> Result<(), String> {
    if let Event::Tool { generation, .. } = &event {
        if *generation != shared.registry.generation() {
            shared.ingress_rejections += 1;
            return Ok(());
        }
        if shared.tool_events.len() >= 64 {
            return Err("Tools event queue overflow; reconnect explicitly".into());
        }
        shared.tool_events.push_back(event);
        return Ok(());
    }
    if let Event::SceneFrames { generation, frames } = event {
        if generation == shared.registry.generation() {
            shared.scene_frames = Some(frames);
        } else {
            shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        }
        return Ok(());
    }
    if let Event::Graph { topics } = event {
        shared.graph = Some(topics);
        return Ok(());
    }
    let mut sample_key = None;
    let mut final_counters = None;
    let accepted = match &event {
        Event::Ready { .. } => {
            let generation = shared
                .registry
                .connect()
                .map_err(|_| "Session generation exhausted; restart app")?;
            shared
                .queue
                .reset(generation)
                .map_err(|_| "Could not initialize session queue")?;
            true
        }
        Event::PoseCleared {
            generation,
            request,
            subscription,
            barrier,
        } => pose_pipe::clear_ack(shared, *generation, *subscription, *request, barrier),
        Event::MarkersCleared {
            generation,
            request,
            subscription,
        } => marker_pipe::clear_ack(shared, *generation, *subscription, *request),
        Event::Started {
            generation,
            request,
            subscription,
        } => shared
            .registry
            .start_ack(*generation, *subscription, *request)
            .is_ok(),
        Event::Stopped {
            generation,
            request,
            subscription,
        } => {
            if shared
                .registry
                .stop_ack(*generation, *subscription, *request)
                .is_ok()
            {
                final_counters = shared
                    .queue
                    .stream_counters(*subscription)
                    .map(|c| (*subscription, c));
                let _ = shared.queue.unregister(*subscription);
                true
            } else {
                false
            }
        }
        Event::RequestError {
            generation,
            request,
            subscription,
            no_handle,
            ..
        } => {
            let clear = shared.registry.entries().iter().any(|e| {
                e.generation == *generation
                    && e.subscription == *subscription
                    && e.clear_request == Some(*request)
            });
            if clear {
                if shared.registry.entries().iter().any(|e| {
                    e.generation == *generation
                        && e.subscription == *subscription
                        && matches!(e.configuration.output, Output::PoseSnapshot(_))
                }) {
                    shared.pose_clear_send_failed(*generation, *subscription, *request);
                    true
                } else {
                    shared
                        .registry
                        .clear_markers_ack(*generation, *subscription, *request)
                        .is_ok()
                }
            } else {
                // A failed request without proof of cleanup must retain its slot.
                let correlated = shared.registry.entries().iter().any(|entry| {
                    entry.generation == *generation
                        && entry.subscription == *subscription
                        && ((!entry.start_acknowledged && entry.start_request == *request)
                            || (entry.state == State::Stopping
                                && entry.stop_request == Some(*request)))
                });
                if correlated && *no_handle {
                    if shared
                        .registry
                        .request_error(*generation, *subscription, *request)
                        .is_ok()
                    {
                        if !shared
                            .registry
                            .entries()
                            .iter()
                            .any(|entry| entry.subscription == *subscription)
                        {
                            final_counters = shared
                                .queue
                                .stream_counters(*subscription)
                                .map(|c| (*subscription, c));
                            let _ = shared.queue.unregister(*subscription);
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    correlated
                }
            }
        }
        Event::StreamError {
            generation,
            subscription,
            ..
        } => {
            if shared
                .registry
                .stream_failed(*generation, *subscription)
                .is_ok()
            {
                final_counters = shared
                    .queue
                    .stream_counters(*subscription)
                    .map(|c| (*subscription, c));
                let _ = shared.queue.unregister(*subscription);
                true
            } else {
                false
            }
        }
        Event::Observation {
            generation,
            subscription,
            sequence,
            sample,
            ..
        } => {
            let identity = shared.registry.entries().iter().any(|entry| {
                entry.generation == *generation
                    && entry.subscription == *subscription
                    && entry.configuration.topic == sample.topic
                    && entry.configuration.message_type == sample.message_type
                    && entry.configuration.output == Output::Typed
            });
            if identity
                && shared
                    .registry
                    .sample(*generation, *subscription, *sequence)
                    .is_ok()
            {
                sample_key = Some((*generation, *subscription, *sequence));
                true
            } else {
                false
            }
        }
        Event::RobotDescriptionObservation { .. } => {
            return Err("Description must use off-UI capture ingress".into())
        }
        Event::CameraSnapshotObservation { .. } => {
            return Err("Camera ingress bypassed paired owner preparation".into())
        }
        Event::PoseSnapshotObservation { .. } => {
            return Err("Pose ingress bypassed owner receiver".into())
        }
        Event::MarkerSnapshotObservation { .. } => {
            return Err("Marker bypassed its worker decoder".into())
        }
        Event::MapSnapshotObservation { .. } => {
            return Err("Map snapshot bypassed its worker decoder".into())
        }
        Event::TfGraphObservation {
            generation,
            subscription,
            sequence,
            sample,
            ..
        } => {
            let identity = shared.registry.entries().iter().any(|entry| {
                entry.generation == *generation
                    && entry.subscription == *subscription
                    && entry.configuration.output == Output::TfGraph
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|c| c.fixed_frame == sample.fixed_frame)
            });
            if identity
                && shared
                    .registry
                    .sample(*generation, *subscription, *sequence)
                    .is_ok()
            {
                sample_key = Some((*generation, *subscription, *sequence));
                true
            } else {
                false
            }
        }
        Event::RobotObservation {
            generation,
            subscription,
            sequence,
            sample,
            ..
        } => {
            let owner = shared.registry.entries().iter().find(|entry| {
                entry.generation == *generation
                    && entry.subscription == *subscription
                    && entry.configuration.output == Output::Robot
            });
            let identity = owner.is_some_and(|entry| {
                entry
                    .configuration
                    .robot
                    .as_ref()
                    .is_some_and(|c| sample.validate(c).is_ok())
            });
            let fresh = owner.is_some_and(|entry| {
                entry.state == State::Active
                    && entry.last_sequence.is_none_or(|last| *sequence > last)
            });
            if fresh
                && !identity
                && !shared
                    .robot_faults
                    .iter()
                    .any(|f| f.generation == *generation && f.subscription == *subscription)
            {
                shared.robot_faults.push(RobotIngressFault {
                    generation: *generation, subscription: *subscription,
                    message: "Robot TF batch identities contradict the active model request; enable again explicitly after cleanup".into(),
                });
            }
            if identity
                && shared
                    .registry
                    .sample(*generation, *subscription, *sequence)
                    .is_ok()
            {
                sample_key = Some((*generation, *subscription, *sequence));
                true
            } else {
                false
            }
        }
        Event::Drops {
            generation,
            subscription,
            ..
        } => shared
            .registry
            .entries()
            .iter()
            .any(|entry| entry.generation == *generation && entry.subscription == *subscription),
        Event::ImageObservation { .. } => {
            return Err("Image observation bypassed its worker decoder".into())
        }
        Event::SpatialObservation { .. } => {
            return Err("Spatial observation bypassed its worker decoder".into())
        }
        Event::PointCloudObservation { .. } => {
            return Err("Cloud observation bypassed its worker decoder".into())
        }
        Event::MappingObservation { .. } => {
            return Err("Mapping observation bypassed its worker decoder".into())
        }
        Event::Tool { .. }
        | Event::SceneFrames { .. }
        | Event::Graph { .. }
        | Event::Error { .. } => unreachable!(),
    };
    if !accepted {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return Ok(());
    }
    let entries = shared.registry.entries();
    shared.robot_faults.retain(|fault| {
        entries.iter().any(|entry| {
            entry.generation == fault.generation && entry.subscription == fault.subscription
        })
    });
    shared.cloud_faults.retain(|fault| {
        entries.iter().any(|entry| {
            entry.generation == fault.generation && entry.subscription == fault.subscription
        })
    });
    #[cfg(feature = "spatial")]
    shared.clouds.retain(|target| {
        let retained = entries.iter().any(|entry| {
            entry.generation == target.generation && entry.subscription == target.subscription
        });
        if !retained {
            target.stream.stop();
        }
        retained
    });
    shared.mapping_faults.retain(|fault| {
        entries.iter().any(|entry| {
            entry.generation == fault.generation && entry.subscription == fault.subscription
        })
    });
    #[cfg(feature = "spatial")]
    shared.mappings.retain(|target| {
        let retained = entries.iter().any(|entry| {
            entry.generation == target.generation && entry.subscription == target.subscription
        });
        if !retained {
            target.stream.stop();
        }
        retained
    });
    shared.descriptions.retain(|target| {
        let retained = entries.iter().any(|entry| {
            entry.generation == target.generation && entry.subscription == target.subscription
        });
        if !retained {
            target.stream.stop();
        }
        retained
    });
    shared.map_faults.retain(|fault| {
        entries
            .iter()
            .any(|e| e.generation == fault.generation && e.subscription == fault.subscription)
    });
    shared.markers.retain(|target| {
        let entry = entries.iter().find(|e| {
            e.generation == target.owner.generation && e.subscription == target.owner.subscription
        });
        if entry.is_none_or(|e| e.state == State::Stopping) {
            target.stream.stop();
        }
        entry.is_some()
    });
    shared.cameras.retain(|target| {
        let entry = entries.iter().find(|e| {
            e.generation == target.owner.generation && e.subscription == target.owner.subscription
        });
        if entry.is_none_or(|e| e.state == State::Stopping) {
            target.stream.stop();
        }
        entry.is_some()
    });
    shared.poses.retain(|target| {
        let entry = entries.iter().find(|e| {
            e.generation == target.owner.generation && e.subscription == target.owner.subscription
        });
        if entry.is_none_or(|e| e.state == State::Stopping) {
            target.stream.stop();
        }
        entry.is_some()
    });
    shared.maps.retain(|target| {
        let retained = entries.iter().any(|entry| {
            entry.generation == target.owner.generation
                && entry.subscription == target.owner.subscription
        });
        if !retained {
            target.stream.stop();
        }
        retained
    });
    shared.images.retain(|target| {
        entries.iter().any(|entry| {
            entry.generation == target.generation && entry.subscription == target.subscription
        })
    });
    #[cfg(feature = "spatial")]
    shared.spatials.retain(|target| {
        entries.iter().any(|entry| {
            entry.generation == target.generation && entry.subscription == target.subscription
        })
    });
    // Charge the bounded serialized typed payload, excluding only fixed envelope
    // metadata. The decoder already enforces node/depth/string bounds; no raw
    // frame is retained alongside decoded values.
    let payload_bytes = match &event {
        Event::TfGraphObservation { sample, .. } => sample.encode()?.len(),
        Event::RobotObservation { sample, .. } => sample.encode_wire()?.len(),
        Event::Observation { sample, .. } => {
            // Count canonical content without validating it a second time or
            // retaining an additional payload-sized serialization buffer.
            struct Counter(usize);
            impl std::io::Write for Counter {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    self.0 += bytes.len();
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut counter = Counter(0);
            serde_json::to_writer(&mut counter, sample).map_err(|error| error.to_string())?;
            counter.0
        }
        _ => wire_bytes,
    };
    let item = TimedEvent {
        event,
        received_at,
        final_counters,
        payload_bytes,
    };
    if let Some((g, s, sequence)) = sample_key {
        // Ordinary pressure is visible through per-stream counters; never fatal.
        let _ = shared.queue.enqueue_sample(g, s, sequence, item);
        return Ok(());
    }
    let generation = shared.registry.generation();
    // Identified errors share this FIFO with final Drops; a diagnostic lane
    // could otherwise overtake the snapshot and erase its UI subscription.
    let result = shared.queue.enqueue_control(generation, item);
    result.map_err(|(reason, _)| format!("Backend reserved event queue overflow ({reason:?}); disconnect and reconnect explicitly"))
}

#[cfg(all(test, feature = "spatial"))]
mod cloud_tests;
#[cfg(test)]
mod image_tests;
#[cfg(all(test, feature = "spatial"))]
mod mapping_tests;
#[cfg(test)]
mod robot_tests;
#[cfg(test)]
#[path = "backend_pipe/tf_graph_tests.rs"]
mod tf_graph_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use studio_protocol::{
        session,
        structured::{Sample, Value, CONTRACT_VERSION},
    };
    fn ready() -> Event {
        Event::ready("jazzy".into(), "fixture-no-ros".into())
    }
    fn owner() -> Shared {
        let mut s = Shared::new(0);
        s.ingest_for_test(ready()).unwrap();
        s
    }
    fn allocate(s: &mut Shared, view: u64) -> StartIntent {
        let i = s
            .registry
            .start_explicit(
                ViewId(view),
                Configuration {
                    map_updates: None,
                    topic: format!("/view_{view}"),
                    message_type: "std_msgs/msg/UInt64".into(),
                    best_effort: false,
                    transient_local: false,
                    output: Output::Typed,
                    spatial: None,
                    robot: None,
                },
            )
            .unwrap();
        s.queue.register(i.generation, i.subscription).unwrap();
        i
    }
    fn activate(s: &mut Shared, i: &StartIntent) {
        ingest(
            s,
            Event::Started {
                generation: i.generation,
                request: i.request,
                subscription: i.subscription,
            },
            Instant::now(),
            100,
        )
        .unwrap();
    }
    fn observation(i: &StartIntent, sequence: u64) -> Event {
        Event::Observation {
            generation: i.generation,
            subscription: i.subscription,
            sequence,
            sample: Box::new(Sample {
                contract_version: CONTRACT_VERSION,
                topic: i.configuration.topic.clone(),
                message_type: i.configuration.message_type.clone(),
                source_time: None,
                value: Value::Unsigned {
                    value: u64::MAX.to_string(),
                },
            }),
            backend_drops: 0,
        }
    }
    fn run(events: &[Event], suffix: &[u8]) -> String {
        let mut bytes = Vec::new();
        for e in events {
            bytes.extend(e.encode().unwrap());
        }
        bytes.extend(suffix);
        pump(Cursor::new(bytes), &Mutex::new(Shared::new(0)))
    }
    #[test]
    fn framing_handshake_and_terminal_detail_are_strict() {
        assert_eq!(
            run(
                &[Event::Error {
                    message: "middleware initialization failed".into()
                }],
                b""
            ),
            "Backend error: middleware initialization failed"
        );
        assert!(run(&[], b"{\"event\":\"hello\",\"version\":2}\n").contains("incompatible"));
        assert!(run(&[ready(), ready()], b"").contains("Repeated"));
        assert!(run(&[Event::Graph { topics: vec![] }], b"").contains("before handshake"));
        assert!(run(&[ready()], b"{}").contains("incomplete"));
        assert!(run(&[ready()], &vec![b'x'; MAX_LINE + 1]).contains("line limit"));
        assert_eq!(
            run(
                &[
                    ready(),
                    Event::Error {
                        message: "precise failure".into()
                    }
                ],
                b""
            ),
            "Backend error: precise failure"
        );
        let wrong = String::from_utf8(ready().encode().unwrap())
            .unwrap()
            .replace(
                &format!("\"version\":{}", session::VERSION),
                "\"version\":11",
            );
        assert!(run(&[], wrong.as_bytes()).contains("incompatible"));
    }
    #[test]
    fn flood_preserves_peer_samples_controls_diagnostics_and_graph() {
        let mut s = owner();
        let a = allocate(&mut s, 1);
        let b = allocate(&mut s, 2);
        activate(&mut s, &a);
        activate(&mut s, &b);
        for n in 1..=1000 {
            ingest(&mut s, observation(&a, n), Instant::now(), 100).unwrap();
        }
        ingest(&mut s, observation(&b, 1), Instant::now(), 100).unwrap();
        let stop = s.registry.stop_explicit(a.subscription).unwrap();
        let _ = s.queue.unregister(a.subscription);
        ingest(
            &mut s,
            Event::Stopped {
                generation: stop.generation,
                request: stop.request,
                subscription: stop.subscription,
            },
            Instant::now(),
            100,
        )
        .unwrap();
        ingest(
            &mut s,
            Event::Graph {
                topics: vec![("g".repeat(256), vec!["t".repeat(256); 16]); 10],
            },
            Instant::now(),
            50000,
        )
        .unwrap();
        assert_eq!(s.graph.as_ref().unwrap().len(), 10);
        let mut peer = false;
        let mut stopped = false;
        while let Some(p) = s.queue.pop() {
            match p {
                Packet::Sample { subscription, .. } if subscription == b.subscription => {
                    peer = true
                }
                Packet::Control(TimedEvent {
                    event: Event::Stopped { .. },
                    ..
                }) => stopped = true,
                _ => {}
            }
        }
        assert!(peer && stopped);
        assert_eq!(s.queue.counters().evicted_samples, 984);
    }
    #[test]
    fn late_unknown_wrong_identity_and_duplicate_samples_cannot_mutate_live_stream() {
        let mut s = owner();
        let i = allocate(&mut s, 1);
        ingest(&mut s, observation(&i, 1), Instant::now(), 100).unwrap();
        assert_eq!(s.queue.queued_samples(), 0);
        activate(&mut s, &i);
        let mut wrong = observation(&i, 2);
        if let Event::Observation { sample, .. } = &mut wrong {
            sample.topic = "/other".into();
        }
        ingest(&mut s, wrong, Instant::now(), 100).unwrap();
        ingest(&mut s, observation(&i, 1), Instant::now(), 100).unwrap();
        ingest(&mut s, observation(&i, 1), Instant::now(), 100).unwrap();
        assert_eq!(s.queue.queued_samples(), 1);
        assert_eq!(s.ingress_rejections, 3);
        assert_eq!(s.registry.entries()[0].last_sequence, Some(1));
    }
    #[test]
    fn uncertain_start_error_retains_slot_and_cleanup_error_only_removes_target() {
        let mut s = owner();
        let a = allocate(&mut s, 1);
        let b = allocate(&mut s, 2);
        activate(&mut s, &b);
        ingest(
            &mut s,
            Event::RequestError {
                generation: a.generation,
                request: a.request,
                subscription: a.subscription,
                message: "unknown handle".into(),
                no_handle: false,
            },
            Instant::now(),
            100,
        )
        .unwrap();
        assert_eq!(s.registry.entries().len(), 2);
        ingest(
            &mut s,
            Event::StreamError {
                generation: a.generation,
                subscription: a.subscription,
                message: "cleaned".into(),
            },
            Instant::now(),
            100,
        )
        .unwrap();
        assert_eq!(s.registry.entries().len(), 1);
        assert_eq!(s.registry.entries()[0].subscription, b.subscription);
    }
    #[test]
    fn terminal_stream_event_preserves_flood_counts_after_unregister() {
        let mut s = owner();
        let i = allocate(&mut s, 1);
        activate(&mut s, &i);
        for n in 1..=100 {
            s.ingest_for_test(observation(&i, n)).unwrap();
        }
        s.ingest_for_test(Event::StreamError {
            generation: i.generation,
            subscription: i.subscription,
            message: "reader failed after cleanup".into(),
        })
        .unwrap();
        assert!(s.queue.stream_counters(i.subscription).is_none());
        assert_eq!(s.queue.queued_samples(), 0);
        let mut found = false;
        while let Some(packet) = s.queue.pop() {
            if let Packet::Control(item) = packet {
                if !matches!(item.event, Event::StreamError { .. }) {
                    continue;
                }
                let (subscription, counters) = item.final_counters.unwrap();
                assert_eq!(subscription, i.subscription);
                assert_eq!(counters.evicted_samples, 84);
                assert_eq!(counters.rejected_samples, 0);
                found = true;
            }
        }
        assert!(found);
    }
    #[test]
    fn cancellation_late_start_then_contradictory_failure_keeps_stop_pending() {
        let mut s = owner();
        let i = allocate(&mut s, 1);
        let stop = s.registry.stop_explicit(i.subscription).unwrap();
        s.queue.unregister(i.subscription).unwrap();
        activate(&mut s, &i);
        ingest(
            &mut s,
            Event::RequestError {
                generation: i.generation,
                request: i.request,
                subscription: i.subscription,
                message: "contradictory".into(),
                no_handle: true,
            },
            Instant::now(),
            100,
        )
        .unwrap();
        ingest(&mut s, observation(&i, 1), Instant::now(), 100).unwrap();
        assert_eq!(s.registry.entries()[0].state, State::Stopping);
        assert_eq!(s.queue.queued_samples(), 0);
        ingest(
            &mut s,
            Event::Stopped {
                generation: stop.generation,
                request: stop.request,
                subscription: stop.subscription,
            },
            Instant::now(),
            100,
        )
        .unwrap();
        assert!(s.registry.entries().is_empty());
        assert_eq!(s.ingress_rejections, 2);
    }
    #[test]
    fn malformed_typed_content_is_terminal_before_ingress_and_accounting_is_exact() {
        let mut s = owner();
        let i = allocate(&mut s, 1);
        activate(&mut s, &i);
        let mut event = observation(&i, 1);
        if let Event::Observation { sample, .. } = &mut event {
            sample.value = Value::Unsigned {
                value: "not an integer".into(),
            };
        }
        let mut bytes = ready().encode().unwrap();
        bytes.extend(serde_json::to_vec(&event).unwrap());
        bytes.push(b'\n');
        assert!(pump(Cursor::new(bytes), &Mutex::new(Shared::new(0))).contains("Invalid"));
        let mut event = observation(&i, 1);
        let expected = if let Event::Observation { sample, .. } = &mut event {
            sample.value = Value::Array {
                items: vec![
                    Value::Text {
                        value: "x".repeat(32768),
                        original_bytes: 32768
                    };
                    15
                ],
                original_items: 15,
            };
            sample.encode().unwrap().len()
        } else {
            unreachable!()
        };
        let wire = event.encode().unwrap();
        let decoded = Event::decode(&wire).unwrap();
        ingest(&mut s, decoded, Instant::now(), wire.len()).unwrap();
        assert!(expected > 480 * 1024);
        assert_eq!(s.queue.sample_bytes(), expected);
    }
    #[test]
    fn reconnect_owners_are_isolated_and_reserved_overflow_is_visible() {
        let mut old = owner();
        let a = allocate(&mut old, 1);
        activate(&mut old, &a);
        let mut new = Shared::new(1);
        ingest(&mut new, ready(), Instant::now(), 200).unwrap();
        let b = allocate(&mut new, 1);
        activate(&mut new, &a);
        assert_eq!(new.registry.entries()[0].state, State::Starting);
        activate(&mut new, &b);
        assert_eq!(new.registry.entries()[0].state, State::Active);
        for _ in 0..RESERVED_ITEMS {
            let _ = new.queue.enqueue_control(
                Generation(2),
                TimedEvent {
                    event: ready(),
                    received_at: Instant::now(),
                    final_counters: None,
                    payload_bytes: 200,
                },
            );
        }
        let stop = new.registry.stop_explicit(b.subscription).unwrap();
        assert!(ingest(
            &mut new,
            Event::Stopped {
                generation: stop.generation,
                request: stop.request,
                subscription: stop.subscription
            },
            Instant::now(),
            100
        )
        .unwrap_err()
        .contains("overflow"));
        assert_eq!(old.registry.entries()[0].state, State::Active);
        assert_eq!(session::VERSION, 21);
    }
    #[test]
    fn drop_snapshots_never_activate_or_change_sequence_and_reject_stale_unknown_owners() {
        let mut s = owner();
        let i = allocate(&mut s, 1);
        s.ingest_for_test(Event::Drops {
            generation: i.generation,
            subscription: i.subscription,
            backend_drops: 2,
            spatial_counters: None,
        })
        .unwrap();
        assert_eq!(s.registry.entries()[0].state, State::Starting);
        assert_eq!(s.registry.entries()[0].last_sequence, None);
        activate(&mut s, &i);
        s.ingest_for_test(observation(&i, 3)).unwrap();
        s.ingest_for_test(Event::Drops {
            generation: i.generation,
            subscription: i.subscription,
            backend_drops: 4,
            spatial_counters: None,
        })
        .unwrap();
        assert_eq!(s.registry.entries()[0].last_sequence, Some(3));
        s.registry.stop_explicit(i.subscription).unwrap();
        s.queue.unregister(i.subscription).unwrap();
        s.ingest_for_test(Event::Drops {
            generation: i.generation,
            subscription: i.subscription,
            backend_drops: 9,
            spatial_counters: None,
        })
        .unwrap();
        assert_eq!(s.registry.entries()[0].state, State::Stopping);
        for (generation, subscription) in [
            (Generation(99), i.subscription),
            (i.generation, SubscriptionId(999)),
        ] {
            s.ingest_for_test(Event::Drops {
                generation,
                subscription,
                backend_drops: 100,
                spatial_counters: None,
            })
            .unwrap();
        }
        assert_eq!(s.ingress_rejections, 2);
    }
    #[test]
    fn final_backend_snapshot_shares_fifo_with_scoped_error_despite_sensor_flood() {
        let mut s = owner();
        let a = allocate(&mut s, 1);
        let b = allocate(&mut s, 2);
        activate(&mut s, &a);
        activate(&mut s, &b);
        for n in 1..=100 {
            s.ingest_for_test(observation(&a, n)).unwrap();
        }
        s.ingest_for_test(observation(&b, 1)).unwrap();
        s.ingest_for_test(Event::Drops {
            generation: a.generation,
            subscription: a.subscription,
            backend_drops: 84,
            spatial_counters: None,
        })
        .unwrap();
        s.ingest_for_test(Event::StreamError {
            generation: a.generation,
            subscription: a.subscription,
            message: "cleanup complete".into(),
        })
        .unwrap();
        let mut saw_drops = false;
        let mut saw_error = false;
        let mut saw_peer = false;
        while let Some(packet) = s.queue.pop() {
            match packet {
                Packet::Control(TimedEvent {
                    event:
                        Event::Drops {
                            backend_drops: 84, ..
                        },
                    ..
                }) => saw_drops = true,
                Packet::Control(TimedEvent {
                    event: Event::StreamError { .. },
                    ..
                }) => {
                    assert!(saw_drops);
                    saw_error = true;
                }
                Packet::Sample { subscription, .. } if subscription == b.subscription => {
                    saw_peer = true
                }
                _ => {}
            }
        }
        assert!(saw_drops && saw_error && saw_peer);
        assert_eq!(s.queue.counters().evicted_samples, 84);
    }
}
