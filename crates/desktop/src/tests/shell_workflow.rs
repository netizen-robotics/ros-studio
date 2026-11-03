use super::*;

#[test]
fn colliding_short_titles_keep_visible_namespaces_in_scene_workspace() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    let second = add(&mut app, &commands, "/two");
    ingest(&mut app, started(&first));
    ingest(&mut app, started(&second));
    let topics = [
        "/robot_alpha/diagnostics/status",
        "/robot_beta/diagnostics/status",
    ];
    for (view, topic) in app.views.iter_mut().zip(topics) {
        view.configuration.topic = topic.into();
    }
    app.sidebar = false;
    app.inspector = false;
    app.workspace_poll(&egui::Context::default());
    let owners: Vec<_> = app.views.iter().map(|view| view.subscription).collect();
    let order = app.workspace.layout.order.clone();
    for _ in 0..2 {
        let ctx = egui::Context::default();
        shell::configure(&ctx);
        ctx.enable_accesskit();
        let output = frame(&mut app, &ctx, None);
        let tree = output.platform_output.accesskit_update.as_ref().unwrap();
        for topic in topics {
            let node = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label().or_else(|| node.value()) == Some(topic))
                .expect("Visible full-topic context must survive collapse");
            let bounds = node.1.bounds().expect("Topic context has bounds");
            assert!(bounds.y0 >= 0.0 && bounds.y1 <= 800.0);
            assert!(tree.nodes.iter().any(|(_, node)| {
                node.label() == Some(format!("Details: {topic} · Messages").as_str())
            }));
        }
    }
    assert_eq!(app.workspace.layout.order, order);
    assert_eq!(
        app.views
            .iter()
            .map(|view| view.subscription)
            .collect::<Vec<_>>(),
        owners
    );
    assert!(commands.try_recv().is_err());
}

#[test]
fn typography_and_control_sizes_survive_native_theme_changes() {
    let ctx = egui::Context::default();
    shell::configure(&ctx);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let style = ctx.style_of(theme);
        assert_eq!(style.spacing.interact_size.y, 34.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, 14.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Monospace].size, 12.0);
        assert_eq!(style.visuals.dark_mode, theme == egui::Theme::Dark);
    }
}

#[test]
fn sidebar_reorder_keeps_exact_live_owners_and_split() {
    let (mut app, commands) = connected();
    let one = add(&mut app, &commands, "/one");
    let two = add(&mut app, &commands, "/two");
    ingest(&mut app, started(&one));
    ingest(&mut app, started(&two));
    app.sidebar = false;
    app.inspector = false;
    app.sidebar = true;
    app.inspector = false;
    app.workspace_poll(&egui::Context::default());
    app.workspace.layout.scene_percent = 75;
    app.selected = Some(app.views[0].id);
    let owners: Vec<_> = app.views.iter().map(|v| (v.id, v.subscription)).collect();
    let first = app.views[0].workspace_id.clone();
    let second = app.views[1].workspace_id.clone();
    let ctx = egui::Context::default();
    shell::configure(&ctx);
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, None);
    let event = click(output, &format!("Move down: /one ({first})"));
    frame(&mut app, &ctx, Some(event));
    assert_eq!(app.workspace.layout.order, [second, first]);
    assert_eq!(app.workspace.layout.scene_percent, 75);
    assert_eq!(
        app.views
            .iter()
            .map(|v| (v.id, v.subscription))
            .collect::<Vec<_>>(),
        owners
    );
    assert!(commands.try_recv().is_err());
}

