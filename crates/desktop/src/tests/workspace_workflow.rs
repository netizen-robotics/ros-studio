use super::*;
use studio_desktop::workspace as file;

fn poll_workspace(app: &mut App) {
    app.workspace_poll(&egui::Context::default());
}

#[test]
fn replacement_waits_for_every_exact_owner_and_never_activates_restored_views() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    let two = add(&mut app, &outgoing, "/two");
    ingest(&mut app, started(&one));
    ingest(&mut app, started(&two));
    let old_ids = app.views.iter().map(|view| view.id).collect::<Vec<_>>();
    app.workspace_review_configuration(file::Preset::Analyze.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    let stop_one = outgoing.try_recv().unwrap();
    let stop_two = outgoing.try_recv().unwrap();
    assert!(matches!(stop_one, Command::Stop { .. }));
    assert!(matches!(stop_two, Command::Stop { .. }));
    assert!(app.workspace_frozen());
    app.begin_add();
    app.configure_view(old_ids[0]);
    app.remove_view(old_ids[0]);
    app.start_view(old_ids[0]);
    app.subscribe();
    assert_eq!(app.views.len(), 2);
    assert!(outgoing.try_recv().is_err());
    let (generation, request, subscription) = stop_one.ids();
    ingest(
        &mut app,
        Event::Stopped {
            generation,
            request: RequestId(request.0 + 99),
            subscription,
        },
    );
    poll_workspace(&mut app);
    assert!(app.workspace_frozen());
    assert_eq!(
        app.views.iter().map(|view| view.id).collect::<Vec<_>>(),
        old_ids
    );
    ingest(&mut app, stopped(&stop_one));
    poll_workspace(&mut app);
    assert!(app.workspace_frozen());
    assert_eq!(app.views[1].id, old_ids[1]);
    ingest(&mut app, stopped(&stop_two));
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert!(app
        .views
        .iter()
        .all(|view| view.subscription.is_none() && view.status == ViewStatus::Stopped));
    assert!(app.views.iter().all(|view| !old_ids.contains(&view.id)));
    assert!(matches!(app.views[0].kind, ViewKind::Plot(_)));
    assert!(outgoing.try_recv().is_err());
    assert_eq!(app.connection.phase, Phase::Connected);
}

#[test]
fn accepted_stream_failure_confirms_group_cleanup_but_unproven_terminal_event_does_not() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.workspace_review_configuration(file::Preset::Inspect.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    let stop = outgoing.try_recv().unwrap();
    let (generation, _, subscription) = stop.ids();
    // Bypass ingress deliberately: matching identity alone must not prove cleanup.
    app.handle_at(
        Event::StreamError {
            generation,
            subscription,
            message: "unproven".into(),
        },
        Instant::now(),
    );
    poll_workspace(&mut app);
    assert!(app.workspace_frozen());
    assert!(app.views[0].subscription.is_some());
    ingest(
        &mut app,
        Event::StreamError {
            generation,
            subscription,
            message: "worker joined".into(),
        },
    );
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert_eq!(app.views[0].configuration.topic, "/demo/status");
    assert!(app.views[0].subscription.is_none());
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn stop_timeout_preserves_panels_until_manual_retry_and_authoritative_cleanup() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let original = app.views[0].id;
    app.workspace_review_configuration(file::Preset::Analyze.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    let stop = outgoing.try_recv().unwrap();
    app.check_deadlines(Instant::now() + Duration::from_secs(11));
    poll_workspace(&mut app);
    assert!(app.workspace_frozen());
    assert_eq!(app.views[0].id, original);
    assert_eq!(app.views[0].status, ViewStatus::NeedsRecovery);
    app.stop_view(original);
    let retry = outgoing.try_recv().unwrap();
    assert_ne!(retry.ids().1, stop.ids().1);
    ingest(&mut app, stopped(&stop));
    poll_workspace(&mut app);
    assert!(app.workspace_frozen());
    ingest(&mut app, stopped(&retry));
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert_eq!(app.views.len(), 2);
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn replacement_adopts_an_existing_stop_without_changing_its_request_or_replaying_pending_start() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.configure_view(ViewId(1));
    app.discovery.select("/two");
    app.subscribe();
    let existing_stop = outgoing.try_recv().unwrap();
    assert!(app.views[0].pending.is_some());
    app.workspace_review_configuration(file::Preset::Inspect.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    assert!(outgoing.try_recv().is_err());
    assert!(app.views[0].pending.is_none());
    assert_eq!(app.views[0].request, Some(existing_stop.ids().1));
    ingest(&mut app, stopped(&existing_stop));
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert_eq!(app.views[0].configuration.topic, "/demo/status");
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn disconnect_invalidates_pending_restore_and_late_ack_cannot_replace_configuration() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    app.workspace_review_configuration(file::Preset::Analyze.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    let stop = outgoing.try_recv().unwrap();
    let original_id = app.views[0].id;
    app.stop();
    app.handle_at(stopped(&stop), Instant::now());
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.views[0].id, original_id);
    assert_eq!(app.views[0].configuration.topic, "/one");
    assert!(app.views[0].subscription.is_none());
    assert!(app.session.is_none());
}

#[test]
fn unsupported_candidate_cannot_stop_current_observation() {
    let (mut app, outgoing) = connected();
    let one = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&one));
    let mut candidate = file::Preset::Visualize.workspace();
    let file::ViewKind::Spatial { transforms, .. } = &mut candidate.views[0].kind else {
        panic!()
    };
    *transforms = None;
    assert!(app.workspace_review_configuration(candidate).is_err());
    assert!(app
        .workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .is_err());
    assert!(app.views[0].status.active());
    assert!(!app.workspace_frozen());
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn verified_spatial_start_persists_only_system_expectation_across_disconnect() {
    let (mut app, outgoing) = connected();
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![
                ("/scan".into(), vec![spatial::SCAN_MESSAGE_TYPE.into()]),
                ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
            ],
        },
    );
    app.begin_add();
    app.discovery.select("/scan");
    app.draft_output = Output::SpatialScan;
    app.draft_spatial.fixed_frame = "map".into();
    app.draft_clock_verified = true;
    app.subscribe();
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    assert_eq!(
        app.views[0].workspace_clock_expectation,
        file::ClockExpectation::System
    );
    app.configure_view(app.views[0].id);
    assert!(app.draft_clock_verified);
    app.stop();
    assert!(!app.views[0].spatial_clock_verified);
    assert!(!app.draft_clock_verified);
    assert_eq!(
        app.draft_spatial.clock_policy,
        studio_protocol::spatial_input::SpatialClockPolicy::Unknown
    );
    let snapshot = app.workspace_snapshot().unwrap();
    let file::ViewKind::Spatial {
        clock_expectation, ..
    } = &snapshot.views[0].kind
    else {
        panic!()
    };
    assert_eq!(*clock_expectation, file::ClockExpectation::System);
    let encoded = String::from_utf8(snapshot.encode().unwrap()).unwrap();
    assert!(!encoded.contains("verified"));
}

