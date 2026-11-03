//! Serialized maximum Camera App/production ingress/egui/native Metal gate.
use super::*;
use crate::{
    display_tests::{ingest, started, stopped},
    performance_support::{gpu, percentile, rss, Surface},
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use studio_protocol::{
    camera_info::Calibration,
    camera_snapshot::{store::Store, transport::Message, InfoSource, Owner},
    image::{HeaderStamp, ImageMetadata, ImageSample},
    latest_pose::LatestPose,
};

fn offer(store: &mut Store, writer: &mut UnixStream, revision: u64) {
    let stamp = i64::try_from(revision).unwrap() * 1_000_000_000;
    let focal = 180. + (revision % 20) as f64;
    let calibration: Calibration = serde_json::from_value(json!({
        "stamp_ns":stamp,"frame_id":"camera_optical","width":256,"height":256,
        "distortion_model":"plumb_bob","d":[],
        "k":[focal,0.,128.,0.,180.,128.,0.,0.,1.],
        "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],
        "p":[focal,0.,128.,0.,0.,180.,128.,0.,0.,0.,1.,0.],
        "binning_x":0,"binning_y":0,
        "roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}
    }))
    .unwrap();
    store.info(calibration).unwrap();
    let barrier = store.invalidation().unwrap();
    let owner = barrier.owner.clone();
    let mut pixels = vec![255; 256 * 256 * 4];
    for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        pixel[0] = ((i + revision as usize) % 256) as u8;
        pixel[1] = ((i / 256 + revision as usize) % 256) as u8;
    }
    let image = ImageSample::from_raw(
        &owner.topic,
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: revision.try_into().unwrap(),
                nanoseconds: 0,
            },
            frame_id: "camera_optical".into(),
            width: 256,
            height: 256,
            encoding: "rgba8".into(),
            is_bigendian: false,
            step: 1024,
        },
        &pixels,
    )
    .unwrap();
    let now = revision * 50_000_000;
    store.image(image, now).unwrap();
    let request = store.request(now).unwrap().unwrap();
    assert!(store
        .complete(
            &request,
            LatestPose::Ready {
                resolved_stamp_ns: stamp,
                components: [0., 0., 5., 1., 0., 0., 0.],
            },
            now,
        )
        .unwrap());
    // Exercise ordered invalidation and the production byte reader, including
    // worker decode/preparation. Do not inject prepared frames into the App.
    for (sequence, sample) in [
        (revision * 2 - 1, Message::Invalidated(barrier)),
        (
            revision * 2,
            Message::Snapshot(Box::new(store.snapshot().unwrap())),
        ),
    ] {
        let event = Event::CameraSnapshotObservation {
            generation: owner.generation,
            subscription: owner.subscription,
            sequence,
            sample: Box::new(sample),
            backend_drops: 0,
        };
        writer.write_all(&event.encode().unwrap()).unwrap();
    }
}

