//! Read-only, session-scoped DiagnosticArray monitor over the existing Typed wire path.
use crate::session::Session;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use studio_protocol::{
    session::{Command, Event},
    structured::Value,
    subscriptions::*,
};

pub const MESSAGE_TYPE: &str = "diagnostic_msgs/msg/DiagnosticArray";
pub const MAX_COMPONENTS: usize = 256;
/// Reserved host-local view identity; never saved as workspace intent.
const HEALTH_VIEW: ViewId = ViewId(u64::MAX);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Waiting,
    Receiving,
    Stopping,
    Error,
}
#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    pub hardware_id: String,
    pub level: u8,
    pub message: String,
    pub values: Vec<(String, String)>,
    pub received: Instant,
}
impl Component {
    pub fn severity(&self) -> &'static str {
        match self.level {
            0 => "OK",
            1 => "WARN",
            2 => "ERROR",
            3 => "STALE (publisher)",
            _ => "Unknown",
        }
    }
}
#[derive(Clone, Debug)]
struct Owner {
    generation: Generation,
    subscription: SubscriptionId,
    request: RequestId,
}
#[derive(Debug)]
pub struct Monitor {
    pub topic: String,
    pub best_effort: bool,
    pub transient_local: bool,
    pub stale_seconds: u32,
    pub phase: Phase,
    pub error: Option<String>,
    pub incomplete: bool,
    pub components: BTreeMap<(String, String, String), Component>,
    pub evicted: u64,
    pub samples: u64,
    pub backend_drops: u64,
    pub ui_drops: u64,
    owner: Option<Owner>,
    deadline: Option<Instant>,
}
impl Default for Monitor {
    fn default() -> Self {
        Self {
            topic: String::new(),
            best_effort: false,
            transient_local: false,
            stale_seconds: 5,
            phase: Phase::Stopped,
            error: None,
            incomplete: false,
            components: BTreeMap::new(),
            evicted: 0,
            samples: 0,
            backend_drops: 0,
            ui_drops: 0,
            owner: None,
            deadline: None,
        }
    }
}
impl Monitor {
    pub fn ui(
        &mut self,
        view: &mut studio_ui_foundation::diagnostics_health::View,
        ui: &mut eframe::egui::Ui,
        topics: &[(String, Vec<String>)],
        session: Option<&Session>,
        enabled: bool,
    ) {
        use studio_ui_foundation::diagnostics_health::{Action, Row, State as UiState};
        let now = Instant::now();
        let rows = self
            .components
            .iter()
            .map(|(key, c)| Row {
                key: key.clone(),
                name: &c.name,
                hardware: &c.hardware_id,
                message: &c.message,
                severity: c.severity(),
                level: c.level,
                stale: self.stale(c, now),
                age: now.saturating_duration_since(c.received).as_secs_f64(),
                values: &c.values,
            })
            .collect();
        let running = self.running();
        let label = self.label();
        let summary = format!(
            "{} components · {} evicted · {} app / {} backend drops",
            self.components.len(),
            self.evicted,
            self.ui_drops,
            self.backend_drops
        );
        let action = view.ui(
            ui,
            UiState {
                topic: &mut self.topic,
                best_effort: &mut self.best_effort,
                transient_local: &mut self.transient_local,
                stale_seconds: &mut self.stale_seconds,
                running,
                enabled,
                label,
                error: self.error.as_deref(),
                summary,
                rows,
            },
            topics,
        );
        if let Some(session) = session {
            match action {
                Some(Action::Start) => self.start(session, topics),
                Some(Action::Stop) => self.stop(session),
                None => (),
            }
        }
    }
    pub fn owner(&self) -> Option<(Generation, SubscriptionId)> {
        self.owner.as_ref().map(|o| (o.generation, o.subscription))
    }
    pub fn running(&self) -> bool {
        self.owner.is_some()
    }
    pub fn stale(&self, c: &Component, now: Instant) -> bool {
        now.saturating_duration_since(c.received)
            >= Duration::from_secs(self.stale_seconds.clamp(1, 300).into())
    }
    pub fn label(&self) -> &'static str {
        match self.phase {
            Phase::Stopped => "Stopped",
            Phase::Starting => "Starting",
            Phase::Waiting => "Waiting for diagnostics",
            Phase::Receiving => {
                if self
                    .components
                    .values()
                    .any(|c| self.stale(c, Instant::now()))
                {
                    "Stale components"
                } else {
                    "Receiving"
                }
            }
            Phase::Stopping => "Stopping",
            Phase::Error => "Error",
        }
    }
    pub fn disconnected(&mut self) {
        self.owner = None;
        self.deadline = None;
        self.phase = Phase::Stopped;
        self.components.clear();
        self.error = None;
        self.incomplete = false;
    }
    pub fn start(&mut self, session: &Session, topics: &[(String, Vec<String>)]) {
        if self.running() {
            return;
        }
        if !topics
            .iter()
            .any(|(t, types)| t == &self.topic && types.iter().any(|ty| ty == MESSAGE_TYPE))
        {
            self.error = Some("Choose an available DiagnosticArray topic".into());
            self.phase = Phase::Error;
            return;
        }
        let config = Configuration {
            topic: self.topic.clone(),
            message_type: MESSAGE_TYPE.into(),
            best_effort: self.best_effort,
            transient_local: self.transient_local,
            output: Output::Typed,
            spatial: None,
            robot: None,
            map_updates: None,
        };
        let result = session
            .shared
            .lock()
            .unwrap()
            .registry
            .start_explicit(HEALTH_VIEW, config);
        let start = match result {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("Could not reserve health reader: {e:?}"));
                self.phase = Phase::Error;
                return;
            }
        };
        self.owner = Some(Owner {
            generation: start.generation,
            subscription: start.subscription,
            request: start.request,
        });
        self.components.clear();
        self.evicted = 0;
        self.samples = 0;
        self.backend_drops = 0;
        self.ui_drops = 0;
        self.incomplete = false;
        self.error = None;
        self.phase = Phase::Starting;
        self.deadline = Some(Instant::now() + Duration::from_secs(15));
        let registered = session
            .shared
            .lock()
            .unwrap()
            .queue
            .register(start.generation, start.subscription);
        if registered.is_err() {
            self.error = Some("Could not register health queue; disconnect to recover".into());
            self.stop(session);
            return;
        }
        if let Err(e) = session.send(Command::Start {
            generation: start.generation,
            request: start.request,
            subscription: start.subscription,
            configuration: start.configuration,
        }) {
            self.error = Some(e);
            self.phase = Phase::Error;
        }
    }
    pub fn stop(&mut self, session: &Session) {
        let Some(owner) = &self.owner else { return };
        if self.phase == Phase::Stopping {
            return;
        }
        let result = {
            let mut shared = session.shared.lock().unwrap();
            let result = shared.registry.stop_explicit(owner.subscription);
            let _ = shared.queue.unregister(owner.subscription);
            result
        };
        match result {
            Ok(stop) => {
                self.owner.as_mut().unwrap().request = stop.request;
                self.phase = Phase::Stopping;
                self.deadline = Some(Instant::now() + Duration::from_secs(10));
                self.components.clear();
                if let Err(e) = session.send(Command::Stop {
                    generation: stop.generation,
                    request: stop.request,
                    subscription: stop.subscription,
                }) {
                    self.phase = Phase::Error;
                    self.error = Some(e);
                }
            }
            Err(e) => {
                self.phase = Phase::Error;
                self.error = Some(format!("Cleanup unavailable: {e:?}; disconnect to recover"));
            }
        }
    }
    pub fn record_counters(&mut self, subscription: SubscriptionId, counters: &Counters) {
        if self
            .owner
            .as_ref()
            .is_some_and(|o| o.subscription == subscription)
        {
            self.ui_drops = self.ui_drops.max(
                counters
                    .evicted_samples
                    .saturating_add(counters.rejected_samples),
            );
        }
    }
    pub fn poll(&mut self, session: Option<&Session>) {
        if let (Some(session), Some(owner)) = (session, &self.owner) {
            if let Some(c) = session
                .shared
                .lock()
                .unwrap()
                .queue
                .stream_counters(owner.subscription)
            {
                self.ui_drops = self
                    .ui_drops
                    .max(c.evicted_samples.saturating_add(c.rejected_samples));
            }
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.deadline = None;
            self.phase = Phase::Error;
            self.error = Some("Acknowledgement timed out; disconnect to recover".into());
        }
    }
    /// Host routes every accepted ingress event here before its app-specific handler.
    /// Terminal cleanup is authoritative only after the shared registry retires the owner.
    pub fn event(&mut self, event: &Event, received: Instant, session: Option<&Session>) -> bool {
        let Some(o) = self.owner.clone() else {
            return false;
        };
        let matches = |g: Generation, s: SubscriptionId| g == o.generation && s == o.subscription;
        match event {
            Event::Started {
                generation,
                subscription,
                request,
            } if matches(*generation, *subscription) => {
                if *request == o.request && self.phase == Phase::Starting {
                    self.phase = Phase::Waiting;
                    self.deadline = None;
                }
                true
            }
            Event::Observation {
                generation,
                subscription,
                sample,
                backend_drops,
                ..
            } if matches(*generation, *subscription) => {
                let active = session.is_some_and(|s| {
                    s.shared.lock().unwrap().registry.entries().iter().any(|e| {
                        e.generation == o.generation
                            && e.subscription == o.subscription
                            && e.state == State::Active
                    })
                });
                if active
                    && matches!(self.phase, Phase::Waiting | Phase::Receiving)
                    && sample.topic == self.topic
                    && sample.message_type == MESSAGE_TYPE
                {
                    self.backend_drops = self.backend_drops.max(*backend_drops);
                    self.ingest(&sample.value, received);
                }
                true
            }
            Event::Drops {
                generation,
                subscription,
                backend_drops,
                ..
            } if matches(*generation, *subscription) => {
                self.backend_drops = self.backend_drops.max(*backend_drops);
                true
            }
            Event::Stopped {
                generation,
                subscription,
                request,
            } if matches(*generation, *subscription) => {
                if *request == o.request {
                    self.finish(session, None);
                }
                true
            }
            Event::StreamError {
                generation,
                subscription,
                message,
            } if matches(*generation, *subscription) => {
                self.finish(session, Some(message.clone()));
                true
            }
            Event::RequestError {
                generation,
                subscription,
                request,
                message,
                ..
            } if matches(*generation, *subscription) => {
                if *request == o.request {
                    self.finish(session, Some(message.clone()));
                    if self.owner.is_some() {
                        self.phase = Phase::Error;
                        self.deadline = None;
                        self.error = Some("Cleanup request failed; disconnect to recover".into());
                    }
                }
                true
            }
            _ => false,
        }
    }
    fn finish(&mut self, session: Option<&Session>, error: Option<String>) {
        let Some(o) = &self.owner else { return };
        if !session.is_some_and(|s| {
            let shared = s.shared.lock().unwrap();
            shared.registry.generation() == o.generation
                && !shared
                    .registry
                    .entries()
                    .iter()
                    .any(|e| e.subscription == o.subscription)
        }) {
            return;
        }
        self.owner = None;
        self.deadline = None;
        self.components.clear();
        self.error = error;
        self.phase = if self.error.is_some() {
            Phase::Error
        } else {
            Phase::Stopped
        };
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"phase":self.label(),"reader_active":self.running(),"components":self.components.len(),
        "samples":self.samples,"backend_drops":self.backend_drops,"ui_drops":self.ui_drops,"evicted_components":self.evicted,"incomplete":self.incomplete,"has_error":self.error.is_some()})
    }
    pub fn ingest(&mut self, value: &Value, now: Instant) {
        self.samples = self.samples.saturating_add(1);
        self.phase = Phase::Receiving;
        let result = decode(value, now);
        match result {
            Ok(rows) => {
                self.incomplete = false;
                self.error = None;
                for c in rows {
                    let key = (self.topic.clone(), c.name.clone(), c.hardware_id.clone());
                    if !self.components.contains_key(&key)
                        && self.components.len() == MAX_COMPONENTS
                    {
                        if let Some(oldest) = self
                            .components
                            .iter()
                            .min_by_key(|(_, c)| c.received)
                            .map(|(k, _)| k.clone())
                        {
                            self.components.remove(&oldest);
                            self.evicted += 1;
                        }
                    }
                    self.components.insert(key, c);
                }
            }
            Err(()) => {
                self.incomplete = true;
                self.error=Some("Incomplete or malformed DiagnosticArray; previous components retain their receipt age".into());
            }
        }
    }
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, ()> {
    if let Value::Object {
        fields,
        original_fields,
    } = value
    {
        if *original_fields != fields.len() {
            return Err(());
        }
        let mut found = fields.iter().filter(|f| f.name == key);
        let value = &found.next().ok_or(())?.value;
        if found.next().is_some() {
            return Err(());
        }
        Ok(value)
    } else {
        Err(())
    }
}
fn text(value: &Value) -> Result<String, ()> {
    if let Value::Text {
        value,
        original_bytes,
    } = value
    {
        if value.len() == *original_bytes {
            return Ok(value.clone());
        }
    }
    Err(())
}
fn array(value: &Value) -> Result<&[Value], ()> {
    if let Value::Array {
        items,
        original_items,
    } = value
    {
        if items.len() == *original_items {
            return Ok(items);
        }
    }
    Err(())
}
fn decode(value: &Value, now: Instant) -> Result<Vec<Component>, ()> {
    let rows = array(field(value, "status")?)?;
    rows.iter()
        .map(|v| {
            let level = match field(v, "level")? {
                Value::Unsigned { value } | Value::Signed { value } => {
                    value.parse::<u8>().map_err(|_| ())?
                }
                _ => return Err(()),
            };
            let values = array(field(v, "values")?)?
                .iter()
                .map(|kv| Ok((text(field(kv, "key")?)?, text(field(kv, "value")?)?)))
                .collect::<Result<Vec<_>, ()>>()?;
            Ok(Component {
                name: text(field(v, "name")?)?,
                hardware_id: text(field(v, "hardware_id")?)?,
                message: text(field(v, "message")?)?,
                level,
                values,
                received: now,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::structured::{Field, Sample, CONTRACT_VERSION};
    fn text(s: &str) -> Value {
        Value::Text {
            value: s.into(),
            original_bytes: s.len(),
        }
    }
    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object {
            original_fields: fields.len(),
            fields: fields
                .into_iter()
                .map(|(name, value)| Field {
                    name: name.into(),
                    value,
                })
                .collect(),
        }
    }
    fn array(items: Vec<Value>) -> Value {
        Value::Array {
            original_items: items.len(),
            items,
        }
    }
    fn status(name: &str, level: u8) -> Value {
        object(vec![
            ("name", text(name)),
            ("hardware_id", text("board")),
            (
                "level",
                Value::Unsigned {
                    value: level.to_string(),
                },
            ),
            ("message", text("PRIVATE_MESSAGE")),
            (
                "values",
                array(vec![object(vec![
                    ("key", text("secret")),
                    ("value", text("PRIVATE_VALUE")),
                ])]),
            ),
        ])
    }
    fn sample(rows: Vec<Value>) -> Value {
        object(vec![("status", array(rows))])
    }
    fn monitor() -> Monitor {
        Monitor {
            topic: "/diagnostics".into(),
            ..Default::default()
        }
    }
    fn topics() -> Vec<(String, Vec<String>)> {
        vec![("/diagnostics".into(), vec![MESSAGE_TYPE.into()])]
    }
    #[test]
    fn components_age_independently_and_reported_severity_survives_staleness() {
        let mut m = monitor();
        let now = Instant::now();
        m.ingest(&sample(vec![status("motor", 0), status("battery", 1)]), now);
        m.ingest(
            &sample(vec![status("motor", 2)]),
            now + Duration::from_secs(6),
        );
        let motor = &m.components[&("/diagnostics".into(), "motor".into(), "board".into())];
        let battery = &m.components[&("/diagnostics".into(), "battery".into(), "board".into())];
        assert_eq!(motor.severity(), "ERROR");
        assert!(!m.stale(motor, now + Duration::from_secs(6)));
        assert_eq!(battery.severity(), "WARN");
        assert!(m.stale(battery, now + Duration::from_secs(6)));
        m.ingest(
            &sample(vec![status("battery", 0)]),
            now + Duration::from_secs(7),
        );
        assert!(!m.stale(
            &m.components[&("/diagnostics".into(), "battery".into(), "board".into())],
            now + Duration::from_secs(7)
        ));
    }
    #[test]
    fn unknown_publisher_stale_duplicates_and_empty_arrays_are_not_healthy_defaults() {
        let mut m = monitor();
        let now = Instant::now();
        m.ingest(
            &sample(vec![status("a", 3), status("b", 99), status("a", 2)]),
            now,
        );
        assert_eq!(m.components.len(), 2);
        assert_eq!(
            m.components[&("/diagnostics".into(), "a".into(), "board".into())].level,
            2
        );
        assert_eq!(
            m.components[&("/diagnostics".into(), "b".into(), "board".into())].severity(),
            "Unknown"
        );
        m.ingest(&sample(vec![]), now + Duration::from_secs(8));
        assert_eq!(m.components.len(), 2);
        assert!(m
            .components
            .values()
            .all(|c| m.stale(c, now + Duration::from_secs(8))));
    }
    #[test]
    fn malformed_and_truncated_arrays_do_not_refresh_retained_healthy_components() {
        let mut m = monitor();
        let now = Instant::now();
        m.ingest(&sample(vec![status("a", 0)]), now);
        let mut partial = sample(vec![status("a", 0)]);
        if let Value::Object { fields, .. } = &mut partial {
            if let Value::Array { original_items, .. } = &mut fields[0].value {
                *original_items += 1;
            }
        }
        m.ingest(&partial, now + Duration::from_secs(6));
        assert!(m.incomplete);
        assert!(m
            .components
            .values()
            .all(|c| m.stale(c, now + Duration::from_secs(6))));
        m.ingest(&object(vec![]), now);
        assert!(m.incomplete);
        m.ingest(&sample(vec![status("a", 1)]), now + Duration::from_secs(7));
        assert!(!m.incomplete);
    }
    #[test]
    fn cache_evicts_oldest_receipt_and_summaries_exclude_component_payloads() {
        let mut m = monitor();
        let now = Instant::now();
        for i in 0..257 {
            m.ingest(
                &sample(vec![status(&format!("component{i:03}"), 0)]),
                now + Duration::from_millis(i),
            );
        }
        assert_eq!(m.components.len(), 256);
        assert_eq!(m.evicted, 1);
        assert!(!m.components.contains_key(&(
            m.topic.clone(),
            "component000".into(),
            "board".into()
        )));
        let report = m.summary().to_string();
        assert!(!report.contains("PRIVATE"));
        assert!(!report.contains("secret"));
    }
    fn route(session: &Session, m: &mut Monitor, event: Event) {
        session
            .shared
            .lock()
            .unwrap()
            .ingest_for_test(event)
            .unwrap();
        loop {
            let packet = session.shared.lock().unwrap().queue.pop();
            let Some(Packet::Control(e) | Packet::Diagnostic(e) | Packet::Sample { bytes: e, .. }) =
                packet
            else {
                break;
            };
            m.event(&e.event, e.received_at, Some(session));
        }
    }
    #[test]
    fn real_ingress_routes_start_sample_stop_and_rejects_late_samples_without_replay() {
        let (session, commands) = Session::fixture(0);
        let mut m = monitor();
        route(
            &session,
            &mut m,
            Event::ready("jazzy".into(), "fixture".into()),
        );
        m.start(&session, &topics());
        let Command::Start {
            generation,
            request,
            subscription,
            configuration,
        } = commands.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(configuration.output, Output::Typed);
        assert_eq!(session.shared.lock().unwrap().registry.entries().len(), 1);
        route(
            &session,
            &mut m,
            Event::Started {
                generation,
                request,
                subscription,
            },
        );
        assert_eq!(m.phase, Phase::Waiting);
        let observation = |sequence| Event::Observation {
            generation,
            subscription,
            sequence,
            backend_drops: 2,
            sample: Box::new(Sample {
                contract_version: CONTRACT_VERSION,
                topic: "/diagnostics".into(),
                message_type: MESSAGE_TYPE.into(),
                source_time: None,
                value: sample(vec![status("motor", 1)]),
            }),
        };
        route(&session, &mut m, observation(1));
        assert_eq!(m.components.len(), 1);
        assert_eq!(m.backend_drops, 2);
        m.stop(&session);
        let Command::Stop { request, .. } = commands.try_recv().unwrap() else {
            panic!()
        };
        route(&session, &mut m, observation(2));
        assert!(m.components.is_empty());
        route(
            &session,
            &mut m,
            Event::Stopped {
                generation,
                request,
                subscription,
            },
        );
        assert!(!m.running());
        assert!(session.shared.lock().unwrap().registry.entries().is_empty());
        m.disconnected();
        assert!(commands.try_recv().is_err());
        assert!(!m.event(&observation(3), Instant::now(), Some(&session)));
        assert!(m.components.is_empty());
    }
    #[test]
    fn cancellation_before_started_keeps_reservation_until_stop_ack_and_retains_final_drops() {
        let (session, commands) = Session::fixture(0);
        let mut m = monitor();
        route(
            &session,
            &mut m,
            Event::ready("jazzy".into(), "fixture".into()),
        );
        m.start(&session, &topics());
        let Command::Start {
            generation,
            request,
            subscription,
            ..
        } = commands.try_recv().unwrap()
        else {
            panic!()
        };
        m.stop(&session);
        let Command::Stop {
            request: stop_request,
            ..
        } = commands.try_recv().unwrap()
        else {
            panic!()
        };
        route(
            &session,
            &mut m,
            Event::Started {
                generation,
                request,
                subscription,
            },
        );
        assert_eq!(m.phase, Phase::Stopping);
        assert!(m.running());
        let counters = Counters {
            evicted_samples: 7,
            rejected_samples: 2,
            ..Default::default()
        };
        m.record_counters(subscription, &counters);
        assert_eq!(m.ui_drops, 9);
        route(
            &session,
            &mut m,
            Event::Stopped {
                generation,
                request: stop_request,
                subscription,
            },
        );
        assert!(!m.running());
        assert_eq!(m.ui_drops, 9);
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn reader_capacity_is_shared_and_failed_start_retains_no_monitor_owner() {
        let (session, commands) = Session::fixture(0);
        let mut m = monitor();
        route(
            &session,
            &mut m,
            Event::ready("jazzy".into(), "fixture".into()),
        );
        {
            let mut shared = session.shared.lock().unwrap();
            for id in 1..=MAX_SUBSCRIPTIONS {
                shared
                    .registry
                    .start_explicit(
                        ViewId(id as u64),
                        Configuration {
                            topic: "/other".into(),
                            message_type: MESSAGE_TYPE.into(),
                            best_effort: false,
                            transient_local: false,
                            output: Output::Typed,
                            spatial: None,
                            robot: None,
                            map_updates: None,
                        },
                    )
                    .unwrap();
            }
        }
        m.start(&session, &topics());
        assert_eq!(m.phase, Phase::Error);
        assert!(!m.running());
        assert!(commands.try_recv().is_err());
    }
    #[test]
    fn unknown_topic_does_not_start_and_stale_generations_cannot_change_monitor() {
        let (session, commands) = Session::fixture(0);
        let mut m = monitor();
        m.start(&session, &[]);
        assert!(!m.running());
        assert!(commands.try_recv().is_err());
        route(
            &session,
            &mut m,
            Event::ready("jazzy".into(), "fixture".into()),
        );
        m.start(&session, &topics());
        let Command::Start {
            generation,
            request,
            subscription,
            ..
        } = commands.try_recv().unwrap()
        else {
            panic!()
        };
        assert!(!m.event(
            &Event::Started {
                generation: Generation(generation.0 + 1),
                request,
                subscription
            },
            Instant::now(),
            Some(&session)
        ));
        assert_eq!(m.phase, Phase::Starting);
    }
}
