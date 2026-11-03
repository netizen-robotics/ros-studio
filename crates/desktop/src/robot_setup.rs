//! Explicit asset draft -> review -> inactive install. No filesystem work on UI frames.
use super::*;
use std::{collections::BTreeMap, path::PathBuf};
use studio_desktop::{
    robot::{
        self, AssetWorker, LoadId, LoadOptions, LoadedRobot, RobotDraft, RobotLive, RobotReview,
        RobotSetupAction, RobotSetupState,
    },
    workspace::RobotAssets,
};
use studio_protocol::spatial_input::SpatialClockPolicy;

#[derive(Clone)]
struct Reviewed {
    id: LoadId,
    assets: RobotAssets,
    loaded: LoadedRobot,
    summary: RobotReview,
}
impl Reviewed {
    fn new(id: LoadId, assets: RobotAssets, loaded: LoadedRobot) -> Self {
        let summary = RobotReview::from_loaded(&loaded);
        Self {
            id,
            assets,
            loaded,
            summary,
        }
    }
}
struct PendingLoad {
    id: LoadId,
    generation: u64,
    assets: RobotAssets,
}
struct Install {
    generation: u64,
    target: Option<ViewId>,
    prior_model: Option<LoadId>,
    draft: RobotDraft,
    reviewed: Reviewed,
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
}
impl RobotSetup {
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
        // Keep the in-flight identity until its completion is consumed. The
        // worker stays Busy during cancellation; no replacement job is queued.
    }
    pub(super) fn disconnected(&mut self) {
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
    pub(super) fn cancel_robot_setup(&mut self) {
        self.robot_setup.cancel();
    }
    pub(super) fn begin_robot_setup(&mut self, target: Option<ViewId>) {
        if self.workspace_frozen() {
            return;
        }
        if target.is_none() && self.views.len() >= MAX_SUBSCRIPTIONS {
            return;
        }
        self.cancel_robot_setup();
        let mut draft = RobotDraft::default();
        if target.is_none() {
            if let Some(key) = self
                .views
                .iter()
                .filter(|v| v.enabled)
                .find_map(|v| v.configuration.scene_key())
            {
                draft.tf = studio_protocol::spatial::Configuration {
                    fixed_frame: key.fixed_frame,
                    dynamic_tf_topic: key.dynamic_tf_topic,
                    dynamic_tf_best_effort: key.dynamic_tf_best_effort,
                    static_tf_topic: key.static_tf_topic,
                    clock_policy: SpatialClockPolicy::Unknown,
                };
            }
        }
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
                    live.loaded().clone(),
                ));
            }
            self.selected = Some(id);
        }
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
        });
        // A new accepted attempt revokes the previous candidate even when the
        // paths are identical. The installed active model remains untouched.
        self.robot_setup.reviewed = None;
        self.robot_setup.error = None;
        self.robot_setup.draft.as_mut().unwrap().clock_verified = false;
        Ok(())
    }
    fn robot_apply(&mut self) -> Result<(), String> {
        if self.workspace_frozen() || self.robot_setup.install.is_some() {
            return Err("Wait for the current replacement to finish".into());
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
        if draft.assets != reviewed.assets {
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
        let document = studio_desktop::workspace::Workspace {
            tools: None,
            presentation: None,
            version: studio_desktop::workspace::FORMAT_VERSION,
            name: "Robot setup".into(),
            panels: studio_desktop::workspace::Panels {
                data_open: true,
                inspector_open: true,
                diagnostics_open: false,
            },
            layout: studio_desktop::workspace::Layout::for_views(&views),
            views,
            connection_suggestion: None,
        };
        document.encode().map_err(|e| e.to_string())?;
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
                return Err("Eight displays are already configured".into());
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
        let completion = self.robot_setup.worker.as_mut().and_then(AssetWorker::poll);
        if let Some(completion) = completion {
            if let Some(load) = self.robot_setup.load.take() {
                if load.id == completion.id
                    && load.generation == self.robot_setup.generation
                    && self.robot_setup.draft.is_some()
                {
                    match completion.result {
                        Ok(loaded) => {
                            self.robot_setup.reviewed =
                                Some(Reviewed::new(completion.id, load.assets, loaded))
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
        let live = RobotLive::new(install.reviewed.id, install.reviewed.loaded)
            .map_err(|e| e.to_string())?;
        let id = if let Some(id) = install.target {
            id
        } else {
            if self.views.len() >= MAX_SUBSCRIPTIONS {
                return Err("Eight displays are already configured".into());
            }
            self.next_view = self
                .next_view
                .checked_add(1)
                .ok_or("View identities exhausted")?;
            ViewId(self.next_view)
        };
        let mut replacement = ViewState::new(id, configuration);
        replacement.robot_assets = Some(install.draft.assets);
        replacement.robot = Some(live);
        replacement.spatial_clock_verified = connected && install.draft.clock_verified;
        replacement.workspace_clock_expectation = if replacement.spatial_clock_verified {
            studio_desktop::workspace::ClockExpectation::System
        } else {
            studio_desktop::workspace::ClockExpectation::Unknown
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
        let busy = self
            .robot_setup
            .worker
            .as_ref()
            .is_some_and(AssetWorker::is_busy);
        let Some(draft) = self.robot_setup.draft.as_mut() else {
            self.setup = false;
            return;
        };
        let assets_match = self
            .robot_setup
            .reviewed
            .as_ref()
            .is_some_and(|r| r.assets == draft.assets);
        let action = robot::show_robot_setup(
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
                description_topics: &[],
                operation: None,
                source_summary: None,
            },
        );
        if let Some(action) = action {
            let result = match action {
                RobotSetupAction::Load => self.robot_load(),
                RobotSetupAction::Apply => self.robot_apply(),
                RobotSetupAction::CancelLoad => {
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
fn workspace_robot(draft: &RobotDraft, connected: bool) -> studio_desktop::workspace::ViewConfig {
    use studio_desktop::workspace as file;
    let dynamic_qos = file::Qos {
        reliability: if draft.tf.dynamic_tf_best_effort {
            file::Reliability::BestEffort
        } else {
            file::Reliability::Reliable
        },
        durability: file::Durability::Volatile,
    };
    file::ViewConfig {
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
    }
}
#[cfg(test)]
#[path = "robot_setup/tests.rs"]
mod tests;
