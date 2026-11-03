//! Regression coverage for passive scene disclosure and independent browser drafts.
use super::*;

#[test]
fn backend_identity_distinguishes_demo_custom_and_ros_without_claiming_hardware() {
    use crate::panels::backend_identity;
    assert_eq!(
        backend_identity(Some(std::ffi::OsStr::new("/tmp/ui-workspace-demo")), None).0,
        "Demo · synthetic"
    );
    assert_eq!(
        backend_identity(Some(std::ffi::OsStr::new("/tmp/other-backend")), None).0,
        "Custom backend"
    );
    assert_eq!(backend_identity(None, None).0, "ROS 2");
    assert_eq!(
        backend_identity(
            None,
            Some(&("jazzy".into(), "synthetic-fixture-no-ROS".into()))
        )
        .0,
        "Demo · synthetic"
    );
    let actual = ("jazzy".into(), "rmw_cyclonedds_cpp".into());
    assert!(backend_identity(None, Some(&actual))
        .1
        .contains("robots or simulators"));
}

#[test]
fn selected_topic_waiting_feedback_fits_without_a_pinned_review_block() {
    let (mut app, commands) = connected();
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    let start = commands.try_recv().unwrap();
    ingest(&mut app, started(&start));
    app.views[0].status = ViewStatus::Waiting;
    app.views[0].spatial_status = "Waiting for acquisition-time transform".into();
    app.global_options_open = false;
    app.topics_tab = true;
    app.topic_query = "/scan".into();
    app.compact_inspector_open = false;
    let ctx = egui::Context::default();
    fonts::configure(&ctx);
    shell::configure(&ctx);
    ctx.enable_accesskit();
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800., 600.),
        )),
        ..Default::default()
    };
    let mut nodes = std::collections::HashMap::new();
    for _ in 0..4 {
        let output = ctx.run(input(), |ctx| app.render(ctx));
        for (id, node) in output.platform_output.accesskit_update.unwrap().nodes {
            nodes.insert(id, node);
        }
    }
    let feedback = nodes
        .values()
        .find(|n| n.label().or_else(|| n.value()) == Some("Waiting for acquisition-time transform"))
        .unwrap_or_else(|| {
            panic!(
                "missing feedback: {:?}",
                nodes.values().filter_map(|n| n.label()).collect::<Vec<_>>()
            )
        })
        .bounds()
        .unwrap();
    assert!(feedback.y1 < 520.0, "feedback is clipped: {feedback:?}");
    assert!(!nodes
        .values()
        .any(|n| n.label() == Some("Review current issues")));
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn robot_source_type_describes_description_and_file_rather_than_tf_input() {
    let (mut app, _) = connected();
    let id = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    let view = app.views.iter_mut().find(|v| v.id == id).unwrap();
    view.kind = ViewKind::Robot;
    view.configuration.message_type = spatial::TF_MESSAGE_TYPE.into();
    assert_eq!(crate::panels::source_type_label(view), "Local URDF file");
    let source = crate::components::workspace::RobotTopicSource {
        topic: "/robot_description".into(),
        ..Default::default()
    };
    let expected = source.message_type.clone();
    view.robot_description = Some(source);
    assert_eq!(crate::panels::source_type_label(view), expected);
    assert_ne!(
        crate::panels::source_type_label(view),
        spatial::TF_MESSAGE_TYPE
    );
    view.robot_appearance_view.error = Some("Invalid local appearance".into());
    assert!(app
        .current_attention()
        .iter()
        .any(|i| i.message.contains("Invalid local appearance")));
}

