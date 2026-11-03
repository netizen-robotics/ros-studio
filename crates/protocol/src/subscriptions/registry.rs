use super::{Generation, RequestId, SubscriptionId, ViewId};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    #[default]
    Typed,
    CompactImage,
    SpatialScan,
    Robot,
    TfGraph,
    RobotDescription,
    PointCloud,
    OccupancyGrid,
    MapSnapshot,
    MarkerSnapshot,
    PoseSnapshot(crate::pose_snapshot::transport::Configuration),
    CameraSnapshot(Box<crate::camera_snapshot::InfoSource>),
    Pose,
    Path,
}

impl Output {
    pub fn mapping_message_type(&self) -> Option<&'static str> {
        match self {
            Self::OccupancyGrid => Some(crate::mapping::MAP_MESSAGE_TYPE),
            Self::Pose => Some(crate::mapping::POSE_MESSAGE_TYPE),
            Self::Path => Some(crate::mapping::PATH_MESSAGE_TYPE),
            _ => None,
        }
    }
    pub fn is_mapping(&self) -> bool {
        self.mapping_message_type().is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub topic: String,
    pub message_type: String,
    pub best_effort: bool,
    pub transient_local: bool,
    /// Explicit on the wire: Image topics inspected as Messages remain Typed.
    pub output: Output,
    pub spatial: Option<crate::spatial::Configuration>,
    pub robot: Option<Box<crate::robot::Configuration>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_updates: Option<crate::map_grid::UpdateSource>,
}
impl Configuration {
    pub fn valid(&self) -> bool {
        fn token(s: &str) -> bool {
            !s.is_empty()
                && s.bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }
        let spatial_valid = match (&self.output, &self.spatial) {
            (
                Output::TfGraph
                | Output::SpatialScan
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::MapSnapshot
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::CameraSnapshot(_)
                | Output::Pose
                | Output::Path,
                Some(s),
            ) => {
                (if let Output::CameraSnapshot(info) = &self.output {
                    info.validate().is_ok()
                        && info.topic != self.topic
                        && self.message_type == crate::image::MESSAGE_TYPE
                } else if let Output::PoseSnapshot(configuration) = self.output {
                    configuration.settings().is_ok()
                        && self.message_type == configuration.kind.message_type()
                } else if self.output == Output::MarkerSnapshot {
                    matches!(
                        self.message_type.as_str(),
                        crate::marker::MESSAGE_TYPE | crate::marker::ARRAY_MESSAGE_TYPE
                    )
                } else {
                    self.message_type
                        == if self.output == Output::TfGraph {
                            crate::spatial_input::TF_MESSAGE_TYPE
                        } else if self.output == Output::SpatialScan {
                            crate::spatial_input::SCAN_MESSAGE_TYPE
                        } else if self.output == Output::PointCloud {
                            crate::cloud::MESSAGE_TYPE
                        } else if self.output == Output::MapSnapshot {
                            crate::mapping::MAP_MESSAGE_TYPE
                        } else {
                            self.output.mapping_message_type().unwrap()
                        }
                }) && (self.output != Output::TfGraph
                    || (self.topic == s.dynamic_tf_topic
                        && self.best_effort == s.dynamic_tf_best_effort
                        && !self.transient_local))
                    && crate::spatial::valid_frame(&s.fixed_frame)
                    && s.clock_policy == crate::spatial_input::SpatialClockPolicy::VerifiedSystem
                    && valid_topic(&s.dynamic_tf_topic)
                    && valid_topic(&s.static_tf_topic)
                    && s.dynamic_tf_topic != s.static_tf_topic
            }
            (
                Output::TfGraph
                | Output::SpatialScan
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::MapSnapshot
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::CameraSnapshot(_)
                | Output::Pose
                | Output::Path,
                None,
            ) => false,
            (_, Some(_)) => false,
            (_, None) => true,
        };
        let robot_valid = match (&self.output, &self.robot) {
            (Output::Robot, Some(r)) => {
                r.validate().is_ok()
                    && self.topic == r.dynamic_tf_topic
                    && self.message_type == crate::spatial_input::TF_MESSAGE_TYPE
                    && self.best_effort == r.dynamic_tf_best_effort
                    && !self.transient_local
            }
            (Output::Robot, None) => false,
            (_, Some(_)) => false,
            (_, None) => true,
        };
        if (self.output == Output::RobotDescription
            && self.message_type != crate::robot_description::MESSAGE_TYPE)
            || !spatial_valid
            || !robot_valid
            || self.map_updates.as_ref().is_some_and(|updates| {
                self.output != Output::MapSnapshot
                    || !updates.valid()
                    || updates.topic == self.topic
            })
            || self.topic.len() > 256
            || self.message_type.len() > 256
            || (self.output == Output::CompactImage
                && self.message_type != crate::image::MESSAGE_TYPE)
        {
            return false;
        }
        let ty: Vec<_> = self.message_type.split('/').collect();
        self.topic.len() <= 256
            && self.topic.starts_with('/')
            && self.topic[1..].split('/').all(token)
            && self.message_type.len() <= 256
            && ty.len() == 3
            && ty[1] == "msg"
            && ty.iter().all(|s| token(s))
    }
    pub fn resource_cost(&self) -> usize {
        match self.output {
            // Shared TF readers are reserved once by resource_usage, not per owner.
            Output::Robot | Output::TfGraph => 0,
            Output::MapSnapshot => 1 + usize::from(self.map_updates.is_some()),
            Output::CameraSnapshot(_) => 2,
            _ => 1,
        }
    }
    pub fn owns_tf(&self) -> bool {
        matches!(
            self.output,
            Output::TfGraph
                | Output::SpatialScan
                | Output::Robot
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::MapSnapshot
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::CameraSnapshot(_)
                | Output::Pose
                | Output::Path
        )
    }
    pub fn scene_key(&self) -> Option<SceneKey> {
        if self.output == Output::Robot {
            let r = self.robot.as_ref()?;
            Some(SceneKey {
                fixed_frame: r.fixed_frame.clone(),
                dynamic_tf_topic: r.dynamic_tf_topic.clone(),
                dynamic_tf_best_effort: r.dynamic_tf_best_effort,
                static_tf_topic: r.static_tf_topic.clone(),
            })
        } else if matches!(
            self.output,
            Output::TfGraph
                | Output::SpatialScan
                | Output::PointCloud
                | Output::MapSnapshot
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::CameraSnapshot(_)
        ) || self.output.is_mapping()
        {
            let s = self.spatial.as_ref()?;
            Some(SceneKey {
                fixed_frame: s.fixed_frame.clone(),
                dynamic_tf_topic: s.dynamic_tf_topic.clone(),
                dynamic_tf_best_effort: s.dynamic_tf_best_effort,
                static_tf_topic: s.static_tf_topic.clone(),
            })
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneKey {
    pub fixed_frame: String,
    pub dynamic_tf_topic: String,
    pub dynamic_tf_best_effort: bool,
    pub static_tf_topic: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceUsage {
    pub logical: usize,
    pub native_handles: usize,
    pub robots: usize,
    pub tf_graphs: usize,
    pub descriptions: usize,
    pub sensor_layers: usize,
    pub maps: usize,
    pub poses: usize,
    pub paths: usize,
    pub markers: usize,
    pub covariance_poses: usize,
    pub odometries: usize,
    pub cameras: usize,
}
/// Reserve all Starting/Active/Stopping owners; callers must retain their
/// configuration until authoritative cleanup. Compatible owners share TF once.
pub fn resource_usage(configurations: &[&Configuration]) -> Result<ResourceUsage, &'static str> {
    if configurations.len() > super::MAX_SUBSCRIPTIONS {
        return Err("Logical subscription capacity exceeded");
    }
    let mut usage = ResourceUsage::default();
    let mut scene = None;
    for configuration in configurations {
        if !configuration.valid() {
            return Err("Invalid observation configuration");
        }
        usage.descriptions += usize::from(configuration.output == Output::RobotDescription);
        usage.logical += 1;
        usage.native_handles += configuration.resource_cost();
        usage.tf_graphs += usize::from(configuration.output == Output::TfGraph);
        usage.robots += usize::from(configuration.output == Output::Robot);
        usage.sensor_layers += usize::from(matches!(
            configuration.output,
            Output::SpatialScan | Output::PointCloud
        ));
        usage.maps += usize::from(matches!(
            configuration.output,
            Output::OccupancyGrid | Output::MapSnapshot
        ));
        usage.poses += usize::from(configuration.output == Output::Pose);
        usage.paths += usize::from(configuration.output == Output::Path);
        usage.markers += usize::from(configuration.output == Output::MarkerSnapshot);
        usage.cameras += usize::from(matches!(configuration.output, Output::CameraSnapshot(_)));
        if let Output::PoseSnapshot(pose) = configuration.output {
            usage.covariance_poses +=
                usize::from(pose.kind == crate::pose_snapshot::Kind::PoseWithCovariance);
            usage.odometries += usize::from(pose.kind == crate::pose_snapshot::Kind::Odometry);
        }
        if let Some(key) = configuration.scene_key() {
            if let Some(current) = &scene {
                if current != &key {
                    return Err("Scene frame and TF inputs must match active scene displays; stop them before changing shared sources");
                }
            } else {
                scene = Some(key);
                usage.native_handles += 2;
            }
        }
    }
    if usage.maps > crate::mapping::MAX_MAP_LAYERS {
        return Err(
            "Map display limit reached (3 active maps); stop a map before enabling another",
        );
    }
    if usage.native_handles > super::MAX_NATIVE_HANDLES {
        return Err("Shared native reader capacity exceeded (64 readers, including TF and map updates); stop a display or disable optional map updates");
    }
    if usage.logical > super::MAX_SUBSCRIPTIONS
        || usage.native_handles > super::MAX_NATIVE_HANDLES
        || usage.tf_graphs > crate::tf_graph::MAX_OWNERS
        || usage.descriptions > crate::robot_description::MAX_OWNERS
        || usage.robots > 1
        || usage.sensor_layers > super::MAX_SENSOR_LAYERS
        || usage.poses > crate::mapping::MAX_POSE_LAYERS
        || usage.paths > crate::mapping::MAX_PATH_LAYERS
        || usage.markers > crate::marker_snapshot::MAX_OWNERS
        || usage.covariance_poses > 1
        || usage.odometries > 1
        || usage.cameras > crate::camera_snapshot::MAX_OWNERS
    {
        return Err("Scene layer, logical subscription or shared native reader capacity exceeded");
    }
    Ok(usage)
}
pub(crate) fn valid_topic(topic: &str) -> bool {
    topic.len() <= 256
        && topic.starts_with('/')
        && topic[1..].split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Active,
    Stopping,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub view: ViewId,
    pub configuration: Configuration,
    pub state: State,
    pub start_request: RequestId,
    /// Records an observed successful start even after entering Stopping.
    pub start_acknowledged: bool,
    pub stop_request: Option<RequestId>,
    pub clear_request: Option<RequestId>,
    pub last_sequence: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartIntent {
    pub generation: Generation,
    pub request: RequestId,
    pub subscription: SubscriptionId,
    pub view: ViewId,
    pub configuration: Configuration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopIntent {
    pub generation: Generation,
    pub request: RequestId,
    pub subscription: SubscriptionId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Disconnected,
    StaleGeneration,
    InvalidConfiguration,
    InvalidId,
    Capacity,
    ViewAlreadyPresent,
    UnknownSubscription,
    WrongRequest,
    WrongState,
    OldSequence,
    ExhaustedIds,
}
/// Runtime entries only. Inactive/draft configurations remain with the workspace owner.
/// No method automatically emits a start intent on connection or reconnection.
#[derive(Debug, Default)]
pub struct Registry {
    generation: u64,
    next_request: u64,
    next_subscription: u64,
    connected: bool,
    entries: Vec<Entry>,
}
impl Registry {
    /// Seed a new transport owner without sharing mutable state with an old reader.
    pub fn with_prior_generation(generation: u64) -> Self {
        Self {
            generation,
            ..Self::default()
        }
    }
    /// Call after a verified new backend session. Clears prior runtime state.
    pub fn connect(&mut self) -> Result<Generation, RegistryError> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or(RegistryError::ExhaustedIds)?;
        self.entries.clear();
        self.generation = next;
        self.connected = true;
        Ok(Generation(next))
    }
    /// Backend failure/disconnect invalidates runtime entries immediately; never replay.
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.entries.clear();
    }
    pub fn generation(&self) -> Generation {
        Generation(self.generation)
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    /// Starting and stopping entries retain their physical native resources.
    pub fn used_resources(&self) -> usize {
        resource_usage(
            &self
                .entries
                .iter()
                .map(|e| &e.configuration)
                .collect::<Vec<_>>(),
        )
        .map_or(super::MAX_NATIVE_HANDLES, |usage| usage.native_handles)
    }
    pub fn available_resources(&self) -> usize {
        super::MAX_NATIVE_HANDLES.saturating_sub(self.used_resources())
    }
    fn check_generation(&self, generation: Generation) -> Result<(), RegistryError> {
        if !self.connected {
            return Err(RegistryError::Disconnected);
        }
        if generation != self.generation() {
            return Err(RegistryError::StaleGeneration);
        }
        Ok(())
    }
    pub fn start_explicit(
        &mut self,
        view: ViewId,
        configuration: Configuration,
    ) -> Result<StartIntent, RegistryError> {
        self.check_generation(self.generation())?;
        if view.0 == 0 {
            return Err(RegistryError::InvalidId);
        }
        if !configuration.valid() {
            return Err(RegistryError::InvalidConfiguration);
        }
        if self.entries.iter().any(|e| e.view == view) {
            return Err(RegistryError::ViewAlreadyPresent);
        }
        let mut configurations: Vec<_> = self.entries.iter().map(|e| &e.configuration).collect();
        configurations.push(&configuration);
        if resource_usage(&configurations).is_err() {
            return Err(RegistryError::Capacity);
        }
        let request = self
            .next_request
            .checked_add(1)
            .ok_or(RegistryError::ExhaustedIds)?;
        let subscription = self
            .next_subscription
            .checked_add(1)
            .ok_or(RegistryError::ExhaustedIds)?;
        self.next_request = request;
        self.next_subscription = subscription;
        let intent = StartIntent {
            generation: self.generation(),
            request: RequestId(request),
            subscription: SubscriptionId(subscription),
            view,
            configuration,
        };
        self.entries.push(Entry {
            generation: intent.generation,
            subscription: intent.subscription,
            view,
            configuration: intent.configuration.clone(),
            state: State::Starting,
            start_request: intent.request,
            start_acknowledged: false,
            stop_request: None,
            clear_request: None,
            last_sequence: None,
        });
        Ok(intent)
    }
    fn entry_mut(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<&mut Entry, RegistryError> {
        self.check_generation(generation)?;
        if subscription.0 == 0 {
            return Err(RegistryError::InvalidId);
        }
        self.entries
            .iter_mut()
            .find(|e| e.subscription == subscription)
            .ok_or(RegistryError::UnknownSubscription)
    }
    pub fn start_ack(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<(), RegistryError> {
        let entry = self.entry_mut(generation, subscription)?;
        if entry.start_request != request {
            return Err(RegistryError::WrongRequest);
        }
        if entry.start_acknowledged {
            return Err(RegistryError::WrongState);
        }
        // A delayed successful start still proves a handle existed. Remember it
        // during cancellation, but never undo the user's stop intent.
        if entry.state == State::Starting {
            entry.state = State::Active;
        }
        entry.start_acknowledged = true;
        Ok(())
    }
    /// Only consume a backend failure that guarantees native-handle cleanup.
    pub fn stream_failed(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<ViewId, RegistryError> {
        let view = self.entry_mut(generation, subscription)?.view;
        self.entries
            .retain(|entry| entry.subscription != subscription);
        Ok(view)
    }
    /// Cancellation can be sent while Starting. Repeated explicit stop retries allocate
    /// a fresh request; an old stop ack cannot close the replacement request.
    pub fn stop_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<StopIntent, RegistryError> {
        let generation = self.generation();
        self.entry_mut(generation, subscription)?;
        let next = self
            .next_request
            .checked_add(1)
            .ok_or(RegistryError::ExhaustedIds)?;
        self.next_request = next;
        let entry = self.entry_mut(generation, subscription)?;
        entry.state = State::Stopping;
        entry.stop_request = Some(RequestId(next));
        entry.clear_request = None;
        Ok(StopIntent {
            generation,
            request: RequestId(next),
            subscription,
        })
    }
    pub fn clear_markers_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<StopIntent, RegistryError> {
        let generation = self.generation();
        if self
            .entry_mut(generation, subscription)?
            .configuration
            .output
            != Output::MarkerSnapshot
        {
            return Err(RegistryError::WrongState);
        }
        self.clear_explicit(subscription)
    }
    pub fn clear_pose_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<StopIntent, RegistryError> {
        let generation = self.generation();
        if !matches!(
            self.entry_mut(generation, subscription)?
                .configuration
                .output,
            Output::PoseSnapshot(_)
        ) {
            return Err(RegistryError::WrongState);
        }
        self.clear_explicit(subscription)
    }
    pub fn clear_markers_ack(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<(), RegistryError> {
        if self
            .entry_mut(generation, subscription)?
            .configuration
            .output
            != Output::MarkerSnapshot
        {
            return Err(RegistryError::WrongState);
        }
        self.clear_ack(generation, subscription, request)
    }
    pub fn clear_pose_ack(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<(), RegistryError> {
        if !matches!(
            self.entry_mut(generation, subscription)?
                .configuration
                .output,
            Output::PoseSnapshot(_)
        ) {
            return Err(RegistryError::WrongState);
        }
        self.clear_ack(generation, subscription, request)
    }
    fn clear_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<StopIntent, RegistryError> {
        let generation = self.generation();
        let entry = self.entry_mut(generation, subscription)?;
        if entry.state != State::Active
            || !matches!(
                entry.configuration.output,
                Output::MarkerSnapshot | Output::PoseSnapshot(_)
            )
            || entry.clear_request.is_some()
        {
            return Err(RegistryError::WrongState);
        }
        let next = self
            .next_request
            .checked_add(1)
            .ok_or(RegistryError::ExhaustedIds)?;
        self.next_request = next;
        self.entry_mut(generation, subscription)?.clear_request = Some(RequestId(next));
        Ok(StopIntent {
            generation,
            request: RequestId(next),
            subscription,
        })
    }
    /// Clear success/error never releases the subscription or native resources.
    fn clear_ack(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<(), RegistryError> {
        let entry = self.entry_mut(generation, subscription)?;
        if entry.state != State::Active
            || !matches!(
                entry.configuration.output,
                Output::MarkerSnapshot | Output::PoseSnapshot(_)
            )
            || entry.clear_request != Some(request)
        {
            return Err(RegistryError::WrongRequest);
        }
        entry.clear_request = None;
        Ok(())
    }
    pub fn stop_ack(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<ViewId, RegistryError> {
        let entry = self.entry_mut(generation, subscription)?;
        if entry.state != State::Stopping {
            return Err(RegistryError::WrongState);
        }
        if entry.stop_request != Some(request) {
            return Err(RegistryError::WrongRequest);
        }
        let view = entry.view;
        self.entries.retain(|e| e.subscription != subscription);
        Ok(view)
    }
    /// Validate before delivering a payload. Data can never activate an entry.
    pub fn sample(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
    ) -> Result<ViewId, RegistryError> {
        let entry = self.entry_mut(generation, subscription)?;
        if entry.state != State::Active {
            return Err(RegistryError::WrongState);
        }
        if entry.last_sequence.is_some_and(|last| sequence <= last) {
            return Err(RegistryError::OldSequence);
        }
        entry.last_sequence = Some(sequence);
        Ok(entry.view)
    }
    /// A matching failed start releases the slot, assuming backend confirms no handle
    /// was created, including cancellation before its start acknowledgement.
    /// An original start error after observed success is contradictory and rejected.
    /// A stop error stays Stopping and requires explicit retry/disconnect.
    /// Error text and request presentation belong to the bounded diagnostics owner.
    pub fn request_error(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) -> Result<ViewId, RegistryError> {
        let entry = self.entry_mut(generation, subscription)?;
        let view = entry.view;
        match entry.state {
            State::Starting | State::Stopping
                if !entry.start_acknowledged && entry.start_request == request =>
            {
                self.entries.retain(|e| e.subscription != subscription)
            }
            State::Stopping if entry.stop_request == Some(request) => (),
            _ => return Err(RegistryError::WrongRequest),
        }
        Ok(view)
    }
}
