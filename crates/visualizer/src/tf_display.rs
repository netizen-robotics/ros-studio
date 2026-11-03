//! App lifecycle adapter; shared TF presentation performs no session actions.
use super::*;
impl App {
    pub(super) fn apply_tf_graph(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        sample: studio_protocol::tf_graph::Sample,
        backend_drops: u64,
    ) {
        let active = self.connection.phase == Phase::Connected
            && self.session.as_ref().is_some_and(|s| {
                s.shared.lock().unwrap().registry.entries().iter().any(|e| {
                    e.generation == generation
                        && e.subscription == subscription
                        && e.state == State::Active
                        && e.configuration.output == Output::TfGraph
                })
            });
        if !active {
            return;
        }
        let Some(view) = self.views.iter_mut().find(|v| {
            v.enabled
                && v.generation == generation
                && v.subscription == Some(subscription)
                && v.configuration.output == Output::TfGraph
                && (v.status.active() || v.status == ViewStatus::Starting)
                && sequence > v.spatial_sequence
        }) else {
            return;
        };
        let id = view.id;
        if let Err(error) = view.tf_graph.apply(sample) {
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.error = Some(error);
            }
            return;
        }
        view.spatial_sequence = sequence;
        view.deadline = None;
        let ready = view.tf_graph.ready_count();
        let count = view.tf_graph.sample().map_or(0, |s| s.frames.len());
        view.status = if ready > 0 {
            ViewStatus::Receiving
        } else {
            ViewStatus::Waiting
        };
        view.spatial_status = format!("{ready}/{count} TF frames drawable · Latest available");
        view.received.typed_samples = view.received.typed_samples.saturating_add(1);
        view.received.backend_drops = view.received.backend_drops.max(backend_drops);
    }
    pub(super) fn tf_setup_ui(&mut self, ui: &mut egui::Ui, active: bool) {
        studio_ui_foundation::theme::strong(ui, "TF frames");
        ui.label("This display uses the shared scene's transform readers. Adding it creates no new ROS readers.");
        let scene = &self.scene_settings.configuration;
        ui.label(format!("Fixed frame: {}", scene.fixed_frame));
        ui.label(format!("Dynamic TF: {}", scene.dynamic_tf_topic));
        ui.label(format!("Static TF: {}", scene.static_tf_topic));
        ui.small("Static: Reliable + Transient local. Change TF topics and dynamic QoS in Scene settings.");
        if !self.scene_settings.verified {
            ui.label("Confirm System time in Scene settings before enabling. A disabled display can be saved offline.");
        }
        if ui.button("Review Scene settings").clicked() {
            self.open_scene_settings();
            return;
        }
        let existing = self
            .views
            .iter()
            .any(|v| v.configuration.output == Output::TfGraph && Some(v.id) != self.setup_target);
        if existing {
            ui.label("A TF display is already configured. Select it in Displays.");
        }
        let mut probe = Configuration {
            map_updates: None,
            topic: scene.dynamic_tf_topic.clone(),
            message_type: studio_protocol::spatial_input::TF_MESSAGE_TYPE.into(),
            best_effort: scene.dynamic_tf_best_effort,
            transient_local: false,
            output: Output::TfGraph,
            spatial: Some(scene.clone()),
            robot: None,
        };
        probe.spatial.as_mut().unwrap().clock_policy =
            studio_protocol::spatial_input::SpatialClockPolicy::VerifiedSystem;
        let valid = probe.valid();
        if !valid {
            ui.label(
                "Configure a valid fixed frame and distinct TF topics in Scene settings first.",
            );
        }
        if active {
            ui.small("Apply stops this display and leaves it disabled. Re-enable explicitly after cleanup.");
        }
        if ui
            .add_enabled(
                valid
                    && !existing
                    && (self.setup_target.is_some() || self.views.len() < MAX_SUBSCRIPTIONS),
                shell::primary(
                    ui.visuals(),
                    if self.setup_target.is_some() {
                        "Apply display changes"
                    } else {
                        "Add disabled display"
                    },
                ),
            )
            .clicked()
        {
            self.draft_spatial = scene.clone();
            self.draft_clock_verified = self.scene_settings.verified;
            self.discovery.topic = scene.dynamic_tf_topic.clone();
            self.discovery.ty = studio_protocol::spatial_input::TF_MESSAGE_TYPE.into();
            self.draft_best_effort = scene.dynamic_tf_best_effort;
            self.draft_transient_local = false;
            self.subscribe();
        }
        if ui.button("Discard display draft").clicked() {
            self.setup = false;
            self.setup_target = None;
        }
    }
}
