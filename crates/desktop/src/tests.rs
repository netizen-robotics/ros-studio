use super::*;
use std::sync::mpsc;
use studio_protocol::structured::{self, Field, Value};
mod robot_workflow;
mod shell_workflow;
mod spatial_workflow;
mod workspace_workflow;

fn camera_event(command: &Command, sequence: u64, encoding: &str) -> Event {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!("Start required")
    };
    let (step, raw) = if encoding == "rgba8" {
        (8, vec![255, 0, 0, 255, 0, 255, 0, 128])
    } else {
        (4, vec![0, 1, 2, 3])
    };
    let sample = studio_protocol::image::ImageSample::from_raw(
        &configuration.topic,
        studio_protocol::image::ImageMetadata {
            header_stamp: studio_protocol::image::HeaderStamp {
                seconds: 42,
                nanoseconds: 7,
            },
            frame_id: "camera_optical".into(),
            width: 2,
            height: 1,
            encoding: encoding.into(),
            is_bigendian: false,
            step,
        },
        &raw,
    )
    .unwrap();
    Event::ImageObservation {
        generation: *generation,
        subscription: *subscription,
        sequence,
        sample: Box::new(sample),
        backend_drops: 9,
    }
}

#[test]
fn image_output_requires_explicit_replacement_and_stale_frames_cannot_repopulate_it() {
    let (mut app, outgoing) = connected();
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![("/camera".into(), vec![images::IMAGE_MESSAGE_TYPE.into()])],
        },
    );
    let typed = add(&mut app, &outgoing, "/camera");
    assert!(
        matches!(&typed, Command::Start { configuration, .. } if configuration.output == Output::Typed)
    );
    ingest(&mut app, started(&typed));
    let id = app.views[0].id;
    app.configure_view(id);
    app.draft_output = Output::CompactImage;
    app.subscribe();
    let stop = outgoing.try_recv().unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert!(outgoing.try_recv().is_err());
    ingest(&mut app, stopped(&stop));
    let compact = outgoing.try_recv().unwrap();
    assert!(
        matches!(&compact, Command::Start { configuration, .. } if configuration.output == Output::CompactImage)
    );
    assert!(matches!(app.views[0].kind, ViewKind::Image));
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .images
            .len(),
        1
    );
    ingest(&mut app, started(&compact));
    ingest(&mut app, camera_event(&compact, 1, "rgba8"));
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    assert_eq!(app.views[0].received.backend_drops, 9);
    let frame = app.views[0].image.frames().take_with_metadata().unwrap();
    assert_eq!(frame.0.size(), [2, 1]);
    assert_eq!(frame.1.unwrap().frame_id, "camera_optical");
    app.configure_view(id);
    app.draft_output = Output::Typed;
    app.subscribe();
    let stop_image = outgoing.try_recv().unwrap();
    ingest(&mut app, stopped(&stop_image));
    let new_typed = outgoing.try_recv().unwrap();
    assert!(
        matches!(&new_typed, Command::Start { configuration, .. } if configuration.output == Output::Typed)
    );
    assert!(matches!(app.views[0].kind, ViewKind::Messages));
    ingest(&mut app, camera_event(&compact, 2, "rgba8"));
    assert!(app.views[0].image.frames().take().is_none());
    assert!(app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .images
        .is_empty());
}

#[test]
fn local_image_error_requests_real_stop_without_releasing_native_owner_or_peer_data() {
    let (mut app, outgoing) = connected();
    let peer = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&peer));
    ingest(&mut app, sample(&peer, 1, 11));
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![("/camera".into(), vec![images::IMAGE_MESSAGE_TYPE.into()])],
        },
    );
    app.begin_add();
    app.discovery.select("/camera");
    app.draft_output = Output::CompactImage;
    app.subscribe();
    let camera = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&camera));
    ingest(&mut app, camera_event(&camera, 1, "16SC1"));
    let stop = outgoing.try_recv().unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert_eq!(app.views[1].status, ViewStatus::Stopping);
    assert!(
        app.views[1].subscription.is_some(),
        "Decoder error does not confirm native cleanup"
    );
    assert!(!app.views[1].image.has_displayed_frame());
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .entries()
            .len(),
        2
    );
    ingest(&mut app, sample(&peer, 2, 12));
    assert_eq!(app.views[0].received.typed_samples, 2);
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[1].status, ViewStatus::Stopped);
    assert!(app.views[1].subscription.is_none());
    assert!(app.views[1]
        .error
        .as_ref()
        .is_some_and(|error| error.contains("Unsupported encoding")));
    assert!(
        outgoing.try_recv().is_err(),
        "No automatic retry after decode failure"
    );
}

