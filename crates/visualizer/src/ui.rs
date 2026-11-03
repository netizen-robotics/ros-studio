use super::*;

pub(super) fn describe_choice(response: &egui::Response, label: &str, value: &str) {
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, response.enabled(), label);
        info.current_text_value = Some(value.into());
        info
    });
}

impl App {
    pub(super) fn render(&mut self, ctx: &egui::Context) {
        let narrow = ctx.screen_rect().width() < 1100.0;
        let mut compact_inspector_top = 240.0;
        let frozen = self.workspace_frozen();
        self.header_ui(ctx, frozen);
        self.network.panel(
            ctx,
            frozen || matches!(self.connection.phase, Phase::Starting | Phase::Connected),
        );
        self.health_ui(ctx, frozen);
        if self.sidebar {
            egui::SidePanel::left("resources")
                .default_width(310.0)
                .frame(
                    egui::Frame::new()
                        .fill(shell::palette(&ctx.style().visuals).subtle)
                        .inner_margin(shell::PADDING),
                )
                .width_range(260.0..=360.0)
                .show(ctx, |ui| {
                    self.displays_sidebar_ui(ui, frozen);
                });
        }
        if !narrow && self.details_open {
            egui::SidePanel::right("inspector")
                .frame(
                    egui::Frame::new()
                        .fill(shell::palette(&ctx.style().visuals).subtle)
                        .inner_margin(shell::PADDING),
                )
                .exact_width(320.0)
                .resizable(false)
                .show(ctx, |ui| {
                    // Some specialist widgets report their intrinsic long-value
                    // width. A two-axis viewport contains it rather than letting
                    // that minimum expand the side panel and shrink the scene.
                    egui::ScrollArea::horizontal()
                        .id_salt("inspector-width-boundary")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.contextual_inspector_ui(ui, frozen);
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(shell::palette(&ctx.style().visuals).surface)
                    .inner_margin(shell::PADDING),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    shell::pane_heading(ui, "Visualizer");
                    if narrow && ui.button("Display details").clicked() {
                        self.compact_inspector_open = !self.compact_inspector_open;
                        if self.compact_inspector_open {
                            self.details_open = true;
                        }
                    }
                    let frame = &self.scene_settings.configuration.fixed_frame;
                    ui.add(
                        egui::Label::new(format!(
                            "Frame: {}",
                            if frame.is_empty() {
                                "not configured"
                            } else {
                                frame
                            }
                        ))
                        .truncate(),
                    )
                    .on_hover_text(frame);
                    if !self.sidebar && ui.button("Displays").clicked() {
                        self.sidebar = true;
                        self.inspector = false;
                    }
                });
                if narrow {
                    let controls =
                        ui.add_enabled_ui(!frozen, |ui| self.shared_scene.compact_controls_ui(ui));
                    compact_inspector_top = controls.response.rect.bottom() + 12.0;
                }
                ui.separator();
                self.workspace_views_ui(ui, frozen);
            });
        if narrow && self.compact_inspector_open {
            let mut open = self.compact_inspector_open;
            egui::Window::new("Display inspector")
                .open(&mut open)
                .fixed_pos(egui::pos2(
                    (ctx.screen_rect().width() - 416.0).max(8.0),
                    compact_inspector_top,
                ))
                .constrain(false)
                .fixed_size(egui::vec2(
                    400.0,
                    (ctx.screen_rect().height() - compact_inspector_top - 100.0)
                        .clamp(120.0, 420.0),
                ))
                .show(ctx, |ui| {
                    egui::ScrollArea::horizontal()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.contextual_inspector_ui(ui, frozen);
                        });
                });
            self.compact_inspector_open = open;
        }
        self.attention_window_ui(ctx);
        self.diagnostics_window_ui(ctx);
        let mut files_open = self.workspace.open;
        egui::Window::new("Workspace files")
            .collapsible(false)
            .open(&mut files_open)
            .default_width(640.0)
            .default_height(560.0)
            .min_size(egui::vec2(360.0, 400.0))
            .max_size(
                (ctx.screen_rect().size() - egui::vec2(32.0, 32.0)).max(egui::vec2(360.0, 400.0)),
            )
            .resizable(true)
            .show(ctx, |ui| {
                self.workspace_render(ui);
            });
        self.workspace.open = files_open && self.workspace.open;
        self.marker_asset_window_ui(ctx, frozen);
        self.help_window_ui(ctx);
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    pub(super) fn back_to_topics(&mut self) {
        self.topics_tab = true;
        self.global_options_open = false;
        self.cancel_robot_setup();
        self.setup = false;
        self.setup_target = None;
        self.catalog_open = false;
    }
    fn health_ui(&mut self, ctx: &egui::Context, frozen: bool) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(shell::CONTROL_HEIGHT + 8.0)
            .frame(shell::strip_frame(&ctx.style().visuals))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    self.robot_capture_health_ui(ui);
                    let receiving = self
                        .views
                        .iter()
                        .filter(|v| v.status == ViewStatus::Receiving)
                        .count();
                    let drops = self.views.iter().fold(0u64, |sum, v| {
                        sum.saturating_add(v.queue_drops)
                            .saturating_add(v.received.backend_drops)
                    });
                    let summary = format!(
                        "{} · {receiving} receiving · {drops} application drops",
                        self.connection.status
                    );
                    let width = (ui.available_width() - 324.0).max(80.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, shell::CONTROL_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&summary).small()).truncate(),
                            )
                            .on_hover_text(&summary);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !frozen,
                                egui::Button::new("Diagnostics")
                                    .min_size(egui::vec2(132.0, shell::CONTROL_HEIGHT)),
                            )
                            .clicked()
                        {
                            self.diagnostics_open = true;
                        }
                        if ui
                            .add_sized(
                                [176.0, shell::CONTROL_HEIGHT],
                                shell::quiet(&format!(
                                    "Needs attention ({})",
                                    self.current_attention().len()
                                )),
                            )
                            .clicked()
                        {
                            self.attention_open = true;
                        }
                    });
                });
            });
    }
    fn header_ui(&mut self, ctx: &egui::Context, frozen: bool) {
        let mut file_action = None;
        egui::TopBottomPanel::top("connection")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin {
                        left: shell::PADDING,
                        right: shell::PADDING,
                        top: 4,
                        bottom: 4,
                    }),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = shell::COMPACT_HEIGHT;
                    egui::menu::bar(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            for (label, action) in [
                                ("Open workspace…", workspace_app::FileAction::Open),
                                ("Save workspace", workspace_app::FileAction::Save),
                                ("Save workspace as…", workspace_app::FileAction::SaveAs),
                            ] {
                                if ui
                                    .add_enabled(
                                        self.workspace_file_available(),
                                        egui::Button::new(label),
                                    )
                                    .clicked()
                                {
                                    file_action = Some(action);
                                    self.workspace.open = true;
                                    ui.close_menu();
                                }
                            }
                            if ui.button("Manage workspace…").clicked() {
                                self.workspace.open = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            shell::exit_menu_item(ui);
                        });
                        ui.menu_button("View", |ui| {
                            ui.checkbox(&mut self.sidebar, "Left sidebar");
                            if ui
                                .checkbox(&mut self.details_open, "Right sidebar")
                                .changed()
                            {
                                self.compact_inspector_open = self.details_open;
                            }
                        });
                        ui.menu_button("Help", |ui| {
                            if ui.button("Visualizer guide…").clicked() {
                                self.help.open = true;
                                ui.close_menu();
                            }
                            if ui.button("About ROS Studio Visualizer").clicked() {
                                self.about = true;
                                ui.close_menu();
                            }
                        });
                    });
                });
                ui.separator();
                shell::appearance_toolbar(ui, |ui| {
                    self.connection_header_ui(ui, frozen);
                });
                if self.connection.phase == Phase::Failed {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&self.connection.status)
                                .color(ui.visuals().error_fg_color),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                }
            });
        if let Some(action) = file_action {
            if let Err(error) = self.workspace_file(action) {
                self.diagnostic(error);
            }
        }
        egui::Window::new("About ROS Studio Visualizer")
            .open(&mut self.about)
            .resizable(false)
            .show(ctx, |ui| {
                studio_ui_foundation::theme::strong(ui, concat!("ROS Studio Visualizer ", env!("CARGO_PKG_VERSION")));
                ui.label("View ROS 2 topics and spatial data. This app does not send robot commands.");
                ui.label("Open Help > Visualizer guide for viewing, workspace and recovery guidance.");
                ui.collapsing("Font licenses", |ui| {
                    ui.label("Noto Sans SC and Noto Emoji, SIL Open Font License 1.1. Full notices below.");
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.label(fonts::NOTICE);
                        });
                });
            });
    }
    fn connection_header_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        // Reserve geometry for every state, rather than sizing to today's text.
        // Progress follows the action, so it never pushes its target sideways.
        const HEIGHT: f32 = shell::CONTROL_HEIGHT;
        const ACTION_WIDTH: f32 = shell::CONNECTION_ACTION_WIDTH;
        const PROGRESS_WIDTH: f32 = 144.0;
        shell::connection_fields(
            ui,
            "connection-middleware",
            &mut self.connection.domain,
            &mut self.connection.cyclone,
            !frozen && matches!(self.connection.phase, Phase::Disconnected | Phase::Failed),
        );
        let button = match self.connection.phase {
            Phase::Starting => egui::Button::new("Cancel startup"),
            Phase::Connected => shell::disconnect(ui.visuals(), "Disconnect"),
            _ => shell::primary(ui.visuals(), "Connect"),
        };
        let available =
            !frozen || matches!(self.connection.phase, Phase::Starting | Phase::Connected);
        let action = shell::action_slot(ui, available, ACTION_WIDTH, button);
        studio_ui_foundation::diagnostics::response(
            ui.ctx(),
            if self.connection.phase == Phase::Connected {
                "Disconnect"
            } else {
                "Connect"
            },
            &action,
        );
        if action.clicked() {
            if matches!(self.connection.phase, Phase::Starting | Phase::Connected) {
                self.stop();
            } else {
                self.connect();
            }
        }
        self.network.control(ui);
        ui.allocate_ui_with_layout(
            egui::vec2(PROGRESS_WIDTH, HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(egui::vec2(PROGRESS_WIDTH, HEIGHT));
                if self.connection.phase == Phase::Starting {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.add(egui::Label::new("Connecting…").truncate());
                } else {
                    let backend = std::env::var_os("STUDIO_BACKEND");
                    let (label, help) = crate::panels::backend_identity(
                        backend.as_deref(),
                        self.connection.actual.as_ref(),
                    );
                    ui.small(label).on_hover_text(help);
                }
            },
        );
    }
    pub(super) fn display_inspector_ui(&mut self, ui: &mut egui::Ui, frozen: bool, id: ViewId) {
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        let topic = self.views[index].source_label().to_owned();
        let ty = crate::panels::source_type_label(&self.views[index]).to_owned();
        let current = self.views[index].configuration.output.clone();
        let mut chosen = None;
        egui::ComboBox::from_id_salt(("topic-display-mode", id.0))
            .selected_text(self.views[index].kind.label())
            .show_ui(ui, |ui| {
                for descriptor in studio_visualization::catalog::DISPLAYS.iter().filter(|d| {
                    d.message_type == Some(ty.as_str())
                        || (d.output == Output::Robot && ty == "std_msgs/msg/String")
                }) {
                    if ui
                        .selectable_label(
                            std::mem::discriminant(&current)
                                == std::mem::discriminant(&descriptor.output),
                            descriptor.label,
                        )
                        .clicked()
                    {
                        chosen = Some(descriptor.output.clone());
                    }
                }
            });
        if let Some(output) = chosen {
            if std::mem::discriminant(&output) != std::mem::discriminant(&current) {
                self.change_topic_mode(output, &topic, &ty);
                return;
            }
        }
        let view = &mut self.views[index];
        studio_ui_foundation::theme::strong(ui, view.kind.label());
        ui.add(
            egui::Label::new(view.source_label())
                .selectable(true)
                .wrap(),
        )
        .on_hover_text(crate::panels::source_type_label(view));
        let feedback = crate::panels::view_feedback(view);
        ui.add(
            egui::Label::new(
                egui::RichText::new(feedback).color(if view.error.is_some() {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().text_color()
                }),
            )
            .selectable(true)
            .wrap(),
        );
        if view.status == ViewStatus::NeedsRecovery {
            ui.label("Reception is stopped. Retry cleanup, or disconnect before trying again.");
        }
        let mut configure = false;
        let mut retry = false;
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            configure = ui
                .add_enabled(
                    !frozen
                        && !matches!(
                            view.status,
                            ViewStatus::Stopping | ViewStatus::NeedsRecovery
                        ),
                    egui::Button::new("Configure…"),
                )
                .clicked();
            if view.status == ViewStatus::NeedsRecovery {
                retry = ui
                    .add_enabled(!frozen, egui::Button::new("Retry cleanup"))
                    .clicked();
            }
            remove = ui
                .add_enabled(
                    !frozen && view.subscription.is_none(),
                    egui::Button::new("Remove display"),
                )
                .clicked();
        });
        if view.configuration.output == Output::MapSnapshot {
            ui.add_enabled_ui(!frozen, |ui| {
                spatial::map_appearance_view::show(ui, &mut view.map.appearance);
            });
            crate::map_display::diagnostics(ui, view);
        }
        if matches!(view.kind, ViewKind::TfGraph) {
            view.tf_graph.show_inspector(
                ui,
                view.enabled && (view.status.active() || view.status == ViewStatus::Starting),
            );
        }
        if matches!(view.kind, ViewKind::Robot) {
            let mut candidate = view.robot_appearance.clone();
            let mut changed = false;
            ui.add_enabled_ui(!frozen, |ui| {
                changed = view.robot_appearance_view.show(
                    ui,
                    &mut candidate,
                    view.robot
                        .as_ref()
                        .map(|live| live.loaded().prepared.as_ref()),
                );
            });
            if changed {
                let result = view.set_robot_appearance(candidate);
                match result {
                    Ok(()) => {
                        view.robot_appearance_view.error = None;
                    }
                    Err(error) => view.robot_appearance_view.error = Some(error),
                }
            }
        }
        if matches!(view.kind, ViewKind::Spatial | ViewKind::PointCloud) {
            let mut candidate = view.sensor_appearance;
            let mut action = spatial::sensor_appearance_view::AppearanceAction::default();
            ui.push_id(("sensor-appearance", id.0), |ui| {
                ui.add_enabled_ui(!frozen, |ui| {
                    action = spatial::sensor_appearance_view::show(
                        ui,
                        &mut candidate,
                        &view.sensor_history,
                    );
                });
            });
            if action.changed {
                view.sensor_appearance_error = view.set_sensor_appearance(candidate).err();
            }
            if action.clear {
                view.sensor_appearance_error = view.clear_sensor().err();
            }
            if let Some(error) = &view.sensor_appearance_error {
                let message = ui.colored_label(ui.visuals().error_fg_color, error);
                if action.changed || action.clear {
                    message.scroll_to_me(Some(egui::Align::Center));
                }
            }
        }
        if matches!(view.kind, ViewKind::Image) {
            view.image.show_details(ui);
        }
        if matches!(view.kind, ViewKind::Plot(_)) {
            plots::PlotRenderer::show_details(ui, &view.trace);
        }
        if matches!(
            self.views[index].configuration.output,
            Output::PoseSnapshot(_)
        ) {
            self.uncertainty_inspector_ui(ui, index, frozen);
        }
        if matches!(
            self.views[index].configuration.output,
            Output::CameraSnapshot(_)
        ) {
            self.camera_inspector_ui(ui, index, frozen);
        }
        if self.views[index].configuration.output == Output::MarkerSnapshot {
            self.marker_inspector_ui(ui, index, frozen);
        }
        let view = &mut self.views[index];
        ui.collapsing("Advanced", |ui| {
        ui.collapsing("Source details", |ui| {
            if let Some(robot) = &view.configuration.robot {
                if let Some(description) = &view.robot_description {
                    ui.label("Robot description topic");
                    ui.add(egui::Label::new(&description.message_type).selectable(true).wrap());
                    ui.small(format!("Description QoS: {} · {}",
                        if description.qos.reliability == crate::components::workspace::Reliability::BestEffort { "Best effort" } else { "Reliable" },
                        if description.qos.durability == crate::components::workspace::Durability::TransientLocal { "Transient local" } else { "Volatile" }));
                    if let Some((bytes, digest)) = &view.robot_description_summary {
                        ui.label(format!("Captured {bytes} bytes"));
                        ui.add(egui::Label::new(format!("SHA-256: {digest}")).selectable(true).wrap());
                    } else {
                        ui.small("Description not captured. Configure this display, then Read and review.");
                    }
                    if let Some(root) = &description.asset_root { ui.label(format!("Asset directory: {root}")); }
                } else { ui.label("Local URDF description"); }
                ui.label(format!("Live poses in fixed frame: {}", robot.fixed_frame));
                ui.label(format!("Dynamic TF: {}", robot.dynamic_tf_topic));
                ui.label(format!("Static TF: {}", robot.static_tf_topic));
                ui.small("Description reception is finished. Enable controls live TF reception.");
            } else {
                ui.add(egui::Label::new(&view.configuration.message_type).selectable(true).wrap());
            }
            if let Some(spatial) = &view.configuration.spatial {
                ui.label(format!("Fixed frame: {}", spatial.fixed_frame));
                ui.small(format!("Dynamic TF: {} · Static TF: {}", spatial.dynamic_tf_topic, spatial.static_tf_topic));
                ui.monospace(&view.spatial_details);
                if matches!(view.kind,ViewKind::TfGraph) {
                    if let Some(sample)=view.tf_graph.sample() {ui.small(format!("TF admitted/rejected or thinned: {}/{} · Dynamic/static batches: {}/{}",sample.counters.tf_accepted,sample.counters.tf_rejected,sample.counters.tf_dynamic_batches,sample.counters.tf_static_batches));}
                } else if matches!(view.kind, ViewKind::PointCloud) {
                    if let Some(frame) = &view.cloud_frame {
                        let c = frame.counters;
                        ui.small(format!("Clouds: {} · Pending replaced: {} · Lookup timeouts: {} · TF admitted/rejected or thinned: {}/{}", c.clouds_received, c.pending_replaced, c.lookup_timeouts, c.tf_accepted, c.tf_rejected));
                    } else {
                        ui.small("Cloud counters unavailable until a sample arrives.");
                    }
                } else if matches!(view.kind, ViewKind::Mapping(_)) {
                    if let Some(frame) = &view.mapping_frame { let c = frame.counters; ui.small(format!("Messages: {} · Pending replaced: {} · Lookup timeouts: {} · TF admitted/rejected or thinned: {}/{}", c.messages_received,c.pending_replaced,c.lookup_timeouts,c.tf_accepted,c.tf_rejected)); }
                } else if matches!(view.kind, ViewKind::Markers) {
                    if let Some(state)=&view.marker.state {ui.small(format!("Adds: {} · Deletes: {} · Expired: {} · Rejected: {}",state.counters.adds,state.counters.deletes,state.counters.expired,state.counters.rejected));}
                } else {
                    ui.small(format!("Scans: {} · Pending replaced: {} · Lookup timeouts: {} · TF admitted/rejected or thinned: {}/{}", view.spatial_counters.scans_received, view.spatial_counters.pending_replaced, view.spatial_counters.lookup_timeouts, view.spatial_counters.tf_accepted, view.spatial_counters.tf_rejected));
                }
            }
        });
        ui.collapsing("Requested QoS and diagnostics", |ui| {
            ui.label(format!(
                "Reliability: {}",
                if view.configuration.best_effort {
                    "Best effort"
                } else {
                    "Reliable"
                }
            ));
            ui.label(format!(
                "Durability: {}",
                if view.configuration.transient_local {
                    "Transient local"
                } else {
                    "Volatile"
                }
            ));
            ui.small("Requested settings; publisher endpoint QoS is not discovered.");
            ui.small(format!(
                "Samples: {} · UI drops: {} · backend drops: {}",
                view.received.typed_samples, view.queue_drops, view.received.backend_drops
            ));
        });
        });
        let mut messages = false;
        let mut plot = false;
        if let ViewKind::Plot(path) = &view.kind {
            ui.monospace(path.label());
            ui.small("Local receipt time; units unknown.");
            messages = ui
                .add_enabled(!frozen, shell::quiet("Return to Messages"))
                .clicked();
        } else if matches!(view.kind, ViewKind::Messages) && view.status.active() {
            ui.separator();
            studio_ui_foundation::theme::strong(ui, "Plot a numeric field");
            plots::PlotRenderer::field_picker(ui, &view.numeric_choices, &mut view.plot_draft);
            let valid = view
                .plot_draft
                .as_ref()
                .zip(view.received.typed.as_ref())
                .is_some_and(|(path, sample)| {
                    plots::numeric_value(&sample.value, path)
                        .and_then(|v| v.coordinate())
                        .is_ok()
                });
            if view.numeric_choices.is_empty() {
                ui.small("No supported numeric fields in this message.");
            }
            plot = ui
                .add_enabled(!frozen && valid, shell::primary(ui.visuals(), "Open plot"))
                .clicked();
        }
        if messages {
            self.views[index].kind = ViewKind::Messages;
        }
        if plot {
            self.open_plot(id);
        }
        if configure {
            self.configure_view(id);
        }
        if retry {
            self.stop_view(id);
        }
        if remove {
            self.remove_view(id);
        }
    }
    fn workspace_views_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        self.prune_scene_resources();
        let companions: Vec<_> = self
            .workspace_ordered_ids()
            .into_iter()
            .filter(|id| {
                self.views
                    .iter()
                    .any(|v| v.id == *id && matches!(v.kind, ViewKind::Image | ViewKind::Camera))
            })
            .collect();
        if !companions.contains(&self.companion_selected.unwrap_or(ViewId(u64::MAX))) {
            self.companion_selected = companions.first().copied();
        }
        if self.companion_open != self.companion_was_open {
            self.companion_layout_revision = self.companion_layout_revision.wrapping_add(1);
            self.companion_was_open = self.companion_open;
        }
        let dock_was_open = self.companion_open;
        if !companions.is_empty() {
            let total = ui.available_height();
            // At short heights the dock needs room for its controls and image.
            // This is an effective native allocation, not a saved-format change.
            let dock_max = (total * if total < 450.0 { 0.7 } else { 0.4 })
                .min((total - 150.0).max(shell::CONTROL_HEIGHT + 8.0))
                .max(shell::CONTROL_HEIGHT + 8.0);
            let dock_min = 180.0_f32.min(dock_max);
            let companion_panel = egui::TopBottomPanel::bottom(egui::Id::new((
                "companions",
                self.companion_layout_revision,
            )))
            .resizable(self.companion_open)
            .default_height(
                (total * f32::from(self.companion_percent) / 100.)
                    .max(dock_min)
                    .min(dock_max),
            )
            .height_range(if self.companion_open {
                dock_min..=dock_max
            } else {
                (shell::CONTROL_HEIGHT + 8.0)..=(shell::CONTROL_HEIGHT + 8.0)
            })
            .show_inside(ui, |ui| {
                if total < 450.0 {
                    ui.spacing_mut().interact_size.y = shell::COMPACT_HEIGHT;
                    ui.spacing_mut().item_spacing.y = 4.0;
                }
                ui.set_min_height(ui.max_rect().height());
                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.companion_open, "Companions");
                    egui::ScrollArea::horizontal()
                        .id_salt("companion-tabs")
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for id in &companions {
                                    let view = self.views.iter().find(|v| v.id == *id).unwrap();
                                    let label =
                                        format!("{} · {}", view.kind.label(), view.source_label());
                                    if ui
                                        .add(
                                            egui::Button::new(egui::RichText::new(&label).color(
                                                if self.companion_selected == Some(*id) {
                                                    ui.visuals().selection.stroke.color
                                                } else {
                                                    ui.visuals().text_color()
                                                },
                                            ))
                                            .selected(self.companion_selected == Some(*id))
                                            .truncate(),
                                        )
                                        .on_hover_text(view.source_label())
                                        .clicked()
                                    {
                                        self.companion_selected = Some(*id);
                                        self.companion_open = true;
                                    }
                                }
                            });
                        });
                });
                if self.companion_open {
                    // A child's intrinsic Image size must never grow the dock.
                    let rect = ui.available_rect_before_wrap();
                    let mut content = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt("companion-content")
                            .max_rect(rect),
                    );
                    content.set_clip_rect(rect.intersect(ui.clip_rect()));
                    let ui = &mut content;
                    if let Some(id) = self.companion_selected {
                        let enabled = self.views.iter().any(|v| v.id == id && v.enabled);
                        if enabled {
                            if self
                                .views
                                .iter()
                                .any(|v| v.id == id && matches!(v.kind, ViewKind::Camera))
                            {
                                let height = ui.available_height();
                                self.combined_scene_ui(ui, height, frozen, Some(id));
                            } else {
                                self.display_frame_ui(ui, id, ui.available_height(), frozen);
                            }
                        } else {
                            ui.label("Layer stopped. Use the topic eye to show it again.");
                        }
                    }
                }
            });
            // An automatic minimum/compact allocation is not new saved intent.
            // A user resize differs from the effective height for this frame.
            let effective_height =
                (total * f32::from(self.companion_percent) / 100.).clamp(dock_min, dock_max);
            if dock_was_open
                && self.companion_open
                && (companion_panel.response.rect.height() - effective_height).abs() > 1.0
            {
                self.companion_percent =
                    ((companion_panel.response.rect.height() / total.max(1.) * 100.).round() as u8)
                        .clamp(15, 40);
            }
        }
        let status = if self.connection.phase == Phase::Starting {
            "Discovering topics…"
        } else if self.views.is_empty() {
            "Find a topic in Topics and use its eye action."
        } else if !self.views.iter().any(|v| v.enabled) {
            "Use a topic’s eye to receive data."
        } else {
            ""
        };
        ui.add(egui::Label::new(status).truncate())
            .on_hover_text(status);
        self.combined_scene_ui(ui, ui.available_height(), frozen, None);
    }
    fn combined_scene_ui(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        frozen: bool,
        camera_id: Option<ViewId>,
    ) {
        use spatial::combined::{
            CombinedSceneFrame, MapLayerFrame, PathLayerFrame, PointLayerFrame, PointStyle,
            PoseLayerFrame, RobotGeometryFrame,
        };
        let uncertainty_layers: Vec<_> = self
            .views
            .iter()
            .filter(|view| view.enabled && view.status.active())
            .filter_map(|view| view.uncertainty.layer(view.id.0))
            .collect();
        let marker_permit = self.marker_permit_for(camera_id.is_some());
        let marker_presentation = self
            .markers
            .projection(camera_id.is_some())
            .zip(marker_permit.as_ref())
            .map(|(output, permit)| spatial::scene_view::MarkerPresentation {
                camera: output.camera,
                size: output.size,
                layer: spatial::marker_primitives::MarkerLayer::new(&output.frame, permit)
                    .expect("permit created for this frame"),
                bounds: self.marker_bounds(None),
                selected_bounds: self.marker_bounds(self.selected.map(|id| id.0)),
            });
        let views: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.enabled && v.configuration.owns_tf())
            .collect();
        let fixed_frame = &self.scene_settings.configuration.fixed_frame;
        // An unconfigured preview uses a local world grid, never a TF fallback.
        let fixed_frame = if fixed_frame.is_empty() {
            "world"
        } else {
            fixed_frame.as_str()
        };
        let layers: Vec<_> = views
            .iter()
            .filter(|v| matches!(v.kind, ViewKind::Spatial | ViewKind::PointCloud))
            .map(|v| PointLayerFrame {
                samples: Some(v.sensor_history.points()),
                appearance: Some(&v.sensor_appearance),
                id: v.id.0,
                points: &[],
                style: PointStyle::for_id(v.id.0),
                status: &v.spatial_status,
                error: v.error.as_deref(),
            })
            .collect();
        let robot = views
            .iter()
            .find_map(|v| {
                v.robot
                    .as_ref()
                    .filter(|_| matches!(v.kind, ViewKind::Robot))
            })
            .map(|live| RobotGeometryFrame {
                prepared: live.prepared(),
                poses: live.poses(),
            });
        use spatial::mapping_live::WorldGeometry;
        let map = views
            .iter()
            .find(|v| v.configuration.output == Output::OccupancyGrid)
            .map(|v| MapLayerFrame {
                id: v.id.0,
                local_generation: v.mapping_generation,
                sequence: v.spatial_sequence,
                map: v
                    .mapping_frame
                    .as_ref()
                    .and_then(|f| match f.geometry.as_ref() {
                        Some(WorldGeometry::Map(m)) => Some(m),
                        _ => None,
                    }),
                status: &v.spatial_status,
                error: v.error.as_deref(),
            });
        let texture_maps: Vec<_> = views
            .iter()
            .filter(|v| v.configuration.output == Output::MapSnapshot)
            .filter_map(|v| {
                v.map
                    .frame
                    .as_ref()
                    .filter(|_| v.error.is_none())
                    .map(|map| spatial::map_texture::TextureMapFrame {
                        id: v.id.0,
                        map,
                        appearance: v.map.appearance.into(),
                    })
            })
            .collect();
        let pose = views
            .iter()
            .find(|v| v.configuration.output == Output::Pose)
            .map(|v| PoseLayerFrame {
                id: v.id.0,
                pose: v
                    .mapping_frame
                    .as_ref()
                    .and_then(|f| match f.geometry.as_ref() {
                        Some(WorldGeometry::Pose(p)) => Some(*p),
                        _ => None,
                    }),
                status: &v.spatial_status,
                error: v.error.as_deref(),
            });
        let path = views
            .iter()
            .find(|v| v.configuration.output == Output::Path)
            .map(|v| PathLayerFrame {
                id: v.id.0,
                points: v
                    .mapping_frame
                    .as_ref()
                    .and_then(|f| match f.geometry.as_ref() {
                        Some(WorldGeometry::Path(p)) => Some(p.as_slice()),
                        _ => None,
                    })
                    .unwrap_or(&[]),
                style: PointStyle::for_id(v.id.0),
                status: &v.spatial_status,
                error: v.error.as_deref(),
            });
        let frame = CombinedSceneFrame {
            tf: views
                .iter()
                .find(|v| v.configuration.output == Output::TfGraph)
                .map(|v| spatial::combined::TfLayerFrame {
                    id: v.id.0,
                    view: &v.tf_graph,
                }),
            fixed_frame,
            robot,
            layers: &layers,
            map,
            pose,
            path,
        };
        self.shared_scene.selected = self.selected.map(|id| id.0);
        self.shared_scene.robot_id = views
            .iter()
            .find(|v| matches!(v.kind, ViewKind::Robot))
            .map(|v| v.id.0);
        let robot_status = views
            .iter()
            .find(|v| matches!(v.kind, ViewKind::Robot))
            .map_or(
                if views
                    .iter()
                    .any(|v| v.configuration.output == Output::MarkerSnapshot)
                {
                    "Markers: source-stamped or latest-available transforms"
                } else if views
                    .iter()
                    .any(|v| v.configuration.output == Output::TfGraph)
                {
                    "Sensors: acquisition time · TF frames: latest available"
                } else {
                    "Acquisition-time sensor transforms"
                },
                |v| v.robot_status.as_str(),
            );
        let error = views.iter().find_map(|v| v.error.as_deref());
        let camera_display = camera_id.and_then(|id| {
            views
                .iter()
                .copied()
                .find(|v| v.id == id && matches!(v.kind, ViewKind::Camera))
        });
        let camera_mode = camera_display.is_some();
        let mut render_error = None;
        ui.push_id(if camera_mode { "camera-companion" } else { "combined-scene-frame" }, |ui| {
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), height.max(1.)), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add_enabled_ui(!frozen, |ui| {
                    if let Some(display) = camera_display {
                        ui.add(egui::Label::new(&display.spatial_status).truncate()).on_hover_text(&display.spatial_status);
                        if display.camera.is_current() {
                            let camera = &display.camera;
                            self.camera_canvas.appearance = camera.appearance;
                            let aligned = spatial::calibrated_view::CalibratedFrame {
                                snapshot: camera.snapshot.as_ref().unwrap(), permission: camera.permission.as_ref().unwrap(),
                                image: camera.image.as_ref().unwrap(), projection: camera.projection.unwrap(), scene: &frame,
                                overlays: spatial::scene_view::SceneOverlays { uncertainty: &uncertainty_layers, maps: &texture_maps, markers: marker_presentation },
                            };
                            if let Err(error) = self.camera_canvas.show(ui, &aligned, self.camera_renderer.as_mut(), (height - 24.).max(1.)) { render_error = Some(error); }
                        } else {
                            self.camera_canvas.clear();
                            if let Some(renderer) = &mut self.camera_renderer { renderer.release_frame(); }
                            // Preserve the image region even when continuity is invalidated.
                            // No stale pixels or live overlays are presented as aligned.
                            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), (height - 24.).max(1.)), egui::Layout::centered_and_justified(egui::Direction::TopDown), |ui| {
                                ui.label("Camera alignment unavailable · review the layer status above");
                            });
                        }
                    } else if let Err(error) = self.shared_scene.show_with_overlays(ui, &frame, robot_status, error, self.spatial_renderer.as_mut(),
                        spatial::scene_view::SceneOverlays { uncertainty: &uncertainty_layers, maps: &texture_maps, markers: marker_presentation }) {
                        render_error = Some(error.to_string());
                    }
                });
            });
        });
        if let Some(error) = render_error {
            // Geometry failure cannot establish ROS cleanup. Stop correlated scene
            // owners, retaining ordinary peers and cleanup reservations.
            let ids: Vec<_> = views
                .iter()
                .filter(|v| !camera_mode || matches!(v.kind, ViewKind::Camera))
                .map(|v| v.id)
                .collect();
            for id in ids {
                self.stop_view(id);
                if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                    view.error = Some(error.clone());
                }
            }
            self.diagnostic(format!("Shared scene rendering failed: {error}"));
        }
    }
    fn display_frame_ui(&mut self, ui: &mut egui::Ui, id: ViewId, _height: f32, frozen: bool) {
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else {
            return;
        };
        ui.push_id(id.0, |ui| {
            if let Some(error) = &view.error {
                ui.add(egui::Label::new(error).truncate())
                    .on_hover_text(error);
            }
            ui.add_enabled_ui(!frozen, |ui| render_view(ui, view, None));
            reveal_focused_control(ui);
        });
    }
    pub(super) fn setup_ui(&mut self, ui: &mut egui::Ui) {
        if self.robot_setup.is_open() {
            self.robot_setup_ui(ui);
            return;
        }
        let target = self
            .setup_target
            .and_then(|id| self.views.iter().find(|v| v.id == id));
        let active = target.is_some_and(|v| v.subscription.is_some());
        studio_ui_foundation::theme::strong(
            ui,
            if self.setup_target.is_some() {
                "Display draft"
            } else {
                "Topic properties"
            },
        );
        if let Some(view) = target {
            ui.small(format!(
                "Saved source: {} · {}",
                view.configuration.topic, view.configuration.message_type
            ));
            ui.small("Edits stay in this draft until Apply. Discard retains the saved source.");
        } else {
            ui.small(
                "Select a display mode and settings, then use this topic's eye to start reception.",
            );
        }
        if self.draft_output == Output::TfGraph {
            self.tf_setup_ui(ui, active);
            return;
        }
        let choices: Vec<_> = self
            .discovery
            .topics
            .iter()
            .filter(|(_, types)| {
                types
                    .iter()
                    .any(|ty| studio_visualization::catalog::for_type(ty).is_some())
            })
            .cloned()
            .collect();
        let caption = ui.label("Source topic");
        let mut chosen = self.discovery.topic.clone();
        let response = egui::ComboBox::from_id_salt("display-source-topic")
            .selected_text(if chosen.is_empty() {
                "Choose topic"
            } else {
                &chosen
            })
            .width((ui.available_width() - 12.0).max(1.0))
            .truncate()
            .show_ui(ui, |ui| {
                for (topic, _) in &choices {
                    ui.selectable_value(&mut chosen, topic.clone(), topic)
                        .on_hover_text(topic);
                }
            })
            .response
            .labelled_by(caption.id);
        describe_choice(&response, "Source topic", &chosen);
        if chosen != self.discovery.topic {
            self.discovery.select(&chosen);
        }
        let types = choices
            .iter()
            .find(|(topic, _)| topic == &self.discovery.topic)
            .map(|(_, types)| types.clone())
            .unwrap_or_default();
        let caption = ui.label("Exact message type");
        let response = egui::ComboBox::from_id_salt("display-source-type")
            .selected_text(if self.discovery.ty.is_empty() {
                "Choose type"
            } else {
                &self.discovery.ty
            })
            .width((ui.available_width() - 12.0).max(1.0))
            .truncate()
            .show_ui(ui, |ui| {
                for ty in &types {
                    if studio_visualization::catalog::for_type(ty).is_some() {
                        ui.selectable_value(&mut self.discovery.ty, ty.clone(), ty);
                    }
                }
            })
            .response
            .labelled_by(caption.id);
        describe_choice(&response, "Exact message type", &self.discovery.ty);
        if types.len() > 1 {
            ui.small("Multiple types advertised. Choose one explicitly.");
        }
        let descriptor = studio_visualization::catalog::for_type(&self.discovery.ty);
        if self.setup_target.is_none() {
            let previous = self.draft_output.clone();
            egui::ComboBox::from_id_salt("topic-draft-mode")
                .selected_text(
                    studio_visualization::catalog::DISPLAYS
                        .iter()
                        .find(|d| {
                            std::mem::discriminant(&d.output)
                                == std::mem::discriminant(&self.draft_output)
                        })
                        .map_or("Display mode", |d| d.label),
                )
                .show_ui(ui, |ui| {
                    for mode in studio_visualization::catalog::DISPLAYS
                        .iter()
                        .filter(|d| d.message_type == Some(self.discovery.ty.as_str()))
                    {
                        ui.selectable_value(
                            &mut self.draft_output,
                            mode.output.clone(),
                            mode.label,
                        );
                    }
                });
            if previous != self.draft_output {
                if let Output::CameraSnapshot(info) = &mut self.draft_output {
                    info.topic = camera_display::inferred_info(&self.discovery.topic);
                }
            }
        }
        let camera_draft = matches!(self.draft_output, Output::CameraSnapshot(_));
        if let Some(descriptor) = descriptor {
            if !(camera_draft && self.discovery.ty == studio_protocol::image::MESSAGE_TYPE) {
                self.draft_output = descriptor.output.clone();
            }
            studio_ui_foundation::theme::strong(
                ui,
                if matches!(self.draft_output, Output::CameraSnapshot(_)) {
                    "Calibrated Camera"
                } else {
                    descriptor.label
                },
            );
            ui.small(if matches!(self.draft_output, Output::CameraSnapshot(_)) {
                "Rectified Image aligned with scene geometry using CameraInfo and optical TF."
            } else {
                descriptor.description
            });
        }
        let scene = matches!(
            self.draft_output,
            Output::SpatialScan
                | Output::CameraSnapshot(_)
                | Output::PointCloud
                | Output::MapSnapshot
                | Output::MarkerSnapshot
                | Output::PoseSnapshot(_)
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        );
        if scene {
            ui.separator();
            ui.label(format!(
                "Scene frame: {}",
                self.scene_settings.configuration.fixed_frame
            ));
            ui.small(if self.scene_settings.verified {
                "Time: System (default)."
            } else {
                "Time: System. Reception starts when the source is enabled."
            });
            if ui.button("Review Scene settings").clicked() {
                self.open_scene_settings();
                return;
            }
        }
        ui.collapsing("Source QoS", |ui| {
            ui.checkbox(&mut self.draft_best_effort, "Best effort");
            ui.checkbox(&mut self.draft_transient_local, "Transient local");
            ui.small("Requested settings. Publisher endpoint compatibility is not discovered yet.");
        });
        if active {
            ui.small("Apply stops and replaces only this display after confirmed cleanup. Peers continue.");
        }
        if !self.discovery.valid_selection() {
            ui.label("Choose an available topic and exact type.");
        }
        let frame_valid = !scene
            || studio_protocol::spatial::valid_frame(
                &self.scene_settings.configuration.fixed_frame,
            );
        if !frame_valid {
            ui.label("Configure the shared fixed frame in Scene settings first.");
        }
        let updates_valid = if self.draft_output == Output::MapSnapshot {
            self.map_update_draft_ui(ui)
        } else {
            true
        };
        let camera_valid = self.camera_source_draft_ui(ui);
        let ready = self.connection.phase == Phase::Connected
            && camera_valid
            && updates_valid
            && self.discovery.valid_selection()
            && descriptor.is_some()
            && frame_valid
            && (!active || !scene || self.scene_settings.verified)
            && (self.setup_target.is_some() || self.views.len() < MAX_SUBSCRIPTIONS);
        if self.setup_target.is_some()
            && ui
                .add_enabled(ready, shell::primary(ui.visuals(), "Apply display changes"))
                .clicked()
        {
            self.draft_spatial = self.scene_settings.configuration.clone();
            self.draft_clock_verified = self.scene_settings.verified;
            self.subscribe();
        }
        if ui.button("Discard display draft").clicked() {
            self.setup = false;
            self.setup_target = None;
        }
    }
}

