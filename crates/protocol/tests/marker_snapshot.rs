use studio_protocol::{
    marker::{self, store::Store, Batch, Geometry, Key, Marker, Operation},
    marker_snapshot::*,
    spatial,
    subscriptions::{Generation, SubscriptionId},
};
fn owner() -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        topic: "/markers".into(),
        message_type: marker::ARRAY_MESSAGE_TYPE.into(),
        fixed_frame: "world".into(),
    }
}
fn clock(mono: u64) -> Clock {
    Clock {
        unix_ns: 1_000_000_000 + mono,
        monotonic_ns: mono,
    }
}
fn key(id: i32) -> Key {
    Key {
        namespace: "objects".into(),
        id,
    }
}
fn marker(id: i32) -> Marker {
    Marker {
        key: key(id),
        frame_id: "world".into(),
        stamp_ns: 10,
        pose: [0., 0., 0., 0., 0., 0., 1.],
        scale: [1.; 3],
        color: [1.; 4],
        lifetime_ns: 0,
        frame_locked: false,
        geometry: Geometry::Cube,
    }
}
fn add(store: &mut Store, sequence: u64, m: Marker, now: u64) {
    store
        .apply(
            store.token().unwrap(),
            sequence,
            Batch {
                contract_version: marker::CONTRACT_VERSION,
                operations: vec![Operation::Add {
                    marker: Box::new(m),
                }],
            },
            now,
        )
        .unwrap();
}
fn store(count: usize, large: bool) -> Store {
    let mut s = Store::default();
    s.start(1, 2, "world").unwrap();
    for i in 0..count {
        let mut m = marker(i as i32);
        if large {
            m.geometry = Geometry::Points {
                points: vec![[0.123456789, 0.987654321, 0.111111111]; marker::MAX_POINTS],
                colors: vec![[0.2, 0.4, 0.6, 0.8]; marker::MAX_POINTS],
            };
        }
        add(&mut s, i as u64 + 1, m, 0);
    }
    s
}
fn snapshot(s: &Store, revision: u64, captured: u64) -> Snapshot {
    Snapshot::from_store(
        owner(),
        revision,
        s,
        clock(captured),
        LossSupport::Unsupported,
        spatial::Counters::default(),
    )
    .unwrap()
}
fn complete(value: Received) -> (u64, std::sync::Arc<Complete>) {
    let Received::Complete { revision, state } = value else {
        panic!("Expected complete: {value:?}")
    };
    (revision, state)
}
fn deliver(r: &mut Receiver, s: &Snapshot, start: u64) -> std::sync::Arc<Complete> {
    let mut last = Received::Pending;
    for index in 0..s.chunk_count() {
        let chunk = Message::Chunk(Box::new(s.chunk(index).unwrap()));
        let bytes = chunk.encode().unwrap();
        assert!(bytes.len() < MAX_SAMPLE_BYTES);
        last = r
            .receive(
                &Message::decode(&bytes).unwrap(),
                clock(start + index as u64),
            )
            .unwrap();
    }
    complete(last).1
}
fn prune(revision: u64, ids: impl IntoIterator<Item = i32>) -> Message {
    Message::Prune(Box::new(Prune {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision,
        keys: ids.into_iter().map(key).collect(),
    }))
}

#[test]
fn complete_state_is_atomic_and_maximum_source_geometry_roundtrips_in_bounded_chunks() {
    let mut s = store(16, true); //65536 points plus aligned RGBA, above one IPC sample.
    for id in 16..256 {
        add(&mut s, id as u64 + 1, marker(id), 0);
    }
    let snap = snapshot(&s, 1, 0);
    assert!(snap.chunk_count() > 1);
    assert!(snap.document_bytes() <= MAX_DOCUMENT_BYTES);
    let mut r = Receiver::new(owner()).unwrap();
    for index in 0..snap.chunk_count() - 1 {
        assert!(matches!(
            r.receive(
                &Message::Chunk(Box::new(snap.chunk(index).unwrap())),
                clock(index as u64)
            )
            .unwrap(),
            Received::Pending
        ));
        assert!(r.in_flight_bytes() <= MAX_DOCUMENT_BYTES);
    }
    let (_, state) = complete(
        r.receive(
            &Message::Chunk(Box::new(snap.chunk(snap.chunk_count() - 1).unwrap())),
            clock(snap.chunk_count() as u64),
        )
        .unwrap(),
    );
    assert_eq!(state.entries().len(), 256);
    assert_eq!(state.visible_entries(100).count(), 256);
    assert_eq!(
        state
            .entries()
            .iter()
            .map(|e| e.marker.geometry.points().len())
            .sum::<usize>(),
        65536
    );
    assert_eq!(
        state.entries()[15].marker.geometry.colors()[4095],
        [0.2, 0.4, 0.6, 0.8]
    );
    assert_eq!(r.in_flight_bytes(), 0);
}