#[test]
fn compact_inspector_and_left_scene_tab_are_independent() {
    let (mut app, commands) = connected();
    app.compact_inspector_open = true;
    app.details_open = true;
    app.global_options_open = false;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800., 600.),
        )),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx.run(input(), |ctx| app.render(ctx));
    }
    let output = ctx.run(input(), |ctx| app.render(ctx));
    let tree = output.platform_output.accesskit_update.unwrap();
    assert!(tree
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Display inspector")));
    app.compact_inspector_open = false;
    app.open_scene_settings();
    assert!(app.global_options_open);
    assert!(!app.compact_inspector_open, "Scene stays on the left");
    app.scene_settings.draft.fixed_frame = "draft_frame".into();
    app.global_options_open = false;
    app.topics_tab = true;
    app.open_scene_settings();
    assert_eq!(app.scene_settings.draft.fixed_frame, "draft_frame");
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn views_stays_visible_when_display_content_scrolls_and_legacy_visibility_is_off() {
    for size in [[800., 600.], [1280., 800.]] {
        let (mut app, commands) = connected();
        let id = add(
            &mut app,
            Output::SpatialScan,
            "/scan",
            spatial::SCAN_MESSAGE_TYPE,
        );
        app.setup = false;
        app.inspector = false; // Older workspaces may have hidden Views.
        app.select_display(id);
        app.views[0].error = Some("Long recoverable display error. ".repeat(300));
        let ctx = egui::Context::default();
        fonts::configure(&ctx);
        shell::configure(&ctx);
        ctx.enable_accesskit();
        let mut reference = None;
        for step in 0..6 {
            let events = if step >= 3 {
                vec![
                    egui::Event::PointerMoved(egui::pos2(size[0] - 100., size[1] - 80.)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0., -400.),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                vec![]
            };
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(size[0], size[1]),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            let tree = output.platform_output.accesskit_update.unwrap();
            let fit = tree
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some("Camera…"))
                .expect("Views controls must always be available")
                .1
                .bounds()
                .unwrap();
            assert!(fit.y1 < 350., "Views was pushed down: {fit:?}");
            if size[0] < 1100.0 {
                let inspector = tree
                    .nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some("Display inspector"))
                    .expect("compact inspector is open")
                    .1
                    .bounds()
                    .unwrap();
                assert!(
                    inspector.y0 >= fit.y1,
                    "compact inspector covers Views: {inspector:?} / {fit:?}"
                );
            }
            if step == 2 {
                reference = Some(fit);
            }
            if step > 2 {
                assert_eq!(Some(fit), reference, "scrolling moved Views");
            }
        }
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
    }
}

#[test]
fn flat_browser_virtualizes_large_graph_and_keeps_navigation_reachable() {
    let (mut app, commands) = connected();
    app.global_options_open = false;
    app.topics_tab = true;
    app.discovery.topics = (0..1000)
        .map(|i| {
            (
                format!("/sensor_{i:04}"),
                vec![images::IMAGE_MESSAGE_TYPE.into()],
            )
        })
        .collect();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    for _ in 0..3 {
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 800.),
                )),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        );
        let tree = out.platform_output.accesskit_update.unwrap();
        let nodes: Vec<_> = tree.nodes.iter().map(|(_, n)| n).collect();
        let shows = nodes
            .iter()
            .filter(|n| n.label().is_some_and(|s| s.starts_with("Show /sensor_")))
            .count();
        assert!(
            (1..20).contains(&shows),
            "must render only visible rows, got {shows}"
        );
        assert!(nodes.iter().any(|n| n.label() == Some("Refresh")));
        assert!(nodes.iter().any(|n| n.label() == Some("Scene")));
    }
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn current_attention_ignores_history_but_retains_waiting_and_cleanup_causes() {
    let (mut app, commands) = connected();
    app.diagnostic("Earlier failure already recovered".into());
    assert!(app.current_attention().is_empty());
    let id = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.setup = false;
    let view = app.views.iter_mut().find(|v| v.id == id).unwrap();
    view.status = ViewStatus::Waiting;
    view.spatial_status.clear();
    assert!(app.current_attention()[0]
        .message
        .contains("Waiting for usable data"));
    app.diagnostics.clear();
    assert_eq!(
        app.current_attention().len(),
        1,
        "acknowledging history cannot hide current causes"
    );
    app.views[0].status = ViewStatus::NeedsRecovery;
    app.views[0].error = Some("Confirmed cleanup is required".into());
    assert!(app.current_attention()[0]
        .message
        .contains("Confirmed cleanup"));
    app.views[0].status = ViewStatus::Stopped;
    app.views[0].error = None;
    assert!(app.current_attention().is_empty());
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn persistent_browser_survives_setup_catalog_and_compact_inspection() {
    for size in [[800., 600.], [1280., 800.]] {
        let (mut app, commands) = connected();
        app.discovery.topics.insert(
            0,
            (
                "/aaa_unknown".into(),
                vec!["example_msgs/msg/Unsupported".into()],
            ),
        );
        assert!(!app.show_all_topics);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        for setup in [false, true] {
            if setup {
                app.begin_source(Output::CompactImage, "/camera", images::IMAGE_MESSAGE_TYPE);
            } else {
                app.begin_add();
            }
            for _ in 0..3 {
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0], size[1]),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.render(ctx),
                );
                let tree = out.platform_output.accesskit_update.unwrap();
                let labels: Vec<_> = tree
                    .nodes
                    .iter()
                    .filter_map(|(_, n)| n.label().or_else(|| n.value()))
                    .collect();
                for label in ["Available topics", "Topics", "Back to topics"] {
                    assert!(
                        labels.iter().any(|s| s.contains(label)),
                        "{size:?}: missing {label} in {labels:?}"
                    );
                }
                assert!(
                    !labels.iter().any(|s| s.starts_with("No visual display")),
                    "nonvisual sources are hidden by default"
                );
            }
            app.back_to_topics();
        }
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
    }
}

