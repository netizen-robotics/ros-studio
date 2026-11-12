use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};
use studio_protocol::spatial_input::SpatialClockPolicy;
#[path = "camera_tests.rs"]
mod camera_tests;
#[path = "control_tests.rs"]
mod control_tests;
#[path = "map_tests.rs"]
mod map_tests;
#[path = "marker_tests.rs"]
mod marker_tests;
#[path = "sensor_tests.rs"]
mod sensor_tests;
#[path = "uncertainty_tests.rs"]
mod uncertainty_tests;

pub(super) fn ingest(app: &mut App, event: Event) {
    app.session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .ingest_for_test(event)
        .unwrap();
    app.poll_backend();
    app.poll_scene_settings();
}
pub(super) fn connected() -> (App, Receiver<Command>) {
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
                ("/scan".into(), vec![spatial::SCAN_MESSAGE_TYPE.into()]),
                ("/camera".into(), vec![images::IMAGE_MESSAGE_TYPE.into()]),
                (
                    "/camera_two".into(),
                    vec![images::IMAGE_MESSAGE_TYPE.into()],
                ),
                ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
            ],
        },
    );
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    (app, outgoing)
}
fn add(app: &mut App, output: Output, topic: &str, ty: &str) -> ViewId {
    app.begin_source(output, topic, ty);
    app.subscribe();
    app.selected.unwrap()
}
pub(super) fn started(command: &Command) -> Event {
    let Command::Start {
        generation,
        request,
        subscription,
        ..
    } = command
    else {
        panic!("Expected Start")
    };
    Event::Started {
        generation: *generation,
        request: *request,
        subscription: *subscription,
    }
}
pub(super) fn stopped(command: &Command) -> Event {
    let Command::Stop {
        generation,
        request,
        subscription,
    } = command
    else {
        panic!("Expected Stop")
    };
    Event::Stopped {
        generation: *generation,
        request: *request,
        subscription: *subscription,
    }
}
#[test]
fn catalog_and_add_are_inactive_then_only_enable_sends_start() {
    let (mut app, outgoing) = connected();
    app.begin_add();
    app.catalog_query = "does not hide configured displays".into();
    let image = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    assert!(!app.views[0].enabled);
    assert!(app.expanded_displays.contains(&app.views[0].workspace_id));
    assert!(app.topics_tab);
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.set_display_enabled(image, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    assert!(app.views[0].enabled);
    assert_eq!(app.views[0].status, ViewStatus::Waiting);
    app.discovery.query = "no matching topics".into();
    app.discovery.topics.clear();
    assert_eq!(app.workspace_ordered_ids(), vec![image]);
    assert!(app.views[0].enabled);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn draft_apply_and_discard_do_not_enable_disabled_displays_or_change_peers() {
    let (mut app, outgoing) = connected();
    let first = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    let peer = add(
        &mut app,
        Output::CompactImage,
        "/camera_two",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.configure_view(first);
    app.discovery.topic = "/camera_two".into();
    app.draft_best_effort = true;
    app.select_display(peer);
    assert_eq!(app.selected, Some(first));
    assert_eq!(app.views[0].configuration.topic, "/camera");
    app.setup = false; // Discard action: root configuration is untouched.
    app.setup_target = None;
    app.configure_view(first);
    assert_eq!(app.discovery.topic, "/camera");
    app.discovery.topic = "/camera_two".into();
    app.subscribe();
    assert_eq!(app.views[0].configuration.topic, "/camera_two");
    assert!(!app.views[0].enabled);
    assert!(!app.views[1].enabled);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    let before = app.workspace_snapshot().unwrap();
    app.workspace
        .layout
        .reorder(&before.views[1].id, 0)
        .unwrap();
    assert_eq!(app.workspace_ordered_ids(), vec![peer, first]);
}
#[test]
fn scene_apply_waits_for_exact_cleanup_resumes_enabled_and_preserves_image() {
    let (mut app, outgoing) = connected();
    let scan = add(
        &mut app,
        Output::SpatialScan,
        "/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    let image = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.set_display_enabled(scan, true);
    let scan_start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan_start));
    app.set_display_enabled(image, true);
    let image_start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&image_start));
    app.scene_settings.draft.fixed_frame = "new_world".into();
    app.scene_settings.draft_verified = false;
    app.apply_scene_settings().unwrap();
    assert!(app.scene_settings.pending.is_some());
    let stop = outgoing.try_recv().unwrap();
    assert!(app.workspace_frozen());
    assert_eq!(app.scene_settings.configuration.fixed_frame, "world");
    assert!(app.views[1].enabled);
    app.set_display_enabled(scan, true);
    ingest(&mut app, started(&scan_start)); // A late Started cannot undo Stop.
    assert_eq!(app.views[0].status, ViewStatus::Stopping);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    ingest(&mut app, stopped(&stop));
    assert!(!app.workspace_frozen());
    assert_eq!(app.scene_settings.configuration.fixed_frame, "new_world");
    assert_eq!(
        app.views[0]
            .configuration
            .spatial
            .as_ref()
            .unwrap()
            .fixed_frame,
        "new_world"
    );
    assert!(app.views[0].enabled);
    assert!(app.views[0].spatial_clock_verified);
    assert!(app.views[1].enabled);
    let restart = outgoing.try_recv().unwrap();
    assert!(matches!(restart, Command::Start { .. }));
    assert_ne!(restart.ids().2, scan_start.ids().2);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn cancel_disconnect_and_uncertain_cleanup_cannot_install_scene_drafts() {
    for mode in ["cancel", "disconnect", "uncertain"] {
        let (mut app, outgoing) = connected();
        let scan = add(
            &mut app,
            Output::SpatialScan,
            "/scan",
            spatial::SCAN_MESSAGE_TYPE,
        );
        app.set_display_enabled(scan, true);
        let start = outgoing.try_recv().unwrap();
        ingest(&mut app, started(&start));
        app.scene_settings.draft.fixed_frame = "never_installed".into();
        app.apply_scene_settings().unwrap();
        let stop = outgoing.try_recv().unwrap();
        match mode {
            "cancel" => {
                app.scene_settings.pending = None;
                ingest(&mut app, stopped(&stop));
            }
            "disconnect" => app.stop(),
            _ => {
                app.views[0].status = ViewStatus::NeedsRecovery;
                app.poll_scene_settings();
            }
        }
        assert!(app.scene_settings.pending.is_none());
        assert_eq!(app.scene_settings.configuration.fixed_frame, "world");
        assert!(!app.views[0].enabled);
        assert!(outgoing.try_recv().is_err());
    }
}
#[test]
fn scene_replacement_adopts_an_existing_stop_without_changing_correlation() {
    let (mut app, outgoing) = connected();
    let scan = add(
        &mut app,
        Output::SpatialScan,
        "/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    app.set_display_enabled(scan, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    app.set_display_enabled(scan, false);
    let stop = outgoing.try_recv().unwrap();
    let request = app.views[0].request;
    app.scene_settings.draft.fixed_frame = "after_cleanup".into();
    app.apply_scene_settings().unwrap();
    assert_eq!(app.views[0].request, request);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    ingest(&mut app, stopped(&stop));
    assert!(app.scene_settings.pending.is_none());
    assert_eq!(
        app.scene_settings.configuration.fixed_frame,
        "after_cleanup"
    );
    assert!(!app.views[0].enabled);
}
#[test]
fn imported_mixed_scene_keys_require_explicit_consolidation_and_fresh_clock() {
    let (mut app, outgoing) = connected();
    let scan = add(
        &mut app,
        Output::SpatialScan,
        "/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    let mut other = app.views[0].configuration.clone();
    other.spatial.as_mut().unwrap().fixed_frame = "different".into();
    app.next_view += 1;
    app.views.push(ViewState::new(ViewId(app.next_view), other));
    app.restore_scene_settings();
    assert!(app.scene_settings.verified);
    assert_eq!(
        app.scene_settings.configuration.clock_policy,
        SpatialClockPolicy::VerifiedSystem
    );
    assert!(!app.scene_matches(&app.views[1].configuration));
    app.set_display_enabled(ViewId(app.next_view), true);
    assert!(!app.scene_settings.open);
    assert!(!app.setup);
    assert!(app.views[1]
        .error
        .as_deref()
        .unwrap()
        .contains("Scene settings"));
    assert!(app.views[1].subscription.is_none());
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    assert!(app
        .views
        .iter()
        .all(|v| app.scene_matches(&v.configuration) && !v.enabled));
    assert!(app.views.iter().all(|v| v.subscription.is_none()));
    assert_eq!(app.workspace_ordered_ids()[0], scan);
}
#[test]
fn headless_narrow_theme_catalog_and_scene_states_keep_named_controls() {
    let ctx = egui::Context::default();
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.set_theme(theme);
        let (mut app, _) = connected();
        for state in ["catalog", "scene", "empty"] {
            app.catalog_open = state == "catalog";
            app.scene_settings.open = state == "scene";
            app.global_options_open = state == "scene";
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760., 560.),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            assert!(app.views.is_empty());
        }
    }
}

#[test]
fn workspace_restore_keeps_scene_and_camera_without_live_displays_or_clock_permission() {
    let mut app = App::default();
    app.scene_settings.configuration.fixed_frame = "empty_world".into();
    app.shared_scene.camera.mode = spatial::CameraMode::TopDown;
    app.shared_scene.camera.target = [123.456, -789.012, 0.];
    app.shared_scene.camera.vertical_span = 25.;
    app.shared_scene
        .saved_views
        .push(studio_workspace::presentation::SavedView {
            name: "Bench".into(),
            camera: app.shared_scene.camera.snapshot(),
        });
    let snapshot = app.workspace_snapshot().unwrap();
    assert!(snapshot.views.is_empty());
    let bytes = studio_workspace::application::encode(
        &snapshot,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    app.shared_scene.camera.reset();
    app.scene_settings.configuration.fixed_frame.clear();
    app.workspace_review_configuration(imported.workspace)
        .unwrap();
    app.workspace_begin_apply(crate::components::workspace::ApplyApproval::DiscardUnsaved)
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    assert_eq!(app.workspace_snapshot().unwrap(), snapshot);
    assert!(app.views.is_empty());
    assert!(app.session.is_none());
    assert!(app.scene_settings.verified);
    assert!(app.scene_settings.draft_verified);
    assert_eq!(
        app.scene_settings.configuration.clock_policy,
        SpatialClockPolicy::Unknown
    );
    assert_eq!(app.shared_scene.camera.mode, spatial::CameraMode::TopDown);
}

fn tf_sample(
    command: &Command,
    sequence: u64,
    status: studio_protocol::latest_pose::LatestPose,
) -> Event {
    let Command::Start {
        generation,
        subscription,
        ..
    } = command
    else {
        panic!("Start required")
    };
    Event::TfGraphObservation {
        generation: *generation,
        subscription: *subscription,
        sequence,
        backend_drops: 0,
        sample: Box::new(studio_protocol::tf_graph::Sample {
            contract_version: 1,
            fixed_frame: "world".into(),
            frames: vec![
                studio_protocol::tf_graph::Frame {
                    name: "base".into(),
                    parent: Some(studio_protocol::tf_graph::Parent {
                        frame: "world".into(),
                        kind: studio_protocol::tf_graph::EdgeKind::Dynamic,
                        stamp_ns: Some(1),
                    }),
                    pose: status,
                },
                studio_protocol::tf_graph::Frame {
                    name: "world".into(),
                    parent: None,
                    pose: studio_protocol::latest_pose::LatestPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                },
            ],
            counters: Default::default(),
        }),
    }
}
#[test]
fn tf_add_enable_selection_stale_stop_and_inactive_restore_keep_ownership_explicit() {
    use studio_protocol::latest_pose::LatestPose;
    let (mut app, outgoing) = connected();
    let id = add(&mut app, Output::TfGraph, "/tf", spatial::TF_MESSAGE_TYPE);
    assert!(matches!(app.views[0].kind, ViewKind::TfGraph));
    assert!(!app.views[0].enabled);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.set_display_enabled(id, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    ingest(
        &mut app,
        tf_sample(
            &start,
            1,
            LatestPose::Ready {
                resolved_stamp_ns: 1,
                components: [1., 2., 3., 0., 0., 0., 1.],
            },
        ),
    );
    assert_eq!(app.views[0].tf_graph.ready_count(), 2);
    assert!(app.views[0].tf_graph.select("base"));
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("resolved_stamp"));
    ingest(&mut app, tf_sample(&start, 2, LatestPose::Stale));
    assert!(app.views[0].tf_graph.bounds(true).is_none());
    app.set_display_enabled(id, false);
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[0].tf_graph.sample().is_none());
    ingest(&mut app, tf_sample(&start, 3, LatestPose::Missing));
    assert!(app.views[0].tf_graph.sample().is_none());
    ingest(&mut app, stopped(&stop));
    let prepared = app.workspace_prepare(&saved).unwrap();
    assert!(!prepared.views[0].enabled);
    assert!(prepared.views[0].tf_graph.sample().is_none());
    assert!(!prepared.views[0].spatial_clock_verified);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn tf_scene_source_replacement_updates_outer_identity_and_resumes_enabled() {
    let (mut app, outgoing) = connected();
    let id = add(&mut app, Output::TfGraph, "/tf", spatial::TF_MESSAGE_TYPE);
    app.set_display_enabled(id, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    app.scene_settings.draft.dynamic_tf_topic = "/new_tf".into();
    app.scene_settings.draft.dynamic_tf_best_effort = true;
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, stopped(&stop));
    let config = &app.views[0].configuration;
    assert_eq!(config.topic, "/new_tf");
    assert!(config.best_effort);
    assert!(config.valid());
    assert!(app.views[0].enabled);
    assert!(matches!(
        outgoing.try_recv().unwrap(),
        Command::Start { .. }
    ));
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn permanent_scene_survives_empty_image_only_disabled_and_disconnected_states() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let (mut app, outgoing) = connected();
    let render = |app: &mut App| {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 800.),
                )),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
        let update = output.platform_output.accesskit_update.unwrap();
        assert!(update
            .nodes
            .iter()
            .any(|(_, n)| n.label().is_some_and(|s| s.contains("Combined 3D scene"))));
        assert!(app.shared_scene.marker_request.is_some());
    };
    render(&mut app);
    let id = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    render(&mut app);
    app.companion_open = false;
    render(&mut app);
    app.set_display_enabled(id, true);
    let start = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&start));
    render(&mut app);
    app.stop();
    render(&mut app);
    assert!(!app.views[0].enabled);
}

