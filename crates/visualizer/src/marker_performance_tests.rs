//! Opt-in actual App/egui/native-Metal load qualification. No OS window or ROS claim.
use super::*;
use crate::display_tests::{ingest, started, stopped};
use crate::performance_support::{gpu, percentile, rss, Surface};
use serde_json::json;
use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
};
use studio_protocol::{
    latest_pose::LatestPose,
    marker::{Geometry, Marker},
    marker_snapshot as wire,
};

fn maximum(command: &Command, mesh: bool) -> Vec<wire::Chunk> {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!()
    };
    let mut geometries = vec![
        Geometry::Points {
            points: vec![[0.1, 0.2, 0.3]; 4096],
            colors: vec![]
        };
        16
    ];
    geometries.extend(vec![
        Geometry::Text {
            text: "A".repeat(4096)
        };
        16
    ]);
    if mesh {
        geometries.push(Geometry::Mesh {
            resource: "maximum.obj".into(),
            use_embedded_materials: false,
        });
    }
    geometries.resize(256, Geometry::Arrow { points: vec![] });
    let entries: Vec<_> = geometries
        .into_iter()
        .enumerate()
        .map(|(id, geometry)| wire::Entry {
            marker: Marker {
                key: Key {
                    namespace: "maximum".into(),
                    id: id as i32,
                },
                frame_id: "world".into(),
                stamp_ns: 0,
                pose: [0., 0., 0., 0., 0., 0., 1.],
                scale: [0.1; 3],
                color: [1.; 4],
                lifetime_ns: 0,
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
            contract_version: 1,
            owner: wire::Owner {
                generation: *generation,
                subscription: *subscription,
                topic: configuration.topic.clone(),
                message_type: configuration.message_type.clone(),
                fixed_frame: "world".into(),
            },
            revision: 1,
            source_revision: 1,
            captured_at_unix_ns: 1,
            manifest: entries
                .iter()
                .map(|e| wire::ManifestEntry {
                    key: e.marker.key.clone(),
                    version: 1,
                    lifetime: wire::Lifetime::Forever,
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
    (0..snapshot.chunk_count())
        .map(|index| snapshot.chunk(index).unwrap())
        .collect()
}
struct Ingress {
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<(u64, [u64; 2])>>,
    reader: Option<JoinHandle<()>>,
    control: UnixStream,
    terminal: std::sync::mpsc::Receiver<String>,
}
impl Ingress {
    fn stop(&mut self) -> (u64, [u64; 2]) {
        self.cancel.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap()
    }
}
impl Drop for Ingress {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.stop();
        }
        let _ = self.control.shutdown(std::net::Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}
fn cpu_seconds() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    usage.ru_utime.tv_sec as f64
        + usage.ru_utime.tv_usec as f64 / 1e6
        + usage.ru_stime.tv_sec as f64
        + usage.ru_stime.tv_usec as f64 / 1e6
}
#[cfg(target_os = "macos")]
fn heap() -> serde_json::Value {
    #[repr(C)]
    #[derive(Default)]
    struct Statistics {
        blocks_in_use: u32,
        size_in_use: usize,
        max_size_in_use: usize,
        size_allocated: usize,
    }
    unsafe extern "C" {
        fn malloc_zone_statistics(zone: *mut std::ffi::c_void, stats: *mut Statistics);
    }
    let mut stats = Statistics::default();
    // Local SDK malloc.h: null zone sums every malloc zone. Diagnostic only.
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut stats) };
    json!({"blocks_in_use":stats.blocks_in_use,"bytes_in_use":stats.size_in_use,"high_water":stats.max_size_in_use,"allocated_bytes":stats.size_allocated})
}
#[cfg(not(target_os = "macos"))]
fn heap() -> serde_json::Value {
    serde_json::Value::Null
}
fn opaque_visible(app: &App) -> usize {
    let Some(output) = &app.markers.projected else {
        return 0;
    };
    let permit = app.marker_permit().unwrap();
    output
        .frame
        .opaque_visible(Some(&permit))
        .map(|(_, r)| r.len())
        .sum()
}
#[test]
#[ignore = "Serialized optimized actual App/egui/Metal maximum-load60s qualification; unique STUDIO_MARKER_PERFORMANCE_EVIDENCE"]
fn maximum_app_metal_load_and_lifecycle() {
    let folder = PathBuf::from(
        std::env::var("STUDIO_MARKER_PERFORMANCE_EVIDENCE")
            .expect("Unique evidence folder required"),
    );
    assert!(
        folder.is_absolute()
            && folder.starts_with(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../artifacts/feasibility/evidence")
                    .canonicalize()
                    .unwrap()
            )
    );
    std::fs::create_dir_all(&folder).unwrap();
    let seconds = std::env::var("STUDIO_MARKER_PERFORMANCE_SECONDS")
        .map_or(60., |v| v.parse::<f64>().unwrap());
    assert!((2.0..=120.0).contains(&seconds));
    let source_hz = std::env::var("STUDIO_MARKER_PERFORMANCE_SOURCE_HZ")
        .map_or(2., |s| s.parse::<f64>().unwrap());
    let camera_hz = std::env::var("STUDIO_MARKER_PERFORMANCE_CAMERA_HZ")
        .map_or(30., |s| s.parse::<f64>().unwrap());
    assert!((0.0..=20.0).contains(&source_hz) && (0.0..=30.0).contains(&camera_hz));
    std::fs::write(
        folder.join("maximum.obj"),
        format!("v 0 0 0\nv 1 0 0\nv 0 1 0\n{}", "f 1 2 3\n".repeat(50_000)),
    )
    .unwrap();
    let state = gpu();
    let surface = Surface::new(state.clone());
    let (session, outgoing) = session::Session::fixture(0);
    let mut app = App::default();
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    let shared = app.session.as_ref().unwrap().shared.clone();
    let (reader, mut control) = UnixStream::pair().unwrap();
    control
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (terminal_tx, terminal) = std::sync::mpsc::sync_channel(1);
    let reader = backend_pipe::spawn(reader, shared, terminal_tx);
    let mut ready =
        serde_json::to_vec(&Event::ready("jazzy".into(), "fixture-no-ros".into())).unwrap();
    ready.push(b'\n');
    control.write_all(&ready).unwrap();
    let handshake = Instant::now();
    while app.connection.phase != Phase::Connected {
        app.poll_backend();
        assert!(handshake.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(1));
    }
    ingest(&mut app, Event::Graph { topics: vec![] });
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.spatial_renderer = Some(spatial::Renderer3d::with_sensor_history(state.clone()));
    let mut topics = app.discovery.topics.clone();
    topics.extend([
        (
            "/maximum_one".into(),
            vec![marker::ARRAY_MESSAGE_TYPE.into()],
        ),
        (
            "/maximum_two".into(),
            vec![marker::ARRAY_MESSAGE_TYPE.into()],
        ),
    ]);
    ingest(&mut app, Event::Graph { topics });
    app.markers
        .review(AssetDraft {
            root: folder.to_string_lossy().into(),
            packages: Vec::new(),
        })
        .unwrap();
    let mut ids = vec![];
    let mut batches = vec![];
    for (i, topic) in ["/maximum_one", "/maximum_two"].iter().enumerate() {
        app.begin_source(Output::MarkerSnapshot, topic, marker::ARRAY_MESSAGE_TYPE);
        app.subscribe();
        let id = app.selected.unwrap();
        app.set_display_enabled(id, true);
        let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
        ingest(&mut app, started(&command));
        batches.push(maximum(&command, i == 0));
        ids.push(id);
    }
    let documents: Vec<_> = batches
        .iter()
        .map(|b| json!({"bytes":b[0].header.document_bytes,"chunks":b.len()}))
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let mut writer = control.try_clone().unwrap();
    let handle = std::thread::spawn(move || {
        let mut revision = 0;
        let mut sequence = [0u64; 2];
        while !worker_cancel.load(Ordering::Relaxed) {
            revision += 1;
            let start = Instant::now();
            for (i, batch) in batches.iter().enumerate() {
                for chunk in batch {
                    let mut chunk = chunk.clone();
                    chunk.header.revision = revision;
                    sequence[i] += 1;
                    let owner = chunk.header.owner.clone();
                    let event = Event::MarkerSnapshotObservation {
                        generation: owner.generation,
                        subscription: owner.subscription,
                        sequence: sequence[i],
                        sample: Box::new(wire::Message::Chunk(Box::new(chunk))),
                        backend_drops: 0,
                    };
                    let mut line = serde_json::to_vec(&event).unwrap();
                    line.push(b'\n');
                    writer.write_all(&line).unwrap();
                }
            }
            while (source_hz == 0. || start.elapsed() < Duration::from_secs_f64(1. / source_hz))
                && !worker_cancel.load(Ordering::Relaxed)
            {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        (revision, sequence)
    });
    let mut ingress = Ingress {
        cancel,
        handle: Some(handle),
        reader: Some(reader),
        control,
        terminal,
    };
    let initial = Instant::now();
    while app
        .markers
        .projected
        .as_ref()
        .is_none_or(|p| p.frame.len() != 313_038)
    {
        surface.frame(&mut app);
        assert!(
            initial.elapsed() < Duration::from_secs(30),
            "Initial maximum frame: {:?}",
            app.markers.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        app.markers.projected.as_ref().unwrap().scene.key_count(),
        512
    );
    assert!(app
        .markers
        .projected
        .as_ref()
        .unwrap()
        .scene
        .diagnostics
        .is_empty());
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_secs(5) {
        surface.frame(&mut app);
        std::thread::sleep(Duration::from_millis(20));
    }
    let start = Instant::now();
    let cpu_start = cpu_seconds();
    let mut samples = vec![];
    let mut frames = vec![];
    let mut phases = vec![];
    let mut last_projection = app.markers.projected.as_ref().unwrap().camera;
    let mut projection_at = start;
    let mut max_gap = 0f64;
    let mut completions = 0;
    let mut max_instances = 0usize;
    let mut max_atlas = 0usize;
    let mut max_mesh = 0usize;
    let mut next_sample = 0.;
    while start.elapsed().as_secs_f64() < seconds {
        let due = Instant::now();
        let elapsed = start.elapsed().as_secs_f64();
        if camera_hz > 0. {
            app.shared_scene.camera.azimuth = 0.5 + elapsed as f32 * 0.1;
        }
        let times = surface.frame(&mut app);
        frames.push(times);
        phases.push(app.frame_trace);
        let output = app
            .markers
            .projected
            .as_ref()
            .expect("Maximum scene vanished");
        assert_eq!(output.frame.len(), 313_038);
        assert!(app.markers.error.is_none(), "{:?}", app.markers.error);
        if output.camera != last_projection {
            max_gap = max_gap.max(projection_at.elapsed().as_secs_f64());
            projection_at = Instant::now();
            last_projection = output.camera;
            completions += 1;
        }
        let allocations = app.spatial_renderer.as_ref().unwrap().marker_allocations();
        max_instances = max_instances.max(allocations.instance_bytes);
        max_atlas = max_atlas.max(allocations.glyph_atlas_bytes);
        max_mesh = max_mesh.max(allocations.mesh_texture_bytes);
        if elapsed >= next_sample {
            samples.push(json!({"seconds":elapsed,"test_pid":std::process::id(),"rss_kib":rss(),"malloc":heap(),"cpu_seconds":cpu_seconds()-cpu_start,
            "revisions":app.views.iter().map(|v|v.marker.revision).collect::<Vec<_>>(),"projection_completions":completions}));
            std::fs::write(
                folder.join("performance-progress.tmp"),
                serde_json::to_vec_pretty(samples.last().unwrap()).unwrap(),
            )
            .unwrap();
            std::fs::rename(
                folder.join("performance-progress.tmp"),
                folder.join("performance-progress.json"),
            )
            .unwrap();
            if elapsed >= 10.
                && !folder.join("vmmap-summary.txt").exists()
                && std::env::var_os("STUDIO_MARKER_PERFORMANCE_VMMAP").is_some()
            {
                let map = std::process::Command::new("vmmap")
                    .args([&std::process::id().to_string(), "-summary"])
                    .output()
                    .unwrap();
                std::fs::write(
                    folder.join("vmmap-summary.txt"),
                    [map.stdout, map.stderr].concat(),
                )
                .unwrap();
            }
            next_sample += 1.;
        }
        if let Some(delay) = Duration::from_secs_f64(1. / 30.).checked_sub(due.elapsed()) {
            std::thread::sleep(delay);
        }
    }
    max_gap = max_gap.max(projection_at.elapsed().as_secs_f64());
    let actual_seconds = start.elapsed().as_secs_f64();
    let cpu = (cpu_seconds() - cpu_start) / actual_seconds * 100.;
    let (revision, sequence) = ingress.stop();
    let source_progress = app
        .views
        .iter()
        .all(|v| v.marker.revision >= revision.saturating_sub(2));
    let replacements = app.markers.workers.as_ref().unwrap().replacements();
    let app_times: Vec<_> = frames.iter().map(|r| r[0]).collect();
    let drained: Vec<_> = frames.iter().map(|r| r[1]).collect();
    let rss_values: Vec<_> = samples
        .iter()
        .map(|v| v["rss_kib"].as_u64().unwrap() as f64)
        .collect();
    let window = 10.min(rss_values.len() / 2).max(1);
    let early = rss_values[..window].iter().sum::<f64>() / window as f64;
    let late = rss_values[rss_values.len() - window..].iter().sum::<f64>() / window as f64;
    app.clear_marker(ids[0]);
    let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
    let Command::ClearMarkers {
        generation,
        subscription,
        request,
    } = command
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
    let owner = wire::Owner {
        generation,
        subscription,
        topic: "/maximum_one".into(),
        message_type: marker::ARRAY_MESSAGE_TYPE.into(),
        fixed_frame: "world".into(),
    };
    ingest(
        &mut app,
        Event::MarkerSnapshotObservation {
            generation,
            subscription,
            sequence: sequence[0] + 1,
            sample: Box::new(wire::Message::Clear(Box::new(wire::Clear {
                contract_version: 1,
                owner,
                revision: revision + 1,
                reason: wire::ClearReason::LocalClear,
                known_history_gap: false,
            }))),
            backend_drops: 0,
        },
    );
    let clear_start = Instant::now();
    while opaque_visible(&app) != 65_984
        || app.markers.projected.as_ref().unwrap().frame.len() != 131_520
    {
        surface.frame(&mut app);
        assert!(
            clear_start.elapsed() < Duration::from_secs(3),
            "Clear lost peer: {}",
            opaque_visible(&app)
        );
    }
    app.stop_view(ids[1]);
    let stop = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
    ingest(&mut app, stopped(&stop));
    for _ in 0..3 {
        surface.frame(&mut app);
    }
    let released = app.spatial_renderer.as_ref().unwrap().marker_allocations();
    let shutdown = Instant::now();
    app.markers.finish_shutdown().unwrap();
    let shutdown_ms = shutdown.elapsed().as_secs_f64() * 1000.;
    let after_stop = json!({"rss_kib":rss(),"malloc":heap()});
    ingress.control.shutdown(std::net::Shutdown::Write).unwrap();
    ingress.reader.take().unwrap().join().unwrap();
    let reader_terminal = ingress
        .terminal
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let gates = json!({"duration":seconds>=60. && source_hz==2. && camera_hz==30.,"app_frame_p95":percentile(&app_times,0.95)<=33.3,
        "gpu_drained_p95":percentile(&drained,0.95)<=100.,"projection_gap":max_gap<=1.,"projection_progress":completions as f64>=seconds,
        "source_progress":source_progress,"rss_limit":rss_values.iter().copied().fold(0.,f64::max)<=1.5*1024.*1024.,"rss_growth":late-early<=64.*1024.,
        "instance_budget":max_instances<=40_085_504 && max_instances>0,"atlas_budget":max_atlas<=4*1024*1024 && max_atlas>0,
        "mesh_texture_budget":max_mesh<=32*1024*1024,"stop_gpu_release":released.instance_bytes+released.glyph_atlas_bytes+released.mesh_texture_bytes==0,
        "shutdown":shutdown_ms<=2000.});
    let passed = gates.as_object().unwrap().values().all(|v| v == true);
    let result = json!({"test":"maximum-actual-App-egui-Metal-load","outcome":if passed {"passed"} else {"failed"},"scope":"Actual App frame/client assembly/workers/egui/Metal; protocol ingress fixture. No OS window/ROS/LAN/hardware claim.",
        "profile":if cfg!(debug_assertions) {"debug"} else {"release"},"test_pid":std::process::id(),"adapter":format!("{:?}",state.adapter.get_info()),
        "keys":512,"points":131072,"glyphs":131072,"mesh_triangles":50000,"arrows":447,"records":313038,"documents":documents,
        "input_hz_per_owner":source_hz,"desired_camera_hz":camera_hz,"seconds":actual_seconds,"frames":frames.len(),"rendered_frames_per_second":frames.len() as f64/actual_seconds,
        "app_ms":{"p50":percentile(&app_times,0.5),"p95":percentile(&app_times,0.95),"max":percentile(&app_times,1.)},
        "gpu_drained_frame_ms":{"p50":percentile(&drained,0.5),"p95":percentile(&drained,0.95),"max":percentile(&drained,1.)},
        "projection_completions":completions,"maximum_projection_gap_seconds":max_gap,"process_cpu_percent":cpu,
        "rss_early_window_kib":early,"rss_late_window_kib":late,"rss_growth_kib":late-early,
        "gpu_logical_bytes":{"instances":max_instances,"glyph_atlas":max_atlas,"mesh_images":max_mesh},
        "ingress_path":"production backend_pipe::spawn over bounded Unix socket; production frame validation and decode outside Shared mutex",
        "reader_terminal":reader_terminal,
        "frame_sample_columns":["app_wall_ms","gpu_drained_wall_ms","app_thread_cpu_ms"],
        "phase_names":["backend","sensor_history","markers","maintenance","render","projection_submit","errors"],
        "phase_sample_columns":["wall_ms","thread_cpu_ms"],"phase_samples_ms":phases,
        "after_stop":after_stop,"coalescing":format!("{replacements:?}"),"clear_peer_records":131520,"shutdown_ms":shutdown_ms,"gates":gates,"samples":samples,"frame_samples_ms":frames});
    std::fs::write(
        folder.join("performance-result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    eprintln!(
        "App load result: {}",
        json!({"gates":result["gates"],"app_ms":result["app_ms"],"rss_growth_kib":result["rss_growth_kib"]})
    );
    assert!(
        passed,
        "App load gates failed; full result retained in {}",
        folder.display()
    );
}
