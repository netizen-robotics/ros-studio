//! Explicit asset draft -> review -> inactive install. No filesystem work on UI frames.
use super::*;
use crate::components::{
    robot::{
        self, AssetWorker, LoadId, LoadOptions, LoadedRobot, RobotDraft, RobotLive, RobotReview,
        RobotSetupAction, RobotSetupState,
    },
    workspace::RobotAssets,
};
use std::{collections::BTreeMap, path::PathBuf};
use studio_protocol::spatial_input::SpatialClockPolicy;
#[cfg(test)]
#[path = "robot_setup/ros_tests.rs"]
mod ros_tests;

#[derive(Clone)]
struct Reviewed {
    id: LoadId,
    assets: RobotAssets,
    description: Option<crate::components::workspace::RobotTopicSource>,
    loaded: LoadedRobot,
    summary: RobotReview,
}
impl Reviewed {
    fn new(
        id: LoadId,
        assets: RobotAssets,
        description: Option<crate::components::workspace::RobotTopicSource>,
        loaded: LoadedRobot,
    ) -> Self {
        let summary = RobotReview::from_loaded(&loaded);
        Self {
            id,
            assets,
            description,
            loaded,
            summary,
        }
    }
}
struct PendingLoad {
    id: LoadId,
    generation: u64,
    assets: RobotAssets,
    description: Option<crate::components::workspace::RobotTopicSource>,
}
struct Install {
    generation: u64,
    target: Option<ViewId>,
    prior_model: Option<LoadId>,
    draft: RobotDraft,
    reviewed: Reviewed,
}
struct DescriptionCapture {
    owner: StartIntent,
    setup_generation: u64,
    source: crate::components::workspace::RobotTopicSource,
    assets: RobotAssets,
    stream: backend_pipe::DescriptionStream,
    result: Option<Result<backend_pipe::CapturedDescription, String>>,
    stop_request: Option<RequestId>,
    cancelled: bool,
    cleaned: bool,
    recovery: bool,
    deadline: Option<Instant>,
    backend_drops: u64,
}
#[derive(Default)]
pub(super) struct RobotSetup {
    worker: Option<AssetWorker>,
    generation: u64,
    draft: Option<RobotDraft>,
    target: Option<ViewId>,
    reviewed: Option<Reviewed>,
    load: Option<PendingLoad>,
    install: Option<Install>,
    error: Option<String>,
    capture: Option<DescriptionCapture>,
    captured_summary: Option<(usize, String)>,
}
impl RobotSetup {
    pub(super) fn current_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn is_open(&self) -> bool {
        self.draft.is_some()
    }
    fn cancel(&mut self) {
        if let (Some(worker), Some(load)) = (&mut self.worker, &self.load) {
            worker.cancel(load.id);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("Robot setup identities exhausted");
        self.draft = None;
        self.reviewed = None;
        self.target = None;
        self.install = None;
        self.error = None;
        self.captured_summary = None;
        // Keep the in-flight identity until its completion is consumed. The
        // worker stays Busy during cancellation; no replacement job is queued.
    }
    pub(super) fn disconnected(&mut self) {
        if let Some(capture) = self.capture.take() {
            capture.stream.stop();
        }
        self.captured_summary = None;
        if self.draft.as_ref().is_some_and(|d| d.description.is_some()) {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("Robot setup identities exhausted");
            self.reviewed = None;
            if let (Some(worker), Some(load)) = (&mut self.worker, &self.load) {
                worker.cancel(load.id);
            }
        }
        self.install = None;
        if let Some(draft) = &mut self.draft {
            draft.clock_verified = false;
            draft.tf.clock_policy = SpatialClockPolicy::Unknown;
        }
    }
    pub(super) fn finish_shutdown(&mut self) -> Result<(), String> {
        self.cancel();
        let result = self.worker.as_mut().map_or(Ok(()), |worker| {
            worker.finish_shutdown().map_err(|e| e.to_string())
        });
        self.worker = None;
        self.load = None;
        result
    }
}
impl App {
    pub(super) fn robot_review_can_resume(&self) -> bool {
        self.robot_setup.capture.is_none()
            && self.robot_setup.load.is_none()
            && self.robot_setup.install.is_none()
            && self
                .robot_setup
                .draft
                .as_ref()
                .zip(self.robot_setup.reviewed.as_ref())
                .is_some_and(|(d, r)| d.assets == r.assets && d.description == r.description)
    }
    pub(super) fn return_to_robot_review(&mut self) {
        if !self.robot_review_can_resume() || self.workspace_frozen() {
            return;
        }
        self.scene_settings.open = false;
        let draft = self.robot_setup.draft.as_mut().unwrap();
        draft.tf = self.scene_settings.configuration.clone();
        draft.clock_verified = self.scene_settings.verified;
        self.setup_target = self.robot_setup.target;
        self.setup = true;
        self.details_open = true;
        self.compact_inspector_open = true;
        self.inspector = true;
    }
    pub(super) fn robot_capture_health_ui(&mut self, ui: &mut egui::Ui) {
        let Some(capture) = &self.robot_setup.capture else {
            return;
        };
        let recovery = capture.recovery;
        let detail = format!("Description: {} · backend queue drops: {}. Reader ownership remains reserved until cleanup.", capture.source.topic, capture.backend_drops);
        if ui
            .small_button(if recovery {
                "Retry description cleanup"
            } else {
                "Cancel description"
            })
            .on_hover_text(detail)
            .clicked()
        {
            if let Err(error) = self.robot_stop_capture(true, recovery) {
                self.robot_setup.error = Some(error.clone());
                self.diagnostic(error);
            }
        }
    }
    fn robot_read_description(&mut self) -> Result<(), String> {
        if self.connection.phase != Phase::Connected {
            return Err("Connect before reading a description topic".into());
        }
        if self.robot_setup.capture.is_some()
            || self
                .robot_setup
                .worker
                .as_ref()
                .is_some_and(AssetWorker::is_busy)
        {
            return Err("Wait for the current capture/load cleanup before reading again".into());
        }
        let draft = self
            .robot_setup
            .draft
            .as_ref()
            .ok_or("Open Robot setup first")?;
        let source = draft
            .description
            .clone()
            .ok_or("Choose a description topic first")?;
        source.validate().map_err(|e| e.to_string())?;
        if !self.discovery.topics.iter().any(|(topic, types)| {
            *topic == source.topic && types.iter().any(|ty| ty == &source.message_type)
        }) {
            return Err("Choose a discovered exact std_msgs/msg/String topic".into());
        }
        let configuration = Configuration {
            map_updates: None,
            topic: source.topic.clone(),
            message_type: source.message_type.clone(),
            best_effort: source.qos.reliability
                == crate::components::workspace::Reliability::BestEffort,
            transient_local: source.qos.durability
                == crate::components::workspace::Durability::TransientLocal,
            output: Output::RobotDescription,
            spatial: None,
            robot: None,
        };
        let next = self
            .next_view
            .checked_add(1)
            .ok_or("View identities exhausted")?;
        let stream = backend_pipe::DescriptionStream::new(source.topic.clone())?;
        let session = self.session.as_ref().ok_or("Connect before capture")?;
        let owner = {
            let mut shared = session.shared.lock().unwrap();
            let owner = shared
                .registry
                .start_explicit(ViewId(next), configuration)
                .map_err(|e| format!("Description reader unavailable: {e:?}"))?;
            let binding = shared
                .queue
                .register(owner.generation, owner.subscription)
                .map_err(|e| format!("Description queue: {e:?}"))
                .and_then(|()| {
                    shared.bind_description(backend_pipe::DescriptionTarget {
                        generation: owner.generation,
                        subscription: owner.subscription,
                        topic: source.topic.clone(),
                        stream: stream.clone(),
                    })
                });
            if let Err(error) = binding {
                let _ = shared.registry.request_error(
                    owner.generation,
                    owner.subscription,
                    owner.request,
                );
                let _ = shared.queue.unregister(owner.subscription);
                return Err(error);
            }
            owner
        };
        self.next_view = next;
        let command = Command::Start {
            generation: owner.generation,
            request: owner.request,
            subscription: owner.subscription,
            configuration: owner.configuration.clone(),
        };
        self.robot_setup.capture = Some(DescriptionCapture {
            owner,
            setup_generation: self.robot_setup.generation,
            source,
            assets: draft.assets.clone(),
            stream,
            result: None,
            stop_request: None,
            cancelled: false,
            cleaned: false,
            recovery: false,
            deadline: Some(Instant::now() + Duration::from_secs(10)),
            backend_drops: 0,
        });
        self.robot_setup.reviewed = None;
        self.robot_setup.captured_summary = None;
        self.robot_setup.error = None;
        if let Err(error) = session.send(command) {
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }
    fn robot_cancel_capture(&mut self) {
        if let Err(error) = self.robot_stop_capture(true, false) {
            self.robot_setup.error = Some(error);
        }
    }
    fn robot_stop_capture(&mut self, cancelled: bool, retry: bool) -> Result<(), String> {
        let Some(capture) = self.robot_setup.capture.as_mut() else {
            return Ok(());
        };
        if cancelled {
            capture.cancelled = true;
            capture.result = None;
        }
        capture.stream.stop();
        if capture.cleaned || (capture.stop_request.is_some() && !retry) {
            return Ok(());
        }
        let session = self
            .session
            .as_ref()
            .ok_or("Capture cleanup requires the current connection")?;
        let stop = {
            let mut shared = session.shared.lock().unwrap();
            if shared.registry.generation() == capture.owner.generation
                && !shared
                    .registry
                    .entries()
                    .iter()
                    .any(|entry| entry.subscription == capture.owner.subscription)
            {
                capture.cleaned = true;
                return Ok(());
            }
            let stop = shared
                .registry
                .stop_explicit(capture.owner.subscription)
                .map_err(|e| format!("Description Stop: {e:?}; disconnect to recover"))?;
            let _ = shared.queue.unregister(capture.owner.subscription);
            shared
                .descriptions
                .retain(|target| target.subscription != capture.owner.subscription);
            stop
        };
        capture.stop_request = Some(stop.request);
        capture.recovery = false;
        capture.deadline = Some(Instant::now() + Duration::from_secs(10));
        if let Err(error) = session.send(Command::Stop {
            generation: stop.generation,
            request: stop.request,
            subscription: stop.subscription,
        }) {
            self.fail(error.clone());
            return Err(error);
        }
        Ok(())
    }
    pub(super) fn robot_capture_stop_owner(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        retry: bool,
    ) -> bool {
        if !self.robot_setup.capture.as_ref().is_some_and(|c| {
            c.owner.generation == generation && c.owner.subscription == subscription
        }) {
            return false;
        }
        if let Err(error) = self.robot_stop_capture(true, retry) {
            self.workspace_stop_failed(generation, subscription, &error);
            self.robot_setup.error = Some(error);
        }
        true
    }
    pub(super) fn robot_capture_event(&mut self, event: &Event) -> bool {
        let Some(capture) = self.robot_setup.capture.as_ref() else {
            return false;
        };
        let (generation, subscription) = (capture.owner.generation, capture.owner.subscription);
        let matching = match event {
            Event::Started {
                generation: g,
                subscription: s,
                ..
            }
            | Event::Stopped {
                generation: g,
                subscription: s,
                ..
            }
            | Event::RequestError {
                generation: g,
                subscription: s,
                ..
            }
            | Event::StreamError {
                generation: g,
                subscription: s,
                ..
            }
            | Event::Drops {
                generation: g,
                subscription: s,
                ..
            } => *g == generation && *s == subscription,
            _ => false,
        };
        if !matching {
            return false;
        }
        let cleaned = self.workspace_cleanup_is_confirmed(generation, subscription);
        let capture = self.robot_setup.capture.as_mut().unwrap();
        let mut failure = None;
        match event {
            Event::Started { request, .. }
                if *request == capture.owner.request && capture.stop_request.is_none() =>
            {
                capture.deadline = None
            }
            Event::Drops { backend_drops, .. } => {
                capture.backend_drops = capture.backend_drops.max(*backend_drops)
            }
            Event::Stopped { request, .. } if Some(*request) == capture.stop_request && cleaned => {
                capture.cleaned = true
            }
            Event::RequestError {
                request,
                no_handle,
                message,
                ..
            } => {
                if *no_handle && cleaned {
                    capture.cleaned = true;
                    capture.result = Some(Err(message.clone()));
                } else if Some(*request) == capture.stop_request {
                    capture.recovery = true;
                    capture.deadline = None;
                    failure = Some(message.clone());
                }
            }
            Event::StreamError { message, .. } if cleaned => {
                capture.cleaned = true;
                capture.result = Some(Err(message.clone()));
            }
            _ => (),
        }
        if capture.cleaned {
            self.workspace_cleanup_confirmed(generation, subscription);
        }
        if let Some(error) = failure {
            self.workspace_stop_failed(generation, subscription, &error);
            self.robot_setup.error = Some(error);
        }
        true
    }
    fn robot_capture_poll(&mut self) {
        let mut needs_stop = false;
        let mut timed_out_stop = None;
        if let Some(capture) = &mut self.robot_setup.capture {
            if !capture.cancelled && capture.stop_request.is_none() {
                if let Some(result) = capture.stream.take() {
                    capture.result = Some(result);
                    needs_stop = true;
                }
            }
            if capture.deadline.is_some_and(|d| Instant::now() >= d) && !capture.cleaned {
                capture.deadline = None;
                capture.recovery = true;
                self.robot_setup.error = Some(
                    "Description operation timed out. Retry Stop or disconnect to confirm cleanup."
                        .into(),
                );
                if capture.stop_request.is_none() {
                    needs_stop = true;
                    capture.cancelled = true;
                    capture.result = None;
                } else {
                    timed_out_stop = Some((capture.owner.generation, capture.owner.subscription));
                }
            }
        }
        if let Some((generation, subscription)) = timed_out_stop {
            self.workspace_stop_failed(
                generation,
                subscription,
                "Description Stop timed out; retry cleanup or disconnect",
            );
        }
        if needs_stop {
            if let Err(error) = self.robot_stop_capture(false, false) {
                self.robot_setup.error = Some(error);
            }
        }
        if !self.robot_setup.capture.as_ref().is_some_and(|c| c.cleaned) {
            return;
        }
        let capture = self.robot_setup.capture.take().unwrap();
        if capture.cancelled
            || capture.setup_generation != self.robot_setup.generation
            || !self.robot_setup.draft.as_ref().is_some_and(|d| {
                d.description.as_ref() == Some(&capture.source) && d.assets == capture.assets
            })
        {
            return;
        }
        let Some(result) = capture.result else {
            return;
        };
        match result {
            Err(error) => self.robot_setup.error = Some(error),
            Ok(description) => {
                let prepared = (|| -> Result<(), String> {
                    if self.robot_setup.worker.is_none() {
                        self.robot_setup.worker =
                            Some(AssetWorker::new().map_err(|e| e.to_string())?);
                    }
                    let source = robot::LoadSource::Description {
                        text: description.text.clone(),
                        asset_root: capture.source.asset_root.as_ref().map(PathBuf::from),
                    };
                    let options = LoadOptions {
                        package_roots: capture
                            .source
                            .package_roots
                            .iter()
                            .map(|root| (root.package.clone(), PathBuf::from(&root.path)))
                            .collect(),
                    };
                    let id = self
                        .robot_setup
                        .worker
                        .as_mut()
                        .unwrap()
                        .start_source(source, options)
                        .map_err(|e| e.to_string())?;
                    self.robot_setup.load = Some(PendingLoad {
                        id,
                        generation: capture.setup_generation,
                        assets: capture.assets,
                        description: Some(capture.source),
                    });
                    self.robot_setup.captured_summary =
                        Some((description.len(), description.sha256));
                    Ok(())
                })();
                if let Err(error) = prepared {
                    self.robot_setup.error = Some(error);
                }
            }
        }
    }
    pub(super) fn cancel_robot_setup(&mut self) {
        if self.robot_setup.is_open() {
            self.setup = false;
        }
        self.robot_cancel_capture();
        self.robot_setup.cancel();
    }
    pub(super) fn begin_robot_topic(&mut self, topic: &str) {
        self.begin_robot_setup(None);
        if let Some(draft) = &mut self.robot_setup.draft {
            if let Some(source) = &mut draft.description {
                source.topic = topic.to_owned();
            }
        }
    }
    pub(super) fn begin_robot_setup(&mut self, target: Option<ViewId>) {
        self.details_open = true;
        self.compact_inspector_open = true;
        if !self.allow_source_navigation() {
            return;
        }
        self.sidebar = true;
        if self.workspace_frozen() {
            return;
        }
        if target.is_none() && self.views.len() >= MAX_SUBSCRIPTIONS {
            return;
        }
        self.cancel_robot_setup();
        let mut draft = RobotDraft {
            description: target.is_none().then(Default::default),
            ..Default::default()
        };
        let mut reviewed = None;
        if let Some(id) = target {
            let Some(view) = self
                .views
                .iter()
                .find(|v| v.id == id && matches!(v.kind, ViewKind::Robot))
            else {
                return;
            };
            draft.assets = view.robot_assets.clone().unwrap_or_default();
            draft.description = view.robot_description.clone();
            self.robot_setup.captured_summary = view.robot_description_summary.clone();
            if let Some(c) = &view.configuration.robot {
                draft.tf = studio_protocol::spatial::Configuration {
                    fixed_frame: c.fixed_frame.clone(),
                    clock_policy: SpatialClockPolicy::Unknown,
                    dynamic_tf_topic: c.dynamic_tf_topic.clone(),
                    dynamic_tf_best_effort: c.dynamic_tf_best_effort,
                    static_tf_topic: c.static_tf_topic.clone(),
                };
            }
            draft.clock_verified = view.spatial_clock_verified;
            if let Some(live) = &view.robot {
                reviewed = Some(Reviewed::new(
                    live.model_id(),
                    draft.assets.clone(),
                    draft.description.clone(),
                    live.loaded().clone(),
                ));
            }
            self.selected = Some(id);
        }
        draft.tf = self.scene_settings.configuration.clone();
        draft.clock_verified = self.scene_settings.verified;
        self.catalog_open = false;
        self.scene_settings.open = false;
        self.robot_setup.target = target;
        self.robot_setup.draft = Some(draft);
        self.robot_setup.reviewed = reviewed;
        self.setup_target = target;
        self.setup = true;
        self.inspector = true;
    }
    fn robot_load(&mut self) -> Result<(), String> {
        if self.workspace_frozen() || self.robot_setup.install.is_some() {
            return Err("Wait for the current replacement to finish".into());
        }
        let draft = self
            .robot_setup
            .draft
            .as_ref()
            .ok_or("Open Robot setup before loading")?;
        if draft.description.is_some() {
            return self.robot_read_description();
        }
        draft.assets.validate().map_err(|e| e.to_string())?;
        let assets = draft.assets.clone();
        if self.robot_setup.worker.is_none() {
            self.robot_setup.worker = Some(AssetWorker::new().map_err(|e| e.to_string())?);
        }
        let options = LoadOptions {
            package_roots: assets
                .package_roots
                .iter()
                .map(|root| (root.package.clone(), PathBuf::from(&root.path)))
                .collect::<BTreeMap<_, _>>(),
        };
        let id = self
            .robot_setup
            .worker
            .as_mut()
            .unwrap()
            .start(PathBuf::from(&assets.urdf_path), options)
            .map_err(|e| e.to_string())?;
        self.robot_setup.load = Some(PendingLoad {
            id,
            generation: self.robot_setup.generation,
            assets,
            description: None,
        });
        // A new accepted attempt revokes the previous candidate even when the
        // paths are identical. The installed active model remains untouched.
        self.robot_setup.reviewed = None;
        self.robot_setup.error = None;
        self.robot_setup.captured_summary = None;
        self.robot_setup.draft.as_mut().unwrap().clock_verified = self.scene_settings.verified;
        Ok(())
    }
    fn robot_apply(&mut self) -> Result<(), String> {
        if self.workspace_frozen() || self.robot_setup.install.is_some() {
            return Err("Wait for the current replacement to finish".into());
        }
        if self.robot_setup.capture.is_some() {
            return Err("Wait for description cleanup before applying".into());
        }
        if self
            .robot_setup
            .worker
            .as_ref()
            .is_some_and(AssetWorker::is_busy)
        {
            return Err("Wait for model loading or cancellation to finish".into());
        }
        let draft = self
            .robot_setup
            .draft
            .clone()
            .ok_or("Open Robot setup before applying")?;
        let reviewed = self
            .robot_setup
            .reviewed
            .clone()
            .ok_or("Load and review a model before applying")?;
        if draft.assets != reviewed.assets || draft.description != reviewed.description {
            return Err(
                "Asset locations changed; Load and review the current settings first".into(),
            );
        }
        // Validate a configuration-only record before stopping or replacing
        // anything. The asset/model ID and exact frame names are checked too.
        let configuration =
            robot_configuration(&draft, &reviewed, self.connection.phase == Phase::Connected);
        let probe = workspace_robot(&draft, self.connection.phase == Phase::Connected);
        let views = vec![probe];
        let document = crate::components::workspace::Workspace {
            tools: None,
            presentation: None,
            version: crate::components::workspace::FORMAT_VERSION,
            name: "Robot setup".into(),
            panels: crate::components::workspace::Panels {
                data_open: true,
                inspector_open: true,
                diagnostics_open: false,
            },
            layout: crate::components::workspace::Layout::for_views(&views),
            views,
            connection_suggestion: None,
        };
        studio_workspace::application::encode(
            &document,
            studio_workspace::application::Application::Visualizer,
        )
        .map_err(|e| e.to_string())?;
        RobotLive::new(reviewed.id, reviewed.loaded.clone()).map_err(|e| e.to_string())?;
        // Exact identities are validated without granting runtime activation.
        let mut validated = configuration.robot.as_ref().unwrap().as_ref().clone();
        validated.clock_policy = SpatialClockPolicy::VerifiedSystem;
        validated.validate()?;
        let target = self.robot_setup.target;
        let prior_model = if let Some(id) = target {
            let view = self
                .views
                .iter()
                .find(|v| v.id == id && matches!(v.kind, ViewKind::Robot))
                .ok_or("Robot target changed; review again")?;
            if matches!(
                view.status,
                ViewStatus::Stopping | ViewStatus::NeedsRecovery
            ) {
                return Err("Wait for cleanup before replacing this model".into());
            }
            view.robot.as_ref().map(RobotLive::model_id)
        } else {
            if self.views.len() >= MAX_SUBSCRIPTIONS {
                return Err("32 displays are already configured".into());
            }
            self.next_view
                .checked_add(1)
                .ok_or("View identities exhausted")?;
            None
        };
        self.robot_setup.install = Some(Install {
            generation: self.robot_setup.generation,
            target,
            prior_model,
            draft,
            reviewed,
        });
        if let Some(id) = target {
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.enabled = false;
                view.pending = None;
            }
            self.stop_view(id);
        }
        self.robot_poll();
        Ok(())
    }
    pub(super) fn robot_poll(&mut self) {
        self.robot_capture_poll();
        let completion = self.robot_setup.worker.as_mut().and_then(AssetWorker::poll);
        if let Some(completion) = completion {
            if let Some(load) = self.robot_setup.load.take() {
                if load.id == completion.id
                    && load.generation == self.robot_setup.generation
                    && self.robot_setup.draft.is_some()
                {
                    match completion.result {
                        Ok(loaded) => {
                            self.robot_setup.reviewed = Some(Reviewed::new(
                                completion.id,
                                load.assets,
                                load.description,
                                loaded,
                            ))
                        }
                        Err(error) => self.robot_setup.error = Some(error.to_string()),
                    }
                }
            }
        }
        if self.workspace_frozen() {
            return;
        }
        let Some(install) = &self.robot_setup.install else {
            return;
        };
        if install.generation != self.robot_setup.generation {
            self.robot_setup.install = None;
            return;
        }
        if let Some(id) = install.target {
            let Some(view) = self.views.iter().find(|v| v.id == id) else {
                self.robot_setup.install = None;
                self.robot_setup.error = Some("Robot target was removed; review again".into());
                return;
            };
            if !matches!(view.kind, ViewKind::Robot)
                || view.robot.as_ref().map(RobotLive::model_id) != install.prior_model
            {
                self.robot_setup.install = None;
                self.robot_setup.error = Some("Robot target model changed; review again".into());
                return;
            }
            if view.subscription.is_some() {
                return;
            }
        }
        let install = self.robot_setup.install.take().unwrap();
        let result = self.robot_install(install);
        match result {
            Ok(()) => {
                self.robot_setup.cancel();
                self.setup = false;
                self.setup_target = None;
            }
            Err(error) => self.robot_setup.error = Some(error),
        }
    }
    fn robot_install(&mut self, install: Install) -> Result<(), String> {
        let connected = self.connection.phase == Phase::Connected;
        let configuration = robot_configuration(&install.draft, &install.reviewed, connected);
        let mut live = RobotLive::new(install.reviewed.id, install.reviewed.loaded)
            .map_err(|e| e.to_string())?;
        let mut appearance = install
            .target
            .and_then(|id| self.views.iter().find(|v| v.id == id))
            .map_or_else(Default::default, |old| {
                let mut appearance = old.robot_appearance.clone();
                let same_source = match (&old.robot_description, &install.draft.description) {
                    (Some(old), Some(new)) => old == new,
                    (None, None) => old.robot_assets.as_ref() == Some(&install.draft.assets),
                    _ => false,
                };
                if !same_source {
                    appearance.links.clear();
                }
                appearance
            });
        // Same-source descriptions may change their link set between reviews.
        // Reject stale names rather than applying an override by link index.
        live.set_appearance(appearance.clone())
            .map_err(|e| format!("{e}. Reset Robot appearance before applying this model."))?;
        let id = if let Some(id) = install.target {
            id
        } else {
            if self.views.len() >= MAX_SUBSCRIPTIONS {
                return Err("32 displays are already configured".into());
            }
            self.next_view = self
                .next_view
                .checked_add(1)
                .ok_or("View identities exhausted")?;
            ViewId(self.next_view)
        };
        let mut replacement = ViewState::new(id, configuration);
        replacement.robot_appearance = std::mem::take(&mut appearance);
        replacement.robot_description = install.draft.description;
        replacement.robot_description_summary = self.robot_setup.captured_summary.clone();
        replacement.robot_assets = Some(install.draft.assets);
        replacement.robot = Some(live);
        replacement.spatial_clock_verified = connected && install.draft.clock_verified;
        replacement.workspace_clock_expectation = if replacement.spatial_clock_verified {
            crate::components::workspace::ClockExpectation::System
        } else {
            crate::components::workspace::ClockExpectation::Unknown
        };
        replacement.robot_status = "Model reviewed · disabled until Enable display".into();
        if let Some(index) = self.views.iter().position(|view| view.id == id) {
            replacement.workspace_id = self.views[index].workspace_id.clone();
            replacement.robot_view.camera = self.views[index].robot_view.camera.clone();
            self.views[index] = replacement;
        } else {
            self.views.push(replacement);
            self.workspace_adopt_new_view(id);
        }
        self.selected = Some(id);
        Ok(())
    }
    pub(super) fn robot_setup_ui(&mut self, ui: &mut egui::Ui) {
        if self.robot_setup.install.is_some() {
            studio_ui_foundation::theme::strong(ui, "Waiting for display cleanup");
            ui.label("The reviewed model will be installed disabled after the previous TF group is released.");
            if ui.button("Cancel replacement").clicked() {
                self.cancel_robot_setup();
                self.setup = false;
            }
            return;
        }
        if ui.button("Review Scene settings").clicked() {
            self.open_scene_settings();
            return;
        }
        let tf_topics: Vec<_> = self
            .discovery
            .topics
            .iter()
            .filter(|(_, types)| types.iter().any(|ty| ty == spatial::TF_MESSAGE_TYPE))
            .map(|(name, _)| name.clone())
            .collect();
        let target_active = self
            .robot_setup
            .target
            .and_then(|id| self.views.iter().find(|v| v.id == id))
            .is_some_and(|v| v.subscription.is_some());
        if self
            .robot_setup
            .capture
            .as_ref()
            .is_some_and(|c| c.recovery)
            && ui.button("Retry description Stop").clicked()
        {
            if let Err(error) = self.robot_stop_capture(true, true) {
                self.robot_setup.error = Some(error);
            }
        }
        let description_topics: Vec<_> = self
            .discovery
            .topics
            .iter()
            .filter(|(_, types)| {
                types
                    .iter()
                    .any(|ty| ty == studio_protocol::robot_description::MESSAGE_TYPE)
            })
            .map(|(topic, _)| topic.clone())
            .collect();
        let operation = self.robot_setup.capture.as_ref().map(|c| {
            if c.recovery {
                "Description cleanup needs recovery"
            } else if c.stop_request.is_some() {
                "Waiting for description reader cleanup…"
            } else if c.deadline.is_some() {
                "Starting description reader…"
            } else {
                "Waiting for a complete description…"
            }
        });
        let source_summary = self
            .robot_setup
            .captured_summary
            .as_ref()
            .map(|(bytes, hash)| format!("Description: {bytes} bytes · SHA-256 {hash}"));
        let busy = self
            .robot_setup
            .worker
            .as_ref()
            .is_some_and(AssetWorker::is_busy);
        let busy = busy || self.robot_setup.capture.is_some();
        let Some(draft) = self.robot_setup.draft.as_mut() else {
            self.setup = false;
            return;
        };
        let assets_match = self
            .robot_setup
            .reviewed
            .as_ref()
            .is_some_and(|r| r.assets == draft.assets && r.description == draft.description);
        draft.tf = self.scene_settings.configuration.clone();
        draft.clock_verified = self.scene_settings.verified;
        let action = robot::show_robot_setup_in_scene(
            ui,
            draft,
            RobotSetupState {
                busy,
                review: self.robot_setup.reviewed.as_ref().map(|r| &r.summary),
                assets_match,
                error: self.robot_setup.error.as_deref(),
                target_active,
                is_replacement: self.robot_setup.target.is_some(),
                connected: self.connection.phase == Phase::Connected,
                tf_topics: &tf_topics,
                description_topics: &description_topics,
                operation,
                source_summary: source_summary.as_deref(),
            },
        );
        if let Some(action) = action {
            let result = match action {
                RobotSetupAction::Load => self.robot_load(),
                RobotSetupAction::Apply => self.robot_apply(),
                RobotSetupAction::CancelLoad => {
                    self.robot_cancel_capture();
                    if let (Some(worker), Some(load)) =
                        (&mut self.robot_setup.worker, &self.robot_setup.load)
                    {
                        worker.cancel(load.id);
                    }
                    Ok(())
                }
                RobotSetupAction::Cancel => {
                    self.cancel_robot_setup();
                    self.setup = false;
                    self.setup_target = None;
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.robot_setup.error = Some(error);
            }
        }
    }
}
fn robot_configuration(draft: &RobotDraft, reviewed: &Reviewed, connected: bool) -> Configuration {
    Configuration {
        map_updates: None,
        topic: draft.tf.dynamic_tf_topic.clone(),
        message_type: spatial::TF_MESSAGE_TYPE.into(),
        best_effort: draft.tf.dynamic_tf_best_effort,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(Box::new(studio_protocol::robot::Configuration {
            fixed_frame: draft.tf.fixed_frame.clone(),
            frames: reviewed.loaded.prepared.link_names.clone(),
            clock_policy: if connected && draft.clock_verified {
                SpatialClockPolicy::VerifiedSystem
            } else {
                SpatialClockPolicy::Unknown
            },
            dynamic_tf_topic: draft.tf.dynamic_tf_topic.clone(),
            dynamic_tf_best_effort: draft.tf.dynamic_tf_best_effort,
            static_tf_topic: draft.tf.static_tf_topic.clone(),
        })),
    }
}
fn workspace_robot(
    draft: &RobotDraft,
    connected: bool,
) -> crate::components::workspace::ViewConfig {
    use crate::components::workspace as file;
    let dynamic_qos = file::Qos {
        reliability: if draft.tf.dynamic_tf_best_effort {
            file::Reliability::BestEffort
        } else {
            file::Reliability::Reliable
        },
        durability: file::Durability::Volatile,
    };
    let mut view = file::ViewConfig {
        id: "robot".into(),
        topic: draft.tf.dynamic_tf_topic.clone(),
        message_type: spatial::TF_MESSAGE_TYPE.into(),
        qos: dynamic_qos.clone(),
        kind: file::ViewKind::Robot {
            assets: draft.assets.clone(),
            fixed_frame: draft.tf.fixed_frame.clone(),
            clock_expectation: if connected && draft.clock_verified {
                file::ClockExpectation::System
            } else {
                file::ClockExpectation::Unknown
            },
            transforms: file::SpatialTransforms {
                dynamic_tf: file::TransformInput {
                    topic: draft.tf.dynamic_tf_topic.clone(),
                    message_type: spatial::TF_MESSAGE_TYPE.into(),
                    qos: dynamic_qos,
                },
                static_tf: file::TransformInput {
                    topic: draft.tf.static_tf_topic.clone(),
                    message_type: spatial::TF_MESSAGE_TYPE.into(),
                    qos: file::Qos {
                        reliability: file::Reliability::Reliable,
                        durability: file::Durability::TransientLocal,
                    },
                },
            },
        },
    };
    if let Some(description) = &draft.description {
        if let file::ViewKind::Robot {
            fixed_frame,
            transforms,
            clock_expectation,
            ..
        } = view.kind
        {
            view.kind = file::ViewKind::RobotTopic {
                description: description.clone(),
                fixed_frame,
                transforms,
                clock_expectation,
            };
        }
    }
    view
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use std::sync::mpsc::{Receiver, TryRecvError};
    use studio_protocol::robot_description::Document;
    fn ingest(app: &mut App, event: Event) {
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .ingest_for_test(event)
            .unwrap();
        app.poll_backend();
        app.robot_poll();
    }
    fn connected() -> (App, Receiver<Command>) {
        let (session, commands) = session::Session::fixture(0);
        let mut app = App::default();
        app.session = Some(session);
        app.connection.phase = Phase::Starting;
        ingest(
            &mut app,
            Event::ready("jazzy".into(), "fixture-no-ROS".into()),
        );
        ingest(
            &mut app,
            Event::Graph {
                topics: vec![
                    ("/description".into(), vec!["std_msgs/msg/String".into()]),
                    ("/camera".into(), vec![images::IMAGE_MESSAGE_TYPE.into()]),
                    ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                    ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                ],
            },
        );
        app.scene_settings.draft.fixed_frame = "world".into();
        app.apply_scene_settings().unwrap();
        (app, commands)
    }
    fn begin(app: &mut App, commands: &Receiver<Command>) -> StartIntent {
        app.begin_robot_setup(None);
        app.robot_setup
            .draft
            .as_mut()
            .unwrap()
            .description
            .as_mut()
            .unwrap()
            .topic = "/description".into();
        app.robot_load().unwrap();
        let capture = app.robot_setup.capture.as_ref().unwrap().owner.clone();
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Start { configuration, .. } if configuration.output == Output::RobotDescription)
        );
        capture
    }
    fn started(owner: &StartIntent) -> Event {
        Event::Started {
            generation: owner.generation,
            subscription: owner.subscription,
            request: owner.request,
        }
    }
    fn chunks(app: &mut App, owner: &StartIntent) {
        let document = Document::new("/description", r#"<robot name="captured"><link name="base"><visual><geometry><box size="1 2 3"/></geometry></visual></link></robot>"#.into()).unwrap();
        ingest(
            app,
            Event::RobotDescriptionObservation {
                generation: owner.generation,
                subscription: owner.subscription,
                sequence: 1,
                sample: Box::new(document.chunk(0).unwrap()),
                backend_drops: 0,
            },
        );
    }
    fn stopped(app: &mut App, command: Command) {
        let Command::Stop {
            generation,
            subscription,
            request,
        } = command
        else {
            panic!("Stop expected");
        };
        ingest(
            app,
            Event::Drops {
                generation,
                subscription,
                backend_drops: 0,
                spatial_counters: None,
            },
        );
        ingest(
            app,
            Event::Stopped {
                generation,
                subscription,
                request,
            },
        );
    }
    fn await_review(app: &mut App) {
        let until = Instant::now() + Duration::from_secs(3);
        while app.robot_setup.reviewed.is_none() {
            app.robot_poll();
            assert!(
                app.robot_setup.error.is_none(),
                "{:?}",
                app.robot_setup.error
            );
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    #[test]
    fn captured_robot_loads_only_after_stop_and_add_restore_stay_inactive() {
        let (mut app, commands) = connected();
        assert!(app.scene_settings.verified);
        let owner = begin(&mut app, &commands);
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        assert!(
            app.robot_setup.worker.is_none(),
            "Complete text must await native reader cleanup"
        );
        assert!(app.views.is_empty());
        let stop = commands.try_recv().unwrap();
        stopped(&mut app, stop);
        await_review(&mut app);
        assert_eq!(
            app.robot_setup.reviewed.as_ref().unwrap().summary.triangles,
            12
        );
        assert!(app.robot_setup.captured_summary.is_some());
        app.robot_apply().unwrap();
        assert_eq!(app.views.len(), 1);
        let view = &app.views[0];
        assert_eq!(view.source_label(), "/description");
        assert!(!view.enabled && view.subscription.is_none() && view.spatial_clock_verified);
        assert!(view.robot_description.is_some() && view.robot.is_some());
        let snapshot = app.workspace_snapshot().unwrap();
        assert_eq!(snapshot.views[0].source_label(), "/description");
        let bytes = studio_workspace::application::encode(
            &snapshot,
            studio_workspace::application::Application::Visualizer,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
            studio_workspace::application::VISUALIZER_VERSION
        );
        let prepared = app.workspace_prepare(&snapshot).unwrap();
        assert!(prepared.views[0].robot.is_none());
        assert!(prepared.views[0].robot_description.is_some());
        assert_eq!(prepared.views[0].source_label(), "/description");
        assert!(
            !prepared.views[0].enabled
                && prepared.views[0].subscription.is_none()
                && !prepared.views[0].spatial_clock_verified
        );
        let appearance = robot::RobotAppearance {
            collision: true,
            opacity_permille: 600,
            links: vec![robot::LinkAppearance {
                name: "base".into(),
                visible: false,
                opacity_permille: 400,
            }],
            ..Default::default()
        };
        app.views[0]
            .set_robot_appearance(appearance.clone())
            .unwrap();
        let snapshot = app.workspace_snapshot().unwrap();
        let encoded = studio_workspace::application::encode(
            &snapshot,
            studio_workspace::application::Application::Visualizer,
        )
        .unwrap();
        let parsed = studio_workspace::application::parse(
            &encoded,
            studio_workspace::application::Application::Visualizer,
        )
        .unwrap();
        let restored = app.workspace_prepare(&parsed.workspace).unwrap();
        assert_eq!(restored.views[0].robot_appearance, appearance);
        assert!(
            restored.views[0].robot.is_none()
                && !restored.views[0].enabled
                && restored.views[0].subscription.is_none()
                && !restored.views[0].spatial_clock_verified
        );
        let loaded = app.views[0].robot.as_ref().unwrap().loaded().clone();
        // Topic restores intentionally have no File-only RobotAssets. Review of
        // the same description source must still retain name-based appearance.
        app.views = restored.views;
        let restored_id = app.views[0].id;
        assert!(app.views[0].robot_assets.is_none());
        app.begin_robot_setup(Some(restored_id));
        let draft = app.robot_setup.draft.as_ref().unwrap();
        app.robot_setup.reviewed = Some(Reviewed::new(
            LoadId(777),
            draft.assets.clone(),
            draft.description.clone(),
            loaded,
        ));
        app.robot_apply().unwrap();
        assert_eq!(app.views[0].robot_appearance, appearance);
        let id = app.views[0].id;
        app.begin_robot_setup(Some(id));
        app.robot_apply().unwrap();
        assert_eq!(app.views[0].robot_appearance, appearance);
        app.begin_robot_setup(Some(id));
        app.robot_setup
            .draft
            .as_mut()
            .unwrap()
            .description
            .as_mut()
            .unwrap()
            .topic = "/different_description".into();
        app.robot_setup.reviewed.as_mut().unwrap().description =
            app.robot_setup.draft.as_ref().unwrap().description.clone();
        app.robot_apply().unwrap();
        assert!(app.views[0].robot_appearance.links.is_empty());
        assert_eq!(app.views[0].robot_appearance.opacity_permille, 600);
        assert!(app.views[0].robot_appearance.collision);
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        app.robot_setup.finish_shutdown().unwrap();
    }
    #[test]
    fn completed_review_survives_explicit_scene_navigation_but_pending_capture_does_not() {
        let (mut app, commands) = connected();
        let owner = begin(&mut app, &commands);
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        stopped(&mut app, commands.try_recv().unwrap());
        await_review(&mut app);
        let model = app.robot_setup.reviewed.as_ref().unwrap().id;
        let summary = app.robot_setup.captured_summary.clone();
        app.open_scene_settings();
        assert!(app.scene_settings.open && !app.setup);
        assert!(app.robot_review_can_resume());
        app.scene_settings.draft.fixed_frame = "different_world".into();
        app.apply_scene_settings().unwrap();
        app.compact_inspector_open = false;
        app.details_open = false;
        app.return_to_robot_review();
        assert!(app.setup && !app.scene_settings.open);
        assert!(app.compact_inspector_open && app.details_open);
        assert_eq!(app.robot_setup.reviewed.as_ref().unwrap().id, model);
        assert_eq!(app.robot_setup.captured_summary, summary);
        assert_eq!(
            app.robot_setup.draft.as_ref().unwrap().tf.fixed_frame,
            "different_world"
        );
        assert!(app.views.is_empty());
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        app.robot_load().unwrap();
        commands.try_recv().unwrap();
        app.open_scene_settings();
        assert!(!app.robot_review_can_resume());
        assert!(app.robot_setup.draft.is_none());
        assert!(matches!(commands.try_recv().unwrap(), Command::Stop { .. }));
        app.stop();
        app.robot_setup.finish_shutdown().unwrap();
    }
    #[test]
    fn cancel_start_then_late_ack_and_data_cannot_load_into_a_new_draft() {
        let (mut app, commands) = connected();
        let owner = begin(&mut app, &commands);
        app.cancel_robot_setup();
        let stop = commands.try_recv().unwrap();
        app.begin_robot_setup(None);
        app.robot_setup
            .draft
            .as_mut()
            .unwrap()
            .description
            .as_mut()
            .unwrap()
            .topic = "/description".into();
        assert!(
            app.robot_load().is_err(),
            "Old capture retains its registry slot through cleanup"
        );
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        assert!(app.robot_setup.worker.is_none() && app.robot_setup.reviewed.is_none());
        stopped(&mut app, stop);
        assert!(app.robot_setup.capture.is_none() && app.robot_setup.load.is_none());
        app.robot_load().unwrap();
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Start { .. }
        ));
        app.stop();
        assert!(app.robot_setup.capture.is_none());
    }
    #[test]
    fn workspace_replacement_adopts_inflight_capture_stop_and_waits_for_exact_cleanup() {
        use crate::components::workspace as file;
        let (mut app, commands) = connected();
        let owner = begin(&mut app, &commands);
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        let stop = commands.try_recv().unwrap();
        app.workspace_review_configuration(App::default().workspace_snapshot().unwrap())
            .unwrap();
        app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
            .unwrap();
        assert!(app.workspace_frozen());
        assert_eq!(
            commands.try_recv(),
            Err(TryRecvError::Empty),
            "Replacement adopts the existing Stop correlation"
        );
        app.workspace_poll(&egui::Context::default());
        assert!(app.workspace_frozen());
        stopped(&mut app, stop);
        app.workspace_poll(&egui::Context::default());
        assert!(!app.workspace_frozen());
        assert!(app.views.is_empty() && app.robot_setup.worker.is_none());
    }
    #[test]
    fn failed_stop_retains_capture_until_retry_matches_and_disconnect_revokes_pending_topic_load() {
        let (mut app, commands) = connected();
        let owner = begin(&mut app, &commands);
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        let first = commands.try_recv().unwrap();
        let Command::Stop {
            generation,
            subscription,
            request,
        } = first
        else {
            panic!("Stop expected");
        };
        ingest(
            &mut app,
            Event::RequestError {
                generation,
                subscription,
                request,
                no_handle: false,
                message: "injected uncertain cleanup".into(),
            },
        );
        assert!(app.robot_setup.capture.as_ref().unwrap().recovery);
        assert!(app.robot_setup.worker.is_none());
        assert!(app.robot_capture_stop_owner(generation, subscription, true));
        let retry = commands.try_recv().unwrap();
        ingest(
            &mut app,
            Event::Stopped {
                generation,
                subscription,
                request,
            },
        );
        assert!(
            app.robot_setup.capture.is_some(),
            "Old Stop cannot confirm the retry"
        );
        stopped(&mut app, retry);
        assert!(app.robot_setup.capture.is_none());
        assert!(
            app.robot_setup.load.is_none(),
            "Recovery cancellation discards the old candidate"
        );
        app.robot_setup.error = None;
        app.robot_load().unwrap();
        let owner = app.robot_setup.capture.as_ref().unwrap().owner.clone();
        commands.try_recv().unwrap();
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        stopped(&mut app, commands.try_recv().unwrap());
        app.stop();
        let until = Instant::now() + Duration::from_secs(3);
        while app.robot_setup.load.is_some() {
            app.robot_poll();
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(app.robot_setup.reviewed.is_none());
        assert_eq!(commands.try_recv(), Err(TryRecvError::Disconnected));
        app.robot_setup.finish_shutdown().unwrap();
    }
    #[test]
    fn drafting_topic_replacement_keeps_robot_and_image_then_apply_stops_only_robot() {
        let (mut app, commands) = connected();
        app.scene_settings.verified = true;
        app.scene_settings.configuration.clock_policy = SpatialClockPolicy::VerifiedSystem;
        let owner = begin(&mut app, &commands);
        ingest(&mut app, started(&owner));
        chunks(&mut app, &owner);
        stopped(&mut app, commands.try_recv().unwrap());
        await_review(&mut app);
        app.robot_apply().unwrap();
        let robot_id = app.selected.unwrap();
        app.set_display_enabled(robot_id, true);
        let Command::Start {
            generation,
            request,
            subscription,
            ..
        } = commands.try_recv().unwrap()
        else {
            panic!("Robot start");
        };
        ingest(
            &mut app,
            Event::Started {
                generation,
                request,
                subscription,
            },
        );
        let old_model = app.views[0].robot.as_ref().unwrap().model_id();
        app.begin_source(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        app.subscribe();
        let image = app.selected.unwrap();
        app.set_display_enabled(image, true);
        let Command::Start {
            generation: ig,
            request: ir,
            subscription: is,
            ..
        } = commands.try_recv().unwrap()
        else {
            panic!("Image start");
        };
        ingest(
            &mut app,
            Event::Started {
                generation: ig,
                request: ir,
                subscription: is,
            },
        );
        app.begin_robot_setup(Some(robot_id));
        app.robot_load().unwrap();
        commands.try_recv().unwrap();
        let capture = app.robot_setup.capture.as_ref().unwrap().owner.clone();
        ingest(&mut app, started(&capture));
        chunks(&mut app, &capture);
        assert_eq!(app.views[0].subscription, Some(subscription));
        assert_eq!(app.views[0].robot.as_ref().unwrap().model_id(), old_model);
        assert_eq!(app.views[1].subscription, Some(is));
        stopped(&mut app, commands.try_recv().unwrap());
        await_review(&mut app);
        app.robot_apply().unwrap();
        let stop = commands.try_recv().unwrap();
        assert!(matches!(stop, Command::Stop { subscription: s, .. } if s == subscription));
        assert_eq!(app.views[1].subscription, Some(is));
        stopped(&mut app, stop);
        assert!(app.views[0].subscription.is_none() && !app.views[0].enabled);
        assert_ne!(app.views[0].robot.as_ref().unwrap().model_id(), old_model);
        assert_eq!(app.views[1].subscription, Some(is));
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        app.robot_setup.finish_shutdown().unwrap();
    }
}