#[test]
#[ignore = "Requires native Metal GPU; serialized with native/load acceptance"]
fn empty_scene_reuses_gpu_texture_between_app_frames() {
    let gpu = crate::performance_support::gpu();
    let mut app = App::default();
    app.spatial_renderer = Some(spatial::Renderer3d::with_sensor_history(gpu.clone()));
    let surface = crate::performance_support::Surface::new(gpu);
    for _ in 0..3 {
        surface.frame_with_size(&mut app, [1280, 800]);
    }
    let texture = app
        .spatial_renderer
        .as_ref()
        .unwrap()
        .color_texture()
        .unwrap()
        .clone();
    for _ in 0..5 {
        surface.frame_with_size(&mut app, [1280, 800]);
        assert_eq!(
            &texture,
            app.spatial_renderer
                .as_ref()
                .unwrap()
                .color_texture()
                .unwrap()
        );
    }
}

#[test]
fn topics_expose_multiple_choices_and_unsupported_context_without_commands() {
    let (mut app, outgoing) = connected();
    app.global_options_open = false;
    app.discovery.topics.push((
        "/camera_info".into(),
        vec!["sensor_msgs/msg/CameraInfo".into()],
    ));
    app.discovery
        .topics
        .push(("/custom".into(), vec!["custom_msgs/msg/Unknown".into()]));
    app.topics_tab = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    for show_all in [false, true] {
        app.show_all_topics = show_all;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440., 900.),
                )),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        let labels: Vec<_> = tree
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().or_else(|| n.value()))
            .collect();
        assert!(!labels.contains(&"In view"), "{show_all}: {labels:?}");
        assert!(labels.contains(&"Refresh"));
        assert!(labels.contains(&"Available topics"));
        assert!(labels.contains(&"Show /camera"));
        assert!(!labels.contains(&"Resource capacity"));
    }
    for (query, expected) in [
        (
            "/custom",
            "No visual display · Messages are available in Tools",
        ),
        ("/camera_info", "Calibration input · CameraInfo"),
    ] {
        app.topic_query = query.into();
        let mut found = false;
        let mut labels = Vec::new();
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440., 900.),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            let tree = output.platform_output.accesskit_update.unwrap();
            labels = tree
                .nodes
                .iter()
                .filter_map(|(_, n)| n.label().or_else(|| n.value()).map(str::to_owned))
                .collect();
            found |= labels.iter().any(|label| label == expected);
        }
        assert!(found, "{query} expected {expected}: {labels:?}");
    }
    assert!(app.views.is_empty());
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.begin_source(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
    app.begin_source(
        Output::CompactImage,
        "/camera_two",
        images::IMAGE_MESSAGE_TYPE,
    );
    assert_eq!(app.discovery.topic, "/camera");
    app.open_scene_settings();
    assert!(app.setup);
    app.scene_settings.open = false;
    app.subscribe();
    assert_eq!(app.views.len(), 1);
    assert!(!app.views[0].enabled);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn permanent_viewport_keeps_minimum_width_and_companions_inside_short_windows() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for size in [[800., 600.], [1280., 800.], [1440., 900.]] {
            let (mut app, outgoing) = connected();
            add(
                &mut app,
                Output::CompactImage,
                "/camera",
                images::IMAGE_MESSAGE_TYPE,
            );
            let ctx = egui::Context::default();
            shell::configure(&ctx);
            ctx.set_theme(theme);
            ctx.enable_accesskit();
            for _ in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0], size[1]),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.render(ctx),
                );
                let tree = output.platform_output.accesskit_update.unwrap();
                let node = tree
                    .nodes
                    .iter()
                    .find(|(_, n)| {
                        n.label()
                            .is_some_and(|s| s.starts_with("Combined 3D scene"))
                    })
                    .unwrap();
                let bounds = node.1.bounds().unwrap();
                assert!(bounds.x1 - bounds.x0 >= 320., "{size:?} {bounds:?}");
                assert!(bounds.y1 <= f64::from(size[1]), "{size:?} {bounds:?}");
                assert!(bounds.y1 - bounds.y0 > 100., "{size:?} {bounds:?}");
            }
            assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