fn connected() -> (App, mpsc::Receiver<Command>) {
    let (session, outgoing) = session::Session::fixture(0);
    let mut app = App::default();
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    ingest(
        &mut app,
        Event::ready("jazzy".into(), "fixture-no-ros".into()),
    );
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![
                ("/one".into(), vec!["std_msgs/msg/Float64".into()]),
                ("/two".into(), vec!["std_msgs/msg/Float64".into()]),
                ("/multi".into(), vec!["a/msg/A".into(), "b/msg/B".into()]),
            ],
        },
    );
    (app, outgoing)
}
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
}
fn enqueue(app: &App, event: Event) {
    app.session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .ingest_for_test(event)
        .unwrap();
}
fn add(app: &mut App, outgoing: &mpsc::Receiver<Command>, topic: &str) -> Command {
    app.begin_add();
    app.discovery.select(topic);
    app.subscribe();
    outgoing.try_recv().unwrap()
}
fn started(command: &Command) -> Event {
    let (generation, request, subscription) = command.ids();
    Event::Started {
        generation,
        request,
        subscription,
    }
}
fn stopped(command: &Command) -> Event {
    let (generation, request, subscription) = command.ids();
    Event::Stopped {
        generation,
        request,
        subscription,
    }
}
fn sample(command: &Command, sequence: u64, value: i64) -> Event {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!("expected start");
    };
    Event::Observation {
        generation: *generation,
        subscription: *subscription,
        sequence,
        backend_drops: 7,
        sample: Box::new(structured::Sample {
            contract_version: 2,
            topic: configuration.topic.clone(),
            message_type: configuration.message_type.clone(),
            source_time: None,
            value: Value::Object {
                fields: vec![Field {
                    name: "data".into(),
                    value: Value::Signed {
                        value: value.to_string(),
                    },
                }],
                original_fields: 1,
            },
        }),
    }
}
fn path() -> plots::FieldPath {
    plots::FieldPath(vec![plots::PathSegment::Field("data".into())])
}

fn reconnected_stopped_view_before_graph() -> (App, mpsc::Receiver<Command>) {
    let (mut app, outgoing) = connected();
    let start = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&start));
    app.stop();
    let (session, reconnect_commands) = session::Session::fixture(app.session_generation);
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    ingest(
        &mut app,
        Event::ready("jazzy".into(), "fixture-no-ros".into()),
    );
    assert_eq!(app.connection.phase, Phase::Connected);
    assert!(!app.discovery.received_graph);
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    (app, reconnect_commands)
}

#[test]
fn pending_discovery_keeps_stopped_view_without_false_unavailable_error_or_activation() {
    let (mut app, outgoing) = reconnected_stopped_view_before_graph();
    app.start_view(ViewId(1));
    assert!(outgoing.try_recv().is_err());
    assert!(app.views[0].subscription.is_none());
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(
        app.views[0].error.is_none(),
        "Pending discovery cannot establish that this topic is unavailable"
    );
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![("/one".into(), vec!["std_msgs/msg/Float64".into()])],
        },
    );
    assert!(app.discovery.received_graph);
    assert!(
        outgoing.try_recv().is_err(),
        "Graph arrival must never replay the previous Subscribe action"
    );
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    app.start_view(ViewId(1));
    assert!(
        matches!(outgoing.try_recv().unwrap(), Command::Start { configuration, .. } if configuration.topic == "/one" && configuration.message_type == "std_msgs/msg/Float64")
    );
    assert_eq!(app.views[0].status, ViewStatus::Starting);
}