#[test]
fn pruning_during_partial_transfer_filters_deleted_keys_without_starving_static_peers() {
    let s = store(4, true);
    let snap = snapshot(&s, 1, 0);
    assert!(snap.chunk_count() > 1);
    let mut r = Receiver::new(owner()).unwrap();
    r.receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(0))
        .unwrap();
    r.receive(&prune(2, [0]), clock(1)).unwrap();
    for revision in 3..1003 {
        r.receive(
            &prune(revision, [revision as i32 + 1_000_000]),
            clock(revision),
        )
        .unwrap();
    }
    assert_eq!(r.retained_prune_count(), 1);
    assert!(r.retained_deadline_count() <= marker::MAX_MARKERS * 2);
    let mut last = Received::Pending;
    for index in 1..snap.chunk_count() {
        last = r
            .receive(
                &Message::Chunk(Box::new(snap.chunk(index).unwrap())),
                clock(2000 + index as u64),
            )
            .unwrap();
    }
    let (revision, state) = complete(last);
    assert_eq!(revision, 1002);
    assert_eq!(
        state
            .visible_entries(3000)
            .map(|e| e.marker.key.id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn pruning_a_visible_state_preserves_peers_and_a_newer_valid_replacement_can_return() {
    let mut s = store(2, false);
    let snap = snapshot(&s, 1, 0);
    let mut r = Receiver::new(owner()).unwrap();
    let first = deliver(&mut r, &snap, 0);
    let weak = std::sync::Arc::downgrade(&first.entries()[0].marker);
    drop(first);
    let (_, pruned) = complete(r.receive(&prune(2, [0]), clock(1)).unwrap());
    assert_eq!(
        pruned
            .entries()
            .iter()
            .map(|e| e.marker.key.id)
            .collect::<Vec<_>>(),
        [1]
    );
    assert!(weak.upgrade().is_none());
    let mut replacement = marker(0);
    replacement.geometry = Geometry::Sphere;
    add(&mut s, 3, replacement, 2);
    let current = snapshot(&s, 3, 2);
    let latest = deliver(&mut r, &current, 3);
    assert_eq!(latest.entries().len(), 2);
    assert!(matches!(
        latest.entries()[0].marker.geometry,
        Geometry::Sphere
    ));
    assert_eq!(r.retained_prune_count(), 0);
    assert!(matches!(
        r.receive(&prune(2, [0]), clock(4)).unwrap(),
        Received::Stale
    ));
}

#[test]
fn clear_cancels_partial_and_completed_state_and_latches_revision_barrier() {
    let s = store(4, true);
    let snap = snapshot(&s, 1, 0);
    let mut r = Receiver::new(owner()).unwrap();
    r.receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(0))
        .unwrap();
    let clear = Message::Clear(Box::new(Clear {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision: 2,
        reason: ClearReason::SourceFault,
        known_history_gap: true,
    }));
    assert!(matches!(
        r.receive(&clear, clock(1)).unwrap(),
        Received::Cleared(_)
    ));
    assert_eq!(r.in_flight_bytes(), 0);
    assert_eq!(r.retained_deadline_count(), 0);
    for i in 1..snap.chunk_count() {
        assert!(matches!(
            r.receive(&Message::Chunk(Box::new(snap.chunk(i).unwrap())), clock(2))
                .unwrap(),
            Received::Stale
        ));
    }
    let empty = store(0, false);
    let next = snapshot(&empty, 3, 3);
    assert!(deliver(&mut r, &next, 4).entries().is_empty());
}

#[test]
fn lifetime_subtracts_first_chunk_delay_and_transfer_time_and_is_never_extended() {
    let mut s = store(0, false);
    let mut m = marker(0);
    m.lifetime_ns = 1000;
    add(&mut s, 1, m, 0);
    let snap = snapshot(&s, 1, 10);
    let mut r = Receiver::new(owner()).unwrap();
    let first_rx = Clock {
        unix_ns: 1_000_000_060,
        monotonic_ns: 10_000,
    };
    let (_, first) = complete(
        r.receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), first_rx)
            .unwrap(),
    );
    assert_eq!(first.entries()[0].expires_at_monotonic_ns, Some(10_940));
    assert_eq!(first.visible_entries(10_939).count(), 1);
    assert_eq!(first.visible_entries(10_940).count(), 0);
    // Later capture shifts the wall-clock anchor forward. An unchanged source
    // version must retain the earlier locally established monotonic deadline.
    let later = Snapshot::from_store(
        owner(),
        2,
        &s,
        Clock {
            unix_ns: 1_000_000_200,
            monotonic_ns: 20,
        },
        LossSupport::Unknown,
        spatial::Counters::default(),
    )
    .unwrap();
    let (_, second) = complete(
        r.receive(
            &Message::Chunk(Box::new(later.chunk(0).unwrap())),
            Clock {
                unix_ns: 1_000_000_210,
                monotonic_ns: 10_150,
            },
        )
        .unwrap(),
    );
    assert_eq!(second.entries()[0].expires_at_monotonic_ns, Some(10_940));
    assert_eq!(second.visible_entries(10_940).count(), 0);
    let mut big = store(4, true);
    let token = big.token().unwrap();
    let mut life = (*big.entries()[&key(0)].marker).clone();
    life.lifetime_ns = 10;
    add(&mut big, 5, life, 0);
    let transfer = snapshot(&big, 1, 0);
    let mut receiver = Receiver::new(owner()).unwrap();
    let visible = deliver(&mut receiver, &transfer, 20);
    assert_eq!(visible.visible_entries(100).count(), 3);
    big.tick(token, 10).unwrap();
}

