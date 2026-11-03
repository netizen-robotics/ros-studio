use super::*;
use studio_protocol::{
    marker::{self, Geometry, Key, Marker},
    marker_snapshot::{self as wire, Message},
};
fn marker_app() -> (App, Receiver<Command>) {
    let (mut app, outgoing) = connected();
    let mut topics = app.discovery.topics.clone();
    topics.extend([
        ("/markers".into(), vec![marker::MESSAGE_TYPE.into()]),
        ("/array".into(), vec![marker::ARRAY_MESSAGE_TYPE.into()]),
        ("/other".into(), vec![marker::MESSAGE_TYPE.into()]),
    ]);
    ingest(&mut app, Event::Graph { topics });
    app.shared_scene.marker_request = Some((spatial::Camera::default().pose(), [800, 600]));
    (app, outgoing)
}
fn activate(
    app: &mut App,
    outgoing: &Receiver<Command>,
    topic: &str,
    ty: &str,
) -> (ViewId, Command) {
    let id = add(app, Output::MarkerSnapshot, topic, ty);
    assert!(!app.views.iter().find(|v| v.id == id).unwrap().enabled);
    assert!(outgoing.try_recv().is_err());
    app.set_display_enabled(id, true);
    let command = outgoing.recv_timeout(Duration::from_secs(1)).unwrap();
    ingest(app, started(&command));
    (id, command)
}
fn snapshot(command: &Command, revision: u64, ids: &[i32]) -> wire::Snapshot {
    snapshot_lifetime(command, revision, ids, 0)
}
fn snapshot_lifetime(
    command: &Command,
    revision: u64,
    ids: &[i32],
    lifetime: u64,
) -> wire::Snapshot {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!()
    };
    let entries: Vec<_> = ids
        .iter()
        .map(|&id| wire::Entry {
            marker: Marker {
                key: Key {
                    namespace: "objects".into(),
                    id,
                },
                frame_id: "world".into(),
                stamp_ns: 0,
                pose: [id as f64, 0., 0., 0., 0., 0., 1.],
                scale: [1.; 3],
                color: [1.; 4],
                lifetime_ns: if id == 1 { lifetime } else { 0 },
                frame_locked: false,
                geometry: Geometry::Cube,
            },
            version: revision,
            transform: wire::Transform::Resolved {
                pose: studio_protocol::latest_pose::LatestPose::Ready {
                    resolved_stamp_ns: 0,
                    components: [0., 0., 0., 0., 0., 0., 1.],
                },
            },
        })
        .collect();
    let captured = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    wire::Snapshot::new(
        wire::Header {
            contract_version: wire::CONTRACT_VERSION,
            owner: wire::Owner {
                generation: *generation,
                subscription: *subscription,
                topic: configuration.topic.clone(),
                message_type: configuration.message_type.clone(),
                fixed_frame: "world".into(),
            },
            revision,
            source_revision: revision,
            captured_at_unix_ns: captured,
            manifest: entries
                .iter()
                .map(|e| wire::ManifestEntry {
                    key: e.marker.key.clone(),
                    version: e.version,
                    lifetime: if e.marker.lifetime_ns == 0 {
                        wire::Lifetime::Forever
                    } else {
                        wire::Lifetime::Expires {
                            remaining_ns: e.marker.lifetime_ns,
                            at_unix_ns: captured + e.marker.lifetime_ns,
                        }
                    },
                })
                .collect(),
            document_bytes: 1,
            sha256: "0".repeat(64),
        },
        wire::State {
            entries,
            diagnostics: vec![],
            counters: Default::default(),
            source_counters: Default::default(),
            known_history_gap: false,
            loss_support: wire::LossSupport::Unknown,
        },
    )
    .unwrap()
}
fn send(app: &mut App, command: &Command, sequence: u64, revision: u64, ids: &[i32]) {
    let snap = snapshot(command, revision, ids);
    assert_eq!(snap.chunk_count(), 1);
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
        Event::MarkerSnapshotObservation {
            generation: *generation,
            subscription: *subscription,
            sequence,
            sample: Box::new(Message::Chunk(Box::new(snap.chunk(0).unwrap()))),
            backend_drops: 0,
        },
    );
}
fn wait(app: &mut App, mut predicate: impl FnMut(&App) -> bool) {
    let ctx = egui::Context::default();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        app.poll_markers(&ctx);
        if predicate(app) {
            return;
        }
        assert!(
            Instant::now() < until,
            "Marker app did not progress: {:?}",
            app.markers.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn visible_count(app: &App) -> usize {
    app.markers
        .projected
        .as_ref()
        .zip(app.marker_permit())
        .map_or(0, |(output, permit)| {
            output
                .frame
                .opaque_visible(Some(&permit))
                .map(|(_, r)| r.len())
                .sum()
        })
}
#[test]
fn explicit_sources_prepare_off_ui_and_clear_stop_preserve_cached_peers() {
    let (mut app, outgoing) = marker_app();
    let (a, start_a) = activate(&mut app, &outgoing, "/markers", marker::MESSAGE_TYPE);
    let (b, start_b) = activate(&mut app, &outgoing, "/array", marker::ARRAY_MESSAGE_TYPE);
    send(&mut app, &start_a, 1, 1, &[1, 2]);
    send(&mut app, &start_b, 1, 1, &[3]);
    wait(&mut app, |a| visible_count(a) == 3);
    assert_eq!(app.marker_bounds(None).unwrap().max[0], 3.5);
    assert_eq!(app.marker_bounds(Some(a.0)).unwrap().max[0], 2.5);
    let frame = app.markers.projected.as_ref().unwrap().frame.clone();
    let peer = app
        .views
        .iter()
        .find(|v| v.id == b)
        .unwrap()
        .marker
        .stream
        .as_ref()
        .unwrap()
        .clone();
    app.clear_marker(a);
    assert_eq!(visible_count(&app), 1);
    assert!(app.marker_bounds(Some(a.0)).is_none());
    assert_eq!(app.marker_bounds(None).unwrap().min[0], 2.5);
    assert!(!peer.is_stopped());
    assert_eq!(
        frame.len(),
        3,
        "Visibility must hide cached keys without rebuilding them"
    );
    let Command::ClearMarkers {
        generation,
        subscription,
        request,
    } = outgoing.try_recv().unwrap()
    else {
        panic!()
    };
    ingest(
        &mut app,
        Event::MarkersCleared {
            generation,
            subscription,
            request,
        },
    );
    assert!(app
        .views
        .iter()
        .find(|v| v.id == a)
        .unwrap()
        .marker
        .state
        .is_none());
    send(&mut app, &start_a, 2, 2, &[4]);
    wait(&mut app, |a| visible_count(a) == 2);
    app.stop_view(a);
    assert_eq!(visible_count(&app), 1);
    assert!(matches!(outgoing.try_recv(), Ok(Command::Stop { .. })));
    app.stop_view(b);
    assert_eq!(visible_count(&app), 0);
    assert!(peer.is_stopped());
    app.markers.finish_shutdown().unwrap();
}
#[test]
fn clear_rejection_keeps_data_hidden_and_only_matching_retry_ack_recovers() {
    let (mut app, outgoing) = marker_app();
    let (id, start) = activate(&mut app, &outgoing, "/markers", marker::MESSAGE_TYPE);
    send(&mut app, &start, 1, 1, &[1]);
    wait(&mut app, |a| visible_count(a) == 1);
    app.clear_marker(id);
    let Command::ClearMarkers {
        generation,
        subscription,
        request,
    } = outgoing.try_recv().unwrap()
    else {
        panic!()
    };
    ingest(
        &mut app,
        Event::RequestError {
            generation,
            subscription,
            request,
            message: "Clear lane full".into(),
            no_handle: true,
        },
    );
    assert!(app.views[0].marker.clear_request.is_none());
    assert!(app.views[0]
        .marker
        .stream
        .as_ref()
        .unwrap()
        .is_waiting_for_clear());
    assert_eq!(visible_count(&app), 0);
    app.clear_marker(id);
    let Command::ClearMarkers { request: fresh, .. } = outgoing.try_recv().unwrap() else {
        panic!()
    };
    ingest(
        &mut app,
        Event::MarkersCleared {
            generation,
            subscription,
            request,
        },
    );
    assert_eq!(app.views[0].marker.clear_request, Some(fresh));
    ingest(
        &mut app,
        Event::MarkersCleared {
            generation,
            subscription,
            request: fresh,
        },
    );
    send(&mut app, &start, 2, 2, &[2]);
    wait(&mut app, |a| visible_count(a) == 1);
    app.markers.finish_shutdown().unwrap();
}
#[test]
fn third_owner_is_rejected_and_source_or_scene_changes_deny_cached_work() {
    let (mut app, outgoing) = marker_app();
    let (id, start) = activate(&mut app, &outgoing, "/markers", marker::MESSAGE_TYPE);
    let (_, array) = activate(&mut app, &outgoing, "/array", marker::ARRAY_MESSAGE_TYPE);
    send(&mut app, &start, 1, 1, &[1]);
    send(&mut app, &array, 1, 1, &[2]);
    wait(&mut app, |a| visible_count(a) == 2);
    let third = add(
        &mut app,
        Output::MarkerSnapshot,
        "/other",
        marker::MESSAGE_TYPE,
    );
    app.set_display_enabled(third, true);
    assert!(outgoing.try_recv().is_err());
    assert!(app
        .views
        .iter()
        .find(|v| v.id == third)
        .unwrap()
        .error
        .is_some());
    let v = app.views.iter_mut().find(|v| v.id == id).unwrap();
    v.marker.source_epoch += 1;
    assert_eq!(visible_count(&app), 1);
    app.markers.scene_epoch += 1;
    assert_eq!(visible_count(&app), 0);
    app.markers.finish_shutdown().unwrap();
}
#[test]
fn marker_workspace_v15_preserves_intent_and_restores_inactive_without_mesh_authority() {
    let (mut app, outgoing) = marker_app();
    let (id, start) = activate(&mut app, &outgoing, "/array", marker::ARRAY_MESSAGE_TYPE);
    send(&mut app, &start, 1, 1, &[10]);
    wait(&mut app, |a| visible_count(a) == 1);
    app.markers
        .review(crate::marker_display::AssetDraft {
            root: std::env::temp_dir()
                .join("reviewed/local/meshes")
                .to_string_lossy()
                .into_owned(),
            packages: vec![crate::components::workspace::PackageRoot {
                package: "approved_fixture_package".into(),
                path: std::env::temp_dir()
                    .join("approved/packageonly/meshes")
                    .to_string_lossy()
                    .into_owned(),
            }],
        })
        .unwrap();
    let workspace = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &workspace,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        wire["version"],
        studio_workspace::application::VISUALIZER_VERSION
    );
    assert_eq!(wire["views"][0]["kind"]["renderer"], "markers");
    let text = String::from_utf8(bytes.clone()).unwrap();
    for absent in [
        "reviewed/local/meshes",
        "approved_fixture_package",
        "/approved/packageonly/meshes",
        "source_epoch",
        "subscription",
        "clock_verified",
        "enabled",
    ] {
        assert!(!text.contains(absent));
    }
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&imported.workspace).unwrap();
    let view = &prepared.views[0];
    assert!(!view.enabled);
    assert!(!view.spatial_clock_verified);
    assert!(view.marker.stream.is_none());
    assert!(view.marker.state.is_none());
    assert_eq!(view.configuration.message_type, marker::ARRAY_MESSAGE_TYPE);
    app.stop_view(id);
    app.markers.finish_shutdown().unwrap();
}
#[test]
fn marker_enable_allows_static_only_or_later_tf_publishers() {
    let (mut app, outgoing) = marker_app();
    let topics = app
        .discovery
        .topics
        .iter()
        .filter(|(t, _)| t != "/tf")
        .cloned()
        .collect();
    ingest(&mut app, Event::Graph { topics });
    let id = add(
        &mut app,
        Output::MarkerSnapshot,
        "/markers",
        marker::MESSAGE_TYPE,
    );
    app.set_display_enabled(id, true);
    assert!(
        matches!(outgoing.try_recv(), Ok(Command::Start { .. })),
        "Configured TF readers must be allowed to wait for future publishers: {:?}",
        app.views[0].error
    );
}

