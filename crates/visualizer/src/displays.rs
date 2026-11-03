//! Topic-oriented observation with optional per-layer configuration.
use super::*;
use studio_visualization::catalog::{self, DISPLAYS};

impl App {
    pub(super) fn select_display(&mut self, id: ViewId) {
        if self.workspace_frozen() {
            return;
        }
        if self.setup && self.setup_target != Some(id) {
            self.diagnostic(
                "Apply or discard the open display draft before selecting another display.".into(),
            );
            return;
        }
        self.selected = Some(id);
        self.details_open = true;
        self.compact_inspector_open = true;
        if let Some(view) = self.views.iter().find(|v| v.id == id) {
            self.expanded_displays.insert(view.workspace_id.clone());
        }
        self.catalog_open = false;
        self.scene_settings.open = false;
    }
    /// Active layers remain reachable while searching the available graph.
    pub(super) fn displays_sidebar_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        ui.horizontal(|ui| {
            for (label, scene) in [("Topics", false), ("Scene", true)] {
                let selected = self.global_options_open == scene;
                if ui
                    .add_sized(
                        [
                            (ui.available_width() / if scene { 1.0 } else { 2.0 }) - 4.0,
                            shell::CONTROL_HEIGHT,
                        ],
                        shell::selectable(ui.visuals(), label, selected),
                    )
                    .clicked()
                {
                    self.global_options_open = scene;
                    self.topics_tab = !scene;
                }
            }
        });
        ui.separator();
        if self.global_options_open {
            self.scene_settings_ui(ui);
        } else {
            self.topics_ui_with_height(ui, frozen, (ui.available_height() - 112.0).max(0.0));
        }
    }
    pub(super) fn topics_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        self.topics_ui_with_height(ui, frozen, (ui.available_height() - 100.0).max(60.0));
    }
    fn topics_ui_with_height(&mut self, ui: &mut egui::Ui, frozen: bool, height: f32) {
        let search_label = ui
            .horizontal(|ui| {
                let label = studio_ui_foundation::theme::strong(ui, "Available topics");
                ui.menu_button(
                    if self.show_all_topics || self.topic_grouped {
                        "Topic filters •"
                    } else {
                        "Topic filters"
                    },
                    |ui| {
                        ui.checkbox(&mut self.show_all_topics, "Include nonvisual topics");
                        ui.checkbox(&mut self.topic_grouped, "Group by namespace");
                    },
                );
                if ui
                    .add_enabled(
                        self.connection.phase == Phase::Connected
                            && self.topic_workflow.refresh.is_none()
                            && !frozen,
                        egui::Button::new(if self.topic_workflow.refresh.is_some() {
                            "Refreshing…"
                        } else {
                            "Refresh"
                        }),
                    )
                    .clicked()
                {
                    self.refresh_topics();
                }
                label
            })
            .inner;
        if let Some(error) = &self.topic_action_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.topic_query)
                .char_limit(256)
                .hint_text(studio_ui_foundation::theme::hint(
                    ui,
                    "Search available topics…",
                )),
        )
        .labelled_by(search_label.id);
        let query = self.topic_query.to_lowercase();
        let mut tree = TopicBranch::default();
        let mut sources = Vec::new();
        let mut unique = std::collections::BTreeMap::<String, Vec<String>>::new();
        for (topic, types) in &self.discovery.topics {
            let existing = unique.entry(topic.clone()).or_default();
            for ty in types {
                if !existing.contains(ty) {
                    existing.push(ty.clone());
                }
            }
        }
        let mut available: Vec<_> = unique.into_iter().collect();
        for view in &self.views {
            let topic = view.source_label().to_owned();
            let ty = crate::panels::source_type_label(view).to_owned();
            if !topic.starts_with('/') {
                continue;
            }
            if let Some((_, types)) = available.iter_mut().find(|(name, _)| name == &topic) {
                if !types.contains(&ty) {
                    types.push(ty);
                }
            } else {
                available.push((topic, vec![ty]));
            }
        }
        available.sort_by(|a, b| a.0.cmp(&b.0));
        for (topic, types) in &available {
            if !query.is_empty()
                && !topic.to_lowercase().contains(&query)
                && !types.iter().any(|ty| ty.to_lowercase().contains(&query))
            {
                continue;
            }
            let matching: Vec<_> = types
                .iter()
                .filter(|ty| self.show_all_topics || !topic_choices(topic, ty).is_empty())
                .cloned()
                .collect();
            if matching.is_empty() {
                continue;
            }
            if self.topic_grouped {
                let mut node = &mut tree;
                for segment in topic.split('/').filter(|s| !s.is_empty()) {
                    node = node.children.entry(segment.to_owned()).or_default();
                }
                node.topic = Some((topic.clone(), matching));
            } else {
                sources.push((topic.clone(), matching));
            }
        }
        let mut source = None;
        let mut inspect = None;
        let views = (self.views.as_slice(), self.selected);
        ui.add_enabled_ui(!frozen, |ui| {
            let scroll = egui::ScrollArea::vertical()
                .id_salt("available-topics-list")
                .max_height(height)
                .min_scrolled_height(0.0);
            if self.topic_grouped {
                scroll.show(ui, |ui| {
                    topic_tree(
                        ui,
                        &tree,
                        "",
                        &mut source,
                        !query.is_empty(),
                        views,
                        &mut inspect,
                    );
                });
            } else {
                let row_height = shell::COMPACT_HEIGHT
                    + 2.0 * ui.text_style_height(&egui::TextStyle::Small)
                    + 2.0 * ui.spacing().item_spacing.y
                    + 20.0;
                scroll.show_rows(ui, row_height, sources.len(), |ui, range| {
                    for (topic, types) in &sources[range] {
                        topic_row(ui, topic, types, &mut source, views, &mut inspect);
                    }
                });
            }
        });
        if sources.is_empty() && tree.children.is_empty() {
            ui.small(if self.connection.phase != Phase::Connected {
                "Connect to discover topics."
            } else if !self.discovery.received_graph {
                "Discovering topics…"
            } else {
                "No matching topics."
            });
        }
        if let Some((output, topic, ty)) = source {
            self.toggle_topic(output, &topic, &ty);
        }
        if let Some((output, topic, ty)) = inspect {
            self.inspect_topic(output, &topic, &ty);
        }
    }
    pub(super) fn catalog_ui(&mut self, ui: &mut egui::Ui) {
        ui.small("Show ordinary topics directly in Available topics. Choose a display type here for other configurations.");
        studio_ui_foundation::theme::strong(ui, "Display types and overlays");
        ui.small("Topic Show starts reception. Advanced source drafts use Add or Apply.");
        ui.horizontal(|ui| {
            if ui
                .selectable_label(!self.catalog_by_type, "By topic")
                .clicked()
            {
                self.catalog_by_type = false;
                self.catalog_output = None;
            }
            if ui
                .selectable_label(self.catalog_by_type, "By display type")
                .clicked()
            {
                self.catalog_by_type = true;
                self.catalog_output = None;
            }
        });
        if !self.catalog_by_type {
            self.topics_ui(ui, false);
            if ui.button("Back to topics").clicked() {
                self.catalog_open = false;
            }
            return;
        }
        let label = ui.small("Search topics, types or displays");
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.catalog_query)
                .char_limit(256)
                .hint_text(studio_ui_foundation::theme::hint(ui, "Find a source…")),
        )
        .labelled_by(label.id);
        let query = self.catalog_query.to_lowercase();
        let mut robot = false;
        let mut tf_graph = false;
        if self.catalog_by_type {
            for descriptor in DISPLAYS.iter() {
                if !query.is_empty()
                    && !descriptor.label.to_lowercase().contains(&query)
                    && !descriptor.description.to_lowercase().contains(&query)
                    && !descriptor
                        .message_type
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                {
                    continue;
                }
                if ui
                    .add(
                        shell::selectable(
                            ui.visuals(),
                            descriptor.label,
                            self.catalog_output.as_ref() == Some(&descriptor.output),
                        )
                        .wrap(),
                    )
                    .on_hover_text(descriptor.description)
                    .clicked()
                {
                    if descriptor.output == Output::TfGraph {
                        tf_graph = true;
                    } else if descriptor.output == Output::Robot {
                        robot = true;
                    } else {
                        self.catalog_output = Some(descriptor.output.clone());
                    }
                }
            }
            ui.separator();
            if let Some(descriptor) = DISPLAYS
                .iter()
                .find(|d| Some(&d.output) == self.catalog_output.as_ref())
            {
                studio_ui_foundation::theme::strong(ui, descriptor.label);
                ui.small(descriptor.description);
            }
        } else if "robot model local urdf description topic file".contains(&query)
            && ui.button("Robot model from topic or File").clicked()
        {
            robot = true;
        }
        if !self.catalog_by_type
            && (query.is_empty() || "tf frames transforms".contains(&query))
            && ui.button("TF frames from shared scene").clicked()
        {
            tf_graph = true;
        }
        let mut source = None;
        let mut count = 0;
        for (topic, types) in &self.discovery.topics {
            for ty in types {
                let descriptor = if self.catalog_by_type {
                    DISPLAYS.iter().find(|d| {
                        Some(&d.output) == self.catalog_output.as_ref()
                            && (d.message_type == Some(ty.as_str())
                                || (d.output == Output::MarkerSnapshot
                                    && ty == studio_protocol::marker::ARRAY_MESSAGE_TYPE))
                    })
                } else {
                    catalog::for_type(ty)
                };
                let Some(descriptor) = descriptor else {
                    continue;
                };
                if self.catalog_by_type && self.catalog_output.as_ref() != Some(&descriptor.output)
                {
                    continue;
                }
                if !self.catalog_by_type
                    && !topic.to_lowercase().contains(&query)
                    && !ty.to_lowercase().contains(&query)
                    && !descriptor.label.to_lowercase().contains(&query)
                {
                    continue;
                }
                count += 1;
                ui.push_id((topic, ty), |ui| {
                    if ui
                        .add(egui::Button::new(format!("{topic}\n{}", descriptor.label)).wrap())
                        .clicked()
                    {
                        source = Some((descriptor.output.clone(), topic.clone(), ty.clone()));
                    }
                    ui.add(egui::Label::new(egui::RichText::new(ty).small()).wrap());
                    if types.len() > 1 {
                        ui.small(
                            "Multiple advertised types; this row selects the exact type shown.",
                        );
                    }
                });
            }
        }
        if count == 0 {
            ui.label(if self.connection.phase != Phase::Connected { "Connect to discover supported topic sources. Local Robot review is available offline." }
                else if !self.discovery.received_graph { "Discovering topics…" }
                else if self.catalog_by_type && self.catalog_output.is_none() { "Choose a display type to see compatible topics." }
                else { "No compatible sources match. Check the search, domain and middleware." });
        }
        if ui.button("Back to topics").clicked() {
            self.catalog_open = false;
        }
        if tf_graph {
            let topic = self.scene_settings.configuration.dynamic_tf_topic.clone();
            self.begin_source(
                Output::TfGraph,
                &topic,
                studio_protocol::spatial_input::TF_MESSAGE_TYPE,
            );
        }
        if robot {
            self.catalog_open = false;
            self.begin_robot_setup(None);
        }
        if let Some((output, topic, ty)) = source {
            self.begin_source(output, &topic, &ty);
        }
    }
}

