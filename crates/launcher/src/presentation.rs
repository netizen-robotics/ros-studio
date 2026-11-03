//! Passive suite chooser. Filesystem and process work stays in the suite worker.
use eframe::egui;
use studio_launcher::{
    catalog::Application,
    suite::{Job, Snapshot},
};
use studio_ui_foundation::{app_icons::AppIcon, theme};

pub struct Page<'a> {
    pub snapshot: &'a Snapshot,
    pub busy: bool,
}
impl Page<'_> {
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Job> {
        let mut intent = None;
        egui::TopBottomPanel::bottom("suite-footer")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.small(format!("v{}", env!("CARGO_PKG_VERSION")));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small("Netizen Robotics");
                    });
                });
            });
        egui::TopBottomPanel::top("suite-header")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.extreme_bg_color)
                    .inner_margin(24),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    AppIcon::Launcher.show(ui, 40.0);
                    ui.label(
                        egui::RichText::new("ROS Studio")
                            .size(22.0)
                            .family(studio_ui_foundation::fonts::semibold_family(ui.ctx())),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, theme::UtilityIcon::Information, "Documentation")
                            .on_hover_text("Open ROS Studio documentation")
                            .clicked()
                        {
                            ctx.open_url(egui::OpenUrl::new_tab("https://www.netizenrobotics.com"));
                        }
                        theme::appearance_button(ui);
                    });
                });
            });
        if self.snapshot.error.is_some() || self.snapshot.available.contains(&false) {
            egui::TopBottomPanel::bottom("suite-recovery")
                .frame(
                    egui::Frame::new()
                        .fill(ctx.style().visuals.panel_fill)
                        .inner_margin(16),
                )
                .show(ctx, |ui| {
                    if !self.busy {
                        ui.label(self.snapshot.error.as_deref().unwrap_or(
                            "Application files are missing. Reinstall ROS Studio, then retry.",
                        ));
                        if theme::action_slot(ui, true, 120.0, egui::Button::new("Retry")).clicked()
                        {
                            intent = Some(Job::Inspect);
                        }
                    }
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.extreme_bg_color)
                    .inner_margin(24),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if ui.available_width() >= 720.0 {
                        ui.columns(2, |cols| {
                            for (index, ui) in cols.iter_mut().enumerate() {
                                if self.card(ui, index) {
                                    intent = Some(Job::Open(if index == 0 {
                                        Application::Visualizer
                                    } else {
                                        Application::Tools
                                    }));
                                }
                            }
                        });
                    } else {
                        for index in 0..2 {
                            if self.card(ui, index) {
                                intent = Some(Job::Open(if index == 0 {
                                    Application::Visualizer
                                } else {
                                    Application::Tools
                                }));
                            }
                            ui.add_space(8.0);
                        }
                    }
                });
            });
        intent
    }
    fn card(&self, ui: &mut egui::Ui, index: usize) -> bool {
        let visuals = ui.visuals().clone();
        let muted = theme::muted(&visuals);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 230.0),
            egui::Sense::hover(),
        );
        ui.painter().rect(
            rect,
            f32::from(theme::RADIUS),
            theme::palette(&visuals).raised,
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink(20.0);
        let mut opened = false;
        {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            let ui = &mut child;
            let icon = if index == 0 {
                AppIcon::Visualizer
            } else {
                AppIcon::Tools
            };
            icon.show(ui, 48.0);
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(if index == 0 { "Visualizer" } else { "Tools" })
                    .size(22.0)
                    .family(studio_ui_foundation::fonts::semibold_family(ui.ctx())),
            );
            ui.label(
                egui::RichText::new(if self.busy {
                    "Opening…"
                } else if self.snapshot.available[index] {
                    "Ready"
                } else {
                    "Unavailable"
                })
                .color(muted),
            );
            let action = egui::Rect::from_min_size(
                egui::pos2(inner.left(), inner.bottom() - theme::CONTROL_HEIGHT),
                egui::vec2(120.0, theme::CONTROL_HEIGHT),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(action), |ui| {
                let response = theme::action_slot(
                    ui,
                    !self.busy && self.snapshot.available[index],
                    120.0,
                    theme::primary(ui.visuals(), "Open"),
                );
                if response.has_focus() {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                opened = response.clicked();
            });
        }
        opened
    }
}
