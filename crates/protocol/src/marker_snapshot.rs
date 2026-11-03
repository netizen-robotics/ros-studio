//! Complete owner snapshots plus ordered removal barriers. Not advertised by
//! session13: actor/client integration must negotiate this paired contract.
//! Source actions are assembled upstream; only complete states are coalesced.
use crate::{
    binary,
    latest_pose::LatestPose,
    marker::{
        self,
        store::{Counters, Store},
        Key, Marker,
    },
    spatial,
    subscriptions::*,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const CONTRACT_VERSION: u32 = 1;
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 192 * 1024;
pub const MAX_CHUNKS: usize = MAX_DOCUMENT_BYTES.div_ceil(MAX_CHUNK_BYTES);
pub const MAX_SAMPLE_BYTES: usize = crate::structured::MAX_SAMPLE_BYTES;
pub const MAX_OWNERS: usize = 2;
pub const MAX_HZ: u32 = 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_document_bytes: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks: usize,
    pub max_sample_bytes: usize,
    pub max_owners: usize,
    pub max_markers: usize,
    pub max_points_per_marker: usize,
    pub max_operations: usize,
    pub max_batch_points: usize,
    pub max_retained_points: usize,
    pub max_text_bytes_per_marker: usize,
    pub max_resource_bytes_per_marker: usize,
    pub max_retained_text_bytes: usize,
    pub max_retained_resource_bytes: usize,
    pub max_diagnostics: usize,
    pub supported_kinds: [u32; 12],
    pub max_hz: u32,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_document_bytes: MAX_DOCUMENT_BYTES,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_chunks: MAX_CHUNKS,
            max_sample_bytes: MAX_SAMPLE_BYTES,
            max_owners: MAX_OWNERS,
            max_markers: marker::MAX_MARKERS,
            max_points_per_marker: marker::MAX_POINTS,
            max_operations: marker::MAX_OPERATIONS,
            max_batch_points: marker::MAX_BATCH_POINTS,
            max_retained_points: marker::MAX_RETAINED_POINTS,
            max_text_bytes_per_marker: marker::MAX_TEXT_BYTES,
            max_resource_bytes_per_marker: marker::MAX_RESOURCE_BYTES,
            max_retained_text_bytes: marker::MAX_RETAINED_TEXT_BYTES,
            max_retained_resource_bytes: marker::MAX_RETAINED_RESOURCE_BYTES,
            max_diagnostics: marker::MAX_DIAGNOSTICS,
            supported_kinds: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            max_hz: MAX_HZ,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub message_type: String,
    pub fixed_frame: String,
}
impl Owner {
    pub fn validate(&self) -> Result<(), String> {
        if self.generation.0 == 0
            || self.subscription.0 == 0
            || !valid_topic(&self.topic)
            || !spatial::valid_frame(&self.fixed_frame)
            || !matches!(
                self.message_type.as_str(),
                marker::MESSAGE_TYPE | marker::ARRAY_MESSAGE_TYPE
            )
        {
            return Err("Invalid Marker snapshot owner".into());
        }
        Ok(())
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.generation == other.generation && self.subscription == other.subscription
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Lifetime {
    Forever,
    Expires { remaining_ns: u64, at_unix_ns: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    pub key: Key,
    pub version: u64,
    pub lifetime: Lifetime,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub contract_version: u32,
    pub owner: Owner,
    /// Strictly increasing transfer/control identity, not a source DDS sequence.
    pub revision: u64,
    pub source_revision: u64,
    pub captured_at_unix_ns: u64,
    #[serde(deserialize_with = "crate::serde_bounded::entries")]
    pub manifest: Vec<ManifestEntry>,
    pub document_bytes: usize,
    pub sha256: String,
}
fn sorted_unique<'a>(keys: impl IntoIterator<Item = &'a Key>) -> bool {
    let mut previous = None;
    for key in keys {
        if previous.is_some_and(|p| p >= key) {
            return false;
        }
        previous = Some(key);
    }
    true
}
impl Header {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || self.captured_at_unix_ns == 0
            || self.captured_at_unix_ns > i64::MAX as u64
            || self.document_bytes == 0
            || self.document_bytes > MAX_DOCUMENT_BYTES
            || !binary::valid_sha256(&self.sha256)
            || self.manifest.len() > marker::MAX_MARKERS
            || !sorted_unique(self.manifest.iter().map(|e| &e.key))
        {
            return Err("Invalid Marker snapshot header".into());
        }
        for entry in &self.manifest {
            entry.key.validate().map_err(|e| e.to_string())?;
            if entry.version == 0 || entry.version > self.source_revision {
                return Err("Invalid Marker entry version".into());
            }
            if let Lifetime::Expires {
                remaining_ns,
                at_unix_ns,
            } = entry.lifetime
            {
                if remaining_ns == 0
                    || remaining_ns > marker::MAX_LIFETIME_NS
                    || self.captured_at_unix_ns.checked_add(remaining_ns) != Some(at_unix_ns)
                {
                    return Err("Invalid Marker lifetime manifest".into());
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transform {
    Pending,
    Resolved { pose: LatestPose },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub marker: Marker,
    pub version: u64,
    pub transform: Transform,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub key: Key,
    pub reason: marker::Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LossSupport {
    Unknown,
    Unsupported,
    Supported,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    #[serde(deserialize_with = "crate::serde_bounded::entries")]
    pub entries: Vec<Entry>,
    #[serde(deserialize_with = "crate::serde_bounded::diagnostics")]
    pub diagnostics: Vec<Diagnostic>,
    pub counters: Counters,
    pub source_counters: spatial::Counters,
    /// False is not evidence of a complete Marker source history.
    pub known_history_gap: bool,
    pub loss_support: LossSupport,
}
impl State {
    pub fn validate(&self, header: &Header) -> Result<(), String> {
        header.validate()?;
        if self.entries.len() != header.manifest.len()
            || self.diagnostics.len() > marker::MAX_DIAGNOSTICS
            || !sorted_unique(self.diagnostics.iter().map(|d| &d.key))
        {
            return Err("Invalid Marker state shape/diagnostics".into());
        }
        let (mut points, mut text, mut resources) = (0, 0, 0);
        for (entry, binding) in self.entries.iter().zip(&header.manifest) {
            entry.marker.validate().map_err(|e| e.to_string())?;
            if entry.marker.key != binding.key
                || entry.version != binding.version
                || matches!(entry.marker.geometry, marker::Geometry::ArrowStrip { .. })
            {
                return Err("Marker state contradicts manifest/supported kinds".into());
            }
            match (&binding.lifetime, entry.marker.lifetime_ns) {
                (Lifetime::Forever, 0) => (),
                (Lifetime::Expires { remaining_ns, .. }, total) if *remaining_ns <= total => (),
                _ => return Err("Marker lifetime contradicts source".into()),
            }
            if let Transform::Resolved { pose } = &entry.transform {
                marker::validate_transform(entry.marker.lookup_time(), pose)
                    .map_err(|e| e.to_string())?;
            }
            points += entry.marker.geometry.points().len();
            text += entry.marker.geometry.text_bytes();
            resources += entry.marker.geometry.resource_bytes();
        }
        for diagnostic in &self.diagnostics {
            diagnostic.key.validate().map_err(|e| e.to_string())?;
        }
        if points > marker::MAX_RETAINED_POINTS
            || text > marker::MAX_RETAINED_TEXT_BYTES
            || resources > marker::MAX_RETAINED_RESOURCE_BYTES
        {
            return Err("Marker state exceeds aggregate limits".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub header: Header,
    pub index: usize,
    pub count: usize,
    pub data_base64: String,
}
impl Chunk {
    fn raw_length(&self) -> Result<usize, String> {
        self.header.validate()?;
        binary::chunk_length(
            self.header.document_bytes,
            self.index,
            self.count,
            MAX_DOCUMENT_BYTES,
            MAX_CHUNK_BYTES,
        )
    }
    pub fn validate(&self) -> Result<(), String> {
        binary::validate_exact(&self.data_base64, self.raw_length()?, MAX_CHUNK_BYTES)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prune {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    #[serde(deserialize_with = "crate::serde_bounded::entries")]
    pub keys: Vec<Key>,
}
impl Prune {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || self.keys.is_empty()
            || self.keys.len() > marker::MAX_MARKERS
            || !sorted_unique(&self.keys)
        {
            return Err("Invalid Marker Prune".into());
        }
        for key in &self.keys {
            key.validate().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearReason {
    LocalClear,
    SourceFault,
    Waiting,
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clear {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    pub reason: ClearReason,
    pub known_history_gap: bool,
}
impl Clear {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || (self.reason == ClearReason::SourceFault && !self.known_history_gap)
        {
            return Err("Invalid Marker Clear".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Chunk(Box<Chunk>),
    Prune(Box<Prune>),
    Clear(Box<Clear>),
}
impl Message {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Chunk(c) => c.validate(),
            Self::Prune(p) => p.validate(),
            Self::Clear(c) => c.validate(),
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Marker message exceeds512KiB".into());
        }
        let message: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        message.validate()?;
        Ok(message)
    }
    pub fn owner_revision(&self) -> (&Owner, u64) {
        match self {
            Self::Chunk(c) => (&c.header.owner, c.header.revision),
            Self::Prune(p) => (&p.owner, p.revision),
            Self::Clear(c) => (&c.owner, c.revision),
        }
    }
}

/// Immutable bounded bytes, producing only the next admitted outbound chunk.
pub struct Snapshot {
    header: Header,
    bytes: Box<[u8]>,
}
impl Snapshot {
    pub fn new(mut header: Header, state: State) -> Result<Self, String> {
        // Length/hash are computed locally; callers cannot smuggle a mismatch.
        header.document_bytes = 1;
        header.sha256 = "0".repeat(64);
        state.validate(&header)?;
        let mut bytes = crate::session::encode_line(&state, MAX_DOCUMENT_BYTES)?;
        bytes.pop();
        header.document_bytes = bytes.len();
        header.sha256 = binary::sha256(&bytes);
        header.validate()?;
        Ok(Self {
            header,
            bytes: bytes.into_boxed_slice(),
        })
    }
    pub fn from_store(
        owner: Owner,
        revision: u64,
        store: &Store,
        now: Clock,
        loss_support: LossSupport,
        source_counters: spatial::Counters,
    ) -> Result<Self, String> {
        now.validate()?;
        if now.monotonic_ns < store.observed_time_ns() {
            return Err("Marker snapshot precedes the store clock".into());
        }
        let token = store.token().ok_or("Marker store is stopped")?;
        if token.generation != owner.generation.0
            || token.subscription != owner.subscription.0
            || store.fixed_frame() != Some(owner.fixed_frame.as_str())
        {
            return Err("Marker store contradicts snapshot owner".into());
        }
        let mut manifest = Vec::new();
        let mut entries = Vec::new();
        for (key, entry) in store.entries() {
            let lifetime = match entry.expires_at_ns {
                Some(deadline) if deadline <= now.monotonic_ns => continue,
                Some(deadline) => {
                    let remaining_ns = deadline - now.monotonic_ns;
                    Lifetime::Expires {
                        remaining_ns,
                        at_unix_ns: now
                            .unix_ns
                            .checked_add(remaining_ns)
                            .ok_or("Marker expiry overflow")?,
                    }
                }
                None => Lifetime::Forever,
            };
            manifest.push(ManifestEntry {
                key: key.clone(),
                version: entry.version,
                lifetime,
            });
            entries.push(Entry {
                marker: (*entry.marker).clone(),
                version: entry.version,
                transform: entry
                    .transform
                    .clone()
                    .map_or(Transform::Pending, |pose| Transform::Resolved { pose }),
            });
        }
        Self::new(
            Header {
                contract_version: CONTRACT_VERSION,
                owner,
                revision,
                source_revision: store.revision(),
                captured_at_unix_ns: now.unix_ns,
                manifest,
                document_bytes: 0,
                sha256: String::new(),
            },
            State {
                entries,
                diagnostics: store
                    .diagnostics()
                    .iter()
                    .map(|(key, reason)| Diagnostic {
                        key: key.clone(),
                        reason: *reason,
                    })
                    .collect(),
                counters: store.counters(),
                source_counters,
                known_history_gap: store.incomplete_history(),
                loss_support,
            },
        )
    }
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn document_bytes(&self) -> usize {
        self.bytes.len()
    }
    pub fn chunk_count(&self) -> usize {
        self.bytes.len().div_ceil(MAX_CHUNK_BYTES)
    }
    pub fn chunk(&self, index: usize) -> Option<Chunk> {
        if index >= self.chunk_count() {
            return None;
        }
        let start = index * MAX_CHUNK_BYTES;
        Some(Chunk {
            header: self.header.clone(),
            index,
            count: self.chunk_count(),
            data_base64: binary::encode(
                &self.bytes[start..(start + MAX_CHUNK_BYTES).min(self.bytes.len())],
            ),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub unix_ns: u64,
    pub monotonic_ns: u64,
}
impl Clock {
    fn validate(self) -> Result<(), String> {
        if self.unix_ns == 0
            || self.unix_ns > i64::MAX as u64
            || self.monotonic_ns > u64::MAX - marker::MAX_LIFETIME_NS
        {
            return Err("Invalid Marker same-host System/monotonic clock".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct LiveEntry {
    pub marker: Arc<Marker>,
    pub version: u64,
    pub transform: Transform,
    pub expires_at_monotonic_ns: Option<u64>,
}
impl LiveEntry {
    pub fn visible_at(&self, now_monotonic_ns: u64) -> bool {
        self.expires_at_monotonic_ns
            .is_none_or(|deadline| now_monotonic_ns < deadline)
            && matches!(
                self.transform,
                Transform::Resolved {
                    pose: LatestPose::Ready { .. }
                }
            )
    }
}
#[derive(Clone, Debug)]
pub struct Complete {
    header: Header,
    entries: Vec<LiveEntry>,
    pub diagnostics: Vec<Diagnostic>,
    pub counters: Counters,
    pub source_counters: spatial::Counters,
    pub known_history_gap: bool,
    pub loss_support: LossSupport,
}
impl Complete {
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Diagnostic source entries include hidden/expired items. Renderers must
    /// use visible_entries and re-evaluate monotonic expiry on each frame.
    pub fn entries(&self) -> &[LiveEntry] {
        &self.entries
    }
    pub fn visible_entries(&self, now_monotonic_ns: u64) -> impl Iterator<Item = &LiveEntry> {
        self.entries
            .iter()
            .filter(move |e| e.visible_at(now_monotonic_ns))
    }
}
#[derive(Debug)]
pub enum Received {
    Stale,
    Pending,
    Complete { revision: u64, state: Arc<Complete> },
    Cleared(Clear),
}
struct Partial {
    header: Header,
    next: usize,
    bytes: Vec<u8>,
    digest: Sha256,
}
/// Same-host verified System clock only. Receiver clocks are local observations,
/// never serialized monotonic origins. Faults latch until a new activation.
pub struct Receiver {
    owner: Owner,
    partial: Option<Partial>,
    complete: Option<Arc<Complete>>,
    snapshot_revision: u64,
    control_revision: u64,
    clear_revision: u64,
    prunes: BTreeMap<Key, u64>,
    deadlines: BTreeMap<Key, (u64, Option<u64>)>,
    clock: Option<Clock>,
    faulted: bool,
    stopped: bool,
}
impl Receiver {
    pub fn new(owner: Owner) -> Result<Self, String> {
        owner.validate()?;
        Ok(Self {
            owner,
            partial: None,
            complete: None,
            snapshot_revision: 0,
            control_revision: 0,
            clear_revision: 0,
            prunes: BTreeMap::new(),
            deadlines: BTreeMap::new(),
            clock: None,
            faulted: false,
            stopped: false,
        })
    }
    pub fn in_flight_bytes(&self) -> usize {
        self.partial.as_ref().map_or(0, |p| p.bytes.capacity())
    }
    pub fn retained_prune_count(&self) -> usize {
        self.prunes.len()
    }
    pub fn retained_deadline_count(&self) -> usize {
        self.deadlines.len()
    }
    fn release(&mut self) {
        self.partial = None;
        self.complete = None;
        self.prunes.clear();
        self.deadlines.clear();
    }
    pub fn stop(&mut self) {
        self.stopped = true;
        self.release();
    }
    fn clock(&mut self, now: Clock) -> Result<(), String> {
        now.validate()?;
        if self
            .clock
            .is_some_and(|old| now.unix_ns < old.unix_ns || now.monotonic_ns < old.monotonic_ns)
        {
            return Err("Marker receiver clock regressed; reactivation required".into());
        }
        self.clock = Some(now);
        Ok(())
    }
    fn retained_keys(&self) -> BTreeSet<Key> {
        self.complete
            .iter()
            .flat_map(|c| &c.header.manifest)
            .chain(self.partial.iter().flat_map(|p| &p.header.manifest))
            .map(|e| e.key.clone())
            .collect()
    }
    fn compact(&mut self) {
        let keys = self.retained_keys();
        self.prunes.retain(|key, _| keys.contains(key));
        self.deadlines.retain(|key, _| keys.contains(key));
        debug_assert!(
            self.prunes.len() <= marker::MAX_MARKERS * 2
                && self.deadlines.len() <= marker::MAX_MARKERS * 2
        );
    }
    pub fn receive(&mut self, message: &Message, now: Clock) -> Result<Received, String> {
        let (owner, _) = message.owner_revision();
        if self.stopped || !self.owner.same_identity(owner) {
            return Ok(Received::Stale);
        }
        if self.faulted {
            return Err("Marker receiver faulted; new activation required".into());
        }
        let result = (|| {
            if owner != &self.owner {
                return Err("Marker owner topic/type/frame contradiction".into());
            }
            self.clock(now)?;
            match message {
                Message::Chunk(c) => self.chunk(c, now),
                Message::Prune(p) => self.prune(p),
                Message::Clear(c) => self.clear(c),
            }
        })();
        if result.is_err() {
            self.faulted = true;
            self.release();
        }
        result
    }
    fn chunk(&mut self, chunk: &Chunk, now: Clock) -> Result<Received, String> {
        let revision = chunk.header.revision;
        if revision == 0 {
            return Err("Marker revision must be positive".into());
        }
        if revision <= self.clear_revision
            || revision < self.snapshot_revision
            || (revision == self.snapshot_revision && self.partial.is_none())
        {
            return Ok(Received::Stale);
        }
        if revision > self.snapshot_revision && revision <= self.control_revision {
            return Ok(Received::Stale);
        }
        chunk.validate()?;
        if chunk.header.captured_at_unix_ns > now.unix_ns {
            return Err("Marker snapshot capture is in the future; clock recovery required".into());
        }
        if revision > self.snapshot_revision {
            if chunk.index != 0 {
                return Err("New Marker snapshot must begin at chunk zero".into());
            }
            self.partial = None; // release superseded assembly before allocation
            self.snapshot_revision = revision;
            let mut retained = self.retained_keys();
            retained.extend(chunk.header.manifest.iter().map(|entry| entry.key.clone()));
            self.deadlines.retain(|key, _| retained.contains(key));
            for binding in &chunk.header.manifest {
                let mut deadline = match binding.lifetime {
                    Lifetime::Forever => None,
                    Lifetime::Expires {
                        remaining_ns,
                        at_unix_ns,
                    } => Some(
                        now.monotonic_ns + remaining_ns.min(at_unix_ns.saturating_sub(now.unix_ns)),
                    ),
                };
                if let Some((version, old)) = self.deadlines.get(&binding.key) {
                    if *version == binding.version {
                        if let (Some(old), Some(new)) = (old, deadline) {
                            deadline = Some((*old).min(new));
                        }
                        if old.is_some() != deadline.is_some() {
                            return Err("Unchanged Marker version changed lifetime policy".into());
                        }
                    }
                }
                self.deadlines
                    .insert(binding.key.clone(), (binding.version, deadline));
            }
            self.partial = Some(Partial {
                header: chunk.header.clone(),
                next: 0,
                bytes: Vec::with_capacity(chunk.header.document_bytes),
                digest: Sha256::new(),
            });
            self.compact();
        }
        let partial = self.partial.as_mut().ok_or("Missing Marker transfer")?;
        if partial.header != chunk.header || partial.next != chunk.index {
            return Err("Marker chunks changed, repeated or arrived out of order".into());
        }
        let bytes = binary::decode_exact(&chunk.data_base64, chunk.raw_length()?, MAX_CHUNK_BYTES)?;
        partial.digest.update(&bytes);
        partial.bytes.extend_from_slice(&bytes);
        partial.next += 1;
        if partial.next != chunk.count {
            return Ok(Received::Pending);
        }
        let partial = self.partial.take().expect("complete Marker transfer");
        if partial.bytes.len() != partial.header.document_bytes
            || format!("{:x}", partial.digest.finalize()) != partial.header.sha256
        {
            return Err("Marker snapshot length or SHA-256 mismatch".into());
        }
        let state: State = serde_json::from_slice(&partial.bytes).map_err(|e| e.to_string())?;
        state.validate(&partial.header)?;
        let entries = state
            .entries
            .into_iter()
            .filter(|e| {
                self.prunes
                    .get(&e.marker.key)
                    .is_none_or(|floor| *floor < revision)
            })
            .map(|e| {
                let deadline = self
                    .deadlines
                    .get(&e.marker.key)
                    .expect("manifest deadline")
                    .1;
                LiveEntry {
                    marker: Arc::new(e.marker),
                    version: e.version,
                    transform: e.transform,
                    expires_at_monotonic_ns: deadline,
                }
            })
            .collect();
        self.prunes.retain(|_, floor| *floor > revision);
        let complete = Arc::new(Complete {
            header: partial.header,
            entries,
            diagnostics: state.diagnostics,
            counters: state.counters,
            source_counters: state.source_counters,
            known_history_gap: state.known_history_gap,
            loss_support: state.loss_support,
        });
        self.complete = Some(Arc::clone(&complete));
        self.compact();
        Ok(Received::Complete {
            revision: revision.max(self.control_revision),
            state: complete,
        })
    }
    fn prune(&mut self, prune: &Prune) -> Result<Received, String> {
        if prune.revision == 0 {
            return Err("Marker Prune revision must be positive".into());
        }
        if prune.revision <= self.clear_revision
            || prune.revision <= self.control_revision
            || prune.revision < self.snapshot_revision
        {
            return Ok(Received::Stale);
        }
        prune.validate()?;
        if prune.revision == self.snapshot_revision {
            return Err("Marker Prune reused a snapshot revision".into());
        }
        self.control_revision = prune.revision;
        let keys = self.retained_keys();
        for key in &prune.keys {
            if keys.contains(key) {
                self.prunes.insert(key.clone(), prune.revision);
            }
        }
        self.compact();
        if let Some(old) = &self.complete {
            let mut state = (**old).clone();
            state
                .entries
                .retain(|e| !prune.keys.contains(&e.marker.key));
            let state = Arc::new(state);
            self.complete = Some(Arc::clone(&state));
            Ok(Received::Complete {
                revision: prune.revision,
                state,
            })
        } else {
            Ok(Received::Pending)
        }
    }
    fn clear(&mut self, clear: &Clear) -> Result<Received, String> {
        if clear.revision == 0 {
            return Err("Marker Clear revision must be positive".into());
        }
        if clear.revision <= self.clear_revision
            || clear.revision <= self.control_revision
            || clear.revision < self.snapshot_revision
        {
            return Ok(Received::Stale);
        }
        clear.validate()?;
        if clear.revision == self.snapshot_revision {
            return Err("Marker Clear reused a snapshot revision".into());
        }
        self.clear_revision = clear.revision;
        self.control_revision = clear.revision;
        self.release();
        Ok(Received::Cleared(clear.clone()))
    }
}
