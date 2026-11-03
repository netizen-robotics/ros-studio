//! Workspace document, lifecycle and layout integration for the native App.
use super::*;
use crate::components::{workspace as file, workspace_document::Controller};
#[path = "workspace_app/model.rs"]
mod model;
#[cfg(test)]
mod tests;
mod ui;

pub(super) struct WorkspaceUi {
    pub open: bool,
    pub name: String,
    pub layout: file::Layout,
    pub connection_suggestion: Option<file::ConnectionSuggestion>,
    pub path_entry: String,
    recovery_error: Option<String>,
    last_rendered_error: Option<String>,
    pub(crate) focus_path: bool,
    restored_notice: bool,
    controller: Controller,
    prepared: Option<PreparedWorkspace>,
}
pub(super) struct PreparedWorkspace {
    revision: u64,
    base_next_view: u64,
    next_view: u64,
    configuration: file::Workspace,
    pub(super) views: Vec<ViewState>,
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
            recovery_error: None,
            last_rendered_error: None,
            focus_path: false,
            restored_notice: false,
            controller: Controller::for_application(
                empty,
                file::SupportedRenderers {
                    messages: false,
                    numeric_plot: false,
                    ..file::SupportedRenderers::all()
                },
                studio_workspace::application::Application::Visualizer,
            )
            .expect("valid empty workspace"),
            prepared: None,
        }
    }
}
impl App {
    pub(super) fn workspace_window_title(&self) -> String {
        let filename = self
            .workspace
            .controller
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled workspace".into());
        format!(
            "{filename}{} — ROS Studio Visualizer",
            if self.workspace_dirty() { " *" } else { "" }
        )
    }
    pub(super) fn workspace_needs_attention(&self) -> bool {
        self.workspace_error().is_some()
    }
    pub(crate) fn workspace_error(&self) -> Option<&str> {
        self.workspace
            .recovery_error
            .as_deref()
            .or(self.workspace.controller.error())
    }
    pub(super) fn workspace_dirty(&self) -> bool {
        self.workspace.controller.dirty()
    }
    pub(super) fn workspace_file_available(&self) -> bool {
        !self.workspace.controller.is_busy() && !self.workspace_frozen()
    }
    pub(super) fn workspace_file(&mut self, action: FileAction) -> Result<(), String> {
        self.workspace.restored_notice = false;
        let result = self.workspace_observe().and_then(|_| match action {
            FileAction::Open => self.workspace.controller.open(),
            FileAction::Save => self.workspace.controller.save(),
            FileAction::SaveAs => self.workspace.controller.save_as(),
        });
        match &result {
            Ok(()) => {
                self.workspace.controller.acknowledge_error();
                self.workspace.recovery_error = None;
            }
            Err(error) => self.workspace.recovery_error = Some(error.clone()),
        }
        result
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
            if !self.workspace.layout.order.contains(&view.workspace_id) {
                self.workspace.layout.order.push(view.workspace_id.clone());
            }
        }
    }
    pub(super) fn workspace_frozen(&self) -> bool {
        self.workspace.controller.is_applying() || self.scene_settings.pending.is_some()
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
                    let transforms = file::SpatialTransforms {
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
                    };
                    if let Some(description) = &view.robot_description {
                        file::ViewKind::RobotTopic {
                            description: description.clone(),
                            fixed_frame: config.fixed_frame.clone(),
                            transforms,
                            clock_expectation: view.workspace_clock_expectation,
                        }
                    } else {
                        file::ViewKind::Robot {
                            assets: view
                                .robot_assets
                                .clone()
                                .ok_or("Robot asset locations are missing")?,
                            fixed_frame: config.fixed_frame.clone(),
                            transforms,
                            clock_expectation: view.workspace_clock_expectation,
                        }
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
                ViewKind::TfGraph
                | ViewKind::Markers
                | ViewKind::Uncertainty(_)
                | ViewKind::Camera
                | ViewKind::PointCloud
                | ViewKind::Mapping(_) => {
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
                    match view.configuration.output.clone() {
                        Output::CameraSnapshot(info) => file::ViewKind::Camera {
                            info: *info,
                            appearance: view.camera.appearance,
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::PoseSnapshot(configuration) => file::ViewKind::Uncertainty {
                            configuration,
                            appearance: view.uncertainty.appearance,
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::MarkerSnapshot => file::ViewKind::Markers {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::TfGraph => file::ViewKind::TfGraph {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::PointCloud => file::ViewKind::PointCloud {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                        },
                        Output::MapSnapshot => file::ViewKind::Map {
                            fixed_frame,
                            transforms,
                            clock_expectation,
                            appearance: view.map.appearance,
                            updates: view.configuration.map_updates.as_ref().map(|source| {
                                studio_workspace::map::UpdateSource {
                                    topic: source.topic.clone(),
                                    message_type: studio_protocol::map_grid::UPDATE_MESSAGE_TYPE
                                        .into(),
                                    qos: file::Qos {
                                        reliability: if source.best_effort {
                                            file::Reliability::BestEffort
                                        } else {
                                            file::Reliability::Reliable
                                        },
                                        durability: if source.transient_local {
                                            file::Durability::TransientLocal
                                        } else {
                                            file::Durability::Volatile
                                        },
                                    },
                                }
                            }),
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
            presentation: Some(self.presentation_snapshot()),
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
    pub(super) fn workspace_prepare(
        &self,
        configuration: &file::Workspace,
    ) -> Result<PreparedWorkspace, String> {
        if configuration
            .views
            .iter()
            .any(|v| !studio_workspace::application::Application::Visualizer.accepts(&v.kind))
        {
            return Err("Workspace contains displays belonging to the other application".into());
        }
        file::SupportedRenderers::all()
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
                file::ViewKind::Map {
                    appearance,
                    clock_expectation,
                    ..
                } => {
                    view.map.appearance = *appearance;
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
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
                file::ViewKind::Camera {
                    appearance,
                    clock_expectation,
                    ..
                } => {
                    view.camera.appearance = *appearance;
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                }
                file::ViewKind::Uncertainty {
                    appearance,
                    clock_expectation,
                    ..
                } => {
                    view.uncertainty.appearance = *appearance;
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                }
                file::ViewKind::Markers {
                    clock_expectation, ..
                }
                | file::ViewKind::TfGraph {
                    clock_expectation, ..
                }
                | file::ViewKind::PointCloud {
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
                file::ViewKind::RobotTopic {
                    description,
                    clock_expectation,
                    ..
                } => {
                    view.robot_description = Some(description.clone());
                    view.workspace_clock_expectation = *clock_expectation;
                    view.spatial_clock_verified = false;
                    view.robot_status =
                        "Restored inactive · Read and review the description before enabling"
                            .into();
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
            if let Some(settings) = configuration
                .presentation
                .as_ref()
                .and_then(|p| p.robots.iter().find(|robot| robot.view == saved.id))
            {
                view.robot_appearance = settings.appearance.clone();
            }
            if let Some(settings) = configuration
                .presentation
                .as_ref()
                .and_then(|p| p.sensors.iter().find(|s| s.view == saved.id))
            {
                view.sensor_appearance = settings.appearance;
                view.sensor_appearance_explicit = true;
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
            if self.health.owner() == Some((intent.owner.generation, intent.owner.subscription)) {
                if let Some(session) = &self.session {
                    self.health.stop(session);
                }
            } else if let Some(view) = view {
                self.stop_view(view);
            } else if self.robot_capture_stop_owner(
                intent.owner.generation,
                intent.owner.subscription,
                false,
            ) {
                // Temporary captures share the exact replacement cleanup barrier.
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
        let title = self.workspace_window_title();
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
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
                self.restore_scene_settings();
                // The prepared token has already validated presentation before
                // any current owners were stopped. Restore never authorizes ROS.
                self.restore_presentation(saved.presentation.as_ref())
                    .expect("validated presentation in restore token");
                self.catalog_open = false;
                self.next_view = prepared.next_view;
                self.selected = self.views.first().map(|view| view.id);
                self.workspace.name = saved.name.clone();
                self.workspace.layout = saved.layout.clone();
                self.workspace.connection_suggestion = saved.connection_suggestion.clone();
                self.workspace.restored_notice = true;
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
        self.workspace.restored_notice = false;
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
            } else if self.robot_capture_stop_owner(
                intent.owner.generation,
                intent.owner.subscription,
                true,
            ) {
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