#[test]
#[ignore = "Serialized release Camera App/production ingress/egui/Metal 60s; STUDIO_CAMERA_PERFORMANCE_EVIDENCE required"]
fn maximum_camera_app_metal_load_and_lifecycle() {
    let folder = PathBuf::from(std::env::var("STUDIO_CAMERA_PERFORMANCE_EVIDENCE").unwrap());
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
    let mut app = App::default();
    app.session = Some(session);
    app.connection.phase = Phase::Starting;
    let (input, mut control) = UnixStream::pair().unwrap();
    control
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (terminal_tx, terminal) = std::sync::mpsc::sync_channel(1);
    let reader = backend_pipe::spawn(input, shared, terminal_tx);
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
            topics: vec![(
                "/maximum/image_rect".into(),
                vec![images::IMAGE_MESSAGE_TYPE.into()],
            )],
        },
    );
    app.scene_settings.draft.fixed_frame = "world".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.spatial_renderer = Some(spatial::Renderer3d::with_sensor_history(state.clone()));
    app.camera_renderer = Some(spatial::Renderer3d::with_sensor_history(state.clone()));
    app.begin_source(
        Output::CameraSnapshot(Box::new(InfoSource {
            topic: "/maximum/camera_info".into(),
            best_effort: false,
            transient_local: false,
        })),
        "/maximum/image_rect",
        images::IMAGE_MESSAGE_TYPE,
    );
    app.subscribe();
    let id = app.selected.unwrap();
    app.set_display_enabled(id, true);
    let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
    ingest(&mut app, started(&command));
    let owner = Owner {
        generation: command.ids().0,
        subscription: command.ids().2,
        topic: app.views[0].configuration.topic.clone(),
        info: match &app.views[0].configuration.output {
            Output::CameraSnapshot(info) => (**info).clone(),
            _ => panic!("Camera output changed"),
        },
        fixed_frame: "world".into(),
    };
    app.views[0].camera.appearance.placement = studio_workspace::camera::ImagePlacement::Both;
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let mut writer = control.try_clone().unwrap();
    let sender = std::thread::spawn(move || {
        let mut store = Store::new(owner).unwrap();
        let mut revision = 0;
        while !stop.load(Ordering::Acquire) {
            let tick = Instant::now();
            revision += 1;
            offer(&mut store, &mut writer, revision);
            std::thread::sleep(Duration::from_millis(50).saturating_sub(tick.elapsed()));
        }
        store.stop();
        revision
    });
    let warm = Instant::now();
    while !app.views[0].camera.is_current() || app.camera_canvas.marker_request.is_none() {
        surface.frame(&mut app);
        assert!(
            warm.elapsed() < Duration::from_secs(10),
            "Camera warm-up timed out"
        );
    }
    for _ in 0..30 {
        surface.frame(&mut app);
        std::thread::sleep(Duration::from_millis(33));
    }
    let start = Instant::now();
    let mut frames = vec![];
    let mut samples = vec![];
    let mut sample_at = Instant::now() - Duration::from_secs(1);
    let mut last_image = 0;
    let mut last_calibration = 0;
    let mut updated = Instant::now();
    let mut max_gap = 0_f64;
    let mut viewports = BTreeSet::new();
    let mut presented_frames = 0;
    let mut active = true;
    while start.elapsed() < Duration::from_secs(60) {
        let tick = Instant::now();
        app.views[0].camera.appearance.zoom_milli = 1000 + (frames.len() % 200) as u16;
        let size = if (start.elapsed().as_secs() / 2).is_multiple_of(2) {
            [1920, 1080]
        } else {
            [1600, 900]
        };
        frames.push(surface.frame_with_size(&mut app, size));
        max_gap = max_gap.max(updated.elapsed().as_secs_f64());
        if app.views[0].camera.is_current() {
            if let Some(frame) = app.views[0]
                .camera
                .snapshot
                .as_ref()
                .and_then(|s| s.frame.as_ref())
            {
                if frame.image_sequence > last_image {
                    updated = Instant::now();
                    last_image = frame.image_sequence;
                    last_calibration = frame.calibration_revision;
                }
            }
            if let Some((_, size)) = app.camera_canvas.marker_request {
                viewports.insert(size);
                presented_frames += 1;
            }
        }
        active &= app.views[0].enabled && app.views[0].error.is_none();
        if sample_at.elapsed() >= Duration::from_secs(1) {
            samples.push(json!({"seconds":start.elapsed().as_secs_f64(),"rss_kib":rss(),"image":last_image,"calibration":last_calibration}));
            sample_at = Instant::now();
        }
        std::thread::sleep(Duration::from_secs_f64(1. / 30.).saturating_sub(tick.elapsed()));
    }
    let measured_seconds = start.elapsed().as_secs_f64();
    cancel.store(true, Ordering::Release);
    let offered = sender.join().unwrap();
    let cpu: Vec<_> = frames.iter().map(|f| f[0]).collect();
    let drained: Vec<_> = frames.iter().map(|f| f[1]).collect();
    let rss_values: Vec<_> = samples
        .iter()
        .map(|s| s["rss_kib"].as_u64().unwrap() as f64)
        .collect();
    assert!(rss_values.len() >= 20);
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let growth = mean(&rss_values[rss_values.len() - 10..]) - mean(&rss_values[..10]);
    let shutdown = Instant::now();
    app.stop_view(id);
    let command = outgoing.recv_timeout(Duration::from_secs(2)).unwrap();
    ingest(&mut app, stopped(&command));
    surface.frame(&mut app);
    let stopped = app.views[0].subscription.is_none()
        && app.views[0].camera.stream.is_none()
        && app.views[0].camera.snapshot.is_none()
        && app.camera_canvas.marker_request.is_none();
    control.shutdown(std::net::Shutdown::Write).unwrap();
    reader.join().unwrap();
    terminal.recv_timeout(Duration::from_secs(2)).unwrap();
    let stop_seconds = shutdown.elapsed().as_secs_f64();
    let gates = json!({"release_profile":!cfg!(debug_assertions),"duration":measured_seconds>=60.,
        "app_frame_p95":percentile(&cpu,0.95)<=33.3,"gpu_drained_p95":percentile(&drained,0.95)<=100.,
        "max_source_gap":max_gap<=1.,"source_progress":last_image>=offered.saturating_sub(3) && last_calibration>=offered.saturating_sub(3),
        "rss_limit":rss_values.iter().copied().fold(0.,f64::max)<=1.5*1024.*1024.,"rss_growth":growth<=64.*1024.,
        "camera_composition_exercised":active && presented_frames>0 && viewports.len()>=2,
        "stop_shutdown":stopped && stop_seconds<=2.});
    let passed = gates.as_object().unwrap().values().all(|v| v == true);
    let result = json!({"outcome":if passed {"passed"}else{"failed"},"test_pid":std::process::id(),
        "adapter":format!("{:?}",state.adapter.get_info()),"gates":gates,"measured_seconds":measured_seconds,
        "source_hz":20,"camera_hz":30,"image_size":[256,256],"raw_image_bytes":262144,
        "changing_calibration":true,"frames":frames.len(),"presented_frames":presented_frames,"viewports":viewports,
        "app_ms":{"p50":percentile(&cpu,0.5),"p95":percentile(&cpu,0.95),"max":percentile(&cpu,1.)},
        "drained_ms_p95":percentile(&drained,0.95),"max_gap_seconds":max_gap,"offered_revision":offered,
        "last_image":last_image,"last_calibration":last_calibration,"rss_growth_kib":growth,"stop_seconds":stop_seconds,"samples":samples,
        "scope":"Actual App/production byte ingress/Camera worker preparation/egui/native Metal composition; no OS window, ROS throughput, input-to-present, native ROS reader cleanup, LAN or hardware claim"});
    std::fs::write(
        folder.join("performance-result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    assert!(passed, "{gates}");
}