#[test]
fn marker_status_excludes_idle_expiry_without_new_snapshot_and_preserves_live_peer() {
    let (mut app, outgoing) = marker_app();
    let (id, command) = activate(&mut app, &outgoing, "/markers", marker::MESSAGE_TYPE);
    let snap = snapshot_lifetime(&command, 1, &[1, 2], 100_000_000);
    let Command::Start {
        generation,
        subscription,
        ..
    } = command
    else {
        panic!()
    };
    ingest(
        &mut app,
        Event::MarkerSnapshotObservation {
            generation,
            subscription,
            sequence: 1,
            sample: Box::new(Message::Chunk(Box::new(snap.chunk(0).unwrap()))),
            backend_drops: 0,
        },
    );
    wait(&mut app, |a| {
        a.views
            .iter()
            .find(|v| v.id == id)
            .unwrap()
            .marker
            .state
            .is_some()
    });
    let view = app.views.iter().find(|v| v.id == id).unwrap();
    assert_eq!(view.spatial_status, "2 markers · 2 with a ready transform");
    let state = view.marker.state.clone().unwrap();
    let deadline = state
        .entries()
        .iter()
        .find_map(|e| e.expires_at_monotonic_ns)
        .unwrap();
    wait(&mut app, |a| {
        a.views
            .iter()
            .find(|v| v.id == id)
            .unwrap()
            .marker
            .stream
            .as_ref()
            .unwrap()
            .monotonic_ns()
            >= deadline
    });
    let view = app.views.iter().find(|v| v.id == id).unwrap();
    assert_eq!(view.spatial_status, "2 markers · 1 with a ready transform");
    assert_eq!(view.status, ViewStatus::Receiving);
    assert!(std::sync::Arc::ptr_eq(
        &state,
        view.marker.state.as_ref().unwrap()
    ));
    assert!(
        outgoing.try_recv().is_err(),
        "Idle expiry must not replace or restart reception"
    );
}

