//! Bounded, local UI evidence. Never receives samples, drafts or typed text.
use eframe::egui;
use serde_json::{json, Value};
use std::collections::VecDeque;

const KEY: &str = "studio-ui-diagnostic-trace";
const MAX_EVENTS: usize = 128;
const MAX_WIDGETS: usize = 64;
const MAX_REPORT: usize = 64 * 1024;
pub type DiagnosticTabs<'a> = dyn FnMut(&mut egui::Ui, &mut bool) + 'a;

pub fn short(text: &str) -> String {
    text.chars().take(160).collect()
}
#[derive(Clone, Default)]
struct Trace {
    frame: u64,
    time: f64,
    last_time: Option<f64>,
    max_gap_ms: f64,
    last_update_ms: f64,
    max_update_ms: f64,
    poll_ms: f64,
    files_ms: f64,
    ui_ms: f64,
    events: VecDeque<Value>,
    evicted: u64,
    widgets: Vec<Value>,
    omitted_widgets: usize,
    pointer_events: bool,
}
impl Trace {
    fn push(&mut self, event: Value) {
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
            self.evicted += 1;
        }
        self.events.push_back(event);
    }
}
pub fn begin_frame(ctx: &egui::Context) {
    let (time, events, focused) = ctx.input(|i| {
        let events = i.events.iter().filter_map(|event| match event {
            egui::Event::PointerButton { pos, button, pressed, .. } =>
                Some(json!({"event":"pointer", "position":[pos.x,pos.y], "button":format!("{button:?}"), "pressed":pressed})),
            egui::Event::WindowFocused(focused) => Some(json!({"event":"window_focus","focused":focused})),
            // Text, paste, arbitrary keys and editable field values are excluded.
            egui::Event::Key { key, pressed, .. } if matches!(key,
                egui::Key::Tab | egui::Key::Escape | egui::Key::Enter | egui::Key::Space |
                egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::ArrowLeft | egui::Key::ArrowRight) =>
                Some(json!({"event":"navigation_key","key":format!("{key:?}"),"pressed":pressed})),
            _ => None,
        }).take(32).collect::<Vec<_>>();
        (i.time, events, i.focused)
    });
    let popup = ctx.memory(|m| m.any_popup_open());
    ctx.data_mut(|d| {
        let trace = d.get_temp_mut_or_default::<Trace>(egui::Id::new(KEY));
        trace.frame += 1;
        trace.time = time;
        if let Some(last) = trace.last_time {
            trace.max_gap_ms = trace.max_gap_ms.max((time - last) * 1000.0);
        }
        trace.last_time = Some(time);
        trace.widgets.clear();
        trace.omitted_widgets = 0;
        trace.pointer_events = events.iter().any(|v| v["event"] == "pointer");
        for mut event in events {
            event["frame"] = json!(trace.frame);
            event["time_s"] = json!(time);
            event["window_focused"] = json!(focused);
            event["popup_open"] = json!(popup);
            trace.push(event);
        }
    });
}
pub fn response(ctx: &egui::Context, label: &str, response: &egui::Response) {
    // clicked() can read Context input for a real pointer release. Resolve every
    // Context/Response accessor before acquiring the diagnostic data write lock.
    let clicked = response.clicked();
    let pointer = ctx.pointer_interact_pos();
    let overlaps = pointer.is_some_and(|p| response.rect.contains(p));
    let visible = response.interact_rect.is_positive()
        && ctx.screen_rect().intersects(response.interact_rect);
    if !visible && !clicked {
        return;
    }
    let top_layer = pointer
        .and_then(|p| ctx.layer_id_at(p))
        .map(|l| format!("{l:?}"));
    let record = json!({
        "label":safe_label(label),"widget_id":format!("{:?}",response.id),
        "layer":format!("{:?}",response.layer_id),
        "rect":[response.rect.min.x,response.rect.min.y,response.rect.max.x,response.rect.max.y],
        "hit_rect":[response.interact_rect.min.x,response.interact_rect.min.y,response.interact_rect.max.x,response.interact_rect.max.y],
        "enabled":response.enabled(),"hovered":response.hovered(),
        "contains_pointer":response.contains_pointer(),"focused":response.has_focus(),
        "pointer_down_on":response.is_pointer_button_down_on(),"clicked":clicked
    });
    ctx.data_mut(|d| {
        let trace = d.get_temp_mut_or_default::<Trace>(egui::Id::new(KEY));
        if trace.widgets.len() < MAX_WIDGETS {
            trace.widgets.push(record.clone());
        } else {
            trace.omitted_widgets += 1;
        }
        if clicked || (trace.pointer_events && overlaps) {
            trace.push(
                json!({"event":"control_hit","frame":trace.frame,"time_s":trace.time,
                "control":record,"top_layer_at_pointer":top_layer}),
            );
        }
    });
}
pub fn timing(ctx: &egui::Context, poll_ms: f64, files_ms: f64, ui_ms: f64) {
    ctx.data_mut(|d| {
        let trace = d.get_temp_mut_or_default::<Trace>(egui::Id::new(KEY));
        trace.poll_ms = poll_ms;
        trace.files_ms = files_ms;
        trace.ui_ms = ui_ms;
        trace.last_update_ms = poll_ms + files_ms + ui_ms;
        trace.max_update_ms = trace.max_update_ms.max(trace.last_update_ms);
        if trace.last_update_ms > 100.0 {
            trace.push(
                json!({"event":"slow_update","frame":trace.frame,"time_s":trace.time,
                "poll_ms":poll_ms,"files_ms":files_ms,"ui_ms":ui_ms}),
            );
        }
    });
}
#[cfg(test)]
fn report(ctx: &egui::Context, state: Value) -> String {
    report_for(ctx, "Test", state)
}
fn redact_paths(value: &mut Value) -> u64 {
    match value {
        Value::String(text)
            if ["/Users/", "/home/", "/private/", "/tmp/", "\\", "file://"]
                .iter()
                .any(|part| text.contains(part))
                || text.as_bytes().get(1) == Some(&b':') =>
        {
            *text = "[local path omitted]".into();
            1
        }
        Value::Array(rows) => rows.iter_mut().map(redact_paths).sum(),
        Value::Object(fields) => fields.values_mut().map(redact_paths).sum(),
        _ => 0,
    }
}
pub fn report_for(ctx: &egui::Context, app: &str, mut state: Value) -> String {
    let redacted_strings = redact_paths(&mut state);
    let trace = ctx
        .data_mut(|d| d.get_temp::<Trace>(egui::Id::new(KEY)))
        .unwrap_or_default();
    let rect = ctx.screen_rect();
    let mut value = json!({
        "report":"ROS Studio diagnostics","format":2,"application_name":app,
        "captured_unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|d|d.as_secs()),
        "build":env!("CARGO_PKG_VERSION"),
        "versions":{"session":studio_protocol::session::VERSION,"egui":"0.31.1"},
        "platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "privacy":"Interface names and UI state only. No messages, drafts, typed text, parameter values, environment variables or workspace paths. Local copy; no automatic upload.",
        "frame":trace.frame,"time_s":trace.time,"screen":[rect.width(),rect.height()],
        "pixels_per_point":ctx.pixels_per_point(),
        "pointer":{"position":ctx.pointer_interact_pos().map(|p|[p.x,p.y]),
            "wants_input":ctx.wants_pointer_input(),"using_pointer":ctx.is_using_pointer(),
            "dragged_id":ctx.dragged_id().map(|id|format!("{id:?}")),
            "focused_id":ctx.memory(|m|m.focused()).map(|id|format!("{id:?}")),
            "popup_open":ctx.memory(|m|m.any_popup_open())},
        "timing_ms":{"last_update":trace.last_update_ms,"max_update":trace.max_update_ms,
            "max_frame_gap":trace.max_gap_ms,"poll":trace.poll_ms,"files":trace.files_ms,"ui":trace.ui_ms,
            "gap_note":"Frame gaps also include inactive/background time; not proof of a UI stall."},
        "application":state,"recent_input_and_hits":trace.events,"visible_controls":trace.widgets,
        "limits":{"event_capacity":MAX_EVENTS,"control_capacity":MAX_WIDGETS,"report_bytes":MAX_REPORT,
            "evicted_events":trace.evicted,"omitted_controls":trace.omitted_widgets,"report_items_removed":0,"redacted_strings":redacted_strings}
    });
    let mut removed = 0;
    loop {
        value["limits"]["report_items_removed"] = json!(removed);
        let rendered = format!(
            "ROS Studio diagnostic report\n```json\n{}\n```",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        if rendered.len() <= MAX_REPORT {
            return rendered;
        }
        // All inputs above are bounded; shed oldest records explicitly if pretty
        // formatting pushes the final copy above its independent byte ceiling.
        for key in ["recent_input_and_hits", "visible_controls"] {
            let rows = value[key].as_array_mut().unwrap();
            if !rows.is_empty() {
                rows.remove(0);
                removed += 1;
                break;
            }
        }
        if value["recent_input_and_hits"]
            .as_array()
            .unwrap()
            .is_empty()
            && value["visible_controls"].as_array().unwrap().is_empty()
        {
            // Application summaries have fixed bounds, but preserve a truthful
            // fallback if a future caller accidentally expands that contract.
            value["application"] = json!({"omitted":"Application summary exceeded report limit"});
        }
    }
}

/// Only controlled labels or ROS interface identifiers may reach the recorder.
/// Free-form widget text (drafts, values, paths) is deliberately omitted.
fn safe_label(label: &str) -> String {
    match label {
        "Connect" | "Disconnect" | "Diagnostics" | "Copy diagnostics" | "Refresh" | "Start"
        | "Stop" | "Topics" | "Services" | "Actions" | "Nodes" | "Parameters" | "Views"
        | "Show" | "Inspect" | "Open" | "Close" | "Application" | "ROS health" | "List row" => {
            label.into()
        }
        _ => "Interface control".into(),
    }
}
/// Categorize free-form errors instead of leaking embedded paths, payloads or values.
pub fn error_category(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("workspace") || lower.contains("file") {
        "Workspace operation failed; inspect local details"
    } else if lower.contains("backend") || lower.contains("connection") {
        "Backend/connection error; inspect local details"
    } else if lower.contains("subscription") || lower.contains("view") {
        "Subscription/display error; inspect local details"
    } else if lower.contains("transform") || lower.contains("frame") {
        "Transform/frame error; inspect local details"
    } else {
        "Application error; inspect local details"
    }
}

#[derive(Default)]
pub struct Panel {
    pub open: bool,
    capture: bool,
    copy: bool,
    copied_at: Option<std::time::Instant>,
    text: String,
    health_tab: bool,
}
impl Panel {
    pub fn request(&mut self, copy: bool) {
        self.open = true;
        self.capture = true;
        self.copy = copy;
        self.health_tab = false;
    }
    pub fn shortcut(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::D,
            ))
        }) {
            self.request(true);
        }
    }
    pub fn wants_capture(&self) -> bool {
        self.capture || (self.open && self.text.is_empty())
    }
    pub fn clear_snapshot(&mut self) {
        self.capture = true;
        self.copied_at = None;
    }
    /// Returns a history-clear request; the host owns current causes/history separately.
    pub fn finish_frame(
        &mut self,
        ctx: &egui::Context,
        app: &str,
        state: Option<Value>,
        health: impl FnOnce(&mut egui::Ui),
    ) -> bool {
        self.finish_frame_with_tabs(ctx, app, state, health, None)
    }
    /// Optional host tab strip also opts into concise, tooltip-based guidance.
    pub fn finish_frame_with_tabs(
        &mut self,
        ctx: &egui::Context,
        app: &str,
        state: Option<Value>,
        health: impl FnOnce(&mut egui::Ui),
        tabs: Option<&mut DiagnosticTabs<'_>>,
    ) -> bool {
        let concise = tabs.is_some();
        if let Some(state) = state {
            self.text = report_for(ctx, app, state);
            self.capture = false;
            self.copied_at = None;
            if self.copy {
                ctx.copy_text(self.text.clone());
                self.copied_at = Some(std::time::Instant::now());
                self.copy = false;
            }
        }
        let mut clear = false;
        let mut open = self.open;
        if open {
            egui::Window::new("Diagnostics").open(&mut open)
                .default_size(egui::vec2(680., 460.))
                .min_size(egui::vec2(300., 240.))
                .max_size((ctx.screen_rect().size()-egui::vec2(32.,32.)).max(egui::vec2(300.,240.)))
                .show(ctx, |ui| {
                    if let Some(tabs) = tabs { tabs(ui, &mut self.health_tab); } else { ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.health_tab, false, "Application");
                        ui.selectable_value(&mut self.health_tab, true, "ROS health");
                    });
                    ui.separator();
                    }
                    if self.health_tab { health(ui); return; }
                    ui.horizontal(|ui| {
                        if ui.add_sized([142.,28.],egui::Button::new("Copy diagnostics")).clicked() {
                            self.capture=true; self.copy=true;
                        }
                        if ui.add_sized([80.,28.],egui::Button::new("Refresh")).clicked() { self.capture=true; }
                        let copied=self.copied_at.is_some_and(|at|at.elapsed()<std::time::Duration::from_secs(4));
                        ui.add_sized([64.,28.],egui::Label::new(if copied { "Copied" } else { "" }));
                    });
                    if !concise { ui.small("Safe local metadata only. Payloads, drafts, parameter values and local paths are excluded. Nothing is uploaded."); }
                    if ui.button("Clear recorded history").on_hover_text("Preserves current causes. Copy captures fresh state; Refresh updates this preview.").clicked() { clear=true; self.clear_snapshot(); }
                    if !concise { ui.small("Clearing history preserves current causes. Copy captures fresh state; Refresh updates this preview."); }
                    egui::ScrollArea::both().id_salt("shared-diagnostic-report-scroll").auto_shrink([false,false]).show(ui, |ui| {
                        let mut text=self.text.as_str();
                        ui.add(egui::TextEdit::multiline(&mut text).id_salt("shared-diagnostic-report-text")
                            .code_editor().desired_width(ui.available_width()).desired_rows(16).lock_focus(false))
                            .widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,true,"Diagnostic report"));
                    });
                });
        }
        self.open = open;
        clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_pointer_press_release_records_click_without_context_lock_reentry() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ctx = egui::Context::default();
            for index in 0..4 {
                let mut events = vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))];
                if index >= 2 {
                    events.push(egui::Event::PointerButton {
                        pos: egui::pos2(20.0, 20.0),
                        button: egui::PointerButton::Primary,
                        pressed: index == 2,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                let _ = ctx.run(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        begin_frame(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let r = ui.add_sized([180.0, 36.0], egui::Button::new("List row"));
                            response(ctx, "List row", &r);
                        });
                    },
                );
            }
            let trace = ctx
                .data_mut(|d| d.get_temp::<Trace>(egui::Id::new(KEY)))
                .unwrap();
            let _ = tx.send(
                trace
                    .events
                    .iter()
                    .any(|event| event["control"]["clicked"] == true),
            );
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)),Ok(true),
            "native pointer release must finish and record the click without re-entering the context lock");
    }
    #[test]
    fn copy_emits_the_frozen_report_to_platform_clipboard_output() {
        let ctx = egui::Context::default();
        let mut panel = Panel::default();
        panel.request(true);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            begin_frame(ctx);
            panel.finish_frame(ctx, "Tools", Some(json!({"section":"Topics"})), |_| {});
        });
        let copied = output
            .platform_output
            .commands
            .iter()
            .find_map(|c| {
                if let egui::OutputCommand::CopyText(text) = c {
                    Some(text)
                } else {
                    None
                }
            })
            .expect("OS clipboard output");
        assert_eq!(copied, &panel.text);
        assert!(copied.contains("ROS Studio diagnostic report"));
        let original = panel.text.clone();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            begin_frame(ctx);
            panel.finish_frame(ctx, "Tools", None, |_| {});
        });
        assert_eq!(
            panel.text, original,
            "reviewing keeps the incident snapshot frozen"
        );
    }
    #[test]
    fn blocked_list_pointer_records_foreground_layer_without_claiming_a_click() {
        let ctx = egui::Context::default();
        for index in 0..4 {
            let mut events = vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))];
            if index == 3 {
                events.push(egui::Event::PointerButton {
                    pos: egui::pos2(20.0, 20.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    begin_frame(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let r = ui.add_sized([200.0, 40.0], egui::Button::new("List row"));
                        response(ctx, "List row", &r);
                    });
                    egui::Area::new(egui::Id::new("blocking-overlay"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::Pos2::ZERO)
                        .show(ctx, |ui| {
                            ui.allocate_exact_size(egui::vec2(240.0, 120.0), egui::Sense::click());
                        });
                },
            );
        }
        let trace = ctx
            .data_mut(|d| d.get_temp::<Trace>(egui::Id::new(KEY)))
            .unwrap();
        let hit = trace
            .events
            .iter()
            .rev()
            .find(|v| v["event"] == "control_hit")
            .expect("blocked hit evidence");
        assert_eq!(hit["control"]["clicked"], false);
        assert_eq!(hit["control"]["contains_pointer"], false);
        assert_ne!(hit["control"]["layer"], hit["top_layer_at_pointer"]);
    }
    #[test]
    fn capture_ignores_text_and_preserves_pointer_evidence_with_bounded_overflow() {
        let ctx = egui::Context::default();
        for _ in 0..150 {
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::Text("PRIVATE_DRAFT".into()),
                        egui::Event::Paste("PRIVATE_PARAMETER".into()),
                        egui::Event::PointerButton {
                            pos: egui::pos2(20.0, 20.0),
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    begin_frame(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let r = ui.button("/fixture/list");
                        response(ctx, "/fixture/list", &r);
                    });
                },
            );
        }
        let text = report(&ctx, json!({"section":"Topics"}));
        assert!(!text.contains("PRIVATE_DRAFT"));
        assert!(!text.contains("PRIVATE_PARAMETER"));
        assert!(text.contains("control_hit"));
        assert!(text.contains("top_layer_at_pointer"));
        assert!(text.len() <= MAX_REPORT);
        let trace = ctx
            .data_mut(|d| d.get_temp::<Trace>(egui::Id::new(KEY)))
            .unwrap();
        assert_eq!(trace.events.len(), MAX_EVENTS);
        assert!(trace.evicted > 0);
    }
    #[test]
    fn oversized_future_summary_falls_back_without_exceeding_report_limit() {
        let text = report(
            &egui::Context::default(),
            json!({"future":"x".repeat(MAX_REPORT*2)}),
        );
        assert!(text.len() <= MAX_REPORT);
        assert!(text.contains("Application summary exceeded"));
    }
}

