//! Bounded worker-local admission around standard native tf2. No ROS endpoints.
//!
//! Construct, use and drop on one worker: TransformBuffer's raw pointer is not
//! Send/Sync. Acquisition lookups never substitute receipt time or zero/latest;
//! Robot presentation has a separate explicitly latest-available lookup method.
use crate::transform_buffer::TransformBuffer;
#[path = "tf_graph_snapshot.rs"]
mod graph_snapshot;
use std::collections::{BTreeMap, HashSet, VecDeque};

pub const MAX_FRAMES: usize = 256;
pub const MAX_EDGES: usize = 128;
pub const MAX_BATCH: usize = 128;
pub const DYNAMIC_INSERTS_PER_WINDOW: usize = studio_protocol::spatial::DYNAMIC_INSERTS_PER_WINDOW;
/// Matches local tf2/buffer_core.hpp BUFFER_CORE_DEFAULT_CACHE_TIME (10 s).
/// The default constructor in native/tf_shim.cpp uses this standard cache.
pub const CACHE_WINDOW_NS: i64 = 10_000_000_000;
const MAX_STAMP_NS: i64 = i32::MAX as i64 * 1_000_000_000 + 999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockPolicy {
    Unverified,
    VerifiedSystem,
    Simulated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Dynamic,
    Static,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    pub parent: String,
    pub child: String,
    pub stamp_ns: i64,
    /// tx,ty,tz,qx,qy,qz,qw; metres and ROS right-handed quaternion order.
    pub components: [f64; 7],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Inactive,
    ClockUnverified,
    UnsupportedClock,
    Invalid,
    Missing,
    Stale,
    Extrapolated,
    Cyclic,
    OutOfOrder,
    Conflict,
    Capacity,
    RateLimited,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub status: Status,
    pub message: String,
}
impl Failure {
    fn new(status: Status, message: impl AsRef<str>) -> Self {
        Self {
            status,
            message: studio_protocol::session::bounded_error(message.as_ref()),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Lookup {
    pub target: String,
    pub source: String,
    pub stamp_ns: i64,
    pub components: [f64; 7],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub accepted: u64,
    pub duplicate_statics: u64,
    pub rejected: u64,
    pub native_errors: u64,
    pub resets: u64,
}
#[derive(Clone)]
struct Edge {
    parent: String,
    kind: StreamKind,
    last_stamp: Option<i64>,
    last_observed_stamp: Option<i64>,
    static_components: Option<[f64; 7]>,
    window: VecDeque<i64>,
}
#[derive(Clone, Default)]
struct Admission {
    edges: BTreeMap<String, Edge>,
    frames: HashSet<String>,
}
/// Frame/edge admission lasts for the entire native buffer lifetime, including
/// resets, since tf2 may retain frame-name IDs after clearing dynamic history.
/// At most 128 caches, 256 names and 256 accepted dynamic inserts per edge in
/// each inclusive 10-second source-time window. Native allocator overhead is
/// additional; this is an admission/cache bound, not a measured RSS guarantee.
pub struct SpatialSession {
    native: TransformBuffer,
    admission: Admission,
    active: bool,
    counters: Counters,
}
impl SpatialSession {
    pub fn new() -> Result<Self, Failure> {
        Ok(Self {
            native: TransformBuffer::new().map_err(native_failure)?,
            admission: Admission::default(),
            active: false,
            counters: Counters::default(),
        })
    }
    pub fn activate_explicit(&mut self, policy: ClockPolicy) -> Result<(), Failure> {
        match policy {
            ClockPolicy::VerifiedSystem => {
                self.active = true;
                Ok(())
            }
            ClockPolicy::Unverified => {
                self.active = false;
                Err(Failure::new(
                    Status::ClockUnverified,
                    "System clock policy must be verified explicitly",
                ))
            }
            ClockPolicy::Simulated => {
                self.active = false;
                Err(Failure::new(
                    Status::UnsupportedClock,
                    "Simulated clock is unsupported",
                ))
            }
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn frame_count(&self) -> usize {
        self.admission.frames.len()
    }
    pub fn frame_names(&self) -> Vec<String> {
        let mut frames: Vec<_> = self.admission.frames.iter().cloned().collect();
        frames.sort();
        frames
    }
    pub fn edge_count(&self) -> usize {
        self.admission.edges.len()
    }
    fn require_active(&self) -> Result<(), Failure> {
        if self.active {
            Ok(())
        } else {
            Err(Failure::new(
                Status::Inactive,
                "Spatial TF worker requires explicit activation",
            ))
        }
    }
    /// Reset is an explicit clock/session boundary, never automatic recovery.
    /// Static transforms and immutable topology remain; dynamic history is
    /// cleared and lookups/inserts fail until activate_explicit is called again.
    pub fn reset(&mut self) -> Result<(), Failure> {
        self.active = false;
        if let Err(error) = self.native.clear() {
            self.counters.native_errors = self.counters.native_errors.saturating_add(1);
            return Err(native_failure(error));
        }
        for edge in self.admission.edges.values_mut() {
            if edge.kind == StreamKind::Dynamic {
                edge.last_stamp = None;
                edge.last_observed_stamp = None;
                edge.window.clear();
            }
        }
        self.counters.resets = self.counters.resets.saturating_add(1);
        Ok(())
    }
    pub fn insert(&mut self, kind: StreamKind, transform: &Transform) -> Result<(), Failure> {
        self.insert_batch(kind, std::slice::from_ref(transform))
    }
    /// Validate the complete array and proposed topology before any insertion.
    /// A rare native failure after successful earlier items disables this owner;
    /// it must be explicitly reset/re-activated before more work is accepted.
    pub fn insert_batch(
        &mut self,
        kind: StreamKind,
        transforms: &[Transform],
    ) -> Result<(), Failure> {
        self.insert_batch_policy(kind, transforms, false)
    }
    /// Live TF sources are temporally thinned before native insertion. Keep the
    /// inclusive cache bound even for sustained high-rate publishers, rather
    /// than admitting a burst then stopping the entire shared scene.
    pub fn insert_live_batch(
        &mut self,
        kind: StreamKind,
        transforms: &[Transform],
    ) -> Result<(), Failure> {
        self.insert_batch_policy(kind, transforms, true)
    }
    fn insert_batch_policy(
        &mut self,
        kind: StreamKind,
        transforms: &[Transform],
        thin: bool,
    ) -> Result<(), Failure> {
        self.require_active()?;
        let planned = (|| {
            if transforms.len() > MAX_BATCH {
                return Err(Failure::new(
                    Status::Capacity,
                    "TF batch exceeds 128 transforms",
                ));
            }
            let mut proposed = self.admission.clone();
            let mut children = HashSet::new();
            let mut normalized = Vec::with_capacity(transforms.len());
            for transform in transforms {
                if !children.insert(transform.child.as_str()) {
                    return Err(Failure::new(Status::Conflict, "Repeated TF child in batch"));
                }
                let mut transform = transform.clone();
                transform.components = validate_components(transform.components)?;
                let previous = proposed.edges.get(&transform.child).cloned();
                let interval = CACHE_WINDOW_NS / (DYNAMIC_INSERTS_PER_WINDOW as i64 - 1) + 1;
                // One insertion per source-time bin. Any inclusive10s interval
                // intersects at most256 bins, without starving a30Hz publisher
                // down to15Hz as a minimum-spacing gate would.
                let same_bin = thin
                    && kind == StreamKind::Dynamic
                    && previous
                        .as_ref()
                        .and_then(|e| e.last_stamp)
                        .is_some_and(|last| {
                            transform.stamp_ns > last
                                && transform.stamp_ns / interval == last / interval
                        });
                // Validate even skipped inputs: a fast publisher cannot bypass
                // topology, quaternion or source-order validation.
                let admitted = admit(&mut proposed, kind, &transform);
                let skip = same_bin
                    || (thin
                        && admitted
                            .as_ref()
                            .is_err_and(|e| e.status == Status::RateLimited));
                let duplicate = match admitted {
                    Ok(duplicate) => duplicate,
                    Err(error) if thin && error.status == Status::RateLimited => false,
                    Err(error) => return Err(error),
                };
                if skip {
                    let mut edge = previous.ok_or_else(|| {
                        Failure::new(Status::Error, "Cannot thin an unadmitted edge")
                    })?;
                    edge.last_observed_stamp = Some(transform.stamp_ns);
                    proposed.edges.insert(transform.child.clone(), edge);
                }
                normalized.push((transform, duplicate, skip));
            }
            Ok(normalized)
        })();
        let planned = match planned {
            Ok(p) => p,
            Err(error) => {
                self.counters.rejected = self.counters.rejected.saturating_add(1);
                return Err(error);
            }
        };
        for (transform, duplicate, skip) in planned {
            if skip {
                if let Some(edge) = self.admission.edges.get_mut(&transform.child) {
                    edge.last_observed_stamp = Some(transform.stamp_ns);
                }
                self.counters.rejected = self.counters.rejected.saturating_add(1);
                continue;
            }
            if !duplicate {
                if let Err(error) = self.native.insert(
                    &transform.parent,
                    &transform.child,
                    transform.stamp_ns,
                    transform.components,
                    kind == StreamKind::Static,
                ) {
                    self.active = false;
                    self.counters.native_errors = self.counters.native_errors.saturating_add(1);
                    return Err(native_failure(error));
                }
            }
            // Commit only native-successful entries, keeping metadata coherent
            // even if a later native insertion reports a failure.
            admit(&mut self.admission, kind, &transform)?;
            if duplicate {
                self.counters.duplicate_statics = self.counters.duplicate_statics.saturating_add(1);
            } else {
                self.counters.accepted = self.counters.accepted.saturating_add(1);
            }
        }
        Ok(())
    }
    /// Exact acquisition-time lookup only. No freshness is inferred from wall
    /// time or receipt age. Native past/future failures remain extrapolation;
    /// Stale is reserved for a native diagnostic that explicitly proves age.
    pub fn lookup_at(
        &mut self,
        target: &str,
        source: &str,
        stamp_ns: i64,
    ) -> Result<Lookup, Failure> {
        self.require_active()?;
        if !valid_frame(target) || !valid_frame(source) || stamp_ns <= 0 || stamp_ns > MAX_STAMP_NS
        {
            return Err(Failure::new(
                Status::Invalid,
                "Lookup requires valid frames and a positive system acquisition timestamp",
            ));
        }
        if !self.admission.frames.contains(target) || !self.admission.frames.contains(source) {
            return Err(Failure::new(
                Status::Missing,
                "Requested TF frame is not admitted",
            ));
        }
        match self.native.lookup(target, source, stamp_ns) {
            Ok(components) => Ok(Lookup {
                target: target.into(),
                source: source.into(),
                stamp_ns,
                components: validate_components(components)?,
            }),
            Err(error) => Err(native_failure(error)),
        }
    }
    /// Robot presentation only: native latest-common-time per requested link.
    /// Different links may resolve at different stamps; zero is meaningful for
    /// static-only/identity results. No receipt-time freshness is inferred.
    pub fn lookup_latest(&mut self, target: &str, source: &str) -> Result<Lookup, Failure> {
        self.require_active()?;
        if !valid_frame(target) || !valid_frame(source) {
            return Err(Failure::new(
                Status::Invalid,
                "Latest lookup requires valid frames",
            ));
        }
        // tf2 defines identity directly, including before a frame is admitted.
        // Other lookups must not allocate arbitrary native frame-name entries.
        if target != source
            && (!self.admission.frames.contains(target) || !self.admission.frames.contains(source))
        {
            return Err(Failure::new(
                Status::Missing,
                "Requested TF frame is not admitted",
            ));
        }
        let (components, stamp_ns) = self
            .native
            .lookup_latest(target, source)
            .map_err(native_failure)?;
        if !(0..=MAX_STAMP_NS).contains(&stamp_ns) {
            return Err(Failure::new(
                Status::Invalid,
                "Native latest stamp outside ROS Time range",
            ));
        }
        Ok(Lookup {
            target: target.into(),
            source: source.into(),
            stamp_ns,
            components: validate_components(components)?,
        })
    }
}
fn valid_frame(frame: &str) -> bool {
    !frame.is_empty()
        && frame.len() <= 256
        && !frame.starts_with('/')
        && !frame.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn validate_components(mut value: [f64; 7]) -> Result<[f64; 7], Failure> {
    if value.iter().any(|v| !v.is_finite()) || value[..3].iter().any(|v| v.abs() > 1_000_000.) {
        return Err(Failure::new(
            Status::Invalid,
            "TF components must be finite and within metre limits",
        ));
    }
    let norm = value[3..].iter().map(|v| v * v).sum::<f64>();
    if !norm.is_finite() || (norm - 1.).abs() > 1e-6 {
        return Err(Failure::new(
            Status::Invalid,
            "TF quaternion must have unit squared norm within 1e-6",
        ));
    }
    for component in &mut value[3..] {
        *component /= norm.sqrt();
    }
    Ok(value)
}
fn admit(admission: &mut Admission, kind: StreamKind, t: &Transform) -> Result<bool, Failure> {
    if !valid_frame(&t.parent)
        || !valid_frame(&t.child)
        || t.parent == t.child
        || t.stamp_ns < 0
        || t.stamp_ns > MAX_STAMP_NS
        || (kind == StreamKind::Dynamic && t.stamp_ns == 0)
    {
        return Err(Failure::new(
            Status::Invalid,
            "Invalid TF frames or source stamp",
        ));
    }
    if let Some(edge) = admission.edges.get(&t.child) {
        if edge.parent != t.parent || edge.kind != kind {
            return Err(Failure::new(
                Status::Conflict,
                "TF reparenting and static/dynamic replacement require a new session",
            ));
        }
        if kind == StreamKind::Static {
            if edge.last_stamp.is_some_and(|stamp| t.stamp_ns < stamp) {
                return Err(Failure::new(Status::OutOfOrder, "Stale static TF stamp"));
            }
            if edge.static_components != Some(t.components) {
                return Err(Failure::new(
                    Status::Conflict,
                    "Conflicting static TF value",
                ));
            }
            admission.edges.get_mut(&t.child).unwrap().last_stamp = Some(t.stamp_ns);
            return Ok(true);
        }
        if edge
            .last_observed_stamp
            .or(edge.last_stamp)
            .is_some_and(|stamp| t.stamp_ns <= stamp)
        {
            return Err(Failure::new(
                Status::OutOfOrder,
                "Dynamic TF timestamps must increase strictly",
            ));
        }
    } else {
        if admission.edges.len() >= MAX_EDGES {
            return Err(Failure::new(Status::Capacity, "TF edge capacity exhausted"));
        }
        let additional = usize::from(!admission.frames.contains(&t.parent))
            + usize::from(!admission.frames.contains(&t.child));
        if admission.frames.len() + additional > MAX_FRAMES {
            return Err(Failure::new(
                Status::Capacity,
                "TF frame capacity exhausted",
            ));
        }
        let mut parent = t.parent.as_str();
        for _ in 0..=MAX_EDGES {
            if parent == t.child {
                return Err(Failure::new(Status::Cyclic, "TF edge would create a cycle"));
            }
            match admission.edges.get(parent) {
                Some(edge) => parent = &edge.parent,
                None => break,
            }
        }
    }
    let edge = admission
        .edges
        .entry(t.child.clone())
        .or_insert_with(|| Edge {
            parent: t.parent.clone(),
            kind,
            last_stamp: None,
            last_observed_stamp: None,
            static_components: None,
            window: VecDeque::new(),
        });
    if kind == StreamKind::Dynamic {
        let cutoff = t.stamp_ns.saturating_sub(CACHE_WINDOW_NS);
        while edge.window.front().is_some_and(|stamp| *stamp < cutoff) {
            edge.window.pop_front();
        }
        if edge.window.len() >= DYNAMIC_INSERTS_PER_WINDOW {
            return Err(Failure::new(
                Status::RateLimited,
                "TF edge exceeds 256 admitted inserts per inclusive 10-second window",
            ));
        }
        edge.window.push_back(t.stamp_ns);
    } else {
        edge.static_components = Some(t.components);
    }
    edge.last_stamp = Some(t.stamp_ns);
    edge.last_observed_stamp = Some(t.stamp_ns);
    admission.frames.insert(t.parent.clone());
    admission.frames.insert(t.child.clone());
    Ok(false)
}
fn native_failure(message: String) -> Failure {
    let lower = message.to_ascii_lowercase();
    let status = if lower.contains("extrapolation into the past")
        || lower.contains("extrapolation into the future")
        // tf2's one-entry cache has a separate explicit extrapolation diagnostic.
        || lower.starts_with("lookup would require extrapolation at time ")
    {
        Status::Extrapolated
    } else if lower.contains("does not exist") || lower.contains("unconnected trees") {
        Status::Missing
    } else if lower.contains("tf_old_data") {
        Status::Stale
    } else if lower.contains("loop in the tf tree") {
        Status::Cyclic
    } else {
        Status::Error
    };
    Failure::new(status, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SECOND: i64 = 1_000_000_000;
    fn value(parent: &str, child: &str, stamp: i64, x: f64) -> Transform {
        Transform {
            parent: parent.into(),
            child: child.into(),
            stamp_ns: stamp,
            components: [x, 0., 0., 0., 0., 0., 1.],
        }
    }
    fn owner() -> SpatialSession {
        let mut owner = SpatialSession::new().unwrap();
        owner
            .activate_explicit(ClockPolicy::VerifiedSystem)
            .unwrap();
        owner
    }
    #[test]
    fn native_composition_interpolation_and_acquisition_stamp_are_exact() {
        let mut tf = owner();
        tf.insert(StreamKind::Static, &value("map", "odom", 0, 10.))
            .unwrap();
        tf.insert(StreamKind::Dynamic, &value("odom", "base", SECOND, 0.))
            .unwrap();
        tf.insert(StreamKind::Dynamic, &value("odom", "base", 3 * SECOND, 4.))
            .unwrap();
        tf.insert(StreamKind::Static, &value("base", "laser", 0, 1.))
            .unwrap();
        let actual = tf.lookup_at("map", "laser", 2 * SECOND).unwrap();
        assert!((actual.components[0] - 13.).abs() < 1e-12);
        assert_eq!(actual.stamp_ns, 2 * SECOND);
        assert!(
            (tf.lookup_at("laser", "map", 2 * SECOND).unwrap().components[0] + 13.).abs() < 1e-12
        );
        assert_eq!(
            tf.lookup_at("map", "laser", 4 * SECOND).unwrap_err().status,
            Status::Extrapolated
        );
        assert_eq!(
            tf.lookup_at("map", "laser", 0).unwrap_err().status,
            Status::Invalid
        );
    }
    #[test]
    fn static_rotation_and_late_join_survive_explicit_dynamic_reset() {
        let mut tf = owner();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let mut rotate = value("map", "base", 0, 0.);
        rotate.components = [0., 0., 0., 0., 0., q, q];
        tf.insert_batch(StreamKind::Static, &[rotate, value("base", "laser", 0, 2.)])
            .unwrap();
        let actual = tf.lookup_at("map", "laser", 123 * SECOND).unwrap();
        assert!(actual.components[0].abs() < 1e-12);
        assert!((actual.components[1] - 2.).abs() < 1e-12);
        tf.insert(StreamKind::Dynamic, &value("laser", "tip", SECOND, 1.))
            .unwrap();
        tf.reset().unwrap();
        assert_eq!(
            tf.lookup_at("map", "laser", SECOND).unwrap_err().status,
            Status::Inactive
        );
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        assert!(tf.lookup_at("map", "laser", SECOND).is_ok());
        assert!(tf.lookup_at("map", "tip", SECOND).is_err());
        tf.insert(StreamKind::Dynamic, &value("laser", "tip", SECOND / 2, 1.))
            .unwrap();
        assert!(tf.lookup_at("map", "tip", SECOND / 2).is_ok());
    }
    #[test]
    fn cycles_reparenting_kind_replacement_and_time_reversal_are_rejected() {
        let mut tf = owner();
        tf.insert(StreamKind::Dynamic, &value("a", "b", SECOND, 0.))
            .unwrap();
        tf.insert(StreamKind::Static, &value("b", "c", 0, 0.))
            .unwrap();
        for (kind, t, status) in [
            (StreamKind::Static, value("c", "a", 0, 0.), Status::Cyclic),
            (
                StreamKind::Dynamic,
                value("x", "b", 2 * SECOND, 0.),
                Status::Conflict,
            ),
            (StreamKind::Static, value("a", "b", 0, 0.), Status::Conflict),
            (
                StreamKind::Dynamic,
                value("a", "b", SECOND, 0.),
                Status::OutOfOrder,
            ),
            (
                StreamKind::Dynamic,
                value("a", "b", SECOND - 1, 0.),
                Status::OutOfOrder,
            ),
        ] {
            assert_eq!(tf.insert(kind, &t).unwrap_err().status, status);
        }
        assert_eq!(tf.edge_count(), 2);
        assert_eq!(tf.counters().rejected, 5);
    }
    #[test]
    fn static_conflict_stale_and_whole_batch_validation_never_silently_replace() {
        let mut tf = owner();
        tf.insert(StreamKind::Static, &value("map", "laser", SECOND, 1.))
            .unwrap();
        tf.insert(StreamKind::Static, &value("map", "laser", SECOND, 1.))
            .unwrap();
        assert_eq!(tf.counters().duplicate_statics, 1);
        assert_eq!(
            tf.insert(StreamKind::Static, &value("map", "laser", 0, 1.))
                .unwrap_err()
                .status,
            Status::OutOfOrder
        );
        assert_eq!(
            tf.insert(StreamKind::Static, &value("map", "laser", 2 * SECOND, 2.))
                .unwrap_err()
                .status,
            Status::Conflict
        );
        let mut bad = value("map", "bad", 0, 0.);
        bad.components[6] = 0.;
        assert!(tf
            .insert_batch(StreamKind::Static, &[value("map", "good", 0, 0.), bad])
            .is_err());
        assert_eq!(
            tf.lookup_at("map", "good", SECOND).unwrap_err().status,
            Status::Missing
        );
        assert_eq!(
            tf.lookup_at("map", "laser", SECOND).unwrap().components[0],
            1.
        );
    }
    #[test]
    fn live_high_rate_tf_is_bounded_continuous_and_interpolates() {
        for hz in [30, 50, 100] {
            let mut tf = owner();
            for n in 0..(hz * 40) {
                let stamp = SECOND + (i64::from(n) * SECOND / i64::from(hz));
                tf.insert_live_batch(
                    StreamKind::Dynamic,
                    &[value("map", "base", stamp, stamp as f64 / SECOND as f64)],
                )
                .unwrap();
                assert!(tf.is_active());
                assert!(tf.admission.edges["base"].window.len() <= DYNAMIC_INSERTS_PER_WINDOW);
            }
            assert!(tf.counters().accepted > 500);
            assert!(tf.counters().rejected > 0);
            let stamp = tf.admission.edges["base"].last_stamp.unwrap() - 10_000_000;
            let resolved = tf.lookup_at("map", "base", stamp).unwrap();
            assert!((resolved.components[0] - stamp as f64 / SECOND as f64).abs() < 1e-5);
        }
    }
    #[test]
    fn thinning_preserves_atomic_validation_and_rejects_out_of_order_skips() {
        let mut tf = owner();
        tf.insert_live_batch(StreamKind::Dynamic, &[value("map", "base", SECOND, 0.)])
            .unwrap();
        let mut invalid = value("map", "invalid", SECOND + 1, 0.);
        invalid.components[6] = 0.;
        assert!(tf
            .insert_live_batch(
                StreamKind::Dynamic,
                &[value("map", "base", SECOND + 1, 0.), invalid]
            )
            .is_err());
        assert_eq!(tf.counters().accepted, 1);
        assert_eq!(tf.admission.edges["base"].last_observed_stamp, Some(SECOND));
        tf.insert_live_batch(
            StreamKind::Dynamic,
            &[value("map", "base", SECOND + 100, 0.)],
        )
        .unwrap();
        assert_eq!(
            tf.insert_live_batch(
                StreamKind::Dynamic,
                &[value("map", "base", SECOND + 50, 0.)]
            )
            .unwrap_err()
            .status,
            Status::OutOfOrder
        );
        assert!(tf.is_active());
    }
    #[test]
    fn rate_window_caps_native_admission_and_reset_cannot_grow_frame_names() {
        let mut tf = owner();
        for stamp in 1..=256 {
            tf.insert(StreamKind::Dynamic, &value("map", "base", stamp, 0.))
                .unwrap();
        }
        assert_eq!(
            tf.insert(StreamKind::Dynamic, &value("map", "base", 257, 0.))
                .unwrap_err()
                .status,
            Status::RateLimited
        );
        assert_eq!(
            tf.insert(
                StreamKind::Dynamic,
                &value("map", "base", CACHE_WINDOW_NS + 1, 0.)
            )
            .unwrap_err()
            .status,
            Status::RateLimited,
            "The oldest source stamp remains inside the inclusive window"
        );
        assert!(tf
            .insert(
                StreamKind::Dynamic,
                &value("map", "base", CACHE_WINDOW_NS + 257, 0.)
            )
            .is_ok());
        assert_eq!(tf.admission.edges["base"].window.len(), 1);
        tf.reset().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        assert_eq!(
            tf.insert(StreamKind::Dynamic, &value("different", "base", SECOND, 0.))
                .unwrap_err()
                .status,
            Status::Conflict
        );
        assert_eq!(tf.frame_count(), 2);
    }
    #[test]
    fn exact_edge_frame_cap_and_bounded_batches_are_enforced() {
        let mut tf = owner();
        let transforms: Vec<_> = (0..MAX_EDGES)
            .map(|i| value(&format!("parent_{i}"), &format!("child_{i}"), 0, 0.))
            .collect();
        tf.insert_batch(StreamKind::Static, &transforms).unwrap();
        assert_eq!(tf.edge_count(), MAX_EDGES);
        assert_eq!(tf.frame_count(), MAX_FRAMES);
        assert_eq!(
            tf.insert(
                StreamKind::Static,
                &value("extra_parent", "extra_child", 0, 0.)
            )
            .unwrap_err()
            .status,
            Status::Capacity
        );
        tf.reset().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        assert_eq!(tf.frame_count(), MAX_FRAMES);
        assert!(tf.lookup_at("parent_127", "child_127", SECOND).is_ok());
    }
    #[test]
    fn system_clock_and_valid_components_are_required_and_unknown_native_errors_stay_generic() {
        let mut tf = SpatialSession::new().unwrap();
        assert_eq!(
            tf.activate_explicit(ClockPolicy::Unverified)
                .unwrap_err()
                .status,
            Status::ClockUnverified
        );
        assert_eq!(
            tf.activate_explicit(ClockPolicy::Simulated)
                .unwrap_err()
                .status,
            Status::UnsupportedClock
        );
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        for components in [
            [0.; 7],
            [0., 0., 0., 0., 0., 0., 2.],
            [f64::NAN, 0., 0., 0., 0., 0., 1.],
            [1_000_001., 0., 0., 0., 0., 0., 1.],
        ] {
            let mut t = value("map", "base", 0, 0.);
            t.components = components;
            assert_eq!(
                tf.insert(StreamKind::Static, &t).unwrap_err().status,
                Status::Invalid
            );
        }
        assert_eq!(
            native_failure("unexpected vendor diagnostic".into()).status,
            Status::Error
        );
        assert!(native_failure("x".repeat(1000)).message.len() <= 512);
    }
}