#[test]
fn acknowledging_diagnostics_keeps_unresolved_view_error_visible() {
    let (mut app, commands) = connected();
    let one = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&one));
    app.views[0].error = Some("Synthetic view needs recovery".into());
    app.diagnostic("Synthetic view needs recovery".into());
    app.diagnostics_open = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, None);
    let event = click(output, "Acknowledge errors");
    frame(&mut app, &ctx, Some(event));
    assert!(app.diagnostics.is_empty());
    assert_eq!(
        app.views[0].error.as_deref(),
        Some("Synthetic view needs recovery")
    );
    // AccessKit outputs incremental updates. Rebuild a full tree to check the
    // retained error rather than requiring an unchanged node to be re-emitted.
    let fresh = egui::Context::default();
    fresh.enable_accesskit();
    let output = frame(&mut app, &fresh, None);
    assert!(output
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .iter()
        .any(|(_, n)| n
            .label()
            .or_else(|| n.value())
            .is_some_and(|s| s.contains("Synthetic view needs recovery"))));
    assert!(commands.try_recv().is_err());
}

#[test]
fn eight_inactive_panels_explain_and_disable_a_ninth_draft_subscription() {
    let (mut app, commands) = connected();
    for _ in 0..8 {
        let start = add(&mut app, &commands, "/one");
        ingest(&mut app, started(&start));
        let id = app.views.last().unwrap().id;
        app.stop_view(id);
        let stop = commands.try_recv().unwrap();
        ingest(&mut app, stopped(&stop));
    }
    let shared = app.session.as_ref().unwrap().shared.lock().unwrap();
    assert_eq!(shared.registry.used_resources(), 0);
    assert_eq!(
        shared.registry.available_resources(),
        studio_protocol::subscriptions::MAX_NATIVE_HANDLES
    );
    drop(shared);
    app.begin_add();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, None);
    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
    let button = tree
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Enable display"))
        .unwrap();
    assert!(button.1.is_disabled());
    assert!(tree.nodes.iter().any(|(_, n)| n
        .label()
        .or_else(|| n.value())
        .is_some_and(|s| s.contains("Eight displays configured"))));
    let event = click(output, "Enable display");
    frame(&mut app, &ctx, Some(event));
    assert_eq!(app.views.len(), 8);
    assert!(app.views.iter().all(|v| v.status == ViewStatus::Stopped));
    assert!(commands.try_recv().is_err());
}

#[test]
fn workspace_manager_does_not_offer_retired_grid_controls() {
    let mut app = App::default();
    app.workspace.open = true;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, None);
    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
    for label in [
        "Edit layout in workspace",
        "Columns",
        "Relative size",
        "Collapsed",
    ] {
        assert!(!nodes.iter().any(|(_, n)| n.label() == Some(label)));
    }
}

#[test]
fn header_navigation_and_settings_never_replace_live_owners() {
    let (mut app, commands) = connected();
    let command = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&command));
    let owner = app.views[0].subscription;
    let ctx = egui::Context::default();
    shell::configure(&ctx);
    ctx.enable_accesskit();
    let output = frame(&mut app, &ctx, None);
    let event = click(output, "View");
    let output = frame(&mut app, &ctx, Some(event));
    let event = click(output, "Topics");
    frame(&mut app, &ctx, Some(event));
    assert!(!app.sidebar);
    let output = frame(&mut app, &ctx, None);
    let event = click(output, "Connection settings");
    frame(&mut app, &ctx, Some(event));
    assert!(app.settings);
    assert_eq!(app.views[0].subscription, owner);
    assert!(commands.try_recv().is_err());
}

#[test]
fn selecting_display_details_preserves_subscription_and_identity() {
    let (mut app, commands) = connected();
    let first = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&first));
    app.sidebar = false;
    app.inspector = false;
    let owner = app.views[0].subscription;
    let saved = app.views[0].workspace_id.clone();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let event = click(frame(&mut app, &ctx, None), "Details: /one · Messages");
    frame(&mut app, &ctx, Some(event));
    assert_eq!(app.selected, Some(app.views[0].id));
    assert!(app.inspector);
    assert_eq!(app.views[0].subscription, owner);
    assert_eq!(app.views[0].workspace_id, saved);
    assert!(commands.try_recv().is_err());
}

