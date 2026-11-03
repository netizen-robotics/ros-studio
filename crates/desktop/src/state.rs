use crate::{images, kind_for_output, plots, spatial, ViewKind};
use std::time::Instant;
use studio_protocol::subscriptions::*;

pub use studio_client_core::model::{Connection, Discovery, Phase, ViewStatus};
#[derive(Default)]
pub struct Received {
    pub typed: Option<Box<studio_protocol::structured::Sample>>,
    pub typed_samples: u64,
    pub backend_drops: u64,
}
pub struct ViewState {
    /// Stable document identity, never a wire/session/runtime view identity.
    pub workspace_id: String,
    pub workspace_scan_layer_id: String,
    pub workspace_clock_expectation: studio_desktop::workspace::ClockExpectation,
    pub id: ViewId,
    pub configuration: Configuration,
    pub subscription: Option<SubscriptionId>,
    pub generation: Generation,
    pub request: Option<RequestId>,
    pub status: ViewStatus,
    pub deadline: Option<Instant>,
    pub pending: Option<Configuration>,
    pub error: Option<String>,
    pub kind: ViewKind,
    pub received: Received,
    pub queue_drops: u64,
    pub plot_draft: Option<plots::FieldPath>,
    pub numeric_choices: Vec<plots::FieldPath>,
    pub trace: plots::Trace,
    pub origin: Instant,
    pub image: images::ImageView,
    pub image_stream: Option<images::ImageStream>,
    pub image_sequence: u64,
    /// Session-local authorization; never restored from a workspace file.
    pub spatial_clock_verified: bool,
    pub spatial_scene: spatial::Scene,
    pub spatial_camera: spatial::Camera,
    pub spatial_stream: spatial::SpatialStream,
    pub cloud_stream: spatial::cloud_live::CloudStream,
    pub cloud_generation: u64,
    pub mapping_stream: spatial::mapping_live::MappingStream,
    pub mapping_generation: u64,
    pub mapping_frame: Option<spatial::mapping_live::MappingFrame>,
    pub cloud_frame: Option<spatial::cloud_live::CloudFrame>,
    pub spatial_generation: u64,
    pub spatial_sequence: u64,
    pub spatial_token: Option<spatial::LayerToken>,
    pub spatial_status: String,
    pub spatial_details: String,
    pub spatial_counters: studio_protocol::spatial::Counters,
    pub robot: Option<studio_desktop::robot::RobotLive>,
    pub robot_assets: Option<studio_desktop::workspace::RobotAssets>,
    pub robot_token: Option<studio_desktop::robot::RobotToken>,
    pub robot_view: studio_desktop::robot::RobotView,
    pub robot_status: String,
    pub robot_render_failure: Option<String>,
    pub enabled: bool,
}
impl ViewState {
    pub fn status_label(&self) -> &'static str {
        if matches!(self.kind, ViewKind::Spatial)
            && self.status == ViewStatus::Waiting
            && self.spatial_counters.scans_received > 0
        {
            "Waiting for transform"
        } else {
            self.status.label()
        }
    }
    pub fn absorb_spatial_counters(&mut self, incoming: studio_protocol::spatial::Counters) {
        let current = &mut self.spatial_counters;
        current.scans_received = current.scans_received.max(incoming.scans_received);
        current.pending_replaced = current.pending_replaced.max(incoming.pending_replaced);
        current.lookup_timeouts = current.lookup_timeouts.max(incoming.lookup_timeouts);
        current.tf_dynamic_batches = current.tf_dynamic_batches.max(incoming.tf_dynamic_batches);
        current.tf_static_batches = current.tf_static_batches.max(incoming.tf_static_batches);
        current.tf_accepted = current.tf_accepted.max(incoming.tf_accepted);
        current.tf_rejected = current.tf_rejected.max(incoming.tf_rejected);
        current.duplicate_statics = current.duplicate_statics.max(incoming.duplicate_statics);
        current.native_errors = current.native_errors.max(incoming.native_errors);
    }
    pub fn new(id: ViewId, configuration: Configuration) -> Self {
        let kind = kind_for_output(configuration.output.clone());
        Self {
            workspace_id: format!("panel_{}", id.0),
            workspace_scan_layer_id: "scan".into(),
            workspace_clock_expectation: studio_desktop::workspace::ClockExpectation::Unknown,
            id,
            configuration,
            subscription: None,
            generation: Generation(0),
            request: None,
            status: ViewStatus::Stopped,
            deadline: None,
            pending: None,
            error: None,
            kind,
            received: Received::default(),
            queue_drops: 0,
            plot_draft: None,
            numeric_choices: Vec::new(),
            trace: plots::Trace::default(),
            origin: Instant::now(),
            image: images::ImageView::default(),
            image_stream: None,
            image_sequence: 0,
            spatial_clock_verified: false,
            spatial_scene: spatial::Scene::default(),
            spatial_camera: spatial::Camera::default(),
            spatial_stream: spatial::SpatialStream::default(),
            cloud_stream: Default::default(),
            cloud_generation: 0,
            mapping_stream: Default::default(),
            mapping_generation: 0,
            mapping_frame: None,
            cloud_frame: None,
            spatial_generation: 0,
            spatial_sequence: 0,
            spatial_token: None,
            spatial_status: "Stopped".into(),
            spatial_details: String::new(),
            spatial_counters: studio_protocol::spatial::Counters::default(),
            robot: None,
            robot_assets: None,
            robot_token: None,
            robot_view: studio_desktop::robot::RobotView::default(),
            robot_status: "Load a Robot model before enabling".into(),
            robot_render_failure: None,
            enabled: false,
        }
    }
    pub fn clear_live(&mut self) {
        if let Some(robot) = &mut self.robot {
            robot.stop();
        }
        self.robot_token = None;
        self.robot_render_failure = None;
        self.robot_status = "Stopped · model retained, live poses cleared".into();
        self.cloud_stream.stop();
        self.mapping_stream.stop();
        self.mapping_frame = None;
        self.cloud_frame = None;
        self.spatial_stream.stop();
        self.spatial_scene = spatial::Scene::default();
        self.spatial_sequence = 0;
        self.spatial_token = None;
        self.spatial_status = "Stopped".into();
        self.spatial_details.clear();
        self.image.stop();
        self.image_stream = None;
        self.image_sequence = 0;
        self.received = Received {
            backend_drops: self.received.backend_drops,
            ..Received::default()
        };
        self.numeric_choices = Vec::new();
        self.trace.begin(self.subscription.map_or(0, |id| id.0));
    }
    pub fn deactivate(&mut self) {
        self.enabled = false;
        self.subscription = None;
        self.request = None;
        self.status = ViewStatus::Stopped;
        self.deadline = None;
        self.pending = None;
        self.clear_live();
    }
}
