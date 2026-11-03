//! ROS-independent presentation boundary for the shared health monitor.
use eframe::egui;
#[derive(Default)]
pub struct View {
    query: String,
    selected: Option<(String, String, String)>,
    concise: bool,
}
pub struct Row<'a> {
    pub key: (String, String, String),
    pub name: &'a str,
    pub hardware: &'a str,
    pub message: &'a str,
    pub severity: &'a str,
    pub level: u8,
    pub stale: bool,
    pub age: f64,
    pub values: &'a [(String, String)],
}
pub struct State<'a> {
    pub topic: &'a mut String,
    pub best_effort: &'a mut bool,
    pub transient_local: &'a mut bool,
    pub stale_seconds: &'a mut u32,
    pub running: bool,
    pub enabled: bool,
    pub label: &'a str,
    pub error: Option<&'a str>,
    pub summary: String,
    pub rows: Vec<Row<'a>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
}
impl View {
    pub fn set_concise(&mut self, concise: bool) {
        self.concise = concise;
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        state: State<'_>,
        topics: &[(String, Vec<String>)],
    ) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!state.running, |ui| {
                egui::ComboBox::from_id_salt("health-source")
                    .width(260.)
                    .selected_text(if state.topic.is_empty() {
                        "Choose diagnostic topic"
                    } else {
                        state.topic.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for (topic, types) in topics {
                            if types
                                .iter()
                                .any(|t| t == "diagnostic_msgs/msg/DiagnosticArray")
                            {
                                ui.selectable_value(state.topic, topic.clone(), topic);
                            }
                        }
                    });
                ui.checkbox(state.best_effort, "Best effort");
                ui.checkbox(state.transient_local, "Transient local");
            });
            if ui
                .add_enabled(
                    state.enabled && !state.running && !state.topic.is_empty(),
                    egui::Button::new("Start"),
                )
                .clicked()
            {
                action = Some(Action::Start);
            }
            if ui
                .add_enabled(state.running, egui::Button::new("Stop"))
                .clicked()
            {
                action = Some(Action::Stop);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_sized([160., 24.], egui::Label::new(state.label).truncate());
            ui.label("Stale after (seconds)");
            ui.add(egui::DragValue::new(state.stale_seconds).range(1..=300));
            ui.label(&state.summary).on_hover_text("Closing keeps monitoring. Disconnect and workspace replacement stop it. Component details are excluded from Application reports.");
        });
        if !state.enabled {
            ui.small(if self.concise {
                "Connect to discover topics."
            } else {
                "Connect to discover topics. Starting reception is explicit."
            });
        }
        if let Some(error) = state.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
        if !self.concise {
            ui.small("Closing keeps monitoring. Disconnect and workspace replacement stop it. Component details are excluded from Application reports.");
        }
        ui.add(
            crate::theme::input(ui, &mut self.query)
                .hint_text(crate::theme::hint(ui, "Search component or hardware")),
        );
        let query = self.query.to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("health-components")
            .max_height(170.)
            .show(ui, |ui| {
                for c in &state.rows {
                    if !format!("{} {}", c.name, c.hardware)
                        .to_lowercase()
                        .contains(&query)
                    {
                        continue;
                    }
                    let label = format!(
                        "{}{} | {} | {} | {:.1}s",
                        c.severity,
                        if c.stale { " / stale locally" } else { "" },
                        c.name,
                        c.hardware,
                        c.age
                    );
                    let color = if c.stale || c.level == 1 || c.level == 3 {
                        ui.visuals().warn_fg_color
                    } else if c.level == 2 || c.level > 3 {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().text_color()
                    };
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(label).color(color))
                                .selected(self.selected.as_ref() == Some(&c.key))
                                .frame(false)
                                .truncate(),
                        )
                        .clicked()
                    {
                        self.selected = Some(c.key.clone());
                    }
                }
                if state.rows.is_empty() {
                    ui.label("No component status received. Choose a topic and Start.");
                }
            });
        if let Some(c) = state
            .rows
            .iter()
            .find(|c| self.selected.as_ref() == Some(&c.key))
        {
            ui.separator();
            egui::ScrollArea::both()
                .id_salt("health-details")
                .show(ui, |ui| {
                    for text in [c.name, c.hardware, c.message] {
                        ui.add(egui::Label::new(text).selectable(true));
                    }
                    for (key, value) in c.values {
                        ui.add(egui::Label::new(format!("{key}: {value}")).selectable(true));
                    }
                });
        }
        action
    }
}
