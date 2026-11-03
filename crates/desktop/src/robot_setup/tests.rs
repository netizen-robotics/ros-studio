use super::*;
use std::{path::Path, sync::mpsc, thread};
use studio_desktop::workspace as file;
use studio_protocol::{
    robot as wire,
    structured::{self, Value},
};

struct Harness {
    app: App,
    outgoing: mpsc::Receiver<Command>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.app.robot_setup.finish_shutdown().unwrap();
    }
}
fn fixture() -> RobotAssets {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    RobotAssets {
        urdf_path: root.join("articulated.urdf").to_str().unwrap().into(),
        package_roots: vec![file::PackageRoot {
            package: "fixture_robot".into(),
            path: root.to_str().unwrap().into(),
        }],
    }
}
fn ingest(app: &mut App, event: Event) {
    app.session
        .as_ref()
        .unwrap()
        .shared
        .lock()
        .unwrap()
        .ingest_for_test(event)
        .unwrap();
    app.poll_backend();
    app.robot_poll();
}
fn harness(connected: bool) -> Harness {
    let (session, outgoing) = session::Session::fixture(0);
    let mut app = App::default();
    if connected {
        app.session = Some(session);
        app.connection.phase = Phase::Starting;
        ingest(
            &mut app,
            Event::ready("jazzy".into(), "fixture-no-ROS".into()),
        );
        ingest(
            &mut app,
            Event::Graph {
                topics: vec![
                    ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                    ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                    ("/peer".into(), vec!["std_msgs/msg/Int64".into()]),
                ],
            },
        );
    }
    Harness { app, outgoing }
}
fn wait_load(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app
        .robot_setup
        .worker
        .as_ref()
        .is_some_and(AssetWorker::is_busy)
    {
        assert!(
            Instant::now() < deadline,
            "bounded load completion not observed"
        );
        app.robot_poll();
        thread::yield_now();
    }
}
fn load(app: &mut App) -> LoadId {
    app.robot_setup.draft.as_mut().unwrap().assets = fixture();
    app.robot_load().unwrap();
    wait_load(app);
    assert!(
        app.robot_setup.error.is_none(),
        "{:?}",
        app.robot_setup.error
    );
    app.robot_setup.reviewed.as_ref().unwrap().id
}
fn add_robot(h: &mut Harness) -> ViewId {
    h.app.begin_robot_setup(None);
    load(&mut h.app);
    let draft = h.app.robot_setup.draft.as_mut().unwrap();
    draft.tf.fixed_frame = "map".into();
    draft.clock_verified = true;
    h.app.robot_apply().unwrap();
    h.app.views.last().unwrap().id
}
fn enable(h: &mut Harness, id: ViewId) -> Command {
    h.app.set_display_enabled(id, true);
    let command = h.outgoing.try_recv().unwrap();
    let (generation, request, subscription) = command.ids();
    ingest(
        &mut h.app,
        Event::Started {
            generation,
            request,
            subscription,
        },
    );
    command
}
fn sample(command: &Command, sequence: u64) -> Event {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!("start")
    };
    let c = configuration.robot.as_ref().unwrap();
    Event::RobotObservation {
        generation: *generation,
        subscription: *subscription,
        sequence,
        backend_drops: 0,
        sample: Box::new(wire::Sample {
            contract_version: 1,
            fixed_frame: c.fixed_frame.clone(),
            policy: wire::PosePolicy::LatestAvailable,
            links: c
                .frames
                .iter()
                .map(|frame| wire::LinkResult {
                    frame: frame.clone(),
                    pose: wire::LinkPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                })
                .collect(),
            counters: wire::Counters::default(),
        }),
    }
}
fn peer(h: &mut Harness) -> Command {
    h.app.begin_add();
    h.app.discovery.select("/peer");
    h.app.subscribe();
    let command = h.outgoing.try_recv().unwrap();
    let (generation, request, subscription) = command.ids();
    ingest(
        &mut h.app,
        Event::Started {
            generation,
            request,
            subscription,
        },
    );
    command
}
fn peer_sample(command: &Command) -> Event {
    let (generation, _, subscription) = command.ids();
    Event::Observation {
        generation,
        subscription,
        sequence: 1,
        backend_drops: 0,
        sample: Box::new(structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/peer".into(),
            message_type: "std_msgs/msg/Int64".into(),
            source_time: None,
            value: Value::Signed {
                value: "123".into(),
            },
        }),
    }
}
#[test]
fn offline_load_review_apply_is_explicit_inactive_and_saves_only_configuration() {
    let mut h = harness(false);
    h.app.begin_robot_setup(None);
    assert!(h.app.views.is_empty());
    let loaded = load(&mut h.app);
    assert!(
        h.app.views.is_empty(),
        "worker success does not install or subscribe"
    );
    let draft = h.app.robot_setup.draft.as_mut().unwrap();
    draft.tf.fixed_frame = "map".into();
    draft.clock_verified = true;
    h.app.robot_apply().unwrap();
    let view = &h.app.views[0];
    assert_eq!(view.robot.as_ref().unwrap().model_id(), loaded);
    assert!(!view.enabled && !view.spatial_clock_verified && view.subscription.is_none());
    let saved = h.app.workspace_snapshot().unwrap();
    assert_eq!(saved.version, 7);
    assert!(
        matches!(&saved.views[0].kind,file::ViewKind::Robot {assets,clock_expectation:file::ClockExpectation::Unknown,..} if *assets==fixture())
    );
    let wire = String::from_utf8(saved.encode().unwrap()).unwrap();
    for runtime in [
        "model_id",
        "activation",
        "frames",
        "clock_verified",
        "enabled",
    ] {
        assert!(!wire.contains(runtime));
    }
    assert!(h.outgoing.try_recv().is_err());
}
#[test]
fn failed_candidate_load_and_invalid_apply_preserve_active_model_and_peer() {
    let mut h = harness(true);
    let p = peer(&mut h);
    let id = add_robot(&mut h);
    let c = enable(&mut h, id);
    ingest(&mut h.app, sample(&c, 1));
    let old = h.app.views[1].robot.as_ref().unwrap().model_id();
    h.app.begin_robot_setup(Some(id));
    h.app
        .robot_setup
        .draft
        .as_mut()
        .unwrap()
        .assets
        .urdf_path
        .push_str(".missing");
    h.app.robot_load().unwrap();
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.error.is_some());
    assert!(h.app.robot_apply().is_err());
    assert_eq!(h.app.views[1].robot.as_ref().unwrap().model_id(), old);
    assert!(
        h.app.views[1].enabled
            && h.app.views[1]
                .robot
                .as_ref()
                .unwrap()
                .poses()
                .iter()
                .all(Option::is_some)
    );
    assert!(h.outgoing.try_recv().is_err());
    h.app.robot_setup.draft.as_mut().unwrap().assets = fixture();
    h.app.robot_load().unwrap();
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.reviewed.is_some());
    h.app.robot_setup.draft.as_mut().unwrap().tf.fixed_frame = "/invalid".into();
    assert!(h.app.robot_apply().is_err());
    assert!(h.outgoing.try_recv().is_err());
    ingest(&mut h.app, peer_sample(&p));
    assert_eq!(h.app.views[0].status, ViewStatus::Receiving);
}
#[test]
fn reviewed_replacement_waits_for_exact_cleanup_then_installs_inactive_without_replay() {
    let mut h = harness(true);
    let p = peer(&mut h);
    let id = add_robot(&mut h);
    let c = enable(&mut h, id);
    ingest(&mut h.app, sample(&c, 1));
    let old = h.app.views[1].robot.as_ref().unwrap().model_id();
    h.app.begin_robot_setup(Some(id));
    let new = load(&mut h.app);
    assert_ne!(new, old);
    assert!(
        h.app.views[1].enabled
            && h.app.views[1]
                .robot
                .as_ref()
                .unwrap()
                .poses()
                .iter()
                .all(Option::is_some)
    );
    h.app.robot_apply().unwrap();
    let stop = h.outgoing.try_recv().unwrap();
    let (generation, request, subscription) = stop.ids();
    assert!(matches!(stop, Command::Stop { .. }));
    assert_eq!(h.app.views[1].robot.as_ref().unwrap().model_id(), old);
    ingest(
        &mut h.app,
        Event::Stopped {
            generation,
            request: RequestId(request.0 + 1),
            subscription,
        },
    );
    assert_eq!(h.app.views[1].robot.as_ref().unwrap().model_id(), old);
    assert!(h.app.robot_setup.install.is_some());
    ingest(
        &mut h.app,
        Event::Stopped {
            generation,
            request,
            subscription,
        },
    );
    assert_eq!(h.app.views[1].robot.as_ref().unwrap().model_id(), new);
    assert!(!h.app.views[1].enabled && h.app.views[1].subscription.is_none());
    assert!(h.app.views[1]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    assert!(
        h.outgoing.try_recv().is_err(),
        "replacement never enables itself"
    );
    ingest(&mut h.app, sample(&c, 2));
    assert!(h.app.views[1]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    ingest(&mut h.app, peer_sample(&p));
    assert_eq!(h.app.views[0].status, ViewStatus::Receiving);
}
#[test]
fn cancelled_setup_completion_cannot_install_or_populate_a_new_setup_generation() {
    let mut h = harness(false);
    h.app.begin_robot_setup(None);
    h.app.robot_setup.draft.as_mut().unwrap().assets = fixture();
    h.app.robot_load().unwrap();
    let stale = h.app.robot_setup.load.as_ref().unwrap().id;
    h.app.cancel_robot_setup();
    h.app.begin_robot_setup(None);
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.reviewed.is_none() && h.app.views.is_empty());
    let fresh = load(&mut h.app);
    assert_ne!(stale, fresh);
    assert!(h.app.views.is_empty());
    assert!(h.outgoing.try_recv().is_err());
}
#[test]
fn restored_robot_stays_unloaded_and_clock_unverified_until_explicit_local_load() {
    let mut h = harness(true);
    add_robot(&mut h);
    let mut saved = h.app.workspace_snapshot().unwrap();
    let file::ViewKind::Robot { assets, .. } = &mut saved.views[0].kind else {
        panic!("Robot")
    };
    assets.urdf_path.push_str(".unavailable");
    h.app.workspace_review_configuration(saved.clone()).unwrap();
    h.app
        .workspace_begin_apply(file::ApplyApproval::DiscardUnsaved)
        .unwrap();
    h.app.workspace_poll(&egui::Context::default());
    let view = &h.app.views[0];
    assert!(
        view.robot.is_none()
            && view.robot_token.is_none()
            && !view.enabled
            && !view.spatial_clock_verified
    );
    assert!(view.configuration.robot.as_ref().unwrap().frames.is_empty());
    let id = view.id;
    h.app.start_view(id);
    assert!(h.outgoing.try_recv().is_err());
    h.app.begin_robot_setup(Some(id));
    h.app.robot_load().unwrap();
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.error.is_some() && h.app.views[0].robot.is_none());
    assert_eq!(h.app.workspace_snapshot().unwrap(), saved);
}

