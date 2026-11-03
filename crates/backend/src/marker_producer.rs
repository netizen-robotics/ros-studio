//! Worker-owned ordered Marker state and fair full-state transfer. Call poll
//! only after reserving one outbound slot; keep feeding inputs/TF/tick while
//! no slot is available. No native handles, session admission or UI live here.
use std::collections::{BTreeMap, BTreeSet};
use studio_protocol::{
    latest_pose::LatestPose,
    marker::{
        self,
        store::{Store, Token, TransformRequest},
        Batch, Key,
    },
    marker_snapshot::{
        self as wire, Clear, ClearReason, Clock, LossSupport, Message, Owner, Prune, Snapshot,
    },
    spatial,
};
pub struct MarkerProducer {
    owner: Owner,
    store: Store,
    token: Token,
    loss_support: LossSupport,
    revision: u64,
    snapshot: Option<Snapshot>,
    next_chunk: usize,
    last_sent_source: Option<u64>,
    last_sent_keys: BTreeSet<Key>,
    previous: BTreeMap<Key, (u64, bool)>,
    observed_revision: u64,
    prunes: BTreeSet<Key>,
    clear: Option<ClearReason>,
    next_capture_ns: u64,
    stopped: bool,
    now_ns: u64,
    waiting_replacements: BTreeMap<Key, u64>,
}
impl MarkerProducer {
    pub fn new(owner: Owner, loss_support: LossSupport) -> Result<Self, String> {
        owner.validate()?;
        let mut store = Store::default();
        let token = store
            .start(owner.generation.0, owner.subscription.0, &owner.fixed_frame)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            owner,
            store,
            token,
            loss_support,
            revision: 0,
            snapshot: None,
            next_chunk: 0,
            last_sent_source: Some(0),
            last_sent_keys: BTreeSet::new(),
            previous: BTreeMap::new(),
            observed_revision: 0,
            prunes: BTreeSet::new(),
            clear: Some(ClearReason::Waiting),
            next_capture_ns: 0,
            stopped: false,
            now_ns: 0,
            waiting_replacements: BTreeMap::new(),
        })
    }
    pub fn source(&self) -> &Store {
        &self.store
    }
    pub fn token(&self) -> Token {
        self.token
    }
    pub fn in_flight_bytes(&self) -> usize {
        self.snapshot.as_ref().map_or(0, Snapshot::document_bytes)
    }
    pub fn pending_prunes(&self) -> usize {
        self.prunes.len()
    }
    fn revision(&mut self) -> Result<u64, String> {
        self.revision = match self.revision.checked_add(1) {
            Some(revision) => revision,
            None => {
                self.stop();
                return Err("Marker delivery revision exhausted".into());
            }
        };
        Ok(self.revision)
    }
    fn relevant(&self) -> BTreeSet<Key> {
        self.last_sent_keys
            .iter()
            .cloned()
            .chain(
                self.snapshot
                    .iter()
                    .flat_map(|s| s.header().manifest.iter().map(|e| e.key.clone())),
            )
            .collect()
    }
    fn refresh(&mut self) {
        if self.observed_revision == self.store.revision() {
            return;
        }
        self.observed_revision = self.store.revision();
        let current = self
            .store
            .entries()
            .iter()
            .map(|(key, e)| {
                (
                    key.clone(),
                    (
                        e.version,
                        matches!(e.transform, Some(LatestPose::Ready { .. })),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.waiting_replacements.retain(|key, _| {
            self.store.entries().get(key).is_some_and(|entry| {
                matches!(
                    entry.transform,
                    None | Some(LatestPose::Missing | LatestPose::Stale | LatestPose::Extrapolated)
                )
            })
        });
        if self.clear.is_none() {
            let relevant = self.relevant();
            self.prunes.retain(|key| relevant.contains(key));
            for (key, (old_version, was_ready)) in &self.previous {
                let removed = match current.get(key) {
                    None => true,
                    Some((version, ready)) => !*ready && (*was_ready || *old_version != *version),
                };
                if removed && relevant.contains(key) {
                    let transient = self.store.entries().get(key).is_some_and(|entry| {
                        matches!(
                            entry.transform,
                            None | Some(
                                LatestPose::Missing | LatestPose::Stale | LatestPose::Extrapolated
                            )
                        )
                    });
                    if transient && *was_ready {
                        self.waiting_replacements
                            .entry(key.clone())
                            .or_insert(self.now_ns.saturating_add(marker::LOOKUP_TIMEOUT_NS));
                    }
                    if !self.waiting_replacements.contains_key(key) {
                        self.prunes.insert(key.clone());
                    }
                }
            }
            debug_assert!(self.prunes.len() <= marker::MAX_MARKERS * 2);
        }
        self.previous = current;
    }
    fn clear_with(&mut self, reason: ClearReason) {
        self.snapshot = None;
        self.next_chunk = 0;
        self.last_sent_keys.clear();
        self.prunes.clear();
        self.clear = Some(reason);
        self.last_sent_source = None;
        self.waiting_replacements.clear();
    }
    pub fn input(
        &mut self,
        sequence: u64,
        batch: Batch,
        now_monotonic_ns: u64,
    ) -> Result<bool, String> {
        if self.stopped {
            return Ok(false);
        }
        self.now_ns = now_monotonic_ns;
        let faults = self.store.counters().source_faults;
        let result = self
            .store
            .apply(self.token, sequence, batch, now_monotonic_ns);
        if self.store.token().is_none() {
            self.stop();
            return result.map_err(|e| e.to_string());
        }
        if self.store.counters().source_faults != faults {
            self.clear_with(ClearReason::SourceFault);
        }
        self.refresh();
        result.map_err(|e| e.to_string())
    }
    pub fn requests(&mut self, now_monotonic_ns: u64) -> Result<Vec<TransformRequest>, String> {
        if self.stopped {
            return Ok(Vec::new());
        }
        self.now_ns = now_monotonic_ns;
        let result = self.store.requests(self.token, now_monotonic_ns);
        if self.store.token().is_none() {
            self.stop();
            return result.map_err(|e| e.to_string());
        }
        self.refresh();
        result.map_err(|e| e.to_string())
    }
    pub fn complete(
        &mut self,
        request: &TransformRequest,
        pose: LatestPose,
        now_monotonic_ns: u64,
    ) -> Result<bool, String> {
        if self.stopped {
            return Ok(false);
        }
        self.now_ns = now_monotonic_ns;
        let result = self.store.complete(request, pose, now_monotonic_ns);
        if self.store.token().is_none() {
            self.stop();
            return result.map_err(|e| e.to_string());
        }
        self.refresh();
        result.map_err(|e| e.to_string())
    }
    pub fn tick(&mut self, now_monotonic_ns: u64) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.now_ns = now_monotonic_ns;
        if let Err(error) = self.store.tick(self.token, now_monotonic_ns) {
            self.stop();
            return Err(error.to_string());
        }
        self.refresh();
        let expired = self
            .waiting_replacements
            .iter()
            .filter(|(_, deadline)| now_monotonic_ns >= **deadline)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired {
            self.waiting_replacements.remove(&key);
            self.prunes.insert(key);
        }
        Ok(())
    }
    pub fn source_fault(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.store
            .source_fault(self.token)
            .map_err(|e| e.to_string())?;
        self.clear_with(ClearReason::SourceFault);
        self.refresh();
        Ok(())
    }
    pub fn local_clear(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.store.clear(self.token).map_err(|e| e.to_string())?;
        self.clear_with(ClearReason::LocalClear);
        self.refresh();
        Ok(())
    }
    pub fn stop(&mut self) {
        self.stopped = true;
        self.store.stop();
        self.snapshot = None;
        self.previous.clear();
        self.last_sent_keys.clear();
        self.prunes.clear();
        self.clear = None;
        self.waiting_replacements.clear();
    }
    pub fn poll(
        &mut self,
        now: Clock,
        counters: spatial::Counters,
    ) -> Result<Option<Message>, String> {
        if self.stopped {
            return Ok(None);
        }
        self.tick(now.monotonic_ns)?;
        if let Some(reason) = self.clear.take() {
            let revision = self.revision()?;
            return Ok(Some(Message::Clear(Box::new(Clear {
                contract_version: wire::CONTRACT_VERSION,
                owner: self.owner.clone(),
                revision,
                reason,
                known_history_gap: self.store.incomplete_history(),
            }))));
        }
        if !self.prunes.is_empty() {
            let mut keys = Vec::new();
            while keys.len() < marker::MAX_MARKERS {
                let Some(key) = self.prunes.pop_first() else {
                    break;
                };
                keys.push(key);
            }
            let revision = self.revision()?;
            return Ok(Some(Message::Prune(Box::new(Prune {
                contract_version: wire::CONTRACT_VERSION,
                owner: self.owner.clone(),
                revision,
                keys,
            }))));
        }
        // Deletions, expiry and fault controls above never wait for TF. Only a
        // replacement snapshot is deferred; old cells/markers never borrow TF.
        if !self.waiting_replacements.is_empty() {
            return Ok(None);
        }
        if self.snapshot.is_none() {
            if self.last_sent_source == Some(self.store.revision())
                || now.monotonic_ns < self.next_capture_ns
            {
                return Ok(None);
            }
            let revision = self.revision()?;
            self.snapshot = Some(Snapshot::from_store(
                self.owner.clone(),
                revision,
                &self.store,
                now,
                self.loss_support.clone(),
                counters,
            )?);
            self.next_chunk = 0;
        }
        let snapshot = self.snapshot.as_ref().expect("Marker transfer");
        let chunk = snapshot
            .chunk(self.next_chunk)
            .ok_or("Missing Marker chunk")?;
        self.next_chunk += 1;
        if self.next_chunk == snapshot.chunk_count() {
            self.last_sent_source = Some(snapshot.header().source_revision);
            self.last_sent_keys = snapshot
                .header()
                .manifest
                .iter()
                .map(|e| e.key.clone())
                .collect();
            self.snapshot = None;
            self.next_chunk = 0;
            self.next_capture_ns = now
                .monotonic_ns
                .checked_add(1_000_000_000 / u64::from(wire::MAX_HZ))
                .ok_or("Marker capture time exhausted")?;
        }
        Ok(Some(Message::Chunk(Box::new(chunk))))
    }
}
