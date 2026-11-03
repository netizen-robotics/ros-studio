//! Ordered continuity and cached-frame revocation, shared by client and renderer.
use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Weak,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_sample_bytes: usize,
    pub max_control_bytes: usize,
    pub max_calibration_bytes: usize,
    pub max_distortion: usize,
    pub max_owners: usize,
    pub max_pending: usize,
    pub max_hz: u32,
    pub pending_timeout_ns: u64,
    pub retry_interval_ns: u64,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_sample_bytes: MAX_SAMPLE_BYTES,
            max_control_bytes: MAX_CONTROL_BYTES,
            max_calibration_bytes: crate::camera_info::MAX_ENCODED_BYTES,
            max_distortion: crate::camera_info::MAX_DISTORTION,
            max_owners: MAX_OWNERS,
            max_pending: 1,
            max_hz: MAX_HZ,
            pending_timeout_ns: TIMEOUT_NS,
            retry_interval_ns: RETRY_NS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invalidation {
    pub owner: Owner,
    pub revision: u64,
    pub continuity_epoch: u64,
}
impl Invalidation {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.revision == 0 || self.continuity_epoch == 0 {
            return Err("Invalid Camera continuity barrier".into());
        }
        Ok(())
    }
}
impl From<&Snapshot> for Invalidation {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            owner: snapshot.owner.clone(),
            revision: snapshot.revision,
            continuity_epoch: snapshot.continuity_epoch,
        }
    }
}
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
            Self::Invalidated(s) => &s.owner,
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let limit = match self {
            Self::Snapshot(s) => {
                s.validate()?;
                MAX_SAMPLE_BYTES
            }
            Self::Invalidated(s) => {
                s.validate()?;
                MAX_CONTROL_BYTES
            }
        };
        encode(self, limit)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Self = decode(bytes, MAX_SAMPLE_BYTES)?;
        value.encode()?;
        Ok(value)
    }
}