#[test]
fn new_views_render_before_layout_reconciliation_and_keep_saved_order() {
    let (mut app, outgoing) = connected();
    let _ = add(&mut app, &outgoing, "/one");
    let _ = add(&mut app, &outgoing, "/two");
    assert_eq!(app.workspace_ordered_ids(), vec![ViewId(1), ViewId(2)]);
    app.workspace.layout.order = vec![app.views[1].workspace_id.clone()];
    app.workspace.layout.scene_percent = 75;
    assert_eq!(app.workspace_ordered_ids(), vec![ViewId(2), ViewId(1)]);
    app.workspace.open = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 1200.),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let tree = output.platform_output.accesskit_update.unwrap();
    for expected in [
        "Details: /one · Messages",
        "Details: /two · Messages",
        "Restart file worker",
    ] {
        assert!(
            tree.nodes
                .iter()
                .any(|(_, node)| node.label() == Some(expected) || node.value() == Some(expected)),
            "Missing actual rendered control/card: {expected}"
        );
    }
}

#[test]
fn stopped_displays_are_not_painted_but_configuration_survives() {
    let (mut app, outgoing) = connected();
    let start = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&start));
    let before = app.workspace_snapshot().unwrap();
    app.set_display_enabled(app.views[0].id, false);
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, stopped(&stop));
    assert!(!app.views[0].enabled);
    assert_eq!(app.workspace_snapshot().unwrap().views, before.views);
    app.sidebar = false;
    app.inspector = false;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run(egui::RawInput::default(), |ctx| app.render(ctx));
    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
    assert!(!nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Details: /one · Messages")));
    assert!(nodes.iter().any(|(_, n)| n
        .value()
        .or_else(|| n.label())
        .is_some_and(|t| t.contains("Displays are ready"))));
}

#[test]
fn definitive_unacknowledged_start_failure_releases_restore_owner_without_auto_start() {
    let (mut app, outgoing) = connected();
    let start = add(&mut app, &outgoing, "/one");
    app.workspace_review_configuration(file::Preset::Analyze.workspace())
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    let _stop = outgoing.try_recv().unwrap();
    let (generation, request, subscription) = start.ids();
    ingest(
        &mut app,
        Event::RequestError {
            generation,
            request,
            subscription,
            message: "native start failed before handle creation".into(),
            no_handle: true,
        },
    );
    poll_workspace(&mut app);
    assert!(!app.workspace_frozen());
    assert_eq!(app.views.len(), 2);
    assert!(app.views.iter().all(|view| view.subscription.is_none()));
    assert!(outgoing.try_recv().is_err());
}
