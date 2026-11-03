//! Opt-in, serialized actual-ROS decoder qualification; no session admission.
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use studio_backend::{
    map_ingress::Continuity,
    map_producer::MapProducer,
    native_topic::NativeTopic,
    spatial_session::{ClockPolicy, SpatialSession},
};
use studio_protocol::{
    map_grid,
    map_snapshot::{Message, Owner, Received, Receiver},
    mapping,
    subscriptions::{Generation, SubscriptionId},
};

struct Publisher(Child);
impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn receive<T>(mut poll: impl FnMut() -> Result<Option<T>, String>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(value) = poll().unwrap() {
            return value;
        }
        assert!(Instant::now() < deadline, "Native grid receive timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn next_wire(producer: &mut MapProducer, tf: &mut SpatialSession) -> Message {
    let message = producer
        .poll(
            tf,
            Instant::now(),
            &studio_protocol::spatial::Counters::default(),
        )
        .unwrap()
        .unwrap();
    Message::decode(&message.encode().unwrap()).unwrap()
}
fn pump_wire(
    producer: &mut MapProducer,
    tf: &mut SpatialSession,
    receiver: &mut Receiver,
) -> Received {
    for _ in 0..studio_protocol::map_snapshot::MAX_CHUNKS {
        let result = receiver.receive(&next_wire(producer, tf)).unwrap();
        if result != Received::Pending {
            return result;
        }
    }
    panic!("Map transfer did not complete within chunk budget");
}
#[test]
#[ignore = "Requires explicit localhost domain187 matching RMW; run serialized via scripts/dev"]
fn maximum_full_and_patch_cross_native_ros_boundary() {
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
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/map_publisher.py");
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
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let ready = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    println!("reader_pid={} publisher={ready}", std::process::id());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready).unwrap()["ready"],
        true
    );
    let mut full = NativeTopic::open(
        "/studio_fixture/raw_grid",
        mapping::MAP_MESSAGE_TYPE,
        false,
        true,
        false,
    )
    .unwrap();
    let mut updates = NativeTopic::open(
        "/studio_fixture/raw_grid_updates",
        map_grid::UPDATE_MESSAGE_TYPE,
        false,
        false,
        false,
    )
    .unwrap();
    assert!(full.poll_grid_update().is_err());
    assert!(updates.poll_grid().is_err());
    assert!(updates.publication().is_err());
    assert!(updates.message_loss_count().is_err());
    let full_loss = full.enable_message_loss_events().unwrap();
    let update_loss = updates.enable_message_loss_events().unwrap();
    assert_eq!(updates.enable_message_loss_events().unwrap(), update_loss);
    println!("message-lost support full={full_loss} updates={update_loss}");
    writeln!(publisher.0.stdin.as_mut().unwrap(), "full").unwrap();
    let grid = receive(|| full.poll_grid());
    assert_eq!(grid.metadata.width, 2048);
    assert_eq!(grid.metadata.height, 2048);
    assert_eq!(grid.metadata.stamp_ns, 2_000_000_003);
    assert_eq!(grid.metadata.frame_id, "source");
    assert_eq!(grid.cell_bytes(), map_grid::MAX_CELLS);
    assert!(grid
        .cells
        .iter()
        .enumerate()
        .all(|(i, v)| *v == i as u8 as i8));
    let wire_owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(1),
        topic: "/studio_fixture/raw_grid".into(),
        fixed_frame: "source".into(),
    };
    let mut ingress = MapProducer::new(
        wire_owner.clone(),
        true,
        [Some(full_loss), Some(update_loss)],
        Instant::now(),
    )
    .unwrap();
    let owner = ingress.token().unwrap();
    let mut tf = SpatialSession::new().unwrap();
    tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
    let mut wire = Receiver::new(wire_owner).unwrap();
    ingress.full(owner, 1, grid, Instant::now()).unwrap();
    let first = next_wire(&mut ingress, &mut tf);
    assert_eq!(wire.receive(&first).unwrap(), Received::Pending);
    assert!(wire.in_flight_cell_bytes() > 0);
    assert!(rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .contains("full"));
    writeln!(publisher.0.stdin.as_mut().unwrap(), "patch").unwrap();
    let patch = receive(|| updates.poll_grid_update());
    let first_publication = updates.publication().unwrap();
    println!("first update publication={first_publication:?}");
    assert_eq!(
        (patch.x, patch.y, patch.width, patch.height),
        (0, 0, 2048, 2048)
    );
    assert_eq!(patch.stamp_ns, 2_000_000_004);
    assert_eq!(patch.cells.len(), map_grid::MAX_CELLS);
    assert!(patch
        .cells
        .iter()
        .enumerate()
        .all(|(i, v)| *v == i as u8 as i8));
    ingress
        .patch(owner, 2, patch, first_publication.clone(), Instant::now())
        .unwrap();
    assert!(rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .contains("patch"));
    assert!(
        matches!(pump_wire(&mut ingress,&mut tf,&mut wire),Received::Complete(c) if c.header.metadata.stamp_ns==2_000_000_004)
    );
    assert_eq!(wire.receive(&first).unwrap(), Received::Stale);
    let before = updates.message_loss_count().unwrap();
    writeln!(publisher.0.stdin.as_mut().unwrap(), "burst").unwrap();
    assert!(rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .contains("burst"));
    // Deliberately let the depth2 middleware history fill before taking.
    std::thread::sleep(Duration::from_millis(300));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut received = Vec::new();
    let mut publications = Vec::new();
    while Instant::now() < deadline {
        if let Some(patch) = updates.poll_grid_update().unwrap() {
            received.push(patch.x);
            let publication = updates.publication().unwrap();
            publications.push(publication.clone());
            ingress
                .patch(
                    owner,
                    2 + received.len() as u64,
                    patch,
                    publication,
                    Instant::now(),
                )
                .unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(received.last(), Some(&31));
    let after = updates.message_loss_count().unwrap();
    println!("depth2 burst sent=32 received={received:?} reported-loss before={before:?} after={after:?}");
    println!("burst publications={publications:?}");
    for (x, publication) in received.iter().zip(&publications) {
        assert_eq!(publication.publisher_id, first_publication.publisher_id);
        if let (Some(first), Some(current)) = (first_publication.sequence, publication.sequence) {
            assert_eq!(current - first, *x as u64 + 1);
        }
    }
    if first_publication.sequence.is_some() {
        assert_eq!(ingress.source().assembly().cell_bytes(), 0);
        assert_eq!(ingress.source().counters().sequence_faults, 1);
        assert_eq!(ingress.source().recovery_floor(), Some(3_000_000_031));
        assert!(
            matches!(pump_wire(&mut ingress,&mut tf,&mut wire),Received::Cleared(c) if c.source_status.recovery_floor_ns==Some(3_000_000_031))
        );
        assert_eq!(wire.in_flight_cell_bytes(), 0);
        println!("ordered ingress: observed gap invalidated base and wire; waiting for full >=3000000031");
        writeln!(publisher.0.stdin.as_mut().unwrap(), "full").unwrap();
        let old = receive(|| full.poll_grid());
        assert_eq!(
            ingress
                .full(owner, 3 + received.len() as u64, old, Instant::now())
                .unwrap(),
            map_grid::Apply::Stale
        );
        assert_eq!(ingress.source().assembly().cell_bytes(), 0);
        rx.recv_timeout(Duration::from_secs(20)).unwrap();
    } else {
        assert_eq!(ingress.source().continuity(), Continuity::Unknown);
        assert_eq!(
            ingress.source().assembly().snapshot().unwrap().cells[31],
            -33
        );
        assert!(
            matches!(pump_wire(&mut ingress,&mut tf,&mut wire),Received::Complete(c) if c.header.source_status.continuity==Continuity::Unknown && c.grid.cells[31]==-33)
        );
        println!(
            "ordered ingress: sequence unsupported; updated map and wire continuity remain Unknown"
        );
    }
    writeln!(publisher.0.stdin.as_mut().unwrap(), "recovery").unwrap();
    let recovered = receive(|| full.poll_grid());
    ingress
        .full(owner, 4 + received.len() as u64, recovered, Instant::now())
        .unwrap();
    assert_eq!(
        ingress.source().assembly().cell_bytes(),
        map_grid::MAX_CELLS
    );
    assert_eq!(
        ingress
            .source()
            .assembly()
            .snapshot()
            .unwrap()
            .metadata
            .stamp_ns,
        4_000_000_000
    );
    rx.recv_timeout(Duration::from_secs(20)).unwrap();
    println!(
        "ordered ingress: fresh full accepted at4000000000; continuity={:?}",
        ingress.source().continuity()
    );
    assert!(
        matches!(pump_wire(&mut ingress,&mut tf,&mut wire),Received::Complete(c) if c.header.metadata.stamp_ns==4_000_000_000 && c.grid.cell_bytes()==map_grid::MAX_CELLS)
    );
    assert_eq!(ingress.cell_bytes(), (map_grid::MAX_CELLS, 0));
    ingress.stop();
    assert_eq!(ingress.cell_bytes(), (0, 0));
    wire.stop();
    println!("snapshot2: maximum roundtrip, supersession, recovery and Stop passed");
    drop(updates);
    drop(full);
    writeln!(publisher.0.stdin.as_mut().unwrap(), "stop").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = publisher.0.try_wait().unwrap() {
            assert!(status.success());
            println!("publisher_pid={} exited={status}", publisher.0.id());
            break;
        }
        assert!(Instant::now() < deadline, "Publisher shutdown timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
}
