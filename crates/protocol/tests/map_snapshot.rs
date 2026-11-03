use studio_protocol::{
    binary,
    map_grid::{Counters, Grid, Metadata, MAX_CELLS},
    map_snapshot::{
        Chunk, Clear, ClearReason, Owner, Received, Receiver, Snapshot, SourceStatus,
        CONTRACT_VERSION, MAX_CHUNKS, MAX_CHUNK_BYTES, MAX_SAMPLE_BYTES,
    },
    mapping,
    subscriptions::{Generation, SubscriptionId},
};
fn owner() -> Owner {
    Owner {
        generation: Generation(3),
        subscription: SubscriptionId(7),
        topic: "/map".into(),
        fixed_frame: "world".into(),
    }
}
fn source(width: u32, height: u32) -> Grid {
    Grid {
        metadata: Metadata {
            frame_id: "map".into(),
            stamp_ns: 10,
            width,
            height,
            resolution: 0.05,
            origin: [0., 0., 0., 0., 0., 0., 1.],
            map_load_time_ns: 1,
        },
        cells: (0..width as usize * height as usize)
            .map(|i| i as u8 as i8)
            .collect(),
    }
}
fn snapshot(width: u32, height: u32, revision: u64) -> Snapshot {
    Snapshot::new(
        owner(),
        revision,
        source(width, height),
        [0., 0., 0., 0., 0., 0., 1.],
        mapping::Counters::default(),
        Counters::default(),
        SourceStatus::default(),
    )
    .unwrap()
}
fn clear(revision: u64) -> Clear {
    Clear {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision,
        reason: ClearReason::QueueOverflow,
        metadata: None,
        source_status: SourceStatus::default(),
    }
}

