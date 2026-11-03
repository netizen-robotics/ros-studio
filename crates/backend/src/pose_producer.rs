//! Ordered source/history processing and complete, coalesced worker output.
//! The future scene actor owns readers/fair-queue retirement and Clear acks.
use crate::spatial_session::{SpatialSession, Status};
use studio_protocol::{
    latest_pose::{LatestPose, LookupTime},
    pose_snapshot::{
        self as wire,
        store::{Input, Store, Token},
        Decoded, HistorySettings, Owner,
    },
};
pub struct PoseProducer {
    store: Store,
    token: Token,
    last_sent_revision: Option<u64>,
    next_emit: u64,
    // One immutable empty Clear barrier, never a second retained history.
    pending_clear: Option<wire::Snapshot>,
}
impl PoseProducer {
    pub fn new(owner: Owner, history: Option<HistorySettings>) -> Result<Self, String> {
        let mut store = Store::default();
        let token = store.start(owner, history)?;
        Ok(Self {
            store,
            token,
            last_sent_revision: None,
            next_emit: 0,
            pending_clear: None,
        })
    }
    pub fn source(&self) -> &Store {
        &self.store
    }
    pub fn input(&mut self, sequence: u64, decoded: Decoded, now: u64) -> Result<Input, String> {
        self.store
            .input(self.token, sequence, decoded, now)
            .map_err(|e| e.to_string())
    }
    pub fn clear(&mut self) -> Result<bool, String> {
        let changed = self.store.clear(self.token).map_err(|e| e.to_string())?;
        if changed {
            self.pending_clear = self.store.snapshot();
        }
        Ok(changed)
    }
    pub fn source_gap(&mut self) -> Result<bool, String> {
        self.store.source_gap(self.token).map_err(|e| e.to_string())
    }
    pub fn source_fault(&mut self) -> bool {
        self.pending_clear = None;
        self.store.source_fault(self.token)
    }
    pub fn stop(&mut self) {
        self.store.stop();
        self.pending_clear = None;
    }
    /// Process TF/retry/expiry even when the caller has no fair outbound slot.
    /// `now` is owner-relative monotonic nanoseconds, not ROS acquisition time.
    pub fn poll(
        &mut self,
        tf: &mut SpatialSession,
        now: u64,
        can_emit: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        self.poll_snapshot(tf, now, can_emit)?
            .map(|snapshot| snapshot.encode())
            .transpose()
    }
    pub fn poll_snapshot(
        &mut self,
        tf: &mut SpatialSession,
        now: u64,
        can_emit: bool,
    ) -> Result<Option<wire::Snapshot>, String> {
        if self.store.token().is_none() {
            self.pending_clear = None;
            return Ok(None);
        }
        if !tf.is_active() {
            self.source_fault();
            return Err("Pose/covariance requires active VerifiedSystem TF context".into());
        }
        for request in self
            .store
            .requests(self.token, now)
            .map_err(|e| e.to_string())?
        {
            let result = match request.time() {
                LookupTime::At(stamp) => {
                    tf.lookup_at(request.fixed_frame(), request.source_frame(), stamp)
                }
                LookupTime::Latest => {
                    tf.lookup_latest(request.fixed_frame(), request.source_frame())
                }
            };
            let transform = match result {
                Ok(value) => LatestPose::Ready {
                    resolved_stamp_ns: value.stamp_ns,
                    components: value.components,
                },
                Err(error) => match error.status {
                    Status::Missing => LatestPose::Missing,
                    Status::Stale => LatestPose::Stale,
                    Status::Extrapolated => LatestPose::Extrapolated,
                    _ => {
                        self.source_fault();
                        return Err(format!("Pose TF {:?}: {}", error.status, error.message));
                    }
                },
            };
            self.store
                .complete(&request, transform, now)
                .map_err(|e| e.to_string())?;
        }
        if can_emit {
            if let Some(snapshot) = self.pending_clear.take() {
                self.next_emit = now + 1_000_000_000 / u64::from(wire::MAX_HZ);
                self.last_sent_revision = Some(snapshot.revision);
                return Ok(Some(snapshot));
            }
        }
        if !can_emit
            || now < self.next_emit
            || self.last_sent_revision == Some(self.store.revision())
        {
            return Ok(None);
        }
        let snapshot = self
            .store
            .presentation_snapshot(now)
            .ok_or("Pose source retired before capture")?;
        self.next_emit = now + 1_000_000_000 / u64::from(wire::MAX_HZ);
        self.last_sent_revision = Some(snapshot.revision);
        Ok(Some(snapshot))
    }
}
#[cfg(test)]
mod tests;
