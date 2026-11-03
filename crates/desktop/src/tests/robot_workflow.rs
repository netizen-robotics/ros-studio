use super::*;
use std::{
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};
use studio_desktop::robot::{self, LoadId, LoadOptions, LoadedRobot, RobotLive};
use studio_protocol::{robot as wire, spatial_input::SpatialClockPolicy};

fn robot_view(app: &mut App) -> ViewId {
    ingest(
        app,
        Event::Graph {
            topics: vec![
                ("/one".into(), vec!["std_msgs/msg/Float64".into()]),
                ("/tf".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
                ("/tf_static".into(), vec![spatial::TF_MESSAGE_TYPE.into()]),
            ],
        },
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/robot_visualizer/articulated.urdf");
    let cancel = AtomicBool::new(false);
    let options = LoadOptions {
        package_roots: [("fixture_robot".into(), path.parent().unwrap().to_owned())].into(),
    };
    let model = robot::load_model(&path, &options, &cancel).unwrap();
    let prepared = spatial::robot_geometry::prepare_robot(&model, &cancel).unwrap();
    let frames = prepared.link_names.clone();
    let live = RobotLive::new(
        LoadId(42),
        LoadedRobot {
            model: Arc::new(model),
            prepared: Arc::new(prepared),
        },
    )
    .unwrap();
    let configuration = Configuration {
        map_updates: None,
        topic: "/tf".into(),
        message_type: spatial::TF_MESSAGE_TYPE.into(),
        best_effort: false,
        transient_local: false,
        output: Output::Robot,
        spatial: None,
        robot: Some(
            wire::Configuration {
                fixed_frame: "map".into(),
                frames,
                clock_policy: SpatialClockPolicy::VerifiedSystem,
                dynamic_tf_topic: "/tf".into(),
                dynamic_tf_best_effort: false,
                static_tf_topic: "/tf_static".into(),
            }
            .into(),
        ),
    };
    app.next_view += 1;
    let id = ViewId(app.next_view);
    let mut view = ViewState::new(id, configuration);
    view.robot = Some(live);
    app.views.push(view);
    id
}
fn robot_sample(command: &Command, sequence: u64, missing: bool) -> Event {
    let Command::Start {
        generation,
        subscription,
        configuration,
        ..
    } = command
    else {
        panic!("start")
    };
    Event::RobotObservation {
        generation: *generation,
        subscription: *subscription,
        sequence,
        backend_drops: 4,
        sample: Box::new(wire::Sample {
            contract_version: 1,
            fixed_frame: "map".into(),
            policy: wire::PosePolicy::LatestAvailable,
            links: configuration
                .robot
                .as_ref()
                .unwrap()
                .frames
                .iter()
                .enumerate()
                .map(|(i, frame)| wire::LinkResult {
                    frame: frame.clone(),
                    pose: if missing && i == 1 {
                        wire::LinkPose::Missing
                    } else {
                        wire::LinkPose::Ready {
                            resolved_stamp_ns: if i == 0 { 0 } else { 100 + i as i64 },
                            components: [0., 0., 0., 0., 0., 0., 1.],
                        }
                    },
                })
                .collect(),
            counters: wire::Counters::default(),
        }),
    }
}
#[test]
fn app_robot_explicit_clock_partial_recovery_disable_and_disconnect_clear_live_poses() {
    let (mut app, outgoing) = connected();
    let id = robot_view(&mut app);
    app.start_view(id);
    assert!(
        outgoing.try_recv().is_err(),
        "loaded geometry is not clock authorization"
    );
    app.views[0].spatial_clock_verified = true;
    app.start_view(id);
    let command = outgoing.try_recv().unwrap();
    assert!(app.views[0]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    ingest(&mut app, started(&command));
    ingest(&mut app, robot_sample(&command, 1, true));
    let poses = app.views[0].robot.as_ref().unwrap().poses();
    assert!(poses[0].is_some() && poses[1].is_none());
    ingest(&mut app, robot_sample(&command, 2, false));
    assert!(app.views[0]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_some));
    app.set_display_enabled(id, false);
    let stop = outgoing.try_recv().unwrap();
    assert!(app.views[0]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    ingest(&mut app, robot_sample(&command, 3, false));
    assert!(app.views[0]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    ingest(&mut app, stopped(&stop));
    app.stop();
    assert!(!app.views[0].enabled && !app.views[0].spatial_clock_verified);
    assert!(app.views[0].robot_token.is_none());
    assert!(
        app.views[0].robot.is_some(),
        "disable retains loaded assets only"
    );
}
#[test]
fn fresh_wrong_model_batch_clears_geometry_requests_scoped_stop_and_preserves_peer() {
    let (mut app, outgoing) = connected();
    let peer = add(&mut app, &outgoing, "/one");
    ingest(&mut app, started(&peer));
    let id = robot_view(&mut app);
    app.views[1].spatial_clock_verified = true;
    app.start_view(id);
    let command = outgoing.try_recv().unwrap();
    ingest(&mut app, started(&command));
    ingest(&mut app, robot_sample(&command, 1, false));
    let mut wrong = robot_sample(&command, 2, false);
    if let Event::RobotObservation { sample, .. } = &mut wrong {
        sample.links.swap(0, 1);
    }
    enqueue(&app, wrong);
    app.poll_backend();
    let stop = outgoing.try_recv().unwrap();
    assert!(matches!(stop, Command::Stop { .. }));
    assert_eq!(
        app.views[1].status,
        ViewStatus::Stopping,
        "fault is not cleanup proof"
    );
    assert!(app.views[1]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
    assert!(app.views[1].error.as_ref().unwrap().contains("identities"));
    ingest(&mut app, sample(&peer, 1, 123));
    assert_eq!(app.views[0].status, ViewStatus::Receiving);
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
        1
    );
    ingest(&mut app, robot_sample(&command, 3, false));
    assert!(outgoing.try_recv().is_err());
    assert!(app.views[1]
        .robot
        .as_ref()
        .unwrap()
        .poses()
        .iter()
        .all(Option::is_none));
}
