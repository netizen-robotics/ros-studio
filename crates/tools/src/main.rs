//! ROS inspection and interaction application root. It composes shared clients and viewers without a scene.
mod diagnostics;
mod draft;
mod inspection_ui;
mod parameters;
mod presentation;
mod transfers;
mod workbench;
mod workbench_shell;
use eframe::egui;
use std::time::{Duration, Instant};
use studio_client_core::{
    backend_pipe::ImageTarget,
    model::{domain_id, topic_type_available, DEFAULT_DOMAIN_ID},
    session::{self, Session},
};
use studio_images::images;
use studio_inspection::{plots, typed_view};
use studio_protocol::{
    session::{Command, Event},
    subscriptions::*,
};
use studio_ui_foundation::{theme, workspace_document::Controller};
use studio_workspace::{application::Application, workspace as file};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Stopped,
    Starting,
    Waiting,
    Receiving,
    Stopping,
    Recovery,
}
impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Waiting => "Waiting for data",
            Self::Receiving => "Receiving",
            Self::Stopping => "Stopping",
            Self::Recovery => "Cleanup uncertain; disconnect to recover",
        }
    }
    fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Waiting | Self::Receiving)
    }
}
struct ToolView {
    id: ViewId,
    saved: file::ViewConfig,
    draft: file::ViewConfig,
    status: Status,
    generation: Generation,
    subscription: Option<SubscriptionId>,
    request: Option<RequestId>,
    deadline: Option<Instant>,
    pending: Option<file::ViewConfig>,
    error: Option<String>,
    sample: Option<Box<studio_protocol::structured::Sample>>,
    fields: Vec<plots::FieldPath>,
    field_draft: Option<plots::FieldPath>,
    trace: plots::Trace,
    origin: Instant,
    image: images::ImageView,
    stream: Option<images::ImageStream>,
    image_sequence: u64,
    samples: u64,
    backend_drops: u64,
    queue_drops: u64,
}
impl ToolView {
    fn new(id: ViewId, saved: file::ViewConfig) -> Self {
        let mut image = images::ImageView::default();
        if let file::ViewKind::Image { display_mode } = &saved.kind {
            image.set_display_mode(match display_mode {
                file::ImageMode::Fit => images::DisplayMode::Fit,
                file::ImageMode::ActualSize => images::DisplayMode::ActualSize,
            });
        }
        Self {
            id,
            draft: saved.clone(),
            saved,
            status: Status::Stopped,
            generation: Generation(0),
            subscription: None,
            request: None,
            deadline: None,
            pending: None,
            error: None,
            sample: None,
            fields: Vec::new(),
            field_draft: None,
            trace: plots::Trace::default(),
            origin: Instant::now(),
            image,
            stream: None,
            image_sequence: 0,
            samples: 0,
            backend_drops: 0,
            queue_drops: 0,
        }
    }
    fn clear_data(&mut self) {
        self.sample = None;
        self.fields.clear();
        self.image.stop();
        self.stream = None;
        self.image_sequence = 0;
        self.trace.begin(0);
    }
    fn deactivate(&mut self) {
        self.clear_data();
        self.subscription = None;
        self.request = None;
        self.deadline = None;
        self.pending = None;
        self.status = Status::Stopped;
    }
    fn config(&self) -> Configuration {
        configuration(&self.saved)
    }
    fn owner(&self) -> Option<file::HandleIdentity> {
        self.subscription.map(|subscription| file::HandleIdentity {
            generation: self.generation,
            subscription,
        })
    }
}
fn configuration(saved: &file::ViewConfig) -> Configuration {
    Configuration {
        map_updates: None,
        topic: saved.topic.clone(),
        message_type: saved.message_type.clone(),
        best_effort: saved.qos.reliability == file::Reliability::BestEffort,
        transient_local: saved.qos.durability == file::Durability::TransientLocal,
        output: if matches!(saved.kind, file::ViewKind::Image { .. }) {
            Output::CompactImage
        } else {
            Output::Typed
        },
        robot: None,
        spatial: None,
    }
}
fn empty_workspace() -> file::Workspace {
    file::Workspace {
        tools: Some(studio_workspace::tools::Config {
            open: true,
            ..Default::default()
        }),
        presentation: None,
        version: file::FORMAT_VERSION,
        name: "Untitled tools workspace".into(),
        panels: file::Panels {
            data_open: true,
            inspector_open: true,
            diagnostics_open: false,
        },
        layout: file::Layout::for_views(&[]),
        views: Vec::new(),
        connection_suggestion: None,
    }
}
struct Tools {
    network: studio_client_core::network_ui::NetworkSettings,
    last_title: String,
    retiring: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    restore_cleanup: Vec<file::HandleIdentity>,
    workbench: workbench::Workbench,
    session: Option<Session>,
    generation: u64,
    connected: bool,
    connecting: bool,
    deadline: Option<Instant>,
    domain: String,
    cyclone: bool,
    status: String,
    topics: Vec<(String, Vec<String>)>,
    graph_received: bool,
    query: String,
    topic: String,
    message_type: String,
    views: Vec<ToolView>,
    selected: Option<ViewId>,
    next_id: u64,
    files: Controller,
    workspace: file::Workspace,
    files_open: bool,
    path: String,
    discard: bool,
    about: bool,
    help_open: bool,
    view_settings_open: bool,
    diagnostics: diagnostics::Panel,
    health: studio_client_core::diagnostic_health::Monitor,
    health_view: studio_ui_foundation::diagnostics_health::View,
    diagnostic_history: std::collections::VecDeque<&'static str>,
}
impl Default for Tools {
    fn default() -> Self {
        let workspace = empty_workspace();
        let supported = file::SupportedRenderers {
            messages: true,
            numeric_plot: true,
            image: true,
            ..Default::default()
        };
        Self {
            network: if cfg!(test) {
                studio_client_core::network_ui::NetworkSettings::local_fixture()
            } else {
                studio_client_core::network_ui::NetworkSettings::new("tools")
            },
            last_title: String::new(),
            retiring: None,
            restore_cleanup: Vec::new(),
            workbench: workbench::Workbench::fresh(),
            session: None,
            generation: 0,
            connected: false,
            connecting: false,
            deadline: None,
            domain: DEFAULT_DOMAIN_ID.to_string(),
            cyclone: false,
            status: "Disconnected".into(),
            topics: Vec::new(),
            graph_received: false,
            query: String::new(),
            topic: String::new(),
            message_type: String::new(),
            views: Vec::new(),
            selected: None,
            next_id: 0,
            files: Controller::for_application(workspace.clone(), supported, Application::Tools)
                .unwrap(),
            workspace,
            files_open: false,
            path: String::new(),
            discard: false,
            about: false,
            help_open: false,
            view_settings_open: false,
            diagnostics: diagnostics::Panel::default(),
            health: Default::default(),
            health_view: Default::default(),
            diagnostic_history: Default::default(),
        }
    }
}
impl Tools {
    fn frozen(&self) -> bool {
        self.files.is_applying()
    }
    fn disconnect(&mut self) {
        self.disconnect_inner(true);
    }
    fn disconnect_inner(&mut self, notify_files: bool) {
        self.health.disconnected();
        self.workbench.disconnected();
        if let Some(session) = self.session.take() {
            self.retiring = Some(session.completion());
            let mut shared = session.shared.lock().unwrap();
            self.generation = self.generation.max(shared.registry.generation().0);
            shared.registry.disconnect();
        }
        for view in &mut self.views {
            view.deactivate();
        }
        self.connected = false;
        self.connecting = false;
        self.deadline = None;
        self.topics.clear();
        self.graph_received = false;
        if notify_files {
            self.files.disconnect();
        }
        self.status = if self.retiring.is_some() {
            "Disconnecting; waiting for backend cleanup"
        } else {
            "Disconnected"
        }
        .into();
    }
    fn fail(&mut self, error: impl Into<String>) {
        self.disconnect();
        self.status = error.into();
    }
    fn connect(&mut self, ctx: &egui::Context) {
        if self.frozen()
            || self
                .retiring
                .as_ref()
                .is_some_and(|r| !r.load(std::sync::atomic::Ordering::Acquire))
        {
            return;
        }
        self.retiring = None;
        let domain = match domain_id(&self.domain) {
            Ok(n) => n,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let config = match self.network.config(domain, self.cyclone) {
            Ok(config) => config,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        self.domain = domain.to_string();
        self.disconnect();
        let session = session::launch(config, self.generation);
        session.shared.lock().unwrap().repaint = Some(ctx.clone());
        self.session = Some(session);
        self.connecting = true;
        self.deadline = Some(Instant::now() + Duration::from_secs(30));
        self.status = "Connecting…".into();
    }
    fn add(&mut self, image: bool) {
        if self.frozen()
            || self.views.len() >= 8
            || !self.connected
            || !topic_type_available(&self.topics, &self.topic, &self.message_type)
        {
            return;
        }
        if image && self.message_type != images::IMAGE_MESSAGE_TYPE {
            return;
        }
        let Some(next) = self.next_id.checked_add(1) else {
            self.fail("View identities exhausted");
            return;
        };
        self.next_id = next;
        let id = ViewId(next);
        let saved = file::ViewConfig {
            id: format!("tool_{next}"),
            kind: if image {
                file::ViewKind::Image {
                    display_mode: file::ImageMode::Fit,
                }
            } else {
                file::ViewKind::Messages
            },
            topic: self.topic.clone(),
            message_type: self.message_type.clone(),
            qos: file::Qos {
                reliability: file::Reliability::BestEffort,
                durability: file::Durability::Volatile,
            },
        };
        let mut saved = saved;
        while self.views.iter().any(|v| v.saved.id == saved.id) {
            saved.id.push('x');
        }
        self.workspace.layout.order.push(saved.id.clone());
        self.views.push(ToolView::new(id, saved));
        self.selected = Some(id);
    }
    fn start(&mut self, id: ViewId) {
        if self.frozen() || !self.connected || !self.graph_received {
            return;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        if self.views[index].subscription.is_some() {
            return;
        }
        let config = self.views[index].config();
        if !Application::Tools.accepts(&self.views[index].saved.kind)
            || !config.valid()
            || !topic_type_available(&self.topics, &config.topic, &config.message_type)
        {
            self.views[index].error =
                Some("Choose an available topic and exact message type".into());
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let intent = {
            let mut shared = session.shared.lock().unwrap();
            shared
                .registry
                .start_explicit(id, config)
                .and_then(|start| {
                    shared
                        .queue
                        .register(start.generation, start.subscription)
                        .map_err(|_| RegistryError::Capacity)?;
                    Ok(start)
                })
        };
        let start = match intent {
            Ok(start) => start,
            Err(e) => {
                self.views[index].error = Some(format!("Could not start view: {e:?}"));
                return;
            }
        };
        let view = &mut self.views[index];
        view.clear_data();
        view.generation = start.generation;
        view.subscription = Some(start.subscription);
        view.request = Some(start.request);
        view.status = Status::Starting;
        view.deadline = Some(Instant::now() + Duration::from_secs(15));
        view.error = None;
        view.samples = 0;
        view.backend_drops = 0;
        view.queue_drops = 0;
        view.origin = Instant::now();
        view.trace.begin(start.subscription.0);
        if start.configuration.output == Output::CompactImage {
            let generation = view.image.start();
            let stream = images::ImageStream::new(view.image.frames(), generation);
            view.stream = Some(stream.clone());
            let binding = session.shared.lock().unwrap().bind_image(ImageTarget {
                generation: start.generation,
                subscription: start.subscription,
                topic: view.saved.topic.clone(),
                stream,
            });
            if let Err(error) = binding {
                self.fail(error);
                return;
            }
        }
        if let Err(error) = session.send(Command::Start {
            generation: start.generation,
            request: start.request,
            subscription: start.subscription,
            configuration: start.configuration,
        }) {
            self.fail(error);
        }
    }
    fn stop_view(&mut self, id: ViewId) {
        let Some(view) = self.views.iter_mut().find(|v| v.id == id) else {
            return;
        };
        let Some(subscription) = view.subscription else {
            return;
        };
        if matches!(view.status, Status::Stopping | Status::Recovery) {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let stop = {
            let mut shared = session.shared.lock().unwrap();
            let intent = shared.registry.stop_explicit(subscription);
            let _ = shared.queue.unregister(subscription);
            shared.images.retain(|t| t.subscription != subscription);
            intent
        };
        let stop = match stop {
            Ok(stop) => stop,
            Err(e) => {
                view.status = Status::Recovery;
                view.error = Some(format!("Could not request cleanup: {e:?}"));
                return;
            }
        };
        view.status = Status::Stopping;
        view.request = Some(stop.request);
        view.deadline = Some(Instant::now() + Duration::from_secs(10));
        view.clear_data();
        if let Err(error) = session.send(Command::Stop {
            generation: stop.generation,
            request: stop.request,
            subscription,
        }) {
            self.fail(error);
        }
    }
    fn apply_draft(&mut self, id: ViewId) {
        if self.frozen() {
            return;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else {
            return;
        };
        let draft = self.views[index].draft.clone();
        if !Application::Tools.accepts(&draft.kind) || !configuration(&draft).valid() {
            self.views[index].error = Some("Unsupported display configuration".into());
            return;
        }
        if self.views[index].subscription.is_some() {
            self.views[index].pending = Some(draft);
            self.stop_view(id);
        } else {
            self.views[index].saved = draft;
            self.views[index].error = None;
        }
    }
    fn cleanup_absent(&self, generation: Generation, subscription: SubscriptionId) -> bool {
        self.session.as_ref().is_some_and(|s| {
            let shared = s.shared.lock().unwrap();
            shared.registry.generation() == generation
                && !shared
                    .registry
                    .entries()
                    .iter()
                    .any(|e| e.subscription == subscription)
        })
    }
    fn cleaned(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        error: Option<String>,
    ) {
        if !self.cleanup_absent(generation, subscription) {
            return;
        }
        let mut restart = None;
        if let Some(view) = self
            .views
            .iter_mut()
            .find(|v| v.generation == generation && v.subscription == Some(subscription))
        {
            let pending = view.pending.take();
            view.deactivate();
            view.error = error;
            if let Some(saved) = pending {
                view.saved = saved;
                restart = Some(view.id);
            }
        }
        let _ = self.files.cleanup_confirmed(file::HandleIdentity {
            generation,
            subscription,
        });
        if let Some(id) = restart {
            self.start(id);
        }
    }
    fn event(&mut self, event: Event, received: Instant) {
        let health_owner = self.health.owner();
        if self.health.event(&event, received, self.session.as_ref()) {
            if let Some((generation, subscription)) =
                health_owner.filter(|_| self.health.owner().is_none())
            {
                let _ = self.files.cleanup_confirmed(file::HandleIdentity {
                    generation,
                    subscription,
                });
            }
            return;
        }
        if matches!(
            &event,
            Event::Error { .. } | Event::StreamError { .. } | Event::RequestError { .. }
        ) {
            if self.diagnostic_history.len() == 32 {
                self.diagnostic_history.pop_front();
            }
            self.diagnostic_history
                .push_back("Backend request or stream error; inspect the affected task locally");
        }
        match event {
            Event::Ready { distro, rmw, .. } if self.connecting => {
                self.connected = true;
                self.connecting = false;
                self.deadline = None;
                self.status = format!("Connected: {distro} / {rmw}");
                if let Some(session) = &self.session {
                    self.generation = session.shared.lock().unwrap().registry.generation().0;
                }
            }
            Event::Started {
                generation,
                request,
                subscription,
            } => {
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.request == Some(request)
                        && v.status == Status::Starting
                }) {
                    view.status = Status::Waiting;
                    view.deadline = None;
                }
            }
            Event::Stopped {
                generation,
                request,
                subscription,
            } => {
                if self.views.iter().any(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.request == Some(request)
                }) {
                    self.cleaned(generation, subscription, None);
                }
            }
            Event::Observation {
                generation,
                subscription,
                sample,
                backend_drops,
                ..
            } => {
                if !self.connected {
                    return;
                }
                let active = self.session.as_ref().is_some_and(|s| {
                    s.shared.lock().unwrap().registry.entries().iter().any(|e| {
                        e.generation == generation
                            && e.subscription == subscription
                            && e.state == State::Active
                    })
                });
                if !active {
                    return;
                }
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.status.active()
                        && v.saved.topic == sample.topic
                        && v.saved.message_type == sample.message_type
                }) {
                    if let Some(path) = view.saved.kind.plot_path() {
                        if let Some(receipt) = received.checked_duration_since(view.origin) {
                            view.trace.push(
                                subscription.0,
                                plots::Sample {
                                    receipt,
                                    value: plots::numeric_value(&sample.value, &path),
                                },
                            );
                        }
                    }
                    view.fields = plots::numeric_fields(&sample.value);
                    view.sample = Some(sample);
                    view.samples = view.samples.saturating_add(1);
                    view.backend_drops = view.backend_drops.max(backend_drops);
                    view.status = Status::Receiving;
                    view.deadline = None;
                }
            }
            Event::StreamError {
                generation,
                subscription,
                message,
            } => self.cleaned(generation, subscription, Some(message)),
            Event::RequestError {
                generation,
                request,
                subscription,
                message,
                no_handle,
            } => {
                if no_handle && self.cleanup_absent(generation, subscription) {
                    self.cleaned(generation, subscription, Some(message));
                } else if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.request == Some(request)
                }) {
                    view.error = Some(message.clone());
                    if view.status == Status::Stopping {
                        view.status = Status::Recovery;
                        view.deadline = None;
                        let _ = self.files.stop_failed(
                            file::HandleIdentity {
                                generation,
                                subscription,
                            },
                            &message,
                        );
                    }
                }
            }
            Event::Drops {
                generation,
                subscription,
                backend_drops,
                ..
            } => {
                if let Some(view) = self.views.iter_mut().find(|v| {
                    v.generation == generation
                        && v.subscription == Some(subscription)
                        && v.status.active()
                }) {
                    view.backend_drops = view.backend_drops.max(backend_drops);
                }
            }
            Event::Error { message } => self.fail(message),
            _ => {}
        }
    }
    fn poll(&mut self) {
        if self
            .retiring
            .as_ref()
            .is_some_and(|r| r.load(std::sync::atomic::Ordering::Acquire))
        {
            self.retiring = None;
            if self.status == "Disconnecting; waiting for backend cleanup" {
                self.status = "Disconnected".into();
            }
        }
        self.health.poll(self.session.as_ref());
        self.workbench.poll(self.session.as_ref());
        if let Some(error) = self
            .session
            .as_ref()
            .and_then(|s| s.terminal.try_recv().ok())
            .or_else(|| {
                self.session
                    .as_ref()
                    .and_then(|s| s.ready.try_recv().ok())
                    .and_then(Result::err)
            })
        {
            self.fail(error);
            return;
        }
        for _ in 0..64 {
            let packet = self
                .session
                .as_ref()
                .and_then(|s| s.shared.lock().unwrap().queue.pop());
            let timed = match packet {
                Some(
                    Packet::Control(e) | Packet::Diagnostic(e) | Packet::Sample { bytes: e, .. },
                ) => e,
                None => break,
            };
            if let Some((subscription, counters)) = timed.final_counters {
                self.health.record_counters(subscription, &counters);
                if let Some(view) = self
                    .views
                    .iter_mut()
                    .find(|v| v.subscription == Some(subscription))
                {
                    view.queue_drops = view.queue_drops.max(
                        counters
                            .evicted_samples
                            .saturating_add(counters.rejected_samples),
                    );
                }
            }
            self.event(timed.event, timed.received_at);
        }
        if let Some(mut topics) = self
            .session
            .as_ref()
            .and_then(|s| s.shared.lock().unwrap().graph.take())
        {
            topics.sort_by(|a, b| a.0.cmp(&b.0));
            self.topics = topics;
            self.graph_received = true;
        }
        for view in &mut self.views {
            if let Some(update) = view.stream.as_ref().and_then(|s| s.snapshot()) {
                if update.sequence > view.image_sequence && view.status.active() {
                    view.image_sequence = update.sequence;
                    view.status = Status::Receiving;
                    view.deadline = None;
                    view.samples = update.received_frames;
                    view.backend_drops = update.backend_drops;
                    view.error = update.error.clone();
                    if let Some(error) = update.error {
                        view.image.fail(error);
                    }
                }
            }
            if let (Some(session), Some(subscription)) = (&self.session, view.subscription) {
                if let Some(c) = session
                    .shared
                    .lock()
                    .unwrap()
                    .queue
                    .stream_counters(subscription)
                {
                    view.queue_drops = view
                        .queue_drops
                        .max(c.evicted_samples.saturating_add(c.rejected_samples));
                }
            }
        }
        let now = Instant::now();
        if self.deadline.is_some_and(|d| now >= d) {
            self.fail("Connection timed out. Reconnect to try again.");
            return;
        }
        for view in &mut self.views {
            if view.deadline.is_some_and(|d| now >= d) {
                view.deadline = None;
                view.status = Status::Recovery;
                view.clear_data();
                view.error = Some("Acknowledgement timed out; disconnect to recover".into());
                if let Some(owner) = view.owner() {
                    let _ = self.files.stop_failed(owner, "Acknowledgement timed out");
                }
            }
        }
    }
    fn snapshot(&self) -> file::Workspace {
        let mut workspace = self.workspace.clone();
        workspace.tools = Some(self.workbench.saved());
        workspace.views = self
            .views
            .iter()
            .map(|v| {
                let mut saved = v.saved.clone();
                if let file::ViewKind::Image { display_mode } = &mut saved.kind {
                    *display_mode = match v.image.display_mode() {
                        images::DisplayMode::Fit => file::ImageMode::Fit,
                        images::DisplayMode::ActualSize => file::ImageMode::ActualSize,
                    };
                }
                saved
            })
            .collect();
        workspace
    }
    fn poll_files(&mut self, ctx: &egui::Context) {
        let snapshot = self.snapshot();
        if let Err(error) = self.files.observe(snapshot) {
            self.status = error;
        }
        self.files.poll();
        if self.files.take_exit_request() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let owners = self
            .views
            .iter()
            .filter_map(ToolView::owner)
            .collect::<Vec<_>>();
        if self.retiring.is_some() || !transfers::is_idle() {
            return;
        }
        for owner in self.restore_cleanup.drain(..) {
            let _ = self.files.cleanup_confirmed(owner);
        }
        match self.files.take_ready(&owners) {
            Ok(Some(restore)) => {
                let workspace = restore.into_configuration();
                let mut views = Vec::new();
                for saved in &workspace.views {
                    let Some(next) = self.next_id.checked_add(1) else {
                        self.fail("View identities exhausted");
                        return;
                    };
                    self.next_id = next;
                    views.push(ToolView::new(ViewId(next), saved.clone()));
                }
                self.workbench
                    .restore(&workspace.tools.clone().unwrap_or_default());
                self.workspace = workspace;
                self.views = views;
                self.selected = None;
            }
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }
    fn apply_workspace(&mut self) {
        let owners = self
            .views
            .iter()
            .filter_map(ToolView::owner)
            .collect::<Vec<_>>();
        let approval = if self.discard {
            file::ApplyApproval::DiscardUnsaved
        } else {
            file::ApplyApproval::CleanOrSaved
        };
        match self.files.begin_apply(approval, &owners) {
            Ok(_intents) => {
                self.restore_cleanup = owners;
                self.disconnect_inner(false);
            }
            Err(error) => self.status = error,
        }
    }
}
impl Tools {
    fn ui(&mut self, ctx: &egui::Context) {
        let frozen = self.frozen();
        self.workbench_shell(ctx, frozen);
        if self.workbench.open {
            if frozen {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label("Waiting for backend and file cleanup before replacing configuration");
                });
            } else {
                let mut actions = Vec::new();
                let views = &mut self.views;
                let connected = self.connected;
                self.workbench.ui(
                    ctx,
                    if connected {
                        self.session.as_ref()
                    } else {
                        None
                    },
                    self.generation,
                    &mut |ui, name, typ, mode, binding| {
                        if let Some(action) =
                            inspection_ui::show(ui, views, name, typ, mode, binding, connected)
                        {
                            actions.push(action);
                        }
                    },
                );
                for action in actions {
                    self.apply_inspection(action);
                }
            }
            self.workspace_windows(ctx);
            return;
        }
        let compact = ctx.screen_rect().width() < 900.0;
        let mut view_settings_open = self.view_settings_open;
        if compact {
            egui::TopBottomPanel::top("compact-views-controls").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("compact-view-picker")
                        .width(220.0)
                        .truncate()
                        .selected_text(
                            self.selected
                                .and_then(|id| self.views.iter().find(|v| v.id == id))
                                .map(|v| v.saved.topic.as_str())
                                .unwrap_or("Choose an open view"),
                        )
                        .show_ui(ui, |ui| {
                            for view in &self.views {
                                ui.selectable_value(
                                    &mut self.selected,
                                    Some(view.id),
                                    format!("{} · {}", view.saved.topic, view.saved.id),
                                );
                            }
                        });
                    if ui.button("View setup and settings").clicked() {
                        view_settings_open = !view_settings_open;
                    }
                });
            });
        } else {
            egui::SidePanel::left("tools-open-views")
                .default_width(224.0)
                .resizable(true)
                .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(12))
                .show(ctx, |ui| {
                    studio_ui_foundation::theme::strong(ui, "Dashboard panels");
                    ui.add(
                        studio_ui_foundation::theme::input(ui, &mut self.query)
                            .hint_text(studio_ui_foundation::theme::hint(ui, "Filter views")),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("open-views-list")
                        .show(ui, |ui| {
                            for saved_id in &self.workspace.layout.order {
                                if let Some(view) =
                                    self.views.iter().find(|v| &v.saved.id == saved_id)
                                {
                                    if !view
                                        .saved
                                        .topic
                                        .to_lowercase()
                                        .contains(&self.query.to_lowercase())
                                    {
                                        continue;
                                    }
                                    let kind = match view.saved.kind {
                                        file::ViewKind::Image { .. } => "Image",
                                        file::ViewKind::NumericPlot { .. } => "Plot",
                                        _ => "Messages",
                                    };
                                    let label = format!(
                                        "{}\n{} · {}",
                                        presentation::short_name(&view.saved.topic),
                                        kind,
                                        view.status.label()
                                    );
                                    if presentation::list_button(
                                        ui,
                                        label,
                                        self.selected == Some(view.id),
                                        egui::vec2(ui.available_width(), 50.0),
                                    )
                                    .on_hover_text(&view.saved.topic)
                                    .clicked()
                                    {
                                        self.selected = Some(view.id);
                                    }
                                }
                            }
                        });
                    if self.views.is_empty() {
                        presentation::muted(ui, "No panels yet");
                    }
                });
        }
        let mut toggle = None;
        let mut apply = None;
        let mut remove = None;
        let mut reorder = None;
        let mut inspector = |ui: &mut egui::Ui| {
            egui::ScrollArea::vertical()
                .id_salt("tools-inspector-scroll")
                .show(ui, |ui| {
                    ui.heading("Add a panel");
                    egui::ComboBox::from_id_salt("view-source-topic")
                        .width(ui.available_width() - 8.0)
                        .truncate()
                        .selected_text(if self.topic.is_empty() {
                            "Choose a topic"
                        } else {
                            &self.topic
                        })
                        .show_ui(ui, |ui| {
                            for (topic, types) in &self.topics {
                                if ui.selectable_label(self.topic == *topic, topic).clicked() {
                                    self.topic = topic.clone();
                                    self.message_type = types.first().cloned().unwrap_or_default();
                                }
                            }
                        });
                    let types = self
                        .topics
                        .iter()
                        .find(|(t, _)| t == &self.topic)
                        .map(|(_, types)| types.clone())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("tools-message-type")
                        .selected_text(if self.message_type.is_empty() {
                            "Choose message type"
                        } else {
                            &self.message_type
                        })
                        .show_ui(ui, |ui| {
                            for ty in types {
                                ui.selectable_value(&mut self.message_type, ty.clone(), ty);
                            }
                        });
                    ui.add_enabled_ui(
                        self.connected
                            && !frozen
                            && self.views.len() < 8
                            && topic_type_available(&self.topics, &self.topic, &self.message_type),
                        |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("+ Messages").clicked() {
                                    self.add(false);
                                }
                                if self.message_type == images::IMAGE_MESSAGE_TYPE
                                    && ui.button("+ Image").clicked()
                                {
                                    self.add(true);
                                }
                            });
                        },
                    );
                    ui.separator();
                    if let Some(index) = self
                        .selected
                        .and_then(|id| self.views.iter().position(|v| v.id == id))
                    {
                        let view = &mut self.views[index];
                        let id = view.id;
                        ui.heading("View details");
                        ui.label(&view.saved.topic);
                        ui.monospace(&view.saved.message_type);
                        ui.label(view.status.label());
                        let mut enabled = view.status.active();
                        if ui
                            .add_enabled(
                                !frozen
                                    && !matches!(view.status, Status::Stopping | Status::Recovery),
                                egui::Checkbox::new(&mut enabled, "Receive data"),
                            )
                            .changed()
                        {
                            toggle = Some((id, enabled));
                        }
                        if let Some(error) = &view.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        ui.add_enabled_ui(
                            !frozen && !matches!(view.status, Status::Stopping | Status::Recovery),
                            |ui| {
                                ui.collapsing("Source and QoS", |ui| {
                                    ui.label("Topic");
                                    ui.add(studio_ui_foundation::theme::input(
                                        ui,
                                        &mut view.draft.topic,
                                    ));
                                    ui.label("Message type");
                                    ui.add(studio_ui_foundation::theme::input(
                                        ui,
                                        &mut view.draft.message_type,
                                    ));
                                    let mut best =
                                        view.draft.qos.reliability == file::Reliability::BestEffort;
                                    if ui.checkbox(&mut best, "Best effort").changed() {
                                        view.draft.qos.reliability = if best {
                                            file::Reliability::BestEffort
                                        } else {
                                            file::Reliability::Reliable
                                        };
                                    }
                                    let mut retained = view.draft.qos.durability
                                        == file::Durability::TransientLocal;
                                    if ui.checkbox(&mut retained, "Transient local").changed() {
                                        view.draft.qos.durability = if retained {
                                            file::Durability::TransientLocal
                                        } else {
                                            file::Durability::Volatile
                                        };
                                    }
                                    if ui.button("Apply changes").clicked() {
                                        apply = Some(id);
                                    }
                                });
                                if matches!(view.saved.kind, file::ViewKind::Messages) {
                                    studio_ui_foundation::theme::strong(ui, "Plot a numeric field");
                                    plots::PlotRenderer::field_picker(
                                        ui,
                                        &view.fields,
                                        &mut view.field_draft,
                                    );
                                    if ui
                                        .add_enabled(
                                            view.field_draft.is_some(),
                                            egui::Button::new("Open plot"),
                                        )
                                        .clicked()
                                    {
                                        view.draft = view.saved.clone();
                                        view.draft.kind = file::ViewKind::numeric_plot(
                                            view.field_draft.as_ref().unwrap(),
                                        );
                                        apply = Some(id);
                                    }
                                }
                                if matches!(view.saved.kind, file::ViewKind::NumericPlot { .. }) {
                                    if ui.button("Return to Messages").clicked() {
                                        view.draft = view.saved.clone();
                                        view.draft.kind = file::ViewKind::Messages;
                                        apply = Some(id);
                                    }
                                    plots::PlotRenderer::show_details(ui, &view.trace);
                                }
                                if matches!(view.saved.kind, file::ViewKind::Image { .. }) {
                                    view.image.show_details(ui);
                                }
                            },
                        );
                        ui.collapsing("Reception diagnostics", |ui| {
                            ui.label(format!("Samples: {}", view.samples));
                            ui.label(format!("Backend queue drops: {}", view.backend_drops));
                            ui.label(format!("App queue drops: {}", view.queue_drops));
                            ui.small("Queue drops").on_hover_text(
                                "Application counters do not measure DDS or network loss.",
                            );
                        });
                        let position = self
                            .workspace
                            .layout
                            .order
                            .iter()
                            .position(|v| v == &view.saved.id)
                            .unwrap_or(0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!frozen && position > 0, egui::Button::new("Move up"))
                                .clicked()
                            {
                                reorder = Some((view.saved.id.clone(), position - 1));
                            }
                            if ui
                                .add_enabled(
                                    !frozen && position + 1 < self.workspace.layout.order.len(),
                                    egui::Button::new("Move down"),
                                )
                                .clicked()
                            {
                                reorder = Some((view.saved.id.clone(), position + 1));
                            }
                        });
                        if ui
                            .add_enabled(
                                !frozen && view.subscription.is_none(),
                                egui::Button::new("Remove view"),
                            )
                            .clicked()
                        {
                            remove = Some(id);
                        }
                    } else {
                        ui.label("Select a panel to inspect its settings.");
                    }
                });
        };
        if compact {
            if view_settings_open {
                egui::Window::new("View setup and settings")
                    .open(&mut view_settings_open)
                    .default_width(340.0)
                    .show(ctx, &mut inspector);
            }
        } else {
            egui::SidePanel::right("tools-inspector")
                .default_width(320.0)
                .resizable(true)
                .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(12))
                .show(ctx, &mut inspector);
        }
        self.view_settings_open = view_settings_open;
        if let Some((id, enabled)) = toggle {
            if enabled {
                self.start(id);
            } else {
                self.stop_view(id);
            }
        }
        if let Some(id) = apply {
            self.apply_draft(id);
        }
        if let Some((id, target)) = reorder {
            let _ = self.workspace.layout.reorder(&id, target);
        }
        if let Some(id) = remove {
            if let Some(index) = self.views.iter().position(|v| v.id == id) {
                let view = self.views.remove(index);
                self.workspace.layout.order.retain(|v| v != &view.saved.id);
                self.selected = None;
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(18))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Dashboard");
                    presentation::muted(ui, format!("{} / 8", self.views.len()));
                });
                ui.add_space(12.0);
                if self.views.is_empty() {
                    studio_ui_foundation::theme::strong(ui, "Inspect messages and signals");
                    presentation::muted(ui, "Choose a topic in settings to add a view.");
                }
                let ids = self.workspace.layout.order.clone();
                let height =
                    (ui.available_height() / self.views.len().max(1) as f32 - 92.0).max(220.0);
                egui::ScrollArea::vertical()
                    .id_salt("tools-views")
                    .show(ui, |ui| {
                        for saved_id in ids {
                            if let Some(view) =
                                self.views.iter_mut().find(|v| v.saved.id == saved_id)
                            {
                                ui.push_id(view.id.0, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        if presentation::tab(
                                            ui,
                                            &view.saved.topic,
                                            self.selected == Some(view.id),
                                        )
                                        .on_hover_text(&view.saved.topic)
                                        .clicked()
                                        {
                                            self.selected = Some(view.id);
                                        }
                                        presentation::muted(ui, view.status.label());
                                    });
                                    presentation::sample_summary(
                                        ui,
                                        view.samples,
                                        view.backend_drops,
                                        view.queue_drops,
                                    );
                                    if let Some(error) = &view.error {
                                        ui.colored_label(ui.visuals().error_fg_color, error);
                                    }
                                    ui.allocate_ui(
                                        egui::vec2(ui.available_width(), height),
                                        |ui| match &view.saved.kind {
                                            file::ViewKind::Image { .. } => {
                                                view.image.show_content_with_guidance(ui, false)
                                            }
                                            file::ViewKind::NumericPlot { .. } => {
                                                let path = view.saved.kind.plot_path();
                                                let status = match view.status {
                                                    Status::Stopped => plots::PlotStatus::Stopped,
                                                    Status::Starting => plots::PlotStatus::Starting,
                                                    Status::Waiting => plots::PlotStatus::Waiting,
                                                    Status::Receiving => {
                                                        plots::PlotStatus::Receiving
                                                    }
                                                    _ => plots::PlotStatus::Error(
                                                        "Stopping or cleanup required",
                                                    ),
                                                };
                                                plots::PlotRenderer.show_content_with_guidance(
                                                    ui,
                                                    &mut view.trace,
                                                    status,
                                                    path.as_ref(),
                                                    false,
                                                );
                                            }
                                            _ => {
                                                if let Some(sample) = &view.sample {
                                                    typed_view::show(ui, &sample.value);
                                                } else {
                                                    presentation::muted(
                                                        ui,
                                                        if view.status.active() {
                                                            "Waiting for data…"
                                                        } else {
                                                            "Start receiving in view settings."
                                                        },
                                                    );
                                                }
                                            }
                                        },
                                    );
                                    ui.separator();
                                });
                            }
                        }
                    });
            });
        self.workspace_windows(ctx);
    }
    fn workspace_windows(&mut self, ctx: &egui::Context) {
        if self.files_open {
            let mut open = true;
            egui::Window::new("Tools workspace")
                .open(&mut open)
                .resizable(true)
                .vscroll(true)
                .show(ctx, |ui| {
                    match self.files.status() {
                        "Workspace configuration retained after connection change" => {}
                        "Workspace restored inactive; reconnect and activate tasks explicitly" => {
                            presentation::muted(ui, "Workspace restored");
                        }
                        "Review the inactive workspace before replacing configuration" => {}
                        status => {
                            presentation::muted(ui, status);
                        }
                    }
                    ui.add_enabled(
                        !self.frozen(),
                        studio_ui_foundation::theme::input(ui, &mut self.workspace.name),
                    );
                    if let Some(error) = self.files.error() {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    ui.add_enabled_ui(!self.files.is_busy() && !self.frozen(), |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Open…").clicked() {
                                if let Err(e) = self.files.open() {
                                    self.status = e;
                                }
                            }
                            if ui.button("Save").clicked() {
                                if let Err(e) = self.files.save() {
                                    self.status = e;
                                }
                            }
                            if ui.button("Save as…").clicked() {
                                if let Err(e) = self.files.save_as() {
                                    self.status = e;
                                }
                            }
                        });
                        ui.collapsing("Local file path", |ui| {
                            ui.add(studio_ui_foundation::theme::input(ui, &mut self.path));
                            if ui.button("Open path").clicked() {
                                if let Err(e) = self.files.load_path(&self.path) {
                                    self.status = e;
                                }
                            }
                            if ui.button("Save to path").clicked() {
                                if let Err(e) = self.files.save_path(&self.path) {
                                    self.status = e;
                                }
                            }
                        });
                    });
                    if let Some(candidate) = self.files.candidate() {
                        studio_ui_foundation::theme::strong(ui, "Import preview");
                        ui.label(format!(
                            "{}: {} inspection views, {} task tabs",
                            candidate.workspace.name,
                            candidate.workspace.views.len(),
                            candidate
                                .workspace
                                .tools
                                .as_ref()
                                .map_or(0, |t| t.tasks.len())
                        ));
                        if let Some(tools) = &candidate.workspace.tools {
                            for task in &tools.tasks {
                                ui.label(format!(
                                    "Inactive: {} ({})",
                                    task.name, task.message_type
                                ));
                            }
                        }
                        for excluded in &candidate.excluded {
                            ui.label(format!("Excluded: {excluded}"));
                        }
                        for view in &candidate.workspace.views {
                            ui.label(format!("{}: {}", view.id, view.topic));
                        }
                        if let Some(issue) = &candidate.issue {
                            ui.colored_label(ui.visuals().error_fg_color, issue);
                        }
                        let ready = candidate.preview.is_some();
                        ui.checkbox(&mut self.discard, "Discard unsaved configuration");
                        if ui
                            .add_enabled(
                                ready && !self.frozen(),
                                egui::Button::new("Replace workspace"),
                            )
                            .clicked()
                        {
                            self.apply_workspace();
                        }
                        if ui.button("Review candidate again").clicked() {
                            if let Err(e) = self.files.review_candidate() {
                                self.status = e;
                            }
                        }
                    }
                    if (self.files.is_busy() || self.files.is_applying())
                        && ui.button("Cancel workspace operation").clicked()
                    {
                        self.files.cancel();
                    }
                });
            self.files_open = open;
        }
        if self.about {
            egui::Window::new("About ROS Studio Tools")
                .open(&mut self.about)
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.label(concat!("ROS Studio Tools ", env!("CARGO_PKG_VERSION")));
                    ui.heading("Appearance");
                    ui.horizontal(|ui| {
                        for (name, pref) in [
                            ("System", egui::ThemePreference::System),
                            ("Light", egui::ThemePreference::Light),
                            ("Dark", egui::ThemePreference::Dark),
                        ] {
                            if ui
                                .selectable_label(ctx.options(|o| o.theme_preference) == pref, name)
                                .clicked()
                            {
                                ctx.set_theme(pref);
                            }
                        }
                    });
                    ui.collapsing("Font license", |ui| {
                        ui.label(studio_ui_foundation::fonts::NOTICE);
                    });
                });
        }
    }
}
impl eframe::App for Tools {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        diagnostics::begin_frame(ctx);
        self.diagnostics.shortcut(ctx);
        let poll_start = Instant::now();
        self.poll();
        let poll_ms = poll_start.elapsed().as_secs_f64() * 1000.0;
        let files_start = Instant::now();
        self.poll_files(ctx);
        let files_ms = files_start.elapsed().as_secs_f64() * 1000.0;
        let name = self
            .files
            .path()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled workspace".into());
        let title = format!(
            "{}{} - ROS Studio Tools",
            name,
            if self.files.dirty() { " *" } else { "" }
        );
        if title != self.last_title {
            self.last_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        let ui_start = Instant::now();
        self.ui(ctx);
        diagnostics::timing(
            ctx,
            poll_ms,
            files_ms,
            ui_start.elapsed().as_secs_f64() * 1000.0,
        );
        let state = self.diagnostics.wants_capture().then(|| {
            let (readers,available,rejected)=self.session.as_ref().map(|s| {
                let shared=s.shared.lock().unwrap();
                (shared.registry.entries().len(),shared.registry.available_resources(),shared.ingress_rejections)
            }).unwrap_or_default();
            serde_json::json!({
                "connection":{"connected":self.connected,"connecting":self.connecting,"has_session":self.session.is_some(),
                    "retiring":self.retiring.is_some(),"generation":self.generation,"domain":domain_id(&self.domain).ok(),
                    "requested_middleware":if self.cyclone {"Cyclone DDS"} else {"Fast DDS"}},
                "current_issues":{"connection_error":if !self.connected&&!self.connecting&&!self.status.starts_with("Disconnected")&&!self.status.starts_with("Disconnecting") {Some(diagnostics::error_category(&self.status))}else{None},
                    "workspace_error":self.files.error().is_some(),"workbench_error":self.workbench.diagnostic_state()["has_error"]},
                "recorded_history":self.diagnostic_history,
                "counters":{"readers":readers,"available_native_resources":available,"rejected_events":rejected,
                    "ui_drops":self.views.iter().fold(0u64,|n,v|n.saturating_add(v.queue_drops)),
                    "backend_drops":self.views.iter().fold(0u64,|n,v|n.saturating_add(v.backend_drops)),
                    "note":"Application counters do not measure DDS/network loss"},
                "ros_health":self.health.summary(),
                "app_specific":{"workspace_version":9,"frozen":self.frozen(),"workspace_busy":self.files.is_busy(),
                    "windows":{"workspace":self.files_open,"help":self.help_open,"about":self.about,"view_settings":self.view_settings_open},
                    "workbench":self.workbench.diagnostic_state(),
                    "views":self.views.iter().take(8).map(|v|serde_json::json!({
                        "id":v.id.0,"topic":diagnostics::short(&v.saved.topic),"type":diagnostics::short(&v.saved.message_type),
                        "status":v.status.label(),"subscription":v.subscription.map(|s|s.0),"received":v.samples,
                        "best_effort":v.config().best_effort,"transient_local":v.config().transient_local,
                        "backend_drops":v.backend_drops,"app_drops":v.queue_drops,"has_error":v.error.is_some(),
                        "error_category":v.error.as_deref().map(diagnostics::error_category)
                    })).collect::<Vec<_>>()}
            })
        });
        let enabled = self.connected && !self.frozen();
        self.health_view.set_concise(true);
        if self.diagnostics.finish_frame_with_tabs(
            ctx,
            "Tools",
            state,
            |ui| {
                self.health.ui(
                    &mut self.health_view,
                    ui,
                    &self.topics,
                    self.session.as_ref(),
                    enabled,
                )
            },
            Some(&mut |ui, health| {
                if let Some(index) = presentation::tab_strip(
                    ui,
                    "diagnostic-tabs",
                    &["Application", "ROS health"],
                    if *health { "ROS health" } else { "Application" },
                ) {
                    *health = index == 1;
                }
            }),
        ) {
            self.diagnostic_history.clear();
        }
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    fn on_exit(&mut self) {
        self.network.finish_shutdown();
        self.disconnect();
        transfers::request_shutdown();
        self.files.shutdown();
        session::finish_shutdown();
        transfers::finish_shutdown();
    }
}
impl Drop for Tools {
    fn drop(&mut self) {
        self.disconnect();
        self.files.request_shutdown();
    }
}
fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--suite-configuration") {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"distribution":if studio_network::MANAGED_FIREWALL_AVAILABLE {"bundled"} else {"microsoft-store"}})
        );
        return Ok(());
    }
    let _lease = studio_app_lifecycle::state_directory()
        .and_then(|state| studio_app_lifecycle::AppLease::running(&state, "tools"))
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;
    // Reproducible native compact-layout acceptance without changing OS settings.
    // Only a debug build in the explicitly isolated synthetic fixture may use it.
    let compact_fixture = cfg!(debug_assertions)
        && std::env::var("STUDIO_TOOLS_COMPACT_FIXTURE").as_deref() == Ok("1")
        && std::env::var("ROS_DOMAIN_ID").as_deref() == Ok("187")
        && std::env::var("ROS_LOCALHOST_ONLY").as_deref() == Ok("1");
    let size = if compact_fixture {
        [820.0, 680.0]
    } else {
        [1280.0, 820.0]
    };
    eframe::run_native(
        "ROS Studio Tools",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(size)
                .with_icon(studio_ui_foundation::app_icons::AppIcon::Tools.window_icon())
                .with_min_inner_size([720.0, 560.0]),
            ..Default::default()
        },
        Box::new(|cc| {
            studio_ui_foundation::fonts::configure(&cc.egui_ctx);
            presentation::configure(&cc.egui_ctx);
            Ok(Box::new(Tools::default()))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_configuration_cannot_create_spatial_readers() {
        let app = Tools::default();
        assert_eq!(domain_id(&app.domain), Ok(0));
        assert!(app.views.is_empty());
        assert!(app.session.is_none());
        for view in file::Preset::Analyze.workspace().views {
            let config = configuration(&view);
            assert_eq!(config.output, Output::Typed);
            assert!(config.spatial.is_none());
            assert!(config.robot.is_none());
        }
    }
    #[test]
    fn disabled_and_disconnected_views_clear_live_data() {
        let mut view = ToolView::new(ViewId(1), file::Preset::Inspect.workspace().views.remove(0));
        view.subscription = Some(SubscriptionId(1));
        view.status = Status::Receiving;
        view.deactivate();
        assert!(view.subscription.is_none());
        assert_eq!(view.status, Status::Stopped);
        assert!(view.sample.is_none());
    }
    #[test]
    fn headless_tools_empty_and_workspace_states_render() {
        let ctx = egui::Context::default();
        let mut app = Tools::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
        app.files_open = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.ui(ctx));
    }
}