/// egui's focus traversal can reach controls outside a scroll viewport. Reveal
/// only clipped controls belonging to this content area, without following focus
/// in other panels or recentering a control that is already visible.
pub(super) fn reveal_focused_control(ui: &egui::Ui) {
    if let Some(response) = ui
        .memory(|memory| memory.focused())
        .and_then(|id| ui.ctx().read_response(id))
    {
        if ui.min_rect().contains_rect(response.rect)
            && !ui.clip_rect().contains_rect(response.rect)
        {
            response.scroll_to_me_animation(
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
        }
    }
}

fn render_view(
    ui: &mut egui::Ui,
    view: &mut ViewState,
    renderer: Option<&mut spatial::Renderer3d>,
) {
    if !view.status.active() {
        if view.status == ViewStatus::Stopped {
            ui.label("Enable this display from the topic sidebar to receive data.");
        }
        return;
    }
    match &view.kind {
        ViewKind::Robot => {
            if let Some(live) = &view.robot {
                let fixed_frame = view
                    .configuration
                    .robot
                    .as_ref()
                    .map_or("", |c| c.fixed_frame.as_str());
                if let Err(error) = view.robot_view.show(
                    ui,
                    live.prepared(),
                    live.poses(),
                    crate::components::robot::RobotViewState {
                        fixed_frame,
                        status: &view.robot_status,
                        error: view.error.as_deref(),
                        links: live.links(),
                    },
                    renderer,
                ) {
                    view.error = Some(error.to_string());
                    view.robot_render_failure = Some(error.to_string());
                }
            } else {
                ui.label("Load a Robot model before enabling this display.");
            }
        }
        ViewKind::Spatial
        | ViewKind::TfGraph
        | ViewKind::Markers
        | ViewKind::Uncertainty(_)
        | ViewKind::Camera
        | ViewKind::PointCloud
        | ViewKind::Mapping(_) => {
            ui.label(&view.spatial_status);
        }
        ViewKind::Image => view.image.show_content(ui),
        ViewKind::Plot(path) => {
            let status = if view.trace.points().is_empty() {
                plots::PlotStatus::Waiting
            } else {
                plots::PlotStatus::Receiving
            };
            plots::PlotRenderer.show_content(ui, &mut view.trace, status, Some(path));
        }
        ViewKind::Messages => {
            if let Some(sample) = &view.received.typed {
                crate::components::typed_view::show(ui, &sample.value);
            } else {
                ui.label("Waiting for messages…");
                ui.label("Check the publisher and configured QoS. No message rate or endpoint QoS is inferred.");
            }
        }
    }
}
