//! Paired integration contracts; not advertised by session14 yet.
use super::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const MAX_CONTROL_BYTES: usize = 4096;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "sample",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Message {
    Snapshot(Box<Snapshot>),
    Invalidated(Invalidation),
}
impl Message {
    pub fn owner(&self) -> &Owner {
        match self {
            Self::Snapshot(s) => &s.owner,
            Self::Invalidated(b) => &b.owner,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Snapshot(s) => s.validate(),
            Self::Invalidated(b) => b.validate(),
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode(
            self,
            if matches!(self, Self::Invalidated(_)) {
                MAX_CONTROL_BYTES
            } else {
                MAX_SAMPLE_BYTES
            },
        )
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Self = decode(bytes, MAX_SAMPLE_BYTES)?;
        value.validate()?;
        if matches!(value, Self::Invalidated(_)) && bytes.len() > MAX_CONTROL_BYTES {
            return Err("Pose control exceeds4KiB".into());
        }
        Ok(value)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_sample_bytes: usize,
    pub max_control_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_history: usize,
    pub max_pending: usize,
    pub max_owners: usize,
    pub max_owners_per_kind: usize,
    pub max_hz: u32,
    pub retry_ns: u64,
    pub timeout_ns: u64,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_sample_bytes: MAX_SAMPLE_BYTES,
            max_control_bytes: MAX_CONTROL_BYTES,
            max_decoded_bytes: MAX_DECODED_BYTES,
            max_history: MAX_HISTORY,
            max_pending: MAX_PENDING,
            max_owners: MAX_OWNERS,
            max_owners_per_kind: 1,
            max_hz: MAX_HZ,
            retry_ns: store::RETRY_NS,
            timeout_ns: store::TIMEOUT_NS,
        }
    }
}
impl Capabilities {
    pub fn validate(&self) -> Result<(), String> {
        if *self != Self::default() {
            return Err("Incompatible pose/covariance capabilities".into());
        }
        Ok(())
    }
}

