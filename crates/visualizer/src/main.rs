mod camera_display;
mod components;
mod diagnostics;
mod displays;
mod map_display;
mod marker_display;
mod panels;
#[cfg(test)]
mod performance_support;
mod presentation_app;
mod sensor_display;
mod tf_display;
mod topic_workflow;
mod uncertainty_display;
// This opt-in load fixture uses UnixStream; Windows load qualification is separate.
#[cfg(all(test, unix))]
mod uncertainty_performance_tests;
use eframe::egui;
mod backend_pipe;
mod fonts;
mod help;
mod icons;
mod robot_setup;
mod scene_settings;
mod session;
mod shell;
mod state;
mod ui;
mod workspace_app;
use crate::components::{images, plots, spatial};
use state::{Connection, Discovery, Phase, ViewState, ViewStatus};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use studio_protocol::{
    session::{Command, Event},
    subscriptions::*,
};

#[derive(Default)]
enum ViewKind {
    #[default]
    Messages,
    Plot(plots::FieldPath),
    Image,
    Camera,
    Spatial,
    PointCloud,
    Mapping(Output),
    TfGraph,
    Markers,
    Uncertainty(studio_protocol::pose_snapshot::Kind),
    Robot,
}
impl ViewKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Messages => "Messages",
            Self::Plot(_) => "Numeric plot",
            Self::Image => "Image",
            Self::Camera => "Calibrated Camera",
            Self::Spatial => "Spatial scan",
            Self::PointCloud => "Point cloud",
            Self::Mapping(Output::OccupancyGrid | Output::MapSnapshot) => "Map",
            Self::Mapping(Output::Pose) => "Pose",
            Self::TfGraph => "TF frames",
            Self::Markers => "Markers",
            Self::Uncertainty(studio_protocol::pose_snapshot::Kind::PoseWithCovariance) => {
                "Pose covariance"
            }
            Self::Uncertainty(studio_protocol::pose_snapshot::Kind::Odometry) => "Odometry",
            Self::Mapping(_) => "Path",
            Self::Robot => "Robot model",
        }
    }
}
fn kind_for_output(output: Output) -> ViewKind {
    match output {
        Output::CameraSnapshot(_) => ViewKind::Camera,
        Output::PoseSnapshot(configuration) => ViewKind::Uncertainty(configuration.kind),
        Output::MarkerSnapshot => ViewKind::Markers,
        Output::MapSnapshot => ViewKind::Mapping(output),
        Output::RobotDescription => {
            unreachable!("Temporary description captures are not display configurations")
        }
        Output::Typed => ViewKind::Messages,
        Output::CompactImage => ViewKind::Image,
        Output::SpatialScan => ViewKind::Spatial,
        Output::Robot => ViewKind::Robot,
        Output::PointCloud => ViewKind::PointCloud,
        Output::TfGraph => ViewKind::TfGraph,
        Output::OccupancyGrid | Output::Pose | Output::Path => ViewKind::Mapping(output),
    }
}
fn empty_spatial_configuration() -> studio_protocol::spatial::Configuration {
    studio_protocol::spatial::Configuration {
        fixed_frame: "map".into(),
        clock_policy: studio_protocol::spatial_input::SpatialClockPolicy::VerifiedSystem,
        dynamic_tf_topic: "/tf".into(),
        dynamic_tf_best_effort: false,
        static_tf_topic: "/tf_static".into(),
    }
}
#[cfg(test)]
mod frame_trace;
struct App {
    network: studio_client_core::network_ui::NetworkSettings,
    #[cfg(test)]
    frame_trace: [[f64; 2]; 7],
    robot_setup: robot_setup::RobotSetup,
    markers: marker_display::MarkerRuntime,
    spatial_renderer: Option<spatial::Renderer3d>,
    camera_renderer: Option<spatial::Renderer3d>,
    camera_canvas: spatial::calibrated_view::CalibratedSceneView,
    calibrated_scene: bool,
    companion_open: bool,
    companion_selected: Option<ViewId>,
    companion_percent: u8,
    companion_layout_revision: u64,
    companion_was_open: bool,
    shared_scene: spatial::scene_view::SharedSceneView,
    scene_settings: scene_settings::SceneSettings,
    catalog_open: bool,
    topics_tab: bool,
    show_all_topics: bool,
    topic_grouped: bool,
    topic_action_error: Option<String>,
    topic_workflow: topic_workflow::TopicWorkflow,
    observed_tf_frames: Vec<String>,
    recent_tf_frames: Vec<String>,
    expanded_displays: std::collections::BTreeSet<String>,
    global_options_open: bool,
    catalog_by_type: bool,
    catalog_query: String,
    topic_query: String,
    catalog_output: Option<Output>,
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
    details_open: bool,
    compact_inspector_open: bool,
    attention_open: bool,
    setup: bool,
    setup_target: Option<ViewId>,
    draft_best_effort: bool,
    draft_transient_local: bool,
    draft_output: Output,
    draft_map_updates: Option<studio_protocol::map_grid::UpdateSource>,
    draft_spatial: studio_protocol::spatial::Configuration,
    draft_clock_verified: bool,
    window_title: String,
    about: bool,
    help: help::HelpUi,
    diagnostics_open: bool,
    diagnostic_panel: studio_ui_foundation::diagnostics::Panel,
    health: studio_client_core::diagnostic_health::Monitor,
    health_view: studio_ui_foundation::diagnostics_health::View,
    repaint: Option<egui::Context>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            frame_trace: [[0.; 2]; 7],
            robot_setup: robot_setup::RobotSetup::default(),
            markers: Default::default(),
            spatial_renderer: None,
            camera_renderer: None,
            camera_canvas: Default::default(),
            calibrated_scene: false,
            companion_open: false,
            companion_selected: None,
            companion_percent: 30,
            companion_layout_revision: 0,
            companion_was_open: false,
            shared_scene: {
                let mut scene = spatial::scene_view::SharedSceneView::visualizer();
                scene.external_controls = true;
                scene
            },
            scene_settings: Default::default(),
            catalog_open: false,
            topics_tab: true,
            show_all_topics: false,
            topic_grouped: false,
            topic_action_error: None,
            topic_workflow: Default::default(),
            observed_tf_frames: Vec::new(),
            recent_tf_frames: Vec::new(),
            expanded_displays: Default::default(),
            global_options_open: false,
            catalog_by_type: false,
            catalog_query: String::new(),
            topic_query: String::new(),
            catalog_output: None,
            workspace: workspace_app::WorkspaceUi::default(),
            connection: Connection::default(),
            network: if cfg!(test) {
                studio_client_core::network_ui::NetworkSettings::local_fixture()
            } else {
                studio_client_core::network_ui::NetworkSettings::new("visualizer")
            },
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
            details_open: true,
            compact_inspector_open: false,
            attention_open: false,
            setup: false,
            setup_target: None,
            draft_best_effort: false,
            draft_transient_local: false,
            draft_output: Output::CompactImage,
            draft_map_updates: None,
            draft_spatial: empty_spatial_configuration(),
            draft_clock_verified: false,
            window_title: String::new(),
            about: false,
            help: help::HelpUi::default(),
            diagnostics_open: false,
            diagnostic_panel: Default::default(),
            health: Default::default(),
            health_view: Default::default(),
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
        self.topic_workflow = Default::default();
        self.health.disconnected();
        self.scene_settings.disconnect();
        self.observed_tf_frames.clear();
        self.recent_tf_frames.clear();
        self.robot_setup.disconnected();
        if let Some(renderer) = &mut self.spatial_renderer {
            renderer.release_frame();
        }
        self.camera_canvas.clear();
        if let Some(renderer) = &mut self.camera_renderer {
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
        let config = match self.network.config(domain, self.connection.cyclone) {
            Ok(config) => config,
            Err(error) => {
                self.connection.phase = Phase::Failed;
                self.connection.status = error;
                return;
            }
        };
        self.connection.domain = domain.to_string();
        self.stop();
        self.connection.phase = Phase::Starting;
        self.connection.status = "Starting ROS backend; discovering without subscriptions…".into();
        self.connection_deadline = Some(Instant::now() + Duration::from_secs(15));
        self.session = Some(session::launch(config, self.session_generation));
        if let Some(session) = &self.session {
            session.shared.lock().unwrap().repaint = self.repaint.clone();
        }
    }
    fn allow_source_navigation(&mut self) -> bool {
        if self.setup || self.robot_setup.is_open() {
            self.diagnostic(
                "Apply or discard the open display draft before choosing another source.".into(),
            );
            false
        } else {
            true
        }
    }
    #[cfg(test)]
    fn begin_add(&mut self) {
        if !self.allow_source_navigation() {
            return;
        }
        self.sidebar = true;
        self.details_open = true;
        self.compact_inspector_open = true;
        if self.workspace_frozen() {
            return;
        }
        self.cancel_robot_setup();
        self.catalog_open = true;
        self.catalog_output = None;
        self.catalog_query.clear();
        self.scene_settings.open = false;
        self.setup = false;
        self.inspector = true;
    }
    fn begin_source(&mut self, output: Output, topic: &str, ty: &str) {
        if !self.allow_source_navigation() {
            return;
        }
        self.sidebar = true;
        self.details_open = true;
        self.compact_inspector_open = true;
        if self.workspace_frozen() {
            return;
        }
        self.cancel_robot_setup();
        self.catalog_open = false;
        self.scene_settings.open = false;
        self.setup = true;
        self.setup_target = None;
        self.draft_best_effort = matches!(output, Output::SpatialScan | Output::PointCloud);
        self.draft_transient_local = output == Output::MapSnapshot;
        self.draft_map_updates = None;
        self.draft_output = output;
        if let Output::CameraSnapshot(info) = &mut self.draft_output {
            info.topic = camera_display::inferred_info(topic);
        }
        self.discovery.topic = topic.into();
        self.discovery.ty = ty.into();
        self.draft_spatial = self.scene_settings.configuration.clone();
        self.draft_clock_verified = self.scene_settings.verified;
        self.inspector = true;
    }
    /// A graph action owns creation and activation. Repeated Show focuses the
    /// existing exact source rather than allocating another display/reader.
    fn show_topic(&mut self, output: Output, topic: &str, ty: &str) {
        if self.workspace_frozen() || !self.allow_source_navigation() {
            return;
        }
        self.topic_action_error = None;
        if let Some(id) = self
            .views
            .iter()
            .find(|v| {
                if output == Output::Robot {
                    v.configuration.output == Output::Robot
                        && v.robot_description.as_ref().is_some_and(|source| {
                            source.topic == topic && source.message_type == ty
                        })
                } else {
                    v.configuration.topic == topic
                        && v.configuration.message_type == ty
                        && std::mem::discriminant(&v.configuration.output)
                            == std::mem::discriminant(&output)
                }
            })
            .map(|v| v.id)
        {
            self.select_display(id);
            self.set_display_enabled(id, true);
            return;
        }
        if self.views.len() >= MAX_SUBSCRIPTIONS {
            self.topic_action_error = Some(
                "32 displays are configured. Remove a stopped layer before showing another topic."
                    .into(),
            );
            return;
        }
        if output == Output::Robot {
            self.begin_robot_topic(topic);
            return;
        }
        if output != Output::CompactImage
            && self.scene_settings.configuration.fixed_frame.is_empty()
        {
            self.topic_action_error = Some(
                "A parent frame is needed. Choose it in Scene settings, then Show this topic."
                    .into(),
            );
            return;
        }
        let count = self.views.len();
        self.begin_source(output, topic, ty);
        self.subscribe();
        // The temporary construction draft is never part of the Show journey.
        self.setup = false;
        self.setup_target = None;
        if self.views.len() > count {
            let id = self.views.last().unwrap().id;
            self.set_display_enabled(id, true);
        } else {
            self.topic_action_error = Some("Could not show this topic. Check that its exact type is still available and review Diagnostics.".into());
        }
    }

