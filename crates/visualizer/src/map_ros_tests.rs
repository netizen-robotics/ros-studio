//! Actual App/standard ROS backend qualification; no native window or GPU claim.
use super::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use studio_client_core::map_fixture::MapPublisher;
use studio_protocol::{map_grid::UpdateSource, map_snapshot::ClearReason};
const FULL: &str = "/studio_fixture/raw_grid";
const BEST: &str = "/studio_fixture/raw_grid_best";
const UPDATES: &str = "/studio_fixture/raw_grid_updates";
const UPDATE_BEST: &str = "/studio_fixture/raw_grid_updates_best";
const IMAGE: &str = "/studio_fixture/map_image";
macro_rules! case {
    ($cases:expr,$folder:expr,$name:expr $(,)?) => {{
        $cases.push($name);
        std::fs::write(
            $folder.join("case-progress.json"),
            serde_json::to_vec_pretty(&$cases).unwrap(),
        )
        .unwrap();
    }};
}
fn wait(app: &mut App, label: &str, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        app.poll_backend();
        app.poll_scene_settings();
        app.check_deadlines(Instant::now());
        if condition(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: connection={} views={:?}",
            app.connection.status,
            app.views
                .iter()
                .map(|v| (
                    &v.configuration.topic,
                    v.status,
                    &v.spatial_status,
                    &v.error
                ))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn stamp(app: &App, index: usize) -> Option<i64> {
    app.views[index]
        .map
        .frame
        .as_ref()
        .map(|m| m.complete().header.metadata.stamp_ns)
}
fn held(app: &mut App, label: &str, condition: impl Fn(&App) -> bool) {
    let until = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < until {
        app.poll_backend();
        app.check_deadlines(Instant::now());
        assert!(condition(app), "{label}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn observe(app: &mut App, condition: impl Fn(&App) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        app.poll_backend();
        if condition(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}
fn scene(app: &mut App, frame: &str) {
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = frame.into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    wait(app, "scene barrier", |a| a.scene_settings.pending.is_none());
}
fn map_sources_released(
    app: &mut App,
    publisher: &mut MapPublisher,
    image_count: u64,
    allowance: Duration,
) -> Value {
    let started = Instant::now();
    let until = started + allowance;
    loop {
        app.poll_backend();
        let counts = publisher.send("counts");
        if ["full", "updates", "full_best", "updates_best", "static_tf"]
            .iter()
            .all(|key| counts[*key] == 0)
            && counts["image"] == image_count
        {
            return json!({"elapsed_seconds":started.elapsed().as_secs_f64(),"counts":counts});
        }
        assert!(Instant::now() < until, "Remaining map sources: {counts}");
        std::thread::sleep(Duration::from_millis(30));
    }
}
fn no_map_sources(app: &mut App, publisher: &mut MapPublisher, image_count: u64) {
    map_sources_released(app, publisher, image_count, Duration::from_secs(15));
}
fn launches(folder: &Path) -> Vec<Value> {
    std::fs::read_to_string(folder.join("backend-launches.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn process_samples(pids: &[(u32, &str)]) -> Vec<Value> {
    let ids = pids
        .iter()
        .map(|(pid, _)| pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let output = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-p", &ids, "-o", "pid=,%cpu=,rss="])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let rows = text.lines().map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        assert_eq!(fields.len(), 3);
        let pid = fields[0].parse::<u32>().unwrap();
        let role = pids.iter().find(|(candidate, _)| *candidate == pid).unwrap().1;
        json!({"pid":pid,"role":role,"cpu_percent":fields[1].parse::<f64>().unwrap(),"rss_kib":fields[2].parse::<u64>().unwrap()})
    }).collect::<Vec<_>>();
    assert_eq!(rows.len(), pids.len());
    rows
}
fn map_performance(app: &mut App, publisher: &mut MapPublisher, folder: &Path) -> Value {
    let backend_pid = launches(folder).last().unwrap()["pid"].as_u64().unwrap() as u32;
    let pids = [
        (std::process::id(), "app_test"),
        (backend_pid, "backend"),
        (publisher.process.child.id(), "publisher"),
    ];
    let mut rows = Vec::new();
    let mut due = Instant::now();
    let workload_start = due;
    for index in 0..30 {
        if let Some(delay) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
        let started = Instant::now();
        let lateness = started.saturating_duration_since(due).as_secs_f64();
        due = started + Duration::from_secs(1); // No catch-up burst after a late iteration.
        let previous_revision = app.views[1].map.revision;
        let acquisition_ns = 2_000_000_004 + index as i64;
        publisher.send(&format!("full_perf {}", acquisition_ns - 2_000_000_000));
        wait(app, "performance complete new full revision", |a| {
            a.views[1].map.revision > previous_revision && stamp(a, 1) == Some(acquisition_ns)
        });
        let delivery_seconds = started.elapsed().as_secs_f64();
        let map = app.views[1].map.frame.as_ref().unwrap();
        assert_eq!(map.dimensions(), [2048, 2048]);
        assert_eq!(map.bytes().len(), 4_194_304);
        assert!(map
            .bytes()
            .iter()
            .enumerate()
            .all(|(i, byte)| *byte == i as u8));
        let image_before = app.views[0].image_sequence;
        publisher.send("image");
        wait(app, "performance Image peer progress", |a| {
            a.views[0].image_sequence > image_before
        });
        rows.push(json!({"index":index,"offered_at_seconds":started.duration_since(workload_start).as_secs_f64(),
            "schedule_lateness_seconds":lateness,"delivery_seconds":delivery_seconds,"revision":app.views[1].map.revision,
            "image_sequence":app.views[0].image_sequence,"processes":process_samples(&pids)}));
        std::fs::write(
            folder.join("performance-samples.json"),
            serde_json::to_vec_pretty(&rows).unwrap(),
        )
        .unwrap();
    }
    let mut latencies = rows
        .iter()
        .map(|r| r["delivery_seconds"].as_f64().unwrap())
        .collect::<Vec<_>>();
    latencies.sort_by(f64::total_cmp);
    let p95 = latencies[(latencies.len() * 95).div_ceil(100) - 1];
    let maximum = *latencies.last().unwrap();
    let rss = |row: &Value, role: &str| {
        row["processes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["role"] == role)
            .unwrap()["rss_kib"]
            .as_u64()
            .unwrap()
    };
    let combined_max = rows
        .iter()
        .map(|r| pids.iter().map(|(_, role)| rss(r, role)).sum::<u64>())
        .max()
        .unwrap();
    let process_max = rows
        .iter()
        .flat_map(|r| r["processes"].as_array().unwrap())
        .map(|p| p["rss_kib"].as_u64().unwrap())
        .max()
        .unwrap();
    let mut growth = serde_json::Map::new();
    for role in ["app_test", "backend"] {
        let mut warm = rows[..5].iter().map(|r| rss(r, role)).collect::<Vec<_>>();
        let mut final_samples = rows[25..].iter().map(|r| rss(r, role)).collect::<Vec<_>>();
        warm.sort_unstable();
        final_samples.sort_unstable();
        growth.insert(role.into(), json!(final_samples[2] as i64 - warm[2] as i64));
    }
    let passed = p95 < 1.0
        && maximum < 1.5
        && process_max < 512 * 1024
        && combined_max < 1024 * 1024
        && growth
            .values()
            .all(|value| value.as_i64().unwrap() <= 64 * 1024);
    let result = json!({"outcome":if passed {"passed"} else {"failed"},"samples":rows.len(),"target_period_seconds":1,
        "elapsed_seconds":workload_start.elapsed().as_secs_f64(),"p95_delivery_seconds":p95,"maximum_delivery_seconds":maximum,
        "maximum_sampled_process_rss_kib":process_max,"maximum_sampled_combined_rss_kib":combined_max,
        "warm_to_final_median_growth_kib":growth,"limits":{"p95_seconds":1.0,"max_seconds":1.5,"process_rss_kib":524288,"combined_rss_kib":1048576,"growth_kib":65536},
        "scope":"Bounded debug App model/standard backend data path and completed-delivery process samples. No native window/FPS or transient peak-RSS claim."});
    std::fs::write(
        folder.join("performance-result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    assert!(passed, "Declared map performance gate failed: {result}");
    result
}
#[test]
#[ignore = "Requires isolated local ROS, scripts/ui-ros-backend and a unique STUDIO_UI_ROS_EVIDENCE folder"]
fn map_real_ros_visualizer_workflow() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap();
    assert!(matches!(
        rmw.as_str(),
        "rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp"
    ));
    for key in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(
            std::env::var(key).unwrap_or_default().is_empty(),
            "Remove {key}"
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let folder = PathBuf::from(std::env::var("STUDIO_UI_ROS_EVIDENCE").unwrap());
    assert!(
        folder.is_absolute() && folder.starts_with(root.join("artifacts/feasibility/evidence"))
    );
    let prefix = std::env::split_paths(&std::env::var_os("AMENT_PREFIX_PATH").unwrap())
        .next()
        .unwrap();
    struct Finish;
    impl Drop for Finish {
        fn drop(&mut self) {
            session::finish_shutdown();
        }
    }
    let _finish = Finish;
    let diagnostic_profiles = std::env::var("STUDIO_MAP_DIAGNOSTIC_PROFILES").as_deref() == Ok("1");
    let mut publisher = if diagnostic_profiles {
        MapPublisher::with_diagnostic_profile(root, &prefix, &folder)
    } else {
        MapPublisher::new(root, &prefix, &folder)
    };
    let publisher_pid = publisher.process.child.id();
    publisher.send("retained");
    publisher.send("retained_best");
    let mut app = App::default();
    app.connection.domain = "187".into();
    app.connection.cyclone = rmw == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovery", |a| {
        a.connection.phase == Phase::Connected
            && [FULL, BEST, UPDATES, UPDATE_BEST, IMAGE, "/tf", "/tf_static"]
                .iter()
                .all(|topic| a.discovery.topics.iter().any(|(t, _)| t == topic))
    });
    let mut cases = Vec::new();
    case!(
        cases,
        folder,
        "exact full/update/Image/TF discovery without subscription"
    );
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
    scene(&mut app, "world");
    app.begin_source(Output::CompactImage, IMAGE, images::IMAGE_MESSAGE_TYPE);
    app.subscribe();
    let image_id = app.selected.unwrap();
    app.set_display_enabled(image_id, true);
    publisher.send("image");
    wait(&mut app, "Image peer", |a| a.views[0].image_sequence > 0);
    app.begin_source(
        Output::MapSnapshot,
        FULL,
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.draft_map_updates = Some(UpdateSource {
        topic: UPDATES.into(),
        best_effort: false,
        transient_local: false,
    });
    app.subscribe();
    let map_id = app.selected.unwrap();
    assert!(!app.views[1].enabled);
    assert_eq!(
        app.session
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .registry
            .used_resources(),
        1
    );
    case!(
        cases,
        folder,
        "Add-disabled map with no full/update/TF reservation"
    );
    let started = Instant::now();
    app.set_display_enabled(map_id, true);
    wait(&mut app, "retained maximum map", |a| {
        stamp(a, 1) == Some(2_000_000_003)
    });
    let maximum_delivery_seconds = started.elapsed().as_secs_f64();
    {
        let map = app.views[1].map.frame.as_ref().unwrap();
        assert_eq!(map.dimensions(), [2048, 2048]);
        assert_eq!(map.bytes().len(), 4_194_304);
        assert!(map.bytes().iter().enumerate().all(|(i, v)| *v == i as u8));
        assert_eq!(map.corners()[0].0, [2., 0., 0.]);
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
        5
    );
    case!(
        cases,
        folder,
        "explicit Enable retained2048 map with actual static TF and Image peer"
    );
    let performance = (std::env::var("STUDIO_MAP_PERFORMANCE").as_deref() == Ok("1"))
        .then(|| map_performance(&mut app, &mut publisher, &folder));
    let original_subscription = app.views[1].subscription;
    app.configure_view(map_id);
    app.discovery.select(BEST);
    app.draft_map_updates = None;
    app.draft_best_effort = true;
    assert_eq!(app.views[1].configuration.topic, FULL);
    assert_eq!(app.views[1].subscription, original_subscription);
    assert!(app.views[0].enabled);
    case!(
        cases,
        folder,
        "source/update draft selection preserves active owner and peer"
    );
    app.setup = false;
    app.views[1].map.appearance = studio_workspace::map::Appearance {
        palette: studio_workspace::map::Palette::Costmap,
        opacity_per_mille: 450,
        draw_behind: true,
    };
    let patch_stamp = if performance.is_some() {
        2_000_000_034
    } else {
        2_000_000_004
    };
    publisher.send(if performance.is_some() {
        "patch_perf 34"
    } else {
        "patch"
    });
    wait(&mut app, "maximum patch", |a| {
        stamp(a, 1) == Some(patch_stamp)
    });
    assert_eq!(app.views[1].subscription, original_subscription);
    assert_eq!(
        app.views[1]
            .map
            .frame
            .as_ref()
            .unwrap()
            .complete()
            .header
            .assembly_counters
            .patches,
        1
    );
    case!(
        cases,
        folder,
        "local appearance plus maximum update without subscription replacement"
    );
    publisher.send("malformed_patch");
    wait(&mut app, "invalid update hides obsolete geometry", |a| {
        a.views[1].map.clear_reason == Some(ClearReason::InvalidInput)
    });
    assert!(app.views[1].map.frame.is_none());
    assert!(app.views[1]
        .map
        .source_status
        .as_ref()
        .unwrap()
        .recovery_floor_ns
        .is_some());
    publisher.send("recovery");
    wait(&mut app, "fresh full recovery", |a| {
        stamp(a, 1) == Some(4_000_000_000)
    });
    case!(
        cases,
        folder,
        "malformed native patch Clear and fresh-full recovery"
    );
    app.configure_view(map_id);
    app.draft_map_updates = Some(UpdateSource {
        topic: UPDATE_BEST.into(),
        best_effort: false,
        transient_local: false,
    });
    app.subscribe();
    wait(&mut app, "update reader replacement", |a| {
        a.views[1].subscription != original_subscription && stamp(a, 1) == Some(4_000_000_000)
    });
    publisher.send("patch_best_nowait");
    held(
        &mut app,
        "Reliable update cannot receive Best effort data",
        |a| stamp(a, 1) == Some(4_000_000_000),
    );
    assert_eq!(publisher.send("counts")["updates_best"], 0);
    case!(
        cases,
        folder,
        "independent update Reliable/Best effort mismatch"
    );
    let mismatched = app.views[1].subscription;
    app.configure_view(map_id);
    app.draft_map_updates.as_mut().unwrap().best_effort = true;
    app.subscribe();
    wait(&mut app, "corrected update owner", |a| {
        a.views[1].subscription != mismatched && stamp(a, 1) == Some(4_000_000_000)
    });
    publisher.send("patch_best");
    let max_best_update_observed = observe(&mut app, |a| stamp(a, 1) == Some(5_000_000_000));
    let update_probe_before =
        json!({"metadata":app.views[1].map.metadata,"status":app.views[1].map.source_status});
    if !max_best_update_observed {
        publisher.send("patch_best_small");
        wait(
            &mut app,
            "small update diagnostic after unobserved maximum Best effort sample",
            |a| {
                stamp(a, 1) == Some(5_000_000_000)
                    && a.views[1]
                        .map
                        .frame
                        .as_ref()
                        .is_some_and(|m| m.bytes()[0] == 42)
            },
        );
    }
    case!(
        cases,
        folder,
        "explicit update QoS correction after correlated replacement"
    );
    let image_sequence = app.views[0].image_sequence;
    publisher.send("image");
    wait(&mut app, "peer continues", |a| {
        a.views[0].image_sequence > image_sequence
    });
    app.configure_view(map_id);
    app.discovery.select(BEST);
    app.draft_best_effort = false;
    app.draft_map_updates = None;
    app.subscribe();
    wait(&mut app, "full reader mismatch starts", |a| {
        a.views[1].configuration.topic == BEST && a.views[1].status == ViewStatus::Waiting
    });
    held(
        &mut app,
        "Reliable full cannot receive Best effort retained data",
        |a| a.views[1].map.frame.is_none(),
    );
    assert_eq!(publisher.send("counts")["full_best"], 0);
    case!(
        cases,
        folder,
        "independent full-map Reliable/Best effort mismatch"
    );
    app.configure_view(map_id);
    app.draft_best_effort = true;
    app.subscribe();
    let max_best_retained_full_observed = observe(&mut app, |a| stamp(a, 1) == Some(2_000_000_003));
    // Best effort historical delivery is recorded independently. If it was
    // absent, explicitly publish a maximum live sample to the matched reader.
    let max_best_full_observed = if max_best_retained_full_observed {
        true
    } else {
        publisher.send("full_best_size 2048");
        observe(&mut app, |a| stamp(a, 1) == Some(7_000_000_000))
    };
    if !max_best_full_observed {
        publisher.send("retained_best_small");
        wait(
            &mut app,
            "small full-map diagnostic after unobserved maximum Best effort sample",
            |a| stamp(a, 1) == Some(6_000_000_000),
        );
    }
    let best_effort_probe = json!({"maximum_best_effort_update_observed":max_best_update_observed,"maximum_best_effort_full_observed":max_best_full_observed,"maximum_best_effort_retained_full_observed":max_best_retained_full_observed,"observed_best_full_stamp":stamp(&app,1),"update_probe_before_small_diagnostic":update_probe_before,"diagnostic_profiles":diagnostic_profiles});
    std::fs::write(
        folder.join("best-effort-probe.json"),
        serde_json::to_vec_pretty(&best_effort_probe).unwrap(),
    )
    .unwrap();
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
    case!(cases,folder,"full-map QoS correction with updates disabled; maximum versus small delivery recorded separately");

    // Best effort history is not assumed to replay. TF recovery is qualified
    // using the Reliable/Transient local full source that delivered retained data.
    app.configure_view(map_id);
    app.discovery.select(FULL);
    app.draft_best_effort = false;
    app.draft_map_updates = None;
    app.subscribe();
    wait(
        &mut app,
        "return to Reliable retained source before TF recovery",
        |a| stamp(a, 1) == Some(4_000_000_000),
    );
    let best_full_stamp = 4_000_000_000;
    scene(&mut app, "missing_world");
    assert!(!app.views[1].enabled && app.views[1].map.frame.is_none());
    assert!(app.views[0].enabled);
    app.set_display_enabled(map_id, true);
    wait(&mut app, "missing acquisition frame", |a| {
        a.views[1].map.metadata.is_some()
            && a.views[1].map.frame.is_none()
            && a.views[1].status == ViewStatus::Waiting
    });
    publisher.send("connect_frame");
    wait(&mut app, "TF-only recovery", |a| {
        stamp(a, 1) == Some(best_full_stamp)
    });
    assert_eq!(
        app.views[1].map.frame.as_ref().unwrap().corners()[0].0,
        [12., 0., 0.]
    );
    case!(
        cases,
        folder,
        "scene barrier and acquisition-time TF-only recovery without new full data"
    );
    let saved = app.workspace_snapshot().unwrap();
    let bytes = studio_workspace::application::encode(
        &saved,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        studio_workspace::application::VISUALIZER_VERSION
    );
    let imported = studio_workspace::application::parse(
        &bytes,
        studio_workspace::application::Application::Visualizer,
    )
    .unwrap();
    let prepared = app.workspace_prepare(&imported.workspace).unwrap();
    assert!(prepared
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    assert!(prepared.views[1].map.frame.is_none() && prepared.views[1].map.stream.is_none());
    assert!(!prepared.views[1].spatial_clock_verified);
    assert_eq!(
        prepared.views[1].map.appearance,
        app.views[1].map.appearance
    );
    case!(
        cases,
        folder,
        "v14 exact appearance/source intent restores inactive without cells or clock permission",
    );
    app.set_display_enabled(map_id, false);
    wait(&mut app, "scoped Stop", |a| {
        a.views[1].subscription.is_none()
    });
    no_map_sources(&mut app, &mut publisher, 1);
    assert!(app.views[1].map.frame.is_none() && app.views[1].map.stream.is_none());
    let before = app.views[0].image_sequence;
    publisher.send("image");
    wait(&mut app, "Image after map Stop", |a| {
        a.views[0].image_sequence > before
    });
    case!(
        cases,
        folder,
        "Stop releases map/update/TF sources while Image keeps receiving"
    );
    app.set_display_enabled(map_id, true);
    wait(&mut app, "map before backend failure", |a| {
        stamp(a, 1) == Some(best_full_stamp)
    });
    let last = launches(&folder).pop().unwrap();
    assert_eq!(
        last["argv"][0],
        root.join("target/debug/studio-backend").to_str().unwrap()
    );
    let backend_pid = last["pid"].as_u64().unwrap();
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &backend_pid.to_string()])
        .status()
        .unwrap()
        .success());
    wait(&mut app, "backend EOF clears all live maps", |a| {
        a.connection.phase == Phase::Failed
    });
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && v.subscription.is_none()));
    assert!(app.views[1].map.frame.is_none() && app.views[1].map.stream.is_none());
    // Abrupt participant death differs from explicit reader deletion. Installed
    // Fast DDS RTPSParticipantAttributes.h defaults to a 20-second lease; retain
    // the 15-second normal Stop gate and measure crash discovery independently.
    session::finish_shutdown();
    let backend_process = std::process::Command::new("ps")
        .args(["-p", &backend_pid.to_string(), "-o", "pid=,ppid=,comm="])
        .output()
        .unwrap();
    assert_eq!(backend_process.status.code(), Some(1));
    assert!(backend_process.stdout.is_empty());
    let crash_discovery =
        map_sources_released(&mut app, &mut publisher, 0, Duration::from_secs(35));
    std::fs::write(
        folder.join("crash-discovery-release.json"),
        serde_json::to_vec_pretty(&json!({"backend_pid":backend_pid,"backend_process_absent":true,"discovery":crash_discovery})).unwrap(),
    ).unwrap();
    case!(
        cases,
        folder,
        "actual backend EOF cancels prepared delivery and hides obsolete map"
    );
    app.connect();
    wait(&mut app, "explicit reconnect", |a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    assert!(app
        .views
        .iter()
        .all(|v| !v.enabled && !v.spatial_clock_verified));
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
    no_map_sources(&mut app, &mut publisher, 0);
    case!(cases, folder, "reconnect has no activation or clock replay");
    app.stop();
    session::finish_shutdown();
    publisher.finish();
    let records = launches(&folder);
    let report = json!({"outcome":"passed","cases":cases,"rmw":rmw,"domain":187,"publisher_pid":publisher_pid,"test_pid":std::process::id(),"backend_pids":records.iter().map(|r|r["pid"].clone()).collect::<Vec<_>>(),"maximum_delivery_seconds":maximum_delivery_seconds,"maximum_best_effort_update_observed":max_best_update_observed,"maximum_best_effort_full_observed":max_best_full_observed,"maximum_best_effort_retained_full_observed":max_best_retained_full_observed,"update_probe_before_small_diagnostic":update_probe_before,"diagnostic_profiles":diagnostic_profiles,"performance":performance,"scope":"Actual Visualizer App/standard backend/native ROS with synthetic peers; no native-window/GPU/performance/hardware claim"});
    std::fs::write(
        folder.join("result.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
}