#[test]
fn authoritative_empty_discovery_preserves_missing_topic_error_without_starting() {
    let (mut app, outgoing) = reconnected_stopped_view_before_graph();
    ingest(&mut app, Event::Graph { topics: vec![] });
    assert!(app.discovery.received_graph);
    assert!(outgoing.try_recv().is_err());
    app.start_view(ViewId(1));
    assert!(outgoing.try_recv().is_err());
    assert!(app.views[0].subscription.is_none());
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(app.views[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Topic/type is unavailable")));
}

#[test]
fn discovery_and_multitype_browsing_never_activate() {
    let (mut app, outgoing) = connected();
    assert!(app.views.is_empty());
    assert!(outgoing.try_recv().is_err());
    app.discovery.select("/multi");
    app.begin_add();
    app.subscribe();
    assert!(app.views.is_empty());
    assert!(app.discovery.ty.is_empty());
    app.discovery.ty = "b/msg/B".into();
    app.subscribe();
    let command = outgoing.try_recv().unwrap();
    let Command::Start { configuration, .. } = command else {
        panic!()
    };
    assert_eq!(configuration.message_type, "b/msg/B");
    assert_eq!(app.views[0].status, ViewStatus::Starting);
}
#[test]
fn two_views_route_and_stopping_one_preserves_peer_data_and_trace() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let two = add(&mut app, &outgoing, "/two");
    ingest(&mut app, started(&two));
    ingest(&mut app, sample(&one, 1, 11));
    ingest(&mut app, sample(&two, 1, 22));
    let peer_origin = app.views[1].origin;
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[0].received.typed.is_none());
    assert_eq!(app.views[1].received.typed_samples, 1);
    assert_eq!(app.views[1].origin, peer_origin);
    ingest(&mut app, sample(&one, 2, 999));
    assert!(app.views[0].received.typed.is_none());
    ingest(&mut app, stopped(&stop));
    ingest(&mut app, sample(&two, 2, 23));
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert_eq!(app.views[1].received.typed_samples, 2);
    assert_eq!(app.views[1].received.backend_drops, 7);
    assert_eq!(
        app.views[0].received.backend_drops, 7,
        "drops remain visible after stopping"
    );
}
#[test]
fn replacement_is_explicit_and_waits_for_confirmed_cleanup() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.discovery.select("/two");
    assert_eq!(app.views[0].configuration.topic, "/one");
    assert!(outgoing.try_recv().is_err());
    app.configure_view(ViewId(1));
    app.discovery.select("/two");
    app.draft_best_effort = true;
    app.subscribe();
    let stop = outgoing.try_recv().unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert!(outgoing.try_recv().is_err());
    assert_eq!(app.views[0].configuration.topic, "/one");
    ingest(&mut app, stopped(&stop));
    let replacement = outgoing.try_recv().unwrap();
    let Command::Start {
        subscription,
        configuration,
        ..
    } = &replacement
    else {
        panic!()
    };
    assert_ne!(*subscription, one.ids().2);
    assert_eq!(configuration.topic, "/two");
    assert!(configuration.best_effort);
    assert_eq!(app.views.len(), 1);
    ingest(&mut app, started(&one)); // retired identity cannot activate replacement.
    assert_eq!(app.views[0].status, ViewStatus::Starting);
    ingest(&mut app, started(&replacement));
    ingest(&mut app, sample(&replacement, 1, 42));
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
}
#[test]
fn cancel_late_ack_and_contradictory_failure_never_revive_or_free_uncertain_handle() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&one));
    let (generation, request, subscription) = one.ids();
    ingest(
        &mut app,
        Event::RequestError {
            generation,
            request,
            subscription,
            message: "contradictory failure".into(),
            no_handle: true,
        },
    );
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    assert_eq!(app.views[0].subscription, Some(subscription));
    ingest(&mut app, sample(&one, 1, 99));
    assert!(app.views[0].received.typed.is_none());
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
}
#[test]
fn definitive_start_failure_after_cancel_releases_only_that_view() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    let two = add(&mut app, &outgoing, "/two");
    ingest(&mut app, started(&two));
    app.stop_view(ViewId(1));
    outgoing.try_recv().unwrap();
    let (generation, request, subscription) = one.ids();
    ingest(
        &mut app,
        Event::RequestError {
            generation,
            request,
            subscription,
            message: "open failed after cancel".into(),
            no_handle: true,
        },
    );
    assert!(app.views[0].subscription.is_none());
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    ingest(&mut app, sample(&two, 1, 2));
    assert_eq!(app.views[1].status, ViewStatus::Receiving);
}
#[test]
fn stop_retry_rejects_old_ack_and_does_not_restore_pending_replacement() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.stop_view(ViewId(1));
    let old = outgoing.try_recv().unwrap();
    app.check_deadlines(Instant::now() + Duration::from_secs(11));
    assert_eq!(app.views[0].status, ViewStatus::NeedsRecovery);
    app.stop_view(ViewId(1));
    let retry = outgoing.try_recv().unwrap();
    assert_ne!(old.ids().1, retry.ids().1);
    ingest(&mut app, stopped(&old));
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    ingest(&mut app, stopped(&retry));
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(outgoing.try_recv().is_err());
}
#[test]
fn timed_out_start_is_canceled_without_implying_cleanup() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    app.check_deadlines(Instant::now() + Duration::from_secs(16));
    let stop = outgoing.try_recv().unwrap();
    assert_eq!(stop.ids().2, one.ids().2);
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    assert!(app.views[0].subscription.is_some());
}
#[test]
fn retained_sample_survives_fair_dequeue_before_ack_presentation() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    enqueue(&app, started(&one));
    enqueue(&app, sample(&one, 1, 77));
    app.poll_backend();
    assert_eq!(app.views[0].received.typed_samples, 1);
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    assert!(app.views[0].deadline.is_none());
}
#[test]
fn scoped_failure_keeps_flood_drop_counters_and_peer_progress() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let two = add(&mut app, &outgoing, "/two");
    ingest(&mut app, started(&two));
    for n in 1..=100 {
        enqueue(&app, sample(&one, n, n as i64));
    }
    let (generation, _, subscription) = one.ids();
    enqueue(
        &app,
        Event::StreamError {
            generation,
            subscription,
            message: "synthetic failure".into(),
        },
    );
    ingest(&mut app, sample(&two, 1, 2));
    assert_eq!(app.views[0].queue_drops, 84);
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(app.views[0].received.typed.is_none());
    assert_eq!(app.views[1].status, ViewStatus::Receiving);
    assert!(app
        .diagnostics
        .back()
        .unwrap()
        .contains("synthetic failure"));
}
#[test]
fn ninth_configuration_is_rejected_without_implicit_replacement() {
    let (mut app, outgoing) = connected();
    for _ in 0..8 {
        let command = add(&mut app, &outgoing, "/one");
        ingest(&mut app, started(&command));
    }
    app.begin_add();
    app.discovery.select("/two");
    app.subscribe();
    assert_eq!(app.views.len(), 8);
    assert!(outgoing.try_recv().is_err());
    assert!(app.views.iter().all(|v| v.status == ViewStatus::Waiting));
    assert!(app.diagnostics.back().unwrap().contains("Eight displays"));
}
#[test]
fn reconnect_and_early_cancel_keep_monotonic_generations_without_replay() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let old = app.session.as_ref().unwrap().shared.clone();
    app.stop();
    assert_eq!(app.session_generation, 1);
    let (session, new_commands) = session::Session::fixture(app.session_generation);
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    enqueue(&app, Event::ready("jazzy".into(), "fixture".into()));
    // Cancel before the Ready label gets a UI frame.
    app.stop();
    assert_eq!(app.session_generation, 2);
    let (session, third_commands) = session::Session::fixture(app.session_generation);
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    ingest(&mut app, Event::ready("jazzy".into(), "fixture".into()));
    assert_eq!(app.session_generation, 3);
    assert!(new_commands.try_recv().is_err());
    assert!(third_commands.try_recv().is_err());
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    old.lock()
        .unwrap()
        .ingest_for_test(sample(&one, 1, 9))
        .unwrap();
    app.poll_backend();
    assert!(app.views[0].received.typed.is_none());
}
#[test]
fn plot_is_local_and_uses_reader_receipt_time_with_independent_history() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    ingest(&mut app, sample(&one, 1, 1));
    app.views[0].plot_draft = Some(path());
    app.open_plot(ViewId(1));
    assert!(matches!(app.views[0].kind, ViewKind::Plot(_)));
    assert!(outgoing.try_recv().is_err());
    let receipt = Duration::from_secs(1);
    let event = sample(&one, 2, 42);
    // Exercise actual ingress once, then delay only UI presentation.
    enqueue(&app, event);
    let packet = app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .queue
        .pop()
        .unwrap();
    let Packet::Sample { bytes, .. } = packet else {
        panic!()
    };
    app.views[0].origin = bytes.received_at - receipt;
    app.handle_at(bytes.event, bytes.received_at);
    assert_eq!(app.views[0].trace.points().back().unwrap().receipt, receipt);
    app.views[0].trace.set_paused(true);
    ingest(&mut app, sample(&one, 3, 43));
    assert_eq!(app.views[0].trace.points().len(), 1);
    assert_eq!(app.views[0].trace.counters.skipped_paused, 1);
    app.stop_view(ViewId(1));
    assert!(app.views[0].trace.points().is_empty());
}
#[test]
fn command_pressure_fails_visibly_and_clears_all_live_views() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let session = app.session.as_ref().unwrap();
    for n in 0..studio_protocol::subscriptions::MAX_SUBSCRIPTIONS as u64 * 2 {
        session
            .send(Command::Stop {
                generation: Generation(1),
                request: RequestId(n + 10),
                subscription: SubscriptionId(999),
            })
            .unwrap();
    }
    app.stop_view(ViewId(1));
    assert_eq!(app.connection.phase, Phase::Failed);
    assert!(app.session.is_none());
    assert!(app.views[0].subscription.is_none());
}
#[test]
fn domain_and_manifest_validation_are_explicit() {
    let mut app = App::default();
    app.connection.domain = "233".into();
    app.connect();
    assert_eq!(app.connection.phase, Phase::Failed);
    assert!(app.session.is_none());
    assert!(bundled_distro(r#"{"distro":"jazzy"}"#).is_ok());
    for bad in ["{}", r#"{"distro":"unknown"}"#, "not-json"] {
        assert!(bundled_distro(bad).is_err());
    }
}
#[test]
fn configuring_preserves_applied_qos_and_browsing_drafts_do_not_apply() {
    let (mut app, outgoing) = connected();
    app.begin_add();
    app.discovery.select("/one");
    app.draft_best_effort = true;
    app.draft_transient_local = true;
    app.subscribe();
    let one = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&one));
    app.configure_view(ViewId(1));
    assert!(app.draft_best_effort && app.draft_transient_local);
    app.discovery.select("/multi");
    app.draft_best_effort = false;
    assert_eq!(app.views[0].configuration.topic, "/one");
    assert!(app.views[0].configuration.best_effort);
    assert!(outgoing.try_recv().is_err());
}
#[test]
fn native_exit_callback_clears_runtime_without_waiting_for_app_drop() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    eframe::App::on_exit(&mut app);
    assert!(app.session.is_none());
    assert!(app.views[0].subscription.is_none());
    assert_eq!(app.connection.phase, Phase::Disconnected);
}
#[test]
fn all_states_render_at_laptop_and_desktop_sizes() {
    for size in [
        egui::vec2(760.0, 560.0),
        egui::vec2(960.0, 820.0),
        egui::vec2(1440.0, 900.0),
    ] {
        let (mut app, outgoing) = connected();
        let one = add(&mut app, &outgoing, "/one");
        let ctx = egui::Context::default();
        for status in [
            ViewStatus::Stopped,
            ViewStatus::Starting,
            ViewStatus::Waiting,
            ViewStatus::Receiving,
            ViewStatus::Stopping,
            ViewStatus::NeedsRecovery,
        ] {
            app.views[0].status = status;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            assert!(!output.shapes.is_empty());
        }
        ingest(&mut app, started(&one));
    }
}

