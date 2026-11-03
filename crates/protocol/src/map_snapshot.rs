//! Strict large-map snapshot transfer, awaiting observation-session negotiation.
//! Ordered patches are applied upstream; only complete map revisions travel here.
use crate::{binary, map_grid, mapping, spatial, subscriptions::*};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONTRACT_VERSION: u32 = 2;
pub const MAX_CHUNK_BYTES: usize = 192 * 1024;
pub const MAX_CHUNKS: usize = map_grid::MAX_CELLS.div_ceil(MAX_CHUNK_BYTES);
pub const MAX_SAMPLE_BYTES: usize = crate::structured::MAX_SAMPLE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_dimension: u32,
    pub max_cells: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks: usize,
    pub max_sample_bytes: usize,
    pub max_layers: usize,
    pub max_update_readers_per_map: usize,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_dimension: map_grid::MAX_DIMENSION,
            max_cells: map_grid::MAX_CELLS,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_chunks: MAX_CHUNKS,
            max_sample_bytes: MAX_SAMPLE_BYTES,
            max_layers: mapping::MAX_MAP_LAYERS,
            max_update_readers_per_map: 1,
        }
    }
}

// Nullable fields still have to be present in this strict paired grammar.
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStatus {
    pub continuity: map_grid::Continuity,
    /// None means not observed/configured; false means explicitly unsupported.
    #[serde(deserialize_with = "required_option")]
    pub full_loss_events: Option<bool>,
    #[serde(deserialize_with = "required_option")]
    pub update_loss_events: Option<bool>,
    pub ingress_counters: map_grid::IngressCounters,
    #[serde(deserialize_with = "required_option")]
    pub recovery_floor_ns: Option<i64>,
}
impl SourceStatus {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .recovery_floor_ns
            .is_some_and(|stamp| !(1..=mapping::MAX_STAMP_NS).contains(&stamp))
            || (self.continuity == map_grid::Continuity::Disabled
                && self.update_loss_events.is_some())
        {
            return Err("Invalid map continuity diagnostics".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub fixed_frame: String,
}
impl Owner {
    pub fn validate(&self) -> Result<(), String> {
        if self.generation.0 == 0
            || self.subscription.0 == 0
            || !valid_topic(&self.topic)
            || !spatial::valid_frame(&self.fixed_frame)
        {
            return Err("Invalid map snapshot owner".into());
        }
        Ok(())
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.generation == other.generation && self.subscription == other.subscription
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub contract_version: u32,
    pub owner: Owner,
    /// Delivery epoch, advanced for both complete snapshots and Clear controls.
    pub revision: u64,
    pub metadata: map_grid::Metadata,
    /// Ready acquisition-time transform from source frame to fixed frame.
    pub transform: [f64; 7],
    pub source_counters: mapping::Counters,
    pub assembly_counters: map_grid::Counters,
    pub source_status: SourceStatus,
    pub sha256: String,
}
impl Header {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        self.metadata.validate().map_err(str::to_owned)?;
        self.source_status.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || !binary::valid_sha256(&self.sha256)
            || !mapping::valid_pose(&self.transform)
            || self.source_status.recovery_floor_ns.is_some()
        {
            return Err("Invalid map snapshot version, revision, digest or transform".into());
        }
        let corners = mapping::map_corners(
            self.metadata.width,
            self.metadata.height,
            self.metadata.resolution,
            &self.metadata.origin,
        )
        .ok_or("Invalid map source corners")?;
        if corners
            .into_iter()
            .flat_map(|p| mapping::transform_point(&self.transform, p))
            .any(|v| !v.is_finite() || v.abs() > mapping::MAX_COORDINATE)
        {
            return Err("Map transformed corners exceed world bounds".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
            self.header.metadata.cell_count().map_err(str::to_owned)?,
            self.index,
            self.count,
            map_grid::MAX_CELLS,
            MAX_CHUNK_BYTES,
        )
    }
    pub fn validate(&self) -> Result<(), String> {
        binary::validate_exact(&self.data_base64, self.raw_length()?, MAX_CHUNK_BYTES)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Map snapshot chunk exceeds sample budget".into());
        }
        let chunk: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        chunk.validate()?;
        Ok(chunk)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearReason {
    WaitingFull,
    InvalidInput,
    QueueOverflow,
    SourceFailure,
    MissingTransform,
    InvalidTransform,
    Extrapolated,
    LookupTimeout,
    Clear,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clear {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    pub reason: ClearReason,
    /// TF-only hiding retains source metadata for useful waiting diagnostics.
    #[serde(deserialize_with = "required_option")]
    pub metadata: Option<map_grid::Metadata>,
    pub source_status: SourceStatus,
}
impl Clear {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        self.source_status.validate()?;
        if self.contract_version != CONTRACT_VERSION || self.revision == 0 {
            return Err("Invalid map Clear version or revision".into());
        }
        let tf_wait = matches!(
            self.reason,
            ClearReason::MissingTransform
                | ClearReason::InvalidTransform
                | ClearReason::Extrapolated
                | ClearReason::LookupTimeout
        );
        if tf_wait != self.metadata.is_some()
            || (tf_wait && self.source_status.recovery_floor_ns.is_some())
        {
            return Err("TF hiding requires source metadata; invalidated maps retain none".into());
        }
        if let Some(metadata) = &self.metadata {
            metadata.validate().map_err(str::to_owned)?;
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Map Clear exceeds sample budget".into());
        }
        let clear: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        clear.validate()?;
        Ok(clear)
    }
}

/// One strict discriminant for data versus invalidation on a map output stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Message {
    Chunk(Box<Chunk>),
    Clear(Box<Clear>),
}
impl Message {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Chunk(chunk) => chunk.validate(),
            Self::Clear(clear) => clear.validate(),
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
            return Err("Map message exceeds sample budget".into());
        }
        let message: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        message.validate()?;
        Ok(message)
    }
}

/// Immutable producer; chunk generation is demand-driven after queue admission.
pub struct Snapshot {
    header: Header,
    cells: Box<[i8]>,
}
impl Snapshot {
    pub fn new(
        owner: Owner,
        revision: u64,
        grid: map_grid::Grid,
        transform: [f64; 7],
        source_counters: mapping::Counters,
        assembly_counters: map_grid::Counters,
        source_status: SourceStatus,
    ) -> Result<Self, String> {
        grid.validate().map_err(str::to_owned)?;
        let header = Header {
            contract_version: CONTRACT_VERSION,
            owner,
            revision,
            sha256: binary::signed_sha256(&grid.cells),
            metadata: grid.metadata,
            transform,
            source_counters,
            assembly_counters,
            source_status,
        };
        header.validate()?;
        Ok(Self {
            header,
            cells: grid.cells,
        })
    }
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn cell_bytes(&self) -> usize {
        self.cells.len()
    }
    pub fn chunk_count(&self) -> usize {
        self.cells.len().div_ceil(MAX_CHUNK_BYTES)
    }
    pub fn chunk(&self, index: usize) -> Option<Chunk> {
        if index >= self.chunk_count() {
            return None;
        }
        let offset = index * MAX_CHUNK_BYTES;
        let raw: Vec<u8> = self.cells[offset..(offset + MAX_CHUNK_BYTES).min(self.cells.len())]
            .iter()
            .map(|cell| *cell as u8)
            .collect();
        Some(Chunk {
            header: self.header.clone(),
            index,
            count: self.chunk_count(),
            data_base64: binary::encode(&raw),
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct Complete {
    pub header: Header,
    pub grid: map_grid::Grid,
}
#[derive(Debug, PartialEq)]
pub enum Received {
    Stale,
    Pending,
    Complete(Box<Complete>),
    Cleared(Box<Clear>),
}
struct Partial {
    header: Header,
    next: usize,
    cells: Vec<i8>,
    digest: Sha256,
}
/// One exact owner; callers retain only complete output. Pending progress must
/// not replace visible geometry. Error/Clear must clear caller-held geometry.
/// Faults latch until a fresh receiver activation; Stop ignores all later work.
pub struct Receiver {
    owner: Owner,
    revision: u64,
    partial: Option<Partial>,
    faulted: bool,
    stopped: bool,
}
impl Receiver {
    pub fn new(owner: Owner) -> Result<Self, String> {
        owner.validate()?;
        Ok(Self {
            owner,
            revision: 0,
            partial: None,
            faulted: false,
            stopped: false,
        })
    }
    pub fn in_flight_cell_bytes(&self) -> usize {
        self.partial.as_ref().map_or(0, |p| p.cells.capacity())
    }
    pub fn stop(&mut self) {
        self.stopped = true;
        self.partial = None;
    }
    pub fn receive(&mut self, message: &Message) -> Result<Received, String> {
        match message {
            Message::Chunk(chunk) => self.push(chunk),
            Message::Clear(clear) => self.clear(clear),
        }
    }
    fn finish_result(&mut self, result: Result<Received, String>) -> Result<Received, String> {
        if result.is_err() {
            self.faulted = true;
            self.partial = None;
        }
        result
    }
    pub fn push(&mut self, chunk: &Chunk) -> Result<Received, String> {
        let result = self.accept_chunk(chunk);
        self.finish_result(result)
    }
    fn accept_chunk(&mut self, chunk: &Chunk) -> Result<Received, String> {
        if self.stopped || !self.owner.same_identity(&chunk.header.owner) {
            return Ok(Received::Stale);
        }
        if self.faulted {
            return Err("Map receiver faulted; new activation required".into());
        }
        if chunk.header.revision == 0 {
            return Err("Map revision must be positive".into());
        }
        if chunk.header.revision < self.revision
            || (chunk.header.revision == self.revision && self.partial.is_none())
        {
            return Ok(Received::Stale);
        }
        chunk.validate()?;
        if chunk.header.owner != self.owner {
            return Err("Map topic/fixed frame contradicts owner".into());
        }
        if chunk.header.revision > self.revision {
            if chunk.index != 0 {
                return Err("New map revision must begin at chunk zero".into());
            }
            self.partial = None; // Release superseded storage before reserving replacement.
            self.revision = chunk.header.revision;
            self.partial = Some(Partial {
                header: chunk.header.clone(),
                next: 0,
                cells: Vec::with_capacity(
                    chunk.header.metadata.cell_count().map_err(str::to_owned)?,
                ),
                digest: Sha256::new(),
            });
        }
        let partial = self.partial.as_mut().ok_or("Missing map transfer")?;
        if partial.header != chunk.header || partial.next != chunk.index {
            return Err("Map chunk metadata changed, repeated or arrived out of order".into());
        }
        let raw = binary::decode_exact(&chunk.data_base64, chunk.raw_length()?, MAX_CHUNK_BYTES)?;
        partial.digest.update(&raw);
        partial.cells.extend(raw.iter().map(|cell| *cell as i8));
        partial.next += 1;
        if partial.next != chunk.count {
            return Ok(Received::Pending);
        }
        let partial = self.partial.take().expect("complete map transfer");
        if partial.cells.len()
            != partial
                .header
                .metadata
                .cell_count()
                .map_err(str::to_owned)?
            || format!("{:x}", partial.digest.finalize()) != partial.header.sha256
        {
            return Err("Map snapshot length or SHA-256 mismatch".into());
        }
        Ok(Received::Complete(Box::new(Complete {
            grid: map_grid::Grid {
                metadata: partial.header.metadata.clone(),
                cells: partial.cells.into_boxed_slice(),
            },
            header: partial.header,
        })))
    }
    pub fn clear(&mut self, clear: &Clear) -> Result<Received, String> {
        let result = self.accept_clear(clear);
        self.finish_result(result)
    }
    fn accept_clear(&mut self, clear: &Clear) -> Result<Received, String> {
        if self.stopped || !self.owner.same_identity(&clear.owner) {
            return Ok(Received::Stale);
        }
        if self.faulted {
            return Err("Map receiver faulted; new activation required".into());
        }
        if clear.revision == 0 {
            return Err("Map revision must be positive".into());
        }
        if clear.revision < self.revision
            || (clear.revision == self.revision && self.partial.is_none())
        {
            return Ok(Received::Stale);
        }
        clear.validate()?;
        if clear.owner != self.owner || clear.revision == self.revision {
            return Err("Map Clear contradicts owner or does not advance revision".into());
        }
        self.revision = clear.revision;
        self.partial = None;
        Ok(Received::Cleared(Box::new(clear.clone())))
    }
}