#[cfg(test)]
mod shared_tests {
    use super::*;
    #[test]
    fn copy_captures_fresh_state_and_preview_stays_frozen_between_explicit_actions() {
        let ctx = egui::Context::default();
        let mut panel = Panel::default();
        panel.request(false);
        let _ = ctx.run(Default::default(), |ctx| {
            panel.finish_frame(ctx, "Tools", Some(json!({"samples":1})), |_| {});
        });
        let frozen = panel.text.clone();
        let _ = ctx.run(Default::default(), |ctx| {
            panel.finish_frame(ctx, "Tools", None, |_| {});
        });
        assert_eq!(panel.text, frozen);
        panel.request(true);
        let out = ctx.run(Default::default(), |ctx| {
            panel.finish_frame(ctx, "Visualizer", Some(json!({"samples":2})), |_| {});
        });
        let copied = out
            .platform_output
            .commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .unwrap();
        assert_eq!(copied, &panel.text);
        assert!(copied.contains("Visualizer"));
        assert!(copied.contains("\"samples\": 2"));
    }
    #[test]
    fn dynamic_labels_and_error_categories_do_not_disclose_values_or_paths() {
        for label in [
            "PRIVATE_PARAMETER",
            "/Users/private/key.txt",
            "C:\\secret\\draft",
            "PRIVATE PAYLOAD",
        ] {
            assert!(!safe_label(label).contains("PRIVATE"));
            assert!(!safe_label(label).contains("secret"));
            let category = error_category(label);
            assert!(!category.contains(label));
        }
        let ctx = egui::Context::default();
        for _ in 0..4 {
            let _ = ctx.run(Default::default(), |ctx| {
                begin_frame(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.button("PRIVATE_PARAMETER");
                    response(ctx, "PRIVATE_PARAMETER", &r);
                });
            });
        }
        let report = report_for(&ctx, "Tools", json!({"has_error":true}));
        assert!(!report.contains("PRIVATE_PARAMETER"));
        let report = report_for(
            &ctx,
            "Tools",
            json!({"nested":[{"metadata":"/Users/private/secret.txt"},{"metadata":"C:\\private\\secret.txt"}]}),
        );
        assert!(!report.contains("secret.txt"));
        assert!(report.contains("local path omitted"));
        assert!(report.contains("\"redacted_strings\": 2"));
    }
    #[test]
    fn copy_feedback_keeps_action_geometry_stable_at_both_window_sizes() {
        for size in [egui::vec2(800., 600.), egui::vec2(1280., 800.)] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut panel = Panel::default();
            panel.request(false);
            let mut bounds = None;
            for frame in 0..6 {
                if frame == 4 {
                    panel.request(true);
                }
                let state = panel.wants_capture().then(|| json!({"frame":frame}));
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        panel.finish_frame(ctx, "Tools", state.clone(), |_| {});
                    },
                );
                let update = out.platform_output.accesskit_update.unwrap();
                if let Some(rect) = update
                    .nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some("Copy diagnostics"))
                    .and_then(|(_, n)| n.bounds())
                {
                    if frame == 3 {
                        bounds = Some(rect);
                    }
                    if frame == 5 {
                        assert_eq!(bounds, Some(rect));
                    }
                }
            }
            assert!(bounds.is_some());
        }
    }
}
