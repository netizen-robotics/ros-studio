use super::*;
use studio_protocol::{
    map_grid::{Continuity, Grid, Metadata, UpdateSource},
    map_snapshot::{Clear, ClearReason, Message, Owner, Snapshot, SourceStatus, CONTRACT_VERSION},
};
fn map_app() -> (App, Receiver<Command>) {
    let (mut app, outgoing) = connected();
    let mut topics = app.discovery.topics.clone();
    topics.extend([
        (
            "/map".into(),
            vec![studio_protocol::mapping::MAP_MESSAGE_TYPE.into()],
        ),
        (
            "/map_two".into(),
            vec![studio_protocol::mapping::MAP_MESSAGE_TYPE.into()],
        ),
        (
            "/updates".into(),
            vec![studio_protocol::map_grid::UPDATE_MESSAGE_TYPE.into()],
        ),
    ]);
    ingest(&mut app, Event::Graph { topics });
    (app, outgoing)
}
fn snapshot(command: &Command, revision: u64) -> Snapshot {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!("Start")
    };
    Snapshot::new(
        Owner {
            generation: *generation,
            subscription: *subscription,
            topic: configuration.topic.clone(),
            fixed_frame: "world".into(),
        },
        revision,
        Grid {
            metadata: Metadata {
                frame_id: "map".into(),
                stamp_ns: revision as i64,
                width: 2,
                height: 2,
                resolution: 1.,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
            },
            cells: vec![0, 50, 100, -1].into_boxed_slice(),
        },
        [0., 0., 0., 0., 0., 0., 1.],
        Default::default(),
        Default::default(),
        SourceStatus {
            continuity: if configuration.map_updates.is_some() {
                Continuity::Unknown
            } else {
                Continuity::Disabled
            },
            ..Default::default()
        },
    )
    .unwrap()
}
fn send(app: &mut App, command: &Command, sequence: u64, message: Message) {
    let Command::Start {
        generation,
        subscription,
        ..
    } = command
    else {
        panic!()
    };
    ingest(
        app,
        Event::MapSnapshotObservation {
            generation: *generation,
            subscription: *subscription,
            sequence,
            sample: Box::new(message),
            backend_drops: 0,
        },
    );
}
#[test]
fn map_drafts_and_local_appearance_do_not_replace_reception_and_clear_stop_hide_geometry() {
    let (mut app, outgoing) = map_app();
    app.begin_source(
        Output::MapSnapshot,
        "/map",
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.draft_map_updates = Some(UpdateSource {
        topic: "/updates".into(),
        best_effort: false,
        transient_local: false,
    });
    app.subscribe();
    let id = app.selected.unwrap();
    assert!(!app.views[0].enabled);
    assert!(outgoing.try_recv().is_err());
    app.set_display_enabled(id, true);
    let command = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&command));
    let map = snapshot(&command, 1);
    send(
        &mut app,
        &command,
        1,
        Message::Chunk(Box::new(map.chunk(0).unwrap())),
    );
    assert!(app.views[0].map.frame.is_some());
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
    let pointer = std::sync::Arc::as_ptr(app.views[0].map.frame.as_ref().unwrap());
    app.configure_view(id);
    app.discovery.select("/map_two");
    app.draft_map_updates = None;
    app.views[0].map.appearance = studio_workspace::map::Appearance {
        palette: studio_workspace::map::Palette::Raw,
        opacity_per_mille: 400,
        draw_behind: true,
    };
    assert_eq!(app.views[0].configuration.topic, "/map");
    assert!(app.views[0].configuration.map_updates.is_some());
    assert_eq!(
        std::sync::Arc::as_ptr(app.views[0].map.frame.as_ref().unwrap()),
        pointer
    );
    assert!(outgoing.try_recv().is_err());
    let owner = map.chunk(0).unwrap().header.owner;
    send(
        &mut app,
        &command,
        2,
        Message::Clear(Box::new(Clear {
            contract_version: CONTRACT_VERSION,
            owner,
            revision: 2,
            reason: ClearReason::QueueOverflow,
            metadata: None,
            source_status: SourceStatus {
                continuity: Continuity::Unknown,
                recovery_floor_ns: Some(2),
                ..Default::default()
            },
        })),
    );
    assert!(app.views[0].map.frame.is_none());
    assert_eq!(app.views[0].status, ViewStatus::Waiting);
    assert!(app.views[0].spatial_status.contains("fresh full"));
    let recovered = snapshot(&command, 3);
    send(
        &mut app,
        &command,
        3,
        Message::Chunk(Box::new(recovered.chunk(0).unwrap())),
    );
    assert!(app.views[0].map.frame.is_some());
    app.stop_view(id);
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[0].map.frame.is_none());
    assert!(app.views[0].map.stream.is_none());
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
        0
    );
    assert_eq!(app.views[0].map.appearance.opacity_per_mille, 400);
}
#[test]
fn map_workspace_v14_restores_exact_intent_without_activation_or_clock_permission() {
    let (mut app, outgoing) = map_app();
    app.begin_source(
        Output::MapSnapshot,
        "/map",
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.draft_map_updates = Some(UpdateSource {
        topic: "/updates".into(),
        best_effort: true,
        transient_local: true,
    });
    app.subscribe();
    app.views[0].map.appearance = studio_workspace::map::Appearance {
        palette: studio_workspace::map::Palette::Costmap,
        opacity_per_mille: 731,
        draw_behind: true,
    };
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        studio_workspace::application::VISUALIZER_VERSION
    );
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&imported.workspace).unwrap();
    let restored = &prepared.views[0];
    assert_eq!(restored.configuration.output, Output::MapSnapshot);
    assert_eq!(
        restored.configuration.map_updates,
        app.views[0].configuration.map_updates
    );
    assert_eq!(restored.map.appearance, app.views[0].map.appearance);
    assert!(!restored.enabled);
    assert!(!restored.spatial_clock_verified);
    assert!(restored.map.stream.is_none() && restored.map.frame.is_none());
    assert!(outgoing.try_recv().is_err());
}
#[test]
fn unavailable_update_source_cannot_allocate_map_or_clear_active_peer() {
    let (mut app, outgoing) = map_app();
    app.begin_source(
        Output::MapSnapshot,
        "/map",
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.draft_map_updates = Some(UpdateSource {
        topic: "/not_discovered".into(),
        best_effort: false,
        transient_local: false,
    });
    app.subscribe();
    let id = app.selected.unwrap();
    app.set_display_enabled(id, true);
    assert!(app.views[0].subscription.is_none());
    assert!(app.views[0]
        .error
        .as_ref()
        .unwrap()
        .contains("Update topic/type"));
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
        0
    );
}

