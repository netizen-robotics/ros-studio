use super::*;
use studio_protocol::spatial as wire;

fn graph(app: &mut App) {
    ingest(
        app,
        Event::Graph {
            topics: vec![
                ("/one".into(), vec!["std_msgs/msg/Float64".into()]),
                ("/two".into(), vec!["std_msgs/msg/Float64".into()]),
                ("/scan".into(), vec![spatial::SCAN_MESSAGE_TYPE.into()]),
                (
                    "/cloud".into(),
                    vec![studio_protocol::cloud::MESSAGE_TYPE.into()],
                ),
                ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
            ],
        },
    );
}
fn draft(app: &mut App) {
    app.begin_add();
    app.discovery.select("/scan");
    app.draft_output = Output::SpatialScan;
    app.draft_spatial.fixed_frame = "map".into();
}
fn observation(command: &Command, sequence: u64, stamp: i64, tf: wire::TfOutcome) -> Event {
    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object {
            original_fields: fields.len(),
            fields: fields
                .into_iter()
                .map(|(name, value)| Field {
                    name: name.into(),
                    value,
                })
                .collect(),
        }
    }
    fn float(value: f64) -> Value {
        Value::Float {
            value: structured::FloatValue::Finite(value.to_string()),
        }
    }
    let (generation, _, subscription) = command.ids();
    Event::SpatialObservation {
        generation,
        subscription,
        sequence,
        backend_drops: 3,
        sample: Box::new(wire::Sample {
            contract_version: wire::CONTRACT_VERSION,
            fixed_frame: "map".into(),
            source_frame: "laser".into(),
            acquisition_stamp_ns: stamp * 1_000_000_000,
            tf,
            counters: wire::Counters {
                scans_received: sequence,
                ..Default::default()
            },
            scan: Box::new(structured::Sample {
                contract_version: 2,
                topic: "/scan".into(),
                message_type: spatial::SCAN_MESSAGE_TYPE.into(),
                source_time: None,
                value: object(vec![
                    (
                        "header",
                        object(vec![
                            (
                                "frame_id",
                                Value::Text {
                                    value: "laser".into(),
                                    original_bytes: 5,
                                },
                            ),
                            (
                                "stamp",
                                object(vec![
                                    (
                                        "sec",
                                        Value::Signed {
                                            value: stamp.to_string(),
                                        },
                                    ),
                                    ("nanosec", Value::Unsigned { value: "0".into() }),
                                ]),
                            ),
                        ]),
                    ),
                    ("angle_min", float(0.)),
                    ("angle_max", float(0.5)),
                    ("angle_increment", float(0.5)),
                    ("time_increment", float(0.)),
                    ("scan_time", float(0.1)),
                    ("range_min", float(0.1)),
                    ("range_max", float(10.)),
                    (
                        "ranges",
                        Value::Array {
                            items: vec![float(1.), float(2.)],
                            original_items: 2,
                        },
                    ),
                    (
                        "intensities",
                        Value::Array {
                            items: vec![],
                            original_items: 0,
                        },
                    ),
                ]),
            }),
        }),
    }
}
fn ready_tf() -> wire::TfOutcome {
    wire::TfOutcome::Ready {
        components: [1., 0., 0., 0., 0., 0., 1.],
    }
}

fn cloud_observation(command: &Command, sequence: u64, tf: wire::TfOutcome) -> Event {
    let (generation, _, subscription) = command.ids();
    Event::PointCloudObservation {
        generation,
        subscription,
        sequence,
        backend_drops: 0,
        sample: Box::new(studio_protocol::cloud::Sample {
            contract_version: studio_protocol::cloud::CONTRACT_VERSION,
            topic: "/cloud".into(),
            message_type: studio_protocol::cloud::MESSAGE_TYPE.into(),
            fixed_frame: "map".into(),
            source_frame: "cloud_sensor".into(),
            acquisition_stamp_ns: 10_000_000_000,
            source_points: 1,
            skipped_points: 0,
            points: vec![[1., 2., 3.]],
            channels: Default::default(),
            tf,
            counters: Default::default(),
        }),
    }
}

