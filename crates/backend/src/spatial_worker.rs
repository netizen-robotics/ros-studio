//! One worker-local tf2 owner and three native readers; no UI or ROS executor.
use crate::spatial_session::{SpatialSession, Status};
#[cfg(test)]
use crate::{
    observation::{Factory, Reader},
    spatial_session::{ClockPolicy, StreamKind, Transform},
};
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
#[cfg(test)]
use studio_protocol::spatial_input::TfStreamKind;
use studio_protocol::{
    spatial::{self, Counters, TfOutcome},
    spatial_input::{self, SpatialClockPolicy},
    structured,
    subscriptions::{Configuration, Output},
};

struct Pending {
    scan: structured::Sample,
    source: String,
    stamp: i64,
    began: Instant,
    attempt_at: Option<Instant>,
    previous: Option<TfOutcome>,
}
#[cfg(test)]
pub(crate) struct SpatialReader {
    inputs: Vec<(Configuration, Box<dyn Reader>)>,
    fixed_frame: String,
    tf: SpatialSession,
    pending: Option<Pending>,
    last_scan_stamp: Option<i64>,
    counters: Counters,
}
#[cfg(test)]
impl SpatialReader {
    pub(crate) fn open(
        configuration: &Configuration,
        factory: &Factory,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        if !configuration.valid() || configuration.output != Output::SpatialScan {
            return Err("Invalid Spatial group configuration".into());
        }
        let s = configuration
            .spatial
            .as_ref()
            .ok_or("Missing Spatial configuration")?;
        let mut tf = SpatialSession::new().map_err(|e| e.message)?;
        tf.activate_explicit(ClockPolicy::VerifiedSystem)
            .map_err(|e| e.message)?;
        let topic = |name: &str, ty: &str, best_effort, transient_local| Configuration {
            map_updates: None,
            topic: name.into(),
            message_type: ty.into(),
            best_effort,
            transient_local,
            output: Output::Typed,
            spatial: None,
            robot: None,
        };
        // Retained statics are polled first; no assumption of delivery is made.
        let configurations = [
            topic(
                &s.static_tf_topic,
                spatial_input::TF_MESSAGE_TYPE,
                false,
                true,
            ),
            topic(
                &s.dynamic_tf_topic,
                spatial_input::TF_MESSAGE_TYPE,
                s.dynamic_tf_best_effort,
                false,
            ),
            topic(
                &configuration.topic,
                spatial_input::SCAN_MESSAGE_TYPE,
                configuration.best_effort,
                configuration.transient_local,
            ),
        ];
        let mut inputs = Vec::with_capacity(3);
        for c in configurations {
            if cancel.load(Ordering::Acquire) {
                return Err("Spatial group start canceled".into());
            }
            let reader = factory(&c)?;
            inputs.push((c, reader));
        }
        Ok(Self {
            inputs,
            fixed_frame: s.fixed_frame.clone(),
            tf,
            pending: None,
            last_scan_stamp: None,
            counters: Counters::default(),
        })
    }
    fn input(
        &mut self,
        index: usize,
        value: structured::Value,
        now: Instant,
    ) -> Result<(), String> {
        let configuration = &self.inputs[index].0;
        let sample = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: configuration.topic.clone(),
            message_type: configuration.message_type.clone(),
            source_time: None,
            value,
        };
        if index == 2 {
            self.counters.scans_received = self.counters.scans_received.saturating_add(1);
            let scan =
                spatial_input::decode_laser_scan(&sample, SpatialClockPolicy::VerifiedSystem)
                    .map_err(|e| e.to_string())?;
            if self
                .last_scan_stamp
                .is_some_and(|last| scan.stamp_ns <= last)
            {
                return Err(
                    "Out-of-order scan acquisition timestamp; explicit stop/restart required"
                        .into(),
                );
            }
            self.last_scan_stamp = Some(scan.stamp_ns);
            self.counters.pending_replaced = self
                .counters
                .pending_replaced
                .saturating_add(u64::from(self.pending.is_some()));
            self.pending = Some(Pending {
                scan: sample,
                source: scan.frame,
                stamp: scan.stamp_ns,
                began: now,
                attempt_at: None,
                previous: None,
            });
        } else {
            let kind = if index == 0 {
                TfStreamKind::Static
            } else {
                TfStreamKind::Dynamic
            };
            if index == 0 {
                self.counters.tf_static_batches = self.counters.tf_static_batches.saturating_add(1);
            } else {
                self.counters.tf_dynamic_batches =
                    self.counters.tf_dynamic_batches.saturating_add(1);
            }
            let batch =
                spatial_input::decode_tf_message(&sample, kind, SpatialClockPolicy::VerifiedSystem)
                    .map_err(|e| {
                        self.counters.tf_rejected = self.counters.tf_rejected.saturating_add(1);
                        e.to_string()
                    })?;
            let transforms: Vec<_> = batch
                .transforms
                .into_iter()
                .map(|t| Transform {
                    parent: t.parent,
                    child: t.child,
                    stamp_ns: t.stamp_ns,
                    components: t.components,
                })
                .collect();
            let result = self.tf.insert_batch(
                if index == 0 {
                    StreamKind::Static
                } else {
                    StreamKind::Dynamic
                },
                &transforms,
            );
            let counters = self.tf.counters();
            self.counters.tf_accepted = counters.accepted;
            self.counters.tf_rejected = counters.rejected;
            self.counters.duplicate_statics = counters.duplicate_statics;
            self.counters.native_errors = counters.native_errors;
            result.map_err(|e| format!("Spatial TF {:?}: {}", e.status, e.message))?;
        }
        Ok(())
    }
    fn retry(&mut self, now: Instant) -> Result<Option<spatial::Sample>, String> {
        let Some(pending) = &mut self.pending else {
            return Ok(None);
        };
        let expired = now.saturating_duration_since(pending.began)
            >= Duration::from_millis(spatial::PENDING_TIMEOUT_MS);
        if !expired
            && pending.attempt_at.is_some_and(|last| {
                now.saturating_duration_since(last)
                    < Duration::from_millis(spatial::RETRY_INTERVAL_MS)
            })
        {
            return Ok(None);
        }
        pending.attempt_at = Some(now);
        let outcome = if expired {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
            TfOutcome::LookupTimeout
        } else {
            match self
                .tf
                .lookup_at(&self.fixed_frame, &pending.source, pending.stamp)
            {
                Ok(result) => TfOutcome::Ready {
                    components: result.components,
                },
                Err(error) => match error.status {
                    Status::Missing => TfOutcome::Missing,
                    Status::Stale => TfOutcome::Stale,
                    Status::Extrapolated => TfOutcome::Extrapolated,
                    _ => {
                        return Err(format!(
                            "Spatial TF lookup {:?}: {}",
                            error.status, error.message
                        ))
                    }
                },
            }
        };
        let ready = matches!(outcome, TfOutcome::Ready { .. });
        if pending.previous.as_ref() == Some(&outcome) {
            return Ok(None);
        }
        pending.previous = Some(outcome.clone());
        let sample = spatial::Sample {
            contract_version: spatial::CONTRACT_VERSION,
            scan: Box::new(pending.scan.clone()),
            fixed_frame: self.fixed_frame.clone(),
            source_frame: pending.source.clone(),
            acquisition_stamp_ns: pending.stamp,
            tf: outcome,
            counters: self.counters,
        };
        if ready || expired {
            self.pending = None;
        }
        Ok(Some(sample))
    }
}
#[cfg(test)]
impl Reader for SpatialReader {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Err("Spatial group requires identified Spatial output".into())
    }
    fn poll_spatial(&mut self) -> Result<Option<spatial::Sample>, String> {
        let now = Instant::now();
        for index in 0..3 {
            let result = self.inputs[index].1.poll_now().inspect_err(|_| {
                self.counters.native_errors = self.counters.native_errors.saturating_add(1);
            })?;
            if let Some(value) = result {
                self.input(index, value, now)?;
            }
        }
        self.retry(now)
    }
    fn spatial_counters(&self) -> Option<Counters> {
        Some(self.counters)
    }
}

