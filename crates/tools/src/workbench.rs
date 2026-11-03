//! Task drafts are separate from explicit live operation identities.
mod ui;
use eframe::egui;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Instant,
};
use studio_client_core::session::Session;
use studio_protocol::{
    session::{Command, Event},
    subscriptions::*,
    tools::{Operation, MAX_PAYLOAD},
};
#[derive(Default)]
pub struct Workbench {
    pub open: bool,
    section: String,
    query: String,
    pub node: String,
    graph: BTreeMap<String, Vec<String>>,
    tasks: Vec<Task>,
    selected: usize,
    next: u64,
    pending: BTreeMap<u64, (usize, String)>,
    pub error: String,
    browser_open: bool,
    schema_sources: BTreeMap<u64, (String, String)>,
    refresh_graph: bool,
    graph_generation: Option<u64>,
}
struct Task {
    name: String,
    typ: String,
    bundle: String,
    fields: Value,
    definition: Value,
    value: Value,
    yaml: String,
    yaml_mode: bool,
    error: String,
    details: Value,
    history: VecDeque<(String, Value)>,
    active: Option<u64>,
    active_kind: String,
    schema_attempted: bool,
    period: u64,
    timeout: u64,
    accepted: bool,
    best: bool,
    transient: bool,
    started: Option<Instant>,
    inputs: BTreeMap<String, String>,
    parameters: crate::parameters::Panel,
    events: Option<u64>,
    has_draft: bool,
    defaults: Value,
    loaded_source: Option<(String, String)>,
    page: String,
    message_view: Option<ViewId>,
    image_view: Option<ViewId>,
}
impl Task {
    fn sync_yaml(&mut self) {
        match parse_yaml(&self.yaml).and_then(|v| {
            crate::draft::validate(&v, &self.fields)?;
            Ok(v)
        }) {
            Ok(value) => {
                self.value = value;
                self.error.clear();
                self.inputs.clear();
            }
            Err(error) => self.error = error,
        }
    }
    fn new(name: String, typ: String) -> Self {
        let page = match crate::presentation::section(&typ) {
            "Topics" => {
                if typ == "sensor_msgs/msg/Image" {
                    "Image"
                } else {
                    "Messages"
                }
            }
            "Services" => "Request",
            "Actions" => "Goal",
            "Nodes" => "Relationships",
            _ => "Values",
        }
        .into();
        Self {
            name,
            typ,
            bundle: String::new(),
            fields: Value::Null,
            definition: Value::Null,
            value: json!({}),
            yaml: "{}\n".into(),
            yaml_mode: false,
            error: String::new(),
            details: Value::Null,
            history: VecDeque::new(),
            active: None,
            active_kind: String::new(),
            schema_attempted: false,
            period: 1000,
            timeout: 10000,
            accepted: false,
            best: false,
            transient: false,
            started: None,
            inputs: BTreeMap::new(),
            parameters: crate::parameters::Panel::default(),
            events: None,
            has_draft: false,
            defaults: Value::Null,
            loaded_source: None,
            page,
            message_view: None,
            image_view: None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InspectionMode {
    Messages,
    Plot,
    Image,
}
impl Workbench {
    pub fn diagnostic_state(&self) -> Value {
        json!({
            "section":self.section,"open":self.open,
            "selected_index":self.selected,"browser_open":self.browser_open,
            "graph_entries":self.graph.len(),"query_length":self.query.len(),
            "graph_generation":self.graph_generation,"refresh_pending":self.refresh_graph,
            "has_error":!self.error.is_empty(),
            "pending":self.pending.iter().take(64).map(|(request,(task,kind))|
                json!({"request":request,"task":task,"kind":kind})).collect::<Vec<_>>(),
            "tasks":self.tasks.iter().take(16).map(|t|json!({
                "name":crate::diagnostics::short(&t.name),"type":crate::diagnostics::short(&t.typ),"page":t.page,
                "active":t.active,"active_kind":t.active_kind,"events":t.events,
                "has_error":!t.error.is_empty(),"schema_loaded":!t.fields.is_null(),
                "schema_attempted":t.schema_attempted,"history_count":t.history.len(),
                "message_view":t.message_view.map(|id|id.0),"image_view":t.image_view.map(|id|id.0)
            })).collect::<Vec<_>>()
        })
    }
    pub fn fresh() -> Self {
        Self {
            open: true,
            section: "Topics".into(),
            refresh_graph: true,
            ..Default::default()
        }
    }
    pub fn bind_inspection(&mut self, name: &str, typ: &str, id: ViewId, image: bool) {
        if let Some(t) = self
            .tasks
            .get_mut(self.selected)
            .filter(|t| t.name == name && t.typ == typ)
        {
            if image {
                t.image_view = Some(id);
            } else {
                t.message_view = Some(id);
            }
        }
    }
    pub fn saved(&self) -> studio_workspace::tools::Config {
        studio_workspace::tools::Config {
            section: if self.section.is_empty() {
                "Topics".into()
            } else {
                self.section.clone()
            },
            open: self.open,
            selected: self.selected,
            tasks: self
                .tasks
                .iter()
                .map(|t| studio_workspace::tools::Task {
                    name: t.name.clone(),
                    message_type: t.typ.clone(),
                    bundle: t.bundle.clone(),
                    draft: t.value.clone(),
                    yaml: t.yaml.clone(),
                    yaml_mode: t.yaml_mode,
                    period_ms: t.period,
                    timeout_ms: t.timeout,
                    best_effort: t.best,
                    transient_local: t.transient,
                    parameters: t.parameters.saved(),
                })
                .collect(),
        }
    }
    pub fn restore(&mut self, config: &studio_workspace::tools::Config) {
        let mut restored = Self {
            section: config.section.clone(),
            open: config.open,
            selected: config.selected,
            ..Default::default()
        };
        for saved in &config.tasks {
            let mut t = Task::new(saved.name.clone(), saved.message_type.clone());
            t.bundle = saved.bundle.clone();
            t.value = saved.draft.clone();
            t.yaml = saved.yaml.clone();
            t.yaml_mode = saved.yaml_mode;
            t.period = saved.period_ms;
            t.timeout = saved.timeout_ms;
            t.best = saved.best_effort;
            t.transient = saved.transient_local;
            t.parameters.restore(&saved.parameters);
            t.has_draft = true;
            restored.tasks.push(t);
        }
        *self = restored;
    }

    pub fn disconnected(&mut self) {
        self.pending.clear();
        self.schema_sources.clear();
        self.graph.clear();
        self.graph_generation = None;
        self.refresh_graph = true;
        for t in &mut self.tasks {
            if t.active.take().is_some() {
                t.history.push_back((
                    "tracking lost".into(),
                    json!("Disconnected; remote execution may continue"),
                ));
            }
            t.active_kind.clear();
            t.started = None;
            t.accepted = false;
            t.events = None;
            t.parameters.disconnected();
        }
    }
    fn send(
        &mut self,
        session: Option<&Session>,
        generation: u64,
        task: usize,
        kind: &str,
        operation: Operation,
    ) {
        let result = (|| {
            let s = session.ok_or("Connect before running this operation")?;
            operation.validate()?;
            let kind = if let Operation::Graph { section, .. } = &operation {
                format!("graph:{section}")
            } else {
                kind.to_owned()
            };
            if self.pending.len() >= 64 {
                return Err("Too many pending operations".into());
            }
            let schema_source = if let Operation::Schema {
                message_type,
                bundle,
            } = &operation
            {
                Some((message_type.clone(), bundle.clone()))
            } else {
                None
            };
            self.next = self.next.checked_add(1).ok_or("Operation ID exhausted")?;
            s.send(Command::Tool {
                generation: Generation(generation),
                request: RequestId(self.next),
                subscription: SubscriptionId(self.next),
                operation,
            })?;
            self.pending.insert(self.next, (task, kind));
            if let Some(source) = schema_source {
                self.schema_sources.insert(self.next, source);
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.error = e;
        }
    }
    pub fn poll(&mut self, session: Option<&Session>) {
        let Some(s) = session else { return };
        let events: Vec<_> = s.shared.lock().unwrap().tool_events.drain(..).collect();
        for event in events {
            let Event::Tool {
                request, update, ..
            } = event
            else {
                continue;
            };
            let Some((i, kind)) = self.pending.get(&request.0).cloned() else {
                continue;
            };
            let schema_source = if update.terminal {
                self.schema_sources.remove(&request.0)
            } else {
                None
            };
            if update.terminal {
                self.pending.remove(&request.0);
            }
            if kind.starts_with("graph:") && kind != format!("graph:{}", self.section) {
                continue;
            }
            if update.state == "graph" {
                if let Ok(rows) = serde_json::from_value(update.value) {
                    self.graph = rows;
                }
                continue;
            }
            if update.state == "stopped" {
                if let Some(target) = update.value["target"].as_u64() {
                    self.pending.remove(&target);
                    self.pending
                        .retain(|_, (task, kind)| *task != i || kind != "cancel");
                }
            }
            let Some(t) = self.tasks.get_mut(i) else {
                self.error = update.value.to_string();
                continue;
            };
            if schema_source.is_some_and(|source| source != (t.typ.clone(), t.bundle.clone())) {
                t.error = "Interface source changed; load its schema explicitly".into();
                continue;
            }
            if t.typ == "parameters"
                && !["parameter event", "parameter events started"].contains(&update.state.as_str())
            {
                t.parameters.status = update.state.clone();
                if update.terminal && update.state != "parameters" {
                    t.parameters.result = update.value.clone();
                }
            }
            if update.state == "parameters" {
                t.parameters.receive(&update.value);
                t.active = None;
                continue;
            }
            if update.state == "parameter events started" {
                t.events = Some(request.0);
                continue;
            }
            if update.state == "parameter event" {
                t.parameters.event = update.value;
                continue;
            }
            if update.terminal && t.events == Some(request.0) {
                t.events = None;
            }
            if update.state == "stopped" && update.value["target"].as_u64() == t.events {
                t.events = None;
            }
            if update.state == "node" {
                t.details = update.value;
                continue;
            }
            if update.state == "schema" {
                t.definition = update.value.clone();
                t.fields = update.value["fields"].clone();
                t.loaded_source = Some((t.typ.clone(), t.bundle.clone()));
                t.defaults = update.value["default"].clone();
                if !t.has_draft {
                    t.value = t.defaults.clone();
                    t.yaml = serde_yaml::to_string(&t.value).unwrap_or_default();
                }
                t.has_draft = true;
                t.error.clear();
                t.inputs.clear();
            } else if update.state == "details" {
                t.details = update.value;
            } else {
                if [
                    "publishing",
                    "statistics started",
                    "waiting for service",
                    "request sent",
                    "waiting for action server",
                    "goal sent",
                    "goal accepted",
                    "reading parameters",
                    "writing parameters",
                ]
                .contains(&update.state.as_str())
                {
                    t.active = Some(request.0);
                    t.active_kind = kind.clone();
                }
                if update.state == "goal accepted" {
                    t.accepted = true;
                }
                if (kind == "stop" && update.value["target"].as_u64() == t.active)
                    || (update.terminal && t.active == Some(request.0))
                {
                    t.active = None;
                    t.active_kind.clear();
                    t.accepted = false;
                }
                if update.state == "error" {
                    t.error = update.value.as_str().unwrap_or("Operation failed").into();
                }
                if t.history.len() >= 64 {
                    t.history.pop_front();
                }
                t.history.push_back((update.state, update.value));
            }
        }
    }
    fn browser(&mut self, ui: &mut egui::Ui, connected: bool) -> Option<Operation> {
        let mut op = None;
        ui.horizontal(|ui| {
            studio_ui_foundation::theme::strong(ui, &self.section);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(connected, crate::theme::quiet("Refresh"))
                    .on_hover_text("Refresh interfaces")
                    .clicked()
                {
                    op = Some(Operation::Graph {
                        section: self.section.clone(),
                        node: self.node.clone(),
                    });
                }
            });
        });
        ui.add_space(8.0);
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.query)
                .hint_text(studio_ui_foundation::theme::hint(
                    ui,
                    "Search names or types",
                ))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(10.0);
        let query = self.query.to_lowercase();
        let height = ui.available_height().max(100.0);
        let mut matches = 0;
        egui::ScrollArea::vertical()
            .id_salt("task-browser-scroll")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (name, types) in &self.graph {
                    if !name.to_lowercase().contains(&query)
                        && !types.iter().any(|t| t.to_lowercase().contains(&query))
                    {
                        continue;
                    }
                    for typ in types {
                        matches += 1;
                        let selected = self
                            .tasks
                            .get(self.selected)
                            .is_some_and(|t| t.name == *name && t.typ == *typ);
                        let mut text = egui::text::LayoutJob::default();
                        let room = ((ui.available_width() - 24.0) / 7.0).max(12.0) as usize;
                        let label = short_line(name, room);
                        let secondary = short_line(typ, room);
                        text.wrap.max_rows = 2;
                        text.append(
                            &label,
                            0.0,
                            egui::TextFormat {
                                font_id: egui::TextStyle::Body.resolve(ui.style()),
                                color: ui.visuals().text_color(),
                                ..Default::default()
                            },
                        );
                        text.append(
                            &format!("\n{secondary}"),
                            0.0,
                            egui::TextFormat {
                                font_id: egui::TextStyle::Small.resolve(ui.style()),
                                color: crate::theme::muted(ui.visuals()),
                                ..Default::default()
                            },
                        );
                        if crate::presentation::list_button(
                            ui,
                            text,
                            selected,
                            egui::vec2(ui.available_width(), 50.0),
                        )
                        .on_hover_text(format!("{name}\n{typ}"))
                        .clicked()
                        {
                            if let Some(i) = self
                                .tasks
                                .iter()
                                .position(|t| t.name == *name && t.typ == *typ)
                            {
                                self.selected = i;
                            } else if self.tasks.len() < 16 {
                                self.tasks.push(Task::new(name.clone(), typ.clone()));
                                self.selected = self.tasks.len() - 1;
                            } else {
                                self.error = "At most 16 task tabs; close an inactive tab".into();
                            }
                        }
                    }
                }
                if matches == 0 {
                    crate::presentation::muted(
                        ui,
                        if !connected {
                            "Connect to discover interfaces."
                        } else if self.graph.is_empty() {
                            "Waiting for discovery. Refresh to check again."
                        } else {
                            "No matching interfaces."
                        },
                    );
                }
            });
        op
    }
    fn tab_action(&mut self, action: &str) {
        let i = self.selected;
        if i >= self.tasks.len() {
            return;
        }
        match action {
            "duplicate" => {
                let config = self.saved();
                if config.tasks.len() < 16 {
                    let mut single = config.clone();
                    single.tasks = vec![config.tasks[i].clone()];
                    single.selected = 0;
                    let mut copy = Self::default();
                    copy.restore(&single);
                    self.tasks.push(copy.tasks.remove(0));
                    self.selected = self.tasks.len() - 1;
                }
            }
            "close" => {
                if self.tasks[i].active.is_none()
                    && self.tasks[i].events.is_none()
                    && !self.pending.values().any(|(task, _)| *task == i)
                {
                    self.tasks.remove(i);
                    for (task, _) in self.pending.values_mut() {
                        if *task != usize::MAX && *task > i {
                            *task -= 1;
                        }
                    }
                    self.selected = i.min(self.tasks.len().saturating_sub(1));
                }
            }
            "left" | "right" => {
                let next = if action == "left" {
                    i.checked_sub(1)
                } else {
                    (i + 1 < self.tasks.len()).then_some(i + 1)
                };
                if let Some(next) = next {
                    self.tasks.swap(i, next);
                    for (task, _) in self.pending.values_mut() {
                        if *task == i {
                            *task = next;
                        } else if *task == next {
                            *task = i;
                        }
                    }
                    self.selected = next;
                }
            }
            _ => (),
        }
    }
}

