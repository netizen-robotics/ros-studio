//! One explicit scene configuration owner. Apply waits for exact Stop cleanup,
//! changes only scene displays, and resumes only the enabled intent captured by Apply.
use super::*;
use studio_protocol::{
    spatial::Configuration as SceneConfiguration, spatial_input::SpatialClockPolicy,
};

pub(super) struct SceneSettings {
    pub configuration: SceneConfiguration,
    pub draft: SceneConfiguration,
    /// Legacy internal name: selected System-time intent, not publisher verification.
    pub verified: bool,
    pub draft_verified: bool,
    pub open: bool,
    pub pending: Option<(SceneConfiguration, bool)>,
    pub resume: Vec<ViewId>,
    pub resume_generation: u64,
    pub error: Option<String>,
    pub feedback: Option<String>,
    pub focus_requested: bool,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            configuration: empty_spatial_configuration(),
            draft: empty_spatial_configuration(),
            verified: true,
            draft_verified: true,
            open: false,
            pending: None,
            resume: Vec::new(),
            resume_generation: 0,
            error: None,
            feedback: None,
            focus_requested: false,
        }
    }
}
impl SceneSettings {
    pub fn disconnect(&mut self) {
        self.resume.clear();
        if self.pending.take().is_some() {
            self.feedback =
                Some("Disconnected. Application cancelled; displays remain stopped.".into());
        }
        self.verified = false;
        self.draft_verified = false;
        self.configuration.clock_policy = SpatialClockPolicy::Unknown;
        self.draft.clock_policy = SpatialClockPolicy::Unknown;
    }
}
impl App {
    pub(super) fn retain_scene_frames(&mut self, frames: Vec<String>) {
        self.recent_tf_frames = frames.clone();
        let mut retained = frames;
        for frame in &self.observed_tf_frames {
            if retained.len() >= studio_protocol::spatial::MAX_FRAMES {
                break;
            }
            if !retained.contains(frame) {
                retained.push(frame.clone());
            }
        }
        retained.sort();
        retained.dedup();
        retained.truncate(studio_protocol::spatial::MAX_FRAMES);
        self.observed_tf_frames = retained;
    }
    /// System is application intent by default. No remote parameter write or
    /// publisher-verification claim is made. Restored views remain inactive.
    pub(super) fn use_system_scene_defaults(&mut self) {
        self.scene_settings.verified = true;
        self.scene_settings.draft_verified = true;
        self.scene_settings.configuration.clock_policy = SpatialClockPolicy::VerifiedSystem;
        self.scene_settings.draft.clock_policy = SpatialClockPolicy::VerifiedSystem;
        self.draft_clock_verified = true;
        for view in &mut self.views {
            if let Some(scene) = &mut view.configuration.spatial {
                if scene.clock_policy != SpatialClockPolicy::Simulated {
                    scene.clock_policy = SpatialClockPolicy::VerifiedSystem;
                    view.spatial_clock_verified = true;
                }
            }
            if let Some(robot) = &mut view.configuration.robot {
                if robot.clock_policy != SpatialClockPolicy::Simulated {
                    robot.clock_policy = SpatialClockPolicy::VerifiedSystem;
                    view.spatial_clock_verified = true;
                }
            }
        }
    }
    pub(super) fn scene_matches(&self, configuration: &Configuration) -> bool {
        configuration.scene_key().is_none_or(|key| {
            let scene = &self.scene_settings.configuration;
            key.fixed_frame == scene.fixed_frame
                && key.dynamic_tf_topic == scene.dynamic_tf_topic
                && key.static_tf_topic == scene.static_tf_topic
                && key.dynamic_tf_best_effort == scene.dynamic_tf_best_effort
        })
    }
    pub(super) fn open_scene_settings(&mut self) {
        if self.workspace_frozen() {
            return;
        }
        if self.robot_setup.is_open() {
            self.setup = false;
        }
        if !self.robot_review_can_resume() {
            self.cancel_robot_setup();
        }
        self.catalog_open = false;
        self.sidebar = true;
        self.topics_tab = false;
        self.scene_settings.open = true;
        self.global_options_open = true;
        self.scene_settings.focus_requested = true;
    }
    pub(super) fn restore_scene_settings(&mut self) {
        self.markers.revoke_assets();
        self.scene_settings = SceneSettings::default();
        for view in self.views.iter_mut().filter(|v| v.configuration.owns_tf()) {
            view.spatial_clock_verified = false;
            if let Some(s) = &mut view.configuration.spatial {
                s.clock_policy = SpatialClockPolicy::Unknown;
            }
            if let Some(r) = &mut view.configuration.robot {
                r.clock_policy = SpatialClockPolicy::Unknown;
            }
        }
        if let Some(key) = self.views.iter().find_map(|v| v.configuration.scene_key()) {
            self.scene_settings.configuration = SceneConfiguration {
                fixed_frame: key.fixed_frame,
                dynamic_tf_topic: key.dynamic_tf_topic,
                dynamic_tf_best_effort: key.dynamic_tf_best_effort,
                static_tf_topic: key.static_tf_topic,
                clock_policy: SpatialClockPolicy::Unknown,
            };
            self.scene_settings.draft = self.scene_settings.configuration.clone();
        }
        if self.connection.phase == Phase::Connected {
            self.use_system_scene_defaults();
        }
    }
    pub(super) fn apply_scene_settings(&mut self) -> Result<(), String> {
        if self.workspace_frozen() {
            return Err("Wait for the current replacement to finish".into());
        }
        let mut candidate = self.scene_settings.draft.clone();
        // Validate structure independently of runtime clock authorization.
        candidate.clock_policy = SpatialClockPolicy::VerifiedSystem;
        let probe = Configuration {
            map_updates: None,
            topic: "/scene_validation".into(),
            message_type: spatial::SCAN_MESSAGE_TYPE.into(),
            output: Output::SpatialScan,
            spatial: Some(candidate.clone()),
            robot: None,
            best_effort: false,
            transient_local: false,
        };
        if !probe.valid() {
            return Err("Choose a valid fixed frame and two distinct absolute TF topics".into());
        }
        if candidate == self.scene_settings.configuration
            && self.scene_settings.verified
            && self
                .views
                .iter()
                .all(|view| self.scene_matches(&view.configuration))
        {
            return Ok(());
        }
        if self
            .views
            .iter()
            .any(|v| v.configuration.owns_tf() && v.status == ViewStatus::NeedsRecovery)
        {
            return Err(
                "Scene cleanup is unconfirmed. Retry stop or disconnect before applying.".into(),
            );
        }
        let verified = self.connection.phase == Phase::Connected;
        candidate.clock_policy = if verified {
            SpatialClockPolicy::VerifiedSystem
        } else {
            SpatialClockPolicy::Unknown
        };
        if self.robot_setup.is_open() {
            self.setup = false;
        }
        if !self.robot_review_can_resume() {
            self.cancel_robot_setup();
        }
        self.topic_workflow.replacement = None;
        self.scene_settings.resume = self
            .views
            .iter()
            .filter(|v| v.configuration.owns_tf() && v.enabled)
            .map(|v| v.id)
            .collect();
        self.scene_settings.resume_generation = self.session_generation;
        let ids: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.configuration.owns_tf())
            .map(|v| v.id)
            .collect();
        for id in ids {
            let view = self.views.iter_mut().find(|v| v.id == id).unwrap();
            view.pending = None;
            view.enabled = false;
            // Adopt an existing correlated Stop instead of replacing its
            // request identity while its cleanup acknowledgment is in flight.
            if view.status != ViewStatus::Stopping {
                self.stop_view(id);
            }
        }
        // Install this barrier after issuing Stops, whose action guards reject
        // mutations while a scene replacement is pending.
        self.scene_settings.pending = Some((candidate, verified));
        self.scene_settings.error = None;
        self.scene_settings.feedback = None;
        self.poll_scene_settings();
        Ok(())
    }
    pub(super) fn poll_scene_settings(&mut self) {
        if self.scene_settings.pending.is_none() {
            return;
        }
        if self
            .views
            .iter()
            .any(|v| v.configuration.owns_tf() && v.status == ViewStatus::NeedsRecovery)
        {
            self.scene_settings.pending = None;
            self.scene_settings.resume.clear();
            self.scene_settings.error = Some(
                "Scene replacement cancelled: cleanup unconfirmed. Retry cleanup or disconnect."
                    .into(),
            );
            return;
        }
        if self
            .views
            .iter()
            .any(|v| v.configuration.owns_tf() && v.subscription.is_some())
        {
            return;
        }
        let (configuration, verified) = self.scene_settings.pending.take().unwrap();
        let old = &self.scene_settings.configuration;
        if old.dynamic_tf_topic != configuration.dynamic_tf_topic
            || old.static_tf_topic != configuration.static_tf_topic
            || old.dynamic_tf_best_effort != configuration.dynamic_tf_best_effort
        {
            self.observed_tf_frames.clear();
            self.recent_tf_frames.clear();
            if let Some(session) = &self.session {
                session.shared.lock().unwrap().scene_frames = None;
            }
        }
        let resume = std::mem::take(&mut self.scene_settings.resume);
        for view in self.views.iter_mut().filter(|v| v.configuration.owns_tf()) {
            if let Some(s) = &mut view.configuration.spatial {
                *s = configuration.clone();
                if view.configuration.output == Output::TfGraph {
                    view.configuration.topic = configuration.dynamic_tf_topic.clone();
                    view.configuration.best_effort = configuration.dynamic_tf_best_effort;
                    view.configuration.transient_local = false;
                }
            }
            if let Some(r) = &mut view.configuration.robot {
                r.fixed_frame = configuration.fixed_frame.clone();
                r.dynamic_tf_topic = configuration.dynamic_tf_topic.clone();
                r.dynamic_tf_best_effort = configuration.dynamic_tf_best_effort;
                r.static_tf_topic = configuration.static_tf_topic.clone();
                r.clock_policy = configuration.clock_policy;
                view.configuration.topic = r.dynamic_tf_topic.clone();
                view.configuration.best_effort = r.dynamic_tf_best_effort;
            }
            view.spatial_clock_verified = verified;
            view.workspace_clock_expectation = if verified {
                crate::components::workspace::ClockExpectation::System
            } else {
                crate::components::workspace::ClockExpectation::Unknown
            };
            view.error = None;
        }
        self.scene_settings.configuration = configuration.clone();
        self.scene_settings.draft = configuration;
        self.scene_settings.verified = verified;
        self.scene_settings.open = true;
        self.global_options_open = true;
        self.scene_settings.feedback = None;
        if self.connection.phase == Phase::Connected
            && self.scene_settings.resume_generation == self.session_generation
        {
            for id in resume {
                self.start_view(id);
            }
        }
    }
    pub(super) fn scene_draft_changed(&self) -> bool {
        let a = &self.scene_settings.draft;
        let b = &self.scene_settings.configuration;
        a.fixed_frame != b.fixed_frame
            || a.dynamic_tf_topic != b.dynamic_tf_topic
            || a.static_tf_topic != b.static_tf_topic
            || a.dynamic_tf_best_effort != b.dynamic_tf_best_effort
    }
    pub(super) fn discard_scene_draft(&mut self) {
        self.scene_settings.draft = self.scene_settings.configuration.clone();
        self.scene_settings.draft_verified = self.scene_settings.verified;
        self.scene_settings.error = None;
        self.scene_settings.feedback = None;
    }
    pub(super) fn cancel_scene_application(&mut self) {
        self.scene_settings.pending = None;
        self.scene_settings.resume.clear();
        self.scene_settings.feedback =
            Some("Cancelled. Issued Stops are not undone; displays remain stopped.".into());
    }
    pub(super) fn scene_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        if self.scene_settings.pending.is_some() {
            ui.small("Waiting for confirmed scene cleanup. Images continue receiving.");
            if ui.button("Cancel scene replacement").clicked() {
                self.cancel_scene_application();
            }
            return;
        }
        studio_ui_foundation::theme::strong(ui, "Scene settings");
        ui.small("System time");
        egui::ScrollArea::vertical()
            .id_salt("scene-form")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.workspace_frozen(), |ui| self.scene_form_ui(ui));
                if let Some(error) = &self.scene_settings.error {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error)
                                .small()
                                .color(ui.visuals().error_fg_color),
                        )
                        .wrap(),
                    );
                }
                if let Some(feedback) = &self.scene_settings.feedback {
                    ui.add(egui::Label::new(egui::RichText::new(feedback).small()).wrap());
                }
                let changed = self.scene_draft_changed();
                if changed {
                    let affected = self
                        .views
                        .iter()
                        .filter(|v| v.configuration.owns_tf())
                        .count();
                    let active = self
                        .views
                        .iter()
                        .filter(|v| v.configuration.owns_tf() && v.subscription.is_some())
                        .count();
                    ui.small(format!("Updates {affected} displays; {active} active displays resume after cleanup. Images continue."));
                    ui.horizontal_wrapped(|ui| {
                        let apply = ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Apply"));
                        apply.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                apply.enabled(),
                                "Apply scene settings",
                            )
                        });
                        if apply.clicked() {
                            if let Err(error) = self.apply_scene_settings() {
                                self.scene_settings.error = Some(error);
                                self.scene_settings.focus_requested = true;
                            }
                        }
                        let discard =
                            ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Discard"));
                        discard.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                discard.enabled(),
                                "Discard scene draft",
                            )
                        });
                        if discard.clicked() {
                            self.discard_scene_draft();
                        }
                    });
                }
                if self.robot_review_can_resume() && ui.button("Return to Robot review").clicked() {
                    self.return_to_robot_review();
                }
                crate::ui::reveal_focused_control(ui);
            });
    }
    fn scene_form_ui(&mut self, ui: &mut egui::Ui) {
        let selected = self.scene_settings.draft.fixed_frame.clone();
        let caption = ui.label("Fixed frame");
        let response = egui::ComboBox::from_id_salt("scene-fixed-frame")
            .width(180.0)
            .height(240.0)
            .truncate()
            .selected_text(&selected)
            .show_ui(ui, |ui| {
                ui.set_max_width(280.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.selectable_value(
                    &mut self.scene_settings.draft.fixed_frame,
                    selected.clone(),
                    format!("{selected} (selected)"),
                );
                if selected != "map" {
                    ui.selectable_value(
                        &mut self.scene_settings.draft.fixed_frame,
                        "map".into(),
                        "map (default)",
                    );
                }
                for frame in &self.observed_tf_frames {
                    if frame != &selected && frame != "map" {
                        ui.selectable_value(
                            &mut self.scene_settings.draft.fixed_frame,
                            frame.clone(),
                            if self.recent_tf_frames.contains(frame) {
                                frame.clone()
                            } else {
                                format!("{frame} (previously observed)")
                            },
                        )
                        .on_hover_text(frame);
                    }
                }
            })
            .response
            .labelled_by(caption.id)
            .on_hover_text(&selected);
        crate::ui::describe_choice(&response, "Fixed frame", &selected);
        if self.scene_settings.focus_requested {
            response.request_focus();
            response.scroll_to_me_animation(
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
            self.scene_settings.focus_requested = false;
        }
        if self.observed_tf_frames.is_empty() {
            ui.small(
                "Frame choices appear when a scene topic receives TF. You can start with map.",
            );
        } else if !self.observed_tf_frames.contains(&selected) {
            ui.small(format!("{selected} has not been observed in TF. Receiving can continue while waiting for this frame."));
        }
        ui.collapsing("Advanced transform inputs", |ui| {
            for (label, value) in [
                (
                    "Dynamic TF topic",
                    &mut self.scene_settings.draft.dynamic_tf_topic,
                ),
                (
                    "Static TF topic",
                    &mut self.scene_settings.draft.static_tf_topic,
                ),
            ] {
                let caption = ui.label(label);
                ui.add(studio_ui_foundation::theme::input(ui, value).char_limit(256))
                    .labelled_by(caption.id);
            }
            ui.checkbox(
                &mut self.scene_settings.draft.dynamic_tf_best_effort,
                "Dynamic TF best effort",
            );
            ui.small("Static TF requests Reliable + Transient local.");
            ui.small("System time only. Simulation is not supported by this build.");
        });
    }
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use crate::display_tests::{connected, ingest, started, stopped};

    #[test]
    fn frame_inventory_survives_empty_and_changed_snapshots_but_resets_with_inputs() {
        let (mut app, commands) = connected();
        app.retain_scene_frames(vec!["camera".into(), "map".into()]);
        app.retain_scene_frames(vec![]);
        assert_eq!(app.observed_tf_frames, vec!["camera", "map"]);
        assert!(app.recent_tf_frames.is_empty());
        app.retain_scene_frames(vec!["new_camera".into(), "odom".into()]);
        assert!(app.observed_tf_frames.contains(&"camera".into()));
        assert!(app.recent_tf_frames.contains(&"new_camera".into()));
        app.scene_settings.draft.fixed_frame = "odom".into();
        app.apply_scene_settings().unwrap();
        assert!(app.observed_tf_frames.contains(&"camera".into()));
        app.scene_settings.draft.static_tf_topic = "/other_static".into();
        app.apply_scene_settings().unwrap();
        assert!(app.observed_tf_frames.is_empty());
        assert!(app.recent_tf_frames.is_empty());
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn frame_inventory_is_bounded_and_new_session_revokes_previous_names() {
        let (mut app, _) = connected();
        app.retain_scene_frames((0..256).map(|i| format!("old_{i}")).collect());
        app.retain_scene_frames((0..256).map(|i| format!("new_{i}")).collect());
        assert_eq!(app.observed_tf_frames.len(), 256);
        assert!(app.observed_tf_frames.iter().all(|f| f.starts_with("new_")));
        app.stop();
        assert!(app.observed_tf_frames.is_empty());
    }

    #[test]
    fn scene_apply_resumes_only_enabled_scene_displays_after_the_barrier() {
        let (mut app, commands) = connected();
        app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        app.discovery.topics.push((
            "/other_scan".into(),
            vec![spatial::SCAN_MESSAGE_TYPE.into()],
        ));
        app.begin_source(
            Output::SpatialScan,
            "/other_scan",
            spatial::SCAN_MESSAGE_TYPE,
        );
        app.subscribe();
        let inactive = app.views.last().unwrap().id;
        app.scene_settings.draft.fixed_frame = "odom".into();
        app.apply_scene_settings().unwrap();
        let stop = commands.try_recv().unwrap();
        assert!(commands.try_recv().is_err());
        ingest(&mut app, stopped(&stop));
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Start { .. }
        ));
        assert!(commands.try_recv().is_err());
        assert!(!app.views.iter().find(|v| v.id == inactive).unwrap().enabled);
    }
}
