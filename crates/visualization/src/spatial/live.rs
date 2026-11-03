//! Bounded worker input and latest-slot delivery; native tf2 remains upstream.
use super::{Clock, LayerToken, RigidTransform, Scan, Scene, SceneError, TfOutcome};
use std::{
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};
use studio_protocol::{
    spatial as wire,
    spatial_input::{self, SpatialClockPolicy, TypedScan},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpatialLiveError(pub String);
impl fmt::Display for SpatialLiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SpatialLiveError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialTfStatus {
    Ready,
    Missing,
    Stale,
    Extrapolated,
    LookupTimeout,
}
impl SpatialTfStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Transform ready",
            Self::Missing => "Waiting for transform",
            Self::Stale => "Transform stale",
            Self::Extrapolated => "Transform unavailable at acquisition time",
            Self::LookupTimeout => "Transform lookup timed out",
        }
    }
}

/// Owns only bounded decoded inputs, with no raw wire buffer or TF graph/cache.
#[derive(Debug)]
pub struct SpatialFrame {
    /// Worker-prepared fixed-frame samples with source-ray intensity alignment.
    pub sensor_points: Box<[super::sensor_history::SensorPoint]>,
    pub scan: TypedScan,
    pub fixed_frame: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub tf: wire::TfOutcome,
    pub counters: wire::Counters,
}
impl SpatialFrame {
    fn project_sensor_points(
        &self,
    ) -> Result<Box<[super::sensor_history::SensorPoint]>, SpatialLiveError> {
        if self.tf_status() != SpatialTfStatus::Ready {
            return Ok(Box::new([]));
        }
        let mut scene = Scene::default();
        scene
            .set_fixed_frame(&self.fixed_frame)
            .and_then(|_| scene.add_layer(1))
            .and_then(|_| scene.start())
            .map_err(|e| SpatialLiveError(format!("Sensor projection: {e:?}")))?;
        let token = scene
            .layer_token(1)
            .map_err(|e| SpatialLiveError(format!("Sensor projection: {e:?}")))?;
        self.apply(&mut scene, token)
            .map_err(|e| SpatialLiveError(format!("Sensor projection: {e:?}")))?;
        let points = scene.layers()[0].points();
        let mut indices = self.scan.ranges.iter().enumerate().filter_map(|(i, r)| {
            let r = f64::from(*r);
            (r.is_finite() && r >= self.scan.range_min && r <= self.scan.range_max).then_some(i)
        });
        let mut samples = Vec::with_capacity(points.len());
        for point in points {
            let i = indices
                .next()
                .ok_or_else(|| SpatialLiveError("Sensor intensity alignment failed".into()))?;
            samples.push(super::sensor_history::SensorPoint {
                position: *point,
                rgba: None,
                intensity: self
                    .scan
                    .intensities
                    .get(i)
                    .copied()
                    .filter(|v| v.is_finite())
                    .map(f64::from),
            });
        }
        if indices.next().is_some() {
            return Err(SpatialLiveError("Sensor intensity alignment failed".into()));
        }
        Ok(samples.into_boxed_slice())
    }
    pub fn tf_status(&self) -> SpatialTfStatus {
        match self.tf {
            wire::TfOutcome::Ready { .. } => SpatialTfStatus::Ready,
            wire::TfOutcome::Missing => SpatialTfStatus::Missing,
            wire::TfOutcome::Stale => SpatialTfStatus::Stale,
            wire::TfOutcome::Extrapolated => SpatialTfStatus::Extrapolated,
            wire::TfOutcome::LookupTimeout => SpatialTfStatus::LookupTimeout,
        }
    }
    /// Root retains the token from this layer's activation. Recoverable TF errors
    /// clear only that layer through Scene; they do not request group Stop.
    pub fn apply(&self, scene: &mut Scene, token: LayerToken) -> Result<(), SceneError> {
        let scan = Scan {
            frame: &self.scan.frame,
            stamp_ns: self.scan.stamp_ns,
            clock: Clock::System,
            complete: true,
            angle_min: self.scan.angle_min,
            angle_increment: self.scan.angle_increment,
            range_min: self.scan.range_min,
            range_max: self.scan.range_max,
            ranges: &self.scan.ranges,
        };
        let identity = scene.fixed_frame() == Some(self.fixed_frame.as_str())
            && self.source_frame == self.scan.frame
            && self.acquisition_stamp_ns == self.scan.stamp_ns;
        let tf = if !identity {
            TfOutcome::InvalidGraph
        } else {
            match self.tf {
                wire::TfOutcome::Ready { components: c } => {
                    match RigidTransform::new([c[0], c[1], c[2]], [c[3], c[4], c[5], c[6]]) {
                        Some(transform)
                            if c[..3].iter().all(|value| value.abs() <= 1_000_000.0) =>
                        {
                            TfOutcome::Ready {
                                source: &self.source_frame,
                                target: &self.fixed_frame,
                                stamp_ns: self.acquisition_stamp_ns,
                                transform,
                            }
                        }
                        _ => TfOutcome::InvalidGraph,
                    }
                }
                wire::TfOutcome::Missing | wire::TfOutcome::LookupTimeout => TfOutcome::Missing,
                wire::TfOutcome::Stale => TfOutcome::Stale,
                wire::TfOutcome::Extrapolated => TfOutcome::Extrapolated,
            }
        };
        scene.update(token, scan, tf)
    }
}

