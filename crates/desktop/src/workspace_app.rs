//! Workspace document, lifecycle and layout integration for the native App.
use super::*;
use studio_desktop::{
    workspace as file, workspace_dialog::DialogKind, workspace_document::Controller,
};
#[path = "workspace_app/model.rs"]
mod model;

pub(super) struct WorkspaceUi {
    pub open: bool,
    pub name: String,
    pub layout: file::Layout,
    pub connection_suggestion: Option<file::ConnectionSuggestion>,
    pub path_entry: String,
    controller: Controller,
    prepared: Option<PreparedWorkspace>,
}
struct PreparedWorkspace {
    revision: u64,
    base_next_view: u64,
    next_view: u64,
    configuration: file::Workspace,
    views: Vec<ViewState>,
}
pub(super) enum FileAction {
    Open,
    Save,
    SaveAs,
}
impl Default for WorkspaceUi {
    fn default() -> Self {
        let empty = model::empty_workspace();
        Self {
            open: false,
            name: empty.name.clone(),
            layout: empty.layout.clone(),
            connection_suggestion: None,
            path_entry: String::new(),
            controller: Controller::new(
                empty,
                file::SupportedRenderers {
                    map: false,
                    uncertainty: false,
                    camera: false,
                    tf_graph: false,
                    robot_topic: false,
                    ..file::SupportedRenderers::all()
                },
            )
            .expect("valid empty workspace"),
            prepared: None,
        }
    }
}
impl App {
    pub(super) fn workspace_needs_attention(&self) -> bool {
        self.workspace.controller.error().is_some()
    }
    pub(super) fn workspace_dirty(&self) -> bool {
        self.workspace.controller.dirty()
    }
    pub(super) fn workspace_file_available(&self) -> bool {
        !self.workspace.controller.is_busy() && !self.workspace_frozen()
    }
    pub(super) fn workspace_file(&mut self, action: FileAction) -> Result<(), String> {
        self.workspace_observe()?;
        match action {
            FileAction::Open => self.workspace.controller.open(),
            FileAction::Save => self.workspace.controller.save(),
            FileAction::SaveAs => self.workspace.controller.save_as(),
        }
    }
    /// Call after a new user-created panel is pushed; custom restored IDs may
    /// resemble generated names and must never collide or be overwritten.
    pub(super) fn workspace_adopt_new_view(&mut self, id: ViewId) {
        let occupied = self
            .views
            .iter()
            .filter(|view| view.id != id)
            .map(|view| view.workspace_id.clone())
            .collect::<Vec<_>>();
        let base = format!("panel_{}", id.0);
        let mut candidate = base.clone();
        let mut suffix = 1;
        while occupied.contains(&candidate) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        if let Some(view) = self.views.iter_mut().find(|view| view.id == id) {
            view.workspace_id = candidate;
        }
    }
    pub(super) fn workspace_frozen(&self) -> bool {
        self.workspace.controller.is_applying()
    }
    pub(super) fn workspace_snapshot(&self) -> Result<file::Workspace, String> {
        let mut views = Vec::with_capacity(self.views.len());
        for view in &self.views {
            let kind = match &view.kind {
                ViewKind::Robot => {
                    let config = view
                        .configuration
                        .robot
                        .as_ref()
                        .ok_or("Robot TF configuration is missing")?;
                    file::ViewKind::Robot {
                        assets: view
                            .robot_assets
                            .clone()
                            .ok_or("Robot asset locations are missing")?,
                        fixed_frame: config.fixed_frame.clone(),
                        transforms: file::SpatialTransforms {
                            dynamic_tf: file::TransformInput {
                                topic: config.dynamic_tf_topic.clone(),
                                message_type: spatial::TF_MESSAGE_TYPE.into(),
                                qos: model::qos(&view.configuration),
                            },
                            static_tf: file::TransformInput {
                                topic: config.static_tf_topic.clone(),
                                message_type: spatial::TF_MESSAGE_TYPE.into(),
                                qos: file::Qos {
                                    reliability: file::Reliability::Reliable,
                                    durability: file::Durability::TransientLocal,
                                },
                            },
                        },
                        clock_expectation: view.workspace_clock_expectation,
                    }
                }
                ViewKind::Messages => file::ViewKind::Messages,
                ViewKind::Plot(path) => file::ViewKind::numeric_plot(path),
                ViewKind::Image => file::ViewKind::Image {
                    display_mode: match view.image.display_mode() {
                        images::DisplayMode::Fit => file::ImageMode::Fit,
                        images::DisplayMode::ActualSize => file::ImageMode::ActualSize,
                    },
                },
                ViewKind::PointCloud | ViewKind::Mapping(_) => {
                    let config = view
                        .configuration
                        .spatial
                        .as_ref()
                        .ok_or("Scene overlay TF configuration is missing")?;
                    let fixed_frame = config.fixed_frame.clone();
                    let transforms = file::SpatialTransforms {
                        dynamic_tf: file::TransformInput {
                            topic: config.dynamic_tf_topic.clone(),
                            message_type: spatial::TF_MESSAGE_TYPE.into(),
                            qos: file::Qos {
                                reliability: if config.dynamic_tf_best_effort {
                                    file::Reliability::BestEffort
                                } else {
                                    file::Reliability::Reliable
                                },
                                durability: file::Durability::Volatile,
                            },
                        },
                        static_tf: file::TransformInput {
                            topic: config.static_tf_topic.clone(),
                            message_type: spatial::TF_MESSAGE_TYPE.into(),
                            qos: file::Qos {
                                reliability: file::Reliability::Reliable,
                                durability: file::Durability::TransientLocal,
                            },
                        },
                    };
                    let clock_expectation = if view.spatial_clock_verified {
                        file::ClockExpectation::System
                    } else {
                        view.workspace_clock_expectation
                    };
                    match view.configuration.output {
                        Output::PointCloud => file::ViewKind::PointCloud {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::OccupancyGrid => file::ViewKind::OccupancyGrid {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::Pose => file::ViewKind::Pose {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::Path => file::ViewKind::Path {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        _ => {
                            return Err(
                                "Scene overlay renderer contradicts its subscription output".into(),
                            )
                        }
                    }
                }
                ViewKind::Spatial => {
                    let config = view
                        .configuration
                        .spatial
                        .as_ref()
                        .ok_or("Spatial configuration is missing")?;
                    file::ViewKind::Spatial {
                        fixed_frame: config.fixed_frame.clone(),
                        layers: vec![file::SpatialLayerConfig {
                            id: view.workspace_scan_layer_id.clone(),
                            kind: file::LayerKind::LaserScan,
                            topic: view.configuration.topic.clone(),
                            message_type: view.configuration.message_type.clone(),
                            qos: model::qos(&view.configuration),
                        }],
                        transforms: Some(file::SpatialTransforms {
                            dynamic_tf: file::TransformInput {
                                topic: config.dynamic_tf_topic.clone(),
                                message_type: "tf2_msgs/msg/TFMessage".into(),
                                qos: file::Qos {
                                    reliability: if config.dynamic_tf_best_effort {
                                        file::Reliability::BestEffort
                                    } else {
                                        file::Reliability::Reliable
                                    },
                                    durability: file::Durability::Volatile,
                                },
                            },
                            static_tf: file::TransformInput {
                                topic: config.static_tf_topic.clone(),
                                message_type: "tf2_msgs/msg/TFMessage".into(),
                                qos: file::Qos {
                                    reliability: file::Reliability::Reliable,
                                    durability: file::Durability::TransientLocal,
                                },
                            },
                        }),
                        clock_expectation: if view.spatial_clock_verified {
                            file::ClockExpectation::System
                        } else {
                            view.workspace_clock_expectation
                        },
                    }
                }
            };
            views.push(file::ViewConfig {
                id: view.workspace_id.clone(),
                kind,
                topic: view.configuration.topic.clone(),
                message_type: view.configuration.message_type.clone(),
                qos: model::qos(&view.configuration),
            });
        }
        let ids = views.iter().map(|view| view.id.clone()).collect::<Vec<_>>();
        let snapshot = file::Workspace {
            tools: None,
            presentation: None,
            version: file::FORMAT_VERSION,
            name: self.workspace.name.clone(),
            panels: file::Panels {
                data_open: self.sidebar,
                inspector_open: self.inspector,
                diagnostics_open: self.diagnostics_open,
            },
            layout: model::reconcile_layout(&self.workspace.layout, &ids),
            views,
            connection_suggestion: self.workspace.connection_suggestion.clone(),
        };
        snapshot.validate().map_err(|error| error.to_string())?;
        Ok(snapshot)
    }
    fn workspace_observe(&mut self) -> Result<(), String> {
        let snapshot = self.workspace_snapshot()?;
        self.workspace.layout = snapshot.layout.clone();
        self.workspace.controller.observe(snapshot).map(|_| ())
    }
    /// Exact logical worker identities, including Starting/Stopping reservations.
    fn workspace_owners(&self) -> Vec<file::HandleIdentity> {
        self.session.as_ref().map_or_else(Vec::new, |session| {
            session
                .shared
                .lock()
                .unwrap()
                .registry
                .entries()
                .iter()
                .map(|entry| file::HandleIdentity {
                    generation: entry.generation,
                    subscription: entry.subscription,
                })
                .collect()
        })
    }
    fn workspace_prepare(
        &self,
        configuration: &file::Workspace,
    ) -> Result<PreparedWorkspace, String> {
        file::SupportedRenderers {
            map: false,
            uncertainty: false,
            camera: false,
            ..file::SupportedRenderers::all()
        }
        .validate_workspace(configuration)
        .map_err(|error| error.to_string())?;
        let mut next = self.next_view;
        let mut views = Vec::with_capacity(configuration.views.len());
        // File array order is document content; layout order is presentation.
        // Keeping them separate preserves exact saved snapshots after restore.
        for saved in &configuration.views {
            next = next
                .checked_add(1)
                .ok_or("View identities exhausted; current workspace retained")?;
            let mut view = ViewState::new(ViewId(next), model::configuration(saved)?);
            view.workspace_id = saved.id.clone();
            match &saved.kind {
                file::ViewKind::Camera { .. }
                | file::ViewKind::Uncertainty { .. }
                | file::ViewKind::Markers { .. } => {
                    return Err(
                        "Marker displays require the independent Visualizer application".into(),
                    )
                }
                file::ViewKind::Map { .. } => {
                    return Err("Map updates require the independent Visualizer".into())
                }
                file::ViewKind::RobotTopic { .. } => {
                    return Err("Topic Robot requires the independent Visualizer".into())
                }
                file::ViewKind::TfGraph { .. } => {
                    return Err("TF display requires the independent Visualizer".into())
                }
                file::ViewKind::Messages => view.kind = ViewKind::Messages,
                file::ViewKind::NumericPlot { .. } => {
                    view.kind =
                        ViewKind::Plot(saved.kind.plot_path().ok_or("Plot field path is missing")?)
                }
                file::ViewKind::Image { display_mode } => {
                    view.image.set_display_mode(match display_mode {
                        file::ImageMode::Fit => images::DisplayMode::Fit,
                        file::ImageMode::ActualSize => images::DisplayMode::ActualSize,
                    })
                }
                file::ViewKind::PointCloud {
                    clock_expectation, ..
                }
                | file::ViewKind::OccupancyGrid {
                    clock_expectation, ..
                }
                | file::ViewKind::Pose {
                    clock_expectation, ..
                }
                | file::ViewKind::Path {
                    clock_expectation, ..
                } => {
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                }
                file::ViewKind::Spatial {
                    layers,
                    clock_expectation,
                    ..
                } => {
                    view.workspace_scan_layer_id = layers[0].id.clone();
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                }
                file::ViewKind::Robot {
                    assets,
                    clock_expectation,
                    ..
                } => {
                    view.robot_assets = Some(assets.clone());
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                    view.robot_status =
                        "Restored inactive · Load and review the local model before enabling"
                            .into();
                }
            }
            views.push(view);
        }
        Ok(PreparedWorkspace {
            revision: self.workspace.controller.revision(),
            base_next_view: self.next_view,
            next_view: next,
            configuration: configuration.clone(),
            views,
        })
    }
    pub(super) fn workspace_begin_apply(
        &mut self,
        approval: file::ApplyApproval,
    ) -> Result<(), String> {
        if approval == file::ApplyApproval::Cancel {
            self.workspace.controller.cancel();
            self.workspace.prepared = None;
            return Ok(());
        }
        self.workspace_observe()?;
        let configuration = self
            .workspace
            .controller
            .candidate()
            .ok_or("Open or select a preset before replacing configuration")?
            .workspace
            .clone();
        // All mapping checks and ViewId allocation happen before the first Stop.
        let prepared = self.workspace_prepare(&configuration)?;
        let owners = self.workspace_owners();
        let intents = self.workspace.controller.begin_apply(approval, &owners)?;
        self.cancel_robot_setup();
        // Only an approved replacement abandons queued subscription replacements.
        for view in &mut self.views {
            view.pending = None;
        }
        self.workspace.prepared = Some(prepared);
        for intent in intents {
            let view = self
                .views
                .iter()
                .find(|view| {
                    view.generation == intent.owner.generation
                        && view.subscription == Some(intent.owner.subscription)
                })
                .map(|view| view.id);
            if let Some(view) = view {
                self.stop_view(view);
            } else {
                self.workspace.controller.stop_failed(
                    intent.owner,
                    "Active native owner has no matching panel; disconnect and review again",
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn workspace_review_configuration(
        &mut self,
        configuration: file::Workspace,
    ) -> Result<(), String> {
        self.workspace_observe()?;
        self.workspace
            .controller
            .preview_configuration(configuration)
    }
    /// Call after normal backend polling and before rendering; never auto-applies a Load.
    pub(super) fn workspace_poll(&mut self, ctx: &egui::Context) {
        if self.workspace.controller.take_exit_request() {
            // Native file choosers own AppKit key/menu events. Their bounded
            // exit request resumes this normal close path and on_exit cleanup.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if let Err(error) = self.workspace_observe() {
            self.diagnostic(error);
        }
        self.workspace.controller.poll();
        if self.workspace.controller.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if !self.workspace.controller.is_applying() {
            self.workspace.prepared = None;
            return;
        }
        let owners = self.workspace_owners();
        let Some(prepared) = self.workspace.prepared.as_ref() else {
            return;
        };
        if prepared.base_next_view != self.next_view
            || prepared.revision != self.workspace.controller.revision()
        {
            self.workspace.controller.cancel();
            self.workspace.prepared = None;
            self.diagnostic(
                "Workspace changed during replacement; review again. Current panels retained."
                    .into(),
            );
            return;
        }
        // Recheck full mapping without mutating current panels or consuming the token.
        if let Err(error) = self.workspace_prepare(&prepared.configuration) {
            self.workspace.controller.cancel();
            self.workspace.prepared = None;
            self.diagnostic(error);
            return;
        }
        match self.workspace.controller.take_ready(&owners) {
            Ok(Some(token)) => {
                let prepared = self
                    .workspace
                    .prepared
                    .take()
                    .expect("validated prepared workspace");
                let saved = token.configuration();
                debug_assert_eq!(saved, &prepared.configuration);
                self.views = prepared.views;
                self.next_view = prepared.next_view;
                self.selected = self.views.first().map(|view| view.id);
                self.workspace.name = saved.name.clone();
                self.workspace.layout = saved.layout.clone();
                self.workspace.connection_suggestion = saved.connection_suggestion.clone();
                self.sidebar = saved.panels.data_open;
                self.inspector = saved.panels.inspector_open;
                self.diagnostics_open = saved.panels.diagnostics_open;
                self.setup = false;
                self.setup_target = None;
                self.draft_clock_verified = false;
                // Domain/middleware and current discovery/session are deliberately unchanged.
            }
            Ok(None) => (),
            Err(error) => {
                self.workspace.prepared = None;
                self.diagnostic(error);
            }
        }
    }
    /// Only call after transport registry accepted authoritative cleanup for this identity.
    pub(super) fn workspace_cleanup_confirmed(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
    ) {
        if let Err(error) = self
            .workspace
            .controller
            .cleanup_confirmed(file::HandleIdentity {
                generation,
                subscription,
            })
        {
            self.diagnostic(error);
        }
    }
    pub(super) fn workspace_stop_failed(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        message: &str,
    ) {
        if let Err(error) = self.workspace.controller.stop_failed(
            file::HandleIdentity {
                generation,
                subscription,
            },
            message,
        ) {
            self.diagnostic(error);
        }
    }
    pub(super) fn workspace_disconnected(&mut self) {
        self.workspace.controller.disconnect();
        self.workspace.prepared = None;
    }
    pub(super) fn workspace_shutdown(&mut self) {
        self.workspace.controller.shutdown();
    }
    pub(super) fn workspace_ordered_ids(&self) -> Vec<ViewId> {
        let mut ordered: Vec<_> = self
            .workspace
            .layout
            .order
            .iter()
            .filter_map(|id| {
                self.views
                    .iter()
                    .find(|view| &view.workspace_id == id)
                    .map(|view| view.id)
            })
            .collect();
        for view in &self.views {
            if !ordered.contains(&view.id) {
                ordered.push(view.id);
            }
        }
        ordered
    }
    /// Call inside a root-owned workspace Window/panel; actions remain explicit.
    pub(super) fn workspace_render(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<&str> = None;
        ui.horizontal_wrapped(|ui| {
            studio_ui_foundation::theme::strong(ui, "Workspace");
            ui.label(if self.workspace.controller.dirty() {
                "Unsaved changes"
            } else {
                "Saved"
            });
            if ui
                .add_enabled(
                    self.workspace_file_available(),
                    egui::Button::new("Open workspace"),
                )
                .clicked()
            {
                action = Some("open");
            }
            if ui
                .add_enabled(
                    self.workspace_file_available(),
                    egui::Button::new("Save workspace"),
                )
                .clicked()
            {
                action = Some("save");
            }
            if ui
                .add_enabled(
                    self.workspace_file_available(),
                    egui::Button::new("Save workspace as…"),
                )
                .clicked()
            {
                action = Some("save_as");
            }
        });
        ui.label(self.workspace.controller.status());
        #[cfg(target_os = "macos")]
        ui.label("For keyboard quit, cancel the file chooser before pressing Cmd-Q.");
        if let Some(path) = self.workspace.controller.path() {
            workspace_path_ui(ui, path, "Saved workspace");
        }
        if let Some(error) = self.workspace.controller.error() {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.button("Acknowledge workspace error").clicked() {
                self.workspace.controller.acknowledge_error();
            }
        }
        if self.workspace.controller.is_busy() {
            ui.spinner();
            if ui.button("Cancel file operation").clicked() {
                action = Some("cancel");
            }
        }
        if ui
            .add_enabled(
                !self.workspace.controller.is_busy() && !self.workspace_frozen(),
                egui::Button::new("Restart file worker"),
            )
            .clicked()
        {
            action = Some("restart_worker");
        }
        ui.add_enabled_ui(!self.workspace_frozen(), |ui| {
            let label = ui.label("Workspace name");
            ui.add(studio_ui_foundation::theme::input(ui, &mut self.workspace.name))
                .labelled_by(label.id);
            ui.collapsing("Local file path", |ui| {
                if let Some(kind) = self.workspace.controller.fallback_kind() {
                    ui.label(match kind {
                        DialogKind::Open => "Enter a local workspace path to open",
                        DialogKind::Save => "Enter a local workspace path to save",
                    });
                }
                let label = ui.label("Workspace file path");
                ui.add(studio_ui_foundation::theme::input(ui, &mut self.workspace.path_entry))
                    .labelled_by(label.id);
                if ui.button("Open path").clicked() {
                    action = Some("load_path");
                }
                if ui.button("Save to path").clicked() {
                    action = Some("save_path");
                }
            });
            ui.collapsing("Inactive presets", |ui| {
                ui.small("Preview a preset, then explicitly replace configuration. Every restored view stays stopped.");
                for (label, key) in [("Inspect preset", "inspect"), ("Visualize preset", "visualize"), ("Analyze preset", "analyze")] {
                    if ui.button(label).clicked() { action = Some(key); }
                }
            });
        });
        if let Some(candidate) = self.workspace.controller.candidate() {
            ui.separator();
            studio_ui_foundation::theme::strong(ui, "Review replacement configuration");
            ui.small("Scene-first layout: older grid sizes, collapse and visibility settings are discarded. Display order and source settings are preserved; every display starts disabled.");
            if let Some(path) = candidate.path.as_deref() {
                workspace_path_ui(ui, path, "Preview");
            }
            ui.label(format!(
                "{} · {} panels · all subscriptions will remain stopped",
                candidate.workspace.name,
                candidate.workspace.views.len()
            ));
            if let Some(issue) = &candidate.issue {
                ui.colored_label(ui.visuals().error_fg_color, issue);
            }
            for view in &candidate.workspace.views {
                ui.label(format!(
                    "{} · {}",
                    view.source_label(),
                    match &view.kind {
                        file::ViewKind::TfGraph { .. } => "TF frames (Visualizer only)",
                        file::ViewKind::Messages => "Messages",
                        file::ViewKind::NumericPlot { .. } => "Numeric plot",
                        file::ViewKind::Image { .. } => "Image",
                        file::ViewKind::Spatial { .. } => "Spatial scan",
                        file::ViewKind::RobotTopic { .. } => "Topic Robot (Visualizer only)",
                        file::ViewKind::Robot { .. } => "Robot model",
                        file::ViewKind::PointCloud { .. } => "Point cloud",
                        file::ViewKind::Map { .. } => "Map (Visualizer only)",
                        file::ViewKind::OccupancyGrid { .. } => "Occupancy map",
                        file::ViewKind::Pose { .. } => "Pose",
                        file::ViewKind::Path { .. } => "Path",
                        file::ViewKind::Markers { .. } => "Markers (Visualizer only)",
                        file::ViewKind::Uncertainty { .. } => "Pose uncertainty (Visualizer only)",
                        file::ViewKind::Camera { .. } => "Calibrated Camera (Visualizer only)",
                    }
                ));
            }
            ui.collapsing("File contents", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("workspace-preview")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    serde_json::to_string_pretty(&candidate.workspace)
                                        .unwrap_or_else(|_| "Preview unavailable".into()),
                                )
                                .monospace(),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
            });
            ui.label("Connection suggestions are advisory. Domain and middleware will remain unchanged. Spatial clock expectations require fresh verification.");
            if self.workspace.controller.dirty() {
                ui.label("Save current changes, explicitly discard them, or cancel replacement.");
                if ui.button("Save current changes").clicked() {
                    action = Some("save");
                }
            }
            if ui.button("Review candidate again").clicked() {
                action = Some("review");
            }
            let ready = candidate.preview.is_some()
                && !self.workspace.controller.is_busy()
                && !self.workspace_frozen();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(if self.workspace.controller.dirty() {
                        "Discard unsaved changes and replace"
                    } else {
                        "Replace with inactive workspace"
                    }),
                )
                .clicked()
            {
                action = Some("apply");
            }
            if ui.button("Cancel replacement").clicked() {
                action = Some("cancel");
            }
        }
        if let file::RestoreState::Stopping {
            pending, failures, ..
        } = self.workspace.controller.restore_state()
        {
            ui.label(format!(
                "Waiting for {} native worker cleanup acknowledgements",
                pending.len()
            ));
            for failure in failures {
                ui.colored_label(ui.visuals().error_fg_color, &failure.message);
            }
            if !failures.is_empty() && ui.button("Retry failed workspace stops").clicked() {
                action = Some("retry_stops");
            }
        }
        let result = match action {
            Some("open") => self.workspace_file(FileAction::Open),
            Some("save") => self.workspace_file(FileAction::Save),
            Some("save_as") => self.workspace_file(FileAction::SaveAs),
            Some("load_path") => {
                let path = self.workspace.path_entry.clone();
                self.workspace_observe()
                    .and_then(|_| self.workspace.controller.load_path(path))
            }
            Some("save_path") => {
                let path = self.workspace.path_entry.clone();
                self.workspace_observe()
                    .and_then(|_| self.workspace.controller.save_path(path))
            }
            Some("review") => self
                .workspace_observe()
                .and_then(|_| self.workspace.controller.review_candidate()),
            Some("apply") => self.workspace_begin_apply(if self.workspace.controller.dirty() {
                file::ApplyApproval::DiscardUnsaved
            } else {
                file::ApplyApproval::CleanOrSaved
            }),
            Some("cancel") => {
                self.workspace.controller.cancel();
                self.workspace.prepared = None;
                Ok(())
            }
            Some("retry_stops") => self.workspace_retry_stops(),
            Some("restart_worker") => self.workspace.controller.restart_file_worker(),
            Some(preset @ ("inspect" | "visualize" | "analyze")) => {
                let preset = match preset {
                    "inspect" => file::Preset::Inspect,
                    "visualize" => file::Preset::Visualize,
                    _ => file::Preset::Analyze,
                };
                self.workspace_review_configuration(preset.workspace())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.diagnostic(error);
        }
    }
    fn workspace_retry_stops(&mut self) -> Result<(), String> {
        let intents = self.workspace.controller.retry_failed()?;
        for intent in intents {
            if let Some(id) = self
                .views
                .iter()
                .find(|view| {
                    view.generation == intent.owner.generation
                        && view.subscription == Some(intent.owner.subscription)
                })
                .map(|view| view.id)
            {
                self.stop_view(id);
            } else {
                self.workspace.controller.stop_failed(
                    intent.owner,
                    "Native owner no longer matches a view; disconnect and review again",
                )?;
            }
        }
        Ok(())
    }
}

fn workspace_path_ui(ui: &mut egui::Ui, path: &std::path::Path, scope: &str) {
    let full = path.to_string_lossy();
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    ui.add(egui::Label::new(name.as_ref()).truncate())
        .on_hover_text(full.as_ref());
    egui::CollapsingHeader::new(format!("{scope} full path")).id_salt(scope).show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(full.as_ref()).monospace())
                .selectable(true)
                .wrap(),
        );
        let exact = path.to_str();
        if exact.is_none() {
            ui.label("This path contains non-Unicode bytes. File operations preserve the original bytes; an exact text path cannot be copied.");
        }
        if ui.add_enabled(exact.is_some(), egui::Button::new(format!("Copy {scope} path"))).clicked() {
            if let Some(exact) = exact { ui.ctx().copy_text(exact.to_owned()); }
        }
    });
}

#[cfg(test)]
#[path = "workspace_app/tests.rs"]
mod tests;