#[test]
fn malformed_current_owner_chunks_fault_until_reactivation_but_stale_owner_and_stop_are_ignored() {
    let s = store(4, true);
    let snap = snapshot(&s, 1, 0);
    for defect in 0..5 {
        let mut r = Receiver::new(owner()).unwrap();
        let mut chunk = snap.chunk(0).unwrap();
        match defect {
            0 => chunk.header.document_bytes = MAX_DOCUMENT_BYTES + 1,
            1 => chunk.header.sha256 = "bad".into(),
            2 => chunk.index = 1,
            3 => chunk.data_base64.push('='),
            _ => chunk.header.owner.fixed_frame = "contradiction".into(),
        }
        assert!(r
            .receive(&Message::Chunk(Box::new(chunk)), clock(0))
            .is_err());
        assert_eq!(r.in_flight_bytes(), 0);
        assert!(r
            .receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(1))
            .is_err());
    }
    let mut r = Receiver::new(owner()).unwrap();
    let mut old = snap.chunk(0).unwrap();
    old.header.owner.generation = Generation(999);
    old.data_base64.clear();
    assert!(matches!(
        r.receive(&Message::Chunk(Box::new(old)), clock(0)).unwrap(),
        Received::Stale
    ));
    r.stop();
    assert!(matches!(
        r.receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(1))
            .unwrap(),
        Received::Stale
    ));
}

#[test]
fn digest_metadata_chunk_order_and_reused_control_revision_cannot_commit() {
    let s = store(4, true);
    let snap = snapshot(&s, 1, 0);
    for defect in 0..4 {
        let mut r = Receiver::new(owner()).unwrap();
        r.receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(0))
            .unwrap();
        let mut next = snap.chunk(if defect == 0 { 0 } else { 1 }).unwrap();
        if defect == 1 {
            next.header.captured_at_unix_ns += 1;
        }
        if defect == 2 {
            next.header.manifest[0].version += 1;
        }
        if defect == 3 {
            // Corrupt valid base64 without changing length or metadata.
            next.data_base64.replace_range(0..1, "A");
            r.receive(&Message::Chunk(Box::new(next)), clock(1))
                .unwrap();
            let mut final_result = Ok(Received::Pending);
            for index in 2..snap.chunk_count() {
                final_result = r.receive(
                    &Message::Chunk(Box::new(snap.chunk(index).unwrap())),
                    clock(index as u64),
                );
            }
            assert!(final_result.is_err());
            continue;
        }
        assert!(r
            .receive(&Message::Chunk(Box::new(next)), clock(2))
            .is_err());
        assert_eq!(r.in_flight_bytes(), 0);
    }
    for is_clear in [false, true] {
        let mut r = Receiver::new(owner()).unwrap();
        deliver(&mut r, &snapshot(&store(1, false), 1, 0), 0);
        let message = if is_clear {
            Message::Clear(Box::new(Clear {
                contract_version: CONTRACT_VERSION,
                owner: owner(),
                revision: 1,
                reason: ClearReason::LocalClear,
                known_history_gap: false,
            }))
        } else {
            prune(1, [0])
        };
        assert!(r.receive(&message, clock(1)).is_err());
    }
}

