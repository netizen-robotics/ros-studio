use eframe::egui;
mod backend_pipe;
mod fonts;
mod robot_setup;
mod session;
mod shell;
mod state;
mod ui;
mod workspace_app;
use state::{Connection, Discovery, Phase, ViewState, ViewStatus};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use studio_desktop::{images, plots, spatial};
use studio_protocol::{
    session::{Command, Event},
    subscriptions::*,
};

// The retained desktop writer/renderer keep their historical eight-view scope.
const MAX_SUBSCRIPTIONS: usize = 8;

#[derive(Default)]
enum ViewKind {
    #[default]
    Messages,
    Plot(plots::FieldPath),
    Image,
    Spatial,
    PointCloud,
    Mapping(Output),
    Robot,
}
impl ViewKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Messages => "Messages",
            Self::Plot(_) => "Numeric plot",
            Self::Image => "Image",
            Self::Spatial => "Spatial scan",
            Self::PointCloud => "Point cloud",
            Self::Mapping(Output::OccupancyGrid) => "Map",
            Self::Mapping(Output::Pose) => "Pose",
            Self::Mapping(Output::TfGraph) => "TF frames",
            Self::Mapping(_) => "Path",
            Self::Robot => "Robot model",
        }
    }
}
fn kind_for_output(output: Output) -> ViewKind {
    match output {
        Output::CameraSnapshot(_) => {
            unreachable!("Camera UI is not registered at this integration step")
        }
        Output::PoseSnapshot(_) => {
            unreachable!("Retained desktop has no covariance/Odometry display")
        }
        Output::MarkerSnapshot => unreachable!("Retained desktop has no Marker display"),
        Output::MapSnapshot => unreachable!("Retained desktop uses the legacy map path"),
        Output::RobotDescription => {
            unreachable!("Temporary description captures are not display configurations")
        }
        Output::Typed => ViewKind::Messages,
        Output::CompactImage => ViewKind::Image,
        Output::SpatialScan => ViewKind::Spatial,
        Output::Robot => ViewKind::Robot,
        Output::PointCloud => ViewKind::PointCloud,
        Output::TfGraph | Output::OccupancyGrid | Output::Pose | Output::Path => {
            ViewKind::Mapping(output)
        }
    }
}
fn empty_spatial_configuration() -> studio_protocol::spatial::Configuration {
    studio_protocol::spatial::Configuration {
        fixed_frame: String::new(),
        clock_policy: studio_protocol::spatial_input::SpatialClockPolicy::Unknown,
        dynamic_tf_topic: "/tf".into(),
        dynamic_tf_best_effort: false,
        static_tf_topic: "/tf_static".into(),
    }
}
struct App {
    robot_setup: robot_setup::RobotSetup,
    spatial_renderer: Option<spatial::Renderer3d>,
    shared_scene: spatial::scene_view::SharedSceneView,
    workspace: workspace_app::WorkspaceUi,
    connection: Connection,
    discovery: Discovery,
    views: Vec<ViewState>,
    selected: Option<ViewId>,
    next_view: u64,
    session: Option<session::Session>,
    session_generation: u64,
    connection_deadline: Option<Instant>,
    diagnostics: VecDeque<String>,
    sidebar: bool,
    inspector: bool,
    setup: bool,
    setup_target: Option<ViewId>,
    draft_best_effort: bool,
    draft_transient_local: bool,
    draft_output: Output,
    draft_spatial: studio_protocol::spatial::Configuration,
    draft_clock_verified: bool,
    settings: bool,
    about: bool,
    diagnostics_open: bool,
    repaint: Option<egui::Context>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            robot_setup: robot_setup::RobotSetup::default(),
            spatial_renderer: None,
            shared_scene: Default::default(),
            workspace: workspace_app::WorkspaceUi::default(),
            connection: Connection::default(),
            discovery: Discovery::default(),
            views: Vec::new(),
            selected: None,
            next_view: 0,
            session: None,
            session_generation: 0,
            connection_deadline: None,
            diagnostics: VecDeque::new(),
            sidebar: true,
            inspector: true,
            setup: false,
            setup_target: None,
            draft_best_effort: false,
            draft_transient_local: false,
            draft_output: Output::Typed,
            draft_spatial: empty_spatial_configuration(),
            draft_clock_verified: false,
            settings: false,
            about: false,
            diagnostics_open: false,
            repaint: None,
        }
    }
}
impl App {
    fn diagnostic(&mut self, message: String) {
        if self.diagnostics.back() != Some(&message) {
            if self.diagnostics.len() == 32 {
                self.diagnostics.pop_front();
            }
            self.diagnostics.push_back(message);
        }
    }
    fn stop(&mut self) {
        self.robot_setup.disconnected();
        if let Some(renderer) = &mut self.spatial_renderer {
            renderer.release_frame();
        }
        self.workspace_disconnected();
        if let Some(session) = self.session.take() {
            {
                let mut shared = session.shared.lock().unwrap();
                self.session_generation =
                    self.session_generation.max(shared.registry.generation().0);
                shared.registry.disconnect();
            }
            drop(session); // Worker owns process I/O and cleanup.
        }
        self.connection.phase = Phase::Disconnected;
        self.connection.status = "Disconnected".into();
        self.connection.actual = None;
        self.connection_deadline = None;
        self.discovery.topics.clear();
        self.discovery.received_graph = false;
        self.draft_clock_verified = false;
        self.draft_spatial.clock_policy =
            studio_protocol::spatial_input::SpatialClockPolicy::Unknown;
        for view in &mut self.views {
            view.deactivate();
            view.spatial_clock_verified = false;
        }
    }
    fn fail(&mut self, message: String) {
        self.stop();
        self.connection.phase = Phase::Failed;
        self.connection.status = message.clone();
        self.diagnostic(message);
    }
    fn connect(&mut self) {
        let domain = match self.connection.domain_id() {
            Ok(id) => id,
            Err(e) => {
                self.fail(e);
                return;
            }
        };
        self.connection.domain = domain.to_string();
        self.stop();
        self.connection.phase = Phase::Starting;
        self.connection.status = "Starting ROS backend; discovering without subscriptions…".into();
        self.connection_deadline = Some(Instant::now() + Duration::from_secs(15));
        self.session = Some(session::launch(
            studio_client_core::session::inherited_config(domain, self.connection.cyclone),
            self.session_generation,
        ));
        if let Some(session) = &self.session {
            session.shared.lock().unwrap().repaint = self.repaint.clone();
        }
    }
    fn begin_add(&mut self) {
        if self.workspace_frozen() {
            return;
        }
        self.cancel_robot_setup();
        self.setup = true;
        self.setup_target = None;
        self.draft_best_effort = false;
        self.draft_transient_local = false;
        self.draft_output = Output::Typed;
        self.draft_spatial = self
            .views
            .iter()
            .filter(|view| view.enabled)
            .find_map(|view| {
                view.configuration
                    .scene_key()
                    .map(|key| studio_protocol::spatial::Configuration {
                        fixed_frame: key.fixed_frame,
                        dynamic_tf_topic: key.dynamic_tf_topic,
                        dynamic_tf_best_effort: key.dynamic_tf_best_effort,
                        static_tf_topic: key.static_tf_topic,
                        clock_policy: studio_protocol::spatial_input::SpatialClockPolicy::Unknown,
                    })
            })
            .unwrap_or_else(empty_spatial_configuration);
        self.draft_clock_verified = false;
        self.inspector = true;
    }
    fn configure_view(&mut self, id: ViewId) {
        if self.workspace_frozen() {
            return;
        }
        if self
            .views
            .iter()
            .any(|view| view.id == id && matches!(view.kind, ViewKind::Robot))
        {
            self.begin_robot_setup(Some(id));
            return;
        }
        self.cancel_robot_setup();
        let Some(view) = self.views.iter().find(|v| v.id == id) else {
            return;
        };
        self.discovery.select(&view.configuration.topic);
        if self.discovery.topics.iter().any(|(topic, types)| {
            topic == &view.configuration.topic && types.contains(&view.configuration.message_type)
        }) {
            self.discovery.ty = view.configuration.message_type.clone();
        }
        self.draft_best_effort = view.configuration.best_effort;
        self.draft_transient_local = view.configuration.transient_local;
        self.draft_output = view.configuration.output.clone();
        self.draft_spatial = view
            .configuration
            .spatial
            .clone()
            .unwrap_or_else(empty_spatial_configuration);
        self.draft_clock_verified = view.spatial_clock_verified;
        self.selected = Some(id);
        self.setup_target = Some(id);
        self.setup = true;
        self.inspector = true;
    }
    fn subscribe(&mut self) {
        if self.workspace_frozen() {
            return;
        }
        if self.connection.phase != Phase::Connected || !self.discovery.valid_selection() {
            return;
        }
        let configuration = Configuration {
            map_updates: None,
            robot: None,
            topic: self.discovery.topic.clone(),
            message_type: self.discovery.ty.clone(),
            best_effort: self.draft_best_effort,
            transient_local: self.draft_transient_local,
            output: self.draft_output.clone(),
            spatial: matches!(
                self.draft_output,
                Output::SpatialScan
                    | Output::PointCloud
                    | Output::OccupancyGrid
                    | Output::Pose
                    | Output::Path
            )
            .then(|| {
                let mut configuration = self.draft_spatial.clone();
                configuration.clock_policy = if self.draft_clock_verified {
                    studio_protocol::spatial_input::SpatialClockPolicy::VerifiedSystem
                } else {
                    studio_protocol::spatial_input::SpatialClockPolicy::Unknown
                };
                configuration
            }),
        };
        if !configuration.valid() {
            self.diagnostic("Topic/type exceeds supported subscription bounds".into());
            return;
        }
        if self.setup_target.is_none() && self.views.len() >= MAX_SUBSCRIPTIONS {
            self.diagnostic(
                "Eight displays are configured. Stop and remove a view before adding another."
                    .into(),
            );
            return;
        }
        if self
            .setup_target
            .and_then(|id| self.views.iter().find(|v| v.id == id))
            .is_some_and(|v| matches!(v.status, ViewStatus::Stopping | ViewStatus::NeedsRecovery))
        {
            return;
        }
        if let Err(error) = self.admission(&configuration, self.setup_target) {
            self.diagnostic(error);
            return;
        }
        if let Some(id) = self.setup_target {
            let Some(index) = self.views.iter().position(|v| v.id == id) else {
                return;
            };
            self.views[index].spatial_clock_verified = self.draft_clock_verified;
            if self.views[index].subscription.is_some() {
                self.views[index].pending = Some(configuration);
                self.stop_view(id);
            } else {
                if self.views[index].configuration.output != configuration.output {
                    self.views[index].kind = kind_for_output(configuration.output.clone());
                }
                self.views[index].configuration = configuration;
                self.start_view(id);
            }
        } else {
            if self.views.len() >= MAX_SUBSCRIPTIONS {
                self.diagnostic(
                    "Eight displays are configured. Stop and remove a view before adding another."
                        .into(),
                );
                return;
            }
            let Some(next) = self.next_view.checked_add(1) else {
                self.fail("View identities exhausted".into());
                return;
            };
            self.next_view = next;
            let id = ViewId(next);
            self.views.push(ViewState::new(id, configuration));
            self.workspace_adopt_new_view(id);
            self.views.last_mut().unwrap().spatial_clock_verified = self.draft_clock_verified;
            self.selected = Some(id);
            self.start_view(id);
        }
        self.setup = false;
    }
    /// The only normal reception/visibility action. Stopping keeps native ownership
    /// reserved until ingress validates cleanup, independently of enabled intent.
    fn set_display_enabled(&mut self, id: ViewId, enabled: bool) {
        if self.workspace_frozen() {
            return;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        if matches!(
            self.views[index].status,
            ViewStatus::Stopping | ViewStatus::NeedsRecovery
        ) {
            return;
        }
        if enabled {
            self.start_view(id);
            if !self.views[index].enabled {
                self.configure_view(id);
            }
        } else {
            self.views[index].enabled = false;
            self.views[index].pending = None;
            self.stop_view(id);
        }
    }
    /// Include Starting/Stopping owners until authoritative cleanup. Replacement
    /// may credit only its own owner; Start rechecks after cleanup.
    fn admission(
        &self,
        candidate: &Configuration,
        replacing: Option<ViewId>,
    ) -> Result<ResourceUsage, String> {
        let session = self
            .session
            .as_ref()
            .ok_or("Connect before enabling a display")?;
        let shared = session.shared.lock().unwrap();
        let mut configurations: Vec<_> = shared
            .registry
            .entries()
            .iter()
            .filter(|entry| Some(entry.view) != replacing)
            .map(|entry| &entry.configuration)
            .collect();
        configurations.push(candidate);
        // Retained desktop has a single legacy-map renderer. Multi-map display
        // support belongs to the independent Visualizer until desktop cutover.
        if configurations
            .iter()
            .filter(|c| matches!(c.output, Output::OccupancyGrid | Output::MapSnapshot))
            .count()
            > 1
        {
            return Err(
                "The retained desktop supports one active Map; use Visualizer for multiple maps"
                    .into(),
            );
        }
        let usage = resource_usage(&configurations).map_err(str::to_owned)?;
        if usage.native_handles > 8 {
            return Err(
                "Shared native reader capacity exceeded (retained desktop:8 readers)".into(),
            );
        }
        Ok(usage)
    }
    fn start_view(&mut self, id: ViewId) {
        if self.workspace_frozen() {
            return;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        if self.views[index].subscription.is_some() {
            return;
        }
        if self.connection.phase != Phase::Connected || !self.discovery.received_graph {
            return;
        }
        let configuration = self.views[index].configuration.clone();
        if configuration.output == Output::Robot {
            let valid_model = self.views[index]
                .robot
                .as_ref()
                .zip(configuration.robot.as_ref())
                .is_some_and(|(live, c)| c.frames == live.loaded().prepared.link_names);
            if !valid_model || !self.views[index].spatial_clock_verified {
                self.views[index].error = Some(
                    "Load and review the model, then verify System time before enabling Robot."
                        .into(),
                );
                return;
            }
            let robot = configuration.robot.as_ref().unwrap();
            if [&robot.dynamic_tf_topic, &robot.static_tf_topic]
                .iter()
                .any(|name| {
                    !self.discovery.topics.iter().any(|(topic, types)| {
                        topic == *name && types.iter().any(|ty| ty == "tf2_msgs/msg/TFMessage")
                    })
                })
            {
                self.views[index].error = Some("Required Robot TF topic/type is unavailable. Configure both discovered TF inputs.".into());
                return;
            }
        }
        if matches!(
            configuration.output,
            Output::SpatialScan
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        ) {
            if !self.views[index].spatial_clock_verified {
                self.views[index].error = Some(
                    "Verify these publishers use system time before enabling this display.".into(),
                );
                return;
            }
            let Some(spatial) = &configuration.spatial else {
                return;
            };
            if !configuration.output.is_mapping()
                && [&spatial.dynamic_tf_topic, &spatial.static_tf_topic]
                    .iter()
                    .any(|name| {
                        !self.discovery.topics.iter().any(|(topic, types)| {
                            topic == *name && types.iter().any(|ty| ty == "tf2_msgs/msg/TFMessage")
                        })
                    })
            {
                self.views[index].error = Some(
                    "Required TF topic/type is unavailable. Configure both discovered TF inputs."
                        .into(),
                );
                return;
            }
        }
        if !self.discovery.topics.iter().any(|(topic, types)| {
            topic == &configuration.topic && types.contains(&configuration.message_type)
        }) {
            self.views[index].error =
                Some("Topic/type is unavailable. Configure this view from discovered data.".into());
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        if let Err(message) = self.admission(&configuration, None) {
            self.views[index].error = Some(message.clone());
            self.diagnostic(message);
            return;
        }
        let mut queue_failed = false;
        let intent = {
            let mut shared = session.shared.lock().unwrap();
            shared
                .registry
                .start_explicit(id, configuration)
                .and_then(|start| {
                    shared
                        .queue
                        .register(start.generation, start.subscription)
                        .map_err(|_| {
                            queue_failed = true;
                            RegistryError::Capacity
                        })?;
                    Ok(start)
                })
        };
        let start = match intent {
            Ok(start) => start,
            Err(e) => {
                if queue_failed {
                    self.fail(
                        "Subscription queue state is inconsistent; reconnect explicitly".into(),
                    );
                } else {
                    self.diagnostic(format!("Could not start view: {e:?}"));
                }
                return;
            }
        };
        let command = Command::Start {
            generation: start.generation,
            request: start.request,
            subscription: start.subscription,
            configuration: start.configuration,
        };
        let view = &mut self.views[index];
        if matches!(
            view.configuration.output,
            Output::SpatialScan
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        ) && view.spatial_clock_verified
        {
            // Persist configuration intent, separately from session-local verification.
            view.workspace_clock_expectation = studio_desktop::workspace::ClockExpectation::System;
        }
        view.enabled = true;
        view.subscription = Some(start.subscription);
        view.generation = start.generation;
        view.request = Some(start.request);
        view.status = ViewStatus::Starting;
        view.deadline = Some(Instant::now() + Duration::from_secs(15));
        view.error = None;
        view.queue_drops = 0;
        view.received.backend_drops = 0;
        view.clear_live();
        view.origin = Instant::now();
        if view.configuration.output == Output::Robot {
            let result = view
                .robot
                .as_mut()
                .unwrap()
                .start(*view.configuration.robot.clone().unwrap());
            match result {
                Ok(token) => {
                    view.robot_token = Some(token);
                    view.robot_status = "Starting · Latest available TF".into();
                }
                Err(error) => {
                    self.fail(format!("Could not initialize Robot presentation: {error}"));
                    return;
                }
            }
        }
        if view.configuration.output == Output::SpatialScan {
            let configuration = view.configuration.spatial.as_ref().unwrap();
            let mut scene = spatial::Scene::default();
            let prepared = scene
                .set_fixed_frame(&configuration.fixed_frame)
                .and_then(|_| scene.add_layer(1))
                .and_then(|_| scene.start())
                .and_then(|_| scene.layer_token(1));
            let token = match prepared {
                Ok(token) => token,
                Err(error) => {
                    self.fail(format!("Could not initialize Spatial scene: {error:?}"));
                    return;
                }
            };
            scene.set_visible(1, true);
            view.spatial_scene = scene;
            view.spatial_token = Some(token);
            view.spatial_generation = view.spatial_stream.start();
            view.spatial_counters = Default::default();
            view.spatial_status = "Waiting for scan and acquisition-time transforms…".into();
            view.spatial_details.clear();
            let binding =
                session
                    .shared
                    .lock()
                    .unwrap()
                    .bind_spatial(backend_pipe::SpatialTarget {
                        generation: start.generation,
                        subscription: start.subscription,
                        topic: view.configuration.topic.clone(),
                        fixed_frame: configuration.fixed_frame.clone(),
                        local_generation: view.spatial_generation,
                        stream: view.spatial_stream.clone(),
                    });
            if let Err(error) = binding {
                self.fail(error);
                return;
            }
        }
        if view.configuration.output == Output::PointCloud {
            view.cloud_generation = view.cloud_stream.start();
            view.spatial_status = "Waiting for cloud and acquisition-time transforms…".into();
            let binding = session
                .shared
                .lock()
                .unwrap()
                .bind_cloud(backend_pipe::CloudTarget {
                    generation: start.generation,
                    subscription: start.subscription,
                    topic: view.configuration.topic.clone(),
                    fixed_frame: view
                        .configuration
                        .spatial
                        .as_ref()
                        .unwrap()
                        .fixed_frame
                        .clone(),
                    local_generation: view.cloud_generation,
                    stream: view.cloud_stream.clone(),
                });
            if let Err(error) = binding {
                self.fail(error);
                return;
            }
        }
        if view.configuration.output.is_mapping() {
            view.mapping_generation = view.mapping_stream.start();
            view.spatial_status = "Waiting for mapping and acquisition-time transforms…".into();
            let binding =
                session
                    .shared
                    .lock()
                    .unwrap()
                    .bind_mapping(backend_pipe::MappingTarget {
                        generation: start.generation,
                        subscription: start.subscription,
                        topic: view.configuration.topic.clone(),
                        message_type: view.configuration.message_type.clone(),
                        fixed_frame: view
                            .configuration
                            .spatial
                            .as_ref()
                            .unwrap()
                            .fixed_frame
                            .clone(),
                        local_generation: view.mapping_generation,
                        stream: view.mapping_stream.clone(),
                    });
            if let Err(error) = binding {
                self.fail(error);
                return;
            }
        }
        if view.configuration.output == Output::CompactImage {
            let local_generation = view.image.start();
            let stream = images::ImageStream::new(view.image.frames(), local_generation);
            view.image_stream = Some(stream.clone());
            let binding = session
                .shared
                .lock()
                .unwrap()
                .bind_image(backend_pipe::ImageTarget {
                    generation: start.generation,
                    subscription: start.subscription,
                    topic: view.configuration.topic.clone(),
                    stream,
                });
            if let Err(error) = binding {
                self.fail(error);
                return;
            }
        }
        // Bind the image mailbox before Start can reach the backend. No worker
        // can publish a retained first frame into an unbound/replaced view.
        if let Err(e) = session.send(command) {
            self.fail(e);
        }
    }
    fn stop_view(&mut self, id: ViewId) {
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        self.views[index].enabled = false;
        let Some(subscription) = self.views[index].subscription else {
            return;
        };
        // UI toggles are disabled while stopping. Explicit recovery retries can
        // replace correlation; workspace apply must adopt the in-flight Stop.
        if self.workspace_frozen() && self.views[index].status == ViewStatus::Stopping {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let intent = {
            let mut shared = session.shared.lock().unwrap();
            let stop = shared.registry.stop_explicit(subscription);
            if let Some(c) = shared.queue.stream_counters(subscription) {
                self.views[index].queue_drops =
                    c.evicted_samples.saturating_add(c.rejected_samples);
            }
            let _ = shared.queue.unregister(subscription);
            shared
                .images
                .retain(|target| target.subscription != subscription);
            shared
                .spatials
                .retain(|target| target.subscription != subscription);
            stop
        };
        let stop = match intent {
            Ok(stop) => stop,
            Err(e) => {
                self.workspace_stop_failed(
                    self.views[index].generation,
                    subscription,
                    &format!("Stop request could not be issued: {e:?}"),
                );
                self.diagnostic(format!(
                    "Stop state could not be confirmed: {e:?}; disconnect to recover"
                ));
                return;
            }
        };
        if let Err(e) = session.send(Command::Stop {
            generation: stop.generation,
            request: stop.request,
            subscription,
        }) {
            self.fail(e);
            return;
        }
        let view = &mut self.views[index];
        view.status = ViewStatus::Stopping;
        view.request = Some(stop.request);
        view.deadline = Some(Instant::now() + Duration::from_secs(10));
        view.error = None;
        view.clear_live();
    }
    fn robot_failed(&mut self, id: ViewId, message: String) {
        if let Some(view) = self.views.iter_mut().find(|view| view.id == id) {
            view.enabled = false;
            view.pending = None;
            view.clear_live();
        }
        self.stop_view(id);
        if let Some(view) = self.views.iter_mut().find(|view| view.id == id) {
            view.error = Some(message.clone());
        }
        self.diagnostic(message);
    }
    fn remove_view(&mut self, id: ViewId) {
        if self.workspace_frozen() {
            return;
        }
        self.views
            .retain(|v| v.id != id || v.subscription.is_some());
        if self.selected == Some(id) {
            self.selected = self.views.first().map(|v| v.id);
        }
    }
    fn open_plot(&mut self, id: ViewId) {
        if self.workspace_frozen() {
            return;
        }
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else {
            return;
        };
        if !view.status.active() {
            return;
        }
        let (Some(path), Some(sample)) = (view.plot_draft.clone(), view.received.typed.as_ref())
        else {
            return;
        };
        if plots::numeric_value(&sample.value, &path)
            .and_then(|v| v.coordinate())
            .is_err()
        {
            return;
        }
        view.kind = ViewKind::Plot(path);
        view.trace.begin(view.subscription.map_or(0, |s| s.0));
        view.origin = Instant::now();
    }
    /// Absence is cleanup evidence only within the live transport generation;
    /// ingress removes an owner only after its validated terminal event.
    fn workspace_cleanup_is_confirmed(
        &self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> bool {
        self.session.as_ref().is_some_and(|session| {
            let shared = session.shared.lock().unwrap();
            shared.registry.generation() == generation
                && !shared.registry.entries().iter().any(|entry| {
                    entry.generation == generation && entry.subscription == subscription
                })
        })
    }
    fn handle_at(&mut self, event: Event, received_at: Instant) {
        match event {
            Event::Ready { distro, rmw, .. } => {
                if self.connection.phase != Phase::Starting {
                    return;
                }
                self.connection.phase = Phase::Connected;
                self.connection.status = format!("Connected: {distro} / {rmw}");
                self.connection.actual = Some((distro.into_string(), rmw.into_string()));
                self.connection_deadline = None;
                if let Some(session) = &self.session {
                    self.session_generation =
                        session.shared.lock().unwrap().registry.generation().0;
                }
            }
            Event::Graph { mut topics } if self.connection.phase == Phase::Connected => {
                topics.sort_by(|a, b| a.0.cmp(&b.0));
                self.discovery.topics = topics;
                self.discovery.received_graph = true;
            }
            Event::Started {
                generation,
                request,
                subscription,
            } => {
                if let Some(view) = self
                    .views
                    .iter_mut()
                    .find(|v| v.generation == generation && v.subscription == Some(subscription))
                {
                    // A late start acknowledgement can never undo Stop.
                    if view.status == ViewStatus::Starting && view.request == Some(request) {
                        view.status = ViewStatus::Waiting;
                        view.deadline = None;
                    }
                }
            }
            Event::Stopped {
                generation,
                request,
                subscription,
            } => {
                if !self.workspace_cleanup_is_confirmed(generation, subscription) {
                    return;
                }
                let mut replacement = None;
                let mut matched = false;
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.request == Some(request)
                }) {
                    matched = true;
                    let pending = view.pending.take();
                    view.deactivate();
                    if let Some(configuration) = pending {
                        if view.configuration.output != configuration.output {
                            view.kind = kind_for_output(configuration.output.clone());
                        }
                        view.configuration = configuration;
                        replacement = Some(view.id);
                    }
                }
                if matched {
                    self.workspace_cleanup_confirmed(generation, subscription);
                }
                if let Some(id) = replacement {
                    self.start_view(id);
                }
            }
            Event::Observation {
                generation,
                subscription,
                sample,
                backend_drops,
                ..
            } => {
                let active = self.session.as_ref().is_some_and(|session| {
                    session
                        .shared
                        .lock()
                        .unwrap()
                        .registry
                        .entries()
                        .iter()
                        .any(|e| {
                            e.generation == generation
                                && e.subscription == subscription
                                && e.state == State::Active
                        })
                });
                if !active || self.connection.phase != Phase::Connected {
                    return;
                }
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && (v.status.active() || v.status == ViewStatus::Starting)
                }) {
                    // Ingress has already verified Started. Fair dequeue may
                    // present a retained sample before that control's UI label.
                    view.deadline = None;
                    if sample.topic != view.configuration.topic
                        || sample.message_type != view.configuration.message_type
                    {
                        return;
                    }
                    if let ViewKind::Plot(path) = &view.kind {
                        if let Some(receipt) = received_at.checked_duration_since(view.origin) {
                            view.trace.push(
                                subscription.0,
                                plots::Sample {
                                    receipt,
                                    value: plots::numeric_value(&sample.value, path),
                                },
                            );
                        }
                    } else {
                        view.numeric_choices = plots::numeric_fields(&sample.value);
                    }
                    view.status = ViewStatus::Receiving;
                    view.received.backend_drops = view.received.backend_drops.max(backend_drops);
                    view.received.typed_samples = view.received.typed_samples.saturating_add(1);
                    view.received.typed = Some(sample);
                }
            }
            Event::RobotObservation {
                generation,
                subscription,
                sequence,
                sample,
                backend_drops,
            } => {
                let mut failure = None;
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.configuration.output == Output::Robot
                        && v.status.active()
                }) {
                    let result = view
                        .robot
                        .as_mut()
                        .zip(view.robot_token)
                        .ok_or_else(|| "Robot data has no loaded model activation".to_owned())
                        .and_then(|(live, token)| {
                            live.apply(token, sequence, &sample)
                                .map_err(|e| e.to_string())
                        });
                    match result {
                        Ok(()) => {
                            let ready = sample
                                .links
                                .iter()
                                .filter(|l| {
                                    matches!(l.pose, studio_protocol::robot::LinkPose::Ready { .. })
                                })
                                .count();
                            view.robot_status = format!(
                                "{ready}/{} links ready · Latest available TF",
                                sample.links.len()
                            );
                            view.status = if ready > 0 {
                                ViewStatus::Receiving
                            } else {
                                ViewStatus::Waiting
                            };
                            view.received.typed_samples =
                                view.received.typed_samples.saturating_add(1);
                            view.received.backend_drops =
                                view.received.backend_drops.max(backend_drops);
                        }
                        Err(error) => failure = Some((view.id, error)),
                    }
                }
                if let Some((id, error)) = failure {
                    self.robot_failed(id, error);
                }
            }
            Event::Drops {
                generation,
                subscription,
                backend_drops,
                spatial_counters,
            } => {
                if let Some(view) = self.views.iter_mut().find(|view| {
                    view.generation == generation && view.subscription == Some(subscription)
                }) {
                    // A counter snapshot is metadata, never activation or data.
                    view.received.backend_drops = view.received.backend_drops.max(backend_drops);
                    if let Some(counters) = spatial_counters {
                        view.absorb_spatial_counters(counters);
                    }
                }
            }
            Event::RequestError {
                generation,
                request,
                subscription,
                message,
                no_handle,
            } => {
                let no_entry = self.workspace_cleanup_is_confirmed(generation, subscription);
                let mut replacement = None;
                let mut cleanup = false;
                let mut failed_stop = false;
                if let Some(view) = self
                    .views
                    .iter_mut()
                    .find(|v| v.generation == generation && v.subscription == Some(subscription))
                {
                    let current_request = view.request == Some(request);
                    if no_handle && no_entry {
                        cleanup = true;
                        let pending = view.pending.take();
                        view.deactivate();
                        if let Some(configuration) = pending {
                            if view.configuration.output != configuration.output {
                                view.kind = kind_for_output(configuration.output.clone());
                            }
                            view.configuration = configuration;
                            replacement = Some(view.id);
                        }
                    }
                    // A stop error must retain uncertain cleanup ownership.
                    if current_request
                        && matches!(
                            view.status,
                            ViewStatus::Stopping | ViewStatus::NeedsRecovery
                        )
                    {
                        failed_stop = true;
                        view.status = ViewStatus::NeedsRecovery;
                        view.deadline = None;
                    }
                    if current_request || (no_handle && no_entry) {
                        view.error = Some(message.clone());
                    }
                }
                if cleanup {
                    self.workspace_cleanup_confirmed(generation, subscription);
                } else if failed_stop {
                    self.workspace_stop_failed(generation, subscription, &message);
                }
                self.diagnostic(format!("Subscription {}: {message}", subscription.0));
                if let Some(id) = replacement {
                    self.start_view(id);
                }
            }
            Event::StreamError {
                generation,
                subscription,
                message,
            } => {
                if !self.workspace_cleanup_is_confirmed(generation, subscription) {
                    return;
                }
                let mut matched = false;
                if let Some(view) = self
                    .views
                    .iter_mut()
                    .find(|v| v.generation == generation && v.subscription == Some(subscription))
                {
                    matched = true;
                    view.deactivate();
                    view.error = Some(message.clone());
                }
                if matched {
                    self.workspace_cleanup_confirmed(generation, subscription);
                }
                self.diagnostic(format!("Subscription {}: {message}", subscription.0));
            }
            Event::Error { message } => self.fail(message),
            _ => (),
        }
    }
    fn poll_backend(&mut self) {
        if let Some(Err(error)) = self.session.as_ref().and_then(|s| s.ready.try_recv().ok()) {
            self.fail(error);
            return;
        }
        if let Some(error) = self
            .session
            .as_ref()
            .and_then(|s| s.terminal.try_recv().ok())
        {
            self.fail(error);
            return;
        }
        // Drain a bounded amount of fair work per frame, without holding the
        // ingress mutex while rendering or processing configuration changes.
        for _ in 0..64 {
            let packet = self
                .session
                .as_ref()
                .and_then(|s| s.shared.lock().unwrap().queue.pop());
            let timed = match packet {
                Some(
                    Packet::Control(e) | Packet::Diagnostic(e) | Packet::Sample { bytes: e, .. },
                ) => e,
                None => break,
            };
            if let Some((subscription, counters)) = timed.final_counters {
                if let Some(view) = self
                    .views
                    .iter_mut()
                    .find(|v| v.subscription == Some(subscription))
                {
                    view.queue_drops = view.queue_drops.max(
                        counters
                            .evicted_samples
                            .saturating_add(counters.rejected_samples),
                    );
                }
            }
            self.handle_at(timed.event, timed.received_at);
        }
        let graph = self
            .session
            .as_ref()
            .and_then(|s| s.shared.lock().unwrap().graph.take());
        if let Some(topics) = graph {
            self.handle_at(Event::Graph { topics }, Instant::now());
        }
        let robot_faults = self
            .session
            .as_ref()
            .map(|s| std::mem::take(&mut s.shared.lock().unwrap().robot_faults))
            .unwrap_or_default();
        for fault in robot_faults {
            if let Some(id) = self
                .views
                .iter()
                .find(|view| {
                    view.generation == fault.generation
                        && view.subscription == Some(fault.subscription)
                        && view.status.active()
                })
                .map(|view| view.id)
            {
                self.robot_failed(id, fault.message);
            }
        }
        let mut image_errors = Vec::new();
        if let Some(session) = &self.session {
            let shared = session.shared.lock().unwrap();
            for view in &mut self.views {
                if let Some(c) = view
                    .subscription
                    .and_then(|s| shared.queue.stream_counters(s))
                {
                    view.queue_drops = c.evicted_samples.saturating_add(c.rejected_samples);
                }
                let active = shared.registry.entries().iter().any(|entry| {
                    entry.generation == view.generation
                        && Some(entry.subscription) == view.subscription
                        && entry.state == State::Active
                        && entry.configuration.output == Output::CompactImage
                });
                if !active
                    || !matches!(
                        view.status,
                        ViewStatus::Starting | ViewStatus::Waiting | ViewStatus::Receiving
                    )
                {
                    continue;
                }
                if let Some(update) = view
                    .image_stream
                    .as_ref()
                    .and_then(images::ImageStream::snapshot)
                {
                    if update.sequence <= view.image_sequence {
                        continue;
                    }
                    view.image_sequence = update.sequence;
                    view.received.typed_samples = update.received_frames;
                    view.received.backend_drops =
                        view.received.backend_drops.max(update.backend_drops);
                    view.deadline = None;
                    if let Some(error) = update.error {
                        image_errors.push((view.id, error));
                    } else {
                        view.status = ViewStatus::Receiving;
                    }
                }
            }
        }
        // Scene projection consumes one bounded decoded slot after releasing Shared.
        // Native TF lookup and parsing have already happened on their workers.
        let mut spatial_errors = Vec::new();
        for view in &mut self.views {
            if !matches!(view.kind, ViewKind::Spatial)
                || !matches!(
                    view.status,
                    ViewStatus::Starting | ViewStatus::Waiting | ViewStatus::Receiving
                )
            {
                continue;
            }
            let active = self.session.as_ref().is_some_and(|session| {
                session
                    .shared
                    .lock()
                    .unwrap()
                    .registry
                    .entries()
                    .iter()
                    .any(|entry| {
                        entry.generation == view.generation
                            && Some(entry.subscription) == view.subscription
                            && entry.state == State::Active
                    })
            });
            if !active {
                continue;
            }
            if let Some(update) = view.spatial_stream.snapshot() {
                if update.sequence > view.spatial_sequence {
                    view.spatial_sequence = update.sequence;
                    view.received.typed_samples = update.received_updates;
                    view.received.backend_drops =
                        view.received.backend_drops.max(update.backend_drops);
                    view.deadline = None;
                    if let Some(error) = update.error {
                        spatial_errors.push((view.id, error));
                        continue;
                    }
                }
            }
            if let Some((frame, update)) = view
                .spatial_stream
                .take_with_status(view.spatial_generation)
            {
                if let Some(token) = view.spatial_token {
                    let result = frame.apply(&mut view.spatial_scene, token);
                    view.absorb_spatial_counters(update.source_counters);
                    view.spatial_status = format!("TF: {}", frame.tf_status().label());
                    view.spatial_details = format!(
                        "Acquisition: {} ns · Superseded before projection: {}",
                        frame.acquisition_stamp_ns, update.superseded
                    );
                    if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                        if let Err(error) = result {
                            spatial_errors
                                .push((view.id, format!("Spatial projection failed: {error:?}")));
                        } else {
                            view.status = ViewStatus::Receiving;
                        }
                    } else {
                        view.status = ViewStatus::Waiting;
                    }
                }
            }
        }
        let cloud_faults = self
            .session
            .as_ref()
            .map(|s| std::mem::take(&mut s.shared.lock().unwrap().cloud_faults))
            .unwrap_or_default();
        for fault in cloud_faults {
            if let Some(view) = self.views.iter().find(|view| {
                view.generation == fault.generation
                    && view.subscription == Some(fault.subscription)
                    && view.status.active()
            }) {
                spatial_errors.push((view.id, fault.message));
            }
        }
        for view in &mut self.views {
            if !matches!(view.kind, ViewKind::PointCloud)
                || !(view.status.active() || view.status == ViewStatus::Starting)
            {
                continue;
            }
            let active = self.session.as_ref().is_some_and(|s| {
                s.shared.lock().unwrap().registry.entries().iter().any(|e| {
                    e.generation == view.generation
                        && Some(e.subscription) == view.subscription
                        && e.state == State::Active
                })
            });
            if !active {
                continue;
            }
            if let Some(update) = view.cloud_stream.snapshot() {
                if update.sequence > view.spatial_sequence {
                    view.spatial_sequence = update.sequence;
                    view.received.typed_samples = update.received_updates;
                    view.received.backend_drops =
                        view.received.backend_drops.max(update.backend_drops);
                    view.deadline = None;
                    if let Some(error) = update.error {
                        spatial_errors.push((view.id, error));
                        continue;
                    }
                }
            }
            if let Some((frame, update)) = view.cloud_stream.take_with_status(view.cloud_generation)
            {
                view.status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                    ViewStatus::Receiving
                } else {
                    ViewStatus::Waiting
                };
                view.spatial_status = format!("TF: {}", frame.tf_status().label());
                view.spatial_details = format!(
                    "{} source points · {} skipped · Acquisition: {} ns · {} superseded",
                    frame.source_points,
                    frame.skipped_points,
                    frame.acquisition_stamp_ns,
                    update.superseded
                );
                view.cloud_frame = Some(frame);
            }
        }
        let mapping_faults = self
            .session
            .as_ref()
            .map(|s| std::mem::take(&mut s.shared.lock().unwrap().mapping_faults))
            .unwrap_or_default();
        for fault in mapping_faults {
            if let Some(view) = self.views.iter().find(|view| {
                view.generation == fault.generation
                    && view.subscription == Some(fault.subscription)
                    && view.status.active()
            }) {
                spatial_errors.push((view.id, fault.message));
            }
        }
        for view in &mut self.views {
            if !matches!(view.kind, ViewKind::Mapping(_))
                || !(view.status.active() || view.status == ViewStatus::Starting)
            {
                continue;
            }
            let active = self.session.as_ref().is_some_and(|s| {
                s.shared.lock().unwrap().registry.entries().iter().any(|e| {
                    e.generation == view.generation
                        && Some(e.subscription) == view.subscription
                        && e.state == State::Active
                })
            });
            if !active {
                continue;
            }
            if let Some(update) = view.mapping_stream.snapshot() {
                if update.sequence > view.spatial_sequence {
                    view.spatial_sequence = update.sequence;
                    view.received.typed_samples = update.received_updates;
                    view.received.backend_drops =
                        view.received.backend_drops.max(update.backend_drops);
                    view.deadline = None;
                    if let Some(error) = update.error {
                        spatial_errors.push((view.id, error));
                        continue;
                    }
                }
            }
            if let Some((frame, update)) = view
                .mapping_stream
                .take_with_status(view.mapping_generation)
            {
                view.status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                    ViewStatus::Receiving
                } else {
                    ViewStatus::Waiting
                };
                view.spatial_status = format!("TF: {}", frame.tf_status().label());
                view.spatial_details = format!(
                    "Acquisition: {} ns · {} superseded",
                    frame.acquisition_stamp_ns, update.superseded
                );
                view.mapping_frame = Some(frame);
            }
        }
        // A cloud's latched ingress fault and mailbox snapshot describe the
        // same failure. Issue one correlated Stop, preserving its request ID.
        spatial_errors.sort_by_key(|(id, _)| id.0);
        spatial_errors.dedup_by_key(|(id, _)| id.0);
        for (id, error) in spatial_errors {
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.error = Some(error.clone());
            }
            self.diagnostic(format!("Spatial view {}: {error}", id.0));
        }
        for (id, error) in image_errors {
            // Decoder failure cannot prove native cleanup. Use the ordinary
            // correlated Stop and keep its slot until the backend acknowledges.
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|view| view.id == id) {
                view.image.fail(error.clone());
                view.error = Some(error.clone());
            }
            self.diagnostic(format!("Image view {}: {error}", id.0));
        }
        self.check_deadlines(Instant::now());
    }
    fn check_deadlines(&mut self, now: Instant) {
        if self
            .connection_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.fail("Backend startup timed out; reconnect explicitly".into());
            return;
        }
        let mut cancel = Vec::new();
        let mut workspace_failures = Vec::new();
        let mut errors = Vec::new();
        for view in &mut self.views {
            if view.deadline.is_none_or(|deadline| now < deadline) {
                continue;
            }
            view.deadline = None;
            view.pending = None;
            match view.status {
                ViewStatus::Starting => {
                    cancel.push(view.id);
                    errors.push(format!(
                        "View {} start timed out; cancelling its handle",
                        view.id.0
                    ));
                }
                ViewStatus::Stopping => {
                    if let Some(subscription) = view.subscription {
                        workspace_failures.push((view.generation, subscription));
                    }
                    view.status = ViewStatus::NeedsRecovery;
                    view.error = Some(
                        "Stop timed out; cleanup remains unconfirmed. Retry stop or disconnect."
                            .into(),
                    );
                    errors.push(format!("View {} cleanup is unconfirmed", view.id.0));
                }
                _ => (),
            }
        }
        for (generation, subscription) in workspace_failures {
            self.workspace_stop_failed(
                generation,
                subscription,
                "Stop timed out; cleanup remains unconfirmed. Retry stop or disconnect.",
            );
        }
        for message in errors {
            self.diagnostic(message);
        }
        for id in cancel {
            self.stop_view(id);
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.repaint = Some(ctx.clone());
        self.poll_backend();
        self.robot_poll();
        self.workspace_poll(ctx);
        if !self.views.iter().any(|view| {
            view.enabled
                && matches!(
                    view.kind,
                    ViewKind::Spatial
                        | ViewKind::PointCloud
                        | ViewKind::Mapping(_)
                        | ViewKind::Robot
                )
        }) {
            if let Some(renderer) = &mut self.spatial_renderer {
                renderer.release_frame();
            }
        }
        // Release before constructing this frame's egui meshes. A UI action
        // can disable a scene after it was drawn; its texture stays valid until
        // the next frame rather than disappearing before egui submits it.
        self.render(ctx);
        let robot_errors: Vec<_> = self
            .views
            .iter_mut()
            .filter_map(|view| {
                view.robot_render_failure
                    .take()
                    .map(|error| (view.id, error))
            })
            .collect();
        for (id, error) in robot_errors {
            self.robot_failed(id, error);
        }
    }

    fn on_exit(&mut self) {
        // macOS Cmd-Q can bypass run_native's return and normal App drop.
        // Cancel now and let the worker reap its backend before exit proceeds.
        self.stop();
        self.workspace_shutdown();
        if let Err(error) = self.robot_setup.finish_shutdown() {
            eprintln!("Robot asset worker shutdown failed: {error}");
            self.diagnostic(error);
        }
        session::finish_shutdown();
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
fn bundled_distro(manifest: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).map_err(|e| format!("Invalid runtime manifest: {e}"))?;
    match value["distro"].as_str() {
        Some(distro @ ("humble" | "jazzy" | "lyrical")) => Ok(distro.into()),
        _ => Err("Runtime manifest must declare a supported ROS distro".into()),
    }
}
fn main() -> eframe::Result {
    let result = eframe::run_native(
        "ROS Studio",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 820.0])
                .with_icon(studio_ui_foundation::app_icons::AppIcon::Launcher.window_icon())
                .with_min_inner_size([760.0, 560.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            fonts::configure(&cc.egui_ctx);
            shell::configure(&cc.egui_ctx);
            let mut app = App::default();
            app.spatial_renderer = cc.wgpu_render_state.clone().map(spatial::Renderer3d::new);
            Ok(Box::new(app))
        }),
    );
    session::finish_shutdown();
    result
}

#[cfg(test)]
mod tests;
