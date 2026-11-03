//! Existing independent inspection owners, presented inside Workbench topic tasks.
use super::*;
use crate::{presentation as p, workbench::InspectionMode};
pub(crate) enum Action {
    Create {
        topic: String,
        typ: String,
        image: bool,
        start: bool,
    },
    Start(ViewId),
    Stop(ViewId),
    Apply(ViewId),
}
pub(crate) fn show(
    ui: &mut egui::Ui,
    views: &mut [ToolView],
    topic: &str,
    typ: &str,
    mode: InspectionMode,
    binding: &mut Option<ViewId>,
    connected: bool,
) -> Option<Action> {
    let mut action = None;
    let image = mode == InspectionMode::Image;
    if binding.is_some_and(|id| {
        !views.iter().any(|v| {
            v.id == id
                && v.saved.topic == topic
                && v.saved.message_type == typ
                && matches!(v.saved.kind, file::ViewKind::Image { .. }) == image
        })
    }) {
        *binding = None;
    }
    // Binding an already existing view is explicit and never replaces its source.
    let candidates = views
        .iter()
        .filter(|v| {
            v.saved.topic == topic
                && v.saved.message_type == typ
                && matches!(v.saved.kind, file::ViewKind::Image { .. }) == image
        })
        .map(|v| (v.id, format!("{} · {}", v.saved.id, v.status.label())))
        .collect::<Vec<_>>();
    if !candidates.is_empty() {
        ui.horizontal(|ui| {
            p::muted(ui, "View");
            egui::ComboBox::from_id_salt((topic, "inspection-owner", image))
                .selected_text(
                    binding
                        .map(|id| format!("View {}", id.0))
                        .unwrap_or_else(|| "Choose a view".into()),
                )
                .show_ui(ui, |ui| {
                    for (id, label) in &candidates {
                        ui.selectable_value(binding, Some(*id), label);
                    }
                });
        });
    }
    let Some(view) = binding.and_then(|id| views.iter_mut().find(|v| v.id == id)) else {
        ui.add_space(10.0);
        studio_ui_foundation::theme::strong(
            ui,
            if image {
                "Image inspection"
            } else if mode == InspectionMode::Plot {
                "Numeric plot"
            } else {
                "Message inspection"
            },
        );
        p::muted(
            ui,
            if mode == InspectionMode::Plot {
                "Receive messages, then choose a numeric field."
            } else {
                "Start receiving to inspect data."
            },
        );
        ui.horizontal_wrapped(|ui| {
            if p::action(
                ui,
                "Start receiving",
                146.0,
                connected && views.len() < 8,
                true,
            )
            .clicked()
            {
                action = Some(Action::Create {
                    topic: topic.into(),
                    typ: typ.into(),
                    image,
                    start: true,
                });
            }
            if p::action(
                ui,
                "Add stopped view",
                146.0,
                connected && views.len() < 8,
                false,
            )
            .clicked()
            {
                action = Some(Action::Create {
                    topic: topic.into(),
                    typ: typ.into(),
                    image,
                    start: false,
                });
            }
        });
        return action;
    };
    let id = view.id;
    let busy = matches!(view.status, Status::Stopping | Status::Recovery);
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(view.status.label()).color(ui.visuals().hyperlink_color));
        let active = view.status.active();
        if p::action(
            ui,
            if active {
                "Stop receiving"
            } else {
                "Start receiving"
            },
            148.0,
            connected && !busy,
            !active,
        )
        .clicked()
        {
            action = Some(if active {
                Action::Stop(id)
            } else {
                Action::Start(id)
            });
        }
    });
    if let Some(error) = &view.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    p::sample_summary(ui, view.samples, view.backend_drops, view.queue_drops);
    ui.add_space(12.0);
    if image {
        view.image.show_details(ui);
        ui.allocate_ui(egui::vec2(ui.available_width(), 320.0), |ui| {
            view.image.show_content_with_guidance(ui, false)
        });
    } else if mode == InspectionMode::Plot {
        if matches!(view.saved.kind, file::ViewKind::NumericPlot { .. }) {
            plots::PlotRenderer::show_details(ui, &view.trace);
            let status = match view.status {
                Status::Stopped => plots::PlotStatus::Stopped,
                Status::Starting => plots::PlotStatus::Starting,
                Status::Waiting => plots::PlotStatus::Waiting,
                Status::Receiving => plots::PlotStatus::Receiving,
                _ => plots::PlotStatus::Error("Stopping or cleanup required"),
            };
            let path = view.saved.kind.plot_path();
            ui.allocate_ui(egui::vec2(ui.available_width(), 300.0), |ui| {
                plots::PlotRenderer.show_content_with_guidance(
                    ui,
                    &mut view.trace,
                    status,
                    path.as_ref(),
                    false,
                )
            });
        } else {
            studio_ui_foundation::theme::strong(ui, "Plot a numeric field");
            plots::PlotRenderer::field_picker(ui, &view.fields, &mut view.field_draft);
            if p::action(
                ui,
                "Open plot",
                122.0,
                !busy && view.field_draft.is_some(),
                true,
            )
            .clicked()
            {
                view.draft = view.saved.clone();
                view.draft.kind = file::ViewKind::numeric_plot(view.field_draft.as_ref().unwrap());
                action = Some(Action::Apply(id));
            }
        }
    } else {
        if matches!(view.saved.kind, file::ViewKind::NumericPlot { .. })
            && ui.button("Switch this view to Messages").clicked()
        {
            view.draft = view.saved.clone();
            view.draft.kind = file::ViewKind::Messages;
            action = Some(Action::Apply(id));
        }
        studio_ui_foundation::theme::strong(ui, "Latest message");
        if let Some(sample) = &view.sample {
            typed_view::show(ui, &sample.value);
        } else {
            p::muted(
                ui,
                if view.status.active() {
                    "Waiting for data…"
                } else {
                    "No samples. Start receiving to inspect data."
                },
            );
        }
    }
    ui.add_space(14.0);
    ui.collapsing("Source and receive QoS", |ui| {
        ui.add_enabled_ui(!busy, |ui| {
            ui.label("Topic");
            ui.add(
                studio_ui_foundation::theme::input(ui, &mut view.draft.topic)
                    .desired_width(f32::INFINITY),
            );
            ui.label("Message type");
            ui.add(
                studio_ui_foundation::theme::input(ui, &mut view.draft.message_type)
                    .desired_width(f32::INFINITY),
            );
            let mut best = view.draft.qos.reliability == file::Reliability::BestEffort;
            let mut retained = view.draft.qos.durability == file::Durability::TransientLocal;
            ui.horizontal_wrapped(|ui| {
                if ui.checkbox(&mut best, "Best effort").changed() {
                    view.draft.qos.reliability = if best {
                        file::Reliability::BestEffort
                    } else {
                        file::Reliability::Reliable
                    };
                }
                if ui.checkbox(&mut retained, "Transient local").changed() {
                    view.draft.qos.durability = if retained {
                        file::Durability::TransientLocal
                    } else {
                        file::Durability::Volatile
                    };
                }
                if ui.button("Apply source and QoS").clicked() {
                    action = Some(Action::Apply(id));
                }
            });
        });
    });
    ui.collapsing("Reception diagnostics", |ui| {
        p::muted(ui, format!("Samples: {}", view.samples));
        p::muted(ui, format!("Backend queue drops: {}", view.backend_drops));
        p::muted(ui, format!("App queue drops: {}", view.queue_drops));
    })
    .header_response
    .on_hover_text("Queue counters do not measure DDS or network loss.");
    if p::action(
        ui,
        "Add independent view",
        184.0,
        connected && views.len() < 8,
        false,
    )
    .clicked()
    {
        action = Some(Action::Create {
            topic: topic.into(),
            typ: typ.into(),
            image,
            start: false,
        });
    }
    action
}
impl Tools {
    pub(crate) fn apply_inspection(&mut self, action: Action) {
        match action {
            Action::Start(id) => self.start(id),
            Action::Stop(id) => self.stop_view(id),
            Action::Apply(id) => self.apply_draft(id),
            Action::Create {
                topic,
                typ,
                image,
                start,
            } => {
                let before = self.views.len();
                self.topic = topic.clone();
                self.message_type = typ.clone();
                self.add(image);
                if self.views.len() > before {
                    let id = self.views.last().unwrap().id;
                    self.workbench.bind_inspection(&topic, &typ, id, image);
                    if start {
                        self.start(id);
                    }
                } else {
                    self.workbench.error="Could not add inspection view. Connect, choose an available compatible type and keep fewer than 8 views.".into();
                }
            }
        }
    }
}