#[test]
fn scan_and_cloud_share_tf_missing_cloud_recovers_and_stop_keeps_peer_geometry() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scan = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan));
    ingest(&mut app, observation(&scan, 1, 10, ready_tf()));

    app.begin_add();
    app.discovery.select("/cloud");
    app.draft_output = Output::PointCloud;
    app.draft_clock_verified = true;
    app.subscribe();
    let cloud = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&cloud));
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        4
    );
    ingest(&mut app, cloud_observation(&cloud, 1, ready_tf()));
    assert_eq!(
        app.views[1].cloud_frame.as_ref().unwrap().points,
        [spatial::Point3([2., 2., 3.])]
    );
    ingest(
        &mut app,
        cloud_observation(&cloud, 2, wire::TfOutcome::Missing),
    );
    assert_eq!(app.views[1].status, ViewStatus::Waiting);
    assert!(app.views[1].cloud_frame.as_ref().unwrap().points.is_empty());
    assert_eq!(app.views[0].spatial_scene.layers()[0].points().len(), 2);
    ingest(&mut app, cloud_observation(&cloud, 3, ready_tf()));
    assert_eq!(app.views[1].status, ViewStatus::Receiving);
    assert_eq!(app.views[1].cloud_frame.as_ref().unwrap().points.len(), 1);
    app.set_display_enabled(app.views[1].id, false);
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[1].cloud_frame.is_none());
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        4
    );
    ingest(&mut app, cloud_observation(&cloud, 4, ready_tf()));
    assert!(app.views[1].cloud_frame.is_none());
    ingest(&mut app, stopped(&stop));
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        3
    );
    assert_eq!(app.views[0].spatial_scene.layers()[0].points().len(), 2);
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn malformed_current_cloud_stops_only_cloud_and_retains_native_reservation_until_ack() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scan = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan));
    ingest(&mut app, observation(&scan, 1, 10, ready_tf()));
    app.begin_add();
    app.discovery.select("/cloud");
    app.draft_output = Output::PointCloud;
    app.draft_clock_verified = true;
    app.subscribe();
    let cloud = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&cloud));
    ingest(&mut app, cloud_observation(&cloud, 1, ready_tf()));
    let mut malformed = cloud_observation(&cloud, 2, ready_tf());
    if let Event::PointCloudObservation { sample, .. } = &mut malformed {
        sample.source_points = 2;
    }
    ingest(&mut app, malformed);
    let stop = outgoing.try_recv().unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert_eq!(stop.ids().2, cloud.ids().2);
    assert_eq!(app.views[1].status, ViewStatus::Stopping);
    assert!(!app.views[1].enabled);
    assert!(app.views[1].cloud_frame.is_none());
    assert!(app.views[1].error.is_some());
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    assert_eq!(app.views[0].spatial_scene.layers()[0].points().len(), 2);
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        4
    );
    ingest(&mut app, stopped(&stop));
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        3
    );
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn spatial_requires_clock_confirmation_and_reserves_three_resources_until_cleanup() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.subscribe();
    assert!(outgoing.try_recv().is_err());
    assert!(app.views.is_empty());
    app.draft_clock_verified = true;
    app.subscribe();
    let scene = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scene));
    let peer = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&peer));
    for _ in 0..4 {
        let extra = add(&mut app, &outgoing, "/one");
        ingest(&mut app, started(&extra));
    }
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        8
    );
    app.begin_add();
    app.discovery.select("/two");
    app.subscribe();
    assert!(outgoing.try_recv().is_err());
    assert_eq!(app.views.len(), 6);
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        8
    );
    ingest(&mut app, stopped(&stop));
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        5
    );
    ingest(&mut app, sample(&peer, 1, 42));
    assert_eq!(app.views[1].received.typed_samples, 1);
    app.stop();
    assert!(!app.views[0].spatial_clock_verified);
}

#[test]
fn stopped_spatial_retry_respects_remaining_resource_capacity() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scene = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scene));
    let first = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&first));
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, stopped(&stop));
    let second = add(&mut app, &outgoing, "/two");
    ingest(&mut app, started(&second));
    for _ in 0..4 {
        let extra = add(&mut app, &outgoing, "/one");
        ingest(&mut app, started(&extra));
    }
    app.start_view(ViewId(1));
    assert!(outgoing.try_recv().is_err());
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        6
    );
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
    assert!(app.views[0]
        .error
        .as_ref()
        .is_some_and(|error| error.contains("Shared native reader capacity exceeded")));
}

