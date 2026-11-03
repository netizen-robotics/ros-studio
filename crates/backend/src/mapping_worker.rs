//! One canonical mapping payload, shared with its bounded acquisition-time slot.
use crate::spatial_session::{SpatialSession, Status};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use studio_protocol::{
    mapping,
    spatial::{self, TfOutcome},
    subscriptions::Configuration,
};

struct Pending {
    decoded: Arc<mapping::Decoded>,
    began: Instant,
    attempt: Option<Instant>,
    previous: Option<TfOutcome>,
}
pub(crate) struct MappingState {
    configuration: Configuration,
    latest: Option<Arc<mapping::Decoded>>,
    pending: Option<Pending>,
    counters: mapping::Counters,
    continuity: crate::display_continuity::DisplayContinuity,
}
impl MappingState {
    pub(crate) fn new(configuration: &Configuration) -> Result<Self, String> {
        if !configuration.valid() || !configuration.output.is_mapping() {
            return Err("Invalid shared mapping configuration".into());
        }
        Ok(Self {
            configuration: configuration.clone(),
            latest: None,
            pending: None,
            counters: mapping::Counters::default(),
            continuity: Default::default(),
        })
    }
    pub(crate) fn input(&mut self, decoded: mapping::Decoded, now: Instant) -> Result<(), String> {
        decoded.validate()?;
        if decoded.topic != self.configuration.topic
            || decoded.message_type != self.configuration.message_type
        {
            return Err("Mapping source contradicts active subscription".into());
        }
        self.counters.messages_received = self.counters.messages_received.saturating_add(1);
        if let Some(previous) = &self.latest {
            if decoded.acquisition_stamp_ns == previous.acquisition_stamp_ns
                && decoded == **previous
            {
                return Ok(());
            }
            if decoded.acquisition_stamp_ns <= previous.acquisition_stamp_ns {
                return Err("Changed equal-stamp or decreasing mapping acquisition; explicit restart required".into());
            }
        }
        self.continuity.source(&decoded.source_frame);
        let decoded = Arc::new(decoded);
        self.latest = Some(decoded.clone());
        self.counters.pending_replaced = self
            .counters
            .pending_replaced
            .saturating_add(u64::from(self.pending.is_some()));
        self.pending = Some(Pending {
            decoded,
            began: now,
            attempt: None,
            previous: None,
        });
        Ok(())
    }
    pub(crate) fn poll(
        &mut self,
        tf: &mut SpatialSession,
        now: Instant,
        shared: &spatial::Counters,
    ) -> Result<Option<mapping::Sample>, String> {
        let Some(pending) = &mut self.pending else {
            return Ok(None);
        };
        if !tf.is_active() {
            return Err("Mapping requires active VerifiedSystem TF context".into());
        }
        let expired = now.saturating_duration_since(pending.began)
            >= Duration::from_millis(spatial::PENDING_TIMEOUT_MS);
        if !expired
            && pending.attempt.is_some_and(|last| {
                now.saturating_duration_since(last)
                    < Duration::from_millis(spatial::RETRY_INTERVAL_MS)
            })
        {
            return Ok(None);
        }
        pending.attempt = Some(now);
        let fixed = &self
            .configuration
            .spatial
            .as_ref()
            .ok_or("Missing mapping TF configuration")?
            .fixed_frame;
        let outcome = if expired {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
            TfOutcome::LookupTimeout
        } else if fixed == &pending.decoded.source_frame {
            // Mathematical identity needs no admitted TF edge. This is scoped
            // to mapping; scan/cloud lookup_at admission remains unchanged.
            TfOutcome::Ready {
                components: [0., 0., 0., 0., 0., 0., 1.],
            }
        } else {
            match tf.lookup_at(
                fixed,
                &pending.decoded.source_frame,
                pending.decoded.acquisition_stamp_ns,
            ) {
                Ok(result) => TfOutcome::Ready {
                    components: result.components,
                },
                Err(error) => match error.status {
                    Status::Missing => TfOutcome::Missing,
                    Status::Stale => TfOutcome::Stale,
                    Status::Extrapolated => TfOutcome::Extrapolated,
                    _ => return Err(format!("Mapping TF {:?}: {}", error.status, error.message)),
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
        let decoded = &pending.decoded;
        let sample = mapping::Sample {
            contract_version: mapping::CONTRACT_VERSION,
            topic: decoded.topic.clone(),
            message_type: decoded.message_type.clone(),
            fixed_frame: fixed.clone(),
            source_frame: decoded.source_frame.clone(),
            acquisition_stamp_ns: decoded.acquisition_stamp_ns,
            geometry: decoded.geometry.clone(),
            tf: outcome.clone(),
            counters: mapping::Counters {
                messages_received: self.counters.messages_received,
                pending_replaced: self.counters.pending_replaced,
                lookup_timeouts: self.counters.lookup_timeouts,
                tf_dynamic_batches: shared.tf_dynamic_batches,
                tf_static_batches: shared.tf_static_batches,
                tf_accepted: shared.tf_accepted,
                tf_rejected: shared.tf_rejected,
                duplicate_statics: shared.duplicate_statics,
                native_errors: shared.native_errors,
            },
        };
        sample.validate()?;
        if expired || matches!(outcome, TfOutcome::Ready { .. }) {
            self.pending = None;
        }
        Ok(Some(sample))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::spatial_session::{ClockPolicy, StreamKind, Transform};
    use studio_protocol::{mapping::Geometry, subscriptions::Output};
    pub(crate) fn configuration() -> Configuration {
        let mut c = crate::spatial_worker::tests::configuration();
        c.topic = "/map".into();
        c.output = Output::OccupancyGrid;
        c.message_type = mapping::MAP_MESSAGE_TYPE.into();
        c
    }
    fn map(frame: &str, stamp: i64) -> mapping::Decoded {
        mapping::Decoded {
            topic: "/map".into(),
            message_type: mapping::MAP_MESSAGE_TYPE.into(),
            source_frame: frame.into(),
            acquisition_stamp_ns: stamp,
            geometry: Geometry::OccupancyGrid {
                width: 1,
                height: 1,
                resolution: 0.5,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
                cells: vec![-1],
            },
        }
    }
    fn active() -> SpatialSession {
        let mut tf = SpatialSession::new().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        tf
    }
    #[test]
    fn identity_without_admitted_frames_and_exact_duplicate_has_no_replay() {
        let mut state = MappingState::new(&configuration()).unwrap();
        let mut tf = active();
        let now = Instant::now();
        let original = map("map", 1);
        state.input(original.clone(), now).unwrap();
        assert!(Arc::ptr_eq(
            state.latest.as_ref().unwrap(),
            &state.pending.as_ref().unwrap().decoded
        ));
        assert!(
            matches!(state.poll(&mut tf,now,&spatial::Counters::default()).unwrap().unwrap().tf,TfOutcome::Ready{components} if components == [0.,0.,0.,0.,0.,0.,1.])
        );
        assert_eq!(tf.frame_count(), 0);
        assert!(tf.lookup_at("map", "map", 1).is_err());
        state.input(original.clone(), now).unwrap();
        assert!(state
            .poll(&mut tf, now, &spatial::Counters::default())
            .unwrap()
            .is_none());
        let mut changed = original;
        if let Geometry::OccupancyGrid { cells, .. } = &mut changed.geometry {
            cells[0] = 100;
        }
        assert!(state.input(changed, now).is_err());
        assert!(state.input(map("map", 0), now).is_err());
    }
    #[test]
    fn missing_recovers_only_at_header_time_and_retained_history_never_uses_latest() {
        let mut state = MappingState::new(&configuration()).unwrap();
        let mut tf = active();
        let now = Instant::now();
        state.input(map("source", 2_000_000_000), now).unwrap();
        assert!(matches!(
            state
                .poll(&mut tf, now, &spatial::Counters::default())
                .unwrap()
                .unwrap()
                .tf,
            TfOutcome::Missing
        ));
        for (stamp, x) in [(1_000_000_000, 1.), (3_000_000_000, 3.)] {
            tf.insert(
                StreamKind::Dynamic,
                &Transform {
                    parent: "map".into(),
                    child: "source".into(),
                    stamp_ns: stamp,
                    components: [x, 0., 0., 0., 0., 0., 1.],
                },
            )
            .unwrap();
        }
        let ready = state
            .poll(
                &mut tf,
                now + Duration::from_millis(50),
                &spatial::Counters::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(ready.acquisition_stamp_ns, 2_000_000_000);
        assert!(matches!(ready.tf,TfOutcome::Ready{components} if components[0]==2.));
        let mut late = MappingState::new(&configuration()).unwrap();
        late.input(map("source", 500_000_000), now).unwrap();
        assert!(matches!(
            late.poll(&mut tf, now, &spatial::Counters::default())
                .unwrap()
                .unwrap()
                .tf,
            TfOutcome::Extrapolated
        ));
        assert!(matches!(
            late.poll(
                &mut tf,
                now + Duration::from_millis(1000),
                &spatial::Counters::default()
            )
            .unwrap()
            .unwrap()
            .tf,
            TfOutcome::LookupTimeout
        ));
        assert!(late.pending.is_none());
        assert!(late.latest.is_some());
    }
    #[test]
    fn duplicate_pending_does_not_extend_deadline_and_new_empty_payload_clears() {
        let mut state = MappingState::new(&configuration()).unwrap();
        let mut tf = active();
        let now = Instant::now();
        let source = map("unknown", 1);
        state.input(source.clone(), now).unwrap();
        state
            .input(source, now + Duration::from_millis(900))
            .unwrap();
        let timedout = state
            .poll(
                &mut tf,
                now + Duration::from_millis(1000),
                &spatial::Counters::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(timedout.counters.lookup_timeouts, 1);
        let mut empty = map("map", 2);
        if let Geometry::OccupancyGrid {
            width,
            height,
            cells,
            ..
        } = &mut empty.geometry
        {
            *width = 0;
            *height = 0;
            cells.clear();
        }
        state
            .input(empty, now + Duration::from_millis(1001))
            .unwrap();
        let sample = state
            .poll(
                &mut tf,
                now + Duration::from_millis(1001),
                &spatial::Counters::default(),
            )
            .unwrap()
            .unwrap();
        assert!(
            matches!(sample.geometry,Geometry::OccupancyGrid{width:0,height:0,cells,..} if cells.is_empty())
        );
    }
}