#[test]
fn future_capture_and_regressing_receiver_or_source_clocks_require_explicit_recovery() {
    let s = store(1, false);
    let snap = snapshot(&s, 1, 10);
    assert!(Receiver::new(owner())
        .unwrap()
        .receive(&Message::Chunk(Box::new(snap.chunk(0).unwrap())), clock(9))
        .is_err());
    for monotonic in [false, true] {
        let mut r = Receiver::new(owner()).unwrap();
        deliver(&mut r, &snap, 10);
        let now = if monotonic {
            Clock {
                unix_ns: 1_000_000_020,
                monotonic_ns: 9,
            }
        } else {
            Clock {
                unix_ns: 1_000_000_009,
                monotonic_ns: 20,
            }
        };
        assert!(r.receive(&prune(2, [0]), now).is_err());
    }
    let mut newer = store(0, false);
    add(&mut newer, 1, marker(1), 20);
    assert!(Snapshot::from_store(
        owner(),
        1,
        &newer,
        clock(19),
        LossSupport::Unknown,
        spatial::Counters::default()
    )
    .is_err());
}

#[test]
fn bounded_sequence_decode_and_full_state_manifest_binding_reject_oversized_or_substituted_entries()
{
    let s = store(1, false);
    let snap = snapshot(&s, 1, 0);
    let chunk = snap.chunk(0).unwrap();
    let bytes = studio_protocol::binary::decode_exact(
        &chunk.data_base64,
        chunk.header.document_bytes,
        MAX_CHUNK_BYTES,
    )
    .unwrap();
    let mut state: State = serde_json::from_slice(&bytes).unwrap();
    state.entries[0].marker.key.id = 100;
    assert!(Snapshot::new(snap.header().clone(), state.clone()).is_err());
    state.entries = vec![state.entries[0].clone(); marker::MAX_MARKERS + 1];
    assert!(serde_json::from_slice::<State>(&serde_json::to_vec(&state).unwrap()).is_err());
    let mut envelope = serde_json::to_value(Message::Chunk(Box::new(chunk))).unwrap();
    let entry = envelope["header"]["manifest"][0].clone();
    envelope["header"]["manifest"] = serde_json::json!(vec![entry; marker::MAX_MARKERS + 1]);
    assert!(Message::decode(&serde_json::to_vec(&envelope).unwrap()).is_err());
}

#[test]
fn worst_case_namespace_manifest_still_fits_the_existing_outer_sample_budget() {
    let mut s = Store::default();
    s.start(1, 2, "world").unwrap();
    for id in 0..256 {
        let mut m = marker(id);
        m.key.namespace = "\\\"".repeat(128);
        if id == 0 {
            m.geometry = Geometry::Points {
                points: vec![[0.123456789; 3]; marker::MAX_POINTS],
                colors: vec![[0.123456; 4]; marker::MAX_POINTS],
            };
        }
        add(&mut s, id as u64 + 1, m, 0);
    }
    let snap = snapshot(&s, 1, 0);
    assert!(snap.chunk_count() > 1);
    for index in 0..snap.chunk_count() {
        let bytes = Message::Chunk(Box::new(snap.chunk(index).unwrap()))
            .encode()
            .unwrap();
        assert!(bytes.len() < MAX_SAMPLE_BYTES);
        assert!(Message::decode(&bytes).is_ok());
    }
}