#[derive(Clone, Debug)]
pub struct Revocation(Arc<Authority>);
#[derive(Debug, Default)]
struct Authority {
    serial: AtomicU64,
    retired: AtomicBool,
}
impl Revocation {
    pub fn revoke(&self) {
        let _ = self
            .0
            .serial
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_add(1))
            });
    }
    /// A terminal UI Stop/EOF can retire authority without waiting for decoding.
    pub fn retire(&self) {
        self.0.retired.store(true, Ordering::Release);
        self.revoke();
    }
}
#[derive(Clone, Debug)]
pub struct Permission {
    authority: Arc<Authority>,
    serial: u64,
    snapshot: Weak<Snapshot>,
}
impl Permission {
    pub fn is_valid(&self) -> bool {
        !self.authority.retired.load(Ordering::Acquire)
            && self.authority.serial.load(Ordering::Acquire) == self.serial
    }
    pub fn authorizes(&self, snapshot: &Arc<Snapshot>) -> bool {
        self.is_valid()
            && self
                .snapshot
                .upgrade()
                .is_some_and(|s| Arc::ptr_eq(&s, snapshot))
    }
}
pub struct Accepted {
    pub snapshot: Arc<Snapshot>,
    pub permission: Permission,
}
pub struct Receiver {
    owner: Owner,
    active: bool,
    revision: u64,
    epoch: u64,
    barrier: Option<Invalidation>,
    current: Option<Arc<Snapshot>>,
    authority: Arc<Authority>,
    last_image: u64,
    last_calibration: u64,
}
impl Receiver {
    pub fn new(owner: Owner) -> Result<Self, String> {
        owner.validate()?;
        Ok(Self {
            owner,
            active: true,
            revision: 0,
            epoch: 0,
            barrier: None,
            current: None,
            authority: Arc::new(Authority::default()),
            last_image: 0,
            last_calibration: 0,
        })
    }
    pub fn revocation(&self) -> Revocation {
        Revocation(self.authority.clone())
    }
    pub fn stop(&mut self) {
        self.active = false;
        self.revocation().retire();
        self.current = None;
    }
    pub fn invalidate(&mut self, barrier: &Invalidation) -> Result<bool, String> {
        let result = self.invalidate_inner(barrier);
        if result.is_err() {
            self.stop();
        }
        result
    }
    fn invalidate_inner(&mut self, barrier: &Invalidation) -> Result<bool, String> {
        if !self.active || self.authority.retired.load(Ordering::Acquire) {
            return Ok(false);
        }
        barrier.validate()?;
        if barrier.owner != self.owner {
            return Err("Camera barrier owner contradicts binding".into());
        }
        if barrier.continuity_epoch < self.epoch || barrier.revision < self.revision {
            return Ok(false);
        }
        if self.barrier.as_ref() == Some(barrier) {
            return Ok(false);
        }
        if barrier.continuity_epoch == self.epoch && barrier.revision <= self.revision {
            return Ok(false);
        }
        if barrier.continuity_epoch <= self.epoch || barrier.revision <= self.revision {
            return Err("Camera barrier did not advance ordered continuity".into());
        }
        self.revocation().revoke();
        self.current = None;
        self.revision = barrier.revision;
        self.epoch = barrier.continuity_epoch;
        self.barrier = Some(barrier.clone());
        Ok(true)
    }
    pub fn apply(&mut self, snapshot: Snapshot) -> Result<Option<Accepted>, String> {
        let result = self.apply_inner(snapshot);
        if result.is_err() {
            self.stop();
        }
        result
    }
    fn apply_inner(&mut self, snapshot: Snapshot) -> Result<Option<Accepted>, String> {
        if !self.active || self.authority.retired.load(Ordering::Acquire) {
            return Ok(None);
        }
        snapshot.validate()?;
        if snapshot.owner != self.owner {
            return Err("Camera snapshot owner contradicts binding".into());
        }
        if snapshot.continuity_epoch < self.epoch || snapshot.revision < self.revision {
            return Ok(None);
        }
        if self.epoch != 0 && snapshot.continuity_epoch != self.epoch {
            return Err("Camera snapshot bypassed ordered invalidation".into());
        }
        if snapshot.revision == self.revision && self.barrier.is_none() {
            if self.current.as_deref() == Some(&snapshot) {
                return Ok(None);
            }
            return Err("Changed Camera snapshot reused a revision".into());
        }
        if let Some(old) = self.current.as_ref().and_then(|s| s.frame.as_ref()) {
            let Some(frame) = &snapshot.frame else {
                return Err("Camera frame removal bypassed invalidation".into());
            };
            if frame.calibration_revision != old.calibration_revision
                || !frame.calibration.same_parameters(&old.calibration)
            {
                return Err("Changed Camera calibration bypassed invalidation".into());
            }
        }
        if let Some(frame) = &snapshot.frame {
            if frame.image_sequence < self.last_image
                || frame.calibration_revision < self.last_calibration
                || (frame.image_sequence == self.last_image
                    && self.current.as_ref().and_then(|s| s.frame.as_ref()) != Some(frame))
            {
                return Err("Camera frame reused an obsolete Image or calibration identity".into());
            }
            self.last_image = frame.image_sequence;
            self.last_calibration = frame.calibration_revision;
        }
        // A same-continuity frame is a prepared replacement, not an
        // invalidation. Keep the displayed captured Image/TF pair usable while
        // the worker decodes and the UI takes the next result. Permissions stay
        // bound to their exact snapshot; ordered barriers, faults and Stop
        // still revoke all frames immediately through this shared authority.
        let serial = self.authority.serial.load(Ordering::Acquire);
        if serial == u64::MAX {
            return Err("Camera authority exhausted".into());
        }
        self.revision = snapshot.revision;
        self.epoch = snapshot.continuity_epoch;
        self.barrier = None;
        let snapshot = Arc::new(snapshot);
        let permission = Permission {
            authority: self.authority.clone(),
            serial,
            snapshot: Arc::downgrade(&snapshot),
        };
        self.current = Some(snapshot.clone());
        Ok(Some(Accepted {
            snapshot,
            permission,
        }))
    }
}