#[test]
fn accessibility_tree_exposes_named_connection_and_search_inputs() {
    let mut app = App::default();
    app.settings = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let tree = output
        .platform_output
        .accesskit_update
        .expect("Accessibility tree must be emitted");
    let labels: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == egui::accesskit::Role::TextInput)
        .flat_map(|(_, node)| node.labelled_by())
        .filter_map(|label| tree.nodes.iter().find(|(id, _)| id == label))
        .filter_map(|(_, node)| node.label().or_else(|| node.value()))
        .collect();
    assert!(
        labels.contains(&"Domain"),
        "Missing domain label: {labels:?}"
    );
    assert!(
        labels.contains(&"Search topics or types"),
        "Missing topic-search label: {labels:?}"
    );
    assert!(tree
        .nodes
        .iter()
        .any(|(_, node)| node.role() == egui::accesskit::Role::Button
            && node.label() == Some("Connect")));
}

#[test]
fn accessibility_tree_exposes_discovered_topic_and_type_choices() {
    let (mut app, _commands) = connected();
    app.discovery.select("/one");
    app.setup = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let tree = output.platform_output.accesskit_update.unwrap();
    for (label, value) in [("Topic", "/one"), ("Message type", "std_msgs/msg/Float64")] {
        assert!(
            tree.nodes
                .iter()
                .any(|(_, node)| node.role() == egui::accesskit::Role::ComboBox
                    && node.label() == Some(label)
                    && node.value() == Some(value)),
            "Missing {label}: {value}"
        );
    }
}

