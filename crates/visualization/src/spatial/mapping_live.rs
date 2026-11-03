//! Worker-only bounded mapping transforms and generation-bound latest delivery.
use super::{
    map_geometry::{bounded_point, prepare_map, PreparedMap},
    Point3, RigidTransform, SpatialTfStatus,
};
use std::{
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};
use studio_protocol::{mapping as wire, spatial::TfOutcome};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingLiveError(pub String);
impl fmt::Display for MappingLiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MappingLiveError {}
#[derive(Clone, Debug)]
pub enum WorldGeometry {
    Map(PreparedMap),
    Pose(RigidTransform),
    Path(Vec<Point3>),
}
#[derive(Clone, Debug)]
pub struct MappingFrame {
    pub geometry: Option<WorldGeometry>,
    pub message_type: String,
    pub fixed_frame: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub counters: wire::Counters,
    tf_status: SpatialTfStatus,
}
impl MappingFrame {
    pub fn tf_status(&self) -> SpatialTfStatus {
        self.tf_status
    }
}
fn text(v: impl fmt::Display) -> String {
    let mut s = v.to_string();
    let mut n = s.len().min(512);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s.truncate(n);
    s
}
fn transform(c: [f64; 7]) -> Result<RigidTransform, MappingLiveError> {
    if !wire::valid_pose(&c) {
        return Err(MappingLiveError("Invalid mapping rigid transform".into()));
    }
    RigidTransform::new([c[0], c[1], c[2]], [c[3], c[4], c[5], c[6]])
        .ok_or_else(|| MappingLiveError("Invalid mapping quaternion".into()))
}
pub fn decode_mapping_sample(sample: &wire::Sample) -> Result<MappingFrame, MappingLiveError> {
    sample.validate().map_err(|e| MappingLiveError(text(e)))?;
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(b.len())
                .filter(|n| *n < wire::MAX_SAMPLE_BYTES)
                .ok_or_else(|| std::io::Error::other("Mapping envelope exceeds byte budget"))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Count(0), sample).map_err(|e| MappingLiveError(text(e)))?;
    let tf_status = match sample.tf {
        TfOutcome::Ready { .. } => SpatialTfStatus::Ready,
        TfOutcome::Missing => SpatialTfStatus::Missing,
        TfOutcome::Stale => SpatialTfStatus::Stale,
        TfOutcome::Extrapolated => SpatialTfStatus::Extrapolated,
        TfOutcome::LookupTimeout => SpatialTfStatus::LookupTimeout,
    };
    let geometry = if let TfOutcome::Ready { components } = sample.tf {
        let tf = transform(components)?;
        Some(match &sample.geometry {
            wire::Geometry::OccupancyGrid {
                width,
                height,
                resolution,
                origin,
                cells,
                ..
            } => WorldGeometry::Map(
                prepare_map(*width, *height, *resolution, transform(*origin)?, tf, cells)
                    .map_err(|e| MappingLiveError(text(e)))?,
            ),
            wire::Geometry::Pose { components } => {
                let pose = tf.compose(transform(*components)?);
                super::overlay_geometry::pose_segments(pose)
                    .map_err(|e| MappingLiveError(text(e)))?;
                WorldGeometry::Pose(pose)
            }
            wire::Geometry::Path { poses } => {
                let mut points = Vec::with_capacity(poses.len());
                for pose in poses {
                    let c = pose.components;
                    let p = tf.apply(Point3([c[0], c[1], c[2]]));
                    if !bounded_point(p) {
                        return Err(MappingLiveError("Mapping Path exceeds world bounds".into()));
                    }
                    points.push(p);
                }
                WorldGeometry::Path(points)
            }
        })
    } else {
        None
    };
    Ok(MappingFrame {
        geometry,
        message_type: sample.message_type.clone(),
        fixed_frame: sample.fixed_frame.clone(),
        source_frame: sample.source_frame.clone(),
        acquisition_stamp_ns: sample.acquisition_stamp_ns,
        counters: sample.counters,
        tf_status,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingUpdate {
    pub sequence: u64,
    pub received_updates: u64,
    pub backend_drops: u64,
    pub source_counters: wire::Counters,
    pub tf_status: Option<SpatialTfStatus>,
    pub error: Option<String>,
    pub superseded: u64,
    pub stale_rejected: u64,
    pub message_type: String,
    pub fixed_frame: String,
}
#[derive(Default)]
struct Slot {
    generation: u64,
    active: bool,
    pending: Option<(MappingFrame, MappingUpdate)>,
    latest: Option<MappingUpdate>,
    superseded: u64,
    stale_rejected: u64,
}
#[derive(Clone, Default)]
pub struct MappingStream(Arc<Mutex<Slot>>);
impl MappingStream {
    pub fn start(&self) -> u64 {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let generation = s
            .generation
            .checked_add(1)
            .expect("Mapping generation exhausted");
        *s = Slot {
            generation,
            active: true,
            ..Slot::default()
        };
        generation
    }
    pub fn stop(&self) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.active = false;
        s.pending = None;
    }
    pub fn publish(
        &self,
        generation: u64,
        sequence: u64,
        backend_drops: u64,
        result: Result<MappingFrame, MappingLiveError>,
    ) -> bool {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !s.active
            || s.generation != generation
            || sequence == 0
            || s.latest.as_ref().is_some_and(|u| sequence <= u.sequence)
        {
            s.stale_rejected = s.stale_rejected.saturating_add(1);
            return false;
        }
        let previous = s.latest.take();
        let mut update = previous.clone().unwrap_or(MappingUpdate {
            sequence,
            received_updates: 0,
            backend_drops: 0,
            source_counters: wire::Counters::default(),
            tf_status: None,
            error: None,
            superseded: 0,
            stale_rejected: 0,
            message_type: String::new(),
            fixed_frame: String::new(),
        });
        update.sequence = sequence;
        update.received_updates = update.received_updates.saturating_add(1);
        update.backend_drops = update.backend_drops.max(backend_drops);
        let mut pending = None;
        match result {
            Ok(frame) => {
                update.source_counters = max_counters(update.source_counters, frame.counters);
                if update.error.is_none() {
                    update.tf_status = Some(frame.tf_status());
                    update.message_type = frame.message_type.clone();
                    update.fixed_frame = frame.fixed_frame.clone();
                    if s.pending.is_some() {
                        s.superseded = s.superseded.saturating_add(1);
                    }
                    pending = Some(frame);
                }
            }
            Err(e) => {
                if update.error.is_none() {
                    update.error = Some(text(e));
                }
            }
        }
        update.superseded = s.superseded;
        update.stale_rejected = s.stale_rejected;
        s.pending = pending.map(|frame| (frame, update.clone()));
        s.latest = Some(update);
        true
    }
    pub fn take(&self, generation: u64) -> Option<MappingFrame> {
        self.take_with_status(generation).map(|(frame, _)| frame)
    }
    pub fn take_with_status(&self, generation: u64) -> Option<(MappingFrame, MappingUpdate)> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !s.active || s.generation != generation {
            return None;
        }
        s.pending.take()
    }
    pub fn snapshot(&self) -> Option<MappingUpdate> {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.latest.clone().map(|mut u| {
            u.superseded = s.superseded;
            u.stale_rejected = s.stale_rejected;
            u
        })
    }
}
fn max_counters(a: wire::Counters, b: wire::Counters) -> wire::Counters {
    wire::Counters {
        messages_received: a.messages_received.max(b.messages_received),
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
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> wire::Sample {
        wire::Sample {
            contract_version: 1,
            topic: "/pose".into(),
            message_type: wire::POSE_MESSAGE_TYPE.into(),
            fixed_frame: "map".into(),
            source_frame: "sensor".into(),
            acquisition_stamp_ns: 123,
            geometry: wire::Geometry::Pose {
                components: [1., 0., 0., 0., 0., 0., 1.],
            },
            tf: TfOutcome::Ready {
                components: [2., 3., 0., 0., 0., 0., 1.],
            },
            counters: wire::Counters::default(),
        }
    }
    #[test]
    fn acquisition_transform_missing_clearing_recovery_and_error_latch() {
        let stream = MappingStream::default();
        let g = stream.start();
        let mut sample = sample();
        let frame = decode_mapping_sample(&sample).unwrap();
        if let Some(WorldGeometry::Pose(p)) = frame.geometry.as_ref() {
            assert_eq!(p.apply(Point3([0.; 3])).0, [3., 3., 0.]);
        } else {
            panic!("pose");
        }
        assert!(stream.publish(g, 1, 0, Ok(frame)));
        sample.tf = TfOutcome::Missing;
        assert!(stream.publish(g, 2, 0, decode_mapping_sample(&sample)));
        assert!(stream.take(g).unwrap().geometry.is_none());
        sample.tf = TfOutcome::Ready {
            components: [0., 0., 0., 0., 0., 0., 1.],
        };
        assert!(stream.publish(g, 3, 0, decode_mapping_sample(&sample)));
        assert!(stream.take(g).unwrap().geometry.is_some());
        sample.acquisition_stamp_ns = 0;
        assert!(stream.publish(g, 4, 0, decode_mapping_sample(&sample)));
        assert!(stream.snapshot().unwrap().error.is_some());
        sample.acquisition_stamp_ns = 123;
        stream.publish(g, 5, 0, decode_mapping_sample(&sample));
        assert!(stream.take(g).is_none());
        stream.stop();
        assert!(!stream.publish(g, 6, 0, decode_mapping_sample(&sample)));
        let next = stream.start();
        assert!(!stream.publish(g, 7, 0, decode_mapping_sample(&sample)));
        assert!(stream.publish(next, 1, 0, decode_mapping_sample(&sample)));
    }
    #[test]
    fn coalescing_and_duplicate_rejection_preserve_pending_metadata() {
        let s = MappingStream::default();
        let g = s.start();
        for sequence in 1..=1000 {
            s.publish(g, sequence, sequence, decode_mapping_sample(&sample()));
        }
        assert!(!s.publish(g, 999, 0, Err(MappingLiveError("old".into()))));
        let (_, u) = s.take_with_status(g).unwrap();
        assert_eq!(u.sequence, 1000);
        assert_eq!(u.superseded, 999);
        assert!(u.error.is_none());
        assert_eq!(u.fixed_frame, "map");
        assert_eq!(s.snapshot().unwrap().stale_rejected, 1);
    }
}