#[cfg(test)]
#[path = "real_ros.rs"]
mod real_ros;

#[cfg(test)]
mod diagnostic_app_tests {
    use super::*;
    use studio_client_core::{
        diagnostic_acceptance::{self, Host},
        diagnostic_health::Monitor,
    };
    impl Host for Tools {
        fn poll(&mut self) {
            Tools::poll(self);
        }
        fn monitor(&self) -> &Monitor {
            &self.health
        }
        fn topics(&self) -> &[(String, Vec<String>)] {
            &self.topics
        }
        fn start(&mut self, topic: &str, best: bool, transient: bool) {
            self.health.topic = topic.into();
            self.health.best_effort = best;
            self.health.transient_local = transient;
            self.health
                .start(self.session.as_ref().unwrap(), &self.topics);
        }
        fn stop(&mut self) {
            self.health.stop(self.session.as_ref().unwrap());
        }
        fn disconnect(&mut self) {
            Tools::disconnect(self);
        }
        fn connect(&mut self) {
            Tools::connect(self, &egui::Context::default());
        }
        fn connected(&self) -> bool {
            self.connected
        }
    }
    #[test]
    #[ignore = "Requires isolated local ROS diagnostic fixture on domain187"]
    fn health_real_ros_tools() {
        diagnostic_acceptance::guard();
        let _finish = diagnostic_acceptance::Finish;
        let mut app = Tools::default();
        app.domain = "187".into();
        app.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
        Host::connect(&mut app);
        diagnostic_acceptance::run(&mut app, "tools");
    }
}
