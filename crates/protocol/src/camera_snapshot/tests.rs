use super::store::Store;
use super::transport::{Invalidation, Message, Receiver};
use super::*;
use crate::image::HeaderStamp;
use std::sync::Arc;
fn owner() -> Owner {
    Owner {
        generation: Generation(1),
        subscription: SubscriptionId(2),
        topic: "/camera/image_rect".into(),
        info: InfoSource {
            topic: "/camera/camera_info".into(),
            best_effort: true,
            transient_local: false,
        },
        fixed_frame: "world".into(),
    }
}
fn calibration(size: u32) -> Calibration {
    Calibration {
        stamp_ns: 1,
        frame_id: "camera_optical".into(),
        width: size,
        height: size,
        distortion_model: "plumb_bob".into(),
        d: vec![0.1, 0., 0., 0., 0.],
        k: [100., 0., 1., 0., 120., 1., 0., 0., 1.],
        r: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        p: [100., 0., 1., -10., 0., 120., 1., 2., 0., 0., 1., 0.],
        binning_x: 0,
        binning_y: 0,
        roi: crate::camera_info::RegionOfInterest::default(),
    }
}
fn image(size: u32) -> ImageSample {
    ImageSample::from_raw(
        &owner().topic,
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: 1,
                nanoseconds: 0,
            },
            frame_id: "camera_optical".into(),
            width: size,
            height: size,
            encoding: "rgba8".into(),
            is_bigendian: false,
            step: size * 4,
        },
        &vec![255; (size * size * 4) as usize],
    )
    .unwrap()
}
fn ready(stamp: i64) -> LatestPose {
    LatestPose::Ready {
        resolved_stamp_ns: stamp,
        components: [0., 0., 0., 0., 0., 0., 1.],
    }
}
fn initialized(size: u32) -> Store {
    let mut store = Store::new(owner()).unwrap();
    store.info(calibration(size)).unwrap();
    store.image(image(size), 0).unwrap();
    let request = store.request(0).unwrap().unwrap();
    assert!(store.complete(&request, ready(1_000_000_000), 0).unwrap());
    store
}

