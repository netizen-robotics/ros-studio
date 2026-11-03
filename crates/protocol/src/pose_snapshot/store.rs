//! Worker-owned ordered history. No ROS, wall clock, UI or command replay.
use super::*;
use crate::latest_pose::LookupTime;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
};
pub const RETRY_NS: u64 = 50_000_000;
pub const TIMEOUT_NS: u64 = 1_000_000_000;
static ACTIVATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    InvalidInput,
    TimestampConflict,
    ClockRegressed,
    Exhausted,
    InvalidTransform,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "Invalid pose/covariance source; explicitly restart the display",
            Self::TimestampConflict => {
                "Changed equal or decreasing pose acquisition stamp; explicitly restart the display"
            }
            Self::ClockRegressed => {
                "Pose monotonic clock regressed or exhausted; explicitly restart the display"
            }
            Self::Exhausted => "Pose state identities exhausted; explicitly restart the display",
            Self::InvalidTransform => "Invalid pose TF result; explicitly restart the display",
        })
    }
}
impl std::error::Error for Fault {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Updated,
    Duplicate,
    Stale,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    pub anchors: usize,
    pub pending: usize,
    pub head: bool,
    pub duplicate_guard: bool,
    pub resolved_head: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformRequest {
    token: Token,
    epoch: u64,
    sequence: u64,
    id: u64,
    source_frame: String,
    fixed_frame: String,
    time: LookupTime,
}
impl TransformRequest {
    pub fn source_frame(&self) -> &str {
        &self.source_frame
    }
    pub fn fixed_frame(&self) -> &str {
        &self.fixed_frame
    }
    pub fn time(&self) -> LookupTime {
        self.time
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
struct Pending {
    entry: Entry,
    deadline: u64,
    next_lookup: u64,
    request: Option<u64>,
    done: bool,
}
#[derive(Default)]
pub struct Store {
    token: Option<Token>,
    owner: Option<Owner>,
    history: Option<HistorySettings>,
    epoch: u64,
    revision: u64,
    sequence: u64,
    request: u64,
    now: u64,
    source: Option<Source>,
    // Kept across Clear solely to reject/re-ignore same-stamp data, never drawn.
    baseline: Option<Decoded>,
    latest: Option<Entry>,
    // One resolved presentation head, independent of history decimation. This
    // does not change `snapshot()` or the serialized contract.
    resolved_head: Option<(Entry, u64)>,
    trail: VecDeque<Entry>,
    pending: VecDeque<Pending>,
    counters: Counters,
    gap: bool,
    reset: Option<ResetReason>,
    fault: Option<Fault>,
}
impl Store {
    pub fn start(
        &mut self,
        owner: Owner,
        history: Option<HistorySettings>,
    ) -> Result<Token, String> {
        owner.validate()?;
        match (owner.kind, history) {
            (Kind::PoseWithCovariance, None) => (),
            (Kind::Odometry, Some(h)) => h.validate()?,
            _ => return Err("Pose kind and history configuration disagree".into()),
        }
        let nonce = ACTIVATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| Fault::Exhausted.to_string())?
            + 1;
        let token = Token(nonce);
        *self = Self {
            token: Some(token),
            owner: Some(owner),
            history,
            epoch: 1,
            revision: 1,
            ..Self::default()
        };
        Ok(token)
    }
    pub fn token(&self) -> Option<Token> {
        self.token
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn continuity_epoch(&self) -> u64 {
        self.epoch
    }
    pub fn invalidation(&self) -> Option<super::transport::Invalidation> {
        Some(super::transport::Invalidation {
            contract_version: CONTRACT_VERSION,
            owner: self.owner.clone()?,
            revision: self.revision,
            continuity_epoch: self.epoch,
            reason: self.reset?,
        })
    }
    pub fn budget(&self) -> Budget {
        Budget {
            anchors: self.trail.len(),
            pending: self.pending.len(),
            head: self.latest.is_some(),
            duplicate_guard: self.baseline.is_some(),
            resolved_head: self.resolved_head.is_some(),
        }
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        Some(Snapshot {
            contract_version: CONTRACT_VERSION,
            owner: self.owner.clone()?,
            revision: self.revision,
            continuity_epoch: self.epoch,
            source: self.source.clone(),
            history: self.history,
            latest: self.latest.clone(),
            trail: self.trail.iter().cloned().collect(),
            last_reset: self.reset,
            known_history_gap: self.gap,
            counters: self.counters,
        })
    }
    /// Publish the newest resolved pose while a newer input awaits TF. All
    /// pose/covariance/transform fields stay from that same resolved entry.
    pub fn presentation_snapshot(&self, now: u64) -> Option<Snapshot> {
        let mut snapshot = self.snapshot()?;
        let waiting = snapshot.latest.as_ref().is_some_and(|entry| {
            matches!(
                entry.transform,
                LatestPose::Missing | LatestPose::Stale | LatestPose::Extrapolated
            )
        });
        if waiting {
            if let Some((entry, at)) = &self.resolved_head {
                if now.saturating_sub(*at) < TIMEOUT_NS {
                    snapshot.latest = Some(entry.clone());
                }
            }
        }
        Some(snapshot)
    }
    fn resolved(&mut self, entry: Entry, now: u64) {
        if self
            .resolved_head
            .as_ref()
            .is_none_or(|(old, _)| entry.sequence > old.sequence)
        {
            self.resolved_head = Some((entry, now));
        }
    }
    pub fn stop(&mut self) {
        *self = Self::default();
    }
    fn current(&self, token: Token) -> bool {
        self.token == Some(token)
    }
    fn poison(&mut self, fault: Fault) -> Fault {
        self.token = None;
        self.pending.clear();
        self.trail.clear();
        self.latest = None;
        self.resolved_head = None;
        self.baseline = None;
        self.epoch = self.epoch.saturating_add(1);
        self.revision = self.revision.saturating_add(1);
        self.reset = Some(ResetReason::SourceFault);
        self.counters.resets = self.counters.resets.saturating_add(1);
        self.fault = Some(fault);
        fault
    }
    fn touch(&mut self) -> Result<(), Fault> {
        let Some(next) = self.revision.checked_add(1) else {
            return Err(self.poison(Fault::Exhausted));
        };
        self.revision = next;
        Ok(())
    }
    fn clock(&mut self, now: u64) -> Result<(), Fault> {
        if now < self.now || now > u64::MAX - TIMEOUT_NS {
            return Err(self.poison(Fault::ClockRegressed));
        }
        self.now = now;
        Ok(())
    }
    fn reset(&mut self, reason: ResetReason) -> Result<(), Fault> {
        let Some(epoch) = self.epoch.checked_add(1) else {
            return Err(self.poison(Fault::Exhausted));
        };
        self.epoch = epoch;
        self.pending.clear();
        self.trail.clear();
        self.latest = None;
        self.resolved_head = None;
        self.reset = Some(reason);
        self.counters.resets = self.counters.resets.saturating_add(1);
        self.touch()
    }
    pub fn clear(&mut self, token: Token) -> Result<bool, Fault> {
        if !self.current(token) {
            return Ok(false);
        }
        self.reset(ResetReason::LocalClear)?;
        Ok(true)
    }
    pub fn source_gap(&mut self, token: Token) -> Result<bool, Fault> {
        if !self.current(token) {
            return Ok(false);
        }
        self.counters.pending_dropped = self
            .counters
            .pending_dropped
            .saturating_add(self.pending.len() as u64);
        self.counters.known_source_gaps = self.counters.known_source_gaps.saturating_add(1);
        self.gap = true;
        self.reset(ResetReason::SourceGap)?;
        Ok(true)
    }
    pub fn source_fault(&mut self, token: Token) -> bool {
        if !self.current(token) {
            return false;
        }
        self.poison(Fault::InvalidInput);
        true
    }
    pub fn input(
        &mut self,
        token: Token,
        sequence: u64,
        decoded: Decoded,
        now: u64,
    ) -> Result<Input, Fault> {
        if !self.current(token) || (sequence > 0 && sequence <= self.sequence) {
            return Ok(Input::Stale);
        }
        self.tick(token, now)?;
        if sequence == 0
            || decoded.validate().is_err()
            || self
                .owner
                .as_ref()
                .is_none_or(|o| o.kind != decoded.kind || o.topic != decoded.topic)
        {
            return Err(self.poison(Fault::InvalidInput));
        }
        if self.sequence != 0 && self.sequence.checked_add(1) != Some(sequence) {
            self.source_gap(token)?;
        }
        self.sequence = sequence;
        self.counters.received = self.counters.received.saturating_add(1);
        if let Some(previous) = &self.baseline {
            if previous.source != decoded.source {
                self.reset(ResetReason::FrameChanged)?;
            } else if (previous.acquisition_stamp_ns == 0) != (decoded.acquisition_stamp_ns == 0) {
                self.reset(ResetReason::TimePolicyChanged)?;
            } else if decoded.acquisition_stamp_ns > 0
                && decoded.acquisition_stamp_ns <= previous.acquisition_stamp_ns
            {
                if decoded == *previous {
                    self.counters.duplicates = self.counters.duplicates.saturating_add(1);
                    self.touch()?;
                    return Ok(Input::Duplicate);
                }
                return Err(self.poison(Fault::TimestampConflict));
            }
        }
        if decoded.kind == Kind::Odometry && self.pending.len() == MAX_PENDING {
            self.source_gap(token)?;
        }
        if decoded.kind == Kind::PoseWithCovariance {
            self.pending.clear();
        }
        self.source = Some(decoded.source.clone());
        let identity = self.owner.as_ref().unwrap().fixed_frame == decoded.source.frame_id;
        let entry = Entry {
            sequence,
            acquisition_stamp_ns: decoded.acquisition_stamp_ns,
            components: decoded.components,
            covariance: decoded.covariance.clone(),
            transform: if identity {
                LatestPose::Ready {
                    resolved_stamp_ns: decoded.acquisition_stamp_ns,
                    components: [0., 0., 0., 0., 0., 0., 1.],
                }
            } else {
                LatestPose::Missing
            },
        };
        if identity {
            self.resolved(entry.clone(), now);
        }
        self.latest = Some(entry.clone());
        self.baseline = Some(decoded);
        self.pending.push_back(Pending {
            entry,
            deadline: now + TIMEOUT_NS,
            next_lookup: now,
            request: None,
            done: identity,
        });
        self.drain();
        self.touch()?;
        Ok(Input::Updated)
    }
    fn drain(&mut self) -> bool {
        let mut changed = false;
        while self.pending.front().is_some_and(|p| p.done) {
            let p = self.pending.pop_front().unwrap();
            if !matches!(p.entry.transform, LatestPose::Ready { .. }) {
                continue;
            }
            let Some(settings) = self.history else {
                continue;
            };
            if self
                .trail
                .back()
                .is_some_and(|old| !include(settings, &old.components, &p.entry.components))
            {
                self.counters.filtered = self.counters.filtered.saturating_add(1);
                changed = true;
                continue;
            }
            self.trail.push_back(p.entry);
            changed = true;
            while self.trail.len() > settings.keep {
                self.trail.pop_front();
                self.counters.evicted = self.counters.evicted.saturating_add(1);
            }
        }
        changed
    }
    pub fn tick(&mut self, token: Token, now: u64) -> Result<bool, Fault> {
        if !self.current(token) {
            return Ok(false);
        }
        self.clock(now)?;
        let mut changed = false;
        for p in &mut self.pending {
            if !p.done && now >= p.deadline {
                p.done = true;
                p.request = None;
                p.entry.transform = LatestPose::LookupTimeout;
                self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
                self.gap |= self.history.is_some();
                changed = true;
                if self
                    .latest
                    .as_ref()
                    .is_some_and(|e| e.sequence == p.entry.sequence)
                {
                    self.latest = Some(p.entry.clone());
                }
            }
        }
        if changed {
            self.drain();
            self.touch()?;
        }
        Ok(changed)
    }
    pub fn requests(&mut self, token: Token, now: u64) -> Result<Vec<TransformRequest>, Fault> {
        if !self.current(token) {
            return Ok(Vec::new());
        }
        self.tick(token, now)?;
        if self.request.checked_add(MAX_PENDING as u64).is_none() {
            return Err(self.poison(Fault::Exhausted));
        }
        let mut requests = Vec::with_capacity(self.pending.len());
        for p in &mut self.pending {
            if p.done || p.request.is_some() || now < p.next_lookup {
                continue;
            }
            self.request += 1;
            p.request = Some(self.request);
            let owner = self.owner.as_ref().unwrap();
            requests.push(TransformRequest {
                token,
                epoch: self.epoch,
                sequence: p.entry.sequence,
                id: self.request,
                source_frame: self.source.as_ref().unwrap().frame_id.clone(),
                fixed_frame: owner.fixed_frame.clone(),
                time: if p.entry.acquisition_stamp_ns == 0 {
                    LookupTime::Latest
                } else {
                    LookupTime::At(p.entry.acquisition_stamp_ns)
                },
            });
        }
        Ok(requests)
    }
    pub fn complete(
        &mut self,
        request: &TransformRequest,
        transform: LatestPose,
        now: u64,
    ) -> Result<bool, Fault> {
        if !self.current(request.token) || request.epoch != self.epoch {
            return Ok(false);
        }
        let matches =
            |p: &Pending| p.entry.sequence == request.sequence && p.request == Some(request.id);
        if !self.pending.iter().any(matches) {
            return Ok(false);
        }
        self.tick(request.token, now)?;
        let Some(index) = self.pending.iter().position(matches) else {
            return Ok(false);
        };
        if request.time.validate_result(&transform).is_err() {
            return Err(self.poison(Fault::InvalidTransform));
        }
        let p = &mut self.pending[index];
        p.request = None;
        p.next_lookup = now + RETRY_NS;
        p.done = matches!(
            transform,
            LatestPose::Ready { .. } | LatestPose::LookupTimeout
        );
        let timeout = transform == LatestPose::LookupTimeout;
        if timeout {
            self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
            self.gap |= self.history.is_some();
        }
        let head_changed = self
            .latest
            .as_ref()
            .is_some_and(|e| e.sequence == p.entry.sequence && e.transform != transform);
        p.entry.transform = transform;
        if head_changed {
            self.latest = Some(p.entry.clone());
        }
        let resolved =
            matches!(p.entry.transform, LatestPose::Ready { .. }).then(|| p.entry.clone());
        if let Some(entry) = resolved {
            self.resolved(entry, now);
        }
        let anchors_changed = self.drain();
        if head_changed || anchors_changed || timeout {
            self.touch()?;
        }
        Ok(true)
    }
}
fn include(settings: HistorySettings, old: &[f64; 7], new: &[f64; 7]) -> bool {
    let distance = old[..3]
        .iter()
        .zip(&new[..3])
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt();
    if distance >= settings.position_tolerance_m {
        return true;
    }
    let a = old[3..].iter().map(|x| x * x).sum::<f64>().sqrt();
    let b = new[3..].iter().map(|x| x * x).sum::<f64>().sqrt();
    let sign = if old[3..]
        .iter()
        .zip(&new[3..])
        .map(|(x, y)| x * y)
        .sum::<f64>()
        < 0.
    {
        -1.
    } else {
        1.
    };
    let (mut minus, mut plus) = (0., 0.);
    for (x, y) in old[3..].iter().zip(&new[3..]) {
        minus += (x / a - sign * y / b).powi(2);
        plus += (x / a + sign * y / b).powi(2);
    }
    let angle = (4. * minus.sqrt().atan2(plus.sqrt())).min(std::f64::consts::PI);
    angle >= settings.angle_tolerance_rad
}
#[cfg(test)]
mod tests;
