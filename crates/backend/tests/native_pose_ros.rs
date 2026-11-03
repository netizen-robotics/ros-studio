//! Isolated real ROS reader + native TF cache + pure receiver; no session/UI.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use studio_backend::{
    native_topic::NativeTopic,
    pose_producer::PoseProducer,
    spatial_session::{ClockPolicy, SpatialSession, StreamKind, Transform},
};
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{
        self as wire,
        transport::{Applied, Configuration, HistoryConfiguration, Invalidation, Receiver},
        Kind, Owner, ResetReason, Snapshot,
    },
    subscriptions::{Generation, RequestId, SubscriptionId},
};
struct Publisher(Child);
impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn take(reader: &mut NativeTopic) -> Result<wire::Decoded, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(sample) = reader.poll_pose_covariance().unwrap() {
            return sample;
        }
        assert!(Instant::now() < deadline, "Native pose receive timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "Requires serialized localhost/domain187 matching DDS and project Python"]
fn complete_pose_sources_cross_real_ros_and_native_cache_boundary() {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    assert!(matches!(
        std::env::var("RMW_IMPLEMENTATION").as_deref(),
        Ok("rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp")
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
    let prefix = std::env::split_paths(&std::env::var_os("AMENT_PREFIX_PATH").unwrap())
        .next()
        .unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pose_publisher.py");
    let mut publisher = Publisher(
        Command::new(prefix.join("bin/python"))
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = publisher.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(8);
    let worker = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let ready: Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(20)).unwrap()).unwrap();
    assert_eq!(ready["ready"], true);
    println!(
        "test_pid={} publisher_pid={}",
        std::process::id(),
        publisher.0.id()
    );
    let mut pose = NativeTopic::open(
        "/studio_fixture/raw_pose",
        wire::POSE_MESSAGE_TYPE,
        false,
        true,
        false,
    )
    .unwrap();
    let mut odom = NativeTopic::open(
        "/studio_fixture/raw_odom",
        wire::ODOMETRY_MESSAGE_TYPE,
        false,
        true,
        false,
    )
    .unwrap();
    assert!(pose.poll_markers().is_err());
    assert!(odom.poll_mapping().is_err());
    let p = take(&mut pose).unwrap();
    let o = take(&mut odom).unwrap();
    assert_eq!(p.kind, Kind::PoseWithCovariance);
    assert_eq!(p.source.child_frame_id, None);
    assert_eq!(o.kind, Kind::Odometry);
    assert_eq!(o.source.child_frame_id.as_deref(), Some("base"));
    let v = [1., 2., 3., 0.1, 0.2, 0.3];
    let expected: [f64; 36] = std::array::from_fn(|i| v[i / 6] * v[i % 6]);
    for input in [&p, &o] {
        assert_eq!(input.acquisition_stamp_ns, 2_000_000_003);
        assert_eq!(input.source.frame_id, "odom");
        assert_eq!(input.components, [1., 2., 3., 0., 0., 0., 1.]);
        assert_eq!(input.covariance.row_major(), &expected);
        assert_eq!(
            wire::Decoded::decode(&input.encode().unwrap()).unwrap(),
            *input
        );
    }
    println!("retained late join, exact metadata, all36 covariance values and independent type dispatch passed");
    let owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(1),
        kind: Kind::Odometry,
        topic: o.topic.clone(),
        fixed_frame: "world".into(),
    };
    let configuration = Configuration {
        kind: Kind::Odometry,
        history: Some(HistoryConfiguration::default()),
    };
    let mut producer = PoseProducer::new(owner.clone(), configuration.settings().unwrap()).unwrap();
    let mut receiver = Receiver::new(owner, configuration).unwrap();
    let mut tf = SpatialSession::new().unwrap();
    tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
    tf.insert(
        StreamKind::Dynamic,
        &Transform {
            parent: "world".into(),
            child: "odom".into(),
            stamp_ns: 2_000_000_003,
            components: [10., 0., 0., 0., 0., 0., 1.],
        },
    )
    .unwrap();
    producer.input(1, o, 0).unwrap();
    let snapshot = Snapshot::decode(&producer.poll(&mut tf, 0, true).unwrap().unwrap()).unwrap();
    assert!(
        matches!(snapshot.latest.as_ref().unwrap().transform,LatestPose::Ready {resolved_stamp_ns:2_000_000_003,components} if components[0]==10.)
    );
    assert_eq!(snapshot.trail.len(), 1);
    assert_eq!(receiver.apply(snapshot).unwrap(), Applied::Updated);
    let mut send = |command: Value| {
        writeln!(publisher.0.stdin.as_mut().unwrap(), "{command}").unwrap();
        let ack: Value =
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(20)).unwrap()).unwrap();
        assert_eq!(ack["sent"], command["target"]);
    };
    send(json!({"op":"publish","target":"odom","stamp":0}));
    let zero = take(&mut odom).unwrap();
    assert_eq!(zero.acquisition_stamp_ns, 0);
    producer.input(2, zero, 50_000_000).unwrap();
    let newest =
        Snapshot::decode(&producer.poll(&mut tf, 50_000_000, true).unwrap().unwrap()).unwrap();
    assert_eq!(newest.last_reset, Some(ResetReason::TimePolicyChanged));
    assert_eq!(newest.trail.len(), 1);
    assert!(matches!(
        newest.latest.as_ref().unwrap().transform,
        LatestPose::Ready {
            resolved_stamp_ns: 2_000_000_003,
            ..
        }
    ));
    receiver.apply(newest).unwrap();
    receiver.begin_clear(RequestId(3)).unwrap();
    producer.clear().unwrap();
    let cleared =
        Snapshot::decode(&producer.poll(&mut tf, 50_000_000, true).unwrap().unwrap()).unwrap();
    let barrier = Invalidation {
        contract_version: wire::CONTRACT_VERSION,
        owner: cleared.owner.clone(),
        revision: cleared.revision,
        continuity_epoch: cleared.continuity_epoch,
        reason: ResetReason::LocalClear,
    };
    receiver.invalidate(&barrier).unwrap();
    assert!(receiver.acknowledge_clear(RequestId(3), &barrier).unwrap());
    assert!(receiver.snapshot().is_none());
    send(json!({"op":"publish","target":"pose","invalid":true}));
    assert!(take(&mut pose).is_err());
    send(json!({"op":"publish","target":"pose","stamp":3}));
    assert_eq!(take(&mut pose).unwrap().acquisition_stamp_ns, 3_000_000_003);
    println!("real source -> native exact/latest cache -> snapshot -> receiver/Clear and invalid-sample separation passed");
    receiver.stop();
    producer.stop();
    drop(pose);
    drop(odom);
    writeln!(
        publisher.0.stdin.as_mut().unwrap(),
        "{}",
        json!({"op":"stop"})
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = publisher.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.join().unwrap();
    println!(
        "publisher_pid={} clean exit; native readers dropped",
        publisher.0.id()
    );
}