    fn configure_view(&mut self, id: ViewId) {
        self.details_open = true;
        self.compact_inspector_open = true;
        if self.workspace_frozen() {
            return;
        }
        self.catalog_open = false;
        self.scene_settings.open = false;
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
        self.draft_map_updates = view.configuration.map_updates.clone();
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
        if !allowed_output(&self.draft_output) {
            self.diagnostic("Display is unavailable in this application".into());
            return;
        }
        if self.workspace_frozen() {
            return;
        }
        if self.draft_output != Output::TfGraph
            && (self.connection.phase != Phase::Connected || !self.discovery.valid_selection())
        {
            return;
        }
        let configuration = Configuration {
            map_updates: if self.draft_output == Output::MapSnapshot {
                self.draft_map_updates.clone()
            } else {
                None
            },
            robot: None,
            topic: self.discovery.topic.clone(),
            message_type: self.discovery.ty.clone(),
            best_effort: self.draft_best_effort,
            transient_local: self.draft_transient_local,
            output: self.draft_output.clone(),
            spatial: matches!(
                self.draft_output,
                Output::TfGraph
                    | Output::CameraSnapshot(_)
                    | Output::MarkerSnapshot
                    | Output::PoseSnapshot(_)
                    | Output::SpatialScan
                    | Output::PointCloud
                    | Output::MapSnapshot
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
        let mut validation = configuration.clone();
        if let Some(spatial) = &mut validation.spatial {
            spatial.clock_policy =
                studio_protocol::spatial_input::SpatialClockPolicy::VerifiedSystem;
        }
        if !validation.valid() {
            self.diagnostic("Topic/type exceeds supported subscription bounds".into());
            return;
        }
        if self.setup_target.is_none() && self.views.len() >= MAX_SUBSCRIPTIONS {
            self.diagnostic(
                "32 displays are configured. Stop and remove a view before adding another.".into(),
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
        if let Some(id) = self.setup_target {
            let Some(index) = self.views.iter().position(|v| v.id == id) else {
                return;
            };
            self.views[index].spatial_clock_verified = self.draft_clock_verified;
            if self.views[index].subscription.is_some() {
                if let Err(error) = self.admission(&configuration, self.setup_target) {
                    self.diagnostic(error);
                    return;
                }
                self.views[index].pending = Some(configuration);
                self.stop_view(id);
            } else {
                if self.views[index].configuration.output != configuration.output {
                    self.views[index].kind = kind_for_output(configuration.output.clone());
                }
                self.views[index].configuration = configuration;
            }
        } else {
            if self.views.len() >= MAX_SUBSCRIPTIONS {
                self.diagnostic(
                    "32 displays are configured. Stop and remove a view before adding another."
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
            self.expanded_displays
                .insert(self.views.last().unwrap().workspace_id.clone());
            if self
                .views
                .last()
                .is_some_and(|v| matches!(v.kind, ViewKind::Image | ViewKind::Camera))
            {
                self.companion_selected = Some(id);
                self.companion_open = true;
            }
        }
        self.setup = false;
        self.setup_target = None;
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
        resource_usage(&configurations).map_err(str::to_owned)
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
        let source = self.views[index].source_label();
        if self
            .views
            .iter()
            .any(|v| v.id != id && v.source_label() == source && v.subscription.is_some())
        {
            self.views[index].error = Some("Another mode for this topic still owns a reader. Change display mode in the inspector after cleanup.".into());
            return;
        }
        let configuration = self.views[index].configuration.clone();
        if configuration.owns_tf() && !self.scene_matches(&configuration) {
            self.views[index].error = Some("This display differs from Scene settings. Review and apply Scene settings before enabling.".into());
            return;
        }
        if configuration.owns_tf() && !self.scene_settings.verified {
            self.views[index].error =
                Some("Select System time for this scene before starting reception.".into());
            return;
        }
        if !allowed_output(&configuration.output) {
            self.views[index].error = Some("Display belongs to the other application".into());
            return;
        }
        if configuration.output == Output::Robot {
            let valid_model = self.views[index]
                .robot
                .as_ref()
                .zip(configuration.robot.as_ref())
                .is_some_and(|(live, c)| c.frames == live.loaded().prepared.link_names);
            if !valid_model || !self.views[index].spatial_clock_verified {
                self.views[index].error =
                    Some("Load and review the model before starting Robot.".into());
                return;
            }
        }
        if matches!(
            configuration.output,
            Output::TfGraph
                | Output::CameraSnapshot(_)
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::SpatialScan
                | Output::PointCloud
                | Output::MapSnapshot
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        ) && !self.views[index].spatial_clock_verified
        {
            self.views[index].error =
                Some("This display is not configured for System time.".into());
            return;
        }
        if configuration.output != Output::TfGraph
            && !self.discovery.topics.iter().any(|(topic, types)| {
                topic == &configuration.topic && types.contains(&configuration.message_type)
            })
        {
            self.views[index].error =
                Some("Topic/type is unavailable. Configure this view from discovered data.".into());
            return;
        }
        if configuration.output == Output::MapSnapshot
            && !self.map_updates_available(&configuration)
        {
            self.views[index].error=Some("Update topic/type is unavailable. Review the optional update source before enabling.".into());
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
            Output::TfGraph
                | Output::CameraSnapshot(_)
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::SpatialScan
                | Output::PointCloud
                | Output::MapSnapshot
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        ) && view.spatial_clock_verified
        {
            // Persist configuration intent, separately from session-local verification.
            view.workspace_clock_expectation =
                crate::components::workspace::ClockExpectation::System;
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
        if matches!(view.kind, ViewKind::Spatial | ViewKind::PointCloud) {
            if let Err(error) = view.begin_sensor() {
                self.fail(error);
                return;
            }
        }
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
        if view.configuration.output == Output::MapSnapshot {
            let result = view
                .begin_map()
                .and_then(|target| session.shared.lock().unwrap().bind_map(target));
            if let Err(error) = result {
                self.fail(error);
                return;
            }
        }
        if view.configuration.output == Output::MarkerSnapshot {
            let result = view
                .begin_marker()
                .and_then(|target| session.shared.lock().unwrap().bind_marker(target));
            if let Err(error) = result {
                self.fail(error);
                return;
            }
        }
        if matches!(view.configuration.output, Output::CameraSnapshot(_)) {
            let result = view
                .begin_camera()
                .and_then(|target| session.shared.lock().unwrap().bind_camera(target));
            if let Err(error) = result {
                self.fail(error);
                return;
            }
            self.calibrated_scene = true;
        }
        if matches!(view.configuration.output, Output::PoseSnapshot(_)) {
            let result = view
                .begin_uncertainty()
                .and_then(|target| session.shared.lock().unwrap().bind_pose(target));
            if let Err(error) = result {
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
        self.prune_scene_resources();
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
        if self.topic_refresh_event(&event) {
            return;
        }
        let health_owner = self.health.owner();
        if self
            .health
            .event(&event, received_at, self.session.as_ref())
        {
            if let Some((generation, subscription)) =
                health_owner.filter(|_| self.health.owner().is_none())
            {
                self.workspace_cleanup_confirmed(generation, subscription);
            }
            return;
        }
        if self.marker_control_event(&event) {
            return;
        }
        if self.robot_capture_event(&event) {
            return;
        }
        match event {
            Event::Ready { distro, rmw, .. } => {
                if self.connection.phase != Phase::Starting {
                    return;
                }
                self.connection.phase = Phase::Connected;
                self.connection.status = format!("Connected: {distro} / {rmw}");
                self.connection.actual = Some((distro.into_string(), rmw.into_string()));
                self.connection_deadline = None;
                self.use_system_scene_defaults();
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
            Event::TfGraphObservation {
                generation,
                subscription,
                sequence,
                sample,
                backend_drops,
            } => {
                self.apply_tf_graph(generation, subscription, sequence, *sample, backend_drops);
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
        let previous_health_phase = self.health.phase;
        self.health.poll(self.session.as_ref());
        if previous_health_phase != self.health.phase
            && self.health.phase == studio_client_core::diagnostic_health::Phase::Error
        {
            if let Some((generation, subscription)) = self.health.owner() {
                self.workspace_stop_failed(
                    generation,
                    subscription,
                    "Health acknowledgement timed out; disconnect to recover",
                );
            }
        }
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
        let tool_events: Vec<_> = self
            .session
            .as_ref()
            .map(|s| s.shared.lock().unwrap().tool_events.drain(..).collect())
            .unwrap_or_default();
        for event in tool_events {
            self.handle_at(event, Instant::now());
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
                self.health.record_counters(subscription, &counters);
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
        if let Some(frames) = self
            .session
            .as_ref()
            .and_then(|s| s.shared.lock().unwrap().scene_frames.take())
        {
            self.retain_scene_frames(frames);
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
            if let Some((mut frame, update)) = view
                .spatial_stream
                .take_with_status(view.spatial_generation)
            {
                if view.sensor_token.is_some() {
                    if let Err(error) = view.receive_sensor(
                        update.sequence,
                        frame.acquisition_stamp_ns,
                        std::mem::take(&mut frame.sensor_points),
                        frame.tf_status() == spatial::SpatialTfStatus::Ready,
                    ) {
                        spatial_errors.push((view.id, error));
                        continue;
                    }
                    view.absorb_spatial_counters(update.source_counters);
                    view.spatial_status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                        "Receiving".into()
                    } else {
                        format!(
                            "Receiving scans; waiting for {} → {} transform ({})",
                            frame.source_frame,
                            frame.fixed_frame,
                            frame.tf_status().label()
                        )
                    };
                    view.spatial_details = format!(
                        "Acquisition: {} ns · Superseded before display: {}",
                        frame.acquisition_stamp_ns, update.superseded
                    );
                    if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                        view.status = ViewStatus::Receiving;
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
            if let Some((mut frame, update)) =
                view.cloud_stream.take_with_status(view.cloud_generation)
            {
                if let Err(error) = view.receive_sensor(
                    update.sequence,
                    frame.acquisition_stamp_ns,
                    std::mem::take(&mut frame.sensor_points),
                    frame.tf_status() == spatial::SpatialTfStatus::Ready,
                ) {
                    spatial_errors.push((view.id, error));
                    continue;
                }
                view.status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                    ViewStatus::Receiving
                } else {
                    ViewStatus::Waiting
                };
                view.spatial_status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                    "Receiving".into()
                } else {
                    format!(
                        "Receiving scans; waiting for {} → {} transform ({})",
                        frame.source_frame,
                        frame.fixed_frame,
                        frame.tf_status().label()
                    )
                };
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
        let map_faults = self
            .session
            .as_ref()
            .map(|s| std::mem::take(&mut s.shared.lock().unwrap().map_faults))
            .unwrap_or_default();
        for fault in map_faults {
            if let Some(view) = self.views.iter().find(|v| {
                v.generation == fault.generation
                    && v.subscription == Some(fault.subscription)
                    && (v.status.active() || v.status == ViewStatus::Starting)
            }) {
                spatial_errors.push((view.id, fault.message));
            }
        }
        for view in &mut self.views {
            if view.configuration.output != Output::MapSnapshot
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
            if active {
                if let Err(error) = view.poll_map() {
                    spatial_errors.push((view.id, error));
                }
            }
        }
        self.prune_scene_resources();
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
            if !view.configuration.output.is_mapping()
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
                view.spatial_status = if frame.tf_status() == spatial::SpatialTfStatus::Ready {
                    "Receiving".into()
                } else {
                    format!(
                        "Receiving scans; waiting for {} → {} transform ({})",
                        frame.source_frame,
                        frame.fixed_frame,
                        frame.tf_status().label()
                    )
                };
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
impl App {
    /// Shared native frame path; opt-in load qualification runs this same entry point.
    fn update_frame(&mut self, ctx: &egui::Context) {
        studio_ui_foundation::diagnostics::begin_frame(ctx);
        self.diagnostic_panel.open = self.diagnostics_open;
        self.diagnostic_panel.shortcut(ctx);
        self.diagnostics_open = self.diagnostic_panel.open;
        let diagnostic_frame_start = Instant::now();
        #[cfg(test)]
        let mut trace = frame_trace::Trace::new();
        self.repaint = Some(ctx.clone());
        self.poll_backend();
        #[cfg(test)]
        {
            self.frame_trace[0] = trace.lap();
        }
        self.poll_sensor_history(ctx);
        #[cfg(test)]
        {
            self.frame_trace[1] = trace.lap();
        }
        self.poll_markers(ctx);
        self.poll_uncertainty();
        self.poll_cameras();
        #[cfg(test)]
        {
            self.frame_trace[2] = trace.lap();
        }
        self.robot_poll();
        self.workspace_poll(ctx);
        self.poll_scene_settings();
        self.poll_topic_workflow();
        // The scene is permanent, including empty and Image-only workspaces.
        // Keep its GPU attachments until resize or explicit lifecycle cleanup.
        #[cfg(test)]
        {
            self.frame_trace[3] = trace.lap();
        }
        let diagnostic_poll_ms = diagnostic_frame_start.elapsed().as_secs_f64() * 1000.;
        let diagnostic_ui_start = Instant::now();
        self.render(ctx);
        studio_ui_foundation::diagnostics::timing(
            ctx,
            diagnostic_poll_ms,
            0.,
            diagnostic_ui_start.elapsed().as_secs_f64() * 1000.,
        );
        #[cfg(test)]
        {
            self.frame_trace[4] = trace.lap();
        }
        self.submit_marker_projection();
        #[cfg(test)]
        {
            self.frame_trace[5] = trace.lap();
        }
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
        #[cfg(test)]
        {
            self.frame_trace[6] = trace.lap();
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_frame(ctx);
    }

    fn on_exit(&mut self) {
        self.network.finish_shutdown();
        // macOS Cmd-Q can bypass run_native's return and normal App drop.
        // Cancel now and let the worker reap its backend before exit proceeds.
        self.stop();
        self.workspace_shutdown();
        if let Err(error) = self.robot_setup.finish_shutdown() {
            eprintln!("Robot asset worker shutdown failed: {error}");
            self.diagnostic(error);
        }
        if let Err(error) = self.markers.finish_shutdown() {
            eprintln!("Marker worker shutdown failed: {error}");
        }
        session::finish_shutdown();
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.markers.request_shutdown();
        self.stop();
    }
}
fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--suite-configuration") {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"distribution":if studio_network::MANAGED_FIREWALL_AVAILABLE {"bundled"} else {"microsoft-store"}})
        );
        return Ok(());
    }
    let _lease = studio_app_lifecycle::state_directory()
        .and_then(|state| studio_app_lifecycle::AppLease::running(&state, "visualizer"))
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;
    let result = eframe::run_native(
        "ROS Studio Visualizer",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 820.0])
                .with_icon(studio_ui_foundation::app_icons::AppIcon::Visualizer.window_icon())
                .with_min_inner_size([760.0, 560.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            fonts::configure(&cc.egui_ctx);
            shell::configure(&cc.egui_ctx);
            let mut app = App::default();
            app.spatial_renderer = cc
                .wgpu_render_state
                .clone()
                .map(spatial::Renderer3d::with_sensor_history);
            app.camera_renderer = cc
                .wgpu_render_state
                .as_ref()
                .map(|state| spatial::Renderer3d::with_sensor_history(state.clone()));
            Ok(Box::new(app))
        }),
    );
    session::finish_shutdown();
    result
}

#[cfg(test)]
mod display_tests;
#[cfg(test)]
mod split_tests;

fn allowed_output(output: &Output) -> bool {
    !matches!(output, Output::Typed)
}

#[cfg(all(test, unix))]
mod map_ros_tests;
#[cfg(all(test, unix))]
mod marker_ros_tests;
#[cfg(test)]
mod uncertainty_ros_tests;

#[cfg(all(test, unix))]
mod camera_performance_tests;
#[cfg(test)]
mod camera_ros_tests;

#[cfg(test)]
mod scene_defaults_ros_tests;

#[cfg(test)]
mod multi_map_ros_tests;

#[cfg(all(test, unix))]
mod capacity32_ros_tests;