#[derive(Default)]
struct TopicBranch {
    children: std::collections::BTreeMap<String, TopicBranch>,
    topic: Option<(String, Vec<String>)>,
}
fn display_icon(output: &Output) -> crate::icons::Icon {
    use crate::icons::Icon;
    match output {
        Output::SpatialScan => Icon::Scan,
        Output::PointCloud => Icon::Cloud,
        Output::CompactImage => Icon::Image,
        Output::CameraSnapshot(_) => Icon::Camera,
        Output::Robot | Output::RobotDescription => Icon::Robot,
        Output::TfGraph => Icon::Frames,
        Output::MapSnapshot | Output::OccupancyGrid => Icon::Map,
        Output::Pose => Icon::Pose,
        Output::Path => Icon::Path,
        Output::MarkerSnapshot => Icon::Markers,
        Output::PoseSnapshot(c) => match c.kind {
            studio_protocol::pose_snapshot::Kind::PoseWithCovariance => Icon::Covariance,
            studio_protocol::pose_snapshot::Kind::Odometry => Icon::Odometry,
        },
        Output::Typed => Icon::Document,
    }
}
fn topic_choices(topic: &str, ty: &str) -> Vec<&'static catalog::DisplayDescriptor> {
    catalog::for_type_all(ty)
        .into_iter()
        .filter(|d| {
            d.output != Output::Robot || topic.rsplit('/').next() == Some("robot_description")
        })
        .collect()
}
fn topic_row(
    ui: &mut egui::Ui,
    topic: &str,
    types: &[String],
    source: &mut Option<(Output, String, String)>,
    views: (&[ViewState], Option<ViewId>),
    inspect: &mut Option<(Output, String, String)>,
) {
    let (views, selected) = views;
    ui.push_id(topic, |ui| {
        let choices: Vec<_> = types
            .iter()
            .flat_map(|ty| topic_choices(topic, ty).into_iter().map(move |d| (ty, d)))
            .collect();
        let existing = views
            .iter()
            .find(|v| v.source_label() == topic && v.enabled)
            .or_else(|| {
                views
                    .iter()
                    .find(|v| v.source_label() == topic && Some(v.id) == selected)
            })
            .or_else(|| views.iter().find(|v| v.source_label() == topic));
        let active = existing.is_some_and(|v| v.enabled);
        let palette = *shell::palette(ui.visuals());
        egui::Frame::new()
            .fill(if active {
                palette.selected
            } else {
                palette.raised
            })
            .stroke(egui::Stroke::new(1.0, palette.line))
            .corner_radius(shell::RADIUS)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    choices
                        .first()
                        .map_or(crate::icons::Icon::Document, |(_, d)| {
                            display_icon(&d.output)
                        })
                        .show(ui, "Topic display");
                    let width = (ui.available_width() - 38.0).max(1.0);
                    let name = ui
                        .allocate_ui_with_layout(
                            egui::vec2(width, shell::COMPACT_HEIGHT),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(width);
                                ui.add(
                                    egui::Label::new(topic)
                                        .halign(egui::Align::Min)
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                )
                            },
                        )
                        .inner;
                    name.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, name.enabled(), topic)
                    });
                    if name
                        .on_hover_text(format!("{topic}\n{}", types.join("\n")))
                        .clicked()
                    {
                        if let Some((ty, d)) = choices.first() {
                            *inspect = Some((d.output.clone(), topic.into(), (*ty).clone()));
                        }
                    }
                    let action = if choices.is_empty() {
                        "Unavailable"
                    } else if active {
                        "Hide"
                    } else {
                        "Show"
                    };
                    let busy = existing.is_some_and(|v| {
                        matches!(v.status, ViewStatus::Stopping | ViewStatus::NeedsRecovery)
                    });
                    let eye = ui.add_enabled(
                        !choices.is_empty() && types.len() == 1 && !busy,
                        egui::Button::new("")
                            .selected(active)
                            .min_size(egui::vec2(30.0, shell::COMPACT_HEIGHT)),
                    );
                    eye.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            eye.enabled(),
                            format!("{action} {topic}"),
                        )
                    });
                    studio_ui_foundation::diagnostics::response(ui.ctx(), action, &eye);
                    crate::icons::Icon::Eye.paint(
                        ui,
                        egui::Rect::from_center_size(eye.rect.center(), egui::vec2(18.0, 18.0)),
                        if eye.enabled() {
                            palette.accent
                        } else {
                            palette.muted
                        },
                    );
                    if eye
                        .on_hover_text(format!("{action} {topic}; settings are preserved"))
                        .clicked()
                    {
                        if let Some((ty, d)) = choices.first() {
                            *source = Some((d.output.clone(), topic.into(), (*ty).clone()));
                        }
                    }
                });
                let kind = existing.map_or_else(
                    || {
                        choices
                            .first()
                            .map_or("No visual display", |(_, d)| d.label)
                    },
                    |v| v.kind.label(),
                );
                let detail = if choices.is_empty() {
                    if types.iter().any(|ty| ty == "sensor_msgs/msg/CameraInfo") {
                        "Calibration input · CameraInfo".to_owned()
                    } else {
                        "No visual display · Messages are available in Tools".to_owned()
                    }
                } else {
                    format!("{kind} · {}", types.join(", "))
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(&detail).small().color(palette.muted))
                        .truncate(),
                )
                .on_hover_text(&detail);
                if let Some(view) = existing {
                    let feedback = crate::panels::view_feedback(view);
                    ui.add(
                        egui::Label::new(egui::RichText::new(&feedback).small().color(
                            if view.error.is_some() {
                                palette.danger
                            } else {
                                palette.muted
                            },
                        ))
                        .truncate(),
                    )
                    .on_hover_text(feedback);
                } else {
                    ui.small("Not shown");
                }
            });
        ui.add_space(4.0);
    });
}
fn topic_tree(
    ui: &mut egui::Ui,
    tree: &TopicBranch,
    prefix: &str,
    source: &mut Option<(Output, String, String)>,
    searching: bool,
    views: (&[ViewState], Option<ViewId>),
    inspect: &mut Option<(Output, String, String)>,
) {
    for (segment, branch) in &tree.children {
        let path = format!("{prefix}/{segment}");
        if branch.children.is_empty() {
            if let Some((topic, types)) = &branch.topic {
                topic_row(ui, topic, types, source, views, inspect);
            }
        } else {
            egui::CollapsingHeader::new(segment)
                .id_salt(&path)
                .open(searching.then_some(true))
                .default_open(false)
                .show(ui, |ui| {
                    if let Some((topic, types)) = &branch.topic {
                        topic_row(ui, topic, types, source, views, inspect);
                    }
                    topic_tree(ui, branch, &path, source, searching, views, inspect);
                });
        }
    }
}