pub(crate) fn parse_yaml(text: &str) -> Result<Value, String> {
    if text.len() > MAX_PAYLOAD {
        return Err("YAML exceeds 64 KiB".into());
    }
    // Deliberately reject aliases/tags: expansion must not evade the input budget.
    let mut quote = None;
    let mut escaped = false;
    let mut previous = ' ';
    for ch in text.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' && q == '"' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ['&', '*', '!'].contains(&ch)
            && (previous.is_whitespace() || "[{,:".contains(previous))
        {
            return Err("YAML aliases and tags are unsupported".into());
        }
        previous = ch;
    }
    let value: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > MAX_PAYLOAD {
        return Err("Expanded payload exceeds 64 KiB".into());
    }
    Ok(value)
}
fn form(
    ui: &mut egui::Ui,
    value: &mut Value,
    fields: &Value,
    path: &str,
    invalid: &mut BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    let Some(fields) = fields.as_array() else {
        return false;
    };
    for f in fields {
        let name = f["name"].as_str().unwrap_or("");
        let key = format!("{path}/{name}");
        let v = &mut value[name];
        let kind = f["kind"].as_i64().unwrap_or(0);
        ui.push_id(&key, |ui| {
            if f["array"].as_u64().unwrap_or(0) > 0 {
                ui.collapsing(format!("{name} (array)"), |ui| {
                    let mut text = invalid
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| serde_json::to_string(v).unwrap_or_default());
                    if ui
                        .add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY))
                        .changed()
                    {
                        match parse_yaml(&text) {
                            Ok(a) if a.is_array() => {
                                *v = a;
                                invalid.remove(&key);
                                changed = true;
                            }
                            _ => {
                                invalid.insert(key.clone(), text);
                            }
                        }
                    }
                });
            } else if kind == 18 {
                ui.collapsing(name, |ui| {
                    changed |= form(ui, v, &f["fields"], &key, invalid);
                });
            } else if kind == 6 {
                ui.horizontal(|ui| {
                    crate::theme::label_slot(ui, 120.0, name).on_hover_text(name);
                    let mut b = v.as_bool().unwrap_or(false);
                    if ui.checkbox(&mut b, "").changed() {
                        *v = json!(b);
                        changed = true;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let label_width = (ui.available_width() * 0.38).clamp(70.0, 160.0);
                    crate::theme::label_slot(ui, label_width, name).on_hover_text(name);
                    let field_width = ui.available_width().max(70.0);
                    let mut text = invalid.get(&key).cloned().unwrap_or_else(|| {
                        if kind == 16 {
                            v.as_str().unwrap_or("").into()
                        } else {
                            v.to_string()
                        }
                    });
                    if ui
                        .add(
                            studio_ui_foundation::theme::input(ui, &mut text)
                                .desired_width(field_width)
                                .font(egui::TextStyle::Monospace),
                        )
                        .changed()
                    {
                        let parsed = if kind == 16 {
                            Ok(json!(text))
                        } else {
                            serde_json::from_str::<Value>(&text).map_err(|_| ())
                        };
                        match parsed {
                            Ok(n) if kind == 16 || n.is_number() => {
                                *v = n;
                                invalid.remove(&key);
                                changed = true;
                            }
                            _ => {
                                invalid.insert(key.clone(), text);
                            }
                        }
                    }
                });
            }
        });
    }
    changed
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_preserves_integer_extremes_and_invalid_draft() {
        let value =
            parse_yaml("signed: -9223372036854775808\nunsigned: 18446744073709551615\n").unwrap();
        assert_eq!(value["signed"].as_i64(), Some(i64::MIN));
        assert_eq!(value["unsigned"].as_u64(), Some(u64::MAX));
        assert!(parse_yaml("a: [").is_err());
        assert!(parse_yaml("a: &x [1]\nb: *x").is_err());
    }
}

