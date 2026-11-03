//! One bounded acquisition-time cloud pending slot; shared TF remains upstream.
use crate::spatial_session::{SpatialSession, Status};
use std::time::{Duration, Instant};
use studio_protocol::{
    cloud,
    spatial::{self, TfOutcome},
    subscriptions::{Configuration, Output},
};
struct Pending {
    cloud: cloud::DecodedCloud,
    began: Instant,
    attempt: Option<Instant>,
    previous: Option<TfOutcome>,
}
pub(crate) struct CloudState {
    configuration: Configuration,
    pending: Option<Pending>,
    last_stamp: Option<i64>,
    counters: cloud::Counters,
    continuity: crate::display_continuity::DisplayContinuity,
}
impl CloudState {
    pub(crate) fn new(configuration: &Configuration) -> Result<Self, String> {
        if !configuration.valid() || configuration.output != Output::PointCloud {
            return Err("Invalid shared PointCloud2 configuration".into());
        }
        Ok(Self {
            configuration: configuration.clone(),
            pending: None,
            last_stamp: None,
            counters: cloud::Counters::default(),
            continuity: Default::default(),
        })
    }
    pub(crate) fn input(&mut self, cloud: cloud::DecodedCloud, now: Instant) -> Result<(), String> {
        cloud.channels.validate(cloud.points.len())?;
        if cloud.acquisition_stamp_ns <= 0
            || self
                .last_stamp
                .is_some_and(|last| cloud.acquisition_stamp_ns <= last)
        {
            return Err("Out-of-order PointCloud2 acquisition; explicit restart required".into());
        }
        if !studio_protocol::spatial::valid_frame(&cloud.source_frame)
            || cloud.source_points > cloud::MAX_SOURCE_POINTS
            || cloud.skipped_points > cloud.source_points
            || cloud.points.len() != cloud.source_points - cloud.skipped_points
            || cloud
                .points
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
        {
            return Err("Invalid complete bounded PointCloud2 input".into());
        }
        self.continuity.source(&cloud.source_frame);
        self.last_stamp = Some(cloud.acquisition_stamp_ns);
        self.counters.clouds_received = self.counters.clouds_received.saturating_add(1);
        self.counters.pending_replaced = self
            .counters
            .pending_replaced
            .saturating_add(u64::from(self.pending.is_some()));
        self.pending = Some(Pending {
            cloud,
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
    ) -> Result<Option<cloud::Sample>, String> {
        let Some(pending) = &mut self.pending else {
            return Ok(None);
        };
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
            .ok_or("Missing PointCloud2 TF configuration")?
            .fixed_frame;
        let outcome = if expired {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
            TfOutcome::LookupTimeout
        } else {
            match tf.lookup_at(
                fixed,
                &pending.cloud.source_frame,
                pending.cloud.acquisition_stamp_ns,
            ) {
                Ok(result) => TfOutcome::Ready {
                    components: result.components,
                },
                Err(error) => match error.status {
                    Status::Missing => TfOutcome::Missing,
                    Status::Stale => TfOutcome::Stale,
                    Status::Extrapolated => TfOutcome::Extrapolated,
                    _ => {
                        return Err(format!(
                            "PointCloud2 TF {:?}: {}",
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
        let sample = cloud::Sample {
            contract_version: cloud::CONTRACT_VERSION,
            topic: self.configuration.topic.clone(),
            message_type: cloud::MESSAGE_TYPE.into(),
            fixed_frame: fixed.clone(),
            source_frame: pending.cloud.source_frame.clone(),
            acquisition_stamp_ns: pending.cloud.acquisition_stamp_ns,
            source_points: pending.cloud.source_points,
            skipped_points: pending.cloud.skipped_points,
            points: pending.cloud.points.clone(),
            channels: pending.cloud.channels.clone(),
            tf: outcome.clone(),
            counters: cloud::Counters {
                clouds_received: self.counters.clouds_received,
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
