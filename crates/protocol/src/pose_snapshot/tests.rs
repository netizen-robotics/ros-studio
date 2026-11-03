use super::*;
use serde_json::json;
fn covariance() -> Covariance {
    Covariance::new(std::array::from_fn(
        |i| if i / 6 == i % 6 { 1. } else { 0. },
    ))
    .unwrap()
}
fn entry(sequence: u64, stamp: i64) -> Entry {
    Entry {
        sequence,
        acquisition_stamp_ns: stamp,
        components: [1., 2., 3., 0., 0., 0., 1.],
        covariance: covariance(),
        transform: LatestPose::Ready {
            resolved_stamp_ns: stamp,
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
    }
}
fn source() -> Source {
    Source {
        frame_id: "odom".into(),
        child_frame_id: Some("base_link".into()),
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        contract_version: CONTRACT_VERSION,
        owner: Owner {
            generation: Generation(1),
            subscription: SubscriptionId(2),
            kind: Kind::Odometry,
            topic: "/odom".into(),
            fixed_frame: "world".into(),
        },
        revision: 1,
        continuity_epoch: 1,
        source: Some(source()),
        history: Some(HistorySettings::default()),
        latest: Some(entry(3, 30)),
        trail: vec![entry(1, 10), entry(2, 20)],
        last_reset: None,
        known_history_gap: false,
        counters: Counters::default(),
    }
}
#[test]
fn covariance_has_exact_bounded_flat_36_value_serde_and_validation() {
    let matrix = covariance();
    let encoded = serde_json::to_vec(&matrix).unwrap();
    assert_eq!(
        serde_json::from_slice::<Covariance>(&encoded).unwrap(),
        matrix
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&encoded)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        36
    );
    for n in [0, 35, 37, 4096] {
        assert!(serde_json::from_value::<Covariance>(json!(vec![0.; n])).is_err());
    }
    let mut values = vec![0.; 36];
    values[0] = -1.;
    assert!(serde_json::from_value::<Covariance>(json!(values)).is_err());
    assert!(serde_json::from_value::<Covariance>(json!({"values":[0]})).is_err());
}
#[test]
fn decoded_roundtrip_distinguishes_exact_types_header_child_and_zero_time() {
    let mut value = Decoded {
        kind: Kind::Odometry,
        topic: "/odom".into(),
        source: source(),
        acquisition_stamp_ns: 0,
        components: [0., 0., 0., 0., 0., 0., 1.],
        covariance: covariance(),
    };
    assert_eq!(Decoded::decode(&value.encode().unwrap()).unwrap(), value);
    assert_eq!(
        Kind::from_message_type(ODOMETRY_MESSAGE_TYPE).unwrap(),
        value.kind
    );
    assert!(Kind::from_message_type(mapping::POSE_MESSAGE_TYPE).is_err());
    value.kind = Kind::PoseWithCovariance;
    assert!(value.validate().is_err());
    value.source.child_frame_id = None;
    value.acquisition_stamp_ns = crate::latest_pose::MAX_STAMP_NS;
    assert!(value.validate().is_ok());
    value.acquisition_stamp_ns = -1;
    assert!(value.validate().is_err());
    value.acquisition_stamp_ns = 0;
    value.topic = "/invalid topic".into();
    assert!(value.validate().is_err());
    assert!(Decoded::decode(&vec![b' '; MAX_DECODED_BYTES + 1])
        .unwrap_err()
        .contains("byte budget"));
}
#[test]
fn full_snapshot_keeps_fresh_head_separate_from_older_ready_anchors() {
    let mut value = snapshot();
    assert_eq!(Snapshot::decode(&value.encode().unwrap()).unwrap(), value);
    value.latest.as_mut().unwrap().transform = LatestPose::Missing;
    assert_eq!(Snapshot::decode(&value.encode().unwrap()).unwrap(), value);
    value.trail[0].transform = LatestPose::Missing;
    assert!(value.validate().is_err());
}
#[test]
fn ordered_ids_time_policy_and_duplicate_head_payload_are_checked() {
    let mut value = snapshot();
    value.trail[1].sequence = 1;
    assert!(value.validate().is_err());
    value = snapshot();
    value.trail[1].acquisition_stamp_ns = 10;
    assert!(value.validate().is_err());
    value = snapshot();
    value.trail.push(value.latest.clone().unwrap());
    assert!(value.validate().is_ok());
    value.trail.last_mut().unwrap().components[0] += 1.;
    assert!(value.validate().is_err());
    value = snapshot();
    value.trail[0] = entry(1, 0);
    assert!(value.validate().is_err());
    value = snapshot();
    value.trail[1] = entry(2, 30);
    assert!(value.validate().is_err());
    value = snapshot();
    value.latest.as_mut().unwrap().sequence = 0;
    assert!(value.validate().is_err());
}
#[test]
fn captured_tf_requires_exact_requested_stamp_and_same_frame_identity() {
    let mut value = snapshot();
    value.latest.as_mut().unwrap().transform = LatestPose::Ready {
        resolved_stamp_ns: 29,
        components: [0., 0., 0., 0., 0., 0., 1.],
    };
    assert!(value.validate().is_err());
    value = snapshot();
    value.owner.fixed_frame = "odom".into();
    assert!(value.validate().is_ok());
    value.latest.as_mut().unwrap().transform = LatestPose::Ready {
        resolved_stamp_ns: 30,
        components: [1., 0., 0., 0., 0., 0., 1.],
    };
    assert!(value.validate().is_err());
    value.latest.as_mut().unwrap().transform = LatestPose::Ready {
        resolved_stamp_ns: 30,
        components: [0., 0., 0., 0., 0., 0., -1.],
    };
    assert!(value.validate().is_ok());
    value.latest = Some(entry(4, 0));
    value.trail.clear();
    value.latest.as_mut().unwrap().transform = LatestPose::Ready {
        resolved_stamp_ns: 123,
        components: [0., 0., 0., 0., 0., 0., 1.],
    };
    assert!(value.validate().is_ok());
}
#[test]
fn empty_clear_and_kind_history_owner_loss_contradictions_are_explicit() {
    let mut value = snapshot();
    value.latest = None;
    assert!(value.validate().is_err());
    value.trail.clear();
    value.last_reset = Some(ResetReason::LocalClear);
    value.continuity_epoch += 1;
    assert!(value.validate().is_ok());
    value.owner.kind = Kind::PoseWithCovariance;
    assert!(value.validate().is_err());
    value.source.as_mut().unwrap().child_frame_id = None;
    value.history = None;
    assert!(value.validate().is_ok());
    value.owner.subscription = SubscriptionId(0);
    assert!(value.validate().is_err());
    value = snapshot();
    value.counters.known_source_gaps = 1;
    assert!(value.validate().is_err());
    value.known_history_gap = true;
    assert!(value.validate().is_ok());
    value = snapshot();
    value.history.as_mut().unwrap().keep = 1;
    assert!(value.validate().is_err());
    value = snapshot();
    value.history.as_mut().unwrap().angle_tolerance_rad = std::f64::consts::PI + 1.;
    assert!(value.validate().is_err());
}
#[test]
fn maximum_full_history_stays_below_the_existing_sample_budget() {
    let mut value = snapshot();
    value.history.as_mut().unwrap().keep = MAX_HISTORY;
    value.owner.topic = format!("/{}", "a".repeat(254));
    value.owner.fixed_frame = "\\".repeat(256);
    value.source = Some(Source {
        frame_id: "\"".repeat(256),
        child_frame_id: Some("child".repeat(51)),
    });
    let v = [
        1.234567890123456e-100,
        2.123456789012345e-100,
        3.234567890123456e-100,
        4.345678901234567e-100,
        5.456789012345678e-100,
        6.567890123456789e-100,
    ];
    let dense = Covariance::new(std::array::from_fn(|i| v[i / 6] * v[i % 6])).unwrap();
    value.trail = (0..MAX_HISTORY)
        .map(|i| {
            let mut e = entry(
                u64::MAX - MAX_HISTORY as u64 + i as u64,
                crate::latest_pose::MAX_STAMP_NS - MAX_HISTORY as i64 + i as i64,
            );
            e.covariance = dense.clone();
            e.components[..3].fill(1.234567890123456e-100);
            e
        })
        .collect();
    value.latest = value.trail.last().cloned();
    let bytes = value.encode().unwrap();
    eprintln!(
        "Maximum pose snapshot: {} bytes / {}",
        bytes.len(),
        MAX_SAMPLE_BYTES
    );
    assert!(bytes.len() > 256 * 1024);
    assert!(bytes.len() <= MAX_SAMPLE_BYTES);
    let decoded = Snapshot::decode(&bytes).unwrap();
    for (i, (before, after)) in value
        .latest
        .as_ref()
        .unwrap()
        .covariance
        .row_major()
        .iter()
        .zip(decoded.latest.as_ref().unwrap().covariance.row_major())
        .enumerate()
    {
        assert_eq!(
            before.to_bits(),
            after.to_bits(),
            "covariance element {i}: {before:e} -> {after:e}"
        );
    }
    assert_eq!(decoded, value);
    let mut malicious = serde_json::to_value(&value).unwrap();
    malicious["trail"]
        .as_array_mut()
        .unwrap()
        .push(json!("must not decode as Entry"));
    assert!(Snapshot::decode(&serde_json::to_vec(&malicious).unwrap())
        .unwrap_err()
        .contains("Sequence exceeds"));
}
#[test]
fn strict_fields_versions_and_frame_shapes_reject_before_use() {
    let value = snapshot();
    let mut json = serde_json::to_value(value).unwrap();
    json["extra"] = json!(true);
    assert!(Snapshot::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json.as_object_mut().unwrap().remove("extra");
    json["contract_version"] = json!(CONTRACT_VERSION + 1);
    assert!(Snapshot::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json["contract_version"] = json!(CONTRACT_VERSION);
    json["source"]["frame_id"] = json!("/invalid");
    assert!(Snapshot::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(Snapshot::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1])
        .unwrap_err()
        .contains("byte budget"));
}