#[test]
fn simultaneous_orbit_and_camera_projection_keep_independent_frames_and_permissions() {
    let (mut app, outgoing) = marker_app();
    let (_, command) = activate(&mut app, &outgoing, "/markers", marker::MESSAGE_TYPE);
    let camera = add(
        &mut app,
        Output::CameraSnapshot(Box::new(studio_protocol::camera_snapshot::InfoSource {
            topic: "/camera_info".into(),
            best_effort: false,
            transient_local: false,
        })),
        "/camera",
        images::IMAGE_MESSAGE_TYPE,
    );
    // Projection scheduling only; no Camera samples or live activation needed.
    app.views
        .iter_mut()
        .find(|v| v.id == camera)
        .unwrap()
        .enabled = true;
    app.companion_selected = Some(camera);
    app.companion_open = true;
    let optical = spatial::Camera {
        target: [3., 2., 1.],
        ..Default::default()
    };
    let camera_request = (optical.pose(), [400, 220]);
    app.camera_canvas.marker_request = Some(camera_request);
    send(&mut app, &command, 1, 1, &[1, 2]);
    wait(&mut app, |a| {
        a.markers.projected.is_some() && a.markers.camera_projected.is_some()
    });
    assert_eq!(app.markers.projected.as_ref().unwrap().size, [800, 600]);
    assert_eq!(
        app.markers.camera_projected.as_ref().unwrap().size,
        camera_request.1
    );
    assert_eq!(
        app.markers.camera_projected.as_ref().unwrap().camera,
        camera_request.0
    );
    assert!(app.marker_permit_for(true).is_some());
    let old = app.markers.camera_projected.as_ref().unwrap();
    let permit = app.marker_permit_for(true).unwrap();
    assert!(old.frame.opaque_visible(Some(&permit)).next().is_some());
    app.clear_marker(ViewId(1));
    let permit = app.marker_permit_for(true).unwrap();
    assert!(app
        .markers
        .camera_projected
        .as_ref()
        .unwrap()
        .frame
        .opaque_visible(Some(&permit))
        .next()
        .is_none());
}
