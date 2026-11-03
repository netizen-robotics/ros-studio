use super::*;
use std::sync::mpsc;
use studio_protocol::{
    latest_pose::LatestPose,
    marker::{Geometry, Key, Marker},
    marker_snapshot::{self as wire, Clock, Lifetime, Message, Owner, Received, Receiver},
    subscriptions::{Generation, SubscriptionId},
};
pub(crate) fn input(geometries: Vec<Geometry>, owner: u64) -> OwnerInput {
    input_with_lifetime(geometries, owner, 0)
}
fn input_with_lifetime(geometries: Vec<Geometry>, owner: u64, lifetime_ns: u64) -> OwnerInput {
    input_styled(geometries, owner, lifetime_ns, [0.1; 3])
}
fn input_styled(
    geometries: Vec<Geometry>,
    owner: u64,
    lifetime_ns: u64,
    scale: [f64; 3],
) -> OwnerInput {
    let wire_owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(owner),
        topic: format!("/markers{owner}"),
        message_type: studio_protocol::marker::ARRAY_MESSAGE_TYPE.into(),
        fixed_frame: "world".into(),
    };
    let entries: Vec<_> = geometries
        .into_iter()
        .enumerate()
        .map(|(i, geometry)| wire::Entry {
            marker: Marker {
                key: Key {
                    namespace: "objects".into(),
                    id: i as i32,
                },
                frame_id: "world".into(),
                stamp_ns: 0,
                pose: [0., 0., 0., 0., 0., 0., 1.],
                scale,
                color: [1.; 4],
                lifetime_ns: if i == 0 { lifetime_ns } else { 0 },
                frame_locked: false,
                geometry,
            },
            version: 1,
            transform: wire::Transform::Resolved {
                pose: LatestPose::Ready {
                    resolved_stamp_ns: 0,
                    components: [0., 0., 0., 0., 0., 0., 1.],
                },
            },
        })
        .collect();
    let snapshot = wire::Snapshot::new(
        wire::Header {
            contract_version: wire::CONTRACT_VERSION,
            owner: wire_owner.clone(),
            revision: 1,
            source_revision: 1,
            captured_at_unix_ns: 1,
            manifest: entries
                .iter()
                .map(|e| wire::ManifestEntry {
                    key: e.marker.key.clone(),
                    version: 1,
                    lifetime: if e.marker.lifetime_ns == 0 {
                        Lifetime::Forever
                    } else {
                        Lifetime::Expires {
                            remaining_ns: e.marker.lifetime_ns,
                            at_unix_ns: 1 + e.marker.lifetime_ns,
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
    .unwrap();
    let mut receiver = Receiver::new(wire_owner).unwrap();
    let mut state = None;
    for i in 0..snapshot.chunk_count() {
        if let Received::Complete {
            state: complete, ..
        } = receiver
            .receive(
                &Message::Chunk(Box::new(snapshot.chunk(i).unwrap())),
                Clock {
                    unix_ns: 1,
                    monotonic_ns: 0,
                },
            )
            .unwrap()
        {
            state = Some(complete);
        }
    }
    OwnerInput {
        owner,
        source_epoch: 1,
        revision: 1,
        origin: Instant::now(),
        state: state.unwrap(),
        context: (),
    }
}
fn request(geometries: Vec<Geometry>) -> PrepareRequest {
    PrepareRequest {
        scene_epoch: 1,
        owners: vec![input(geometries, 1)],
        assets: None,
    }
}
fn camera() -> CameraPose {
    CameraPose::orbit(0.5, 0.8, 8., [0.; 3])
}
fn ready<T>(worker: &mut LatestWorker<u64, T>, wakes: &mpsc::Receiver<()>) -> Completion<T>
where
    T: Send + 'static,
{
    wakes.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.poll().unwrap()
}
#[test]
fn active_work_finishes_while_pending_replaces_and_completed_results_coalesce() {
    let (wake_tx, wakes) = mpsc::channel();
    let (entered_tx, entered) = mpsc::channel();
    let (release_tx, release) = mpsc::channel();
    let mut worker = LatestWorker::new(
        "marker-latest-test",
        Arc::new(move || {
            wake_tx.send(()).unwrap();
        }),
        move |value, cancel| {
            assert!(!cancel.load(Ordering::Acquire));
            entered_tx.send(value).unwrap();
            release.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(!cancel.load(Ordering::Acquire));
            Ok(value)
        },
    )
    .unwrap();
    let first = worker.submit(1).unwrap();
    assert_eq!(entered.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
    worker.submit(2).unwrap();
    let latest = worker.submit(3).unwrap();
    assert_eq!(worker.replacements().requests, 1);
    release_tx.send(()).unwrap();
    assert_eq!(entered.recv_timeout(Duration::from_secs(5)).unwrap(), 3);
    let completion = ready(&mut worker, &wakes);
    assert_eq!(completion.id, first);
    assert_eq!(completion.result.unwrap(), 1);
    release_tx.send(()).unwrap();
    let completion = ready(&mut worker, &wakes);
    assert_eq!(completion.id, latest);
    assert_eq!(completion.result.unwrap(), 3);
    worker.submit(4).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    wakes.recv_timeout(Duration::from_secs(5)).unwrap();
    let last = worker.submit(5).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    wakes.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(worker.replacements().results, 1);
    assert_eq!(worker.poll().unwrap().id, last);
    worker.finish_shutdown().unwrap();
    assert!(worker.submit(6).is_err());
}
#[test]
fn explicit_cancel_discards_running_pending_and_already_queued_success_and_panic_recovers() {
    let (wake_tx, wakes) = mpsc::channel();
    let (entered_tx, entered) = mpsc::channel();
    let (release_tx, release) = mpsc::channel();
    let mut worker = LatestWorker::new(
        "marker-cancel-test",
        Arc::new(move || {
            let _ = wake_tx.send(());
        }),
        move |value, _| {
            entered_tx.send(value).unwrap();
            release.recv_timeout(Duration::from_secs(5)).unwrap();
            if value == 4 {
                panic!("injected decoder failure");
            }
            Ok(value)
        },
    )
    .unwrap();
    worker.submit(1).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.submit(2).unwrap();
    worker.cancel();
    let fresh = worker.submit(3).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(entered.recv_timeout(Duration::from_secs(5)).unwrap(), 3);
    assert!(worker.poll().is_none());
    release_tx.send(()).unwrap();
    wakes.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.cancel();
    assert!(worker.poll().is_none());
    let failed = worker.submit(4).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    let completion = ready(&mut worker, &wakes);
    assert_eq!(completion.id, failed);
    assert!(completion.result.unwrap_err().contains("retry explicitly"));
    let next = worker.submit(5).unwrap();
    assert!(next > fresh);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(ready(&mut worker, &wakes).result.unwrap(), 5);
    worker.finish_shutdown().unwrap();
}
#[test]
fn shutdown_cancels_running_and_releases_queued_jobs_without_joining_on_request() {
    let (entered_tx, entered) = mpsc::channel();
    let (release_tx, release) = mpsc::channel();
    let mut worker = LatestWorker::new(
        "marker-shutdown-test",
        Arc::new(|| {}),
        move |value, cancel| {
            entered_tx.send(value).unwrap();
            release.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(cancel.load(Ordering::Acquire));
            Ok(value)
        },
    )
    .unwrap();
    worker.submit(1).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.submit(2).unwrap();
    worker.request_shutdown();
    assert!(worker.submit(3).is_err());
    release_tx.send(()).unwrap();
    worker.finish_shutdown().unwrap();
    assert!(worker.poll().is_none());
    assert!(entered.try_recv().is_err());
    worker.finish_shutdown().unwrap();
}
#[test]
fn two_background_stages_prepare_all_supported_bodies_and_reuse_text_for_camera_changes() {
    let points = vec![[0., 0., 0.], [1., 0., 0.]];
    let geometries = vec![
        Geometry::Cube,
        Geometry::Sphere,
        Geometry::Cylinder,
        Geometry::Arrow { points: vec![] },
        Geometry::CubeList {
            points: points.clone(),
            colors: vec![],
        },
        Geometry::SphereList {
            points: points.clone(),
            colors: vec![],
        },
        Geometry::Points {
            points: points.clone(),
            colors: vec![],
        },
        Geometry::LineList {
            points: points.clone(),
            colors: vec![],
        },
        Geometry::LineStrip {
            points,
            colors: vec![],
        },
        Geometry::Triangles {
            points: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            colors: vec![],
        },
        Geometry::Text {
            text: "Robot 机器人".into(),
        },
    ];
    let (tx, rx) = mpsc::channel();
    let mut workers = Workers::new(Arc::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    let mut job = request(geometries);
    job.owners.push(input(vec![Geometry::Cube], 2));
    let id = workers.prepare(job).unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let prepared = workers.poll_prepared().unwrap();
    assert_eq!(prepared.id, id);
    let scene = prepared.result.unwrap();
    assert_eq!(scene.key_count(), 12);
    assert!(scene.diagnostics.is_empty());
    assert_eq!(scene.owners.len(), 2);
    let layout = scene
        .items
        .iter()
        .find_map(|i| {
            if let Drawing::Text(t) = &i.drawing {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    let atlas = Arc::downgrade(layout.atlas());
    for (n, size) in [[800, 600], [1200, 400]].into_iter().enumerate() {
        workers
            .project(ProjectionRequest {
                scene: scene.clone(),
                camera: CameraPose::orbit(n as f32, 0.8, 8., [0.; 3]),
                size,
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let output = workers.poll_projected().unwrap().result.unwrap();
        assert!(Arc::ptr_eq(&scene, &output.scene));
        assert!(output.frame.len() > 12);
        assert!(output
            .frame
            .has_visible(Some(&output.frame.permit(|_| true))));
        assert!(atlas.upgrade().is_some());
    }
    workers.finish_shutdown().unwrap();
}
#[test]
fn missing_and_unapproved_meshes_are_key_scoped_and_policy_epochs_cannot_be_reused() {
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let cancel = AtomicBool::new(false);
    let job = || {
        request(vec![
            Geometry::Cube,
            Geometry::Mesh {
                resource: "missing.obj".into(),
                use_embedded_materials: true,
            },
        ])
    };
    let scene = preparation.run(job(), &cancel).unwrap();
    assert_eq!(scene.key_count(), 1);
    assert_eq!(scene.diagnostics.len(), 1);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    let policy = Arc::new(AssetPolicy::new(1, Some(root.clone()), BTreeMap::new()).unwrap());
    let mut reviewed = job();
    reviewed.assets = Some(policy.clone());
    let scene = preparation.run(reviewed, &cancel).unwrap();
    assert_eq!(scene.key_count(), 1);
    assert_eq!(scene.diagnostics.len(), 1);
    let mut changed = job();
    changed.assets = Some(Arc::new(
        AssetPolicy::new(1, None, BTreeMap::new()).unwrap(),
    ));
    assert!(preparation
        .run(changed, &cancel)
        .unwrap_err()
        .contains("without a new epoch"));
    preparation.run(job(), &cancel).unwrap();
    let mut retired = job();
    retired.assets = Some(policy);
    assert!(preparation
        .run(retired, &cancel)
        .unwrap_err()
        .contains("retired"));
    let mut reloaded = job();
    reloaded.assets = Some(Arc::new(
        AssetPolicy::new(2, Some(root), BTreeMap::new()).unwrap(),
    ));
    assert_eq!(preparation.run(reloaded, &cancel).unwrap().key_count(), 1);
}
#[test]
fn loaded_mesh_preparation_and_projection_share_reviewed_robot_assets_and_rebase_expiry() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    let mut job = request(vec![
        Geometry::Mesh {
            resource: "materials/textured.obj".into(),
            use_embedded_materials: true,
        },
        Geometry::Cube,
    ]);
    job.assets = Some(Arc::new(
        AssetPolicy::new(1, Some(root), BTreeMap::new()).unwrap(),
    ));
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let cancel = AtomicBool::new(false);
    let scene = preparation.run(job.clone(), &cancel).unwrap();
    assert_eq!(scene.key_count(), 2);
    assert!(scene.diagnostics.is_empty());
    let again = preparation.run(job, &cancel).unwrap();
    let mesh = |scene: &PreparedScene| match &scene.items[0].drawing {
        Drawing::Mesh(mesh) => mesh.parts[0].texture.clone().unwrap(),
        _ => panic!(),
    };
    assert!(Arc::ptr_eq(&mesh(&scene), &mesh(&again)));
    let output = project(
        ProjectionRequest {
            scene: scene.clone(),
            camera: camera(),
            size: [800, 600],
        },
        &cancel,
    )
    .unwrap();
    assert!(output.frame.len() > 1);
    assert!(output
        .frame
        .has_visible(Some(&output.frame.permit(|key| key.asset_epoch == Some(1)))));
    let mut expired = (*scene).clone();
    expired.items[0].deadline = Some(0);
    let output = project(
        ProjectionRequest {
            scene: Arc::new(expired),
            camera: camera(),
            size: [800, 600],
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(output.frame.len(), 1);
    assert!(!output
        .frame
        .has_visible(Some(&output.frame.permit(|key| key.asset_epoch.is_some()))));
}
#[test]
fn malformed_owner_and_camera_requests_fail_even_when_empty_and_cancel_is_atomic() {
    assert!(AssetPolicy::new(0, None, BTreeMap::new()).is_err());
    assert!(AssetPolicy::new(
        1,
        None,
        BTreeMap::from([(
            "x".repeat(crate::robot::MAX_NAME_BYTES + 1),
            PathBuf::from(".")
        )])
    )
    .is_err());
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let cancel = AtomicBool::new(false);
    let mut duplicate = request(vec![Geometry::Cube]);
    duplicate.owners.push(duplicate.owners[0].clone());
    assert!(preparation.run(duplicate, &cancel).is_err());
    let mut invalid = request(vec![]);
    invalid.scene_epoch = 0;
    assert!(preparation.run(invalid, &cancel).is_err());
    let scene = preparation.run(request(vec![]), &cancel).unwrap();
    assert!(project(
        ProjectionRequest {
            scene: scene.clone(),
            camera: camera(),
            size: [0, 600]
        },
        &cancel
    )
    .is_err());
    cancel.store(true, Ordering::Release);
    assert!(project(
        ProjectionRequest {
            scene,
            camera: camera(),
            size: [800, 600]
        },
        &cancel
    )
    .is_err());
    assert!(preparation
        .run(request(vec![Geometry::Cube]), &cancel)
        .is_err());
}
#[test]
fn receiver_deadlines_keep_the_same_absolute_instant_across_worker_clock_origins() {
    let mut owner = input_with_lifetime(vec![Geometry::Cube, Geometry::Sphere], 1, 3_000_000_000);
    owner.origin = Instant::now() - Duration::from_secs(2);
    let clock = MarkerClock::new();
    let expected = clock.deadline_from(owner.origin, 3_000_000_000).unwrap();
    let mut preparation = preparation::Preparation::new(clock);
    let scene = preparation
        .run(
            PrepareRequest {
                scene_epoch: 1,
                owners: vec![owner],
                assets: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(scene.items[0].deadline, Some(expected));
    assert_eq!(scene.items[1].deadline, None);
    assert!(expected <= 1_000_000_000);
}
#[test]
fn captured_source_context_survives_both_stages_without_frontend_dependencies() {
    let owner = input(vec![Geometry::Cube], 1);
    let observation = Arc::new(("captured visibility", 7u64));
    let (tx, rx) = mpsc::channel();
    let mut workers = Workers::new(Arc::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    workers
        .prepare(PrepareRequest {
            scene_epoch: 1,
            assets: None,
            owners: vec![OwnerInput {
                owner: owner.owner,
                source_epoch: owner.source_epoch,
                revision: owner.revision,
                origin: owner.origin,
                state: owner.state,
                context: observation.clone(),
            }],
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let scene = workers.poll_prepared().unwrap().result.unwrap();
    assert!(Arc::ptr_eq(&scene.owners[0].context, &observation));
    workers
        .project(ProjectionRequest {
            scene,
            camera: camera(),
            size: [128, 128],
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let output = workers.poll_projected().unwrap().result.unwrap();
    assert!(Arc::ptr_eq(&output.scene.owners[0].context, &observation));
    assert_eq!(output.scene.owners[0].revision, 1);
    workers.finish_shutdown().unwrap();
}
#[test]
fn unbounded_billboard_world_extent_is_key_scoped_before_projection_and_fit() {
    let owner = input_styled(
        vec![
            Geometry::Text {
                text: "A".repeat(4096),
            },
            Geometry::Cube,
        ],
        1,
        0,
        [0.1, 0.1, 10_000.],
    );
    let cancel = AtomicBool::new(false);
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let scene = preparation
        .run(
            PrepareRequest {
                scene_epoch: 1,
                owners: vec![owner],
                assets: None,
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(scene.key_count(), 1);
    assert_eq!(scene.diagnostics.len(), 1);
    assert_eq!(scene.diagnostics[0].key.id, 0);
    let bounds = scene.bounds(|_| true).unwrap();
    assert_eq!(bounds.max[2], 5000.);
    assert!(project(
        ProjectionRequest {
            scene,
            camera: camera(),
            size: [800, 600]
        },
        &cancel
    )
    .is_ok());
}
#[test]
fn mesh_cache_keeps_successes_and_failures_immutable_until_explicit_reload() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../artifacts/feasibility/evidence/slice5c2c3-cache-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let cancel = AtomicBool::new(false);
    let job = |epoch| {
        let mut job = request(vec![
            Geometry::Cube,
            Geometry::Mesh {
                resource: "appears.obj".into(),
                use_embedded_materials: false,
            },
        ]);
        job.assets = Some(Arc::new(
            AssetPolicy::new(epoch, Some(root.clone()), BTreeMap::new()).unwrap(),
        ));
        job
    };
    assert_eq!(preparation.run(job(1), &cancel).unwrap().key_count(), 1);
    std::fs::write(
        root.join("appears.obj"),
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
    )
    .unwrap();
    assert_eq!(preparation.run(job(1), &cancel).unwrap().key_count(), 1);
    let loaded = preparation.run(job(2), &cancel).unwrap();
    assert_eq!(loaded.key_count(), 2);
    std::fs::write(root.join("appears.obj"), "invalid mesh").unwrap();
    assert_eq!(preparation.run(job(2), &cancel).unwrap().key_count(), 2);
    assert_eq!(preparation.run(job(3), &cancel).unwrap().key_count(), 1);
}
#[test]
fn mesh_cache_churn_is_bounded_across_both_owners_and_reload_recovers() {
    let mut preparation = preparation::Preparation::new(MarkerClock::new());
    let cancel = AtomicBool::new(false);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    let geometries = |start| {
        (start..start + 256)
            .map(|i| Geometry::Mesh {
                resource: format!("missing-{i}.obj"),
                use_embedded_materials: false,
            })
            .collect()
    };
    let policy = Arc::new(AssetPolicy::new(1, Some(root.clone()), BTreeMap::new()).unwrap());
    let scene = preparation
        .run(
            PrepareRequest {
                scene_epoch: 1,
                owners: vec![input(geometries(0), 1), input(geometries(256), 2)],
                assets: Some(policy.clone()),
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(scene.key_count(), 0);
    assert_eq!(scene.diagnostics.len(), 512);
    let mut exhausted = request(vec![Geometry::Mesh {
        resource: "another.obj".into(),
        use_embedded_materials: false,
    }]);
    exhausted.assets = Some(policy);
    assert!(preparation
        .run(exhausted.clone(), &cancel)
        .unwrap_err()
        .contains("cache full"));
    exhausted.assets = Some(Arc::new(
        AssetPolicy::new(2, Some(root), BTreeMap::new()).unwrap(),
    ));
    let scene = preparation.run(exhausted, &cancel).unwrap();
    assert_eq!(scene.diagnostics.len(), 1);
}

#[test]
#[ignore = "explicit serialized optimized maximum-source projection timing; no GPU or app FPS claim"]
fn maximum_source_projection_probe() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../artifacts/feasibility/evidence/slice5c2c3-projection-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let obj = format!("v 0 0 0\nv 1 0 0\nv 0 1 0\n{}", "f 1 2 3\n".repeat(50_000));
    std::fs::write(root.join("maximum.obj"), obj).unwrap();
    let geometries = |mesh| {
        let mut entries = vec![
            Geometry::Points {
                points: vec![[0.1, 0.2, 0.3]; 4096],
                colors: vec![]
            };
            16
        ];
        entries.extend(vec![
            Geometry::Text {
                text: "A".repeat(4096)
            };
            16
        ]);
        if mesh {
            entries.push(Geometry::Mesh {
                resource: "maximum.obj".into(),
                use_embedded_materials: false,
            });
        }
        entries.resize(256, Geometry::Arrow { points: vec![] });
        entries
    };
    let owners = vec![input(geometries(true), 1), input(geometries(false), 2)];
    let (tx, rx) = mpsc::channel();
    let mut workers = Workers::new(Arc::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    workers
        .prepare(PrepareRequest {
            scene_epoch: 1,
            owners,
            assets: Some(Arc::new(
                AssetPolicy::new(1, Some(root.clone()), BTreeMap::new()).unwrap(),
            )),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(60)).unwrap();
    let scene = workers.poll_prepared().unwrap().result.unwrap();
    assert_eq!(scene.key_count(), 512);
    assert!(scene.diagnostics.is_empty());
    let mut times = vec![];
    let mut instances = 0;
    for angle in [0., 0.5, 1., 1.5, 2.] {
        workers
            .project(ProjectionRequest {
                scene: scene.clone(),
                camera: CameraPose::orbit(angle, 0.8, 8., [0.; 3]),
                size: [1920, 1080],
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(60)).unwrap();
        let frame = workers.poll_projected().unwrap().result.unwrap();
        instances = frame.frame.len();
        assert_eq!(instances, 313_038);
        times.push(frame.projection_time.as_secs_f64() * 1000.);
    }
    workers.finish_shutdown().unwrap();
    println!(
        "Marker maximum source projection: {}",
        serde_json::json!({"keys":512,"instances":instances,"points":131072,"glyphs":131072,"mesh_triangles":50000,"arrows":447,"preparation_ms":scene.preparation_time.as_secs_f64()*1000.,"projection_ms":times,"generated_asset":root.join("maximum.obj"),"scope":"worker-only source/prepare/project, no GPU upload/drawing or native app FPS"})
    );
}