#[test]
fn update_source_control_has_an_accessible_name_without_activating_a_draft() {
    let (mut app, outgoing) = map_app();
    app.begin_source(
        Output::MapSnapshot,
        "/map",
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.draft_map_updates = Some(UpdateSource {
        topic: "/updates".into(),
        best_effort: false,
        transient_local: false,
    });
    let context = egui::Context::default();
    context.enable_accesskit();
    let output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(360., 900.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(app.map_update_draft_ui(ui));
            });
        },
    );
    assert!(output
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .iter()
        .any(|(_, node)| node.label() == Some("Update topic")
            && node.role() == egui::accesskit::Role::ComboBox));
    assert!(app.views.is_empty());
    assert!(outgoing.try_recv().is_err());
}

#[test]
fn three_map_content_clear_stop_restore_and_stale_events_are_isolated() {
    let (mut app, outgoing) = map_app();
    let mut topics = app.discovery.topics.clone();
    for topic in [
        "/global_costmap/costmap",
        "/local_costmap/costmap",
        "/fourth_map",
    ] {
        topics.push((
            topic.into(),
            vec![studio_protocol::mapping::MAP_MESSAGE_TYPE.into()],
        ));
    }
    ingest(&mut app, Event::Graph { topics });
    let mut commands = Vec::new();
    let mut ids = Vec::new();
    for (index, topic) in ["/map", "/global_costmap/costmap", "/local_costmap/costmap"]
        .iter()
        .enumerate()
    {
        app.begin_source(
            Output::MapSnapshot,
            topic,
            studio_protocol::mapping::MAP_MESSAGE_TYPE,
        );
        app.subscribe();
        let id = app.selected.unwrap();
        app.set_display_enabled(id, true);
        let command = outgoing.try_recv().unwrap_or_else(|error| {
            panic!(
                "map{index} {error:?}: phase={:?} connection={} views={:?}",
                app.connection.phase,
                app.connection.status,
                app.views
                    .iter()
                    .map(|v| (&v.configuration.topic, &v.error, v.status))
                    .collect::<Vec<_>>()
            )
        });
        ingest(&mut app, started(&command));
        let map = snapshot(&command, index as u64 + 1);
        send(
            &mut app,
            &command,
            1,
            Message::Chunk(Box::new(map.chunk(0).unwrap())),
        );
        app.views[index].map.appearance.opacity_per_mille = 1000 - index as u16 * 200;
        commands.push(command);
        ids.push(id);
    }
    assert!(app
        .views
        .iter()
        .all(|v| v.map.frame.is_some() && v.status == ViewStatus::Receiving));
    let peer = app.views[0].map.frame.as_ref().unwrap().clone();
    app.begin_source(
        Output::MapSnapshot,
        "/fourth_map",
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.subscribe();
    let fourth = app.selected.unwrap();
    app.set_display_enabled(fourth, true);
    assert!(outgoing.try_recv().is_err());
    assert!(app.views[3]
        .error
        .as_ref()
        .unwrap()
        .contains("Map display limit"));
    let saved = app.workspace_snapshot().unwrap();
    let prepared = app.workspace_prepare(&saved).unwrap();
    assert!(prepared
        .views
        .iter()
        .all(|v| !v.enabled && v.map.frame.is_none()));
    assert_eq!(prepared.views[2].map.appearance.opacity_per_mille, 600);
    app.stop_view(ids[1]);
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[1].map.frame.is_none());
    assert!(std::sync::Arc::ptr_eq(
        app.views[0].map.frame.as_ref().unwrap(),
        &peer
    ));
    assert!(app.views[2].map.frame.is_some());
    app.set_display_enabled(fourth, true);
    assert!(
        outgoing.try_recv().is_err(),
        "Stopping owner reserves capacity"
    );
    ingest(&mut app, stopped(&stop));
    app.set_display_enabled(fourth, true);
    let replacement = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&replacement));
    let retired = snapshot(&commands[1], 2);
    send(
        &mut app,
        &commands[1],
        2,
        Message::Chunk(Box::new(retired.chunk(0).unwrap())),
    );
    assert!(
        app.views[1].map.frame.is_none(),
        "Retired stream must not revive"
    );
    let replacement_map = snapshot(&replacement, 1);
    send(
        &mut app,
        &replacement,
        1,
        Message::Chunk(Box::new(replacement_map.chunk(0).unwrap())),
    );
    assert!(app.views[3].map.frame.is_some());
    assert!(std::sync::Arc::ptr_eq(
        app.views[0].map.frame.as_ref().unwrap(),
        &peer
    ));
    assert_eq!(app.views[2].map.appearance.opacity_per_mille, 600);
}
