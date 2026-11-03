//! Actual application-root test; the dedicated runner owns isolated publishers.
use super::*;
use crate::components::workspace::{self as file, Durability, PackageRoot, Reliability};
use std::path::Path;

const PREFIX: &str = "/studio_fixture/description_app/";
struct Harness(App);
impl std::ops::Deref for Harness {
    type Target = App;
    fn deref(&self) -> &App {
        &self.0
    }
}
impl std::ops::DerefMut for Harness {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.0
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.0.stop();
        let _ = self.0.robot_setup.finish_shutdown();
        session::finish_shutdown();
    }
}
fn tick(app: &mut App) {
    app.poll_backend();
    app.poll_scene_settings();
    app.robot_poll();
    app.check_deadlines(Instant::now());
    app.workspace_poll(&egui::Context::default());
}
fn wait(app: &mut App, label: &str, predicate: impl Fn(&App) -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !predicate(app) {
        tick(app);
        assert!(
            Instant::now() < until,
            "{label}: connection={}, setup={:?}, views={:?}",
            app.connection.status,
            app.robot_setup.error,
            app.views
                .iter()
                .map(|v| (
                    &v.configuration.topic,
                    &v.error,
                    v.status,
                    &v.spatial_status,
                    (v.configuration.best_effort, v.configuration.transient_local)
                ))
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn observe_wait(app: &mut App, peer: ViewId) {
    let before = app
        .views
        .iter()
        .find(|v| v.id == peer)
        .unwrap()
        .image_sequence;
    let until = Instant::now() + Duration::from_millis(700);
    while Instant::now() < until {
        tick(app);
        assert!(app.robot_setup.capture.is_some());
        assert!(app.robot_setup.reviewed.is_none() && app.robot_setup.load.is_none());
        assert!(
            app.robot_setup.error.is_none(),
            "{:?}",
            app.robot_setup.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.views
            .iter()
            .find(|v| v.id == peer)
            .unwrap()
            .image_sequence
            > before
    );
}
fn source(app: &mut App, target: Option<ViewId>, topic: &str, best: bool, retained: bool) {
    app.begin_robot_setup(target);
    let description = app
        .robot_setup
        .draft
        .as_mut()
        .unwrap()
        .description
        .get_or_insert_with(Default::default);
    description.topic = format!("{PREFIX}{topic}");
    description.qos.reliability = if best {
        Reliability::BestEffort
    } else {
        Reliability::Reliable
    };
    description.qos.durability = if retained {
        Durability::TransientLocal
    } else {
        Durability::Volatile
    };
}
fn read(app: &mut App) {
    app.robot_load().unwrap();
    wait(app, "description capture/asset review", |a| {
        a.robot_setup.reviewed.is_some() || a.robot_setup.error.is_some()
    });
    assert!(
        app.robot_setup.error.is_none(),
        "{:?}",
        app.robot_setup.error
    );
    assert!(app.robot_setup.capture.is_none());
    assert!(!app
        .session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .registry
        .entries()
        .iter()
        .any(|e| e.configuration.output == Output::RobotDescription));
}
fn cancel(app: &mut App) {
    app.cancel_robot_setup();
    wait(app, "capture cancellation", |a| {
        a.robot_setup.capture.is_none()
    });
    assert!(app.robot_setup.reviewed.is_none());
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

#[test]
#[ignore = "Requires scripts/test-description-app-ros.py matching localhost domain187 publishers"]
fn description_real_ros_visualizer_workflow() {
    assert_eq!(std::env::var("ROS_DOMAIN_ID").unwrap(), "187");
    assert_eq!(std::env::var("ROS_LOCALHOST_ONLY").unwrap(), "1");
    assert_eq!(
        std::env::var("ROS_AUTOMATIC_DISCOVERY_RANGE").unwrap(),
        "LOCALHOST"
    );
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap();
    assert!(["rmw_fastrtps_cpp", "rmw_cyclonedds_cpp"].contains(&rmw.as_str()));
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
            std::env::var_os(key).is_none(),
            "Remove custom/remote discovery {key}"
        );
    }
    let mut app = Harness(App::default());
    app.connection.domain = "187".into();
    app.connection.cyclone = rmw == "rmw_cyclonedds_cpp";
    app.connect();
    wait(&mut app, "discovery", |a| {
        a.connection.phase == Phase::Connected
            && a.discovery
                .topics
                .iter()
                .any(|(t, _)| t == &format!("{PREFIX}assets"))
    });
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
    app.open_scene_settings();
    app.scene_settings.draft.fixed_frame = "map".into();
    app.scene_settings.draft_verified = true;
    app.apply_scene_settings().unwrap();
    app.begin_source(
        Output::CompactImage,
        &format!("{PREFIX}image"),
        images::IMAGE_MESSAGE_TYPE,
    );
    app.subscribe();
    let image = app.selected.unwrap();
    app.set_display_enabled(image, true);
    wait(&mut app, "Image peer", |a| {
        a.views
            .iter()
            .any(|v| v.id == image && v.image_sequence > 0)
    });
    println!("CASE passed: inactive discovery and explicit Image peer reception");

    source(&mut app, None, "retained", false, false);
    app.robot_load().unwrap();
    observe_wait(&mut app, image);
    cancel(&mut app);
    println!("CASE passed: Volatile late join waits; cancellation releases reader while Image progresses");

    source(&mut app, None, "retained", false, true);
    let began = Instant::now();
    read(&mut app);
    assert_eq!(
        app.robot_setup.captured_summary.as_ref().unwrap().0,
        studio_protocol::robot_description::MAX_DOCUMENT_BYTES
    );
    let summary = app.robot_setup.reviewed.as_ref().unwrap().summary.clone();
    assert_eq!(summary.name, "topic_fixture_robot");
    assert_eq!((summary.links, summary.triangles), (2, 140));
    println!(
        "MEASURE retained_read_review_seconds={}",
        began.elapsed().as_secs_f64()
    );
    app.robot_apply().unwrap();
    let robot = app.selected.unwrap();
    assert!(app
        .views
        .iter()
        .find(|v| v.id == robot)
        .unwrap()
        .subscription
        .is_none());
    app.set_display_enabled(robot, true);
    wait(&mut app, "native TF poses", |a| {
        a.views
            .iter()
            .find(|v| v.id == robot)
            .unwrap()
            .robot
            .as_ref()
            .unwrap()
            .poses()
            .iter()
            .all(Option::is_some)
    });
    let installed = app
        .views
        .iter()
        .find(|v| v.id == robot)
        .unwrap()
        .robot
        .as_ref()
        .unwrap()
        .model_id();
    println!("CASE passed: retained 1 MiB capture stops before asset review; Add disabled then explicit Robot TF Enable");

    for (topic, expected) in [("bad_xml", ""), ("oversize", "1 MiB")] {
        source(&mut app, Some(robot), topic, false, true);
        app.robot_load().unwrap();
        wait(&mut app, "rejected source", |a| {
            a.robot_setup.error.is_some()
                && a.robot_setup.capture.is_none()
                && a.robot_setup.load.is_none()
        });
        assert!(app.robot_setup.error.as_ref().unwrap().contains(expected));
        let active = app.views.iter().find(|v| v.id == robot).unwrap();
        assert!(active.subscription.is_some());
        assert_eq!(active.robot.as_ref().unwrap().model_id(), installed);
        assert!(app
            .views
            .iter()
            .find(|v| v.id == image)
            .unwrap()
            .subscription
            .is_some());
    }
    println!("CASE passed: malformed/oversized descriptions preserve installed Robot and Image");

    source(&mut app, Some(robot), "best", false, false);
    app.robot_load().unwrap();
    observe_wait(&mut app, image);
    cancel(&mut app);
    source(&mut app, Some(robot), "best", true, false);
    read(&mut app);
    let prior_subscription = app
        .views
        .iter()
        .find(|v| v.id == robot)
        .unwrap()
        .subscription;
    assert!(prior_subscription.is_some());
    app.robot_apply().unwrap();
    wait(&mut app, "Robot replacement cleanup", |a| {
        a.robot_setup.install.is_none()
    });
    let replaced = app.views.iter().find(|v| v.id == robot).unwrap();
    assert!(!replaced.enabled && replaced.subscription.is_none());
    assert_ne!(replaced.robot.as_ref().unwrap().model_id(), installed);
    assert!(app
        .views
        .iter()
        .find(|v| v.id == image)
        .unwrap()
        .subscription
        .is_some());
    app.set_display_enabled(robot, true);
    wait(&mut app, "corrected Robot TF", |a| {
        a.views
            .iter()
            .find(|v| v.id == robot)
            .unwrap()
            .robot
            .as_ref()
            .unwrap()
            .poses()
            .iter()
            .all(Option::is_some)
    });
    println!("CASE passed: QoS mismatch waits; explicit correction/review/Apply replaces only Robot and leaves it disabled");

    source(&mut app, Some(robot), "assets", false, true);
    app.robot_load().unwrap();
    wait(&mut app, "relative-root rejection", |a| {
        a.robot_setup.error.is_some()
            && a.robot_setup.capture.is_none()
            && a.robot_setup.load.is_none()
    });
    assert!(app
        .robot_setup
        .error
        .as_ref()
        .unwrap()
        .contains("explicit asset root"));
    let assets = root().join("fixtures/robot_visualizer");
    app.robot_setup
        .draft
        .as_mut()
        .unwrap()
        .description
        .as_mut()
        .unwrap()
        .asset_root = Some(assets.to_string_lossy().into_owned());
    app.robot_load().unwrap();
    wait(&mut app, "package-map rejection", |a| {
        a.robot_setup.error.is_some()
            && a.robot_setup.capture.is_none()
            && a.robot_setup.load.is_none()
    });
    assert!(app
        .robot_setup
        .error
        .as_ref()
        .unwrap()
        .contains("Unmapped package"));
    app.robot_setup
        .draft
        .as_mut()
        .unwrap()
        .description
        .as_mut()
        .unwrap()
        .package_roots = vec![PackageRoot {
        package: "fixture_robot".into(),
        path: assets.to_string_lossy().into_owned(),
    }];
    read(&mut app);
    let topic_summary = app.robot_setup.reviewed.as_ref().unwrap().summary.clone();
    assert_eq!(topic_summary.links, 5);
    app.robot_apply().unwrap();
    wait(&mut app, "asset model replacement", |a| {
        a.robot_setup.install.is_none()
    });
    app.begin_robot_setup(Some(robot));
    {
        let draft = app.robot_setup.draft.as_mut().unwrap();
        draft.description = None;
        draft.assets = RobotAssets {
            urdf_path: assets
                .join("articulated.urdf")
                .to_string_lossy()
                .into_owned(),
            package_roots: vec![PackageRoot {
                package: "fixture_robot".into(),
                path: assets.to_string_lossy().into_owned(),
            }],
        };
    }
    app.robot_load().unwrap();
    wait(&mut app, "File review", |a| {
        a.robot_setup.reviewed.is_some() || a.robot_setup.error.is_some()
    });
    assert!(
        app.robot_setup.error.is_none(),
        "{:?}",
        app.robot_setup.error
    );
    assert_eq!(
        app.robot_setup.reviewed.as_ref().unwrap().summary,
        topic_summary
    );
    assert!(
        app.robot_setup.captured_summary.is_none(),
        "A File review cannot retain a Topic digest"
    );
    println!("CASE passed: Topic assets require explicit root/package mapping; shared worker produces identical File/Topic model summary");

    // Shared native TF readers align all three spatial source types.
    app.begin_source(
        Output::SpatialScan,
        &format!("{PREFIX}scan"),
        spatial::SCAN_MESSAGE_TYPE,
    );
    app.subscribe();
    let scan = app.selected.unwrap();
    app.set_display_enabled(scan, true);
    app.begin_source(
        Output::OccupancyGrid,
        &format!("{PREFIX}map"),
        studio_protocol::mapping::MAP_MESSAGE_TYPE,
    );
    app.subscribe();
    let map = app.selected.unwrap();
    app.set_display_enabled(map, true);
    wait(&mut app, "scan/map peers", |a| {
        !a.views
            .iter()
            .find(|v| v.id == scan)
            .unwrap()
            .sensor_history
            .points()
            .is_empty()
            && a.views
                .iter()
                .find(|v| v.id == map)
                .unwrap()
                .mapping_frame
                .as_ref()
                .is_some_and(|f| f.geometry.is_some())
    });
    println!(
        "CASE passed: synthetic scan/map peers explicitly enabled with Image and native shared TF"
    );
    for (key, count) in [
        ("obj", 4),
        ("collada", 4),
        ("rich", 1106),
        ("collision", 26),
    ] {
        source(&mut app, Some(robot), key, false, true);
        app.robot_setup
            .draft
            .as_mut()
            .unwrap()
            .description
            .as_mut()
            .unwrap()
            .asset_root = Some(assets.to_string_lossy().into_owned());
        read(&mut app);
        let candidate = app
            .robot_setup
            .reviewed
            .as_ref()
            .unwrap()
            .loaded
            .prepared
            .clone();
        assert_eq!(candidate.triangle_count, count);
        let before = app
            .views
            .iter()
            .find(|v| v.id == image)
            .unwrap()
            .image_sequence;
        app.robot_apply().unwrap();
        wait(&mut app, "rich install", |a| {
            a.robot_setup.install.is_none()
        });
        assert!(!app.views.iter().find(|v| v.id == robot).unwrap().enabled);
        assert!(app
            .views
            .iter()
            .filter(|v| [image, scan, map].contains(&v.id))
            .all(|v| v.enabled));
        app.set_display_enabled(robot, true);
        wait(&mut app, "rich actual TF", |a| {
            a.views
                .iter()
                .find(|v| v.id == robot)
                .unwrap()
                .robot
                .as_ref()
                .unwrap()
                .poses()
                .iter()
                .all(Option::is_some)
                && a.views
                    .iter()
                    .find(|v| v.id == image)
                    .unwrap()
                    .image_sequence
                    > before
                && !a
                    .views
                    .iter()
                    .find(|v| v.id == scan)
                    .unwrap()
                    .sensor_history
                    .points()
                    .is_empty()
                && a.views
                    .iter()
                    .find(|v| v.id == map)
                    .unwrap()
                    .mapping_frame
                    .as_ref()
                    .is_some_and(|f| f.geometry.is_some())
        });
        if key == "rich" {
            let live = app
                .views
                .iter()
                .find(|v| v.id == robot)
                .unwrap()
                .robot
                .as_ref()
                .unwrap();
            let pose = live.poses()[0].unwrap();
            assert_eq!(
                pose.apply(spatial::Point3(candidate.visuals[0].triangles[0].0[0]))
                    .0,
                [1., 0., 0.]
            );
            let scan_view = app.views.iter().find(|v| v.id == scan).unwrap();
            let point = &scan_view.sensor_history.points().first().unwrap().position;
            assert!((point.0[0] - 2.).abs() < 1e-5 && point.0[1].abs() < 1e-5);
            let grid = &app
                .views
                .iter()
                .find(|v| v.id == map)
                .unwrap()
                .mapping_frame
                .as_ref()
                .unwrap()
                .geometry;
            match grid {
                Some(spatial::mapping_live::WorldGeometry::Map(grid)) => {
                    assert_eq!(grid.corners[0].0, [1., 0., 0.])
                }
                _ => panic!("missing aligned grid"),
            }
            let textures = candidate
                .visuals
                .iter()
                .filter_map(|v| v.texture.as_ref())
                .collect::<Vec<_>>();
            assert_eq!(textures.len(), 3);
            assert!(textures
                .iter()
                .all(|t| std::sync::Arc::ptr_eq(t, textures[0])));
            assert!(candidate.visuals.iter().any(|v| v.rgba[3] == 0.5));
        }
        if key == "collision" {
            let view = app.views.iter_mut().find(|view| view.id == robot).unwrap();
            let subscription = view.subscription;
            let generation = view.generation;
            let poses = view.robot.as_ref().unwrap().poses().to_vec();
            assert_eq!(poses.len(), 2);
            let tip = poses[1]
                .unwrap()
                .apply(spatial::Point3(candidate.visuals[2].triangles[0].0[0]))
                .0;
            assert!(
                (tip[0] - 1.).abs() < 1e-6 && tip[1].abs() < 1e-6 && (tip[2] - 1.8).abs() < 1e-6,
                "{tip:?}"
            );
            view.set_robot_appearance(robot::RobotAppearance {
                visual: false,
                collision: true,
                opacity_permille: 500,
                links: vec![
                    robot::LinkAppearance {
                        name: "base".into(),
                        visible: true,
                        opacity_permille: 400,
                    },
                    robot::LinkAppearance {
                        name: "tip".into(),
                        visible: false,
                        opacity_permille: 1000,
                    },
                ],
            })
            .unwrap();
            let live = view.robot.as_ref().unwrap();
            assert_eq!(
                live.prepared()
                    .visuals
                    .iter()
                    .map(|v| v.visible)
                    .collect::<Vec<_>>(),
                [false, true, false]
            );
            assert!((live.prepared().visuals[1].rgba[3] - 0.2).abs() < 1e-6);
            assert_eq!(live.poses(), poses);
            assert_eq!(view.subscription, subscription);
            assert_eq!(view.generation, generation);
            println!("CASE passed: collision local role/link/opacity edits preserve real TF subscription and poses; collision origin uses external TF, not joint metadata");
        }
        app.begin_robot_setup(Some(robot));
        let draft = app.robot_setup.draft.as_mut().unwrap();
        draft.description = None;
        draft.assets = RobotAssets {
            urdf_path: assets
                .join(format!("{key}.urdf"))
                .to_string_lossy()
                .into_owned(),
            package_roots: vec![],
        };
        app.robot_load().unwrap();
        wait(&mut app, "rich File review", |a| {
            a.robot_setup.reviewed.is_some() || a.robot_setup.error.is_some()
        });
        assert!(
            app.robot_setup.error.is_none(),
            "{:?}",
            app.robot_setup.error
        );
        let file = &app.robot_setup.reviewed.as_ref().unwrap().loaded.prepared;
        assert_eq!(file.triangle_count, candidate.triangle_count);
        assert_eq!(file.visuals.len(), candidate.visuals.len());
        for (a, b) in file.visuals.iter().zip(&candidate.visuals) {
            assert_eq!(a.triangles, b.triangles);
            assert_eq!(a.uvs, b.uvs);
            assert_eq!(a.rgba, b.rgba);
            assert_eq!(a.texture, b.texture);
            assert_eq!(a.role, b.role);
            assert_eq!(a.visible, b.visible);
        }
        println!("CASE passed: {key} retained Topic/File equivalence; disabled replacement then actual TF; Image/scan/map peers preserved");
    }
    source(&mut app, Some(robot), "assets", false, true);
    let description = app
        .robot_setup
        .draft
        .as_mut()
        .unwrap()
        .description
        .as_mut()
        .unwrap();
    description.asset_root = Some(assets.to_string_lossy().into_owned());
    description.package_roots = vec![PackageRoot {
        package: "fixture_robot".into(),
        path: assets.to_string_lossy().into_owned(),
    }];
    read(&mut app);
    app.robot_apply().unwrap();
    wait(&mut app, "Topic install", |a| {
        a.robot_setup.install.is_none()
    });
    let view = app.views.iter_mut().find(|view| view.id == robot).unwrap();
    assert!(view.robot_appearance.links.is_empty());
    assert!(view.robot_appearance.collision && !view.robot_appearance.visual);
    view.set_robot_appearance(Default::default()).unwrap();
    println!("CASE passed: source replacement clears link overrides while retaining global appearance; explicit reset restores imported materials");
    let saved_appearance = robot::RobotAppearance {
        collision: true,
        opacity_permille: 650,
        links: vec![robot::LinkAppearance {
            name: "base".into(),
            visible: false,
            opacity_permille: 500,
        }],
        ..Default::default()
    };
    view.set_robot_appearance(saved_appearance.clone()).unwrap();
    let saved = app.workspace_snapshot().unwrap();
    assert_eq!(
        app.views
            .iter()
            .find(|v| v.id == robot)
            .unwrap()
            .source_label(),
        format!("{PREFIX}assets")
    );
    assert_eq!(
        saved
            .views
            .iter()
            .find(|v| v.id
                == app
                    .views
                    .iter()
                    .find(|v| v.id == robot)
                    .unwrap()
                    .workspace_id)
            .unwrap()
            .source_label(),
        format!("{PREFIX}assets")
    );
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
    // This retained workflow deliberately uses the original OccupancyGrid peer.
    // Current envelopes migrate that peer to Map; all other intent must survive.
    let mut expected = saved.clone();
    let legacy_map = expected
        .views
        .iter_mut()
        .find(|view| matches!(view.kind, studio_workspace::ViewKind::OccupancyGrid { .. }))
        .expect("The legacy Map peer is part of this retained fixture");
    let studio_workspace::ViewKind::OccupancyGrid {
        fixed_frame,
        transforms,
        clock_expectation,
    } = legacy_map.kind.clone()
    else {
        unreachable!()
    };
    legacy_map.kind = studio_workspace::ViewKind::Map {
        fixed_frame,
        transforms,
        clock_expectation,
        updates: None,
        appearance: Default::default(),
    };
    assert!(
        imported.legacy,
        "Legacy Map peer must be marked as migrated"
    );
    assert_eq!(imported.workspace, expected);
    app.workspace_review_configuration(imported.workspace)
        .unwrap();
    app.workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    wait(&mut app, "inactive workspace replacement", |a| {
        !a.workspace_frozen()
    });
    assert!(app.views.iter().all(|v| !v.enabled
        && v.subscription.is_none()
        && !v.spatial_clock_verified
        && v.robot.is_none()));
    assert!(app.views.iter().any(|v| v.robot_description.is_some()));
    assert_eq!(
        app.views
            .iter()
            .find(|v| v.robot_description.is_some())
            .unwrap()
            .robot_appearance,
        saved_appearance
    );
    assert_eq!(
        studio_workspace::application::encode(
            &saved,
            studio_workspace::application::Application::Visualizer
        )
        .unwrap(),
        bytes
    );
    println!("CASE passed: current workspace replacement migrates the legacy Map peer and restores exact Topic appearance unloaded/inactive/unverified");

    source(&mut app, None, "best", false, false);
    app.robot_load().unwrap();
    wait(&mut app, "pending description owner", |a| {
        a.robot_setup.capture.is_some()
            && a.session
                .as_ref()
                .unwrap()
                .shared
                .lock()
                .unwrap()
                .registry
                .entries()
                .iter()
                .any(|e| {
                    e.configuration.output == Output::RobotDescription && e.state == State::Active
                })
    });
    app.stop();
    app.connect();
    wait(&mut app, "explicit reconnect", |a| {
        a.connection.phase == Phase::Connected && a.discovery.received_graph
    });
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        tick(&mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.robot_setup.capture.is_none() && app.robot_setup.load.is_none());
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
    println!("CASE passed: pending capture disconnect/reconnect never replays capture, model load or display activation");
    source(&mut app, None, "best", false, false);
    app.robot_load().unwrap();
    wait(&mut app, "exit capture started", |a| {
        a.robot_setup
            .capture
            .as_ref()
            .is_some_and(|c| c.deadline.is_none())
    });
    eframe::App::on_exit(&mut app.0);
    assert!(app.session.is_none() && app.robot_setup.capture.is_none());
    println!("CASE passed: native exit callback cancels pending description reader and waits for backend cleanup");
}
