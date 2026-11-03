//! Worker-only PointCloud2 validation/projection and bounded latest-slot delivery.
//! Native TF remains upstream; no acquisition-time result is replaced by latest,
//! receipt-time or identity transforms here.
use super::{Point3, RigidTransform, SpatialTfStatus};
use std::{
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};
use studio_protocol::{cloud as wire, spatial::TfOutcome};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudLiveError(pub String);
impl fmt::Display for CloudLiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CloudLiveError {}
#[derive(Debug)]
pub struct CloudFrame {
    pub sensor_points: Box<[super::sensor_history::SensorPoint]>,
    /// Fixed-frame points, empty whenever the acquisition transform is unavailable.
    pub points: Vec<Point3>,
    pub channels: wire::Channels,
    pub fixed_frame: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub counters: wire::Counters,
    /// Source decode accounting remains available even while TF hides geometry.
    pub source_points: usize,
    pub skipped_points: usize,
    tf_status: SpatialTfStatus,
}
impl CloudFrame {
    pub fn tf_status(&self) -> SpatialTfStatus {
        self.tf_status
    }
}
/// Call off UI frames, after the ingress owner checks topic/fixed-frame identity.
/// Full source metadata/counts and the whole serialized envelope are bounded
/// before allocating the fixed-frame output. An invalid world pose fails the
/// layer rather than retaining or quietly truncating any previous geometry.
pub fn decode_cloud_sample(sample: &wire::Sample) -> Result<CloudFrame, CloudLiveError> {
    let invalid = |message: &dyn fmt::Display| CloudLiveError(bounded_text(message));
    sample.validate().map_err(|error| invalid(&error))?;
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n < wire::MAX_SAMPLE_BYTES)
                .ok_or_else(|| {
                    std::io::Error::other("PointCloud2 envelope exceeds its byte budget")
                })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Count(0), sample).map_err(|error| invalid(&error))?;
    let tf_status = match sample.tf {
        TfOutcome::Ready { .. } => SpatialTfStatus::Ready,
        TfOutcome::Missing => SpatialTfStatus::Missing,
        TfOutcome::Stale => SpatialTfStatus::Stale,
        TfOutcome::Extrapolated => SpatialTfStatus::Extrapolated,
        TfOutcome::LookupTimeout => SpatialTfStatus::LookupTimeout,
    };
    let mut points = Vec::new();
    if let TfOutcome::Ready { components: c } = sample.tf {
        let transform = RigidTransform::new([c[0], c[1], c[2]], [c[3], c[4], c[5], c[6]])
            .ok_or_else(|| invalid(&"PointCloud2 acquisition transform is invalid"))?;
        points.reserve_exact(sample.points.len());
        for source in &sample.points {
            let point = transform.apply(Point3(source.map(f64::from)));
            if !point
                .0
                .iter()
                .all(|v| v.is_finite() && v.abs() <= wire::MAX_COORDINATE)
            {
                return Err(invalid(
                    &"PointCloud2 transformed geometry exceeds world coordinate bounds",
                ));
            }
            points.push(point);
        }
    }
    let channels = if tf_status == SpatialTfStatus::Ready {
        sample.channels.clone()
    } else {
        sample.channels.empty_values()
    };
    let sensor_points =
        super::sensor_history::attributed_points(&points, &channels).map_err(|e| invalid(&e))?;
    Ok(CloudFrame {
        sensor_points,
        points,
        channels,
        fixed_frame: sample.fixed_frame.clone(),
        source_frame: sample.source_frame.clone(),
        acquisition_stamp_ns: sample.acquisition_stamp_ns,
        counters: sample.counters,
        source_points: sample.source_points,
        skipped_points: sample.skipped_points,
        tf_status,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudUpdate {
    pub sequence: u64,
    pub received_updates: u64,
    pub backend_drops: u64,
    pub source_counters: wire::Counters,
    pub tf_status: Option<SpatialTfStatus>,
    pub error: Option<String>,
    pub superseded: u64,
    pub stale_rejected: u64,
    pub source_points: usize,
    pub skipped_points: usize,
}
#[derive(Default)]
struct Slot {
    generation: u64,
    active: bool,
    pending: Option<(CloudFrame, CloudUpdate)>,
    latest: Option<CloudUpdate>,
    superseded: u64,
    stale_rejected: u64,
}
/// One pending decoded cloud and one coalesced bounded status; no history.
#[derive(Clone, Default)]
pub struct CloudStream(Arc<Mutex<Slot>>);
impl CloudStream {
    pub fn start(&self) -> u64 {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let generation = slot
            .generation
            .checked_add(1)
            .expect("Cloud local generation exhausted");
        *slot = Slot {
            generation,
            active: true,
            ..Slot::default()
        };
        generation
    }
    pub fn stop(&self) {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        slot.active = false;
        slot.pending = None;
    }
    pub fn publish(
        &self,
        generation: u64,
        sequence: u64,
        backend_drops: u64,
        result: Result<CloudFrame, CloudLiveError>,
    ) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !slot.active
            || generation != slot.generation
            || sequence == 0
            || slot
                .latest
                .as_ref()
                .is_some_and(|update| sequence <= update.sequence)
        {
            slot.stale_rejected = slot.stale_rejected.saturating_add(1);
            return false;
        }
        let previous = slot.latest.take();
        let mut error = previous.as_ref().and_then(|update| update.error.clone());
        let mut counters = previous
            .as_ref()
            .map_or(wire::Counters::default(), |update| update.source_counters);
        let mut tf_status = previous.as_ref().and_then(|update| update.tf_status);
        let mut source_points = previous.as_ref().map_or(0, |update| update.source_points);
        let mut skipped_points = previous.as_ref().map_or(0, |update| update.skipped_points);
        let mut pending = None;
        match result {
            Ok(frame) => {
                counters = max_counters(counters, frame.counters);
                if error.is_none() {
                    tf_status = Some(frame.tf_status());
                    source_points = frame.source_points;
                    skipped_points = frame.skipped_points;
                    if slot.pending.is_some() {
                        slot.superseded = slot.superseded.saturating_add(1);
                    }
                    pending = Some(frame);
                }
            }
            Err(failure) => {
                if error.is_none() {
                    error = Some(bounded_text(&failure));
                }
                slot.pending = None;
            }
        }
        let update = CloudUpdate {
            sequence,
            received_updates: previous
                .as_ref()
                .map_or(1, |update| update.received_updates.saturating_add(1)),
            backend_drops: previous.as_ref().map_or(backend_drops, |update| {
                update.backend_drops.max(backend_drops)
            }),
            source_counters: counters,
            tf_status,
            error,
            superseded: slot.superseded,
            stale_rejected: slot.stale_rejected,
            source_points,
            skipped_points,
        };
        if let Some(frame) = pending {
            slot.pending = Some((frame, update.clone()));
        }
        slot.latest = Some(update);
        true
    }
    pub fn take(&self, generation: u64) -> Option<CloudFrame> {
        self.take_with_status(generation).map(|(frame, _)| frame)
    }
    /// Atomically retain the exact status that accompanied this pending frame.
    /// snapshot() may describe newer wire activity than an already applied scene.
    pub fn take_with_status(&self, generation: u64) -> Option<(CloudFrame, CloudUpdate)> {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !slot.active || generation != slot.generation {
            return None;
        }
        slot.pending.take()
    }
    pub fn snapshot(&self) -> Option<CloudUpdate> {
        let slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        slot.latest.clone().map(|mut update| {
            update.superseded = slot.superseded;
            update.stale_rejected = slot.stale_rejected;
            update
        })
    }
}
fn max_counters(a: wire::Counters, b: wire::Counters) -> wire::Counters {
    wire::Counters {
        clouds_received: a.clouds_received.max(b.clouds_received),
        pending_replaced: a.pending_replaced.max(b.pending_replaced),
        lookup_timeouts: a.lookup_timeouts.max(b.lookup_timeouts),
        tf_dynamic_batches: a.tf_dynamic_batches.max(b.tf_dynamic_batches),
        tf_static_batches: a.tf_static_batches.max(b.tf_static_batches),
        tf_accepted: a.tf_accepted.max(b.tf_accepted),
        tf_rejected: a.tf_rejected.max(b.tf_rejected),
        duplicate_statics: a.duplicate_statics.max(b.duplicate_statics),
        native_errors: a.native_errors.max(b.native_errors),
    }
}
fn bounded_text(value: &(impl fmt::Display + ?Sized)) -> String {
    use fmt::Write;
    struct Text(String);
    impl Write for Text {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            let mut end = value.len().min(512usize.saturating_sub(self.0.len()));
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&value[..end]);
            if end == value.len() {
                Ok(())
            } else {
                Err(fmt::Error)
            }
        }
    }
    let mut text = Text(String::with_capacity(512));
    if write!(&mut text, "{value}").is_err() {
        let marker = " … [truncated]";
        let mut end = (512 - marker.len()).min(text.0.len());
        while !text.0.is_char_boundary(end) {
            end -= 1;
        }
        text.0.truncate(end);
        text.0.push_str(marker);
    }
    text.0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(stamp: i64, tf: TfOutcome) -> wire::Sample {
        wire::Sample {
            contract_version: wire::CONTRACT_VERSION,
            topic: "/cloud".into(),
            message_type: wire::MESSAGE_TYPE.into(),
            fixed_frame: "map".into(),
            source_frame: "lidar".into(),
            acquisition_stamp_ns: stamp,
            source_points: 2,
            skipped_points: 1,
            points: vec![[1.0, 0.0, 0.0]],
            channels: Default::default(),
            tf,
            counters: wire::Counters {
                clouds_received: stamp as u64,
                tf_accepted: 2 * stamp as u64,
                ..Default::default()
            },
        }
    }
    fn identity() -> TfOutcome {
        TfOutcome::Ready {
            components: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        }
    }
    fn frame(stamp: i64) -> CloudFrame {
        decode_cloud_sample(&sample(stamp, identity())).unwrap()
    }
    #[test]
    fn acquisition_transform_is_applied_once_and_source_counts_are_preserved() {
        let q = std::f64::consts::FRAC_PI_4;
        let sample = sample(
            42,
            TfOutcome::Ready {
                components: [2.0, 3.0, 4.0, 0.0, 0.0, q.sin(), q.cos()],
            },
        );
        let frame = decode_cloud_sample(&sample).unwrap();
        for (actual, expected) in frame.points[0].0.into_iter().zip([2.0, 4.0, 4.0]) {
            assert!((actual - expected).abs() < 1e-10);
        }
        assert_eq!(frame.acquisition_stamp_ns, 42);
        assert_eq!(frame.fixed_frame, "map");
        assert_eq!(frame.source_frame, "lidar");
        assert_eq!((frame.source_points, frame.skipped_points), (2, 1));
        assert_eq!(frame.tf_status(), SpatialTfStatus::Ready);
    }
    #[test]
    fn channels_stay_aligned_through_tf_clear_and_malformed_channels_fail() {
        let mut source = sample(42, identity());
        source.channels = wire::Channels {
            color: wire::Channel::Values(vec![[1, 2, 3, 4]]),
            intensity: wire::Channel::Values(vec![Some(42.)]),
        };
        let ready = decode_cloud_sample(&source).unwrap();
        assert_eq!(ready.channels, source.channels);
        source.tf = TfOutcome::Missing;
        let hidden = decode_cloud_sample(&source).unwrap();
        assert!(hidden.points.is_empty());
        assert_eq!(
            hidden.channels,
            wire::Channels {
                color: wire::Channel::Values(vec![]),
                intensity: wire::Channel::Values(vec![])
            }
        );
        hidden.channels.validate(0).unwrap();
        source.channels.intensity = wire::Channel::Values(vec![]);
        assert!(decode_cloud_sample(&source).is_err());
    }
    #[test]
    fn missing_transforms_clear_pending_geometry_and_ready_can_recover() {
        let stream = CloudStream::default();
        let generation = stream.start();
        assert!(stream.publish(generation, 1, 0, Ok(frame(1))));
        for (sequence, tf, status) in [
            (2, TfOutcome::Missing, SpatialTfStatus::Missing),
            (3, TfOutcome::Stale, SpatialTfStatus::Stale),
            (4, TfOutcome::Extrapolated, SpatialTfStatus::Extrapolated),
            (5, TfOutcome::LookupTimeout, SpatialTfStatus::LookupTimeout),
        ] {
            let mut unavailable = sample(sequence, tf);
            unavailable.source_frame = "map".into();
            let frame = decode_cloud_sample(&unavailable).unwrap();
            assert!(
                frame.points.is_empty(),
                "same source/fixed frame never fabricates identity"
            );
            assert!(stream.publish(generation, sequence as u64, 0, Ok(frame)));
            let (frame, update) = stream.take_with_status(generation).unwrap();
            assert!(frame.points.is_empty());
            assert_eq!(frame.tf_status(), status);
            assert_eq!(update.tf_status, Some(status));
            assert_eq!((update.source_points, update.skipped_points), (2, 1));
            assert!(update.error.is_none());
        }
        assert!(stream.publish(generation, 6, 0, Ok(frame(6))));
        assert_eq!(stream.take(generation).unwrap().points.len(), 1);
        let mut empty = sample(7, identity());
        empty.source_points = 0;
        empty.skipped_points = 0;
        empty.points.clear();
        assert!(stream.publish(generation, 7, 0, decode_cloud_sample(&empty)));
        assert!(
            stream.take(generation).unwrap().points.is_empty(),
            "valid empty cloud clears without an error"
        );
    }
    #[test]
    fn latest_slot_coalesces_and_keeps_drops_and_native_counters_monotonic() {
        let stream = CloudStream::default();
        let generation = stream.start();
        let worker = stream.clone();
        std::thread::spawn(move || {
            for sequence in 1..=1000 {
                assert!(worker.publish(
                    generation,
                    sequence,
                    sequence % 17,
                    Ok(frame(sequence as i64))
                ));
            }
        })
        .join()
        .unwrap();
        let update = stream.snapshot().unwrap();
        assert_eq!(update.sequence, 1000);
        assert_eq!(update.received_updates, 1000);
        assert_eq!(update.superseded, 999);
        assert_eq!(update.backend_drops, 16);
        assert_eq!(update.source_counters.clouds_received, 1000);
        let (frame, paired) = stream.take_with_status(generation).unwrap();
        assert_eq!(frame.acquisition_stamp_ns, 1000);
        assert_eq!(paired.sequence, 1000);
        assert_eq!(paired.tf_status, Some(frame.tf_status()));
        assert!(stream.take(generation).is_none());
        let mut lower = sample(1001, identity());
        lower.counters = wire::Counters::default();
        assert!(stream.publish(generation, 1001, 0, decode_cloud_sample(&lower)));
        let update = stream.snapshot().unwrap();
        assert_eq!(update.source_counters.clouds_received, 1000);
        assert_eq!(update.backend_drops, 16);
    }
    #[test]
    fn stale_generation_sequence_and_stop_cannot_touch_replacement_geometry() {
        let stream = CloudStream::default();
        let old = stream.start();
        stream.publish(old, 1, 0, Ok(frame(1)));
        let current = stream.start();
        stream.publish(current, 2, 0, Ok(frame(2)));
        assert!(!stream.publish(old, 3, 999, Err(CloudLiveError("old generation".into()))));
        assert!(!stream.publish(current, 2, 999, Err(CloudLiveError("old sequence".into()))));
        assert!(!stream.publish(current, 0, 999, Err(CloudLiveError("zero sequence".into()))));
        assert!(stream.take(old).is_none());
        let update = stream.snapshot().unwrap();
        assert!(update.error.is_none());
        assert_eq!(update.backend_drops, 0);
        assert_eq!(update.stale_rejected, 3);
        assert_eq!(stream.take(current).unwrap().acquisition_stamp_ns, 2);
        stream.publish(current, 3, 0, Ok(frame(3)));
        stream.stop();
        assert!(stream.take(current).is_none());
        assert!(!stream.publish(current, 4, 0, Ok(frame(4))));
        let fresh = stream.start();
        assert!(stream.snapshot().is_none());
        assert!(stream.publish(fresh, 1, 0, Ok(frame(5))));
        assert_eq!(stream.take(fresh).unwrap().points.len(), 1);
    }
    #[test]
    fn fatal_decoder_error_clears_pending_and_latches_until_explicit_start() {
        let stream = CloudStream::default();
        let generation = stream.start();
        stream.publish(generation, 1, 0, Ok(frame(1)));
        assert!(stream.publish(generation, 2, 4, Err(CloudLiveError("🤖".repeat(2000)))));
        assert!(stream.take(generation).is_none());
        let error = stream.snapshot().unwrap().error.unwrap();
        assert!(error.len() <= 512);
        assert!(error.ends_with("[truncated]"));
        stream.publish(generation, 3, 0, Ok(frame(3)));
        assert!(stream.take(generation).is_none());
        assert_eq!(stream.snapshot().unwrap().error, Some(error));
        let next = stream.start();
        stream.publish(next, 1, 0, Ok(frame(4)));
        assert_eq!(stream.take(next).unwrap().points.len(), 1);
    }
    #[test]
    fn malformed_counts_quaternions_source_and_world_bounds_fail_explicitly() {
        let mut invalid = sample(1, identity());
        invalid.source_points = 0;
        assert!(decode_cloud_sample(&invalid).is_err());
        invalid = sample(1, identity());
        invalid.points[0][0] = f32::NAN;
        assert!(decode_cloud_sample(&invalid).is_err());
        invalid = sample(
            1,
            TfOutcome::Ready {
                components: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0],
            },
        );
        assert!(decode_cloud_sample(&invalid).is_err());
        invalid = sample(0, identity());
        assert!(decode_cloud_sample(&invalid).is_err());
        invalid = sample(
            1,
            TfOutcome::Ready {
                components: [wire::MAX_COORDINATE, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            },
        );
        assert!(decode_cloud_sample(&invalid)
            .unwrap_err()
            .0
            .contains("world coordinate"));
        let mut maximum = sample(1, identity());
        maximum.source_points = wire::MAX_SOURCE_POINTS;
        maximum.skipped_points = 0;
        maximum.points = vec![[0.125, -0.25, 1.0]; wire::MAX_SOURCE_POINTS];
        assert_eq!(
            decode_cloud_sample(&maximum).unwrap().points.len(),
            wire::MAX_SOURCE_POINTS
        );
    }
}
