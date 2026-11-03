//! Topic selection is passive; visibility and mode changes are explicit intents.
use super::*;
use studio_protocol::tools::Operation;

#[derive(Default)]
pub(super) struct TopicWorkflow {
    pub replacement: Option<(Output, String, String, bool)>,
    pub refresh: Option<(Generation, RequestId, Instant)>,
    next_refresh: u64,
}

impl App {
    pub(super) fn inspect_topic(&mut self, output: Output, topic: &str, ty: &str) {
        if let Some(id) = self
            .views
            .iter()
            .find(|v| v.source_label() == topic && v.enabled)
            .or_else(|| {
                self.views
                    .iter()
                    .find(|v| Some(v.id) == self.selected && v.source_label() == topic)
            })
            .or_else(|| self.views.iter().find(|v| v.source_label() == topic))
            .map(|v| v.id)
        {
            self.select_display(id);
        } else if output == Output::Robot {
            self.begin_robot_topic(topic);
        } else {
            self.begin_source(output, topic, ty);
        }
    }

    pub(super) fn toggle_topic(&mut self, output: Output, topic: &str, ty: &str) {
        if self.workspace_frozen() {
            return;
        }
        if self.setup
            && self.setup_target.is_none()
            && self.discovery.topic == topic
            && output != Output::Robot
        {
            self.subscribe();
            if !self.setup {
                if let Some(id) = self.selected {
                    self.set_display_enabled(id, true);
                }
            }
            return;
        }
        let active: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.source_label() == topic && (v.enabled || v.subscription.is_some()))
            .map(|v| v.id)
            .collect();
        if !active.is_empty()
            || self
                .topic_workflow
                .replacement
                .as_ref()
                .is_some_and(|(_, name, _, _)| name == topic)
        {
            self.topic_workflow.replacement = None;
            for id in active {
                if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                    view.enabled = false;
                    view.pending = None;
                }
                if self
                    .views
                    .iter()
                    .any(|v| v.id == id && v.status != ViewStatus::Stopping)
                {
                    self.stop_view(id);
                }
            }
            return;
        }
        let preferred = self
            .views
            .iter()
            .find(|v| Some(v.id) == self.selected && v.source_label() == topic)
            .or_else(|| self.views.iter().find(|v| v.source_label() == topic));
        let (mode, typ) = preferred
            .map(|v| {
                (
                    v.configuration.output.clone(),
                    if v.configuration.output == Output::Robot {
                        ty.into()
                    } else {
                        v.configuration.message_type.clone()
                    },
                )
            })
            .unwrap_or((output, ty.into()));
        self.request_topic_mode(mode, topic, &typ, true);
    }

    pub(super) fn change_topic_mode(&mut self, output: Output, topic: &str, ty: &str) {
        let enabled = self
            .views
            .iter()
            .any(|v| v.source_label() == topic && v.enabled)
            || self
                .topic_workflow
                .replacement
                .as_ref()
                .is_some_and(|(_, name, _, enabled)| name == topic && *enabled);
        self.request_topic_mode(output, topic, ty, enabled);
    }

    fn request_topic_mode(&mut self, output: Output, topic: &str, ty: &str, enabled: bool) {
        if self.workspace_frozen() || !self.allow_source_navigation() {
            return;
        }
        self.topic_workflow.replacement = Some((output, topic.into(), ty.into(), enabled));
        let ids: Vec<_> = self
            .views
            .iter()
            .filter(|v| v.source_label() == topic && v.subscription.is_some())
            .map(|v| v.id)
            .collect();
        for id in ids {
            let view = self.views.iter_mut().find(|v| v.id == id).unwrap();
            view.enabled = false;
            view.pending = None;
            if view.status != ViewStatus::Stopping {
                self.stop_view(id);
            }
        }
        self.poll_topic_workflow();
    }

    pub(super) fn poll_topic_workflow(&mut self) {
        if self
            .topic_workflow
            .refresh
            .is_some_and(|(_, _, at)| at.elapsed() >= Duration::from_secs(8))
        {
            self.topic_workflow.refresh = None;
            self.topic_action_error =
                Some("Topic refresh timed out. The last list is retained; retry Refresh.".into());
        }
        let Some((_, topic, _, _)) = &self.topic_workflow.replacement else {
            return;
        };
        if self.workspace_frozen() {
            return;
        }
        if self.connection.phase != Phase::Connected
            || self
                .views
                .iter()
                .any(|v| v.source_label() == topic && v.status == ViewStatus::NeedsRecovery)
        {
            self.topic_workflow.replacement = None;
            self.topic_action_error = Some(
                "Display change cancelled. Reconnect or confirm cleanup before retrying.".into(),
            );
            return;
        }
        if self
            .views
            .iter()
            .any(|v| v.source_label() == topic && v.subscription.is_some())
        {
            return;
        }
        let (output, topic, ty, enabled) = self.topic_workflow.replacement.take().unwrap();
        if enabled {
            self.show_topic(output, &topic, &ty);
        } else if let Some(id) = self
            .views
            .iter()
            .find(|v| {
                v.source_label() == topic
                    && std::mem::discriminant(&v.configuration.output)
                        == std::mem::discriminant(&output)
            })
            .map(|v| v.id)
        {
            self.select_display(id);
        } else {
            self.begin_source(output, &topic, &ty);
            self.subscribe();
        }
    }

    pub(super) fn refresh_topics(&mut self) {
        if self.connection.phase != Phase::Connected || self.topic_workflow.refresh.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(next) = self.topic_workflow.next_refresh.checked_add(1) else {
            return;
        };
        self.topic_workflow.next_refresh = next;
        let generation = session.shared.lock().unwrap().registry.generation();
        let request = RequestId(next);
        match session.send(Command::Tool {
            generation,
            request,
            subscription: SubscriptionId(request.0),
            operation: Operation::Graph {
                section: "Topics".into(),
                node: String::new(),
            },
        }) {
            Ok(()) => {
                self.topic_workflow.refresh = Some((generation, request, Instant::now()));
                self.topic_action_error = None;
            }
            Err(error) => {
                self.topic_action_error =
                    Some(format!("Could not refresh topics: {error}. Retry Refresh."))
            }
        }
    }

    pub(super) fn topic_refresh_event(&mut self, event: &Event) -> bool {
        let Event::Tool {
            generation,
            request,
            subscription,
            update,
        } = event
        else {
            return false;
        };
        if subscription.0 != request.0 {
            return false;
        }
        if !self
            .topic_workflow
            .refresh
            .is_some_and(|(g, r, _)| g == *generation && r == *request)
        {
            return true;
        }
        if update.terminal {
            self.topic_workflow.refresh = None;
            if update.state == "graph" {
                match serde_json::from_value::<std::collections::BTreeMap<String, Vec<String>>>(
                    update.value.clone(),
                ) {
                    Ok(topics) => {
                        let graph = Event::Graph {
                            topics: topics.into_iter().collect(),
                        };
                        if graph.validate().is_ok() {
                            self.handle_at(graph, Instant::now());
                        } else {
                            self.topic_action_error = Some("Topic refresh exceeded supported limits. The previous list is retained.".into());
                        }
                    }
                    Err(_) => {
                        self.topic_action_error = Some(
                            "Invalid topic refresh response. The previous list is retained.".into(),
                        )
                    }
                }
            } else {
                self.topic_action_error = Some(format!(
                    "Topic refresh failed: {}. Retry Refresh.",
                    update.value
                ));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_tests::{connected, ingest, started, stopped};
    use std::sync::mpsc::TryRecvError;

    #[test]
    fn topic_eye_stops_retains_settings_and_restarts_only_explicitly() {
        let (mut app, commands) = connected();
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        let id = app.views[0].id;
        let saved = app.views[0].configuration.clone();
        app.inspect_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let stop = commands.try_recv().unwrap();
        assert!(!app.views[0].enabled);
        ingest(&mut app, stopped(&stop));
        assert_eq!(app.views[0].configuration, saved);
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Start { .. }
        ));
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.views[0].id, id);
    }

    #[test]
    fn topic_mode_waits_for_cleanup_and_disconnect_cancels_intent() {
        let (mut app, commands) = connected();
        app.show_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        let camera =
            Output::CameraSnapshot(Box::new(studio_protocol::camera_snapshot::InfoSource {
                topic: "/camera_info".into(),
                best_effort: false,
                transient_local: false,
            }));
        app.change_topic_mode(camera, "/camera", images::IMAGE_MESSAGE_TYPE);
        let stop = commands.try_recv().unwrap();
        assert!(matches!(stop, Command::Stop { .. }));
        app.poll_topic_workflow();
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        assert!(app.topic_workflow.replacement.is_some());
        app.stop();
        assert!(app.topic_workflow.replacement.is_none());
    }

    #[test]
    fn active_mode_round_trip_waits_for_stop_and_restores_saved_settings() {
        let (mut app, commands) = connected();
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        let saved = app.views[0].configuration.clone();
        let camera =
            Output::CameraSnapshot(Box::new(studio_protocol::camera_snapshot::InfoSource {
                topic: "/camera_info".into(),
                best_effort: false,
                transient_local: false,
            }));
        app.change_topic_mode(camera, "/camera", images::IMAGE_MESSAGE_TYPE);
        let stop = commands.try_recv().unwrap();
        assert!(matches!(stop, Command::Stop { .. }));
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        ingest(&mut app, stopped(&stop));
        app.poll_topic_workflow();
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        assert_eq!(app.views.iter().filter(|v| v.enabled).count(), 1);
        app.change_topic_mode(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let stop = commands.try_recv().unwrap();
        assert!(matches!(stop, Command::Stop { .. }));
        ingest(&mut app, stopped(&stop));
        app.poll_topic_workflow();
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Start { .. }
        ));
        assert_eq!(app.views[0].configuration, saved);
        assert_eq!(app.views.len(), 2);
        assert_eq!(app.views.iter().filter(|v| v.enabled).count(), 1);
    }

    #[test]
    fn changing_a_stopped_topics_mode_preserves_stopped_intent() {
        let (mut app, commands) = connected();
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let start = commands.try_recv().unwrap();
        ingest(&mut app, started(&start));
        app.toggle_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        let stop = commands.try_recv().unwrap();
        ingest(&mut app, stopped(&stop));
        let camera =
            Output::CameraSnapshot(Box::new(studio_protocol::camera_snapshot::InfoSource {
                topic: "/camera_info".into(),
                best_effort: false,
                transient_local: false,
            }));
        app.change_topic_mode(camera, "/camera", images::IMAGE_MESSAGE_TYPE);
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(app.views.len(), 2);
        assert!(app
            .views
            .iter()
            .all(|v| !v.enabled && v.subscription.is_none()));
        app.change_topic_mode(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
        assert_eq!(app.selected, Some(app.views[0].id));
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn topic_refresh_is_bounded_and_stale_responses_preserve_the_list() {
        let (mut app, commands) = connected();
        let before = app.discovery.topics.clone();
        app.refresh_topics();
        let wire_command = commands.try_recv().unwrap();
        wire_command
            .encode()
            .expect("Refresh must satisfy the actual wire contract");
        let Command::Tool {
            generation,
            request,
            subscription: _,
            ..
        } = wire_command
        else {
            panic!("Expected explicit refresh")
        };
        app.refresh_topics();
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        let response = |request: RequestId, value| Event::Tool {
            generation,
            request,
            subscription: SubscriptionId(request.0),
            update: studio_protocol::tools::Update::new("graph", value, true),
        };
        assert!(app.topic_refresh_event(&response(RequestId(request.0 + 1), serde_json::json!([]))));
        assert_eq!(app.discovery.topics, before);
        app.topic_refresh_event(&response(
            request,
            serde_json::json!({"/fresh": ["sensor_msgs/msg/Image"]}),
        ));
        assert_eq!(app.discovery.topics[0].0, "/fresh");
        assert!(app.topic_workflow.refresh.is_none());
        assert!(app.views.is_empty());
    }
}
