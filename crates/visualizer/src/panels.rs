//! Persistent navigation and contextual panels. Browsing never issues ROS work.
use super::*;

pub(super) fn backend_identity(
    override_path: Option<&std::ffi::OsStr>,
    actual: Option<&(String, String)>,
) -> (&'static str, String) {
    if actual.is_some_and(|(_, rmw)| rmw.contains("synthetic"))
        || override_path.is_some_and(|path| {
            std::path::Path::new(path)
                .file_name()
                .is_some_and(|name| name == "ui-workspace-demo")
        })
    {
        return ("Demo · synthetic", "Synthetic protocol data. This backend cannot discover a robot, regardless of network settings.".into());
    }
    if let Some((distro, rmw)) = actual {
        return ("ROS 2", format!("{distro} / {rmw}. Topics may originate from robots or simulators; verify the publisher."));
    }
    if override_path.is_some() {
        return (
            "Custom backend",
            "A development backend override is configured. Verify its source before connecting."
                .into(),
        );
    }
    ("ROS 2", "Uses the ROS backend. Choose the robot’s domain, middleware and network, then Connect to discover topics.".into())
}

#[derive(Clone, Copy)]
pub(super) enum AttentionTarget {
    View(ViewId),
    Scene,
    Workspace,
    Connection,
    Topics,
    RobotSetup,
    MeshAssets,
}
pub(super) struct Attention {
    pub target: AttentionTarget,
    pub message: String,
}

pub(super) fn source_type_label(view: &ViewState) -> &str {
    if matches!(view.kind, ViewKind::Robot) {
        view.robot_description
            .as_ref()
            .map_or("Local URDF file", |source| source.message_type.as_str())
    } else {
        &view.configuration.message_type
    }
}

pub(super) fn view_feedback(view: &ViewState) -> String {
    if let Some(error) = view
        .error
        .as_ref()
        .or(view.sensor_appearance_error.as_ref())
        .or(view.robot_render_failure.as_ref())
        .or(view.robot_appearance_view.error.as_ref())
    {
        return error.clone();
    }
    if view.status == ViewStatus::Waiting {
        if matches!(view.kind, ViewKind::Robot) && !view.robot_status.is_empty() {
            return view.robot_status.clone();
        }
        if !view.spatial_status.is_empty() && view.spatial_status != "Stopped" {
            return view.spatial_status.clone();
        }
        return "Waiting for usable data. Review source, publisher and requested QoS.".into();
    }
    view.status_label().into()
}

impl App {
    pub(super) fn current_attention(&self) -> Vec<Attention> {
        let mut items = Vec::new();
        if self.connection.phase == Phase::Failed {
            items.push(Attention {
                target: AttentionTarget::Connection,
                message: self.connection.status.clone(),
            });
        }
        if self.workspace_needs_attention() {
            items.push(Attention {
                target: AttentionTarget::Workspace,
                message:
                    "Workspace operation needs attention. Review full file error and recovery."
                        .into(),
            });
        }
        if let Some(error) = &self.scene_settings.error {
            items.push(Attention {
                target: AttentionTarget::Scene,
                message: error.clone(),
            });
        }
        if let Some(error) = &self.topic_action_error {
            items.push(Attention {
                target: AttentionTarget::Topics,
                message: error.clone(),
            });
        }
        if let Some(error) = self.robot_setup.current_error() {
            items.push(Attention {
                target: AttentionTarget::RobotSetup,
                message: format!("Robot setup: {error}"),
            });
        }
        if let Some(error) = self
            .markers
            .current_asset_error()
            .or(self.markers.error.as_deref())
        {
            items.push(Attention {
                target: AttentionTarget::MeshAssets,
                message: format!("Marker meshes: {error}"),
            });
        }
        for view in &self.views {
            if view.error.is_some()
                || view.sensor_appearance_error.is_some()
                || view.robot_render_failure.is_some()
                || view.robot_appearance_view.error.is_some()
                || matches!(view.status, ViewStatus::NeedsRecovery | ViewStatus::Waiting)
            {
                items.push(Attention {
                    target: AttentionTarget::View(view.id),
                    message: format!("{}: {}", view.source_label(), view_feedback(view)),
                });
            }
        }
        items
    }
    pub(super) fn contextual_inspector_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        // Keep the legacy workspace field for round trips; Views is now permanent.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        if ui.ctx().screen_rect().width() >= 1100.0 {
            studio_ui_foundation::theme::pane_heading(ui, "Views");
            ui.add_enabled_ui(!frozen, |ui| self.shared_scene.controls_ui(ui));
            ui.add_space(8.0);
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .id_salt("contextual-inspector-body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                if self.setup || self.catalog_open {
                    if ui
                        .add_enabled(!frozen, egui::Button::new("Back to topics"))
                        .clicked()
                    {
                        self.back_to_topics();
                    }
                    if self.setup {
                        ui.add_enabled_ui(!frozen, |ui| self.setup_ui(ui));
                    } else {
                        ui.add_enabled_ui(!frozen, |ui| self.catalog_ui(ui));
                    }
                } else if self.details_open {
                    if let Some(id) = self.selected {
                        ui.push_id(id.0, |ui| self.display_inspector_ui(ui, frozen, id));
                    } else {
                        studio_ui_foundation::theme::strong(ui, "Display");
                        ui.label("Select a topic to inspect its display settings.");
                        ui.small("Use its eye to show or hide it.");
                    }
                }
                crate::ui::reveal_focused_control(ui);
            });
    }
    pub(super) fn attention_window_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.attention_open;
        let mut target = None;
        egui::Window::new("Needs attention")
            .open(&mut open)
            .default_size(egui::vec2(540.0, 320.0))
            .max_size(
                (ctx.screen_rect().size() - egui::vec2(32.0, 32.0)).max(egui::vec2(300.0, 240.0)),
            )
            .show(ctx, |ui| {
                ui.label(
                    "Current waiting and recovery states. Recorded diagnostic history is separate.",
                );
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let items = self.current_attention();
                    if items.is_empty() {
                        ui.label("No current problems.");
                    }
                    for item in items {
                        ui.separator();
                        ui.add(egui::Label::new(item.message).selectable(true).wrap());
                        if ui.button("Review cause").clicked() {
                            target = Some(item.target);
                        }
                    }
                    crate::ui::reveal_focused_control(ui);
                });
            });
        self.attention_open = open;
        if let Some(target) = target {
            self.reveal_attention_target(target);
        }
    }
    pub(super) fn reveal_attention_target(&mut self, target: AttentionTarget) {
        match target {
            AttentionTarget::View(id) => self.select_display(id),
            AttentionTarget::Scene => self.open_scene_settings(),
            AttentionTarget::Workspace => self.workspace.open = true,
            AttentionTarget::Connection => self.diagnostics_open = true,
            AttentionTarget::Topics => {
                self.sidebar = true;
                self.topics_tab = true;
                self.global_options_open = false;
            }
            AttentionTarget::RobotSetup => {
                self.details_open = true;
                self.compact_inspector_open = true;
                self.setup = self.robot_setup.is_open();
            }
            AttentionTarget::MeshAssets => self.markers.reveal_assets(),
        }
        self.attention_open = false;
    }
}