#[test]
fn image_fit_uses_bounded_companion_without_resizing_the_scene() {
    for size in [[760., 560.], [800., 600.], [1280., 800.]] {
        let (mut app, outgoing) = connected();
        let id = add(
            &mut app,
            Output::CompactImage,
            "/camera",
            images::IMAGE_MESSAGE_TYPE,
        );
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        shell::configure(&ctx);
        let render = |app: &mut App| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(size[0], size[1]),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            )
        };
        render(&mut app);
        render(&mut app);
        let before = render(&mut app);
        let scene_rect = |out: &egui::FullOutput| {
            out.platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|(_, n)| {
                    n.label()
                        .is_some_and(|s| s.starts_with("Combined 3D scene"))
                })
                .unwrap()
                .1
                .bounds()
                .unwrap()
        };
        let initial = scene_rect(&before);
        app.set_display_enabled(id, true);
        let start = outgoing.try_recv().unwrap();
        ingest(&mut app, started(&start));
        let view = &mut app.views[0];
        let token = view.image.start();
        let pixels = vec![255; 64 * 48 * 4];
        let decoded = images::decode(images::RawFrame {
            big_endian: false,
            width: 64,
            height: 48,
            step: 256,
            encoding: "rgba8",
            data: &pixels,
        })
        .unwrap();
        assert!(view.image.frames().publish(token, decoded));
        render(&mut app);
        let output = render(&mut app);
        let tree = output.platform_output.accesskit_update.as_ref().unwrap();
        let image = tree
            .nodes
            .iter()
            .find(|(_, n)| n.role() == egui::accesskit::Role::Image)
            .unwrap()
            .1
            .bounds()
            .unwrap();
        // Platform font metrics can shift this bounded dock by a fraction of a
        // logical pixel. Keep the minimum-size regression with one pixel of
        // layout tolerance, rather than requiring identical glyph geometry.
        assert!(
            image.y1 - image.y0 + 1.0 >= if size[1] < 700. { 32. } else { 48. },
            "Image Fit must use its viewport: {image:?}"
        );
        assert_eq!(scene_rect(&output), initial);
        assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    }
}