#[test]
fn pending_tf_keeps_complete_pair_but_replacement_input_cannot_extend_deadline() {
    let mut store = initialized(2);
    let epoch = store.epoch();
    store.image(image(2), 10).unwrap();
    let request = store.request(10).unwrap().unwrap();
    assert!(!store
        .complete(&request, LatestPose::Extrapolated, 10)
        .unwrap());
    assert_eq!(store.epoch(), epoch);
    assert_eq!(store.snapshot().unwrap().frame.unwrap().image_sequence, 1);
    store.image(image(2), RETRY_NS).unwrap();
    let request = store.request(RETRY_NS).unwrap().unwrap();
    assert!(store
        .complete(&request, ready(1_000_000_000), RETRY_NS)
        .unwrap());
    assert_eq!(store.snapshot().unwrap().frame.unwrap().image_sequence, 3);
    for now in [RETRY_NS * 2, TIMEOUT_NS / 2, TIMEOUT_NS + RETRY_NS] {
        store.image(image(2), now).unwrap();
        let request = store.request(now).unwrap().unwrap();
        store
            .complete(&request, LatestPose::Extrapolated, now)
            .unwrap();
    }
    assert!(store.snapshot().unwrap().frame.is_none());
    assert_eq!(store.snapshot().unwrap().status, Status::TfTimeout);
    // A fresh, directly resolvable image after an idle gap must still succeed.
    let now = TIMEOUT_NS * 3;
    store.request(now).unwrap();
    store.image(image(2), now).unwrap();
    let request = store.request(now).unwrap().unwrap();
    assert!(store.complete(&request, ready(1_000_000_000), now).unwrap());
    store.invalid_info("bad calibration".into()).unwrap();
    assert!(store.snapshot().unwrap().frame.is_none());
}
#[test]
fn calibration_change_invalidates_captured_frame_and_requires_a_subsequent_image() {
    let mut store = initialized(2);
    let old = store.snapshot().unwrap();
    let frame = old.frame.as_ref().unwrap();
    assert_eq!(frame.image_sequence, 1);
    assert_eq!(frame.calibration_revision, 1);
    let mut cal = calibration(2);
    cal.p[2] = 0.5;
    store.info(cal.clone()).unwrap();
    let changed = store.snapshot().unwrap();
    assert!(changed.continuity_epoch > old.continuity_epoch);
    assert!(changed.frame.is_none());
    assert_eq!(changed.status, Status::WaitingImage);
    assert!(store.request(1).unwrap().is_none());
    store.image(image(2), 2).unwrap();
    let request = store.request(2).unwrap().unwrap();
    store.complete(&request, ready(1_000_000_000), 2).unwrap();
    let new = store.snapshot().unwrap();
    let frame = new.frame.as_ref().unwrap();
    assert_eq!(frame.calibration, cal);
    assert_eq!(frame.calibration_revision, 2);
    assert_eq!(frame.image_sequence, 2);
    // Equal-parameter CameraInfo repeats are not calibration changes.
    let epoch = new.continuity_epoch;
    cal.stamp_ns = 999;
    store.info(cal).unwrap();
    assert_eq!(store.epoch(), epoch);
    assert_eq!(store.snapshot().unwrap().frame, new.frame);
}
#[test]
fn one_pending_slot_rejects_late_tf_after_replacement_calibration_and_stop() {
    let mut store = Store::new(owner()).unwrap();
    store.info(calibration(2)).unwrap();
    store.image(image(2), 0).unwrap();
    let stale = store.request(0).unwrap().unwrap();
    store.image(image(2), 1).unwrap();
    assert_eq!(store.pending_count(), 1);
    assert!(!store.complete(&stale, ready(1_000_000_000), 1).unwrap());
    let request = store.request(1).unwrap().unwrap();
    assert_eq!(store.snapshot().unwrap().counters.pending_dropped, 1);
    let mut cal = calibration(2);
    cal.p[0] = 150.;
    store.info(cal).unwrap();
    assert!(!store.complete(&request, ready(1_000_000_000), 2).unwrap());
    assert_eq!(store.pending_count(), 0);
    store.image(image(2), 3).unwrap();
    let request = store.request(3).unwrap().unwrap();
    store.stop();
    assert!(!store.complete(&request, ready(1_000_000_000), 4).unwrap());
    assert!(store.snapshot().is_none());
    let mut other = Store::new(owner()).unwrap();
    other.info(calibration(2)).unwrap();
    other.image(image(2), 0).unwrap();
    assert!(!other.complete(&stale, ready(1_000_000_000), 0).unwrap());
}
#[test]
fn exact_positive_latest_zero_retry_and_inclusive_timeout_are_distinct() {
    let mut store = Store::new(owner()).unwrap();
    store.info(calibration(2)).unwrap();
    store.image(image(2), 0).unwrap();
    let request = store.request(0).unwrap().unwrap();
    assert_eq!(request.time(), LookupTime::At(1_000_000_000));
    assert!(store.complete(&request, ready(2_000_000_000), 0).is_err());
    assert!(!store.complete(&request, LatestPose::Missing, 0).unwrap());
    assert!(store.request(RETRY_NS - 1).unwrap().is_none());
    assert!(store.request(RETRY_NS).unwrap().is_some());
    assert!(store.request(TIMEOUT_NS).unwrap().is_none());
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.status, Status::TfTimeout);
    assert_eq!(snapshot.counters.lookup_timeouts, 1);
    assert_eq!(store.pending_count(), 0);
    assert!(!store
        .complete(&request, ready(1_000_000_000), TIMEOUT_NS)
        .unwrap());
    let mut zero = image(2);
    zero.metadata.header_stamp.seconds = 0;
    store.image(zero, TIMEOUT_NS + 1).unwrap();
    let request = store.request(TIMEOUT_NS + 1).unwrap().unwrap();
    assert_eq!(request.time(), LookupTime::Latest);
    assert!(store.complete(&request, ready(99), TIMEOUT_NS + 1).unwrap());
}
#[test]
fn missing_and_malformed_inputs_hide_old_alignment_and_recover_explicitly() {
    let mut store = initialized(2);
    let mut mismatch = image(2);
    mismatch.metadata.frame_id = "other_optical".into();
    store.image(mismatch, 1).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::Incompatible);
    assert!(store.snapshot().unwrap().frame.is_none());
    let mut cal = calibration(2);
    cal.k = [0.; 9];
    cal.r = [0.; 9];
    cal.p = [0.; 12];
    store.info(cal).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::Uncalibrated);
    store.image(image(2), 2).unwrap();
    assert!(store.request(2).unwrap().is_none());
    store.invalid_info("malformed source".into()).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::InvalidInfo);
    store.image(image(2), 3).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::WaitingInfo);
    store.info(calibration(2)).unwrap();
    assert!(store.request(3).unwrap().is_none());
    store.image(image(2), 4).unwrap();
    let request = store.request(4).unwrap().unwrap();
    store.complete(&request, ready(1_000_000_000), 4).unwrap();
    let mut unsupported = image(2);
    unsupported.metadata.encoding = "16UC1".into();
    store.image(unsupported, 5).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::Unsupported);
    assert!(store.snapshot().unwrap().frame.is_none());
    let mut negative = image(2);
    negative.metadata.header_stamp.seconds = -1;
    store.image(negative, 6).unwrap();
    assert_eq!(store.snapshot().unwrap().status, Status::InvalidImage);
    // A backward monotonic clock retires this source instead of replaying data.
    assert!(store.request(5).is_err());
    assert!(store.snapshot().is_none());
}
#[test]
fn receiver_revokes_exact_cached_frames_on_barrier_and_terminal_retirement() {
    let mut store = initialized(2);
    let first = store.snapshot().unwrap();
    let mut receiver = Receiver::new(owner()).unwrap();
    let accepted = receiver.apply(first.clone()).unwrap().unwrap();
    assert!(accepted.permission.authorizes(&accepted.snapshot));
    assert!(!accepted.permission.authorizes(&Arc::new(first.clone())));
    let mut cal = calibration(2);
    cal.p[2] = 0.5;
    store.info(cal).unwrap();
    let empty = store.snapshot().unwrap();
    let barrier = Invalidation::from(&empty);
    assert!(receiver.invalidate(&barrier).unwrap());
    assert!(!accepted.permission.is_valid());
    assert!(receiver.apply(first).unwrap().is_none());
    receiver.apply(empty).unwrap().unwrap();
    assert!(!receiver.invalidate(&barrier).unwrap());
    store.image(image(2), 1).unwrap();
    let request = store.request(1).unwrap().unwrap();
    store.complete(&request, ready(1_000_000_000), 1).unwrap();
    let next = store.snapshot().unwrap();
    let current = receiver.apply(next.clone()).unwrap().unwrap();
    assert!(current.permission.is_valid());
    receiver.revocation().retire();
    assert!(!current.permission.is_valid());
    assert!(receiver.apply(next).unwrap().is_none());
}
#[test]
fn receiver_rejects_unannounced_calibration_and_replayed_image_identity() {
    let mut store = initialized(2);
    let first = store.snapshot().unwrap();
    let mut receiver = Receiver::new(owner()).unwrap();
    receiver.apply(first.clone()).unwrap();
    let mut changed = first.clone();
    changed.revision += 1;
    changed.frame.as_mut().unwrap().calibration.p[2] = 0.5;
    assert!(receiver.apply(changed).is_err());
    let mut receiver = Receiver::new(owner()).unwrap();
    receiver.apply(first.clone()).unwrap();
    let mut cal = calibration(2);
    cal.p[2] = 0.5;
    store.info(cal).unwrap();
    let barrier = store.invalidation().unwrap();
    receiver.invalidate(&barrier).unwrap();
    let mut replay = first;
    replay.revision = barrier.revision + 1;
    replay.continuity_epoch = barrier.continuity_epoch;
    assert!(receiver.apply(replay).is_err());
}
#[test]
fn maximum_frame_strict_wire_and_same_frame_identity_stay_bounded() {
    let mut snapshot = initialized(2048).snapshot().unwrap();
    let bytes = snapshot.encode().unwrap();
    assert!(bytes.len() < MAX_SAMPLE_BYTES);
    assert_eq!(Snapshot::decode(&bytes).unwrap(), snapshot);
    let message = Message::Snapshot(Box::new(snapshot.clone()));
    assert!(message.encode().unwrap().len() < MAX_SAMPLE_BYTES);
    assert_eq!(
        Message::decode(&message.encode().unwrap()).unwrap(),
        message
    );
    let control = Message::Invalidated(Invalidation::from(&snapshot));
    assert!(control.encode().unwrap().len() < MAX_CONTROL_BYTES);
    let mut json = serde_json::to_value(&snapshot).unwrap();
    json.as_object_mut().unwrap().remove("frame");
    assert!(Snapshot::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    let mut json = serde_json::to_value(&snapshot).unwrap();
    json["unknown"] = true.into();
    assert!(Snapshot::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(Snapshot::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1]).is_err());
    snapshot.owner.fixed_frame = "camera_optical".into();
    snapshot.frame.as_mut().unwrap().transform = LatestPose::Ready {
        resolved_stamp_ns: 1_000_000_000,
        components: [1., 0., 0., 0., 0., 0., 1.],
    };
    assert!(snapshot.validate().is_err());
    snapshot.frame.as_mut().unwrap().transform = ready(1_000_000_000);
    assert!(snapshot.validate().is_ok());
}