#[test]
fn diagnostics_are_required_strict_and_versioned_without_claiming_freshness() {
    let snapshot = snapshot(1, 1, 1);
    let chunk = snapshot.chunk(0).unwrap();
    for field in [
        "full_loss_events",
        "update_loss_events",
        "recovery_floor_ns",
    ] {
        let mut value = serde_json::to_value(&chunk).unwrap();
        value["header"]["source_status"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(Chunk::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = serde_json::to_value(&chunk).unwrap();
    value["header"]["contract_version"] = serde_json::json!(1);
    assert!(Chunk::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(&chunk).unwrap();
    value["header"]
        .as_object_mut()
        .unwrap()
        .remove("source_status");
    assert!(Chunk::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(&chunk).unwrap();
    value["header"]["source_status"]["continuity"] = serde_json::json!("verified");
    assert!(Chunk::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut bad = chunk.clone();
    bad.header.source_status.recovery_floor_ns = Some(10);
    assert!(bad.validate().is_err());
    let mut bad = chunk.clone();
    bad.header.source_status.update_loss_events = Some(true);
    assert!(bad.validate().is_err());
    let mut status = SourceStatus {
        continuity: studio_protocol::map_grid::Continuity::Unknown,
        full_loss_events: Some(true),
        update_loss_events: Some(false),
        ..Default::default()
    };
    status.validate().unwrap();
    status.recovery_floor_ns = Some(-1);
    assert!(status.validate().is_err());
    let mut clear = clear(2);
    clear.source_status.continuity = studio_protocol::map_grid::Continuity::Unknown;
    clear.source_status.recovery_floor_ns = Some(10);
    clear.validate().unwrap();
    clear.reason = ClearReason::InvalidTransform;
    clear.metadata = Some(source(1, 1).metadata);
    assert!(clear.validate().is_err());
    clear.source_status.recovery_floor_ns = None;
    let mut value = serde_json::to_value(&clear).unwrap();
    value.as_object_mut().unwrap().remove("metadata");
    assert!(Clear::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    assert_eq!(Clear::decode(&clear.encode().unwrap()).unwrap(), clear);
}

#[test]
fn maximum_snapshot_roundtrips_atomically_in_twenty_two_bounded_chunks() {
    let source = snapshot(2048, 2048, 1);
    assert_eq!(source.cell_bytes(), MAX_CELLS);
    assert_eq!(source.chunk_count(), 22);
    assert_eq!(source.chunk_count(), MAX_CHUNKS);
    let mut receiver = Receiver::new(owner()).unwrap();
    for index in 0..source.chunk_count() {
        let encoded = source.chunk(index).unwrap().encode().unwrap();
        assert!(encoded.len() < MAX_SAMPLE_BYTES);
        let received = receiver.push(&Chunk::decode(&encoded).unwrap()).unwrap();
        if index + 1 == source.chunk_count() {
            let Received::Complete(complete) = received else {
                panic!("expected complete map")
            };
            assert_eq!(complete.header, *source.header());
            assert_eq!(complete.grid.cell_bytes(), MAX_CELLS);
            assert!(complete
                .grid
                .cells
                .iter()
                .enumerate()
                .all(|(i, v)| *v == i as u8 as i8));
            assert_eq!(receiver.in_flight_cell_bytes(), 0);
        } else {
            assert_eq!(received, Received::Pending);
            assert_eq!(receiver.in_flight_cell_bytes(), MAX_CELLS);
        }
    }
    assert!(source.chunk(usize::MAX).is_none());
    assert_eq!(
        receiver.push(&source.chunk(0).unwrap()).unwrap(),
        Received::Stale
    );
}

#[test]
fn newer_revision_supersedes_partial_work_and_old_chunks_cannot_replace_complete_output() {
    let old = snapshot(2048, 100, 1);
    let new = snapshot(2, 2, 2);
    let mut receiver = Receiver::new(owner()).unwrap();
    assert_eq!(
        receiver.push(&old.chunk(0).unwrap()).unwrap(),
        Received::Pending
    );
    assert_eq!(receiver.in_flight_cell_bytes(), 204800);
    let Received::Complete(complete) = receiver.push(&new.chunk(0).unwrap()).unwrap() else {
        panic!("new map")
    };
    assert_eq!(&*complete.grid.cells, &[0, 1, 2, 3]);
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
    let mut stale = old.chunk(1).unwrap();
    stale.data_base64 = "not base64".into();
    assert_eq!(receiver.push(&stale).unwrap(), Received::Stale);
    assert_eq!(&*complete.grid.cells, &[0, 1, 2, 3]);
}

#[test]
fn clear_releases_partial_storage_without_replay_and_next_full_revision_can_recover() {
    let old = snapshot(2048, 100, 1);
    let mut receiver = Receiver::new(owner()).unwrap();
    receiver.push(&old.chunk(0).unwrap()).unwrap();
    let command = Clear::decode(&clear(2).encode().unwrap()).unwrap();
    assert_eq!(
        receiver.clear(&command).unwrap(),
        Received::Cleared(Box::new(command.clone()))
    );
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
    assert_eq!(
        receiver.push(&old.chunk(1).unwrap()).unwrap(),
        Received::Stale
    );
    assert_eq!(receiver.clear(&command).unwrap(), Received::Stale);
    assert!(matches!(
        receiver.push(&snapshot(1, 1, 3).chunk(0).unwrap()).unwrap(),
        Received::Complete(_)
    ));
    receiver.stop();
    assert_eq!(
        receiver
            .push(&snapshot(2048, 100, 4).chunk(0).unwrap())
            .unwrap(),
        Received::Stale
    );
    assert_eq!(receiver.clear(&clear(5)).unwrap(), Received::Stale);
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
}

#[test]
fn foreign_owner_is_ignored_but_current_source_binding_contradictions_latch_fault() {
    let data = snapshot(2048, 100, 1);
    let mut receiver = Receiver::new(owner()).unwrap();
    receiver.push(&data.chunk(0).unwrap()).unwrap();
    let reserved = receiver.in_flight_cell_bytes();
    for field in 0..2 {
        let mut foreign = data.chunk(1).unwrap();
        if field == 0 {
            foreign.header.owner.generation = Generation(2);
        } else {
            foreign.header.owner.subscription = SubscriptionId(8);
        }
        foreign.header.revision = u64::MAX;
        foreign.data_base64 = "bad".into();
        assert_eq!(receiver.push(&foreign).unwrap(), Received::Stale);
        assert_eq!(receiver.in_flight_cell_bytes(), reserved);
    }
    let mut foreign = clear(2);
    foreign.owner.generation = Generation(2);
    assert_eq!(receiver.clear(&foreign).unwrap(), Received::Stale);
    for field in 0..2 {
        let mut receiver = Receiver::new(owner()).unwrap();
        receiver.push(&data.chunk(0).unwrap()).unwrap();
        let mut bad = data.chunk(1).unwrap();
        if field == 0 {
            bad.header.owner.topic = "/other".into();
        } else {
            bad.header.owner.fixed_frame = "other".into();
        }
        assert!(receiver.push(&bad).is_err());
        assert_eq!(receiver.in_flight_cell_bytes(), 0);
        assert!(receiver.push(&snapshot(1, 1, 2).chunk(0).unwrap()).is_err());
    }
}

#[test]
fn gaps_duplicates_metadata_changes_and_nonadvancing_clear_release_storage_and_latch() {
    let data = snapshot(2048, 300, 1);
    for variant in 0..8 {
        let mut receiver = Receiver::new(owner()).unwrap();
        if variant != 0 {
            receiver.push(&data.chunk(0).unwrap()).unwrap();
        }
        let mut bad = data.chunk(1).unwrap();
        match variant {
            0 => {}
            1 => bad = data.chunk(0).unwrap(),
            2 => bad = data.chunk(2).unwrap(),
            3 => bad.header.transform[0] = 1.,
            4 => bad.header.metadata.origin[0] = 1.,
            5 => bad.header.source_counters.messages_received = 1,
            6 => bad.header.assembly_counters.patches = 1,
            _ => bad.header.revision = 2,
        }
        assert!(receiver.push(&bad).is_err(), "variant {variant}");
        assert_eq!(receiver.in_flight_cell_bytes(), 0);
        assert!(receiver.clear(&clear(3)).is_err());
    }
    let mut receiver = Receiver::new(owner()).unwrap();
    receiver.push(&data.chunk(0).unwrap()).unwrap();
    assert!(receiver.clear(&clear(1)).is_err());
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
}

#[test]
fn digest_failure_never_publishes_partial_or_corrupt_cells() {
    let data = snapshot(2048, 100, 1);
    for variant in 0..2 {
        let mut receiver = Receiver::new(owner()).unwrap();
        for index in 0..data.chunk_count() {
            let mut chunk = data.chunk(index).unwrap();
            if variant == 0 {
                chunk.header.sha256 = "0".repeat(64);
            } else if index + 1 == data.chunk_count() {
                let length = 204800 - MAX_CHUNK_BYTES;
                let mut raw =
                    binary::decode_exact(&chunk.data_base64, length, MAX_CHUNK_BYTES).unwrap();
                raw[0] ^= 1;
                chunk.data_base64 = binary::encode(&raw);
            }
            let result = receiver.push(&chunk);
            if index + 1 == data.chunk_count() {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), Received::Pending);
            }
        }
        assert_eq!(receiver.in_flight_cell_bytes(), 0);
    }
}

#[test]
fn hostile_bounds_digest_base64_and_owner_fields_fail_before_reserving_cells() {
    let data = snapshot(1, 1, 1);
    for variant in 0..12 {
        let mut chunk = data.chunk(0).unwrap();
        match variant {
            0 => chunk.header.metadata.width = u32::MAX,
            1 => chunk.index = usize::MAX,
            2 => chunk.count = usize::MAX,
            3 => chunk.data_base64 = "YR==".into(),
            4 => chunk.data_base64.push('A'),
            5 => chunk.header.sha256 = "A".repeat(64),
            6 => chunk.header.contract_version += 1,
            7 => chunk.header.revision = 0,
            8 => chunk.header.metadata.resolution = f64::NAN,
            9 => chunk.header.transform[6] = 2.,
            10 => chunk.header.metadata.frame_id.clear(),
            _ => chunk.header.owner.topic.clear(),
        }
        let mut receiver = Receiver::new(owner()).unwrap();
        assert!(receiver.push(&chunk).is_err(), "variant {variant}");
        assert_eq!(receiver.in_flight_cell_bytes(), 0);
    }
    let mut invalid = owner();
    invalid.generation = Generation(0);
    assert!(Receiver::new(invalid).is_err());
    assert!(Chunk::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1]).is_err());
}

#[test]
fn strict_json_rejects_unknown_and_duplicate_nested_fields_and_unknown_clear_reasons() {
    let chunk = snapshot(1, 1, 1).chunk(0).unwrap();
    for location in [
        "top",
        "header",
        "owner",
        "metadata",
        "source_counters",
        "assembly_counters",
    ] {
        let mut value = serde_json::to_value(&chunk).unwrap();
        match location {
            "top" => value["extra"] = true.into(),
            "header" => value["header"]["extra"] = true.into(),
            "owner" => value["header"]["owner"]["extra"] = true.into(),
            "metadata" => value["header"]["metadata"]["extra"] = true.into(),
            "source_counters" => value["header"]["source_counters"]["extra"] = true.into(),
            _ => value["header"]["assembly_counters"]["extra"] = true.into(),
        }
        assert!(
            Chunk::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{location}"
        );
    }
    let encoded = String::from_utf8(chunk.encode().unwrap()).unwrap();
    for (from, to) in [
        ("\"index\":0", "\"index\":0,\"index\":0"),
        ("\"width\":1", "\"width\":1,\"width\":1"),
        ("\"generation\":3", "\"generation\":3,\"generation\":3"),
    ] {
        assert!(encoded.contains(from));
        assert!(Chunk::decode(encoded.replacen(from, to, 1).as_bytes()).is_err());
    }
    let mut value = serde_json::to_value(clear(1)).unwrap();
    value["reason"] = "unknown".into();
    assert!(Clear::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    value = serde_json::to_value(clear(1)).unwrap();
    value["extra"] = true.into();
    assert!(Clear::decode(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn source_and_fixed_frame_corners_use_full_three_dimensional_pose_and_reject_overflow() {
    let mut data = source(2, 1);
    data.metadata.resolution = 0.5;
    let half = std::f64::consts::FRAC_1_SQRT_2;
    data.metadata.origin = [1., 2., 3., 0., 0., half, half];
    let transform = [10., 20., 30., half, 0., 0., half];
    let corners = mapping::map_corners(2, 1, 0.5, &data.metadata.origin).unwrap();
    let fixed = mapping::transform_point(&transform, corners[0]);
    for (actual, expected) in fixed.into_iter().zip([11., 17., 32.]) {
        assert!((actual - expected).abs() < 1e-10);
    }
    Snapshot::new(
        owner(),
        1,
        data,
        transform,
        mapping::Counters::default(),
        Counters::default(),
        SourceStatus::default(),
    )
    .unwrap();
    let mut header = snapshot(1, 1, 1).header().clone();
    header.transform[0] = mapping::MAX_COORDINATE;
    assert!(header.validate().is_err());
    header.transform[0] = f64::INFINITY;
    assert!(header.validate().is_err());
}

#[test]
fn binary_helpers_share_exact_layout_and_signed_raw_digest_rules() {
    let raw: Vec<u8> = (0..=255).cycle().take(8193).collect();
    let signed: Vec<i8> = raw.iter().map(|v| *v as i8).collect();
    assert_eq!(binary::sha256(&raw), binary::signed_sha256(&signed));
    assert!(binary::valid_sha256(&binary::sha256(&raw)));
    assert_eq!(binary::chunk_length(11, 2, 3, 20, 5).unwrap(), 1);
    for args in [
        (0, 0, 0, 20, 5),
        (21, 0, 5, 20, 5),
        (11, 3, 3, 20, 5),
        (11, 0, 3, 20, 0),
        (11, 0, 4, 20, 5),
    ] {
        assert!(binary::chunk_length(args.0, args.1, args.2, args.3, args.4).is_err());
    }
}

#[test]
fn stop_mid_transfer_releases_reserved_storage_and_ignores_all_late_work() {
    let data = snapshot(2048, 100, 1);
    let mut receiver = Receiver::new(owner()).unwrap();
    assert_eq!(
        receiver.push(&data.chunk(0).unwrap()).unwrap(),
        Received::Pending
    );
    assert!(receiver.in_flight_cell_bytes() > 0);
    receiver.stop();
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
    let mut late = data.chunk(1).unwrap();
    late.data_base64 = "bad".into();
    assert_eq!(receiver.push(&late).unwrap(), Received::Stale);
    assert_eq!(receiver.clear(&clear(2)).unwrap(), Received::Stale);
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
}

#[test]
fn tagged_message_requires_exact_data_or_clear_variant() {
    use studio_protocol::map_snapshot::Message;
    let chunk = snapshot(1, 1, 1).chunk(0).unwrap();
    let tagged = Message::Chunk(Box::new(chunk.clone()));
    let encoded = tagged.encode().unwrap();
    assert_eq!(Message::decode(&encoded).unwrap(), tagged);
    let text = String::from_utf8(encoded).unwrap();
    assert!(Message::decode(
        text.replacen(
            "\"kind\":\"chunk\"",
            "\"kind\":\"chunk\",\"kind\":\"clear\"",
            1
        )
        .as_bytes()
    )
    .is_err());
    assert!(Message::decode(&chunk.encode().unwrap()).is_err());
    let mut receiver = Receiver::new(owner()).unwrap();
    assert!(matches!(
        receiver.receive(&tagged).unwrap(),
        Received::Complete(_)
    ));
    let control = Message::Clear(Box::new(clear(2)));
    let mut mixed = serde_json::to_value(&control).unwrap();
    mixed["data_base64"] = "AA==".into();
    assert!(Message::decode(&serde_json::to_vec(&mixed).unwrap()).is_err());
    assert_eq!(
        Message::decode(&control.encode().unwrap()).unwrap(),
        control
    );
    assert!(matches!(
        receiver.receive(&control).unwrap(),
        Received::Cleared(_)
    ));
    for variant in 0..4 {
        let mut value = serde_json::to_value(&tagged).unwrap();
        match variant {
            0 => value["kind"] = "other".into(),
            1 => value["reason"] = "clear".into(),
            2 => value["owner"] = serde_json::to_value(owner()).unwrap(),
            _ => value["extra"] = true.into(),
        }
        assert!(
            Message::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "variant {variant}"
        );
    }
}

#[test]
fn missing_tf_clears_display_transfer_while_valid_source_map_can_recover_without_new_input() {
    use studio_protocol::map_grid::Assembly;
    let mut assembly = Assembly::default();
    let token = assembly.start().unwrap();
    assembly.full(token, 1, source(2, 2)).unwrap();
    let publish = |assembly: &Assembly, revision, transform| {
        let cached = assembly.snapshot().unwrap();
        Snapshot::new(
            owner(),
            revision,
            Grid {
                metadata: cached.metadata.clone(),
                cells: cached.cells.to_vec().into_boxed_slice(),
            },
            transform,
            mapping::Counters::default(),
            assembly.counters(),
            SourceStatus::default(),
        )
        .unwrap()
    };
    let mut receiver = Receiver::new(owner()).unwrap();
    let ready = publish(&assembly, 1, [0., 0., 0., 0., 0., 0., 1.]);
    assert!(matches!(
        receiver.push(&ready.chunk(0).unwrap()).unwrap(),
        Received::Complete(_)
    ));
    let mut unavailable = clear(2);
    unavailable.reason = ClearReason::MissingTransform;
    unavailable.metadata = Some(assembly.snapshot().unwrap().metadata.clone());
    assert_eq!(
        Clear::decode(&unavailable.encode().unwrap()).unwrap(),
        unavailable
    );
    let mut missing_context = unavailable.clone();
    missing_context.metadata = None;
    assert!(missing_context.validate().is_err());
    let mut false_valid_base = clear(2);
    false_valid_base.metadata = unavailable.metadata.clone();
    assert!(false_valid_base.validate().is_err());
    assert_eq!(
        receiver.clear(&unavailable).unwrap(),
        Received::Cleared(Box::new(unavailable.clone()))
    );
    assert_eq!(receiver.in_flight_cell_bytes(), 0);
    assert_eq!(assembly.cell_bytes(), 4);
    assert_eq!(assembly.revision(), 1);
    let recovered = publish(&assembly, 3, [10., 0., 0., 0., 0., 0., 1.]);
    let Received::Complete(complete) = receiver.push(&recovered.chunk(0).unwrap()).unwrap() else {
        panic!("TF recovery must deliver the cached source grid")
    };
    assert_eq!(&*complete.grid.cells, &[0, 1, 2, 3]);
    assert_eq!(complete.grid.metadata.stamp_ns, 10);
    assert_eq!(complete.header.revision, 3);
    assert_eq!(complete.header.transform[0], 10.);
}