/// Acquisition-time state for one scan served by the shared scene TF owner.
/// It owns no TF readers/cache and never resets a peer's graph.
pub(crate) struct ScanState {
    configuration: Configuration,
    pending: Option<Pending>,
    last_stamp: Option<i64>,
    continuity: crate::display_continuity::DisplayContinuity,
    counters: Counters,
}
impl ScanState {
    pub(crate) fn new(configuration: &Configuration) -> Result<Self, String> {
        if configuration.output != Output::SpatialScan || !configuration.valid() {
            return Err("Invalid shared scan configuration".into());
        }
        Ok(Self {
            configuration: configuration.clone(),
            pending: None,
            last_stamp: None,
            continuity: Default::default(),
            counters: Counters::default(),
        })
    }
    pub(crate) fn input(&mut self, value: structured::Value, now: Instant) -> Result<(), String> {
        let scan = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: self.configuration.topic.clone(),
            message_type: self.configuration.message_type.clone(),
            source_time: None,
            value,
        };
        self.counters.scans_received = self.counters.scans_received.saturating_add(1);
        let decoded = spatial_input::decode_laser_scan(&scan, SpatialClockPolicy::VerifiedSystem)
            .map_err(|e| e.to_string())?;
        if self.last_stamp.is_some_and(|last| decoded.stamp_ns <= last) {
            return Err(
                "Out-of-order scan acquisition timestamp; explicit stop/restart required".into(),
            );
        }
        self.continuity.source(&decoded.frame);
        self.last_stamp = Some(decoded.stamp_ns);
        self.counters.pending_replaced = self
            .counters
            .pending_replaced
            .saturating_add(u64::from(self.pending.is_some()));
        self.pending = Some(Pending {
            scan,
            source: decoded.frame,
            stamp: decoded.stamp_ns,
            began: now,
            attempt_at: None,
            previous: None,
        });
        Ok(())
    }
    pub(crate) fn counters(&self, shared: &Counters) -> Counters {
        Counters {
            scans_received: self.counters.scans_received,
            pending_replaced: self.counters.pending_replaced,
            lookup_timeouts: self.counters.lookup_timeouts,
            ..*shared
        }
    }
    pub(crate) fn poll(
        &mut self,
        tf: &mut SpatialSession,
        now: Instant,
        shared: &Counters,
    ) -> Result<Option<spatial::Sample>, String> {
        let Some(pending) = &mut self.pending else {
            return Ok(None);
        };
        let expired = now.saturating_duration_since(pending.began)
            >= Duration::from_millis(spatial::PENDING_TIMEOUT_MS);
        if !expired
            && pending.attempt_at.is_some_and(|last| {
                now.saturating_duration_since(last)
                    < Duration::from_millis(spatial::RETRY_INTERVAL_MS)
            })
        {
            return Ok(None);
        }
        pending.attempt_at = Some(now);
        let fixed = &self
            .configuration
            .spatial
            .as_ref()
            .ok_or("Missing scan TF configuration")?
            .fixed_frame;
        let outcome = if expired {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
            TfOutcome::LookupTimeout
        } else {
            match tf.lookup_at(fixed, &pending.source, pending.stamp) {
                Ok(result) => TfOutcome::Ready {
                    components: result.components,
                },
                Err(error) => match error.status {
                    Status::Missing => TfOutcome::Missing,
                    Status::Stale => TfOutcome::Stale,
                    Status::Extrapolated => TfOutcome::Extrapolated,
                    _ => {
                        return Err(format!(
                            "Shared scan TF lookup {:?}: {}",
                            error.status, error.message
                        ))
                    }
                },
            }
        };
        let was_timeout = matches!(outcome, TfOutcome::LookupTimeout);
        let Some(outcome) = self.continuity.publication(outcome, now) else {
            return Ok(None);
        };
        if !was_timeout && matches!(outcome, TfOutcome::LookupTimeout) {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
        }
        if pending.previous.as_ref() == Some(&outcome) {
            return Ok(None);
        }
        pending.previous = Some(outcome.clone());
        let sample = spatial::Sample {
            contract_version: spatial::CONTRACT_VERSION,
            scan: Box::new(pending.scan.clone()),
            fixed_frame: fixed.clone(),
            source_frame: pending.source.clone(),
            acquisition_stamp_ns: pending.stamp,
            tf: outcome.clone(),
            counters: Counters {
                scans_received: self.counters.scans_received,
                pending_replaced: self.counters.pending_replaced,
                lookup_timeouts: self.counters.lookup_timeouts,
                ..*shared
            },
        };
        if expired || matches!(outcome, TfOutcome::Ready { .. }) {
            self.pending = None;
        }
        Ok(Some(sample))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{rc::Rc, sync::atomic::AtomicUsize, thread};
    use structured::{Field, FloatValue, Value};
    pub(super) fn number(v: f64) -> Value {
        Value::Float {
            value: FloatValue::Finite(v.to_string()),
        }
    }
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
    fn text(value: &str) -> Value {
        Value::Text {
            value: value.into(),
            original_bytes: value.len(),
        }
    }
    fn header(frame: &str, second: i64) -> Value {
        object(vec![
            ("frame_id", text(frame)),
            (
                "stamp",
                object(vec![
                    (
                        "sec",
                        Value::Signed {
                            value: second.to_string(),
                        },
                    ),
                    ("nanosec", Value::Unsigned { value: "0".into() }),
                ]),
            ),
        ])
    }
    pub(crate) fn scan(second: i64) -> Value {
        object(vec![
            ("header", header("laser", second)),
            ("angle_min", number(0.)),
            ("angle_max", number(1.)),
            ("angle_increment", number(1.)),
            ("time_increment", number(0.)),
            ("scan_time", number(0.1)),
            ("range_min", number(0.1)),
            ("range_max", number(10.)),
            (
                "ranges",
                Value::Array {
                    items: vec![number(1.), number(2.)],
                    original_items: 2,
                },
            ),
            (
                "intensities",
                Value::Array {
                    items: vec![],
                    original_items: 0,
                },
            ),
        ])
    }
    pub(crate) fn tf(parent: &str, child: &str, second: i64, x: f64) -> Value {
        object(vec![(
            "transforms",
            Value::Array {
                original_items: 1,
                items: vec![object(vec![
                    ("header", header(parent, second)),
                    ("child_frame_id", text(child)),
                    (
                        "transform",
                        object(vec![
                            (
                                "translation",
                                object(vec![
                                    ("x", number(x)),
                                    ("y", number(0.)),
                                    ("z", number(0.)),
                                ]),
                            ),
                            (
                                "rotation",
                                object(vec![
                                    ("x", number(0.)),
                                    ("y", number(0.)),
                                    ("z", number(0.)),
                                    ("w", number(1.)),
                                ]),
                            ),
                        ]),
                    ),
                ])],
            },
        )])
    }
    pub(crate) fn configuration() -> Configuration {
        Configuration {
            map_updates: None,
            topic: "/scan".into(),
            message_type: spatial_input::SCAN_MESSAGE_TYPE.into(),
            best_effort: true,
            transient_local: false,
            output: Output::SpatialScan,
            spatial: Some(spatial::Configuration {
                fixed_frame: "map".into(),
                clock_policy: SpatialClockPolicy::VerifiedSystem,
                dynamic_tf_topic: "/tf".into(),
                dynamic_tf_best_effort: false,
                static_tf_topic: "/tf_static".into(),
            }),
            robot: None,
        }
    }
    #[test]
    fn next_scan_tf_retry_keeps_completed_frame_but_outage_has_a_fixed_deadline() {
        let mut tf = SpatialSession::new().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        let transform = |second: i64| Transform {
            parent: "map".into(),
            child: "laser".into(),
            stamp_ns: second * 1_000_000_000,
            components: [0., 0., 0., 0., 0., 0., 1.],
        };
        tf.insert_live_batch(StreamKind::Dynamic, &[transform(2)])
            .unwrap();
        let now = Instant::now();
        let shared = Counters::default();
        let mut state = ScanState::new(&configuration()).unwrap();
        state.input(scan(2), now).unwrap();
        assert!(matches!(
            state.poll(&mut tf, now, &shared).unwrap().unwrap().tf,
            TfOutcome::Ready { .. }
        ));
        state
            .input(scan(3), now + Duration::from_millis(100))
            .unwrap();
        assert!(state
            .poll(&mut tf, now + Duration::from_millis(100), &shared)
            .unwrap()
            .is_none());
        tf.insert_live_batch(StreamKind::Dynamic, &[transform(3)])
            .unwrap();
        assert!(matches!(
            state
                .poll(&mut tf, now + Duration::from_millis(150), &shared)
                .unwrap()
                .unwrap()
                .tf,
            TfOutcome::Ready { .. }
        ));
        let mut invalidated = false;
        for n in 4..20 {
            let at = now + Duration::from_millis((n - 2) * 100);
            state.input(scan(n as i64), at).unwrap();
            if let Some(sample) = state.poll(&mut tf, at, &shared).unwrap() {
                assert!(
                    at.saturating_duration_since(now + Duration::from_millis(150))
                        >= Duration::from_millis(spatial::PENDING_TIMEOUT_MS)
                );
                assert!(!matches!(sample.tf, TfOutcome::Ready { .. }));
                invalidated = true;
                break;
            }
        }
        assert!(
            invalidated,
            "Replacing pending scans must not extend valid-frame retention forever"
        );
    }
    struct Empty {
        owner: thread::ThreadId,
        drops: Arc<AtomicUsize>,
        _local: Rc<()>,
    }
    impl Reader for Empty {
        fn poll(&mut self) -> Result<Option<Value>, String> {
            assert_eq!(self.owner, thread::current().id());
            Ok(None)
        }
    }
    impl Drop for Empty {
        fn drop(&mut self) {
            assert_eq!(self.owner, thread::current().id());
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn factory(drops: Arc<AtomicUsize>) -> Factory {
        Arc::new(move |_| {
            Ok(Box::new(Empty {
                owner: thread::current().id(),
                drops: drops.clone(),
                _local: Rc::new(()),
            }))
        })
    }
    fn owner() -> SpatialReader {
        SpatialReader::open(
            &configuration(),
            &factory(Arc::new(AtomicUsize::new(0))),
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    }

    #[test]
    fn acquisition_lookup_retries_missing_then_composes_interpolated_dynamic_and_retained_static() {
        let mut worker = owner();
        let now = Instant::now();
        worker.input(2, scan(2), now).unwrap();
        assert!(matches!(
            worker.retry(now).unwrap().unwrap().tf,
            TfOutcome::Missing
        ));
        worker.input(0, tf("base", "laser", 0, 1.), now).unwrap();
        worker.input(1, tf("map", "base", 1, 1.), now).unwrap();
        worker.input(1, tf("map", "base", 3, 3.), now).unwrap();
        assert!(worker
            .retry(now + Duration::from_millis(49))
            .unwrap()
            .is_none());
        let sample = worker
            .retry(now + Duration::from_millis(50))
            .unwrap()
            .unwrap();
        sample.validate().unwrap();
        assert_eq!(sample.acquisition_stamp_ns, 2_000_000_000);
        assert!(matches!(sample.tf,TfOutcome::Ready{components} if (components[0]-3.).abs()<1e-12));
        assert!(worker.pending.is_none());
        assert_eq!(sample.counters.tf_accepted, 3);
    }
    #[test]
    fn one_pending_replacement_and_timeout_never_use_latest_transform() {
        let mut worker = owner();
        let now = Instant::now();
        worker.input(0, tf("base", "laser", 0, 1.), now).unwrap();
        worker.input(1, tf("map", "base", 1, 1.), now).unwrap();
        worker.input(2, scan(2), now).unwrap();
        assert!(matches!(
            worker.retry(now).unwrap().unwrap().tf,
            TfOutcome::Extrapolated
        ));
        worker
            .input(2, scan(3), now + Duration::from_millis(10))
            .unwrap();
        assert_eq!(worker.pending.as_ref().unwrap().stamp, 3_000_000_000);
        assert!(matches!(
            worker
                .retry(now + Duration::from_millis(10))
                .unwrap()
                .unwrap()
                .tf,
            TfOutcome::Extrapolated
        ));
        let timeout = worker
            .retry(now + Duration::from_millis(1010))
            .unwrap()
            .unwrap();
        assert!(matches!(timeout.tf, TfOutcome::LookupTimeout));
        assert_eq!(
            (
                timeout.counters.scans_received,
                timeout.counters.pending_replaced,
                timeout.counters.lookup_timeouts
            ),
            (2, 1, 1)
        );
        assert!(worker.pending.is_none());
    }
    #[test]
    fn tf_arrival_after_pending_deadline_cannot_resurrect_an_expired_scan() {
        let mut worker = owner();
        let now = Instant::now();
        worker.input(2, scan(2), now).unwrap();
        assert!(matches!(
            worker.retry(now).unwrap().unwrap().tf,
            TfOutcome::Missing
        ));
        worker
            .input(
                0,
                tf("map", "laser", 0, 1.),
                now + Duration::from_millis(1001),
            )
            .unwrap();
        let event = worker
            .retry(now + Duration::from_millis(1001))
            .unwrap()
            .unwrap();
        assert!(matches!(event.tf, TfOutcome::LookupTimeout));
        assert!(worker.pending.is_none());
    }
    #[test]
    fn out_of_order_scan_and_conflicting_graph_fail_without_fallback() {
        let mut worker = owner();
        let now = Instant::now();
        worker.input(2, scan(2), now).unwrap();
        assert!(worker
            .input(2, scan(1), now)
            .unwrap_err()
            .contains("Out-of-order"));
        worker.input(0, tf("map", "laser", 0, 1.), now).unwrap();
        assert!(worker
            .input(1, tf("other", "laser", 1, 2.), now)
            .unwrap_err()
            .contains("Conflict"));
        assert_eq!(worker.counters.tf_rejected, 1);
    }
    #[test]
    fn partial_open_and_cancellation_drop_every_native_owner_on_the_creating_thread() {
        for fail_at in 1..=3 {
            let opens = Arc::new(AtomicUsize::new(0));
            let drops = Arc::new(AtomicUsize::new(0));
            let (o, d) = (opens.clone(), drops.clone());
            let factory: Factory = Arc::new(move |c| {
                let n = o.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    assert!(c.transient_local && !c.best_effort);
                    assert_eq!(c.topic, "/tf_static");
                }
                if n == fail_at {
                    return Err("controlled open failure".into());
                }
                Ok(Box::new(Empty {
                    owner: thread::current().id(),
                    drops: d.clone(),
                    _local: Rc::new(()),
                }))
            });
            assert!(SpatialReader::open(
                &configuration(),
                &factory,
                &Arc::new(AtomicBool::new(false))
            )
            .is_err());
            assert_eq!(drops.load(Ordering::SeqCst), fail_at - 1);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let (c, d) = (cancel.clone(), drops.clone());
        let factory: Factory = Arc::new(move |_| {
            c.store(true, Ordering::Release);
            Ok(Box::new(Empty {
                owner: thread::current().id(),
                drops: d.clone(),
                _local: Rc::new(()),
            }))
        });
        assert!(SpatialReader::open(&configuration(), &factory, &cancel).is_err());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
