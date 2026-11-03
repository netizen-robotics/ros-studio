//! Two worker-local TF readers and one standard native tf2 owner. No URDF,
//! render state, receipt-time transforms or reconnect replay live here.
use crate::spatial_session::{SpatialSession, Status};
#[cfg(test)]
use crate::{
    observation::{Factory, Reader},
    spatial_session::{ClockPolicy, StreamKind, Transform},
};
#[cfg(test)]
use std::sync::Arc;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use studio_protocol::{
    robot::{self, LinkPose, LinkResult},
    subscriptions::{Configuration, Output},
};
#[cfg(test)]
use studio_protocol::{
    spatial_input::{self, SpatialClockPolicy, TfStreamKind},
    structured,
};

#[cfg(test)]
pub(crate) struct RobotReader {
    inputs: Vec<(Configuration, Box<dyn Reader>)>,
    configuration: robot::Configuration,
    tf: SpatialSession,
    cancel: Arc<AtomicBool>,
    next_batch: Option<Instant>,
    counters: robot::Counters,
}
#[cfg(test)]
impl RobotReader {
    pub(crate) fn open(
        configuration: &Configuration,
        factory: &Factory,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Self, String> {
        if !configuration.valid() || configuration.output != Output::Robot {
            return Err("Invalid Robot group configuration".into());
        }
        let robot = configuration
            .robot
            .as_ref()
            .ok_or("Missing Robot configuration")?
            .clone();
        robot.validate()?;
        let mut tf = SpatialSession::new().map_err(|e| e.message)?;
        tf.activate_explicit(ClockPolicy::VerifiedSystem)
            .map_err(|e| e.message)?;
        let topic = |name: &str, best_effort, transient_local| Configuration {
            map_updates: None,
            topic: name.into(),
            message_type: spatial_input::TF_MESSAGE_TYPE.into(),
            best_effort,
            transient_local,
            output: Output::Typed,
            spatial: None,
            robot: None,
        };
        let configurations = [
            topic(&robot.static_tf_topic, false, true),
            topic(&robot.dynamic_tf_topic, robot.dynamic_tf_best_effort, false),
        ];
        let mut inputs = Vec::with_capacity(2);
        for input in configurations {
            if cancel.load(Ordering::Acquire) {
                return Err("Robot group start canceled".into());
            }
            let reader = factory(&input)?;
            inputs.push((input, reader));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Robot group start canceled".into());
        }
        Ok(Self {
            inputs,
            configuration: *robot,
            tf,
            cancel: cancel.clone(),
            next_batch: None,
            counters: robot::Counters::default(),
        })
    }
    fn input(&mut self, index: usize, value: structured::Value) -> Result<(), String> {
        let configuration = &self.inputs[index].0;
        let sample = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: configuration.topic.clone(),
            message_type: configuration.message_type.clone(),
            source_time: None,
            value,
        };
        let kind = if index == 0 {
            self.counters.tf_static_batches = self.counters.tf_static_batches.saturating_add(1);
            TfStreamKind::Static
        } else {
            self.counters.tf_dynamic_batches = self.counters.tf_dynamic_batches.saturating_add(1);
            TfStreamKind::Dynamic
        };
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
        result.map_err(|e| format!("Robot TF {:?}: {}", e.status, e.message))
    }
    #[cfg(test)]
    fn poll_at(&mut self, now: Instant) -> Result<Option<robot::Sample>, String> {
        self.poll_with_clock(|| now)
    }
    fn poll_with_clock(
        &mut self,
        clock: impl Fn() -> Instant,
    ) -> Result<Option<robot::Sample>, String> {
        let now = clock();
        // Drain at most one bounded message per input per poll, even between
        // display ticks. Static delivery is always considered before dynamic.
        for index in 0..2 {
            if self.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let value = self.inputs[index].1.poll_now().inspect_err(|_| {
                self.counters.native_errors = self.counters.native_errors.saturating_add(1);
            })?;
            if let Some(value) = value {
                self.input(index, value)?;
            }
        }
        if self.next_batch.is_some_and(|next| now < next) {
            return Ok(None);
        }
        let mut links = Vec::with_capacity(self.configuration.frames.len());
        for frame in &self.configuration.frames {
            if self.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let pose = match self
                .tf
                .lookup_latest(&self.configuration.fixed_frame, frame)
            {
                Ok(lookup) => LinkPose::Ready {
                    resolved_stamp_ns: lookup.stamp_ns,
                    components: lookup.components,
                },
                Err(error) => match error.status {
                    Status::Missing => LinkPose::Missing,
                    Status::Stale => LinkPose::Stale,
                    Status::Extrapolated => LinkPose::Extrapolated,
                    _ => {
                        return Err(format!(
                            "Robot link {frame}: {:?}: {}",
                            error.status, error.message
                        ))
                    }
                },
            };
            links.push(LinkResult {
                frame: frame.clone(),
                pose,
            });
        }
        if self.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.counters.pose_batches = self.counters.pose_batches.saturating_add(1);
        let sample = robot::Sample {
            contract_version: robot::CONTRACT_VERSION,
            fixed_frame: self.configuration.fixed_frame.clone(),
            policy: robot::PosePolicy::LatestAvailable,
            links,
            counters: self.counters,
        };
        sample.validate(&self.configuration)?;
        // Schedule from completion, not the pre-poll instant: slow bounded
        // reads/lookups must not permit the next batch to follow immediately.
        self.next_batch = Some(clock() + Duration::from_millis(1000 / u64::from(robot::MAX_HZ)));
        Ok(Some(sample))
    }
}
#[cfg(test)]
impl Reader for RobotReader {
    fn poll(&mut self) -> Result<Option<structured::Value>, String> {
        Err("Robot group requires identified Robot output".into())
    }
    fn poll_robot(&mut self) -> Result<Option<robot::Sample>, String> {
        self.poll_with_clock(Instant::now)
    }
}