#[test]
fn missing_transform_clears_points_and_same_acquisition_can_later_resolve_without_restart() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let command = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&command));
    ingest(&mut app, observation(&command, 1, 10, ready_tf()));
    assert_eq!(
        app.views[0].spatial_scene.layers()[0].points()[0],
        spatial::Point3([2., 0., 0.])
    );
    ingest(
        &mut app,
        observation(&command, 2, 11, wire::TfOutcome::Missing),
    );
    assert!(app.views[0].spatial_scene.layers()[0].points().is_empty());
    assert_eq!(app.views[0].status, ViewStatus::Waiting);
    assert!(outgoing.try_recv().is_err());
    ingest(&mut app, observation(&command, 3, 11, ready_tf()));
    assert_eq!(app.views[0].spatial_scene.layers()[0].points().len(), 2);
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    assert_eq!(app.views[0].spatial_status, "TF: Transform ready");
    assert!(app.views[0]
        .spatial_details
        .contains("Acquisition: 11000000000 ns"));
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    ingest(&mut app, observation(&command, 4, 12, ready_tf()));
    assert!(app.views[0].spatial_scene.layers().is_empty());
    assert!(app.views[0].spatial_details.is_empty());
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[0].status, ViewStatus::Stopped);
}

#[test]
fn spatial_fixed_frame_identity_is_checked_before_sequence_and_final_counters_survive_stop() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let command = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&command));
    let mut wrong = observation(&command, 1, 10, ready_tf());
    if let Event::SpatialObservation { sample, .. } = &mut wrong {
        sample.fixed_frame = "odom".into();
    }
    ingest(&mut app, wrong);
    assert!(app.views[0].spatial_scene.layers()[0].points().is_empty());
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .ingress_rejections,
        1
    );
    ingest(&mut app, observation(&command, 1, 10, ready_tf()));
    assert_eq!(app.views[0].spatial_scene.layers()[0].points().len(), 2);
    app.stop_view(ViewId(1));
    let stop = outgoing.try_recv().unwrap();
    let (generation, _, subscription) = command.ids();
    ingest(
        &mut app,
        Event::Drops {
            generation,
            subscription,
            backend_drops: 9,
            spatial_counters: Some(wire::Counters {
                tf_accepted: 42,
                pending_replaced: 5,
                ..Default::default()
            }),
        },
    );
    ingest(
        &mut app,
        Event::Drops {
            generation,
            subscription,
            backend_drops: 2,
            spatial_counters: Some(Default::default()),
        },
    );
    assert_eq!(app.views[0].spatial_counters.tf_accepted, 42);
    ingest(&mut app, stopped(&stop));
    assert_eq!(app.views[0].spatial_counters.pending_replaced, 5);
    assert_eq!(app.views[0].received.backend_drops, 9);
    assert!(!app.views[0].spatial_scene.is_active());
}

