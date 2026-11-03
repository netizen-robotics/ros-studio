//! Approved Workbench shell and contextual task pages. Browsing never activates a reader.
use super::*;
use crate::presentation as p;
type Request = Option<(&'static str, Operation)>;
type Navigation = Option<(String, String, String)>;
type Inspector<'a> = dyn FnMut(&mut egui::Ui, &str, &str, InspectionMode, &mut Option<ViewId>) + 'a;

impl Workbench {
    pub(super) fn choose_section(&mut self, section: &str) {
        if section == "Dashboard" {
            self.open = false;
            return;
        }
        self.open = true;
        if self.section != section {
            self.section = section.into();
            self.graph.clear();
            self.refresh_graph = true;
            if let Some(i) = self
                .tasks
                .iter()
                .position(|t| p::section(&t.typ) == section)
            {
                self.selected = i;
            }
        }
    }
    pub(super) fn select_task(&mut self, i: usize) {
        if let Some(t) = self.tasks.get(i) {
            self.selected = i;
            let section = p::section(&t.typ);
            if self.section != section {
                self.section = section.into();
                self.graph.clear();
                self.refresh_graph = true;
            }
        }
    }
    pub fn navigation(&mut self, ui: &mut egui::Ui, compact: bool) {
        let mut chosen = None;
        let mut draw = |ui: &mut egui::Ui| {
            for section in [
                "Topics",
                "Services",
                "Actions",
                "Nodes",
                "Parameters",
                "Dashboard",
            ] {
                let selected = if section == "Dashboard" {
                    !self.open
                } else {
                    self.open && self.section == section
                };
                let response = if compact {
                    p::tab(ui, section, selected)
                } else {
                    p::list_button(
                        ui,
                        section,
                        selected,
                        egui::vec2(ui.available_width(), 36.0),
                    )
                };
                if response.clicked() {
                    chosen = Some(section);
                }
                if !compact {
                    ui.add_space(4.0);
                }
            }
        };
        if compact {
            egui::ScrollArea::horizontal()
                .id_salt("section-navigation")
                .show(ui, |ui| {
                    ui.horizontal(&mut draw);
                });
        } else {
            draw(ui);
        }
        if let Some(section) = chosen {
            self.choose_section(section);
        }
    }
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        session: Option<&Session>,
        generation: u64,
        inspect: &mut Inspector<'_>,
    ) {
        let connected = session.is_some();
        if self.section.is_empty() {
            self.section = "Topics".into();
        }
        if connected && (self.refresh_graph || self.graph_generation != Some(generation)) {
            self.refresh_graph = false;
            self.graph_generation = Some(generation);
            self.send(
                session,
                generation,
                usize::MAX,
                "graph",
                Operation::Graph {
                    section: self.section.clone(),
                    node: self.node.clone(),
                },
            );
        }
        let mut request = None;
        let mut navigation = None;
        let mut tab_action = None;
        let compact = ctx.screen_rect().width() < 900.0;
        if !compact {
            egui::SidePanel::left("task-browser")
                .default_width(224.0)
                .width_range(190.0..=340.0)
                .resizable(true)
                .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(12))
                .show(ctx, |ui| {
                    if let Some(op) = self.browser(ui, connected) {
                        request = Some((usize::MAX, "graph", op));
                    }
                });
        } else {
            egui::TopBottomPanel::top("compact-task-browser").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if p::action(ui, "Browse interfaces", 142.0, true, false).clicked() {
                        self.browser_open = !self.browser_open;
                    }
                    p::muted(ui, &self.section);
                });
            });
            if self.browser_open {
                let mut open = true;
                egui::Window::new("Browse interfaces")
                    .open(&mut open)
                    .default_width(320.0)
                    .show(ctx, |ui| {
                        if let Some(op) = self.browser(ui, connected) {
                            request = Some((usize::MAX, "graph", op));
                        }
                    });
                self.browser_open = open;
            }
        }
        // Standard runtime schema queries are metadata only. Custom libraries still
        // require explicit review/loading and are never restored as live resources.
        if connected {
            if let Some((i, typ, bundle)) = self
                .tasks
                .get_mut(self.selected)
                .filter(|t| {
                    p::section(&t.typ) == self.section
                        && t.fields.is_null()
                        && !t.schema_attempted
                        && t.bundle.is_empty()
                        && t.typ.contains('/')
                })
                .map(|t| {
                    t.schema_attempted = true;
                    (self.selected, t.typ.clone(), t.bundle.clone())
                })
            {
                self.send(
                    session,
                    generation,
                    i,
                    "schema",
                    Operation::Schema {
                        message_type: typ,
                        bundle,
                    },
                );
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(18))
            .show(ctx, |ui| {
                // One source header replaces the mixed global tab strip. The task
                // menu scopes navigation to this section without dropping peers.
                let mut chosen_task = None;
                let current = self
                    .tasks
                    .get(self.selected)
                    .filter(|t| p::section(&t.typ) == self.section);
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 156.0).max(100.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 52.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(width);
                            if let Some(t) = current {
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&t.name).heading())
                                        .truncate(),
                                )
                                .on_hover_text(&t.name);
                                if t.typ.contains('/') {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&t.typ)
                                                .monospace()
                                                .color(crate::theme::muted(ui.visuals())),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&t.typ);
                                }
                            }
                        },
                    );
                    let count = self
                        .tasks
                        .iter()
                        .filter(|t| p::section(&t.typ) == self.section)
                        .count();
                    if count > 0 {
                        ui.menu_button(
                            if count > 1 {
                                format!("Open tasks · {count}")
                            } else {
                                "Task options".into()
                            },
                            |ui| {
                                if count > 1 {
                                    for (i, t) in self
                                        .tasks
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, t)| p::section(&t.typ) == self.section)
                                    {
                                        if p::list_button(
                                            ui,
                                            &t.name,
                                            i == self.selected,
                                            egui::vec2(240.0, 32.0),
                                        )
                                        .on_hover_text(&t.name)
                                        .clicked()
                                        {
                                            chosen_task = Some(i);
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                }
                                for (label, action, enabled) in [
                                    (
                                        "Duplicate inactive task",
                                        "duplicate",
                                        self.tasks.len() < 16,
                                    ),
                                    (
                                        "Close task",
                                        "close",
                                        current.is_some_and(|t| {
                                            t.active.is_none() && t.events.is_none()
                                        }) && !self
                                            .pending
                                            .values()
                                            .any(|(i, _)| *i == self.selected),
                                    ),
                                    (
                                        "Move earlier",
                                        "left",
                                        self.selected > 0 && self.selected < self.tasks.len(),
                                    ),
                                    ("Move later", "right", self.selected + 1 < self.tasks.len()),
                                ] {
                                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                        tab_action = Some(action);
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                    }
                });
                if let Some(i) = chosen_task {
                    self.select_task(i);
                }
                if !self.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &self.error);
                }
                let pending = &self.pending;
                let Some(t) = self
                    .tasks
                    .get_mut(self.selected)
                    .filter(|t| p::section(&t.typ) == self.section)
                else {
                    ui.add_space(24.0);
                    ui.heading(format!("Explore {}", self.section.to_lowercase()));
                    ui.label(if connected {
                        "Choose an interface in the browser to open a task."
                    } else {
                        "Connect to discover interfaces, then choose one to open a task."
                    });
                    return;
                };
                let selected = self.selected;
                let task_pages = pages(&t.typ);
                if task_pages.len() > 1 {
                    if let Some(index) = p::tab_strip(ui, "task-pages", &task_pages, &t.page) {
                        t.page = task_pages[index].into();
                    }
                }
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .id_salt(("task-body", selected))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !connected {
                            p::muted(ui, "Disconnected");
                            ui.add_space(8.0);
                        }
                        ui.push_id(selected, |ui| {
                            let busy = pending.values().any(|(i, k)| {
                                *i == selected
                                    && ["publish", "call", "goal", "parameters"]
                                        .contains(&k.as_str())
                            });
                            if t.typ == "parameters" {
                                if let Some(op) = t.parameters.ui(
                                    ui,
                                    &t.name,
                                    connected,
                                    t.active.is_some() || busy,
                                    t.events,
                                ) {
                                    let kind = match op {
                                        Operation::ParameterEvents { .. } => "events",
                                        Operation::Stop { .. } => "stop",
                                        _ => "parameters",
                                    };
                                    request = Some((selected, kind, op));
                                }
                            } else if t.typ == "node" {
                                node(ui, t, connected, &mut request, selected, &mut navigation);
                            } else if t.page == "Definition" {
                                if let Some((kind, op)) = definition(ui, t, connected) {
                                    request = Some((selected, kind, op));
                                }
                            } else if ["Endpoints", "Providers"].contains(&t.page.as_str()) {
                                if let Some((kind, op)) = endpoints(ui, t, connected) {
                                    request = Some((selected, kind, op));
                                }
                            } else if t.typ.contains("/msg/") && t.page != "Publish" {
                                let mode = match t.page.as_str() {
                                    "Plot" => InspectionMode::Plot,
                                    "Image" => InspectionMode::Image,
                                    _ => InspectionMode::Messages,
                                };
                                let binding = if mode == InspectionMode::Image {
                                    &mut t.image_view
                                } else {
                                    &mut t.message_view
                                };
                                inspect(ui, &t.name, &t.typ, mode, binding);
                                if let Some((kind, op)) = statistics(ui, t, connected) {
                                    request = Some((selected, kind, op));
                                }
                            } else {
                                let is_service = t.typ.contains("/srv/");
                                let is_action = t.typ.contains("/action/");
                                let mut operation = None;
                                if is_service || is_action {
                                    if ui.available_width() >= 640.0 {
                                        ui.columns(2, |cols| {
                                            operation = compose(
                                                &mut cols[0],
                                                t,
                                                connected,
                                                busy,
                                                pending,
                                                selected,
                                            );
                                            result(&mut cols[1], t, is_action);
                                        });
                                    } else {
                                        operation =
                                            compose(ui, t, connected, busy, pending, selected);
                                        ui.separator();
                                        result(ui, t, is_action);
                                    }
                                } else {
                                    operation = compose(ui, t, connected, busy, pending, selected);
                                }
                                if let Some((kind, op)) = operation {
                                    request = Some((selected, kind, op));
                                }
                            }
                        });
                        if !t.history.is_empty() {
                            ui.add_space(18.0);
                            ui.separator();
                            ui.collapsing(
                                format!("Operation history · {}", t.history.len()),
                                |ui| {
                                    for (i, (state, value)) in t.history.iter().enumerate().rev() {
                                        ui.push_id(i, |ui| {
                                            ui.collapsing(state, |ui| {
                                                json_value(ui, value);
                                            });
                                        });
                                    }
                                },
                            );
                        }
                    });
            });
        if let Some(action) = tab_action {
            self.tab_action(action);
        }
        if let Some((section, name, typ)) = navigation {
            self.choose_section(&section);
            if let Some(i) = self
                .tasks
                .iter()
                .position(|t| t.name == name && t.typ == typ)
            {
                self.selected = i;
            } else if self.tasks.len() < 16 {
                self.tasks.push(Task::new(name, typ));
                self.selected = self.tasks.len() - 1;
            }
        }
        if let Some((task, kind, op)) = request {
            self.send(session, generation, task, kind, op);
        }
    }
}
fn pages(typ: &str) -> Vec<&'static str> {
    match p::section(typ) {
        "Parameters" => vec!["Values"],
        "Nodes" => vec!["Relationships"],
        "Services" => vec!["Request", "Providers", "Definition"],
        "Actions" => vec!["Goal", "Definition"],
        _ => {
            let mut pages = vec!["Messages", "Plot"];
            if typ == "sensor_msgs/msg/Image" {
                pages.insert(0, "Image");
            }
            pages.extend(["Publish", "Endpoints", "Definition"]);
            pages
        }
    }
}
fn schema_op(t: &Task) -> Operation {
    Operation::Schema {
        message_type: t.typ.clone(),
        bundle: t.bundle.clone(),
    }
}
fn definition(ui: &mut egui::Ui, t: &mut Task, connected: bool) -> Request {
    let mut request = None;
    ui.horizontal(|ui| {
        p::section_heading(ui, "Interface definition");
        if p::action(
            ui,
            "Load schema",
            118.0,
            connected && t.active.is_none(),
            false,
        )
        .clicked()
        {
            request = Some(("schema", schema_op(t)));
        }
    });
    if !t.definition.is_null() {
        for (label, key) in [
            ("Editable fields", "fields"),
            ("Response", "response_fields"),
            ("Feedback", "feedback_fields"),
            ("Result", "result_fields"),
        ] {
            if t.definition[key].is_array() {
                ui.add_space(10.0);
                studio_ui_foundation::theme::strong(ui, label);
                let text = schema_text(&t.definition[key], 0);
                ui.add(
                    egui::Label::new(egui::RichText::new(text).monospace())
                        .selectable(true)
                        .wrap(),
                );
            }
        }
        ui.collapsing("Native schema metadata", |ui| json_value(ui, &t.definition));
    } else {
        p::muted(ui, "Load schema to view fields.");
    }
    ui.add_space(14.0);
    ui.collapsing("Custom interface bundle", |ui| {
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut t.bundle)
                .hint_text(studio_ui_foundation::theme::hint(ui, "Bundle path"))
                .desired_width(f32::INFINITY),
        )
        .on_hover_text("Compatible prebuilt bundle; validated when loaded.");
    });
    if !t.error.is_empty() {
        ui.colored_label(ui.visuals().error_fg_color, &t.error);
    }
    request
}
fn endpoints(ui: &mut egui::Ui, t: &mut Task, connected: bool) -> Request {
    let mut request = None;
    ui.horizontal(|ui| {
        studio_ui_foundation::theme::strong(
            ui,
            if t.typ.contains("/msg/") {
                "Endpoints and QoS"
            } else {
                "Providers"
            },
        );
        if p::action(ui, "Refresh endpoints", 150.0, connected, false).clicked() {
            request = Some((
                "details",
                if t.typ.contains("/srv/") {
                    Operation::ServiceInfo {
                        service: t.name.clone(),
                    }
                } else if t.typ.contains("/action/") {
                    Operation::ServiceInfo {
                        service: format!("{}/_action/send_goal", t.name),
                    }
                } else {
                    Operation::Details {
                        topic: t.name.clone(),
                    }
                },
            ));
        }
    });
    if t.details.is_null() {
        p::muted(ui, "Refresh to load endpoints.");
    } else {
        for (key, title) in [
            ("publishers", "Publishers"),
            ("subscribers", "Subscribers"),
            ("providers", "Providers"),
        ] {
            if let Some(rows) = t.details[key].as_array() {
                p::section_heading(ui, &format!("{title} · {}", rows.len()));
                if rows.is_empty() {
                    p::muted(ui, "None discovered");
                }
                for (index, row) in rows.iter().enumerate() {
                    ui.push_id((key, index), |ui| {
                        let name = row["node"]
                            .as_str()
                            .or_else(|| row["name"].as_str())
                            .unwrap_or("Unknown node");
                        let namespace = row["namespace"].as_str().unwrap_or("/");
                        let full_name = format!("{}/{name}", namespace.trim_end_matches('/'));
                        ui.add(egui::Label::new(&full_name).truncate())
                            .on_hover_text(&full_name);
                        if let Some(typ) = row["type"].as_str() {
                            p::muted(ui, typ);
                        }
                        if let Some(qos) = row["qos"].as_str() {
                            ui.collapsing("QoS", |ui| {
                                ui.add(egui::Label::new(qos).wrap());
                            });
                        }
                        ui.separator();
                    });
                }
            }
        }
        ui.collapsing("Raw metadata", |ui| json_value(ui, &t.details));
    }
    request
}
fn compose(
    ui: &mut egui::Ui,
    t: &mut Task,
    connected: bool,
    busy: bool,
    pending: &BTreeMap<u64, (usize, String)>,
    selected: usize,
) -> Request {
    let mut request = None;
    let service = t.typ.contains("/srv/");
    let action = t.typ.contains("/action/");
    ui.horizontal(|ui| {
        studio_ui_foundation::theme::strong(
            ui,
            if service {
                "Request"
            } else if action {
                "Goal draft"
            } else {
                "Publish draft"
            },
        );
        if p::action(
            ui,
            "Load schema",
            118.0,
            connected && t.active.is_none(),
            false,
        )
        .clicked()
        {
            request = Some(("schema", schema_op(t)));
        }
    });
    if t.fields.is_null() {
        if !t.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &t.error);
            if ui.button("Choose interface bundle…").clicked() {
                t.page = "Definition".into();
            }
            p::muted(ui, "Choose a compatible bundle under Definition → Custom interface bundle, then retry Load schema.");
        } else {
            p::muted(ui, "Load schema to edit fields.");
        }
        return request;
    }
    if let Some(index) = p::tab_strip(
        ui,
        "draft-format",
        &["Typed form", "YAML"],
        if t.yaml_mode { "YAML" } else { "Typed form" },
    ) {
        t.yaml_mode = index == 1;
    }
    if t.yaml_mode {
        ui.add(
            egui::TextEdit::multiline(&mut t.yaml)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(8),
        );
        t.sync_yaml();
    } else {
        merge_defaults(&t.defaults, &mut t.value);
        if form(ui, &mut t.value, &t.fields, "", &mut t.inputs) {
            t.yaml = serde_yaml::to_string(&t.value).unwrap_or_default();
        }
        t.error = if t.inputs.is_empty() {
            String::new()
        } else {
            "Correct invalid fields before sending".into()
        };
    }
    if let Err(e) = parse_yaml(&t.yaml).and_then(|v| crate::draft::validate(&v, &t.fields)) {
        t.error = e;
    }
    if t.loaded_source != Some((t.typ.clone(), t.bundle.clone())) {
        t.error = "Interface source changed; load its schema explicitly".into();
    }
    if t.error.is_empty() {
        if let Err(e) = crate::draft::validate(&t.value, &t.fields) {
            t.error = e;
        }
    }
    if !t.error.is_empty() {
        ui.add_sized(
            [ui.available_width(), 24.0],
            egui::Label::new(egui::RichText::new(&t.error).color(ui.visuals().error_fg_color))
                .truncate(),
        )
        .on_hover_text(&t.error);
    }
    let enabled = connected && t.active.is_none() && t.error.is_empty() && !busy;
    if action {
        ui.horizontal_wrapped(|ui| {
            if p::action(ui, "Send goal", 118.0, enabled, true).clicked() {
                request = Some((
                    "goal",
                    Operation::Goal {
                        action: t.name.clone(),
                        message_type: t.typ.clone(),
                        bundle: t.bundle.clone(),
                        value: t.value.clone(),
                        timeout_ms: t.timeout,
                    },
                ));
            }
            if p::action(
                ui,
                "Cancel this goal",
                144.0,
                connected
                    && t.active.is_some()
                    && t.accepted
                    && !pending
                        .values()
                        .any(|(i, k)| *i == selected && k == "cancel"),
                false,
            )
            .clicked()
            {
                request = Some((
                    "cancel",
                    Operation::Cancel {
                        target: t.active.unwrap(),
                    },
                ));
            }
        });
        if p::action(ui, "Stop tracking", 132.0, t.active.is_some(), false).clicked() {
            request = Some((
                "stop",
                Operation::Stop {
                    target: t.active.unwrap(),
                },
            ));
        }
        if t.active.is_some() {
            p::muted(ui, "Stop tracking leaves the remote goal running.");
        }
    } else if service {
        ui.horizontal_wrapped(|ui| {
            if p::action(ui, "Call service", 122.0, enabled, true).clicked() {
                t.started = Some(Instant::now());
                request = Some((
                    "call",
                    Operation::Call {
                        service: t.name.clone(),
                        message_type: t.typ.clone(),
                        bundle: t.bundle.clone(),
                        value: t.value.clone(),
                        timeout_ms: t.timeout,
                    },
                ));
            }
            if p::action(ui, "Stop waiting", 122.0, t.active.is_some(), false).clicked() {
                request = Some((
                    "stop",
                    Operation::Stop {
                        target: t.active.unwrap(),
                    },
                ));
            }
        });
        if t.active.is_some() {
            p::muted(ui, "Stop waiting does not cancel the remote request.");
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            for (label, period) in [("Publish once", 0), ("Start publishing", t.period)] {
                if p::action(ui, label, 138.0, enabled, period == 0).clicked() {
                    t.started = Some(Instant::now());
                    request = Some((
                        "publish",
                        Operation::Publish {
                            topic: t.name.clone(),
                            message_type: t.typ.clone(),
                            bundle: t.bundle.clone(),
                            value: t.value.clone(),
                            interval_ms: period,
                            best_effort: t.best,
                            transient_local: t.transient,
                        },
                    ));
                }
            }
            if p::action(
                ui,
                "Stop publishing",
                138.0,
                t.active.is_some() && t.active_kind == "publish",
                false,
            )
            .clicked()
            {
                request = Some((
                    "stop",
                    Operation::Stop {
                        target: t.active.unwrap(),
                    },
                ));
            }
        });
        if let Some((state, value)) = t.history.back() {
            ui.label(state);
            json_value(ui, value);
        }
    }
    ui.add_space(12.0);
    ui.collapsing(
        if service || action {
            "Timeout and draft options"
        } else {
            "QoS, repeat and draft options"
        },
        |ui| {
            if service || action {
                ui.horizontal(|ui| {
                    ui.label("Discovery / response timeout (ms)");
                    ui.add(egui::DragValue::new(&mut t.timeout).range(1..=60_000));
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut t.best, "Best effort");
                    ui.checkbox(&mut t.transient, "Transient local");
                    ui.label("Repeat period (ms)");
                    ui.add(egui::DragValue::new(&mut t.period).range(10..=3_600_000));
                });
            }
            if ui.button("Reset draft to defaults").clicked() {
                t.value = t.defaults.clone();
                t.yaml = serde_yaml::to_string(&t.value).unwrap_or_default();
                t.inputs.clear();
                t.error.clear();
            }
        },
    );
    request
}
fn result(ui: &mut egui::Ui, t: &Task, action: bool) {
    studio_ui_foundation::theme::strong(ui, if action { "Goal tracking" } else { "Response" });
    ui.add_space(8.0);
    if let Some((state, value)) = t.history.back() {
        ui.label(egui::RichText::new(state).color(ui.visuals().hyperlink_color));
        if let Some(ms) = value["elapsed_ms"].as_u64() {
            p::muted(ui, format!("Elapsed {ms} ms"));
        }
        if action {
            ui.add_space(8.0);
            for (state, _) in current_goal_events(t)
                .filter(|(state, _)| state != "feedback")
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                ui.label(state);
            }
            if let Some((_, feedback)) = current_goal_events(t).rev().find(|(s, _)| s == "feedback")
            {
                ui.add_space(12.0);
                studio_ui_foundation::theme::strong(ui, "Latest feedback");
                json_value(ui, &feedback["feedback"]);
                if let Some(drops) = feedback["coalesced_feedback"].as_u64() {
                    p::muted(ui, format!("Coalesced feedback: {drops}"));
                }
            }
            if !value["result"].is_null() {
                ui.add_space(12.0);
                studio_ui_foundation::theme::strong(ui, "Result");
                json_value(ui, &value["result"]);
            }
            if let Some(meaning) = value["meaning"].as_str() {
                ui.label(meaning);
            }
            if let Some(error) = value["error"].as_str() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.collapsing("Goal identity and latest event", |ui| {
                if let Some(id) = current_goal_events(t).rev().find_map(|(_, v)| goal_uuid(v)) {
                    ui.monospace(id);
                }
                json_value(ui, value);
            });
        } else {
            let content = if !value["response"].is_null() {
                &value["response"]
            } else {
                value
            };
            json_value(ui, content);
        }
    } else {
        p::muted(
            ui,
            if action {
                "No goal sent."
            } else {
                "No response yet."
            },
        );
    }
}
fn current_goal_events(t: &Task) -> impl DoubleEndedIterator<Item = &(String, Value)> {
    let start = t
        .history
        .iter()
        .rposition(|(state, _)| state == "waiting for action server" || state == "error")
        .unwrap_or(0);
    t.history.iter().skip(start)
}
fn goal_uuid(value: &Value) -> Option<String> {
    let bytes = value["goal_id"]["uuid"].as_array()?;
    if bytes.len() != 16 {
        return None;
    }
    bytes
        .iter()
        .map(|byte| {
            u8::try_from(byte.as_u64()?)
                .ok()
                .map(|byte| format!("{byte:02x}"))
        })
        .collect()
}
fn statistics(ui: &mut egui::Ui, t: &mut Task, connected: bool) -> Request {
    let mut request = None;
    ui.collapsing("Received rate and serialized bandwidth",|ui|{
        ui.horizontal_wrapped(|ui|{
            if p::action(ui,"Start measuring",152.0,connected&&t.active.is_none(),false).clicked(){request=Some(("statistics",Operation::Statistics{topic:t.name.clone(),message_type:t.typ.clone(),bundle:t.bundle.clone(),best_effort:t.best,transient_local:t.transient}));}
            if p::action(ui,"Stop measuring",152.0,t.active.is_some() && t.active_kind == "statistics",false).clicked(){request=Some(("stop",Operation::Stop{target:t.active.unwrap()}));}
        });
        if let Some((_,v))=t.history.iter().rev().find(|(s,_)|s=="statistics"){json_value(ui,v);}
        ui.small("Received traffic · serialized bytes").on_hover_text("Traffic received by Tools. Bytes exclude transport overhead; counters do not measure network-wide delivery.");
    });
    request
}
fn node(
    ui: &mut egui::Ui,
    t: &mut Task,
    connected: bool,
    request: &mut Option<(usize, &'static str, Operation)>,
    selected: usize,
    navigation: &mut Navigation,
) {
    ui.horizontal_wrapped(|ui| {
        if p::action(ui, "Inspect node endpoints", 184.0, connected, false).clicked() {
            *request = Some((
                selected,
                "node",
                Operation::NodeInfo {
                    node: t.name.clone(),
                },
            ));
        }
        if ui.button("Open parameters").clicked() {
            *navigation = Some(("Parameters".into(), t.name.clone(), "parameters".into()));
        }
    });
    if t.details.is_null() {
        p::muted(ui, "Inspect node endpoints to view relationships.");
        return;
    }
    ui.label(format!("Observed instances: {}", t.details["instances"]));
    if let Some(text) = t.details["ambiguity"].as_str() {
        ui.label(text);
    }
    ui.add_space(14.0);
    for (category, section, label) in [
        ("publishers", "Topics", "Published topics"),
        ("subscribers", "Topics", "Subscribed topics"),
        ("service_servers", "Services", "Service servers"),
        ("service_clients", "Services", "Service clients"),
        ("action_servers", "Actions", "Action servers"),
        ("action_clients", "Actions", "Action clients"),
    ] {
        let rows = t.details[category].as_object();
        let count = rows.map_or(0, |rows| rows.len());
        egui::CollapsingHeader::new(format!("{label} · {count}"))
            .id_salt(category)
            .default_open(count > 0)
            .show(ui, |ui| {
                if let Some(rows) = rows {
                    for (name, types) in rows {
                        for typ in types
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            ui.vertical(|ui| {
                                if p::list_button(
                                    ui,
                                    format!("{name}\n{typ}"),
                                    false,
                                    egui::vec2(ui.available_width(), 50.0),
                                )
                                .on_hover_text(format!("{name}\n{typ}"))
                                .clicked()
                                {
                                    *navigation = Some((section.into(), name.clone(), typ.into()));
                                }
                            });
                        }
                    }
                }
            });
    }
}
fn json_value(ui: &mut egui::Ui, value: &Value) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(serde_json::to_string_pretty(value).unwrap_or_default())
                .monospace(),
        )
        .selectable(true)
        .wrap(),
    );
}
fn schema_text(fields: &Value, depth: usize) -> String {
    if depth > 16 {
        return "Nesting limit reached\n".into();
    }
    let mut out = String::new();
    for f in fields.as_array().into_iter().flatten() {
        let kind = match f["kind"].as_u64().unwrap_or(0) {
            1 => "float32",
            2 => "float64",
            3 => "long double (unsupported)",
            4 => "char",
            5 => "wchar (unsupported)",
            6 => "bool",
            7 => "octet",
            8 => "uint8",
            9 => "int8",
            10 => "uint16",
            11 => "int16",
            12 => "uint32",
            13 => "int32",
            14 => "uint64",
            15 => "int64",
            16 => "string",
            17 => "wstring (unsupported)",
            18 => "message",
            _ => "unsupported",
        };
        let bound = f["bound"].as_u64().unwrap_or(0);
        let suffix = match f["array"].as_u64().unwrap_or(0) {
            1 => format!("[{bound}]"),
            2 => {
                if bound > 0 {
                    format!("[<={bound}]")
                } else {
                    "[]".into()
                }
            }
            _ => String::new(),
        };
        out += &format!(
            "{}{}{} {}",
            "  ".repeat(depth),
            kind,
            suffix,
            f["name"].as_str().unwrap_or("?")
        );
        if f["string_bound"].as_u64().unwrap_or(0) > 0 {
            out += &format!(" (string bound {})", f["string_bound"]);
        }
        out.push('\n');
        if f["kind"] == 18 {
            out += &schema_text(&f["fields"], depth + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_excludes_task_drafts_payloads_queries_and_bundle_paths() {
        let mut w = Workbench::fresh();
        w.query = "PRIVATE_SEARCH".into();
        let mut t = Task::new("/fixture/topic".into(), "std_msgs/msg/String".into());
        t.value = json!({"data":"PRIVATE_MESSAGE"});
        t.yaml = "data: PRIVATE_DRAFT".into();
        t.bundle = "/private/PRIVATE_BUNDLE".into();
        t.history
            .push_back(("response".into(), json!("PRIVATE_RESPONSE")));
        t.inputs.insert("data".into(), "PRIVATE_INPUT".into());
        w.tasks.push(t);
        let snapshot = w.diagnostic_state().to_string();
        assert!(snapshot.contains("/fixture/topic"));
        assert!(!snapshot.contains("PRIVATE_"));
    }
    #[test]
    fn goal_identity_is_exact_and_rejects_malformed_native_bytes() {
        let bytes = (0..16).collect::<Vec<_>>();
        assert_eq!(
            goal_uuid(&json!({"goal_id":{"uuid":bytes}})).as_deref(),
            Some("000102030405060708090a0b0c0d0e0f")
        );
        assert!(goal_uuid(&json!({"goal_id":{"uuid":[0,1]}})).is_none());
        assert!(goal_uuid(&json!({"goal_id":{"uuid":vec![256;16]}})).is_none());
    }
    #[test]
    fn new_goal_does_not_show_previous_goals_feedback_or_identity() {
        let mut t = Task::new(
            "/fixture/goal".into(),
            "example_interfaces/action/Fibonacci".into(),
        );
        t.history.push_back((
            "waiting for action server".into(),
            json!({"goal_id":{"uuid":vec![1;16]}}),
        ));
        t.history
            .push_back(("feedback".into(), json!({"feedback":{"sequence":[0,1]}})));
        t.history
            .push_back(("succeeded".into(), json!({"result":{"sequence":[0,1]}})));
        t.history.push_back((
            "waiting for action server".into(),
            json!({"goal_id":{"uuid":vec![2;16]}}),
        ));
        assert!(!current_goal_events(&t).any(|(state, _)| state == "feedback"));
        assert_eq!(
            current_goal_events(&t)
                .find_map(|(_, value)| goal_uuid(value))
                .as_deref(),
            Some("02020202020202020202020202020202")
        );
        assert_eq!(t.history.len(), 4, "full history remains available");
    }
    #[test]
    fn returning_to_the_same_tab_index_selects_its_section_without_replacing_owner() {
        let mut w = Workbench::fresh();
        let mut topic = Task::new("/fixture/text".into(), "std_msgs/msg/String".into());
        topic.active = Some(100);
        topic.active_kind = "publish".into();
        topic.message_view = Some(ViewId(9));
        topic.value = json!({"data":"retained draft"});
        w.tasks.push(topic);
        w.selected = 0;
        w.pending.insert(100, (0, "publish".into()));
        w.choose_section("Services");
        assert_eq!(w.selected, 0);
        w.select_task(0);
        assert_eq!(w.section, "Topics");
        assert_eq!(w.tasks[0].active, Some(100));
        assert_eq!(w.tasks[0].message_view, Some(ViewId(9)));
        assert_eq!(w.pending[&100], (0, "publish".into()));
        assert_eq!(w.tasks[0].value["data"], "retained draft");
    }
    #[test]
    fn pages_and_native_definition_preserve_exact_types_and_interface_bounds() {
        let fields = json!([{"name":"counter","kind":14},{"name":"samples","kind":15,"array":2,"bound":4},{"name":"label","kind":16,"string_bound":8}]);
        let text = schema_text(&fields, 0);
        assert!(text.contains("uint64 counter"));
        assert!(text.contains("int64[<=4] samples"));
        assert!(text.contains("string bound 8"));
        assert!(pages("sensor_msgs/msg/Image").contains(&"Image"));
        assert!(!pages("std_msgs/msg/String").contains(&"Image"));
    }
    #[test]
    fn all_context_pages_render_without_outgoing_commands_from_navigation() {
        let ctx = egui::Context::default();
        let mut w = Workbench::fresh();
        for typ in [
            "std_msgs/msg/String",
            "example_interfaces/srv/AddTwoInts",
            "example_interfaces/action/Fibonacci",
            "node",
            "parameters",
        ] {
            w.tasks
                .push(Task::new("/fixture/source".into(), typ.into()));
        }
        for i in 0..w.tasks.len() {
            w.select_task(i);
            for page in pages(&w.tasks[i].typ) {
                w.tasks[i].page = page.into();
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 650.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        w.ui(ctx, None, 0, &mut |ui, _, _, _, _| {
                            ui.label("Inactive inspection owner");
                        })
                    },
                );
                assert!(w.pending.is_empty());
                assert!(w.tasks.iter().all(|t| t.active.is_none()));
            }
        }
    }

    #[test]
    fn valid_goal_is_available_when_connected_but_rendering_never_sends_it() {
        for connected in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut task = Task::new(
                "/studio_fixture/action".into(),
                "example_interfaces/action/Fibonacci".into(),
            );
            task.fields = json!([{"name":"order","kind":13,"array":0}]);
            task.value = json!({"order":5});
            task.defaults = task.value.clone();
            task.yaml = "order: 5\n".into();
            task.loaded_source = Some((task.typ.clone(), String::new()));
            let mut requested = None;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 700.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        requested = compose(ui, &mut task, connected, false, &BTreeMap::new(), 0);
                    });
                },
            );
            assert!(requested.is_none(), "A repaint must never issue a goal");
            let tree = output.platform_output.accesskit_update.unwrap();
            let button = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Send goal"))
                .expect("Explicit Send goal remains available");
            assert_eq!(button.1.is_disabled(), !connected);
            assert!(!tree
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Enable interaction")));
        }
    }
}