#[test]
fn accessible_type_selection_closes_popup_without_subscribing() {
    let (mut app, _commands) = connected();
    app.discovery.select("/multi");
    app.setup = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut frame = |event: Option<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                events: event.into_iter().collect(),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        )
    };
    let initial = frame(None).platform_output.accesskit_update.unwrap();
    let type_choice = initial
        .nodes
        .iter()
        .find(|(_, node)| {
            node.role() == egui::accesskit::Role::ComboBox && node.label() == Some("Message type")
        })
        .unwrap()
        .0;
    let click = |target| {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target,
            data: None,
        })
    };
    let opened = frame(Some(click(type_choice)))
        .platform_output
        .accesskit_update
        .unwrap();
    let selected = opened
        .nodes
        .iter()
        .find(|(_, node)| {
            node.role() == egui::accesskit::Role::Button && node.label() == Some("b/msg/B")
        })
        .unwrap()
        .0;
    frame(Some(click(selected)));
    assert!(
        !ctx.memory(|memory| memory.any_popup_open()),
        "A confirmed type must close its choice popup"
    );
    assert_eq!(app.discovery.ty, "b/msg/B");
    assert!(!app.views.iter().any(|view| view.subscription.is_some()));
    assert!(_commands.try_recv().is_err());
}

#[test]
fn long_setup_choices_fit_inspector_and_keep_full_accessible_identity() {
    let topic = format!("/namespace/{}/odometry", "long_robot_subsystem/".repeat(16));
    let ty = format!(
        "{}_interfaces/msg/LongSensorMessage",
        "robot_subsystem_".repeat(16)
    );
    for width in [960.0, 1280.0] {
        let (mut app, _commands) = connected();
        app.discovery.topics = vec![(topic.clone(), vec![ty.clone()])];
        app.discovery.select(&topic);
        app.setup = true;
        app.inspector = true;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut frame = |event: Option<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 820.0),
                    )),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            )
        };
        let output = frame(None).platform_output.accesskit_update.unwrap();
        let mut topic_choice = None;
        let mut type_choice = None;
        for (label, value) in [("Topic", &topic), ("Message type", &ty)] {
            let (id, node) = output
                .nodes
                .iter()
                .find(|(_, node)| {
                    node.role() == egui::accesskit::Role::ComboBox && node.label() == Some(label)
                })
                .expect("Setup must expose both choices");
            assert_eq!(
                node.value(),
                Some(value.as_str()),
                "Full identity must survive visual truncation"
            );
            let bounds = node.bounds().unwrap();
            assert!(
                bounds.x1 <= f64::from(width),
                "{label} right edge {} exceeds window {width}",
                bounds.x1
            );
            assert!(
                bounds.x1 - bounds.x0 <= 370.0,
                "{label} must stay within maximum inspector width"
            );
            if label == "Topic" {
                topic_choice = Some(*id);
            } else {
                type_choice = Some(*id);
            }
        }
        let output = frame(Some(egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target: topic_choice.unwrap(),
                data: None,
            },
        )))
        .platform_output
        .accesskit_update
        .unwrap();
        let (option_id, option) = output
            .nodes
            .iter()
            .find(|(_, node)| {
                node.role() == egui::accesskit::Role::Button && node.label() == Some(topic.as_str())
            })
            .expect("Menu must retain the full original topic label");
        let bounds = option.bounds().unwrap();
        assert!(
            bounds.x1 <= f64::from(width),
            "Topic option right edge {} exceeds window {width}",
            bounds.x1
        );
        frame(Some(egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target: *option_id,
                data: None,
            },
        )));
        let output = frame(Some(egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target: type_choice.unwrap(),
                data: None,
            },
        )))
        .platform_output
        .accesskit_update
        .unwrap();
        let (_, option) = output
            .nodes
            .iter()
            .find(|(_, node)| {
                node.role() == egui::accesskit::Role::Button && node.label() == Some(ty.as_str())
            })
            .expect("Menu must retain the full original message type label");
        assert!(
            option.bounds().unwrap().x1 <= f64::from(width),
            "Message type option must fit the window"
        );
        assert!(
            !app.views.iter().any(|view| view.subscription.is_some()),
            "Browsing choices must never subscribe"
        );
    }
}

