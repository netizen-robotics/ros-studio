//! Publication policy only: never substitutes a transform or changes wire data.
use std::time::{Duration, Instant};
use studio_protocol::spatial::{self, TfOutcome};

#[derive(Default)]
pub(crate) struct DisplayContinuity {
    source: Option<String>,
    ready_at: Option<Instant>,
}
impl DisplayContinuity {
    pub fn source(&mut self, frame: &str) {
        if self.source.as_deref() != Some(frame) {
            self.source = Some(frame.into());
            self.ready_at = None;
        }
    }
    pub fn publication(&mut self, outcome: TfOutcome, now: Instant) -> Option<TfOutcome> {
        if matches!(outcome, TfOutcome::Ready { .. }) {
            self.ready_at = Some(now);
            return Some(outcome);
        }
        if let Some(at) = self.ready_at {
            if !matches!(outcome, TfOutcome::LookupTimeout)
                && now.saturating_duration_since(at)
                    < Duration::from_millis(spatial::PENDING_TIMEOUT_MS)
            {
                return None;
            }
            self.ready_at = None;
            return Some(TfOutcome::LookupTimeout);
        }
        Some(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_gap_is_suppressed_but_input_and_frame_changes_cannot_extend_it() {
        let mut policy = DisplayContinuity::default();
        let now = Instant::now();
        policy.source("source");
        assert_eq!(
            policy.publication(TfOutcome::Missing, now),
            Some(TfOutcome::Missing)
        );
        let ready = TfOutcome::Ready {
            components: [0., 0., 0., 0., 0., 0., 1.],
        };
        assert_eq!(policy.publication(ready.clone(), now), Some(ready.clone()));
        for millis in [50, 250, 900] {
            policy.source("source");
            assert!(policy
                .publication(TfOutcome::Extrapolated, now + Duration::from_millis(millis))
                .is_none());
        }
        assert_eq!(
            policy.publication(TfOutcome::Extrapolated, now + Duration::from_millis(1000)),
            Some(TfOutcome::LookupTimeout)
        );
        policy.publication(ready, now + Duration::from_millis(1100));
        policy.source("another_source");
        assert_eq!(
            policy.publication(TfOutcome::Missing, now + Duration::from_millis(1150)),
            Some(TfOutcome::Missing)
        );
    }
}