#[test]
fn workspace18_restores_shell_selection_by_stable_id_without_reception() {
    let (mut app, outgoing) = connected();
    let first = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    let second = add(
        &mut app,
        Output::CompactImage,
        "/camera_two",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.topics_tab = true;
    app.global_options_open = true;
    app.companion_percent = 35;
    app.companion_selected = Some(second);
    let snapshot = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &snapshot,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert!(!imported.legacy);
    app.workspace_review_configuration(imported.workspace)
        .unwrap();
    app.workspace_begin_apply(crate::components::workspace::ApplyApproval::DiscardUnsaved)
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    assert!(app.topics_tab && app.global_options_open && app.companion_open);
    assert_eq!(app.companion_percent, 35);
    let selected = app
        .views
        .iter()
        .find(|v| Some(v.id) == app.companion_selected)
        .unwrap();
    assert_eq!(selected.configuration.topic, "/camera_two");
    assert_ne!(selected.id, first);
    assert_ne!(selected.id, second);
    assert_eq!(app.expanded_displays.len(), 2);
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    assert!(app.scene_settings.verified);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn rendered_workspace_restore_and_companion_reopen_honor_saved_allocation() {
    let (mut app, _) = connected();
    add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    let ctx = egui::Context::default();
    let render = |app: &mut App| {
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280., 800.),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
        }
    };
    render(&mut app);
    let mut saved = app.presentation_snapshot();
    saved.shell.companion_percent = 35;
    app.restore_presentation(Some(&saved)).unwrap();
    render(&mut app);
    assert!(
        app.companion_percent.abs_diff(35) <= 1,
        "Restored height ignored: {}",
        app.companion_percent
    );
    app.companion_open = false;
    render(&mut app);
    assert!(app.companion_percent.abs_diff(35) <= 1);
    app.companion_open = true;
    render(&mut app);
    assert!(
        app.companion_percent.abs_diff(35) <= 1,
        "Reopen lost height: {}",
        app.companion_percent
    );
    saved.shell.companion_percent = 20;
    app.restore_presentation(Some(&saved)).unwrap();
    render(&mut app);
    assert!(
        app.companion_percent.abs_diff(20) <= 1,
        "Prior panel cache overrode second restore: {}",
        app.companion_percent
    );
}