#[test]
fn queued_old_stop_error_cannot_cancel_a_fresh_stop_retry_deadline() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.stop_view(ViewId(1));
    let first_stop = outgoing.try_recv().unwrap();
    let (generation, request, subscription) = first_stop.ids();
    enqueue(
        &app,
        Event::RequestError {
            generation,
            request,
            subscription,
            message: "old stop failed".into(),
            no_handle: false,
        },
    );
    app.stop_view(ViewId(1));
    let retry = outgoing.try_recv().unwrap();
    let deadline = app.views[0].deadline;
    app.poll_backend();
    assert_eq!(app.views[0].request, Some(retry.ids().1));
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    assert_eq!(app.views[0].deadline, deadline);
    assert!(app.views[0].error.is_none());
    assert!(app.diagnostics.back().unwrap().contains("old stop failed"));
}

#[test]
fn final_backend_drops_survive_stop_without_a_final_sample() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    let (generation, _, subscription) = one.ids();
    enqueue(
        &app,
        Event::Drops {
            generation,
            subscription,
            backend_drops: 84,
            spatial_counters: None,
        },
    );
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[0].received.backend_drops, 84);
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(app.views[0].received.typed.is_none());
}

#[test]
fn counter_metadata_never_activates_or_regresses_behind_queued_samples() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    let (generation, _, subscription) = one.ids();
    ingest(
        &mut app,
        Event::Drops {
            generation,
            subscription,
            backend_drops: 84,
            spatial_counters: None,
        },
    );
    assert_eq!(app.views[0].status, ViewStatus::Starting);
    assert!(app.views[0].received.typed.is_none());
    ingest(&mut app, started(&one));
    ingest(&mut app, sample(&one, 1, 10));
    assert_eq!(app.views[0].received.backend_drops, 84);
    ingest(
        &mut app,
        Event::StreamError {
            generation,
            subscription,
            message: "worker failed".into(),
        },
    );
    assert_eq!(app.views[0].received.backend_drops, 84);
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
}

