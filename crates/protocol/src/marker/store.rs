//! Single-owner bounded state. Time is caller-supplied monotonic nanoseconds;
//! callbacks must use the exact activation/entry/request identity they received.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static ACTIVATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub generation: u64,
    pub subscription: u64,
    activation: u64,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub marker: Arc<Marker>,
    pub version: u64,
    pub received_at_ns: u64,
    pub expires_at_ns: Option<u64>,
    /// None is pending. Only Ready entries may produce visible geometry.
    pub transform: Option<LatestPose>,
    next_lookup_ns: u64,
    pending: Option<(u64, u64)>, // request, deadline
}
#[derive(Clone, Debug)]
pub struct TransformRequest {
    pub token: Token,
    pub key: Key,
    pub entry_version: u64,
    pub request_id: u64,
    pub source_frame: String,
    pub fixed_frame: String,
    pub time: LookupTime,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub batches: u64,
    pub adds: u64,
    pub deletes: u64,
    pub expired: u64,
    pub rejected: u64,
    pub duplicate_keys: u64,
    pub stale: u64,
    pub source_gaps: u64,
    pub source_faults: u64,
    pub lookup_timeouts: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    pub points: usize,
    pub text_bytes: usize,
    pub resource_bytes: usize,
}
impl Budget {
    fn add(&mut self, marker: &Marker) {
        self.points += marker.geometry.points().len();
        self.text_bytes += marker.geometry.text_bytes();
        self.resource_bytes += marker.geometry.resource_bytes();
    }
    fn remove(&mut self, marker: &Marker) {
        self.points -= marker.geometry.points().len();
        self.text_bytes -= marker.geometry.text_bytes();
        self.resource_bytes -= marker.geometry.resource_bytes();
    }
    fn validate(&self) -> Result<(), Error> {
        if self.points > MAX_RETAINED_POINTS {
            Err(Error::PointLimit)
        } else if self.text_bytes > MAX_RETAINED_TEXT_BYTES {
            Err(Error::TextLimit)
        } else if self.resource_bytes > MAX_RETAINED_RESOURCE_BYTES {
            Err(Error::ResourceLimit)
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
pub struct Store {
    token: Option<Token>,
    fixed_frame: String,
    sequence: u64,
    revision: u64,
    request: u64,
    last_now_ns: u64,
    entries: BTreeMap<Key, Entry>,
    diagnostics: BTreeMap<Key, Error>,
    budget: Budget,
    counters: Counters,
    incomplete_history: bool,
}
impl Store {
    pub fn start(
        &mut self,
        generation: u64,
        subscription: u64,
        fixed_frame: &str,
    ) -> Result<Token, Error> {
        if generation == 0 || subscription == 0 || !spatial::valid_frame(fixed_frame) {
            return Err(Error::InvalidOwner);
        }
        let activation = ACTIVATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .map_err(|_| Error::CounterExhausted)?
            + 1;
        *self = Self::default();
        let token = Token {
            generation,
            subscription,
            activation,
        };
        self.token = Some(token);
        self.fixed_frame = fixed_frame.into();
        Ok(token)
    }
    pub fn token(&self) -> Option<Token> {
        self.token
    }
    pub fn fixed_frame(&self) -> Option<&str> {
        self.token.map(|_| self.fixed_frame.as_str())
    }
    pub fn observed_time_ns(&self) -> u64 {
        self.last_now_ns
    }
    pub fn entries(&self) -> &BTreeMap<Key, Entry> {
        &self.entries
    }
    pub fn diagnostics(&self) -> &BTreeMap<Key, Error> {
        &self.diagnostics
    }
    pub fn budget(&self) -> Budget {
        self.budget
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn incomplete_history(&self) -> bool {
        self.incomplete_history
    }
    fn current(&mut self, token: Token) -> bool {
        if self.token == Some(token) {
            true
        } else {
            self.counters.stale = self.counters.stale.saturating_add(1);
            false
        }
    }
    fn advance(&mut self) -> Result<(), Error> {
        if let Some(revision) = self.revision.checked_add(1) {
            self.revision = revision;
            Ok(())
        } else {
            self.stop();
            Err(Error::CounterExhausted)
        }
    }
    fn clock(&mut self, now: u64) -> Result<(), Error> {
        if now < self.last_now_ns || now > u64::MAX - MAX_LIFETIME_NS {
            self.stop();
            return Err(Error::ClockRegressed);
        }
        self.last_now_ns = now;
        Ok(())
    }
    fn clear_entries(&mut self) {
        self.entries.clear();
        self.budget = Budget::default();
    }
    pub fn stop(&mut self) {
        self.token = None;
        self.clear_entries();
        self.diagnostics.clear();
    }
    /// Local Clear does not replay a subscription or erase known history loss.
    pub fn clear(&mut self, token: Token) -> Result<bool, Error> {
        if !self.current(token) {
            return Ok(false);
        }
        self.clear_entries();
        self.diagnostics.clear();
        self.advance()?;
        Ok(true)
    }
    fn remove(&mut self, key: &Key) {
        if let Some(old) = self.entries.remove(key) {
            self.budget.remove(&old.marker);
        }
    }
    fn diagnostic(&mut self, key: Key, error: Error) {
        if !self.diagnostics.contains_key(&key) && self.diagnostics.len() == MAX_DIAGNOSTICS {
            self.diagnostics.pop_first();
        }
        self.diagnostics.insert(key, error);
        self.counters.rejected = self.counters.rejected.saturating_add(1);
    }
    /// Missing ordered deltas can leave ghosts. Forget only this owner and keep
    /// a visible history-gap diagnostic while subsequent messages rebuild it.
    pub fn source_fault(&mut self, token: Token) -> Result<bool, Error> {
        if !self.current(token) {
            return Ok(false);
        }
        self.clear_entries();
        self.diagnostics.clear();
        self.incomplete_history = true;
        self.counters.source_faults = self.counters.source_faults.saturating_add(1);
        self.advance()?;
        Ok(true)
    }
    pub fn apply(
        &mut self,
        token: Token,
        sequence: u64,
        batch: Batch,
        now: u64,
    ) -> Result<bool, Error> {
        if !self.current(token) {
            return Ok(false);
        }
        if sequence == 0 || sequence <= self.sequence {
            self.counters.stale = self.counters.stale.saturating_add(1);
            return Ok(false);
        }
        self.clock(now)?;
        if let Err(error) = batch.validate() {
            self.source_fault(token)?;
            return Err(error);
        }
        if batch.encode().is_err() {
            self.source_fault(token)?;
            return Err(Error::BatchLimit);
        }
        if self.sequence.checked_add(1) != Some(sequence) {
            self.source_fault(token)?;
            self.counters.source_gaps = self.counters.source_gaps.saturating_add(1);
        }
        self.sequence = sequence;
        self.tick(token, now)?;
        self.advance()?;
        self.counters.batches = self.counters.batches.saturating_add(1);
        let mut keys = BTreeSet::new();
        for operation in batch.operations {
            if let Some(key) = operation.key() {
                if !keys.insert(key.clone()) {
                    self.counters.duplicate_keys = self.counters.duplicate_keys.saturating_add(1);
                }
            }
            match operation {
                Operation::Add { marker } => {
                    self.advance()?;
                    let key = marker.key.clone();
                    self.remove(&key);
                    self.diagnostics.remove(&key);
                    let mut budget = self.budget;
                    budget.add(&marker);
                    let error = if self.entries.len() >= MAX_MARKERS {
                        Some(Error::EntryLimit)
                    } else {
                        budget.validate().err()
                    };
                    if let Some(error) = error {
                        self.diagnostic(key, error);
                        continue;
                    }
                    self.budget = budget;
                    let transform =
                        (marker.frame_id == self.fixed_frame).then(|| LatestPose::Ready {
                            resolved_stamp_ns: if marker.lookup_time() == LookupTime::Latest {
                                0
                            } else {
                                marker.stamp_ns
                            },
                            components: [0., 0., 0., 0., 0., 0., 1.],
                        });
                    let expires_at_ns = (marker.lifetime_ns != 0).then(|| now + marker.lifetime_ns);
                    self.entries.insert(
                        key,
                        Entry {
                            marker: Arc::from(marker),
                            version: self.revision,
                            received_at_ns: now,
                            expires_at_ns,
                            transform,
                            next_lookup_ns: now,
                            pending: None,
                        },
                    );
                    self.counters.adds = self.counters.adds.saturating_add(1);
                }
                Operation::Delete { key } => {
                    self.remove(&key);
                    self.diagnostics.remove(&key);
                    self.counters.deletes = self.counters.deletes.saturating_add(1);
                }
                Operation::DeleteAll { namespace } => {
                    let doomed = self
                        .entries
                        .keys()
                        .filter(|k| namespace.is_empty() || k.namespace == namespace)
                        .cloned()
                        .collect::<Vec<_>>();
                    for key in doomed {
                        self.remove(&key);
                        self.counters.deletes = self.counters.deletes.saturating_add(1);
                    }
                    self.diagnostics
                        .retain(|key, _| !namespace.is_empty() && key.namespace != namespace);
                }
                Operation::Rejected { key, reason } => {
                    self.remove(&key);
                    self.diagnostic(key, reason);
                }
            }
        }
        Ok(true)
    }
    pub fn tick(&mut self, token: Token, now: u64) -> Result<bool, Error> {
        if !self.current(token) {
            return Ok(false);
        }
        self.clock(now)?;
        let expired = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at_ns.is_some_and(|t| now >= t))
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        let mut changed = !expired.is_empty();
        for key in expired {
            self.remove(&key);
            self.diagnostics.remove(&key);
            self.counters.expired = self.counters.expired.saturating_add(1);
        }
        for entry in self.entries.values_mut() {
            if entry.pending.is_some_and(|(_, deadline)| now >= deadline) {
                entry.pending = None;
                entry.transform = Some(LatestPose::LookupTimeout);
                self.counters.lookup_timeouts = self.counters.lookup_timeouts.saturating_add(1);
                changed = true;
            }
        }
        if changed {
            self.advance()?;
        }
        Ok(changed)
    }
    pub fn requests(&mut self, token: Token, now: u64) -> Result<Vec<TransformRequest>, Error> {
        if !self.current(token) {
            return Ok(Vec::new());
        }
        self.tick(token, now)?;
        // Reserve enough identity space for a complete bounded request pass.
        // Exhaustion must not leave a partially issued, reusable activation.
        if self.request.checked_add(MAX_MARKERS as u64).is_none() {
            self.stop();
            return Err(Error::CounterExhausted);
        }
        let mut requests = Vec::new();
        for (key, entry) in &mut self.entries {
            if entry.marker.frame_id == self.fixed_frame
                || entry.pending.is_some()
                || now < entry.next_lookup_ns
                || (!entry.marker.frame_locked
                    && matches!(entry.transform, Some(LatestPose::Ready { .. })))
            {
                continue;
            }
            self.request = self.request.checked_add(1).ok_or(Error::CounterExhausted)?;
            entry.pending = Some((self.request, now + LOOKUP_TIMEOUT_NS));
            entry.next_lookup_ns = now + RETRY_NS;
            requests.push(TransformRequest {
                token,
                key: key.clone(),
                entry_version: entry.version,
                request_id: self.request,
                source_frame: entry.marker.frame_id.clone(),
                fixed_frame: self.fixed_frame.clone(),
                time: entry.marker.lookup_time(),
            });
        }
        Ok(requests)
    }
    pub fn complete(
        &mut self,
        request: &TransformRequest,
        transform: LatestPose,
        now: u64,
    ) -> Result<bool, Error> {
        if !self.current(request.token) {
            return Ok(false);
        }
        self.tick(request.token, now)?;
        let Some(entry) = self.entries.get_mut(&request.key) else {
            return Ok(false);
        };
        if entry.version != request.entry_version
            || entry.pending.map(|p| p.0) != Some(request.request_id)
        {
            return Ok(false);
        }
        entry.pending = None;
        // The entry, not caller-modifiable request metadata, owns lookup meaning.
        let valid = validate_transform(entry.marker.lookup_time(), &transform);
        entry.transform = Some(if valid.is_ok() {
            transform
        } else {
            LatestPose::Missing
        });
        if let Err(error) = valid {
            self.diagnostic(request.key.clone(), error);
            self.advance()?;
            return Err(error);
        }
        self.diagnostics.remove(&request.key);
        self.advance()?;
        Ok(true)
    }
}