/// Exact integer configuration for equality, dirty tracking and future saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryConfiguration {
    pub keep: u16,
    pub position_tolerance_um: u32,
    pub angle_tolerance_urad: u32,
}
impl Default for HistoryConfiguration {
    fn default() -> Self {
        Self::DEFAULT
    }
}
impl HistoryConfiguration {
    pub const DEFAULT: Self = Self {
        keep: 100,
        position_tolerance_um: 100_000,
        angle_tolerance_urad: 100_000,
    };
    pub fn settings(self) -> Result<HistorySettings, String> {
        let settings = HistorySettings {
            keep: usize::from(self.keep),
            position_tolerance_m: f64::from(self.position_tolerance_um) / 1_000_000.,
            angle_tolerance_rad: f64::from(self.angle_tolerance_urad) / 1_000_000.,
        };
        settings.validate()?;
        Ok(settings)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub kind: Kind,
    pub history: Option<HistoryConfiguration>,
}
impl Configuration {
    pub const fn for_kind(kind: Kind) -> Self {
        Self {
            kind,
            history: match kind {
                Kind::PoseWithCovariance => None,
                Kind::Odometry => Some(HistoryConfiguration::DEFAULT),
            },
        }
    }
    pub fn settings(self) -> Result<Option<HistorySettings>, String> {
        match (self.kind, self.history) {
            (Kind::PoseWithCovariance, None) => Ok(None),
            (Kind::Odometry, Some(history)) => history.settings().map(Some),
            _ => Err("Pose kind and source history configuration disagree".into()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invalidation {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    pub continuity_epoch: u64,
    pub reason: ResetReason,
}
impl Invalidation {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || self.continuity_epoch == 0
        {
            return Err("Invalid pose continuity barrier version or identity".into());
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode(self, MAX_CONTROL_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let result: Self = decode(bytes, MAX_CONTROL_BYTES)?;
        result.validate()?;
        Ok(result)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Updated,
    Stale,
    WaitingForClear,
}
/// Cached preparation results must carry and recheck this permission when used.
#[derive(Clone, Debug)]
pub struct Permission {
    serial: u64,
    authority: Arc<AtomicU64>,
    snapshot: std::sync::Weak<Snapshot>,
}
#[derive(Clone)]
pub struct Revocation(Arc<AtomicU64>);
impl Revocation {
    pub fn revoke(&self) {
        self.0.store(0, Ordering::Release);
    }
}
impl Permission {
    pub fn is_valid(&self) -> bool {
        self.authority.load(Ordering::Acquire) == self.serial
    }
    pub fn authorizes(&self, snapshot: &Arc<Snapshot>) -> bool {
        self.is_valid()
            && self
                .snapshot
                .upgrade()
                .is_some_and(|current| Arc::ptr_eq(&current, snapshot))
    }
}

/// One current owner's client boundary. No I/O, renderer or command replay.
pub struct Receiver {
    owner: Owner,
    history: Option<HistorySettings>,
    active: bool,
    revision: u64,
    epoch: u64,
    snapshot: Option<Arc<Snapshot>>,
    pending_clear: Option<RequestId>,
    clear_floor: Option<(u64, u64)>,
    last_barrier: Option<Invalidation>,
    last_clear: u64,
    fault: Option<String>,
    serial: u64,
    authority: Arc<AtomicU64>,
}
impl Receiver {
    pub fn new(owner: Owner, configuration: Configuration) -> Result<Self, String> {
        owner.validate()?;
        if owner.kind != configuration.kind {
            return Err("Pose owner and configuration disagree".into());
        }
        Ok(Self {
            owner,
            history: configuration.settings()?,
            active: true,
            revision: 0,
            epoch: 0,
            snapshot: None,
            pending_clear: None,
            clear_floor: None,
            last_barrier: None,
            last_clear: 0,
            fault: None,
            serial: 0,
            authority: Arc::new(AtomicU64::new(0)),
        })
    }
    /// Data ownership only. Cached use also requires a current `permission`.
    pub fn snapshot(&self) -> Option<&Arc<Snapshot>> {
        self.snapshot.as_ref()
    }
    pub fn permission(&self) -> Option<Permission> {
        self.snapshot.as_ref()?;
        Some(Permission {
            serial: self.serial,
            authority: self.authority.clone(),
            snapshot: Arc::downgrade(self.snapshot.as_ref()?),
        })
    }
    pub fn revocation(&self) -> Revocation {
        Revocation(self.authority.clone())
    }
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
    pub fn pending_clear(&self) -> Option<RequestId> {
        self.pending_clear
    }
    pub fn stop(&mut self) {
        self.authority.store(0, Ordering::Release);
        self.active = false;
        self.snapshot = None;
        self.pending_clear = None;
        self.clear_floor = None;
        self.last_barrier = None;
    }
    fn fail<T>(&mut self, message: &str) -> Result<T, String> {
        self.stop();
        self.fault = Some(message.into());
        Err(message.into())
    }
    fn identifies(&self, owner: &Owner) -> bool {
        self.active
            && self.owner.generation == owner.generation
            && self.owner.subscription == owner.subscription
    }
    pub fn apply(&mut self, snapshot: Snapshot) -> Result<Applied, String> {
        if !self.identifies(&snapshot.owner) {
            return Ok(Applied::Stale);
        }
        if self.owner != snapshot.owner {
            return self.fail("Pose snapshot contradicts current owner metadata");
        }
        let follows_barrier = self.snapshot.is_none()
            && self.last_barrier.as_ref().is_some_and(|barrier| {
                barrier.revision == snapshot.revision
                    && barrier.continuity_epoch == snapshot.continuity_epoch
                    && snapshot.last_reset == Some(barrier.reason)
            });
        if snapshot.revision < self.revision
            || (snapshot.revision == self.revision && !follows_barrier)
            || snapshot.continuity_epoch < self.epoch
        {
            return Ok(Applied::Stale);
        }
        if self.pending_clear.is_some() {
            return Ok(Applied::WaitingForClear);
        }
        if snapshot.history != self.history || snapshot.validate().is_err() {
            return self.fail("Invalid current pose snapshot or history configuration");
        }
        if snapshot.last_reset == Some(ResetReason::SourceFault) {
            return self.fail("Pose source fault; explicitly restart the display");
        }
        // A complete, same-continuity replacement must not blank geometry while
        // the worker prepares its covariance. Permissions remain bound to their
        // exact snapshot. Non-ready heads and all barriers still revoke at once.
        let replacement = self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.source == snapshot.source)
            && snapshot.continuity_epoch == self.epoch
            && self.authority.load(Ordering::Acquire) == self.serial
            && snapshot.latest.as_ref().is_some_and(|entry| {
                matches!(
                    entry.transform,
                    crate::latest_pose::LatestPose::Ready { .. }
                )
            });
        if !replacement {
            let Some(serial) = self.serial.checked_add(1) else {
                return self.fail("Pose cached permission identity exhausted");
            };
            self.serial = serial;
            self.authority.store(serial, Ordering::Release);
        }
        self.revision = snapshot.revision;
        self.epoch = snapshot.continuity_epoch;
        self.snapshot = Some(Arc::new(snapshot));
        self.last_barrier = None;
        Ok(Applied::Updated)
    }
    pub fn invalidate(&mut self, barrier: &Invalidation) -> Result<Applied, String> {
        if !self.identifies(&barrier.owner) {
            return Ok(Applied::Stale);
        }
        if self.owner != barrier.owner {
            return self.fail("Pose barrier contradicts current owner metadata");
        }
        if barrier.revision <= self.revision || barrier.continuity_epoch <= self.epoch {
            return Ok(Applied::Stale);
        }
        if barrier.validate().is_err() {
            return self.fail("Invalid current pose continuity barrier");
        }
        self.revision = barrier.revision;
        self.epoch = barrier.continuity_epoch;
        self.authority.store(0, Ordering::Release);
        self.snapshot = None;
        self.last_barrier = Some(barrier.clone());
        if barrier.reason == ResetReason::SourceFault {
            return self.fail("Pose source fault; explicitly restart the display");
        }
        Ok(Applied::Updated)
    }
    pub fn begin_clear(&mut self, request: RequestId) -> Result<(), String> {
        if !self.active || self.pending_clear.is_some() || request.0 <= self.last_clear {
            return Err("Pose Clear needs an active owner and fresh request".into());
        }
        self.last_clear = request.0;
        self.pending_clear = Some(request);
        self.clear_floor = Some((self.revision, self.epoch));
        self.authority.store(0, Ordering::Release);
        self.snapshot = None;
        Ok(())
    }
    pub fn acknowledge_clear(
        &mut self,
        request: RequestId,
        barrier: &Invalidation,
    ) -> Result<bool, String> {
        if !self.identifies(&barrier.owner) || self.pending_clear != Some(request) {
            return Ok(false);
        }
        if self.owner != barrier.owner
            || barrier.reason != ResetReason::LocalClear
            || barrier.validate().is_err()
        {
            return self.fail("Invalid correlated pose Clear acknowledgment");
        }
        let (before_revision, before_epoch) = self.clear_floor.unwrap();
        if barrier.revision <= before_revision || barrier.continuity_epoch <= before_epoch {
            return Ok(false);
        }
        // A continuity control may already have delivered this exact barrier.
        if barrier.revision < self.revision || barrier.continuity_epoch < self.epoch {
            return Ok(false);
        }
        if (barrier.revision == self.revision) != (barrier.continuity_epoch == self.epoch) {
            return self.fail("Contradictory pose Clear acknowledgment identity");
        }
        if barrier.revision == self.revision && self.last_barrier.as_ref() != Some(barrier) {
            return self.fail("Pose Clear acknowledgment contradicts delivered continuity control");
        }
        self.revision = barrier.revision;
        self.epoch = barrier.continuity_epoch;
        self.snapshot = None;
        self.pending_clear = None;
        self.clear_floor = None;
        self.last_barrier = Some(barrier.clone());
        Ok(true)
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests;