#[test]
fn independently_named_sidebar_toggles_stop_only_the_selected_owner() {
    let (mut app, commands) = connected();
    let one = add(&mut app, &commands, "/one");
    let two = add(&mut app, &commands, "/two");
    ingest(&mut app, started(&one));
    ingest(&mut app, started(&two));
    app.inspector = false;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let label = format!(
        "Enable display: /two · Messages ({})",
        app.views[1].workspace_id
    );
    let event = click(frame(&mut app, &ctx, None), &label);
    frame(&mut app, &ctx, Some(event));
    assert!(app.views[0].enabled);
    assert!(!app.views[1].enabled);
    let stop = commands.try_recv().unwrap();
    assert_eq!(stop.ids().2, two.ids().2);
    assert!(commands.try_recv().is_err());
}
fn key_press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

#[test]
fn keyboard_type_choice_returns_focus_to_selector_then_continues_setup() {
    let (mut app, commands) = connected();
    app.begin_add();
    app.discovery.select("/multi");
    app.sidebar = false;
    let context = egui::Context::default();
    context.enable_accesskit();
    context.all_styles_mut(|style| style.animation_time = 0.0);
    let output = frame(&mut app, &context, None);
    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
    let type_selector = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Message type"))
        .unwrap()
        .0;
    let messages = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Messages"))
        .unwrap()
        .0;
    let focus = |target| {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target,
            data: None,
        })
    };
    frame(&mut app, &context, Some(focus(type_selector)));
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Space)));
    let row = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("b/msg/B"))
        .expect("Keyboard opens type choices")
        .0;
    frame(&mut app, &context, Some(focus(row)));
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Enter)));
    assert_eq!(app.discovery.ty, "b/msg/B");
    assert_eq!(
        output.platform_output.accesskit_update.unwrap().focus,
        type_selector,
        "Closing selected popup row must restore focus to its selector"
    );
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Tab)));
    assert_eq!(
        output.platform_output.accesskit_update.unwrap().focus,
        messages,
        "Tab must continue setup rather than restart at the header"
    );
    assert!(app.views.is_empty());
    assert!(commands.try_recv().is_err());
}

#[test]
fn keyboard_topic_choice_keeps_focus_and_advances_to_type_without_subscribing() {
    let (mut app, commands) = connected();
    app.begin_add();
    app.discovery.select("/one");
    app.sidebar = false;
    let context = egui::Context::default();
    context.enable_accesskit();
    context.all_styles_mut(|style| style.animation_time = 0.0);
    let output = frame(&mut app, &context, None);
    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
    let topic_selector = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Topic"))
        .unwrap()
        .0;
    let type_selector = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Message type"))
        .unwrap()
        .0;
    let focus = |target| {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target,
            data: None,
        })
    };
    frame(&mut app, &context, Some(focus(topic_selector)));
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Space)));
    let row = output
        .platform_output
        .accesskit_update
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("/multi"))
        .expect("Keyboard opens discovered topic choices")
        .0;
    frame(&mut app, &context, Some(focus(row)));
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Enter)));
    assert_eq!(app.discovery.topic, "/multi");
    assert!(
        app.discovery.ty.is_empty(),
        "A multi-type topic still requires explicit type selection"
    );
    assert_eq!(
        output.platform_output.accesskit_update.unwrap().focus,
        topic_selector
    );
    let output = frame(&mut app, &context, Some(key_press(egui::Key::Tab)));
    assert_eq!(
        output.platform_output.accesskit_update.unwrap().focus,
        type_selector,
        "Tab after choosing a topic must continue to message type"
    );
    assert!(app.views.is_empty());
    assert!(commands.try_recv().is_err());
}