#[test]
fn display_toggle_cancels_start_and_never_replays_while_cleanup_is_pending() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    let id = app.views[0].id;
    assert!(app.views[0].enabled);
    app.set_display_enabled(id, false);
    let stop = commands.try_recv().unwrap();
    assert!(!app.views[0].enabled);
    assert!(app.views[0].subscription.is_some());
    app.set_display_enabled(id, true);
    app.set_display_enabled(id, false);
    ingest(&mut app, started(&first));
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    assert!(commands.try_recv().is_err());
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(!app.views[0].enabled);
    assert!(commands.try_recv().is_err());
    app.set_display_enabled(id, true);
    let next = commands.try_recv().unwrap();
    assert_ne!(first.ids().2, next.ids().2);
    assert!(app.views[0].enabled);
    ingest(&mut app, sample(&first, 9, 999));
    assert!(app.views[0].received.typed.is_none());
}

#[test]
fn disable_clears_history_and_buffers_and_preserves_plot_configuration() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&first));
    ingest(&mut app, sample(&first, 1, 10));
    let id = app.views[0].id;
    let path = app.views[0].numeric_choices[0].clone();
    app.views[0].plot_draft = Some(path.clone());
    app.open_plot(id);
    ingest(&mut app, sample(&first, 2, 20));
    assert!(!app.views[0].trace.points().is_empty());
    let before = app.workspace_snapshot().unwrap();
    app.set_display_enabled(id, false);
    let stop = commands.try_recv().unwrap();
    ingest(&mut app, stopped(&stop));
    assert!(app.views[0].trace.points().is_empty());
    assert!(app.views[0].received.typed.is_none());
    assert_eq!(app.workspace_snapshot().unwrap().views, before.views);
    app.set_display_enabled(id, true);
    let next = commands.try_recv().unwrap();
    assert!(matches!(&app.views[0].kind, ViewKind::Plot(p) if p == &path));
    ingest(&mut app, started(&next));
    ingest(&mut app, sample(&first, 3, 30));
    assert!(app.views[0].trace.points().is_empty());
    ingest(&mut app, sample(&next, 1, 40));
    assert_eq!(app.views[0].trace.points().len(), 1);
}