#[test]
fn scene_draft_survives_selection_review_and_disconnect_until_explicit_discard() {
    let (mut app, commands) = connected();
    let image = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "draft_frame".into();
    app.scene_settings.draft.dynamic_tf_topic = "/custom_tf".into();
    app.global_options_open = false;
    app.select_display(image);
    app.open_scene_settings();
    assert_eq!(app.scene_settings.draft.fixed_frame, "draft_frame");
    assert!(app.global_options_open && app.scene_draft_changed());
    app.stop();
    assert_eq!(app.scene_settings.draft.dynamic_tf_topic, "/custom_tf");
    assert_eq!(app.scene_settings.draft.fixed_frame, "draft_frame");
    app.discard_scene_draft();
    assert_eq!(app.scene_settings.draft.fixed_frame, "world");
    assert!(!app.scene_draft_changed());
    assert!(commands.try_recv().is_err());
}

#[test]
fn add_and_back_preserve_topic_filter_and_selection_without_starting() {
    let (mut app, commands) = connected();
    let image = add(
        &mut app,
        Output::CompactImage,
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.topic_query = "/camera".into();
    app.begin_add();
    app.catalog_query = "Map".into();
    app.back_to_topics();
    assert_eq!(app.topic_query, "/camera");
    assert_eq!(app.selected, Some(image));
    assert!(!app.catalog_open && !app.setup);
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn cancelling_scene_apply_preserves_draft_and_never_undoes_stop() {
    let (mut app, commands) = connected();
    app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
    let start = commands.try_recv().unwrap();
    ingest(&mut app, started(&start));
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "draft_frame".into();
    app.apply_scene_settings().unwrap();
    let stop = commands.try_recv().unwrap();
    app.cancel_scene_application();
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.scene_settings.configuration.fixed_frame, "world");
    assert_eq!(app.scene_settings.draft.fixed_frame, "draft_frame");
    assert!(!app.views[0].enabled && app.views[0].subscription.is_none());
    assert!(app
        .scene_settings
        .feedback
        .as_deref()
        .unwrap()
        .contains("not undone"));
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn scene_tab_keeps_apply_and_navigation_reachable_at_all_target_sizes() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for size in [[800., 600.], [1280., 800.], [1440., 900.]] {
            let (mut app, commands) = connected();
            app.open_scene_settings();
            app.scene_settings.draft.fixed_frame = "a_long_unapplied_frame_name".repeat(4);
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            ctx.set_theme(theme);
            for _ in 0..3 {
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0], size[1]),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.render(ctx),
                );
                let tree = out.platform_output.accesskit_update.unwrap();
                let labels: Vec<_> = tree
                    .nodes
                    .iter()
                    .filter_map(|(_, n)| n.label().or_else(|| n.value()))
                    .collect();
                for label in [
                    "File",
                    "View",
                    "Help",
                    "Topics",
                    "Scene",
                    "Scene settings",
                    "Apply scene settings",
                    "Discard scene draft",
                ] {
                    assert!(
                        labels.contains(&label),
                        "{theme:?} {size:?}: missing {label} in {labels:?}"
                    );
                }
                assert!(!labels.contains(&"More…"));
                assert!(!labels.contains(&"Done"));
            }
            assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