/// Only call for a negotiated Spatial subscription with explicitly verified
/// system-clock policy. A plain header never establishes that policy itself.
/// Validate/decode on the worker outside the shared UI registry/queue mutex.
pub fn decode_spatial_sample(sample: &wire::Sample) -> Result<SpatialFrame, SpatialLiveError> {
    fn invalid(message: impl fmt::Display) -> SpatialLiveError {
        SpatialLiveError(bounded_text(&message))
    }
    if sample.contract_version != wire::CONTRACT_VERSION
        || !wire::valid_frame(&sample.fixed_frame)
        || !wire::valid_frame(&sample.source_frame)
    {
        return Err(invalid("Invalid Spatial contract or frame identity"));
    }
    let scan = spatial_input::decode_laser_scan(&sample.scan, SpatialClockPolicy::VerifiedSystem)
        .map_err(invalid)?;
    if sample.source_frame != scan.frame || sample.acquisition_stamp_ns != scan.stamp_ns {
        return Err(invalid(
            "Spatial transform identity contradicts acquisition header",
        ));
    }
    if let wire::TfOutcome::Ready { components: c } = sample.tf {
        if c[..3].iter().any(|value| value.abs() > 1_000_000.0)
            || RigidTransform::new([c[0], c[1], c[2]], [c[3], c[4], c[5], c[6]]).is_none()
        {
            return Err(invalid("Invalid Spatial rigid transform"));
        }
    }
    // Count the whole envelope without allocating a second serialized payload
    // or decoding the complete typed scan again through Sample::validate().
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|size| *size < wire::MAX_SAMPLE_BYTES)
                .ok_or_else(|| std::io::Error::other("Spatial sample exceeds byte budget"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Count(0), sample).map_err(invalid)?;
    let mut frame = SpatialFrame {
        sensor_points: Box::new([]),
        scan,
        fixed_frame: sample.fixed_frame.clone(),
        source_frame: sample.source_frame.clone(),
        acquisition_stamp_ns: sample.acquisition_stamp_ns,
        tf: sample.tf.clone(),
        counters: sample.counters,
    };
    frame.sensor_points = frame.project_sensor_points()?;
    Ok(frame)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpatialUpdate {
    pub sequence: u64,
    pub received_updates: u64,
    pub backend_drops: u64,
    pub source_counters: wire::Counters,
    pub tf_status: Option<SpatialTfStatus>,
    pub error: Option<String>,
    pub superseded: u64,
    pub stale_rejected: u64,
}
#[derive(Default)]
struct Slot {
    generation: u64,
    active: bool,
    pending: Option<(SpatialFrame, SpatialUpdate)>,
    latest: Option<SpatialUpdate>,
    superseded: u64,
    stale_rejected: u64,
}
/// One pending decoded scan and one coalesced bounded status; no history.
#[derive(Clone, Default)]
pub struct SpatialStream(Arc<Mutex<Slot>>);
impl SpatialStream {
    pub fn start(&self) -> u64 {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let generation = slot
            .generation
            .checked_add(1)
            .expect("Spatial local generation exhausted");
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
        result: Result<SpatialFrame, SpatialLiveError>,
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
        let mut pending = None;
        match result {
            Ok(frame) => {
                counters = max_counters(counters, frame.counters);
                if error.is_none() {
                    tf_status = Some(frame.tf_status());
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
        let update = SpatialUpdate {
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
        };
        if let Some(frame) = pending {
            slot.pending = Some((frame, update.clone()));
        }
        slot.latest = Some(update);
        true
    }
    pub fn take(&self, generation: u64) -> Option<SpatialFrame> {
        self.take_with_status(generation).map(|(frame, _)| frame)
    }
    /// Atomically retain the exact status that accompanied this pending frame.
    /// snapshot() may describe newer wire activity than an already applied scene.
    pub fn take_with_status(&self, generation: u64) -> Option<(SpatialFrame, SpatialUpdate)> {
        let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !slot.active || generation != slot.generation {
            return None;
        }
        slot.pending.take()
    }
    pub fn snapshot(&self) -> Option<SpatialUpdate> {
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
        scans_received: a.scans_received.max(b.scans_received),
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
fn bounded_text(value: &impl fmt::Display) -> String {
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
    use crate::spatial::{LayerStatus, Point3};
    use studio_protocol::structured::{self, Field, FloatValue, Value};
    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object {
            original_fields: fields.len(),
            fields: fields
                .into_iter()
                .map(|(name, value)| Field {
                    name: name.into(),
                    value,
                })
                .collect(),
        }
    }
    fn float(value: f64) -> Value {
        Value::Float {
            value: if value.is_nan() {
                FloatValue::Nan
            } else {
                FloatValue::Finite(value.to_string())
            },
        }
    }
    fn uint(value: u64) -> Value {
        Value::Unsigned {
            value: value.to_string(),
        }
    }
    fn text(value: &str) -> Value {
        Value::Text {
            value: value.into(),
            original_bytes: value.len(),
        }
    }
    fn sample(stamp: u64, tf: wire::TfOutcome) -> wire::Sample {
        wire::Sample {
            contract_version: wire::CONTRACT_VERSION,
            fixed_frame: "map".into(),
            source_frame: "laser".into(),
            acquisition_stamp_ns: stamp as i64 * 1_000_000_000,
            tf,
            counters: wire::Counters {
                scans_received: stamp,
                tf_accepted: stamp * 2,
                ..wire::Counters::default()
            },
            scan: Box::new(structured::Sample {
                contract_version: structured::CONTRACT_VERSION,
                topic: "/scan".into(),
                message_type: spatial_input::SCAN_MESSAGE_TYPE.into(),
                source_time: None,
                value: object(vec![
                    (
                        "header",
                        object(vec![
                            ("frame_id", text("laser")),
                            (
                                "stamp",
                                object(vec![
                                    (
                                        "sec",
                                        Value::Signed {
                                            value: stamp.to_string(),
                                        },
                                    ),
                                    ("nanosec", uint(0)),
                                ]),
                            ),
                        ]),
                    ),
                    ("angle_min", float(0.0)),
                    ("angle_max", float(0.5)),
                    ("angle_increment", float(0.25)),
                    ("time_increment", float(0.0)),
                    ("scan_time", float(0.1)),
                    ("range_min", float(0.1)),
                    ("range_max", float(10.0)),
                    (
                        "ranges",
                        Value::Array {
                            items: vec![float(1.0), float(f64::NAN), float(2.0)],
                            original_items: 3,
                        },
                    ),
                    (
                        "intensities",
                        Value::Array {
                            items: vec![],
                            original_items: 0,
                        },
                    ),
                ]),
            }),
        }
    }
    fn identity() -> wire::TfOutcome {
        wire::TfOutcome::Ready {
            components: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        }
    }
    #[test]
    fn worker_projection_keeps_intensity_source_indices_after_invalid_rays_and_tf_clear() {
        let mut source = sample(
            1,
            wire::TfOutcome::Ready {
                components: [1., 2., 3., 0., 0., 0., 1.],
            },
        );
        let Value::Object { fields, .. } = &mut source.scan.value else {
            panic!("scan fields")
        };
        fields
            .iter_mut()
            .find(|f| f.name == "intensities")
            .unwrap()
            .value = Value::Array {
            original_items: 3,
            items: vec![float(100.), float(999.), float(f64::NAN)],
        };
        let frame = decode_spatial_sample(&source).unwrap();
        assert_eq!(frame.sensor_points.len(), 2);
        assert_eq!(frame.sensor_points[0].position, Point3([2., 2., 3.]));
        assert_eq!(frame.sensor_points[0].intensity, Some(100.));
        assert_eq!(frame.sensor_points[1].intensity, None);
        assert!((frame.sensor_points[1].position.0[0] - (1. + 2. * 0.5f64.cos())).abs() < 1e-6);
        source.tf = wire::TfOutcome::Missing;
        assert!(decode_spatial_sample(&source)
            .unwrap()
            .sensor_points
            .is_empty());
        source.tf = wire::TfOutcome::Ready {
            components: [1_000_000., 0., 0., 0., 0., 0., 1.],
        };
        assert!(decode_spatial_sample(&source).is_err());
    }
    fn frame(stamp: u64, tf: wire::TfOutcome) -> SpatialFrame {
        decode_spatial_sample(&sample(stamp, tf)).unwrap()
    }
    fn scene() -> (Scene, LayerToken) {
        let mut scene = Scene::default();
        scene.set_fixed_frame("map").unwrap();
        scene.add_layer(1).unwrap();
        scene.start().unwrap();
        let token = scene.layer_token(1).unwrap();
        (scene, token)
    }

    #[test]
    fn exact_native_transform_projects_scan_without_clock_or_identity_fallback() {
        let (mut scene, token) = scene();
        let input = frame(
            1,
            wire::TfOutcome::Ready {
                components: [10.0, -5.0, 0.0, 0.0, 0.0, 0.5f64.sqrt(), 0.5f64.sqrt()],
            },
        );
        input.apply(&mut scene, token).unwrap();
        assert_eq!(scene.layers()[0].skipped_rays, 1);
        let Point3([x, y, z]) = scene.layers()[0].points()[0];
        assert!((x - 10.0).abs() < 1e-12);
        assert!((y + 4.0).abs() < 1e-12);
        assert_eq!(z, 0.0);
        assert_eq!(scene.layers()[0].status(), &LayerStatus::Receiving);
        assert_eq!(input.acquisition_stamp_ns, input.scan.stamp_ns);
    }

    #[test]
    fn missing_then_ready_same_acquisition_and_all_recoverable_tf_states_keep_group_active() {
        let (mut scene, token) = scene();
        frame(1, identity()).apply(&mut scene, token).unwrap();
        assert_eq!(
            frame(2, wire::TfOutcome::Missing).apply(&mut scene, token),
            Err(SceneError::MissingTf)
        );
        assert!(scene.layers()[0].points().is_empty());
        assert!(scene.is_active());
        frame(2, identity()).apply(&mut scene, token).unwrap();
        for (stamp, tf, expected, status) in [
            (
                3,
                wire::TfOutcome::Stale,
                SceneError::StaleTf,
                SpatialTfStatus::Stale,
            ),
            (
                4,
                wire::TfOutcome::Extrapolated,
                SceneError::ExtrapolatedTf,
                SpatialTfStatus::Extrapolated,
            ),
            (
                5,
                wire::TfOutcome::LookupTimeout,
                SceneError::MissingTf,
                SpatialTfStatus::LookupTimeout,
            ),
        ] {
            let input = frame(stamp, tf);
            assert_eq!(input.tf_status(), status);
            assert_eq!(input.apply(&mut scene, token), Err(expected));
            assert!(scene.layers()[0].points().is_empty());
            assert!(scene.is_active());
            frame(stamp, identity()).apply(&mut scene, token).unwrap();
        }
    }

    #[test]
    fn worker_flood_is_latest_only_and_source_drop_counters_never_decrease() {
        let stream = SpatialStream::default();
        let generation = stream.start();
        let worker = stream.clone();
        std::thread::spawn(move || {
            for sequence in 1..=1000 {
                assert!(worker.publish(
                    generation,
                    sequence,
                    sequence % 17,
                    Ok(frame(sequence, identity()))
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
        let (input, paired) = stream.take_with_status(generation).unwrap();
        assert_eq!(paired.sequence, 1000);
        assert_eq!(input.scan.stamp_ns, 1_000_000_000_000);
        assert_eq!(paired.tf_status, Some(input.tf_status()));
        assert!(stream.take(generation).is_none());
        let mut lower = frame(1001, identity());
        lower.counters = wire::Counters::default();
        assert!(stream.publish(generation, 1001, 0, Ok(lower)));
        let next = stream.snapshot().unwrap();
        assert_eq!(next.source_counters.scans_received, 1000);
        assert_eq!(next.source_counters.tf_accepted, 2000);
        assert_eq!(next.backend_drops, 16);
        assert_eq!(next.superseded, 999);
    }

    #[test]
    fn old_generation_error_and_take_cannot_touch_replacement_pending_data() {
        let stream = SpatialStream::default();
        let old = stream.start();
        assert!(stream.publish(old, 1, 0, Ok(frame(1, identity()))));
        let current = stream.start();
        assert!(stream.publish(current, 1, 0, Ok(frame(2, identity()))));
        assert!(!stream.publish(old, 2, 99, Err(SpatialLiveError("old error".into()))));
        assert!(stream.take(old).is_none());
        assert_eq!(stream.snapshot().unwrap().error, None);
        assert_eq!(stream.take(current).unwrap().scan.stamp_ns, 2_000_000_000);
        stream.stop();
        assert!(!stream.publish(current, 2, 0, Ok(frame(3, identity()))));
        assert!(stream.take(current).is_none());
    }

    #[test]
    fn errors_latch_bounded_text_until_restart_without_inventing_native_cleanup() {
        let stream = SpatialStream::default();
        let generation = stream.start();
        assert!(stream.publish(generation, 1, 9, Ok(frame(1, identity()))));
        assert!(stream.publish(generation, 2, 0, Err(SpatialLiveError("🤖".repeat(2000)))));
        assert!(stream.take(generation).is_none());
        let error = stream.snapshot().unwrap().error.unwrap();
        assert!(error.len() <= 512);
        assert!(error.ends_with("[truncated]"));
        assert!(stream.publish(generation, 3, 0, Ok(frame(3, identity()))));
        assert!(stream.take(generation).is_none());
        assert_eq!(stream.snapshot().unwrap().error, Some(error));
        assert_eq!(stream.snapshot().unwrap().backend_drops, 9);
        let generation = stream.start();
        assert!(stream.snapshot().is_none());
        assert!(stream.publish(generation, 1, 0, Ok(frame(4, identity()))));
        assert!(stream.take(generation).is_some());
    }

    #[test]
    fn paired_status_describes_applied_frame_even_when_latest_wire_status_advances() {
        let stream = SpatialStream::default();
        let generation = stream.start();
        assert!(stream.publish(generation, 1, 0, Ok(frame(1, wire::TfOutcome::Missing))));
        assert!(stream.publish(generation, 2, 0, Ok(frame(1, identity()))));
        let (input, paired) = stream.take_with_status(generation).unwrap();
        assert_eq!(paired.sequence, 2);
        assert_eq!(paired.tf_status, Some(SpatialTfStatus::Ready));
        assert!(stream.publish(generation, 3, 0, Ok(frame(2, wire::TfOutcome::Missing))));
        assert_eq!(
            stream.snapshot().unwrap().tf_status,
            Some(SpatialTfStatus::Missing)
        );
        let (mut scene, token) = scene();
        input.apply(&mut scene, token).unwrap();
        assert_eq!(paired.tf_status, Some(SpatialTfStatus::Ready));
        assert!(!scene.layers()[0].points().is_empty());
        for sequence in [0, 1, 2, 3] {
            assert!(!stream.publish(
                generation,
                sequence,
                99,
                Err(SpatialLiveError("reordered".into()))
            ));
        }
        assert_eq!(stream.snapshot().unwrap().error, None);
        assert_eq!(stream.snapshot().unwrap().backend_drops, 0);
    }

    #[test]
    fn stale_layer_tokens_and_same_frame_missing_tf_never_fabricate_or_clear_replacements() {
        let (mut scene, old) = scene();
        scene.stop();
        scene.start().unwrap();
        let current = scene.layer_token(1).unwrap();
        frame(3, identity()).apply(&mut scene, current).unwrap();
        let points = scene.layers()[0].points().to_vec();
        assert_eq!(
            frame(2, wire::TfOutcome::Missing).apply(&mut scene, old),
            Err(SceneError::StaleGeneration)
        );
        assert_eq!(scene.layers()[0].points(), points);
        let mut same = frame(4, wire::TfOutcome::Missing);
        same.source_frame = "map".into();
        same.scan.frame = "map".into();
        assert_eq!(same.apply(&mut scene, current), Err(SceneError::MissingTf));
        assert!(scene.layers()[0].points().is_empty());
    }

    #[test]
    fn decoder_rejects_mismatched_clock_identity_transform_and_incomplete_input() {
        let mut wrong = sample(1, identity());
        wrong.acquisition_stamp_ns += 1;
        assert!(decode_spatial_sample(&wrong).is_err());
        let mut wrong = sample(1, identity());
        wrong.source_frame = "other".into();
        assert!(decode_spatial_sample(&wrong).is_err());
        let mut wrong = sample(1, identity());
        wrong.fixed_frame = "/map".into();
        assert!(decode_spatial_sample(&wrong).is_err());
        let mut wrong = sample(
            1,
            wire::TfOutcome::Ready {
                components: [0.0; 7],
            },
        );
        assert!(decode_spatial_sample(&wrong).is_err());
        wrong.tf = wire::TfOutcome::Ready {
            components: [f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        };
        assert!(decode_spatial_sample(&wrong).is_err());
        let mut wrong = sample(1, identity());
        wrong.scan.source_time = Some(structured::SourceTime {
            seconds: 1,
            nanoseconds: 0,
            clock: structured::ClockDomain::Ros { epoch: 1 },
            header_path: vec![
                structured::PathSegment::Member("header".into()),
                structured::PathSegment::Member("stamp".into()),
            ],
        });
        assert!(decode_spatial_sample(&wrong).is_err());
        let mut wrong = sample(1, identity());
        let Value::Object { fields, .. } = &mut wrong.scan.value else {
            unreachable!()
        };
        let Value::Array { original_items, .. } = &mut fields
            .iter_mut()
            .find(|field| field.name == "ranges")
            .unwrap()
            .value
        else {
            unreachable!()
        };
        *original_items += 1;
        assert!(decode_spatial_sample(&wrong).is_err());
    }

    #[test]
    fn decoded_scan_obeys_2048_ray_transport_bound_not_scene_4096_cap() {
        let mut sample = sample(1, identity());
        let Value::Object { fields, .. } = &mut sample.scan.value else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|field| field.name == "angle_max")
            .unwrap()
            .value = float((wire::MAX_SCAN_RAYS - 1) as f64 * 0.25);
        fields
            .iter_mut()
            .find(|field| field.name == "ranges")
            .unwrap()
            .value = Value::Array {
            items: vec![float(1.0); wire::MAX_SCAN_RAYS],
            original_items: wire::MAX_SCAN_RAYS,
        };
        assert_eq!(
            decode_spatial_sample(&sample).unwrap().scan.ranges.len(),
            2048
        );
        let Value::Object { fields, .. } = &mut sample.scan.value else {
            unreachable!()
        };
        let Value::Array {
            items,
            original_items,
        } = &mut fields
            .iter_mut()
            .find(|field| field.name == "ranges")
            .unwrap()
            .value
        else {
            unreachable!()
        };
        items.push(float(1.0));
        *original_items += 1;
        assert!(decode_spatial_sample(&sample).is_err());
    }

    #[test]
    fn whole_spatial_envelope_bound_includes_metadata_beyond_its_typed_scan() {
        let mut sample = sample(1, identity());
        let Value::Object {
            fields,
            original_fields,
        } = &mut sample.scan.value
        else {
            unreachable!()
        };
        for (index, len) in [structured::MAX_TEXT_BYTES, structured::MAX_TEXT_BYTES, 0]
            .into_iter()
            .enumerate()
        {
            fields.push(Field {
                name: format!("extra_{index}"),
                value: Value::Text {
                    value: "\0".repeat(len),
                    original_bytes: len,
                },
            });
            *original_fields += 1;
        }
        let mut low = 0;
        let mut high = structured::MAX_TEXT_BYTES + 1;
        while low + 1 < high {
            let mid = (low + high) / 2;
            let Value::Object { fields, .. } = &mut sample.scan.value else {
                unreachable!()
            };
            fields.last_mut().unwrap().value = Value::Text {
                value: "\0".repeat(mid),
                original_bytes: mid,
            };
            if sample.scan.encode().is_ok() {
                low = mid;
            } else {
                high = mid;
            }
        }
        let Value::Object { fields, .. } = &mut sample.scan.value else {
            unreachable!()
        };
        fields.last_mut().unwrap().value = Value::Text {
            value: "\0".repeat(low),
            original_bytes: low,
        };
        let typed = sample.scan.encode().unwrap();
        assert!(typed.len() <= structured::MAX_SAMPLE_BYTES);
        assert!(sample.encode().is_err());
        assert!(decode_spatial_sample(&sample).is_err());
    }
}
