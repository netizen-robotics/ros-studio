//! Explicit local actual-backend acceptance; never part of ordinary offline cargo test.
use super::*;
fn wait(app: &mut Tools, test: impl Fn(&Tools) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(35);
    while !test(app) {
        app.poll();
        assert!(Instant::now() < deadline, "Timed out: {}", app.status);
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "Requires scripts/test-split-apps-ros.py localhost publishers and explicit middleware"]
fn split_real_ros_tools_workflow() {
    assert_eq!(std::env::var("ROS_DOMAIN_ID").unwrap(), "187");
    assert_eq!(std::env::var("ROS_LOCALHOST_ONLY").unwrap(), "1");
    assert_eq!(
        std::env::var("ROS_AUTOMATIC_DISCOVERY_RANGE").unwrap(),
        "LOCALHOST"
    );
    for key in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(std::env::var(key).unwrap_or_default().is_empty());
    }
    struct Finished;
    impl Drop for Finished {
        fn drop(&mut self) {
            session::finish_shutdown();
        }
    }
    let _finished = Finished;
    let mut app = Tools::default();
    app.domain = "187".into();
    app.cyclone = std::env::var("RMW_IMPLEMENTATION").unwrap() == "rmw_cyclonedds_cpp";
    app.connect(&egui::Context::default());
    wait(&mut app, |a| {
        a.connected
            && a.topics
                .iter()
                .any(|(t, _)| t == "/studio_fixture/split/numeric")
    });
    assert!(app.views.is_empty());
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
    app.topic = "/studio_fixture/split/numeric".into();
    app.message_type = "std_msgs/msg/UInt64".into();
    app.add(false);
    let first = app.selected.unwrap();
    assert_eq!(app.views[0].status, Status::Stopped);
    app.start(first);
    wait(&mut app, |a| a.views[0].sample.is_some());
    let original = app.views[0].subscription;
    // Browsing never replaces an active reader.
    app.topic = "/studio_fixture/split/best".into();
    app.message_type = "std_msgs/msg/String".into();
    assert_eq!(app.views[0].subscription, original);
    let field = app.views[0].fields[0].clone();
    app.views[0].draft.kind = file::ViewKind::numeric_plot(&field);
    app.apply_draft(first);
    wait(&mut app, |a| !a.views[0].trace.points().is_empty());
    assert_ne!(app.views[0].subscription, original);
    app.add(false);
    let best = app.selected.unwrap();
    app.views[1].draft.qos.reliability = file::Reliability::Reliable;
    app.apply_draft(best);
    app.start(best);
    wait(&mut app, |a| a.views[1].status == Status::Waiting);
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        app.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(app.views[1].sample.is_none());
    let peer = app.views[0].samples;
    app.views[1].draft.qos.reliability = file::Reliability::BestEffort;
    app.apply_draft(best);
    wait(&mut app, |a| a.views[1].sample.is_some());
    assert!(app.views[0].samples > peer);
    app.topic = "/studio_fixture/split/retained".into();
    app.message_type = "std_msgs/msg/String".into();
    app.add(false);
    let retained = app.selected.unwrap();
    app.views[2].draft.qos.reliability = file::Reliability::Reliable;
    app.views[2].draft.qos.durability = file::Durability::TransientLocal;
    app.apply_draft(retained);
    app.start(retained);
    wait(&mut app, |a| a.views[2].sample.is_some());
    app.topic = "/studio_fixture/split/image".into();
    app.message_type = images::IMAGE_MESSAGE_TYPE.into();
    app.add(true);
    let image = app.selected.unwrap();
    app.start(image);
    wait(&mut app, |a| a.views[3].samples > 0);
    assert!(app.views[3].error.is_none());
    app.stop_view(best);
    wait(&mut app, |a| a.views[1].subscription.is_none());
    assert!(app.views[1].sample.is_none());
    assert!(app.views[0].subscription.is_some());
    app.disconnect();
    assert!(app
        .views
        .iter()
        .all(|v| v.subscription.is_none() && v.sample.is_none()));
    wait(&mut app, |a| a.retiring.is_none());
    app.connect(&egui::Context::default());
    wait(&mut app, |a| a.connected && a.graph_received);
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
    assert!(app.views.iter().all(|v| v.status == Status::Stopped));
    // Exercise the exact Workbench inspection action boundary, independently of
    // the existing legacy add/start coverage above. Binding and page changes do
    // not issue starts; explicit Create/Start does.
    app.apply_inspection(crate::inspection_ui::Action::Create {
        topic: "/studio_fixture/split/reliable".into(),
        typ: "std_msgs/msg/String".into(),
        image: false,
        start: false,
    });
    let workbench_id = app.selected.unwrap();
    assert!(app.views.last().unwrap().subscription.is_none());
    app.apply_inspection(crate::inspection_ui::Action::Start(workbench_id));
    wait(&mut app, |a| a.views.last().unwrap().sample.is_some());
    let workbench_owner = app.views.last().unwrap().subscription;
    app.apply_inspection(crate::inspection_ui::Action::Create {
        topic: "/studio_fixture/split/image".into(),
        typ: images::IMAGE_MESSAGE_TYPE.into(),
        image: true,
        start: false,
    });
    assert_eq!(app.views[4].subscription, workbench_owner);
    assert!(app.views[5].subscription.is_none());
    app.apply_inspection(crate::inspection_ui::Action::Stop(workbench_id));
    wait(&mut app, |a| a.views[4].subscription.is_none());
    app.disconnect();
}