#[test]
fn same_path_failed_or_cancelled_reload_revokes_candidate_review_without_stopping_valid_model() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let folder = std::env::temp_dir().join(format!(
        "studio-robot-reload-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir_all(folder.join("meshes")).unwrap();
    let _cleanup = Temp(folder.clone());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    for name in [
        "articulated.urdf",
        "meshes/wedge-ascii.stl",
        "meshes/wedge-binary.stl",
    ] {
        std::fs::copy(source.join(name), folder.join(name)).unwrap();
    }
    let assets = RobotAssets {
        urdf_path: folder.join("articulated.urdf").to_str().unwrap().into(),
        package_roots: vec![file::PackageRoot {
            package: "fixture_robot".into(),
            path: folder.to_str().unwrap().into(),
        }],
    };
    let mut h = harness(true);
    h.app.begin_robot_setup(None);
    let draft = h.app.robot_setup.draft.as_mut().unwrap();
    draft.assets = assets.clone();
    draft.tf.fixed_frame = "map".into();
    h.app.robot_load().unwrap();
    wait_load(&mut h.app);
    h.app.robot_setup.draft.as_mut().unwrap().clock_verified = true;
    h.app.robot_apply().unwrap();
    let id = h.app.views[0].id;
    let command = enable(&mut h, id);
    ingest(&mut h.app, sample(&command, 1));
    let old = h.app.views[0].robot.as_ref().unwrap().model_id();
    h.app.begin_robot_setup(Some(id));
    assert!(h.app.robot_setup.reviewed.is_some());
    std::fs::write(&assets.urdf_path, b"<robot name='broken'>").unwrap();
    h.app.robot_load().unwrap();
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.error.is_some());
    assert!(
        h.app.robot_setup.reviewed.is_none(),
        "a failed same-path reload cannot authorize the old snapshot"
    );
    assert!(h.app.robot_apply().is_err());
    assert!(h.outgoing.try_recv().is_err());
    assert_eq!(h.app.views[0].robot.as_ref().unwrap().model_id(), old);
    assert!(h.app.views[0].enabled);
    std::fs::copy(source.join("articulated.urdf"), &assets.urdf_path).unwrap();
    h.app.begin_robot_setup(Some(id));
    h.app.robot_load().unwrap();
    let load = h.app.robot_setup.load.as_ref().unwrap().id;
    assert!(h.app.robot_setup.worker.as_mut().unwrap().cancel(load));
    wait_load(&mut h.app);
    assert!(h.app.robot_setup.error.is_some());
    assert!(h.app.robot_setup.reviewed.is_none());
    assert!(h.app.robot_apply().is_err());
    assert!(h.outgoing.try_recv().is_err());
    assert!(
        h.app.views[0].enabled
            && h.app.views[0]
                .robot
                .as_ref()
                .unwrap()
                .poses()
                .iter()
                .all(Option::is_some)
    );
}