#[test]
fn disable_abandons_pending_replacement_without_restarting_after_cleanup() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&first));
    let id = app.views[0].id;
    app.configure_view(id);
    app.draft_best_effort = true;
    app.subscribe();
    let stop = commands.try_recv().unwrap();
    assert!(app.views[0].pending.is_some());
    // Replacement is already Stopping, so the disabled toggle cannot be used.
    // Explicit disconnect also abandons the pending replacement.
    app.stop();
    assert!(app.views[0].pending.is_none());
    assert!(!app.views[0].enabled);
    assert!(commands.try_recv().is_err());
    assert!(matches!(stop, Command::Stop { .. }));
}

#[test]
fn failed_activation_leaves_toggle_off_with_error_and_no_automatic_retry() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    let (generation, request, subscription) = first.ids();
    ingest(
        &mut app,
        Event::RequestError {
            generation,
            request,
            subscription,
            no_handle: true,
            message: "Fixture start rejected".into(),
        },
    );
    assert!(!app.views[0].enabled);
    assert!(app.views[0].subscription.is_none());
    assert!(app.views[0].error.as_ref().unwrap().contains("rejected"));
    assert!(commands.try_recv().is_err());
    let id = app.views[0].id;
    app.discovery.topics.clear();
    app.set_display_enabled(id, true);
    assert!(!app.views[0].enabled);
    assert!(app.setup);
    assert!(app.views[0].error.as_ref().unwrap().contains("unavailable"));
    assert!(commands.try_recv().is_err());
}