#[test]
fn spatial_setup_exposes_named_frame_tf_and_clock_controls_without_activating() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 1100.),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
    let target = nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Transform inputs"))
        .unwrap()
        .0;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 1100.),
            )),
            events: vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target,
                    data: None,
                },
            )],
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440., 1100.),
            )),
            ..Default::default()
        },
        |ctx| app.render(ctx),
    );
    let tree = output.platform_output.accesskit_update.unwrap();
    assert!(tree
        .nodes
        .iter()
        .any(|(_, node)| node.role() == egui::accesskit::Role::TextInput
            && node.labelled_by().iter().any(|label| tree
                .nodes
                .iter()
                .any(|(id, node)| id == label && node.value() == Some("Fixed frame")))));
    for label in ["Dynamic TF topic", "Static TF topic"] {
        assert!(tree
            .nodes
            .iter()
            .any(|(_, node)| node.role() == egui::accesskit::Role::ComboBox
                && node.label() == Some(label)));
    }
    assert!(tree
        .nodes
        .iter()
        .any(|(_, node)| node.label() == Some("I verified these publishers use system time")));
    assert!(app.views.is_empty());
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn mapping_replacement_draft_edits_revoke_clock_but_explicit_checkbox_does_not() {
    use egui::accesskit::{Action, ActionRequest, NodeId, Role, TreeUpdate};
    fn run(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> TreeUpdate {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 1400.),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.setup_ui(ui));
            },
        )
        .platform_output
        .accesskit_update
        .unwrap()
    }
    fn action(target: NodeId, action: Action) -> Vec<egui::Event> {
        vec![egui::Event::AccessKitActionRequest(ActionRequest {
            action,
            target,
            data: None,
        })]
    }
    fn label(tree: &TreeUpdate, name: &str) -> NodeId {
        tree.nodes
            .iter()
            .find(|(_, n)| n.label() == Some(name))
            .unwrap_or_else(|| panic!("Missing {name}"))
            .0
    }
    fn input(tree: &TreeUpdate, name: &str) -> NodeId {
        tree.nodes
            .iter()
            .find(|(_, n)| {
                n.role() == Role::TextInput
                    && n.labelled_by().iter().any(|label| {
                        tree.nodes
                            .iter()
                            .any(|(id, node)| id == label && node.value() == Some(name))
                    })
            })
            .unwrap_or_else(|| panic!("Missing input {name}"))
            .0
    }
    let clock = "I verified these publishers use system time";
    for edit in [
        "Fixed frame",
        "Dynamic TF topic",
        "Static TF topic",
        "Dynamic TF best effort",
        "Topic",
        "Message type",
        "Messages",
    ] {
        let (mut app, outgoing) = connected();
        ingest(
            &mut app,
            Event::Graph {
                topics: vec![
                    (
                        "/map".into(),
                        vec![
                            studio_protocol::mapping::MAP_MESSAGE_TYPE.into(),
                            studio_protocol::mapping::POSE_MESSAGE_TYPE.into(),
                        ],
                    ),
                    (
                        "/pose".into(),
                        vec![studio_protocol::mapping::POSE_MESSAGE_TYPE.into()],
                    ),
                ],
            },
        );
        app.begin_add();
        app.discovery.select("/map");
        app.discovery.ty = studio_protocol::mapping::MAP_MESSAGE_TYPE.into();
        app.draft_output = Output::OccupancyGrid;
        app.draft_spatial.fixed_frame = "map".into();
        app.draft_clock_verified = true;
        app.subscribe();
        let command = outgoing.try_recv().unwrap();
        ingest(&mut app, started(&command));
        let id = app.views[0].id;
        let original = app.views[0].configuration.clone();
        app.configure_view(id);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.style_mut(|style| style.animation_time = 0.);
        let mut tree = run(&mut app, &ctx, vec![]);
        assert!(
            app.draft_clock_verified,
            "Opening reviewed replacement keeps its existing verification"
        );
        let toggle = label(&tree, clock);
        run(&mut app, &ctx, action(toggle, Action::Click));
        assert!(!app.draft_clock_verified);
        tree = run(&mut app, &ctx, vec![]);
        run(&mut app, &ctx, action(label(&tree, clock), Action::Click));
        assert!(
            app.draft_clock_verified,
            "Checking only the clock must not revoke itself"
        );
        tree = run(&mut app, &ctx, vec![]);
        match edit {
            "Fixed frame" => {
                run(&mut app, &ctx, action(input(&tree, edit), Action::Focus));
                run(&mut app, &ctx, vec![egui::Event::Text("_other".into())]);
                assert_ne!(
                    app.draft_spatial.fixed_frame,
                    original.spatial.as_ref().unwrap().fixed_frame
                );
            }
            "Dynamic TF topic" | "Static TF topic" | "Dynamic TF best effort" => {
                run(
                    &mut app,
                    &ctx,
                    action(label(&tree, "Transform inputs"), Action::Click),
                );
                tree = run(&mut app, &ctx, vec![]);
                if edit == "Dynamic TF best effort" {
                    run(&mut app, &ctx, action(label(&tree, edit), Action::Click));
                } else {
                    run(&mut app, &ctx, action(input(&tree, edit), Action::Focus));
                    run(&mut app, &ctx, vec![egui::Event::Text("_other".into())]);
                }
            }
            "Topic" | "Message type" => {
                run(&mut app, &ctx, action(label(&tree, edit), Action::Click));
                tree = run(&mut app, &ctx, vec![]);
                let choice = if edit == "Topic" {
                    "/pose"
                } else {
                    studio_protocol::mapping::POSE_MESSAGE_TYPE
                };
                run(&mut app, &ctx, action(label(&tree, choice), Action::Click));
            }
            _ => {
                run(&mut app, &ctx, action(label(&tree, edit), Action::Click));
            }
        }
        assert!(
            !app.draft_clock_verified,
            "{edit} must revoke old publisher verification"
        );
        assert_eq!(
            app.views[0].configuration, original,
            "Draft editing must preserve the active source"
        );
        assert!(app.views[0].enabled);
        assert!(
            outgoing.try_recv().is_err(),
            "{edit} emitted an unsolicited Start/Stop"
        );
    }
}

