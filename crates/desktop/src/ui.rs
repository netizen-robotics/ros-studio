use super::*;

fn describe_choice(response: &egui::Response, label: &str, value: &str) {
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, response.enabled(), label);
        info.current_text_value = Some(value.into());
        info
    });
}
fn tf_topic_choice(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut String,
    topics: &[(String, Vec<String>)],
) {
    let caption = ui.label(label);
    let mut chosen = false;
    let response = egui::ComboBox::from_id_salt(label)
        .selected_text(if selected.is_empty() {
            "Choose a discovered TF topic"
        } else {
            selected.as_str()
        })
        .width((ui.available_width() - 15.0).max(1.0))
        .truncate()
        .show_ui(ui, |ui| {
            for (name, types) in topics
                .iter()
                .filter(|(_, types)| types.iter().any(|ty| ty == "tf2_msgs/msg/TFMessage"))
            {
                let _ = types;
                if ui
                    .add(egui::Button::new(name).selected(name == selected).wrap())
                    .clicked()
                {
                    *selected = name.clone();
                    chosen = true;
                    ui.memory_mut(|memory| memory.close_popup());
                }
            }
        })
        .response
        .labelled_by(caption.id);
    describe_choice(&response, label, selected);
    if chosen {
        response.request_focus();
    }
}
impl App {
    pub(super) fn render(&mut self, ctx: &egui::Context) {
        let narrow = ctx.screen_rect().width() < 1000.0;
        let frozen = self.workspace_frozen();
        self.header_ui(ctx, frozen, narrow);
        self.health_ui(ctx, frozen);
        if self.sidebar && !(narrow && self.inspector) {
            egui::SidePanel::left("resources")
                .default_width(245.0)
                .frame(
                    egui::Frame::new()
                        .fill(shell::palette(&ctx.style().visuals).subtle)
                        .inner_margin(shell::PADDING),
                )
                .width_range(180.0..=360.0)
                .show(ctx, |ui| {
                    // Reserve the measured footer before the scrollable navigation.
                    // Expanded capacity guidance therefore reduces the list, not its own visibility.
                    egui::TopBottomPanel::bottom("resources-footer")
                        .frame(egui::Frame::new())
                        .show_inside(ui, |ui| self.sidebar_footer_ui(ui, frozen));
                    self.topic_sidebar_ui(ui, frozen);
                });
        }
        if self.inspector {
            egui::SidePanel::right("inspector")
                .frame(
                    egui::Frame::new()
                        .fill(shell::palette(&ctx.style().visuals).subtle)
                        .inner_margin(shell::PADDING),
                )
                .default_width(280.0)
                .width_range(220.0..=370.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        studio_ui_foundation::theme::pane_heading(ui, "Inspector");
                        if ui.button("Close").clicked() {
                            self.inspector = false;
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("inspector-content")
                        .show(ui, |ui| {
                            if self.setup {
                                ui.add_enabled_ui(!frozen, |ui| self.setup_ui(ui));
                            } else {
                                self.display_inspector_ui(ui, frozen);
                            }
                            reveal_focused_control(ui);
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
                    studio_ui_foundation::theme::pane_heading(ui, "Robot overview");
                    if (!self.sidebar || (narrow && self.inspector))
                        && ui.button("Topics").clicked()
                    {
                        self.sidebar = true;
                        self.inspector = false;
                    }
                });
                ui.separator();
                self.workspace_views_ui(ui, frozen);
            });
        let mut files_open = self.workspace.open;
        egui::Window::new("Workspace files")
            .collapsible(false)
            .open(&mut files_open)
            .default_width(640.0)
            .default_height(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("workspace-files-scroll").show(ui, |ui| {
                    self.workspace_render(ui);
                    ui.separator();
                    ui.collapsing("Connection suggestion", |ui| {
                        if let Some(suggestion) = &self.workspace.connection_suggestion {
                            ui.label(format!("Saved suggestion: domain {} · {}", suggestion.domain, match suggestion.middleware {
                                Some(studio_desktop::workspace::Middleware::FastDds) => "Fast DDS",
                                Some(studio_desktop::workspace::Middleware::CycloneDds) => "Cyclone DDS",
                                None => "middleware unspecified",
                            }));
                            ui.small("Advisory only. Choose connection settings explicitly in the top bar.");
                        } else { ui.label("No connection suggestion saved."); }
                        ui.add_enabled_ui(!self.workspace_frozen(), |ui| {
                            if ui.button("Save current settings as suggestion").clicked() {
                                match self.connection.domain_id() {
                                    Ok(domain) => self.workspace.connection_suggestion = Some(studio_desktop::workspace::ConnectionSuggestion { domain, middleware: Some(if self.connection.cyclone { studio_desktop::workspace::Middleware::CycloneDds } else { studio_desktop::workspace::Middleware::FastDds }) }),
                                    Err(error) => self.diagnostic(error),
                                }
                            }
                            if ui.button("Remove saved suggestion").clicked() { self.workspace.connection_suggestion = None; }
                        });
                    });
                });
            });
        self.workspace.open = files_open && self.workspace.open;
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    fn sidebar_footer_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        ui.separator();
        if ui
            .add_enabled(
                !frozen && self.views.len() < MAX_SUBSCRIPTIONS,
                shell::primary(ui.visuals(), "+ Robot model"),
            )
            .clicked()
        {
            self.begin_robot_setup(None);
        }
        if ui
            .add_enabled(
                !frozen
                    && self.connection.phase == Phase::Connected
                    && self.views.len() < MAX_SUBSCRIPTIONS,
                shell::primary(ui.visuals(), "+ Add display"),
            )
            .clicked()
        {
            self.begin_add();
        }
        ui.small(format!("{} / 8 displays", self.views.len()));
        let used = self.session.as_ref().map_or(0, |session| {
            session.shared.lock().unwrap().registry.used_resources()
        });
        if self.views.len() == MAX_SUBSCRIPTIONS {
            ui.small("Eight displays configured. Remove a disabled display to add another.");
        }
        ui.collapsing("Resource capacity", |ui| {
            ui.small(format!("Native readers: {used} / 8"));
            ui.small("Scene layers share two TF readers. Each sensor adds one reader; Robot reuses TF. Up to four sensor layers and one Robot.");
        });
    }
    fn health_ui(&mut self, ctx: &egui::Context, frozen: bool) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let ui_drops = self.views.iter().fold(0u64, |sum, v| sum.saturating_add(v.queue_drops));
            let backend_drops = self.views.iter().fold(0u64, |sum, v| sum.saturating_add(v.received.backend_drops));
            let active = self.views.iter().filter(|v| v.status.active()).count();
            let transitioning = self.views.iter().filter(|v| matches!(v.status, ViewStatus::Starting | ViewStatus::Stopping | ViewStatus::NeedsRecovery)).count();
            let receiving = self.views.iter().filter(|v| v.status == ViewStatus::Receiving).count();
            let rejected = self.session.as_ref().map_or(0, |s| s.shared.lock().unwrap().ingress_rejections);
            // Live counters must not wrap and resize the scene above this panel.
            let drops = if ui_drops != 0 || backend_drops != 0 {
                format!("Application drops: UI {ui_drops} · backend {backend_drops}")
            } else { "No application drops".into() };
            let summary = format!("{receiving} receiving · {active} subscribed · {transitioning} starting / stopping / cleanup · {drops}");
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!frozen, |ui| {
                        ui.toggle_value(&mut self.diagnostics_open, format!("Diagnostics ({})", self.diagnostics.len()));
                    });
                    let color = if ui_drops != 0 || backend_drops != 0 { ui.visuals().warn_fg_color } else { ui.visuals().text_color() };
                    ui.add(egui::Label::new(egui::RichText::new(&summary).small().color(color)).truncate()).on_hover_text(&summary);
                });
            });
            // Reserve one attention row even without an error. Full details stay
            // available in the inspector/diagnostics rather than changing its height.
            let workspace_attention = self.workspace_needs_attention() && !self.workspace.open;
            let attention = if workspace_attention {
                "Workspace needs attention".to_owned()
            } else if let Some(error) = self.diagnostics.back() {
                format!("! {error}")
            } else if self.connection.phase == Phase::Failed {
                "Connection failed. Review the error, adjust Connection settings, then Connect explicitly.".to_owned()
            } else { String::new() };
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if workspace_attention {
                        if ui.small_button("Review workspace error").clicked() { self.workspace.open = true; }
                    } else if ui.add_enabled(!attention.is_empty(), egui::Button::new("Review diagnostics").small()).clicked() {
                        self.diagnostics_open = true;
                    }
                    ui.add(egui::Label::new(egui::RichText::new(&attention).color(ui.visuals().error_fg_color)).truncate()).on_hover_text(&attention);
                });
            });
            if self.diagnostics_open {
                egui::ScrollArea::vertical().id_salt("diagnostics").max_height(140.0).min_scrolled_height(140.0).auto_shrink([false, false]).show(ui, |ui| {
                    ui.small(format!("UI sample drops: {ui_drops} · backend sample drops: {backend_drops} · rejected stale/invalid events: {rejected}"));
                    ui.small("Application queue counters only; DDS/network loss is not measured here.");
                    ui.label(&self.connection.status);
                    for error in &self.diagnostics { ui.add(egui::Label::new(error).selectable(true)); }
                    if self.diagnostics.is_empty() { ui.label("No recorded errors."); }
                    ui.small("Acknowledgment clears this log. Unresolved view and workspace errors remain at their source.");
                    if ui.button("Acknowledge errors").clicked() { self.diagnostics.clear(); }
                });
            }
        });
    }
    fn header_ui(&mut self, ctx: &egui::Context, frozen: bool, narrow: bool) {
        let mut file_action = None;
        egui::TopBottomPanel::top("connection")
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(shell::PADDING))
            .show(ctx, |ui| {
                // Secondary menu/connection actions rest on the header surface;
                // primary Connect retains its explicit fill and native focus stroke.
                ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.horizontal_wrapped(|ui| {
                    studio_ui_foundation::theme::strong(ui, "ROS Studio");
                    ui.menu_button("File", |ui| {
                        for (label, action) in [
                            ("Open workspace…", workspace_app::FileAction::Open),
                            ("Save workspace", workspace_app::FileAction::Save),
                            ("Save workspace as…", workspace_app::FileAction::SaveAs),
                        ] {
                            if ui.add_enabled(self.workspace_file_available(), egui::Button::new(label)).clicked() {
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
                        studio_ui_foundation::theme::exit_menu_item(ui);
                    });
                    ui.menu_button("View", |ui| {
                        ui.menu_button("Appearance", studio_ui_foundation::theme::appearance);
                        if ui.add_enabled(!frozen, egui::Button::new("Topics").selected(self.sidebar && !(narrow && self.inspector))).clicked() {
                            if narrow && self.inspector { self.inspector = false; self.sidebar = true; }
                            else { self.sidebar = !self.sidebar; }
                            ui.close_menu();
                        }
                        ui.add_enabled_ui(!frozen, |ui| { ui.checkbox(&mut self.inspector, "Inspector"); });
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("About ROS Studio").clicked() { self.about = true; ui.close_menu(); }
                    });
                    ui.separator();
                    let name = format!("{}{}", self.workspace.name, if self.workspace_dirty() { " *" } else { "" });
                    ui.add_sized([if narrow { 120.0 } else { 160.0 }, studio_ui_foundation::theme::CONTROL_HEIGHT], egui::Label::new(&name).truncate())
                        .on_hover_text(format!("{}\n{}", self.workspace.name, if self.workspace_dirty() { "Unsaved changes" } else { "Saved" }));
                    ui.label(egui::RichText::new("Observe only").small().family(studio_ui_foundation::fonts::semibold_family(ui.ctx())))
                        .on_hover_text("Discovery and subscriptions only. No robot commands.");
                    if !narrow { self.connection_header_ui(ui, frozen); }
                });
                if narrow {
                    ui.horizontal_wrapped(|ui| self.connection_header_ui(ui, frozen));
                }
                if self.settings {
                    ui.separator();
                    ui.add_enabled_ui(!frozen && matches!(self.connection.phase, Phase::Disconnected | Phase::Failed), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let label = ui.label("Domain");
                            ui.add(studio_ui_foundation::theme::input(ui, &mut self.connection.domain).hint_text(studio_ui_foundation::theme::hint(ui, "0")).desired_width(48.0)).labelled_by(label.id);
                            ui.label("Middleware");
                            ui.radio_value(&mut self.connection.cyclone, false, "Fast DDS");
                            ui.radio_value(&mut self.connection.cyclone, true, "Cyclone DDS");
                        });
                    });
                    ui.label(&self.connection.status);
                    ui.small("Settings apply on the next connection. Reconnect discovers without subscribing.");
                }
            });
        if let Some(action) = file_action {
            if let Err(error) = self.workspace_file(action) {
                self.diagnostic(error);
            }
        }
        egui::Window::new("About ROS Studio")
            .open(&mut self.about)
            .resizable(false)
            .show(ctx, |ui| {
                studio_ui_foundation::theme::strong(ui, concat!("ROS Studio ", env!("CARGO_PKG_VERSION")));
                ui.label("Observe-only ROS 2 workspace.");
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
        match self.connection.phase {
            Phase::Starting => {
                ui.spinner();
                if ui.button("Cancel startup").clicked() {
                    self.stop();
                }
            }
            Phase::Connected => {
                if ui.button("Disconnect").clicked() {
                    self.stop();
                }
            }
            _ => {
                if ui
                    .add_enabled(!frozen, shell::primary(ui.visuals(), "Connect"))
                    .clicked()
                {
                    self.connect();
                }
            }
        }
        ui.toggle_value(&mut self.settings, "Connection settings");
        let phase = match self.connection.phase {
            Phase::Disconnected => "Disconnected",
            Phase::Starting => "Starting…",
            Phase::Connected => "Connected",
            Phase::Failed => "Connection failed",
        };
        ui.add(egui::Label::new(format!("{phase} · Domain {}", self.connection.domain)).truncate())
            .on_hover_text(&self.connection.status);
    }
    fn topic_sidebar_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        studio_ui_foundation::theme::strong(ui, "Topics");
        let label = ui.small("Search topics or types");
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.discovery.query)
                .hint_text(studio_ui_foundation::theme::hint(ui, "Find a topic…"))
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
        ui.add_space(shell::SPACE);
        let mut topics = self.discovery.topics.clone();
        for view in &self.views {
            if !topics
                .iter()
                .any(|(topic, _)| topic == &view.configuration.topic)
            {
                topics.push((
                    view.configuration.topic.clone(),
                    vec![view.configuration.message_type.clone()],
                ));
            }
        }
        topics.sort_by(|a, b| a.0.cmp(&b.0));
        let ids = self.workspace_ordered_ids();
        let mut toggle = None;
        let mut select = None;
        let mut add = None;
        let mut reorder = None;
        egui::ScrollArea::vertical()
            .id_salt("topic-list")
            .show(ui, |ui| {
                let mut count = 0;
                for (topic, types) in &topics {
                    if !self.discovery.matches(topic, types) {
                        continue;
                    }
                    count += 1;
                    let configured: Vec<_> = ids
                        .iter()
                        .filter_map(|id| self.views.iter().find(|v| v.id == *id))
                        .filter(|v| &v.configuration.topic == topic)
                        .collect();
                    ui.push_id(topic, |ui| {
                        ui.horizontal(|ui| {
                            if configured.is_empty() {
                                let mut enabled = false;
                                let response = ui
                                    .add_enabled(
                                        !frozen
                                            && self.connection.phase == Phase::Connected
                                            && self.views.len() < MAX_SUBSCRIPTIONS,
                                        egui::Checkbox::without_text(&mut enabled),
                                    )
                                    .on_hover_text("Configure and enable a display");
                                response.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        response.enabled(),
                                        false,
                                        format!("Enable topic: {topic}"),
                                    )
                                });
                                if response.changed() {
                                    add = Some(topic.clone());
                                }
                            }
                            let selected = configured.iter().any(|v| self.selected == Some(v.id));
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(topic).color(
                                        if selected {
                                            ui.visuals().selection.stroke.color
                                        } else {
                                            ui.visuals().text_color()
                                        },
                                    ))
                                    .selected(selected)
                                    .fill(if selected {
                                        ui.visuals().selection.bg_fill
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    })
                                    .wrap(),
                                )
                                .clicked()
                            {
                                if let Some(view) = configured.first() {
                                    select = Some(view.id);
                                } else {
                                    add = Some(topic.clone());
                                }
                            }
                        });
                        if configured.is_empty() {
                            ui.small(types.join(" · "));
                        }
                        for view in configured {
                            ui.push_id(view.id.0, |ui| {
                                let mut enabled = view.enabled;
                                ui.horizontal(|ui| {
                                    let response = ui.add_enabled(
                                        !frozen
                                            && !matches!(
                                                view.status,
                                                ViewStatus::Stopping | ViewStatus::NeedsRecovery
                                            ),
                                        egui::Checkbox::without_text(&mut enabled),
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::Checkbox,
                                            response.enabled(),
                                            view.enabled,
                                            format!(
                                                "Enable display: {} · {} ({})",
                                                topic,
                                                view.kind.label(),
                                                view.workspace_id
                                            ),
                                        )
                                    });
                                    if response.changed() {
                                        toggle = Some((view.id, enabled));
                                    }
                                    if ui
                                        .selectable_label(
                                            self.selected == Some(view.id),
                                            view.kind.label(),
                                        )
                                        .on_hover_text(&view.configuration.message_type)
                                        .clicked()
                                    {
                                        select = Some(view.id);
                                    }
                                });
                                ui.small(if view.error.is_some() {
                                    "Error"
                                } else {
                                    match view.status {
                                        ViewStatus::Stopped => "Disabled",
                                        ViewStatus::Starting => "Starting…",
                                        ViewStatus::Waiting => "Waiting…",
                                        ViewStatus::Receiving => "Receiving",
                                        ViewStatus::Stopping => "Stopping…",
                                        ViewStatus::NeedsRecovery => "Cleanup needs attention",
                                    }
                                });
                                if let Some(error) = &view.error {
                                    ui.colored_label(ui.visuals().error_fg_color, error);
                                }
                                if self.selected == Some(view.id) {
                                    let index = ids.iter().position(|id| *id == view.id).unwrap();
                                    ui.horizontal(|ui| {
                                        let up = ui.add_enabled(
                                            !frozen && index > 0,
                                            shell::quiet("Move up"),
                                        );
                                        up.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Button,
                                                up.enabled(),
                                                format!("Move up: {topic} ({})", view.workspace_id),
                                            )
                                        });
                                        if up.clicked() {
                                            reorder = Some((view.workspace_id.clone(), index - 1));
                                        }
                                        let down = ui.add_enabled(
                                            !frozen && index + 1 < ids.len(),
                                            shell::quiet("Move down"),
                                        );
                                        down.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Button,
                                                down.enabled(),
                                                format!(
                                                    "Move down: {topic} ({})",
                                                    view.workspace_id
                                                ),
                                            )
                                        });
                                        if down.clicked() {
                                            reorder = Some((view.workspace_id.clone(), index + 1));
                                        }
                                    });
                                }
                            });
                        }
                        ui.add_space(shell::SPACE);
                    });
                }
                if count == 0 {
                    ui.label(if self.connection.phase != Phase::Connected {
                        "Connect to discover ROS topics."
                    } else if !self.discovery.received_graph {
                        "Discovering topics…"
                    } else if topics.is_empty() {
                        "No topics discovered. Check domain and middleware."
                    } else {
                        "No topics match your search."
                    });
                }
            });
        if let Some(id) = select {
            self.selected = Some(id);
            self.setup = false;
            self.inspector = true;
        }
        if let Some((id, enabled)) = toggle {
            self.set_display_enabled(id, enabled);
        }
        if let Some((id, target)) = reorder {
            let _ = self.workspace.layout.reorder(&id, target);
        }
        if let Some(topic) = add {
            self.begin_add();
            self.discovery.select(&topic);
            self.draft_output = if let Some(output) =
                [Output::OccupancyGrid, Output::Pose, Output::Path]
                    .into_iter()
                    .find(|output| {
                        output.mapping_message_type() == Some(self.discovery.ty.as_str())
                    }) {
                output
            } else if self.discovery.ty == images::IMAGE_MESSAGE_TYPE {
                Output::CompactImage
            } else if self.discovery.ty == studio_protocol::cloud::MESSAGE_TYPE {
                Output::PointCloud
            } else if self.discovery.ty == spatial::SCAN_MESSAGE_TYPE {
                Output::SpatialScan
            } else {
                Output::Typed
            };
        }
    }
    fn display_inspector_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        let Some(index) = self
            .selected
            .and_then(|id| self.views.iter().position(|v| v.id == id))
        else {
            studio_ui_foundation::theme::strong(ui, "Display details");
            ui.label("Choose a topic on the left to configure its display.");
            return;
        };
        let view = &mut self.views[index];
        let id = view.id;
        studio_ui_foundation::theme::strong(ui, view.kind.label());
        ui.add(
            egui::Label::new(&view.configuration.topic)
                .selectable(true)
                .wrap(),
        );
        shell::status(ui, view.status).on_hover_text(view.status_label());
        if let Some(error) = &view.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let configure = ui
            .add_enabled(
                !frozen
                    && !matches!(
                        view.status,
                        ViewStatus::Stopping | ViewStatus::NeedsRecovery
                    ),
                shell::quiet("Configure display"),
            )
            .clicked();
        let retry =
            view.status == ViewStatus::NeedsRecovery && ui.button("Retry cleanup").clicked();
        let remove = ui
            .add_enabled(
                !frozen && view.subscription.is_none(),
                shell::quiet("Remove display"),
            )
            .clicked();
        if view.status == ViewStatus::NeedsRecovery {
            ui.label("Reception is disabled. Retry cleanup or disconnect before enabling again.");
        }
        ui.separator();
        ui.collapsing("Source details", |ui| {
            ui.add(egui::Label::new(&view.configuration.message_type).selectable(true).wrap());
            if let Some(spatial) = &view.configuration.spatial {
                ui.label(format!("Fixed frame: {}", spatial.fixed_frame));
                ui.small(format!("Dynamic TF: {} · Static TF: {}", spatial.dynamic_tf_topic, spatial.static_tf_topic));
                ui.monospace(&view.spatial_details);
                if matches!(view.kind, ViewKind::PointCloud) {
                    if let Some(frame) = &view.cloud_frame {
                        let c = frame.counters;
                        ui.small(format!("Clouds: {} · Pending replaced: {} · Lookup timeouts: {} · TF accepted/rejected: {}/{}", c.clouds_received, c.pending_replaced, c.lookup_timeouts, c.tf_accepted, c.tf_rejected));
                    } else {
                        ui.small("Cloud counters unavailable until a sample arrives.");
                    }
                } else if matches!(view.kind, ViewKind::Mapping(_)) {
                    if let Some(frame) = &view.mapping_frame { let c = frame.counters; ui.small(format!("Messages: {} · Pending replaced: {} · Lookup timeouts: {} · TF accepted/rejected: {}/{}", c.messages_received,c.pending_replaced,c.lookup_timeouts,c.tf_accepted,c.tf_rejected)); }
                } else {
                    ui.small(format!("Scans: {} · Pending replaced: {} · Lookup timeouts: {} · TF accepted/rejected: {}/{}", view.spatial_counters.scans_received, view.spatial_counters.pending_replaced, view.spatial_counters.lookup_timeouts, view.spatial_counters.tf_accepted, view.spatial_counters.tf_rejected));
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
        if matches!(view.kind, ViewKind::Image) {
            view.image.show_details(ui);
        }
        if matches!(view.kind, ViewKind::Plot(_)) {
            plots::PlotRenderer::show_details(ui, &view.trace);
        }
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
        let ids: Vec<_> = self
            .workspace_ordered_ids()
            .into_iter()
            .filter(|id| self.views.iter().any(|v| v.id == *id && v.enabled))
            .collect();
        if ids.is_empty() {
            ui.add_space(24.0);
            ui.heading(if self.connection.phase == Phase::Starting {
                "Discovering topics…"
            } else {
                "Enable a display to see your data"
            });
            if self.views.is_empty() {
                ui.label("Choose a topic on the left, configure its display, then choose Enable display.");
            } else {
                ui.label("Displays are ready. Enable the ones you want to receive.");
            }
            if self.connection.phase != Phase::Connected {
                ui.label("Connect to your ROS data using the header above.");
            }
            return;
        }
        let scene = ids.iter().copied().find(|id| {
            self.views.iter().any(|v| {
                v.id == *id
                    && matches!(
                        v.kind,
                        ViewKind::Spatial
                            | ViewKind::PointCloud
                            | ViewKind::Mapping(_)
                            | ViewKind::Robot
                    )
            })
        });
        let peers: Vec<_> = ids
            .into_iter()
            .filter(|id| {
                self.views
                    .iter()
                    .any(|v| v.id == *id && !v.configuration.owns_tf())
            })
            .collect();
        let height = ui.available_height();
        egui::ScrollArea::vertical().id_salt("workspace-views").auto_shrink([false, false]).show(ui, |ui| {
            if scene.is_some() {
                let scene_height = if peers.is_empty() { height } else { height * f32::from(self.workspace.layout.scene_percent) / 100.0 };
                self.combined_scene_ui(ui, scene_height.max(300.0), frozen);
                if !peers.is_empty() {
                    ui.add_enabled_ui(!frozen, |ui| {
                        ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut self.workspace.layout.scene_percent, 35..=80).text("Scene height (%)")))
                            .on_hover_text("Drag or focus and use arrow keys to resize the scene and companion area.");
                    });
                    ui.separator();
                }
            }
            let companion_height = if scene.is_some() { height * (1.0 - f32::from(self.workspace.layout.scene_percent) / 100.0) - 40.0 } else { height };
            let each = (companion_height / peers.len().max(1) as f32).max(180.0);
            for id in peers { self.display_frame_ui(ui, id, each, frozen); }
            reveal_focused_control(ui);
        });
    }
    fn combined_scene_ui(&mut self, ui: &mut egui::Ui, height: f32, frozen: bool) {
        use spatial::combined::{
            CombinedSceneFrame, MapLayerFrame, PathLayerFrame, PointLayerFrame, PointStyle,
            PoseLayerFrame, RobotGeometryFrame,
        };
        let views: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.enabled && v.configuration.owns_tf())
            .collect();
        let Some(first) = views.first() else {
            return;
        };
        let key = first.configuration.scene_key().expect("scene owner");
        let layers: Vec<_> = views
            .iter()
            .filter(|v| matches!(v.kind, ViewKind::Spatial | ViewKind::PointCloud))
            .map(|v| PointLayerFrame {
                samples: None,
                appearance: None,
                id: v.id.0,
                points: if matches!(v.kind, ViewKind::PointCloud) {
                    v.cloud_frame
                        .as_ref()
                        .map_or(&[], |frame| frame.points.as_slice())
                } else {
                    v.spatial_scene
                        .layers()
                        .first()
                        .map_or(&[], |layer| layer.points())
                },
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
            tf: None,
            fixed_frame: &key.fixed_frame,
            robot,
            layers: &layers,
            map,
            pose,
            path,
        };
        let robot_status = views
            .iter()
            .find(|v| matches!(v.kind, ViewKind::Robot))
            .map_or("Acquisition-time sensor transforms", |v| {
                v.robot_status.as_str()
            });
        let error = views.iter().find_map(|v| v.error.as_deref());
        let mut render_error = None;
        ui.push_id("combined-scene-frame", |ui| {
            shell::view_frame(ui, false).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("combined-scene-content")
                    .auto_shrink([false, false])
                    .max_height(height)
                    .min_scrolled_height(height)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!frozen, |ui| {
                            if let Err(error) = self.shared_scene.show(
                                ui,
                                &frame,
                                robot_status,
                                error,
                                self.spatial_renderer.as_mut(),
                            ) {
                                render_error = Some(error.to_string());
                            }
                            reveal_focused_control(ui);
                        });
                    });
            });
        });
        if let Some(error) = render_error {
            // Geometry failure cannot establish ROS cleanup. Stop correlated scene
            // owners, retaining ordinary peers and cleanup reservations.
            let ids: Vec<_> = views.iter().map(|v| v.id).collect();
            for id in ids {
                self.stop_view(id);
                if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                    view.error = Some(error.clone());
                }
            }
            self.diagnostic(format!("Shared scene rendering failed: {error}"));
        }
    }
    fn display_frame_ui(&mut self, ui: &mut egui::Ui, id: ViewId, height: f32, frozen: bool) {
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        let current = &self.views[index];
        let title = shell::view_title(&current.configuration.topic, current.kind.label());
        let collision = self.views.iter().any(|v| {
            v.id != id
                && v.configuration.topic != current.configuration.topic
                && shell::view_title(&v.configuration.topic, v.kind.label()) == title
        });
        let view = &mut self.views[index];
        let renderer = self.spatial_renderer.as_mut();
        let mut selected = false;
        ui.push_id(id.0, |ui| {
            shell::view_frame(ui, self.selected == Some(id)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let response = ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(&title)
                                    .family(studio_ui_foundation::fonts::semibold_family(ui.ctx())),
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .truncate(),
                        )
                        .on_hover_text(format!(
                            "{}\n{}",
                            view.configuration.topic, view.configuration.message_type
                        ));
                    view_action_name(&response, "Details", view);
                    selected = response.clicked();
                    shell::status(ui, view.status).on_hover_text(view.status_label());
                });
                if collision {
                    ui.add(
                        egui::Label::new(&view.configuration.topic)
                            .selectable(true)
                            .wrap(),
                    );
                }
                if matches!(
                    view.kind,
                    ViewKind::Spatial
                        | ViewKind::PointCloud
                        | ViewKind::Mapping(_)
                        | ViewKind::Robot
                ) {
                    let error = view.error.as_deref().unwrap_or("");
                    ui.add_sized(
                        [
                            ui.available_width(),
                            ui.text_style_height(&egui::TextStyle::Body),
                        ],
                        egui::Label::new(
                            egui::RichText::new(error).color(ui.visuals().error_fg_color),
                        )
                        .truncate(),
                    )
                    .on_hover_text(error);
                } else if let Some(error) = &view.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                egui::ScrollArea::both()
                    .id_salt("view-content")
                    .auto_shrink([false, false])
                    .max_height((height - 44.0).max(120.0))
                    .min_scrolled_height((height - 44.0).max(120.0))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!frozen, |ui| render_view(ui, view, renderer));
                        reveal_focused_control(ui);
                    });
            });
            ui.add_space(shell::SPACE);
        });
        if selected {
            self.selected = Some(id);
            self.setup = false;
            self.inspector = true;
        }
    }
    pub(super) fn setup_ui(&mut self, ui: &mut egui::Ui) {
        if self.robot_setup.is_open() {
            self.robot_setup_ui(ui);
            return;
        }
        // Verification applies to these publishers and transform settings. Draft
        // edits revoke it before this frame can enable/apply a replacement; the
        // checkbox itself is deliberately excluded from the fingerprint.
        let clock_source = (
            self.discovery.topic.clone(),
            self.discovery.ty.clone(),
            self.draft_output.clone(),
            self.draft_spatial.clone(),
        );
        let replacing = self
            .setup_target
            .and_then(|id| self.views.iter().find(|v| v.id == id))
            .is_some_and(|v| v.subscription.is_some());
        studio_ui_foundation::theme::strong(
            ui,
            if replacing {
                "Draft changes"
            } else if self.setup_target.is_some() {
                "Enable configured display"
            } else {
                "Set up a display"
            },
        );
        ui.small(if replacing {
            "Edits take effect only when you choose Apply changes."
        } else {
            "Choose a source and display, then Enable display."
        });
        ui.separator();
        let topic_label = ui.label("1. Choose a topic");
        let choices = self.discovery.topics.clone();
        let mut selected = self.discovery.topic.clone();
        let mut topic_chosen = false;
        let topic_response = egui::ComboBox::from_id_salt("topic-choice")
            .selected_text(if selected.is_empty() {
                "Select topic"
            } else {
                &selected
            })
            .width((ui.available_width() - 15.0).max(1.0))
            .truncate()
            .show_ui(ui, |ui| {
                for (name, _) in &choices {
                    if ui
                        .add(egui::Button::new(name).selected(selected == *name).wrap())
                        .on_hover_text(name)
                        .clicked()
                    {
                        selected = name.clone();
                        topic_chosen = true;
                        ui.memory_mut(|memory| memory.close_popup());
                    }
                }
            })
            .response
            .labelled_by(topic_label.id)
            .on_hover_text(&selected);
        describe_choice(&topic_response, "Topic", &selected);
        if topic_chosen {
            topic_response.request_focus();
        }
        if selected != self.discovery.topic {
            self.discovery.select(&selected);
        }
        ui.add_space(8.0);
        let type_label = ui.label("2. Confirm message type");
        let types = choices
            .iter()
            .find(|(name, _)| name == &self.discovery.topic)
            .map(|(_, types)| types.clone())
            .unwrap_or_default();
        let mut type_chosen = false;
        let type_response = egui::ComboBox::from_id_salt("type-choice")
            .selected_text(if self.discovery.ty.is_empty() {
                "Select type"
            } else {
                &self.discovery.ty
            })
            .width((ui.available_width() - 15.0).max(1.0))
            .truncate()
            .show_ui(ui, |ui| {
                for ty in &types {
                    if ui
                        .add(
                            egui::Button::new(ty)
                                .selected(self.discovery.ty == *ty)
                                .wrap(),
                        )
                        .on_hover_text(ty)
                        .clicked()
                    {
                        self.discovery.ty = ty.clone();
                        type_chosen = true;
                        ui.memory_mut(|memory| memory.close_popup());
                    }
                }
            })
            .response
            .labelled_by(type_label.id)
            .on_hover_text(&self.discovery.ty);
        describe_choice(&type_response, "Message type", &self.discovery.ty);
        if type_chosen {
            // The chosen popup row disappears. Keep keyboard traversal at the
            // selector rather than restarting at the application header.
            type_response.request_focus();
        }
        if types.len() > 1 {
            ui.small("This topic advertises multiple types. Choose one explicitly.");
        }
        if !self.discovery.topic.is_empty() && types.is_empty() {
            ui.label("Topic is no longer discovered. Choose an available topic.");
        }
        ui.add_space(8.0);
        ui.label("3. Choose a display");
        if self
            .draft_output
            .mapping_message_type()
            .is_some_and(|ty| ty != self.discovery.ty)
        {
            self.draft_output = Output::Typed;
        }
        if (self.draft_output == Output::CompactImage
            && self.discovery.ty != images::IMAGE_MESSAGE_TYPE)
            || (self.draft_output == Output::SpatialScan
                && self.discovery.ty != spatial::SCAN_MESSAGE_TYPE)
            || (self.draft_output == Output::PointCloud
                && self.discovery.ty != studio_protocol::cloud::MESSAGE_TYPE)
        {
            self.draft_output = Output::Typed;
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.draft_output, Output::Typed, "Messages");
            // The v6 handshake validates exact renderer capabilities before Connected.
            if self.connection.phase == Phase::Connected
                && self.discovery.ty == images::IMAGE_MESSAGE_TYPE
            {
                ui.selectable_value(&mut self.draft_output, Output::CompactImage, "Image");
            }
            if self.connection.phase == Phase::Connected
                && self.discovery.ty == spatial::SCAN_MESSAGE_TYPE
            {
                ui.selectable_value(&mut self.draft_output, Output::SpatialScan, "Spatial scan");
            }
            if self.connection.phase == Phase::Connected
                && self.discovery.ty == studio_protocol::cloud::MESSAGE_TYPE
            {
                ui.selectable_value(&mut self.draft_output, Output::PointCloud, "Point cloud");
            }
            for (output, label) in [
                (Output::OccupancyGrid, "Map"),
                (Output::Pose, "Pose"),
                (Output::Path, "Path"),
            ] {
                if self.connection.phase == Phase::Connected
                    && output.mapping_message_type() == Some(self.discovery.ty.as_str())
                {
                    ui.selectable_value(&mut self.draft_output, output, label);
                }
            }
        });
        ui.small(if self.draft_output == Output::CompactImage {
            "View bounded mono8/RGB/BGR/RGBA/BGRA frames. Other encodings show an error."
        } else if self.draft_output == Output::SpatialScan {
            "Acquisition-time scan layer in the shared 3D scene."
        } else if self.draft_output.is_mapping() {
            "Read-only mapping overlay in the shared 3D scene."
        } else if self.draft_output == Output::PointCloud {
            "Bounded XYZ point cloud in the shared 3D scene. Up to 8192 source points."
        } else {
            "Inspect the fields supplied by the backend."
        });
        if matches!(
            self.draft_output,
            Output::SpatialScan
                | Output::PointCloud
                | Output::OccupancyGrid
                | Output::Pose
                | Output::Path
        ) {
            let frame_label = ui.label("Fixed frame");
            ui.add(
                studio_ui_foundation::theme::input(ui, &mut self.draft_spatial.fixed_frame)
                    .char_limit(256)
                    .hint_text(studio_ui_foundation::theme::hint(ui, "For example: odom")),
            )
            .labelled_by(frame_label.id);
            ui.checkbox(
                &mut self.draft_clock_verified,
                "I verified these publishers use system time",
            );
            ui.small("Required after each connection. Simulated time is unsupported.");
            ui.collapsing("Transform inputs", |ui| {
            if self.draft_output.is_mapping() {
                let label=ui.label("Dynamic TF topic");
                ui.add(studio_ui_foundation::theme::input(ui, &mut self.draft_spatial.dynamic_tf_topic).char_limit(256)).labelled_by(label.id);
                let label=ui.label("Static TF topic");
                ui.add(studio_ui_foundation::theme::input(ui, &mut self.draft_spatial.static_tf_topic).char_limit(256)).labelled_by(label.id);
            } else {
            tf_topic_choice(
                ui,
                "Dynamic TF topic",
                &mut self.draft_spatial.dynamic_tf_topic,
                &self.discovery.topics,
            );
            tf_topic_choice(
                ui,
                "Static TF topic",
                &mut self.draft_spatial.static_tf_topic,
                &self.discovery.topics,
            );
            }
            ui.checkbox(
                &mut self.draft_spatial.dynamic_tf_best_effort,
                "Dynamic TF best effort",
            );
            ui.small("Static TF requests Reliable + Transient local retained data.");
            ui.small("Scans require transforms at their acquisition time; missing transforms clear the scene.");
            });
        }
        ui.collapsing("Advanced QoS", |ui| {
            ui.checkbox(&mut self.draft_best_effort, "Best effort");
            ui.small("Often used for sensor streams; delivery is not guaranteed.");
            ui.checkbox(&mut self.draft_transient_local, "Transient local");
            ui.small("Request retained data from compatible publishers. Endpoint QoS is not discovered yet.");
        });
        if clock_source
            != (
                self.discovery.topic.clone(),
                self.discovery.ty.clone(),
                self.draft_output.clone(),
                self.draft_spatial.clone(),
            )
        {
            self.draft_clock_verified = false;
        }
        ui.add_space(8.0);
        ui.small("Up to eight native readers, four sensor layers and one Robot. Compatible scene layers share two TF readers.");
        if !self.discovery.valid_selection() {
            ui.label(if self.discovery.topic.is_empty() {
                "Choose a discovered topic to continue."
            } else {
                "Choose one advertised message type to continue."
            });
        }
        if self.connection.phase != Phase::Connected {
            ui.label("Connect before enabling a display.");
        }
        if self.setup_target.is_none() && self.views.len() >= MAX_SUBSCRIPTIONS {
            ui.label("Eight displays configured. Remove a disabled display before adding another.");
        }
        if replacing {
            ui.label("Apply changes replaces this display after cleanup. Other displays continue.");
        } else {
            ui.label("Enable display starts reception and shows it in the center.");
        }
        if let Some(error) = self
            .setup_target
            .and_then(|id| self.views.iter().find(|v| v.id == id))
            .and_then(|v| v.error.as_ref())
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui
            .add_enabled(
                self.connection.phase == Phase::Connected
                    && self.discovery.valid_selection()
                    && (self.setup_target.is_some() || self.views.len() < MAX_SUBSCRIPTIONS)
                    && (!matches!(
                        self.draft_output,
                        Output::SpatialScan
                            | Output::PointCloud
                            | Output::OccupancyGrid
                            | Output::Pose
                            | Output::Path
                    ) || (self.draft_clock_verified
                        && studio_protocol::spatial::valid_frame(&self.draft_spatial.fixed_frame)
                        && self.draft_spatial.dynamic_tf_topic
                            != self.draft_spatial.static_tf_topic
                        && (self.draft_output.is_mapping()
                            || [
                                &self.draft_spatial.dynamic_tf_topic,
                                &self.draft_spatial.static_tf_topic,
                            ]
                            .iter()
                            .all(|name| {
                                self.discovery.topics.iter().any(|(topic, types)| {
                                    topic == *name
                                        && types.iter().any(|ty| ty == "tf2_msgs/msg/TFMessage")
                                })
                            })))),
                shell::primary(
                    ui.visuals(),
                    if replacing {
                        "Apply changes"
                    } else {
                        "Enable display"
                    },
                ),
            )
            .clicked()
        {
            self.subscribe();
        }
        if ui.button("Cancel setup").clicked() {
            ui.memory_mut(|memory| memory.close_popup());
            self.setup = false;
        }
    }
}

fn view_action_name(response: &egui::Response, action: &str, view: &ViewState) {
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            response.enabled(),
            format!(
                "{action}: {} · {}",
                view.configuration.topic,
                view.kind.label()
            ),
        )
    });
}

/// egui's focus traversal can reach controls outside a scroll viewport. Reveal
/// only clipped controls belonging to this content area, without following focus
/// in other panels or recentering a control that is already visible.
fn reveal_focused_control(ui: &egui::Ui) {
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
                    studio_desktop::robot::RobotViewState {
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
        ViewKind::Spatial => {
            ui.add(egui::Label::new(&view.spatial_status).truncate())
                .on_hover_text(&view.spatial_status);
            if let Some(renderer) = renderer {
                view.spatial_camera
                    .show_live_gpu(ui, &view.spatial_scene, renderer);
            } else {
                view.spatial_camera.show_live(ui, &view.spatial_scene);
            }
        }
        ViewKind::PointCloud | ViewKind::Mapping(_) => {
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
                studio_desktop::typed_view::show(ui, &sample.value);
            } else {
                ui.label("Waiting for messages…");
                ui.label("Check the publisher and configured QoS. No message rate or endpoint QoS is inferred.");
            }
        }
    }
}