fn merge_defaults(defaults: &Value, value: &mut Value) {
    if let (Some(defaults), Some(object)) = (defaults.as_object(), value.as_object_mut()) {
        for (key, default) in defaults {
            if let Some(value) = object.get_mut(key) {
                merge_defaults(default, value);
            } else {
                object.insert(key.clone(), default.clone());
            }
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use studio_protocol::tools::Update;
    fn fixture() -> Session {
        let (s, _) = Session::fixture(0);
        s.shared
            .lock()
            .unwrap()
            .ingest_for_test(Event::ready("jazzy".into(), "fixture".into()))
            .unwrap();
        s
    }
    fn inject(s: &Session, id: u64, state: &str, value: Value) {
        s.shared
            .lock()
            .unwrap()
            .ingest_for_test(Event::Tool {
                generation: Generation(1),
                request: RequestId(id),
                subscription: SubscriptionId(id),
                update: Update::new(state, value, true),
            })
            .unwrap();
    }
    #[test]
    fn restore_keeps_drafts_without_live_handles_or_history() {
        let mut w = Workbench {
            open: true,
            section: "Actions".into(),
            ..Default::default()
        };
        let mut task = Task::new(
            "/action".into(),
            "example_interfaces/action/Fibonacci".into(),
        );
        task.value = json!({"order":5});
        task.yaml = "order: 5\n".into();
        task.active = Some(77);
        task.history
            .push_back(("goal sent".into(), json!({"goal_id":77})));
        w.tasks.push(task);
        let saved = w.saved();
        saved.validate().unwrap();
        let wire = serde_json::to_value(&saved).unwrap();
        assert!(wire.get("interaction").is_none());
        let mut restored = Workbench::default();
        restored.restore(&saved);
        assert!(restored.pending.is_empty());
        assert!(restored.tasks[0].active.is_none());
        assert!(restored.tasks[0].history.is_empty());
        assert!(restored.tasks[0].fields.is_null());
        assert_eq!(restored.tasks[0].value, json!({"order":5}));
    }
    #[test]
    fn reorder_routes_inflight_reply_to_its_original_task() {
        let s = fixture();
        let mut w = Workbench {
            tasks: vec![
                Task::new("/a".into(), "std_msgs/msg/String".into()),
                Task::new("/b".into(), "std_msgs/msg/String".into()),
            ],
            ..Default::default()
        };
        w.pending.insert(1, (0, "publish".into()));
        w.pending.insert(2, (1, "publish".into()));
        w.tab_action("right");
        inject(&s, 1, "published", json!({"count":1}));
        w.poll(Some(&s));
        assert_eq!(w.tasks[1].name, "/a");
        assert_eq!(w.tasks[1].history.back().unwrap().0, "published");
        assert!(w.tasks[0].history.is_empty());
        assert_eq!(w.pending[&2].0, 0);
    }
    #[test]
    fn stop_retires_original_pending_request_and_clears_active_owner() {
        let s = fixture();
        let mut w = Workbench::default();
        let mut task = Task::new("/a".into(), "std_msgs/msg/String".into());
        task.active = Some(10);
        w.tasks.push(task);
        w.pending.insert(10, (0, "publish".into()));
        w.pending.insert(11, (0, "stop".into()));
        inject(&s, 11, "stopped", json!({"target":10}));
        w.poll(Some(&s));
        assert!(w.pending.is_empty());
        assert!(w.tasks[0].active.is_none());
    }
    #[test]
    fn stale_discovery_and_schema_source_changes_are_not_applied() {
        let s = fixture();
        let mut w = Workbench {
            section: "Services".into(),
            ..Default::default()
        };
        w.pending.insert(1, (usize::MAX, "graph:Topics".into()));
        inject(&s, 1, "graph", json!({"/old":["std_msgs/msg/String"]}));
        w.poll(Some(&s));
        assert!(w.graph.is_empty());
        let mut task = Task::new("/a".into(), "std_msgs/msg/String".into());
        task.bundle = "new-bundle".into();
        w.tasks.push(task);
        w.pending.insert(2, (0, "schema".into()));
        w.schema_sources
            .insert(2, ("std_msgs/msg/String".into(), "old-bundle".into()));
        inject(
            &s,
            2,
            "schema",
            json!({"default":{"data":"old"},"fields":[]}),
        );
        w.poll(Some(&s));
        assert!(w.tasks[0].fields.is_null());
        assert!(w.tasks[0].error.contains("source changed"));
    }
    #[test]
    fn invalid_yaml_or_wrong_field_type_preserves_last_valid_typed_draft() {
        let mut t = Task::new("/a".into(), "std_msgs/msg/String".into());
        t.fields = json!([{"name":"data","kind":16}]);
        t.value = json!({"data":"valid"});
        for text in ["data: [", "data: 4"] {
            t.yaml = text.into();
            t.sync_yaml();
            assert!(!t.error.is_empty());
            assert_eq!(t.value, json!({"data":"valid"}));
        }
        t.yaml = "data: recovered".into();
        t.sync_yaml();
        assert!(t.error.is_empty());
        assert_eq!(t.value, json!({"data":"recovered"}));
    }
}

fn short_line(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        value.into()
    } else {
        let room = maximum.saturating_sub(1);
        let prefix = room / 3;
        let suffix = room - prefix;
        let count = value.chars().count();
        value
            .chars()
            .take(prefix)
            .chain(std::iter::once('…'))
            .chain(value.chars().skip(count - suffix))
            .collect()
    }
}

#[cfg(test)]
mod browser_label_tests {
    #[test]
    fn long_labels_keep_distinguishing_endings_and_unicode_boundaries() {
        let label = super::short_line("/shared_namespace/机器人/tools_numeric", 24);
        assert!(label.starts_with("/shared"));
        assert!(label.ends_with("tools_numeric"));
        assert_eq!(label.chars().count(), 24);
        assert_eq!(super::short_line("短名称", 24), "短名称");
    }
}