#[test]
fn revamp_keeps_connection_and_search_visible_with_selected_properties() {
    for size in [[800., 600.], [1280., 800.], [1440., 900.]] {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let (mut app, commands) = connected();
            app.show_topic(Output::SpatialScan, "/scan", spatial::SCAN_MESSAGE_TYPE);
            let start = commands.try_recv().unwrap();
            ingest(&mut app, started(&start));
            app.inspector = false;
            app.global_options_open = false;
            app.topics_tab = true;
            app.select_display(app.selected.unwrap());
            assert!(!app.inspector, "selection must not reopen Views");
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            ctx.set_theme(theme);
            app.topic_query = "/scan".into();
            for _ in 0..3 {
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(size[0], size[1]),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.render(ctx),
                );
                let tree = out.platform_output.accesskit_update.unwrap();
                let labels: Vec<_> = tree
                    .nodes
                    .iter()
                    .filter_map(|(_, n)| n.label().or_else(|| n.value()))
                    .collect();
                for label in [
                    "Domain",
                    "Middleware",
                    "Disconnect",
                    "Configure…",
                    "Available topics",
                    "Topic filters",
                ] {
                    assert!(labels.contains(&label), "{size:?}: missing {label}");
                }
                let show = tree
                    .nodes
                    .iter()
                    .map(|(_, node)| node)
                    .find(|node| node.label() == Some("/scan"))
                    .expect("Selected topic remains reachable");
                let bounds = show.bounds().expect("Show has native bounds");
                assert!(
                    bounds.y1 <= f64::from(size[1]) - 30.0,
                    "Show clipped at {size:?}: {bounds:?}"
                );
                assert!(!labels.contains(&"Observe only"));
                assert!(!labels.contains(&"Connection settings"));
            }
            assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

#[test]
fn connection_controls_keep_identical_bounds_across_state_and_content() {
    for size in [[800., 600.], [1280., 800.], [1440., 900.]] {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let (mut app, commands) = connected();
            let ctx = egui::Context::default();
            fonts::configure(&ctx);
            shell::configure(&ctx);
            ctx.enable_accesskit();
            ctx.set_theme(theme);
            let mut reference = None;
            for (phase, action, cyclone, domain) in [
                (Phase::Disconnected, "Connect", false, "0"),
                (Phase::Starting, "Cancel startup", false, "187"),
                (Phase::Connected, "Disconnect", false, "187"),
                (Phase::Failed, "Connect", true, "12345678901234567890"),
                (Phase::Disconnected, "Connect", true, "232"),
            ] {
                app.connection.phase = phase;
                app.connection.cyclone = cyclone;
                app.connection.domain = domain.into();
                for _ in 0..3 {
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
                    let action_rect = tree
                        .nodes
                        .iter()
                        .map(|(_, n)| n)
                        .find(|n| n.label() == Some(action))
                        .unwrap()
                        .bounds()
                        .unwrap();
                    let middleware_rect = tree
                        .nodes
                        .iter()
                        .map(|(_, n)| n)
                        .find(|n| n.label() == Some("Middleware") && n.value().is_some())
                        .unwrap()
                        .bounds()
                        .unwrap();
                    let domain_rect = tree
                        .nodes
                        .iter()
                        .map(|(_, n)| n)
                        .find(|n| n.value() == Some(domain))
                        .unwrap()
                        .bounds()
                        .unwrap();
                    let current = [action_rect, middleware_rect, domain_rect];
                    if let Some(expected) = reference {
                        assert_eq!(
                            current, expected,
                            "{theme:?} {size:?} {action} {domain}: control geometry moved"
                        );
                    } else {
                        reference = Some(current);
                    }
                    assert!(action_rect.x1 < f64::from(size[0]), "action clipped");
                }
            }
            assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

#[test]
fn inspector_and_scene_bounds_stay_fixed_as_data_and_errors_change() {
    for output in [
        Output::CompactImage,
        Output::SpatialScan,
        Output::MapSnapshot,
        Output::TfGraph,
    ] {
        let (mut app, commands) = connected();
        let (topic, ty) = match output {
            Output::CompactImage => ("/camera", images::IMAGE_MESSAGE_TYPE),
            Output::MapSnapshot => ("/map", "nav_msgs/msg/OccupancyGrid"),
            Output::TfGraph => ("/tf", spatial::TF_MESSAGE_TYPE),
            _ => ("/scan", spatial::SCAN_MESSAGE_TYPE),
        };
        app.discovery.topics.push((topic.into(), vec![ty.into()]));
        let id = add(&mut app, output, topic, ty);
        app.setup = false;
        app.global_options_open = false;
        app.topics_tab = true;
        let ctx = egui::Context::default();
        fonts::configure(&ctx);
        shell::configure(&ctx);
        let mut reference = None;
        for (status, text) in [
            (
                ViewStatus::Waiting,
                "Waiting for acquisition-time transforms".into(),
            ),
            (ViewStatus::Receiving, "Receiving".into()),
            (
                ViewStatus::NeedsRecovery,
                "Extremely_long_unbroken_frame_and_topic_".repeat(30),
            ),
        ] {
            let view = app.views.iter_mut().find(|v| v.id == id).unwrap();
            view.status = status;
            view.spatial_status = text.clone();
            view.spatial_details = text.clone();
            view.error = (status == ViewStatus::NeedsRecovery).then_some(text);
            for _ in 0..4 {
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
            let rect = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("inspector"))
                .unwrap()
                .rect;
            assert!((rect.width() - 320.).abs() < 1., "inspector grew: {rect:?}");
            if let Some(prior) = reference {
                assert_eq!(rect, prior, "sample/status resized inspector");
            }
            reference = Some(rect);
        }
        assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
    }
}

#[test]
fn topic_filter_default_and_row_alignment_survive_long_names() {
    let (mut app, commands) = connected();
    assert!(!app.show_all_topics);
    app.global_options_open = false;
    app.topics_tab = true;
    app.discovery.topics = vec![
        ("/a".into(), vec![spatial::SCAN_MESSAGE_TYPE.into()]),
        (
            format!("/long_{}", "sensor_".repeat(30)),
            vec![images::IMAGE_MESSAGE_TYPE.into()],
        ),
        (
            "/unsupported".into(),
            vec!["example_msgs/msg/Unknown".into()],
        ),
    ];
    let ctx = egui::Context::default();
    fonts::configure(&ctx);
    shell::configure(&ctx);
    ctx.enable_accesskit();
    for include_nonvisual in [false, true] {
        app.show_all_topics = include_nonvisual;
        let mut nodes = std::collections::HashMap::new();
        for _ in 0..4 {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280., 800.),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            for (id, node) in out.platform_output.accesskit_update.unwrap().nodes {
                nodes.insert(id, node);
            }
        }
        let source_bounds: Vec<_> = nodes
            .values()
            .filter(|n| {
                n.role() == egui::accesskit::Role::Button
                    && n.label()
                        .is_some_and(|s| s == "/a" || s.starts_with("/long_"))
            })
            .filter_map(|n| n.bounds())
            .collect();
        assert_eq!(source_bounds.len(), 2);
        assert!(
            (source_bounds[0].x0 - source_bounds[1].x0).abs() < 1.,
            "topic names are not left aligned"
        );
        let actions: Vec<_> = nodes
            .values()
            .filter(|n| n.label().is_some_and(|s| s.starts_with("Show /")))
            .filter_map(|n| n.bounds())
            .collect();
        assert_eq!(actions.len(), 2);
        assert!(
            (actions[0].x0 - actions[1].x0).abs() < 1.,
            "eye actions moved with name length"
        );
        assert_eq!(
            nodes
                .values()
                .any(|n| n.value() == Some("No visual display · Messages are available in Tools")),
            include_nonvisual
        );
    }
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn compact_inspector_geometry_is_fixed_through_long_live_feedback() {
    let (mut app, commands) = connected();
    let id = add(
        &mut app,
        Output::SpatialScan,
        "/scan",
        spatial::SCAN_MESSAGE_TYPE,
    );
    app.setup = false;
    app.global_options_open = false;
    app.compact_inspector_open = true;
    let ctx = egui::Context::default();
    fonts::configure(&ctx);
    shell::configure(&ctx);
    ctx.enable_accesskit();
    let mut prior = None;
    for text in [
        "Receiving".into(),
        "Waiting for acquisition-time transform".into(),
        "Long_unbroken_input_".repeat(100),
    ] {
        app.views.iter_mut().find(|v| v.id == id).unwrap().error = Some(text);
        let mut bounds = None;
        for _ in 0..5 {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 600.),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            bounds = out
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some("Display inspector"))
                .and_then(|(_, n)| n.bounds())
                .or(bounds);
        }
        let rect = bounds.unwrap();
        assert!(
            rect.x0 >= 0. && rect.y0 >= 0. && rect.x1 <= 800. && rect.y1 <= 600.,
            "compact window outside screen: {rect:?}"
        );
        if let Some(previous) = prior {
            assert_eq!(rect, previous, "content resized compact inspector");
        }
        prior = Some(rect);
    }
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn issue_recovery_reveals_the_topic_error_and_scene_field_without_activation() {
    let (mut app, commands) = connected();
    app.global_options_open = true;
    app.sidebar = false;
    app.topic_action_error = Some("Show needs a corrected source".into());
    app.scene_settings.draft.fixed_frame = "retained_draft".into();
    app.reveal_attention_target(crate::panels::AttentionTarget::Topics);
    assert!(app.sidebar && app.topics_tab && !app.global_options_open);
    assert!(app.topic_action_error.is_some());
    assert_eq!(app.scene_settings.draft.fixed_frame, "retained_draft");
    app.reveal_attention_target(crate::panels::AttentionTarget::Scene);
    assert!(app.global_options_open && app.scene_settings.focus_requested);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800., 600.),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    assert!(!app.scene_settings.focus_requested);
    let focus = ctx
        .memory(|m| m.focused())
        .expect("Scene recovery must request field focus");
    let response = ctx.read_response(focus).unwrap();
    assert!(response.rect.top() > 100. && response.rect.bottom() < 550.);
    assert!(out
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Fixed frame")));
    assert_eq!(commands.try_recv(), Err(TryRecvError::Empty));
}