#[test]
fn incompatible_spatial_toggle_keeps_existing_shared_tf_owner_until_cleanup() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let first = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&first));
    let mut config = app.views[0].configuration.clone();
    config.topic = "/scan".into();
    config.spatial.as_mut().unwrap().fixed_frame = "odom".into();
    app.views.push(ViewState::new(ViewId(90), config));
    app.views[1].spatial_clock_verified = true;
    app.set_display_enabled(ViewId(90), true);
    assert!(!app.views[1].enabled);
    assert!(app.views[1]
        .error
        .as_ref()
        .unwrap()
        .contains("Scene frame and TF inputs must match"));
    assert!(app.views[0].enabled);
    assert!(outgoing.try_recv().is_err());
    app.set_display_enabled(ViewId(1), false);
    let stop = outgoing.try_recv().unwrap();
    app.set_display_enabled(ViewId(90), true);
    assert!(outgoing.try_recv().is_err());
    ingest(&mut app, stopped(&stop));
    app.set_display_enabled(ViewId(90), true);
    assert!(matches!(
        outgoing.try_recv().unwrap(),
        Command::Start { .. }
    ));
}

#[test]
fn scene_split_keyboard_action_changes_allocation_without_changing_owners() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scan = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan));
    let peer = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&peer));
    let owners: Vec<_> = app.views.iter().map(|v| v.subscription).collect();
    app.inspector = false;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut run = |event: Option<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1502., 935.),
                )),
                events: event.into_iter().collect(),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        )
    };
    let output = run(None);
    let nodes = output.platform_output.accesskit_update.unwrap().nodes;
    let target = nodes
        .iter()
        .find(|(_, n)| {
            n.role() == egui::accesskit::Role::Slider && n.label() == Some("Scene height (%)")
        })
        .unwrap()
        .0;
    run(Some(egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target,
            data: None,
        },
    )));
    run(Some(egui::Event::Key {
        key: egui::Key::ArrowRight,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }));
    assert_eq!(app.workspace.layout.scene_percent, 66);
    assert_eq!(
        app.views.iter().map(|v| v.subscription).collect::<Vec<_>>(),
        owners
    );
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn keyboard_focus_reveals_companion_controls_below_minimum_scene_viewport() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scan = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan));
    let mut image = ViewState::new(
        ViewId(90),
        Configuration {
            map_updates: None,
            topic: "/camera".into(),
            message_type: images::IMAGE_MESSAGE_TYPE.into(),
            best_effort: true,
            transient_local: false,
            output: Output::CompactImage,
            spatial: None,
            robot: None,
        },
    );
    image.enabled = true;
    image.status = ViewStatus::Waiting;
    image.image.start();
    app.views.push(image);
    app.sidebar = false;
    app.inspector = true;
    let ctx = egui::Context::default();
    shell::configure(&ctx);
    ctx.enable_accesskit();
    ctx.style_mut(|s| s.animation_time = 0.0);
    let mut run = |event: Option<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760., 560.),
                )),
                events: event.into_iter().collect(),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        )
    };
    let nodes = run(None).platform_output.accesskit_update.unwrap().nodes;
    let target = nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Actual size"))
        .unwrap()
        .0;
    assert!(
        nodes
            .iter()
            .find(|(id, _)| *id == target)
            .unwrap()
            .1
            .bounds()
            .unwrap()
            .y1
            > 560.
    );
    run(Some(egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target,
            data: None,
        },
    )));
    let mut latest = std::collections::BTreeMap::new();
    for _ in 0..4 {
        latest.extend(run(None).platform_output.accesskit_update.unwrap().nodes);
    }
    let node = latest.get(&target).unwrap();
    let bounds = node.bounds().unwrap();
    assert!(
        bounds.y0 >= 0. && bounds.y1 <= 560.,
        "Focused companion must be visible: {bounds:?}"
    );
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn keyboard_focus_reveals_camera_controls_inside_the_scene_scroll_area() {
    let (mut app, outgoing) = connected();
    graph(&mut app);
    draft(&mut app);
    app.draft_clock_verified = true;
    app.subscribe();
    let scan = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&scan));
    app.sidebar = false;
    app.inspector = true;
    app.diagnostics_open = true;
    let ctx = egui::Context::default();
    shell::configure(&ctx);
    ctx.enable_accesskit();
    ctx.style_mut(|s| s.animation_time = 0.0);
    let mut run = |event: Option<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760., 560.),
                )),
                events: event.into_iter().collect(),
                ..Default::default()
            },
            |ctx| app.render(ctx),
        )
    };
    let nodes = run(None).platform_output.accesskit_update.unwrap().nodes;
    let details = nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Camera details"))
        .unwrap()
        .0;
    run(Some(egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target: details,
            data: None,
        },
    )));
    let nodes = run(None).platform_output.accesskit_update.unwrap().nodes;
    let target = nodes
        .iter()
        .find(|(_, n)| {
            n.role() == egui::accesskit::Role::Slider && n.label() == Some("Orbit (rad)")
        })
        .unwrap()
        .0;
    run(Some(egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target,
            data: None,
        },
    )));
    let mut latest = std::collections::BTreeMap::new();
    for _ in 0..5 {
        latest.extend(run(None).platform_output.accesskit_update.unwrap().nodes);
    }
    let bounds = latest.get(&target).unwrap().bounds().unwrap();
    assert!(
        bounds.y0 >= 0. && bounds.y1 <= 430.,
        "Focused camera slider must be above diagnostics/footer: {bounds:?}"
    );
    assert!(
        outgoing.try_recv().is_err(),
        "Camera focus must not change subscriptions"
    );
}

