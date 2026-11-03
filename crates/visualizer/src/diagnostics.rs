//! Shared safe report UI; raw local errors/drafts never enter the copy envelope.
use super::*;
use serde_json::json;
impl App {
    fn diagnostic_state(&self) -> serde_json::Value {
        let (readers, available, rejected) = self
            .session
            .as_ref()
            .map(|s| {
                let shared = s.shared.lock().unwrap();
                (
                    shared.registry.entries().len(),
                    shared.registry.available_resources(),
                    shared.ingress_rejections,
                )
            })
            .unwrap_or_default();
        json!({
            "connection":{"phase":format!("{:?}",self.connection.phase),"domain":self.connection.domain_id().ok(),
                "requested_middleware":if self.connection.cyclone{"Cyclone DDS"}else{"Fast DDS"},
                "actual":self.connection.actual},
            "current_issues":{"count":self.current_attention().len(),"workspace_error":self.workspace_error().is_some(),
                "topic_action_error":self.topic_action_error.is_some(),"detail_policy":"Raw errors may contain paths or payloads; inspect affected controls locally"},
            "recorded_history":self.diagnostics.iter().map(|e|studio_ui_foundation::diagnostics::error_category(e)).collect::<Vec<_>>(),
            "counters":{"readers":readers,"available_native_resources":available,"rejected_events":rejected,
                "ui_drops":self.views.iter().fold(0u64,|sum,v|sum.saturating_add(v.queue_drops)),
                "backend_drops":self.views.iter().fold(0u64,|sum,v|sum.saturating_add(v.received.backend_drops)),
                "note":"Application counters do not measure DDS/network loss"},
            "ros_health":self.health.summary(),
            "app_specific":{"workspace_version":19,"scene":{"fixed_frame":self.scene_settings.configuration.fixed_frame,
                "replacement_pending":self.scene_settings.pending.is_some()},
                "source_draft":{"open":self.setup,"target":self.setup_target.map(|id|id.0),"selection_available":self.discovery.valid_selection()},
                "discovered_topics":self.discovery.topics.len(),"selected_display":self.selected.map(|id|id.0),
                "displays":self.views.iter().map(|v|json!({"id":v.id.0,"kind":v.kind.label(),"topic":v.configuration.topic,
                    "type":v.configuration.message_type,"best_effort":v.configuration.best_effort,"transient_local":v.configuration.transient_local,
                    "status":v.status_label(),"enabled":v.enabled,"subscription":v.subscription.map(|s|s.0),
                    "has_error":v.error.is_some()||v.sensor_appearance_error.is_some()||v.robot_render_failure.is_some(),
                    "error_category":v.error.as_deref().map(studio_ui_foundation::diagnostics::error_category),
                    "samples":v.received.typed_samples,"ui_drops":v.queue_drops,"backend_drops":v.received.backend_drops})).collect::<Vec<_>>()}
        })
    }
    pub(super) fn diagnostics_window_ui(&mut self, ctx: &egui::Context) {
        self.diagnostic_panel.open = self.diagnostics_open;
        let state = self
            .diagnostic_panel
            .wants_capture()
            .then(|| self.diagnostic_state());
        let enabled = self.connection.phase == Phase::Connected && !self.workspace_frozen();
        if self
            .diagnostic_panel
            .finish_frame(ctx, "Visualizer", state, |ui| {
                self.health.ui(
                    &mut self.health_view,
                    ui,
                    &self.discovery.topics,
                    self.session.as_ref(),
                    enabled,
                )
            })
        {
            self.diagnostics.clear();
        }
        self.diagnostics_open = self.diagnostic_panel.open;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_report_excludes_drafts_paths_and_payloads_and_clear_preserves_causes() {
        let (mut app, commands) = crate::display_tests::connected();
        app.diagnostic("Backend error /Users/private/secret.txt PRIVATE_PAYLOAD".into());
        app.topic_action_error = Some("PRIVATE_DRAFT /private/tmp/secret".into());
        app.discovery.topic = "PRIVATE_DRAFT".into();
        let ctx = egui::Context::default();
        let before = studio_ui_foundation::diagnostics::report_for(
            &ctx,
            "Visualizer",
            app.diagnostic_state(),
        );
        for secret in [
            "PRIVATE_PAYLOAD",
            "PRIVATE_DRAFT",
            "/Users/private",
            "/private/tmp",
        ] {
            assert!(!before.contains(secret));
        }
        app.diagnostics.clear();
        let after = app.diagnostic_state();
        assert_eq!(after["recorded_history"], json!([]));
        assert_eq!(after["current_issues"]["topic_action_error"], true);
        assert!(commands.try_recv().is_err());
    }
}

#[cfg(test)]
mod ros_tests {
    use super::*;
    use studio_client_core::{
        diagnostic_acceptance::{self, Host},
        diagnostic_health::Monitor,
    };
    impl Host for App {
        fn poll(&mut self) {
            self.poll_backend();
            self.check_deadlines(Instant::now());
        }
        fn monitor(&self) -> &Monitor {
            &self.health
        }
        fn topics(&self) -> &[(String, Vec<String>)] {
            &self.discovery.topics
        }
        fn start(&mut self, topic: &str, best: bool, transient: bool) {
            self.health.topic = topic.into();
            self.health.best_effort = best;
            self.health.transient_local = transient;
            self.health
                .start(self.session.as_ref().unwrap(), &self.discovery.topics);
        }
        fn stop(&mut self) {
            self.health.stop(self.session.as_ref().unwrap());
        }
        fn disconnect(&mut self) {
            App::stop(self);
        }
        fn connect(&mut self) {
            App::connect(self);
        }
        fn connected(&self) -> bool {
            self.connection.phase == Phase::Connected
        }
    }
    #[test]
    #[ignore = "Requires isolated local ROS diagnostic fixture on domain187"]
    fn health_real_ros_visualizer() {
        diagnostic_acceptance::guard();
        let _finish = diagnostic_acceptance::Finish;
        let mut app = App::default();
        app.connection.domain = "187".into();
        app.connection.cyclone =
            std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
        Host::connect(&mut app);
        diagnostic_acceptance::run(&mut app, "visualizer");
    }
}