#[test]
fn strict_marker_capabilities_reject_old_sessions_and_changed_or_missing_limits() {
    let caps = Capabilities::default();
    let mut json = serde_json::to_value(&caps).unwrap();
    assert_eq!(json["supported_kinds"].as_array().unwrap().len(), 12);
    json.as_object_mut().unwrap().remove("max_document_bytes");
    assert!(serde_json::from_value::<Capabilities>(json).is_err());
    let mut ready = serde_json::to_value(studio_protocol::session::Event::ready(
        "jazzy".into(),
        "rmw_fastrtps_cpp".into(),
    ))
    .unwrap();
    assert_eq!(ready["version"], studio_protocol::session::VERSION);
    assert_eq!(
        ready["marker_snapshot"],
        serde_json::to_value(caps).unwrap()
    );
    let mut old = ready.clone();
    old["version"] = 13.into();
    assert!(
        serde_json::from_value::<studio_protocol::session::Event>(old.clone())
            .unwrap()
            .validate()
            .is_err()
    );
    old.as_object_mut().unwrap().remove("marker_snapshot");
    assert!(serde_json::from_value::<studio_protocol::session::Event>(old).is_err());
    ready["marker_snapshot"]["max_owners"] = 3.into();
    assert!(
        serde_json::from_value::<studio_protocol::session::Event>(ready)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn exact_maximum_document_length_and_chunk_count_reassemble_without_exceeding_reserved_bytes() {
    let snap = snapshot(&store(1, false), 1, 0);
    let chunk = snap.chunk(0).unwrap();
    let mut bytes = studio_protocol::binary::decode_exact(
        &chunk.data_base64,
        chunk.header.document_bytes,
        MAX_CHUNK_BYTES,
    )
    .unwrap();
    bytes.resize(MAX_DOCUMENT_BYTES, b' '); // Valid trailing JSON whitespace.
    let mut header = chunk.header;
    header.document_bytes = bytes.len();
    header.sha256 = studio_protocol::binary::sha256(&bytes);
    let mut receiver = Receiver::new(owner()).unwrap();
    let mut result = Received::Pending;
    for (index, raw) in bytes.chunks(MAX_CHUNK_BYTES).enumerate() {
        let message = Message::Chunk(Box::new(Chunk {
            header: header.clone(),
            index,
            count: MAX_CHUNKS,
            data_base64: studio_protocol::binary::encode(raw),
        }));
        assert!(message.encode().unwrap().len() < MAX_SAMPLE_BYTES);
        result = receiver.receive(&message, clock(index as u64)).unwrap();
        assert!(receiver.in_flight_bytes() <= MAX_DOCUMENT_BYTES);
    }
    assert_eq!(complete(result).1.entries().len(), 1);
    assert_eq!(receiver.in_flight_bytes(), 0);
}

#[test]
fn two_manifests_bound_deadline_and_prune_metadata_and_supersession_releases_old_assembly() {
    let mut s = store(256, false);
    let first = snapshot(&s, 1, 0);
    let mut r = Receiver::new(owner()).unwrap();
    deliver(&mut r, &first, 0);
    s.apply(
        s.token().unwrap(),
        257,
        Batch {
            contract_version: marker::CONTRACT_VERSION,
            operations: vec![Operation::DeleteAll {
                namespace: String::new(),
            }],
        },
        1,
    )
    .unwrap();
    for id in 0..256 {
        let mut m = marker(id);
        m.key.namespace = "new".into();
        if id == 0 {
            m.geometry = Geometry::Points {
                points: vec![[0.123456789; 3]; marker::MAX_POINTS],
                colors: vec![[0.1; 4]; marker::MAX_POINTS],
            };
        }
        add(&mut s, 258 + id as u64, m, 1);
    }
    let partial = snapshot(&s, 2, 1);
    assert!(partial.chunk_count() > 1);
    r.receive(
        &Message::Chunk(Box::new(partial.chunk(0).unwrap())),
        clock(2),
    )
    .unwrap();
    r.receive(&prune(3, 0..256), clock(3)).unwrap();
    let remove_new = Message::Prune(Box::new(Prune {
        contract_version: CONTRACT_VERSION,
        owner: owner(),
        revision: 4,
        keys: (0..256)
            .map(|id| Key {
                namespace: "new".into(),
                id,
            })
            .collect(),
    }));
    r.receive(&remove_new, clock(4)).unwrap();
    assert_eq!(r.retained_deadline_count(), 512);
    assert_eq!(r.retained_prune_count(), 512);
    s.source_fault(s.token().unwrap()).unwrap();
    let empty = snapshot(&s, 5, 5);
    assert!(deliver(&mut r, &empty, 6).entries().is_empty());
    assert_eq!(r.retained_deadline_count(), 0);
    assert_eq!(r.retained_prune_count(), 0);
    assert_eq!(r.in_flight_bytes(), 0);
    assert!(matches!(
        r.receive(
            &Message::Chunk(Box::new(partial.chunk(1).unwrap())),
            clock(7)
        )
        .unwrap(),
        Received::Stale
    ));
}