#[test]
fn no_enabled_displays_has_actionable_center_and_no_renderer_controls() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let nodes = frame(&mut app, &ctx, None)
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes;
    assert!(nodes.iter().any(|(_, n)| n
        .value()
        .or_else(|| n.label())
        .is_some_and(|s| s.contains("Enable a display"))));
    assert!(!nodes
        .iter()
        .any(|(_, n)| n.label() == Some("Scene height (%)")));
}

#[test]
fn closed_manager_keeps_workspace_error_reachable_without_changing_live_document() {
    let (mut app, commands) = connected();
    let start = add(&mut app, &commands, "/one");
    ingest(&mut app, started(&start));
    app.workspace_poll(&egui::Context::default());
    app.workspace.layout.scene_percent = 78;
    app.workspace_shutdown();
    let error = app
        .workspace_file(workspace_app::FileAction::Open)
        .unwrap_err();
    app.workspace.open = false;
    let document = app.workspace_snapshot().unwrap();
    let owner = app.views[0].subscription;
    let context = egui::Context::default();
    context.enable_accesskit();
    let output = frame(&mut app, &context, None);
    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
    assert!(tree.nodes.iter().any(|(_, node)| node
        .label()
        .or_else(|| node.value())
        .is_some_and(|text| text.contains("Workspace needs attention"))));
    assert!(!app.workspace.open);
    let event = click(output, "Review workspace error");
    let output = frame(&mut app, &context, Some(event));
    assert!(app.workspace.open);
    assert!(
        output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| node
                .label()
                .or_else(|| node.value())
                .is_some_and(|text| text.contains(&error))),
        "Review must expose the retained controller error"
    );
    assert_eq!(app.workspace_snapshot().unwrap(), document);
    assert_eq!(app.views[0].subscription, owner);
    assert!(commands.try_recv().is_err());
}

#[test]
fn expanded_capacity_footer_stays_inside_small_window_without_commands() {
    let (mut app, commands) = connected();
    for _ in 0..8 {
        let start = add(&mut app, &commands, "/one");
        ingest(&mut app, started(&start));
        let id = app.views.last().unwrap().id;
        app.stop_view(id);
        let stop = commands.try_recv().unwrap();
        ingest(&mut app, stopped(&stop));
    }
    app.inspector = false;
    for size in [egui::vec2(1280.0, 800.0), egui::vec2(760.0, 560.0)] {
        let ctx = egui::Context::default();
        shell::configure(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        ctx.enable_accesskit();
        let render = |event: Option<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            )
        };
        let mut render = render;
        let output = render(None);
        let event = click(output, "Resource capacity");
        let mut nodes = std::collections::BTreeMap::new();
        for output in [render(Some(event)), render(None), render(None)] {
            nodes.extend(output.platform_output.accesskit_update.unwrap().nodes);
        }
        for text in [
            "Eight displays configured",
            "Native readers",
            "Scene layers share two TF readers",
        ] {
            let node = nodes
                .values()
                .find(|node| {
                    node.label()
                        .or_else(|| node.value())
                        .is_some_and(|value| value.contains(text))
                })
                .unwrap_or_else(|| panic!("Missing footer guidance: {text}"));
            let bounds = node.bounds().expect("footer text has bounds");
            assert!(
                bounds.y0 >= 0.0 && bounds.y1 <= f64::from(size.y),
                "Clipped {text}: {bounds:?} in {size:?}"
            );
            assert!(!node.is_hidden(), "Hidden footer guidance: {text}");
        }
        assert!(commands.try_recv().is_err());
        assert!(app
            .views
            .iter()
            .all(|view| view.status == ViewStatus::Stopped));
    }
}

fn frame(app: &mut App, ctx: &egui::Context, event: Option<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280., 800.),
            )),
            events: event.into_iter().collect(),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    )
}
fn click(output: egui::FullOutput, label: &str) -> egui::Event {
    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
    let target = nodes
        .iter()
        .find(|(_, n)| n.label() == Some(label))
        .unwrap_or_else(|| panic!("Missing control: {label}"))
        .0;
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target,
        data: None,
    })
}