#[test]
fn changing_tf_summary_text_does_not_resize_or_move_the_scene_viewport() {
    for width in [760.0, 1280.0] {
        let (mut app, outgoing) = connected();
        graph(&mut app);
        draft(&mut app);
        app.draft_clock_verified = true;
        app.subscribe();
        let scan = outgoing.try_recv().unwrap();
        ingest(&mut app, started(&scan));
        app.sidebar = false;
        app.inspector = true;
        let ctx = egui::Context::default();
        shell::configure(&ctx);
        ctx.enable_accesskit();
        ctx.style_mut(|style| style.animation_time = 0.0);
        let draw = |app: &mut App| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 560.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.render(ctx),
            );
            let tree = output.platform_output.accesskit_update.unwrap();
            let bounds = tree
                .nodes
                .iter()
                .find(|(_, node)| {
                    node.label()
                        .is_some_and(|label| label.starts_with("Combined 3D scene."))
                })
                .unwrap()
                .1
                .bounds()
                .unwrap();
            [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
        };
        app.views[0].spatial_status = "TF: Transform ready".into();
        draw(&mut app);
        let baseline = draw(&mut app);
        for status in [
            "TF: Transform unavailable at acquisition time",
            "TF: Waiting for transform",
            "TF: Transform lookup timed out",
            "TF: Transform ready",
        ] {
            app.views[0].spatial_status = status.into();
            assert_eq!(
                draw(&mut app),
                baseline,
                "Changing status must preserve viewport at width {width}: {status}"
            );
        }
        for diagnostics_open in [false, true] {
            app.diagnostics_open = diagnostics_open;
            draw(&mut app);
            let baseline = draw(&mut app);
            for changed in [true, false] {
                app.views[0].status = if changed {
                    ViewStatus::Receiving
                } else {
                    ViewStatus::Waiting
                };
                app.views[0].queue_drops = if changed { u64::MAX } else { 0 };
                app.views[0].received.backend_drops = if changed { u64::MAX } else { 0 };
                if changed {
                    app.diagnostic("A long synthetic backend diagnostic that must remain readable without resizing the spatial viewport as live health text changes".into());
                    app.views[0].error = Some("A long synthetic display error requiring recovery without moving the scene".into());
                } else {
                    app.diagnostics.clear();
                    app.views[0].error = None;
                }
                draw(&mut app);
                assert_eq!(draw(&mut app), baseline, "Live health/error text moved viewport at width {width}, diagnostics open: {diagnostics_open}");
            }
        }
        assert!(outgoing.try_recv().is_err());
    }
}

#[test]
fn retained_desktop_does_not_admit_maps_its_single_map_renderer_cannot_show() {
    let (app, _outgoing) = connected();
    let configuration = Configuration {
        topic: "/map".into(),
        message_type: studio_protocol::mapping::MAP_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: true,
        output: Output::OccupancyGrid,
        spatial: Some(studio_protocol::spatial::Configuration {
            fixed_frame: "map".into(),
            clock_policy: studio_protocol::spatial_input::SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }),
        robot: None,
        map_updates: None,
    };
    app.session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .registry
        .start_explicit(ViewId(1), configuration.clone())
        .unwrap();
    assert!(app
        .admission(&configuration, None)
        .unwrap_err()
        .contains("use Visualizer"));
    assert!(app.admission(&configuration, Some(ViewId(1))).is_ok());
}
