//! Visualizer composition for shared complete pose models and local appearance.
use super::*;
use spatial::uncertainty::{Appearance, Geometry, Layer, Prepared};
use std::sync::Arc;
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{
        self as pose,
        transport::{HistoryConfiguration, Permission},
    },
};
pub struct UncertaintyDisplay {
    pub stream: Option<backend_pipe::PoseStream>,
    pub prepared: Option<Arc<Prepared>>,
    pub permission: Option<Permission>,
    pub geometry: Option<Geometry>,
    pub appearance: Appearance,
    pub history_draft: HistoryConfiguration,
    clear_started: Option<Instant>,
}
impl UncertaintyDisplay {
    pub fn new(output: Output) -> Self {
        Self {
            stream: None,
            prepared: None,
            permission: None,
            geometry: None,
            appearance: Default::default(),
            history_draft: match output {
                Output::PoseSnapshot(c) => c.history.unwrap_or_default(),
                _ => Default::default(),
            },
            clear_started: None,
        }
    }
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.stop();
        }
        self.clear_cached();
        self.clear_started = None;
    }
    fn clear_cached(&mut self) {
        self.prepared = None;
        self.permission = None;
        self.geometry = None;
    }
    fn rebuild(&mut self) -> Result<(), String> {
        self.geometry = self
            .prepared
            .as_ref()
            .map(|p| Geometry::new(p, self.appearance))
            .transpose()?;
        Ok(())
    }
    pub fn layer(&self, id: u64) -> Option<Layer<'_>> {
        let layer = Layer {
            id,
            prepared: self.prepared.as_deref()?,
            geometry: self.geometry.as_ref()?,
            permission: self.permission.as_ref()?,
            width: self.appearance.line_width_px,
        };
        layer.is_current().then_some(layer)
    }
}
impl ViewState {
    pub(super) fn begin_uncertainty(&mut self) -> Result<backend_pipe::PoseTarget, String> {
        let Output::PoseSnapshot(configuration) = self.configuration.output else {
            return Err("Missing pose configuration".into());
        };
        let owner = pose::Owner {
            generation: self.generation,
            subscription: self.subscription.ok_or("Missing pose subscription")?,
            kind: configuration.kind,
            topic: self.configuration.topic.clone(),
            fixed_frame: self
                .configuration
                .spatial
                .as_ref()
                .ok_or("Missing pose scene")?
                .fixed_frame
                .clone(),
        };
        let stream = backend_pipe::PoseStream::new(owner.clone(), configuration)?;
        self.uncertainty.stream = Some(stream.clone());
        self.uncertainty.history_draft = configuration.history.unwrap_or_default();
        self.spatial_status = "Waiting for pose data and transforms".into();
        Ok(backend_pipe::PoseTarget {
            owner,
            configuration,
            stream,
        })
    }
    fn poll_uncertainty(&mut self) -> Result<(), String> {
        if let Some(stream) = &self.uncertainty.stream {
            if self.uncertainty.clear_started.is_some() {
                if !stream.is_waiting_for_clear() {
                    self.uncertainty.clear_started = None;
                } else if self
                    .uncertainty
                    .clear_started
                    .is_some_and(|start| start.elapsed() > Duration::from_secs(10))
                {
                    return Err("Clear timed out; stopping this display".into());
                }
            }
        }
        let result = self.uncertainty.stream.as_ref().and_then(|s| s.take());
        if let Some(result) = result {
            match result? {
                backend_pipe::PoseUpdate::Snapshot {
                    prepared,
                    permission,
                    ..
                } => {
                    self.received.typed_samples = prepared.snapshot.counters.received;
                    let historical = prepared.trail.len();
                    self.spatial_status = if prepared.head.is_some() {
                        self.status = ViewStatus::Receiving;
                        if prepared.snapshot.owner.kind == pose::Kind::Odometry {
                            format!("Current pose · {historical} retained poses")
                        } else {
                            "Current pose with covariance".into()
                        }
                    } else {
                        self.status = ViewStatus::Waiting;
                        match prepared.snapshot.latest.as_ref().map(|e| &e.transform) {
                            None => "Waiting for a new pose sample".into(),
                            Some(LatestPose::Ready { .. }) => {
                                "Pose is outside supported world bounds".into()
                            }
                            Some(status) => format!(
                                "No current transform: {status:?} · {historical} retained poses"
                            ),
                        }
                    };
                    if prepared.snapshot.known_history_gap {
                        self.spatial_status.push_str(" · Known history gap");
                    }
                    self.uncertainty.prepared = Some(prepared);
                    self.uncertainty.permission = Some(permission);
                    self.uncertainty.rebuild()?;
                    self.error = None;
                    self.deadline = None;
                }
                backend_pipe::PoseUpdate::Invalidated(barrier) => {
                    self.uncertainty.clear_cached();
                    self.status = ViewStatus::Waiting;
                    self.spatial_status =
                        format!("Pose hidden: {:?} · Waiting for fresh data", barrier.reason);
                }
            }
        }
        if self
            .uncertainty
            .permission
            .as_ref()
            .is_some_and(|p| !p.is_valid())
        {
            self.uncertainty.clear_cached();
        }
        Ok(())
    }
}
impl App {
    pub(super) fn poll_uncertainty(&mut self) {
        let mut errors = Vec::new();
        for view in &mut self.views {
            if matches!(view.kind, ViewKind::Uncertainty(_))
                && (view.status.active() || view.status == ViewStatus::Starting)
            {
                if let Err(error) = view.poll_uncertainty() {
                    errors.push((view.id, error));
                }
            }
        }
        for (id, error) in errors {
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.error = Some(error.clone());
            }
            self.diagnostic(error);
        }
    }
    pub(super) fn clear_uncertainty(&mut self, index: usize) {
        let Some(subscription) = self.views[index].subscription else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let result = session
            .shared
            .lock()
            .unwrap()
            .clear_pose_explicit(subscription);
        match result {
            Ok(command) => {
                self.views[index].uncertainty.clear_cached();
                self.views[index].uncertainty.clear_started = Some(Instant::now());
                self.views[index].status = ViewStatus::Waiting;
                self.views[index].spatial_status = "Clearing pose geometry".into();
                if let Err(error) = session.send(command) {
                    self.fail(error);
                }
            }
            Err(error) => self.views[index].error = Some(error),
        }
    }
    pub(super) fn apply_uncertainty_history(&mut self, index: usize) {
        if self.workspace_frozen() || self.setup {
            return;
        }
        let Output::PoseSnapshot(mut configuration) = self.views[index].configuration.output else {
            return;
        };
        configuration.history = Some(self.views[index].uncertainty.history_draft);
        if let Err(error) = configuration.settings() {
            self.views[index].error = Some(error);
            return;
        }
        let mut source = self.views[index].configuration.clone();
        source.output = Output::PoseSnapshot(configuration);
        if self.views[index].subscription.is_some() {
            self.views[index].pending = Some(source);
            self.stop_view(self.views[index].id);
        } else {
            self.views[index].configuration = source;
        }
    }
    pub(super) fn uncertainty_inspector_ui(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        frozen: bool,
    ) {
        let mut clear = false;
        let mut apply = false;
        let setup = self.setup;
        let view = &mut self.views[index];
        let before = view.uncertainty.appearance;
        ui.separator();
        studio_ui_foundation::theme::strong(ui, "Pose and uncertainty");
        ui.small("Marginal standard-deviation shapes; they do not show joint confidence.");
        ui.add_enabled_ui(!frozen,|ui|{
            ui.checkbox(&mut view.uncertainty.appearance.position,"Position uncertainty");ui.checkbox(&mut view.uncertainty.appearance.orientation,"Angular uncertainty");
            let a=&mut view.uncertainty.appearance;
            let mut sigma=f64::from(a.sigma_milli)/1000.;
            if ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut sigma,0.1..=6.).text("Standard deviations (σ)"))).changed(){a.sigma_milli=(sigma*1000.).round() as u16;}
            ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut a.pose_length_mm,10..=5000).text("Pose size (mm)")));ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut a.line_width_px,1..=8).text("Line width (px)")));
            ui.small("Angular cones use header axes and are capped at89°. X red, Y green, Z blue.");
            clear=ui.add_enabled(view.status.active() && view.uncertainty.stream.as_ref().is_some_and(|s|!s.is_waiting_for_clear()),egui::Button::new("Clear pose geometry")).clicked();
            if let Output::PoseSnapshot(configuration)=view.configuration.output {if let Some(active)=configuration.history {
                ui.separator();studio_ui_foundation::theme::strong(ui, "Odometry history");ui.checkbox(&mut a.history_covariance,"Show uncertainty on historical poses");let draft=&mut view.uncertainty.history_draft;
                ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut draft.keep,1..=256).text("Keep poses")));
                ui.horizontal(|ui|{let label=ui.label("Position threshold (m)");let mut meters=f64::from(draft.position_tolerance_um)/1_000_000.;if ui.add(egui::DragValue::new(&mut meters).speed(0.01).range(0.0..=1000.)).labelled_by(label.id).changed(){draft.position_tolerance_um=(meters*1_000_000.).round() as u32;}});
                ui.horizontal(|ui|{let label=ui.label("Angular threshold (°)");let mut degrees=(f64::from(draft.angle_tolerance_urad)/1_000_000.).to_degrees();if ui.add(egui::DragValue::new(&mut degrees).speed(0.1).range(0.0..=180.)).labelled_by(label.id).changed(){draft.angle_tolerance_urad=(degrees.to_radians()*1_000_000.).round().min(3_141_592.) as u32;}});
                ui.small("Thresholds keep a pose when distance or angle reaches its limit. The current pose always stays fresh.");
                ui.horizontal(|ui|{
                    apply=ui.add_enabled(!setup && *draft!=active && !matches!(view.status,ViewStatus::Starting|ViewStatus::Stopping|ViewStatus::NeedsRecovery),egui::Button::new("Apply history")).on_hover_text("Stops and replaces this source after cleanup; other displays continue.").clicked();
                    if ui.add_enabled(*draft!=active,egui::Button::new("Discard history draft")).clicked(){*draft=active;}
                });
            }}
        });
        if before != view.uncertainty.appearance {
            if let Err(error) = view.uncertainty.rebuild() {
                view.error = Some(error);
            }
        }
        if let Some(geometry) = &view.uncertainty.geometry {
            if geometry.hidden_shapes > 0 {
                ui.small(format!(
                    "{} glyph parts exceed world bounds",
                    geometry.hidden_shapes
                ));
            }
            if geometry.capped_angles > 0 {
                ui.small(format!(
                    "{} angular radii capped at89°",
                    geometry.capped_angles
                ));
            }
        }
        if let Some(prepared) = &view.uncertainty.prepared {
            let snapshot = &prepared.snapshot;
            ui.small(format!(
                "Received: {} · Filtered: {} · Evicted: {} · TF timeouts: {}",
                snapshot.counters.received,
                snapshot.counters.filtered,
                snapshot.counters.evicted,
                snapshot.counters.lookup_timeouts
            ));
            if let Some(latest) = &snapshot.latest {
                ui.monospace(format!("Source stamp: {}ns", latest.acquisition_stamp_ns));
                if let Some(source) = &snapshot.source {
                    ui.add(egui::Label::new(format!("Header frame: {}", source.frame_id)).wrap());
                    if let Some(child) = &source.child_frame_id {
                        ui.add(egui::Label::new(format!("Child frame (metadata): {child}")).wrap());
                    }
                }
                ui.collapsing("Source covariance matrix",|ui|{
                    ui.small("Rows/columns: X Y Z roll pitch yaw. Units: m², rad² and mixed m·rad cross terms.");
                    egui::ScrollArea::horizontal().id_salt(("covariance-matrix-scroll",view.id.0)).show(ui,|ui|{
                        egui::Grid::new(("source-covariance",view.id.0)).show(ui,|ui|{
                            for row in latest.covariance.row_major().chunks_exact(6) {for value in row {ui.monospace(format!("{value:.3e}"));}ui.end_row();}
                        });
                    });
                });
            }
        }
        if clear {
            self.clear_uncertainty(index);
        }
        if apply {
            self.apply_uncertainty_history(index);
        }
    }
}