/// Robot tick state backed by the scene actor's single native TF cache.
pub(crate) struct RobotState {
    configuration: robot::Configuration,
    next_batch: Option<Instant>,
    pose_batches: u64,
}
impl RobotState {
    pub(crate) fn new(configuration: &Configuration) -> Result<Self, String> {
        if configuration.output != Output::Robot || !configuration.valid() {
            return Err("Invalid shared Robot configuration".into());
        }
        Ok(Self {
            configuration: *configuration
                .robot
                .clone()
                .ok_or("Missing Robot configuration")?,
            next_batch: None,
            pose_batches: 0,
        })
    }
    pub(crate) fn poll(
        &mut self,
        tf: &mut SpatialSession,
        cancel: &AtomicBool,
        shared: &studio_protocol::spatial::Counters,
    ) -> Result<Option<robot::Sample>, String> {
        let now = Instant::now();
        if self.next_batch.is_some_and(|next| now < next) {
            return Ok(None);
        }
        let mut links = Vec::with_capacity(self.configuration.frames.len());
        for frame in &self.configuration.frames {
            if cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let pose = match tf.lookup_latest(&self.configuration.fixed_frame, frame) {
                Ok(result) => LinkPose::Ready {
                    resolved_stamp_ns: result.stamp_ns,
                    components: result.components,
                },
                Err(error) => match error.status {
                    Status::Missing => LinkPose::Missing,
                    Status::Stale => LinkPose::Stale,
                    Status::Extrapolated => LinkPose::Extrapolated,
                    _ => {
                        return Err(format!(
                            "Shared Robot TF {:?}: {}",
                            error.status, error.message
                        ))
                    }
                },
            };
            links.push(LinkResult {
                frame: frame.clone(),
                pose,
            });
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.pose_batches = self.pose_batches.saturating_add(1);
        let sample = robot::Sample {
            contract_version: robot::CONTRACT_VERSION,
            fixed_frame: self.configuration.fixed_frame.clone(),
            policy: robot::PosePolicy::LatestAvailable,
            links,
            counters: robot::Counters {
                tf_dynamic_batches: shared.tf_dynamic_batches,
                tf_static_batches: shared.tf_static_batches,
                tf_accepted: shared.tf_accepted,
                tf_rejected: shared.tf_rejected,
                duplicate_statics: shared.duplicate_statics,
                native_errors: shared.native_errors,
                pose_batches: self.pose_batches,
            },
        };
        sample.validate(&self.configuration)?;
        self.next_batch =
            Some(Instant::now() + Duration::from_millis(1000 / u64::from(robot::MAX_HZ)));
        Ok(Some(sample))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{rc::Rc, sync::atomic::AtomicUsize, thread};
    pub(crate) fn configuration() -> Configuration {
        Configuration {
            map_updates: None,
            topic: "/tf".into(),
            message_type: spatial_input::TF_MESSAGE_TYPE.into(),
            best_effort: false,
            transient_local: false,
            output: Output::Robot,
            spatial: None,
            robot: Some(
                robot::Configuration {
                    fixed_frame: "map".into(),
                    frames: vec!["map".into(), "base".into(), "tool".into(), "absent".into()],
                    clock_policy: SpatialClockPolicy::VerifiedSystem,
                    dynamic_tf_topic: "/tf".into(),
                    dynamic_tf_best_effort: false,
                    static_tf_topic: "/tf_static".into(),
                }
                .into(),
            ),
        }
    }
    struct Empty {
        owner: thread::ThreadId,
        drops: Arc<AtomicUsize>,
        _local: Rc<()>,
    }
    impl Reader for Empty {
        fn poll(&mut self) -> Result<Option<structured::Value>, String> {
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
    #[test]
    fn complete_ordered_latest_batches_are_rate_limited_and_recover_missing_links() {
        let drops = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader =
            RobotReader::open(&configuration(), &factory(drops.clone()), &cancel).unwrap();
        let now = Instant::now();
        let first = reader.poll_at(now).unwrap().unwrap();
        assert_eq!(
            first
                .links
                .iter()
                .map(|l| l.frame.as_str())
                .collect::<Vec<_>>(),
            ["map", "base", "tool", "absent"]
        );
        assert!(matches!(
            first.links[0].pose,
            LinkPose::Ready {
                resolved_stamp_ns: 0,
                ..
            }
        ));
        assert!(matches!(first.links[1].pose, LinkPose::Missing));
        reader
            .input(0, crate::spatial_worker::tests::tf("base", "tool", 0, 2.))
            .unwrap();
        reader
            .input(1, crate::spatial_worker::tests::tf("map", "base", 1, 1.))
            .unwrap();
        assert!(reader
            .poll_at(now + Duration::from_millis(99))
            .unwrap()
            .is_none());
        let next = reader
            .poll_at(now + Duration::from_millis(100))
            .unwrap()
            .unwrap();
        assert!(
            matches!(next.links[1].pose,LinkPose::Ready {resolved_stamp_ns:1_000_000_000,components} if components[0]==1.)
        );
        assert!(
            matches!(next.links[2].pose,LinkPose::Ready {resolved_stamp_ns:1_000_000_000,components} if components[0]==3.)
        );
        assert!(matches!(next.links[3].pose, LinkPose::Missing));
        assert_eq!(next.counters.pose_batches, 2);
        assert_eq!(next.counters.tf_accepted, 2);
        cancel.store(true, Ordering::Release);
        assert!(reader
            .poll_at(now + Duration::from_secs(1))
            .unwrap()
            .is_none());
        drop(reader);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn slow_batch_work_still_requires_a_full_interval_after_completion() {
        let drops = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader = RobotReader::open(&configuration(), &factory(drops), &cancel).unwrap();
        let start = Instant::now();
        let calls = std::cell::Cell::new(0);
        // Deterministically simulate 300 ms spent between the initial clock read
        // and completed batch, without sleeping or depending on host speed.
        let first = reader
            .poll_with_clock(|| {
                let call = calls.get();
                calls.set(call + 1);
                if call == 0 {
                    start
                } else {
                    start + Duration::from_millis(300)
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(first.counters.pose_batches, 1);
        assert_eq!(reader.next_batch, Some(start + Duration::from_millis(400)));
        assert!(reader
            .poll_at(start + Duration::from_millis(300))
            .unwrap()
            .is_none());
        assert!(reader
            .poll_at(start + Duration::from_millis(399))
            .unwrap()
            .is_none());
        assert_eq!(
            reader
                .poll_at(start + Duration::from_millis(400))
                .unwrap()
                .unwrap()
                .counters
                .pose_batches,
            2
        );
    }
    #[test]
    fn static_reader_opens_first_and_partial_failure_or_cancel_drops_on_owner_thread() {
        for cancel_second in [false, true] {
            let drops = Arc::new(AtomicUsize::new(0));
            let cancel = Arc::new(AtomicBool::new(false));
            let d = drops.clone();
            let c = cancel.clone();
            let open: Factory = Arc::new(move |config| {
                if config.topic == "/tf_static" {
                    assert!(!config.best_effort && config.transient_local);
                } else {
                    assert_eq!(config.topic, "/tf");
                    assert_eq!(d.load(Ordering::SeqCst), 0);
                    if cancel_second {
                        c.store(true, Ordering::Release);
                    } else {
                        return Err("controlled second open failure".into());
                    }
                }
                Ok(Box::new(Empty {
                    owner: thread::current().id(),
                    drops: d.clone(),
                    _local: Rc::new(()),
                }))
            });
            assert!(RobotReader::open(&configuration(), &open, &cancel).is_err());
            assert_eq!(
                drops.load(Ordering::SeqCst),
                if cancel_second { 2 } else { 1 }
            );
        }
    }
    #[test]
    fn malformed_tf_and_conflicting_static_fail_without_identity_fallback() {
        let drops = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader = RobotReader::open(&configuration(), &factory(drops), &cancel).unwrap();
        assert!(reader
            .input(1, structured::Value::Unsigned { value: "1".into() })
            .is_err());
        reader
            .input(0, crate::spatial_worker::tests::tf("map", "base", 0, 1.))
            .unwrap();
        assert!(reader
            .input(0, crate::spatial_worker::tests::tf("map", "base", 0, 2.))
            .is_err());
    }
    struct CancelPoll {
        cancel: Arc<AtomicBool>,
    }
    impl Reader for CancelPoll {
        fn poll(&mut self) -> Result<Option<structured::Value>, String> {
            self.cancel.store(true, Ordering::Release);
            Ok(None)
        }
    }
    #[test]
    fn cancellation_during_input_poll_prevents_any_partial_pose_batch() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let open: Factory = Arc::new(move |_| Ok(Box::new(CancelPoll { cancel: c.clone() })));
        let mut reader = RobotReader::open(&configuration(), &open, &cancel).unwrap();
        assert!(reader.poll_at(Instant::now()).unwrap().is_none());
        assert_eq!(reader.counters.pose_batches, 0);
    }
}
