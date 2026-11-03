//! Serialized complete App/production ingress/egui/Metal maximum-history load.
use super::*;
use crate::{
    display_tests::{ingest, started, stopped},
    performance_support::{gpu, percentile, rss, Surface},
};
use serde_json::json;
use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{
        self as pose,
        transport::{Configuration, Message},
    },
    uncertainty::Covariance,
};

fn snapshot(command: &Command, revision: u64) -> pose::Snapshot {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!()
    };
    let Output::PoseSnapshot(settings) = configuration.output else {
        panic!()
    };
    let first = revision * 300;
    let entry = |sequence: u64| {
        let angle = sequence as f64 * 0.025;
        let stamp = sequence as i64 + 1;
        pose::Entry {
            sequence,
            acquisition_stamp_ns: stamp,
            components: [
                angle.cos() * 3.,
                angle.sin() * 3.,
                0.2,
                0.,
                0.,
                (angle * 0.5).sin(),
                (angle * 0.5).cos(),
            ],
            covariance: Covariance::new(std::array::from_fn(|i| {
                if i / 6 == i % 6 {
                    0.04
                } else {
                    0.
                }
            }))
            .unwrap(),
            transform: LatestPose::Ready {
                resolved_stamp_ns: stamp,
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        }
    };
    pose::Snapshot {
        contract_version: 1,
        owner: pose::Owner {
            generation: *generation,
            subscription: *subscription,
            kind: settings.kind,
            topic: configuration.topic.clone(),
            fixed_frame: "world".into(),
        },
        revision,
        continuity_epoch: 1,
        source: Some(pose::Source {
            frame_id: "world".into(),
            child_frame_id: (settings.kind == pose::Kind::Odometry).then(|| "base".into()),
        }),
        history: settings.history.map(|h| h.settings().unwrap()),
        latest: Some(entry(first + 257)),
        trail: if settings.kind == pose::Kind::Odometry {
            (1..=256).map(|i| entry(first + i)).collect()
        } else {
            vec![]
        },
        last_reset: None,
        known_history_gap: false,
        counters: pose::Counters {
            received: first + 257,
            ..Default::default()
        },
    }
}
#[test]
#[ignore = "Serialized release actual App/production ingress/egui/Metal 60s; STUDIO_UNCERTAINTY_PERFORMANCE_EVIDENCE required"]
fn maximum_uncertainty_app_metal_load_and_lifecycle() {
    let folder = PathBuf::from(std::env::var("STUDIO_UNCERTAINTY_PERFORMANCE_EVIDENCE").unwrap());
    assert!(
        folder.is_absolute()
            && folder.starts_with(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../artifacts/feasibility/evidence")
                    .canonicalize()
                    .unwrap()
            )
    );
    let state = gpu();
    let surface = Surface::new(state.clone());
    let (session, outgoing) = session::Session::fixture(0);
    let shared = session.shared.clone();
    // App owns shutdown cleanup through Drop; initialise then assign rather than
    // attempting to move the remaining fields out of a temporary App.
    let mut app = App::default();
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    let (reader, mut control) = UnixStream::pair().unwrap();
    control
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (terminal_tx, terminal) = std::sync::mpsc::sync_channel(1);
    let reader = backend_pipe::spawn(reader, shared, terminal_tx);
    control
        .write_all(
            &Event::ready("jazzy".into(), "fixture-no-ros".into())
                .encode()
                .unwrap(),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.connection.phase != Phase::Connected {
        app.poll_backend();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    ingest(
        &mut app,
        Event::Graph {
            topics: vec![
                (
                    "/maximum_covariance".into(),
                    vec![pose::POSE_MESSAGE_TYPE.into()],
                ),
                (
                    "/maximum_odometry".into(),
                    vec![pose::ODOMETRY_MESSAGE_TYPE.into()],
                ),
            ],
        },
    );
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.spatial_renderer = Some(spatial::Renderer3d::with_sensor_history(state.clone()));
    let mut starts = vec![];
    let mut ids = vec![];
    for (kind, topic) in [
        (pose::Kind::PoseWithCovariance, "/maximum_covariance"),
        (pose::Kind::Odometry, "/maximum_odometry"),
    ] {
        let mut config = Configuration::for_kind(kind);
        if let Some(h) = &mut config.history {
            h.keep = 256;
        }
        app.begin_source(Output::PoseSnapshot(config), topic, kind.message_type());
        app.subscribe();
        let id = app.selected.unwrap();
        app.set_display_enabled(id, true);
        let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
        ingest(&mut app, started(&command));
        app.views
            .iter_mut()
            .find(|v| v.id == id)
            .unwrap()
            .uncertainty
            .appearance
            .history_covariance = true;
        ids.push(id);
        starts.push(command);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let mut writer = control.try_clone().unwrap();
    let sender = std::thread::spawn(move || {
        let mut revision = 0;
        while !stop.load(Ordering::Relaxed) {
            let start = Instant::now();
            revision += 1;
            for command in &starts {
                let value = snapshot(command, revision);
                let owner = value.owner.clone();
                let event = Event::PoseSnapshotObservation {
                    generation: owner.generation,
                    subscription: owner.subscription,
                    sequence: revision,
                    sample: Box::new(Message::Snapshot(Box::new(value))),
                    backend_drops: 0,
                };
                writer.write_all(&event.encode().unwrap()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(50).saturating_sub(start.elapsed()));
        }
        revision
    });
    let warm = Instant::now();
    while !app.views.iter().all(|v| {
        v.uncertainty
            .layer(v.id.0)
            .and_then(|l| l.bounds())
            .is_some()
    }) {
        surface.frame(&mut app);
        assert!(warm.elapsed() < Duration::from_secs(10));
    }
    for _ in 0..30 {
        surface.frame(&mut app);
    }
    let start = Instant::now();
    let mut frames = vec![];
    let mut samples = vec![];
    let mut sample_at = Instant::now();
    let mut revision = [0; 2];
    let mut updated = [Instant::now(); 2];
    let mut max_gap = 0_f64;
    let mut max_vertices = 0;
    while start.elapsed() < Duration::from_secs(60) {
        let tick = Instant::now();
        app.shared_scene.camera.azimuth = 0.5 + start.elapsed().as_secs_f32() * 0.1;
        frames.push(surface.frame(&mut app));
        let mut vertices = 0;
        for (i, v) in app.views.iter().enumerate() {
            if let Some(p) = &v.uncertainty.prepared {
                if p.snapshot.revision > revision[i] {
                    max_gap = max_gap.max(updated[i].elapsed().as_secs_f64());
                    updated[i] = Instant::now();
                    revision[i] = p.snapshot.revision;
                }
            }
            vertices += v
                .uncertainty
                .geometry
                .as_ref()
                .map_or(0, |g| g.segments.len() * 6);
        }
        max_vertices = max_vertices.max(vertices);
        if sample_at.elapsed() >= Duration::from_secs(1) {
            samples.push(json!({"seconds":start.elapsed().as_secs_f64(),"rss_kib":rss(),"revisions":revision,"vertices":vertices}));
            sample_at = Instant::now();
        }
        std::thread::sleep(Duration::from_secs_f64(1. / 30.).saturating_sub(tick.elapsed()));
    }
    cancel.store(true, Ordering::Relaxed);
    let offered = sender.join().unwrap();
    let mut cpu: Vec<_> = frames.iter().map(|f| f[0]).collect();
    let drained: Vec<_> = frames.iter().map(|f| f[1]).collect();
    let rss_values: Vec<_> = samples
        .iter()
        .map(|s| s["rss_kib"].as_u64().unwrap() as f64)
        .collect();
    let window = 10.min(rss_values.len() / 2).max(1);
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let growth = mean(&rss_values[rss_values.len() - window..]) - mean(&rss_values[..window]);
    let shutdown = Instant::now();
    for id in ids {
        app.stop_view(id);
        let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
        ingest(&mut app, stopped(&command));
    }
    assert!(app
        .views
        .iter()
        .all(|v| v.uncertainty.layer(v.id.0).is_none() && v.uncertainty.stream.is_none()));
    surface.frame(&mut app);
    control.shutdown(std::net::Shutdown::Write).unwrap();
    reader.join().unwrap();
    terminal.recv_timeout(Duration::from_secs(2)).unwrap();
    let gates = json!({"release_profile":!cfg!(debug_assertions),"duration":start.elapsed().as_secs_f64()>=60.,"app_frame_p95":percentile(&cpu,0.95)<=33.3,
        "gpu_drained_p95":percentile(&drained,0.95)<=100.,"max_source_gap":max_gap<=1.,"source_progress":revision.iter().all(|&r|r>=offered.saturating_sub(3)),
        "rss_limit":rss_values.iter().copied().fold(0.,f64::max)<=1.5*1024.*1024.,"rss_growth":growth<=64.*1024.,
        "maximum_history_exercised":max_vertices>96_000 && max_vertices<=spatial::uncertainty::MAX_VERTICES,"stop_shutdown":shutdown.elapsed().as_secs_f64()<=2.});
    let passed = gates.as_object().unwrap().values().all(|v| v == true);
    let result = json!({"outcome":if passed {"passed"}else{"failed"},"test_pid":std::process::id(),"adapter":format!("{:?}",state.adapter.get_info()),"gates":gates,
        "source_hz":20,"camera_hz":30,"history_keep":256,"historical_covariance":true,"frames":frames.len(),"app_ms":{"p50":percentile(&cpu,0.5),"p95":percentile(&cpu,0.95),"max":percentile(&cpu,1.)},
        "drained_ms_p95":percentile(&drained,0.95),"max_gap_seconds":max_gap,"offered_revision":offered,"last_revisions":revision,"rss_growth_kib":growth,"max_vertices":max_vertices,"samples":samples,
        "scope":"Actual App and production ingress/worker geometry/egui/native Metal; no OS window, ROS source, input-to-present, other arrangements, LAN or hardware claim"});
    std::fs::write(
        folder.join("performance-result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    cpu.clear();
    assert!(passed, "{gates}");
}