#[test]
fn scene_apply_keeps_persistent_section_expanded_after_success() {
    let (mut app, outgoing) = connected();
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "reviewed_world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    assert!(app.scene_settings.open);
    assert!(app.global_options_open);
    assert!(app.scene_settings.verified);
    assert_eq!(
        app.scene_settings.configuration.fixed_frame,
        "reviewed_world"
    );
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn show_is_one_action_with_no_draft_and_duplicate_focus_has_no_command() {
    let (mut app, outgoing) = connected();
    app.discovery
        .topics
        .retain(|(name, _)| name != "/tf_static");
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    assert_eq!(app.views.len(), 1);
    assert!(app.views[0].enabled);
    assert!(!app.setup);
    assert!(!app.scene_settings.open);
    let start = outgoing.try_recv().unwrap();
    assert!(matches!(start, Command::Start { .. }));
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.selected, Some(app.views[0].id));
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    ingest(&mut app, started(&start));
    app.set_display_enabled(app.views[0].id, false);
    assert!(matches!(outgoing.try_recv().unwrap(), Command::Stop { .. }));
    assert!(!app.setup);
    assert!(!app.scene_settings.open);
}
#[test]
fn show_with_default_scene_context_starts_scan_without_forms() {
    let (mut app, outgoing) = connected();
    app.scene_settings = Default::default();
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    assert!(app.views[0].enabled);
    assert!(!app.setup && !app.scene_settings.open);
    assert!(app.topic_action_error.is_none());
    let Command::Start { configuration, .. } = outgoing.try_recv().unwrap() else {
        panic!("Expected one Start");
    };
    assert_eq!(configuration.spatial.unwrap().fixed_frame, "map");
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn duplicate_show_clears_previous_error_and_opens_only_existing_properties() {
    let (mut app, outgoing) = connected();
    app.show_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
    let _ = outgoing.try_recv().unwrap();
    let stable = app.views[0].workspace_id.clone();
    app.expanded_displays.remove(&stable);
    app.topic_action_error = Some("Previous unrelated failure".into());
    app.show_topic(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
    assert_eq!(app.views.len(), 1);
    assert!(app.topic_action_error.is_none());
    assert!(app.expanded_displays.contains(&stable));
    assert!(!app.setup && !app.scene_settings.open);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn existing_robot_description_focus_does_not_open_another_capture() {
    let (mut app, outgoing) = connected();
    let id = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    let view = &mut app.views[0];
    view.configuration.output = Output::Robot;
    view.configuration.topic = "/tf".into();
    view.kind = ViewKind::Robot;
    view.robot_description = Some(crate::components::workspace::RobotTopicSource {
        topic: "/robot_description".into(),
        ..Default::default()
    });
    app.show_topic(
        Output::Robot,
        "/robot_description",
        studio_protocol::robot_description::MESSAGE_TYPE,
    );
    assert_eq!(app.selected, Some(id));
    assert_eq!(app.views.len(), 1);
    assert!(!app.robot_setup.is_open());
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn robot_topic_capacity_failure_has_a_remedy_without_starting_capture() {
    let (mut app, outgoing) = connected();
    for i in 0..MAX_SUBSCRIPTIONS {
        app.discovery.topics.push((
            format!("/image{i}"),
            vec![images::IMAGE_MESSAGE_TYPE.into()],
        ));
        add(
            &mut app,
            Output::CompactImage,
            &format!("/image{i}"),
            images::IMAGE_MESSAGE_TYPE,
        );
    }
    app.show_topic(
        Output::Robot,
        "/robot_description",
        studio_protocol::robot_description::MESSAGE_TYPE,
    );
    assert!(app
        .topic_action_error
        .as_deref()
        .unwrap()
        .contains("Remove a stopped layer"));
    assert!(!app.robot_setup.is_open());
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn default_map_system_show_and_reconnect_require_no_scene_reapply() {
    let (mut app, outgoing) = connected();
    app.scene_settings = Default::default();
    assert_eq!(app.scene_settings.configuration.fixed_frame, "map");
    assert!(app.scene_settings.verified);
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    let start = outgoing.try_recv().unwrap();
    let Command::Start { configuration, .. } = &start else {
        panic!("one Show must start");
    };
    assert!(configuration.best_effort);
    assert_eq!(configuration.spatial.as_ref().unwrap().fixed_frame, "map");
    assert!(!app.setup && !app.scene_settings.open);
    ingest(&mut app, started(&start));
    app.open_scene_settings();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let rendered = ctx.run(egui::RawInput::default(), |ctx| app.render(ctx));
    let tree = rendered.platform_output.accesskit_update.unwrap();
    assert!(!tree.nodes.iter().any(|(_, n)| n.label() == Some("Done")));
    assert!(!tree
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Apply scene settings")));
    app.apply_scene_settings().unwrap();
    assert!(app.views[0].enabled);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.set_display_enabled(app.views[0].id, false);
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, stopped(&stop));
    app.scene_settings.configuration.fixed_frame = "odom".into();
    app.scene_settings.disconnect();
    app.connection.phase = Phase::Starting;
    app.handle_at(
        Event::ready("jazzy".into(), "fixture".into()),
        Instant::now(),
    );
    assert_eq!(app.scene_settings.configuration.fixed_frame, "odom");
    assert!(app.scene_settings.verified);
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}
#[test]
fn observed_frames_are_coalesced_generation_bound_and_do_not_subscribe() {
    let (mut app, outgoing) = connected();
    let generation = app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .registry
        .generation();
    ingest(
        &mut app,
        Event::SceneFrames {
            generation,
            frames: vec!["laser".into(), "map".into(), "odom".into()],
        },
    );
    assert_eq!(app.observed_tf_frames, vec!["laser", "map", "odom"]);
    ingest(
        &mut app,
        Event::SceneFrames {
            generation: Generation(generation.0 + 1),
            frames: vec!["wrong".into()],
        },
    );
    assert!(!app.observed_tf_frames.contains(&"wrong".into()));
    app.open_scene_settings();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let out = ctx.run(egui::RawInput::default(), |ctx| app.render(ctx));
    let tree = out.platform_output.accesskit_update.unwrap();
    assert!(!tree
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("I verified all scene publishers use system time")));
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn connected_workspace_import_stays_inactive_but_explicit_enable_needs_no_clock_apply() {
    let (mut app, outgoing) = connected();
    add(
        &mut app,
        Output::SpatialScan,
        "/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    let saved = app.workspace_snapshot().unwrap();
    app.workspace_review_configuration(saved).unwrap();
    app.workspace_begin_apply(crate::components::workspace::ApplyApproval::DiscardUnsaved)
        .unwrap();
    app.workspace_poll(&egui::Context::default());
    assert!(app.scene_settings.verified);
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    assert_eq!(outgoing.try_recv(), Err(TryRecvError::Empty));
    app.set_display_enabled(app.views[0].id, true);
    assert!(matches!(
        outgoing.try_recv().unwrap(),
        Command::Start { .. }
    ));
    assert!(!app.setup && !app.scene_settings.open);
}

#[test]
fn unchanged_offline_defaults_need_no_apply_or_done_action() {
    let mut app = App::default();
    app.scene_settings.disconnect();
    app.open_scene_settings();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let out = ctx.run(egui::RawInput::default(), |ctx| app.render(ctx));
    let tree = out.platform_output.accesskit_update.unwrap();
    assert!(!tree.nodes.iter().any(|(_, n)| n.label() == Some("Done")));
    assert!(!tree
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Apply scene settings")));
    assert_eq!(app.scene_settings.configuration.fixed_frame, "map");
    assert!(app.session.is_none() && app.views.is_empty());
}

#[test]
fn capacity32_saves_restores_inactive_rejects33_and_stops_every_owner_in_one_burst() {
    let (mut app, outgoing) = connected();
    let mut ids = Vec::new();
    for index in 0..32 {
        let topic = format!("/images/image{index}");
        app.discovery
            .topics
            .push((topic.clone(), vec![images::IMAGE_MESSAGE_TYPE.into()]));
        let id = add(
            &mut app,
            Output::CompactImage,
            &topic,
            images::IMAGE_MESSAGE_TYPE,
        );
        app.set_display_enabled(id, true);
        let start = outgoing.try_recv().unwrap();
        ingest(&mut app, started(&start));
        ids.push(id);
    }
    assert_eq!(app.views.len(), 32);
    let owners: Vec<_> = app.views.iter().map(|v| v.subscription).collect();
    app.discovery.topics.push((
        "/images/excess".into(),
        vec![images::IMAGE_MESSAGE_TYPE.into()],
    ));
    app.show_topic(
        Output::CompactImage,
        "/images/excess",
        images::IMAGE_MESSAGE_TYPE,
    );
    assert_eq!(app.views.len(), 32);
    assert!(app
        .topic_action_error
        .as_deref()
        .unwrap()
        .contains("Remove a stopped layer"));
    assert_eq!(
        app.views.iter().map(|v| v.subscription).collect::<Vec<_>>(),
        owners
    );
    assert!(outgoing.try_recv().is_err());
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let parsed = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&parsed.workspace).unwrap();
    assert_eq!(prepared.views.len(), 32);
    assert!(prepared
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    for id in ids {
        app.set_display_enabled(id, false);
    }
    let stops: Vec<_> = outgoing.try_iter().collect();
    assert_eq!(
        stops.len(),
        32,
        "Burst shutdown must not overflow command channel"
    );
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
        32
    );
    for stop in stops {
        ingest(&mut app, stopped(&stop));
    }
    assert!(app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .registry
        .entries()
        .is_empty());
}
