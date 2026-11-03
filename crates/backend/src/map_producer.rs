//! Worker-owned map/TF/snapshot producer. No reader, session or UI admission.
//! Call poll only after reserving one fair outbound slot; it emits at most one
//! bounded message and never stores a queue of chunks.
use crate::{
    map_ingress::{MapIngress, Reader},
    native_topic::Publication,
    spatial_session::{SpatialSession, Status as TfStatus},
};
use std::time::{Duration, Instant};
use studio_protocol::{
    map_grid::{self, Apply, Grid, Invalidation, Patch, Token},
    map_snapshot::{self, Clear, ClearReason, Message, Owner, Snapshot, SourceStatus},
    mapping, spatial,
};

type Hidden = (ClearReason, Option<map_grid::Metadata>, SourceStatus);
pub struct MapProducer {
    owner: Owner,
    token: Option<Token>,
    ingress: MapIngress,
    sequence: u64,
    revision: u64,
    snapshot: Option<Snapshot>,
    next_chunk: usize,
    pending: bool,
    began: Instant,
    attempted: Option<Instant>,
    hidden: Option<Hidden>,
    last_delivered: Option<Instant>,
    support: [Option<bool>; 2],
    counters: mapping::Counters,
}
impl MapProducer {
    pub fn new(
        owner: Owner,
        updates: bool,
        support: [Option<bool>; 2],
        now: Instant,
    ) -> Result<Self, String> {
        owner.validate()?;
        let mut ingress = MapIngress::default();
        let token = ingress.start(updates)?;
        SourceStatus {
            continuity: ingress.continuity(),
            full_loss_events: support[0],
            update_loss_events: support[1],
            ..Default::default()
        }
        .validate()?;
        Ok(Self {
            owner,
            token: Some(token),
            ingress,
            sequence: 0,
            revision: 0,
            snapshot: None,
            next_chunk: 0,
            pending: false,
            began: now,
            attempted: None,
            hidden: None,
            last_delivered: None,
            support,
            counters: Default::default(),
        })
    }
    pub fn token(&self) -> Option<Token> {
        self.token
    }
    pub fn source(&self) -> &MapIngress {
        &self.ingress
    }
    /// Requested cell bytes only, excluding native/RMW, metadata, allocator,
    /// caller-owned incoming data, chunk encoding, IPC, client and GPU storage.
    pub fn cell_bytes(&self) -> (usize, usize) {
        (
            self.ingress.assembly().cell_bytes(),
            self.snapshot.as_ref().map_or(0, Snapshot::cell_bytes),
        )
    }
    pub fn stop(&mut self) {
        self.token = None;
        self.ingress.stop();
        self.snapshot = None;
        self.pending = false;
        self.hidden = None;
        self.last_delivered = None;
    }
    fn current(&mut self, token: Token, sequence: u64, message: bool) -> bool {
        if self.token != Some(token) || sequence <= self.sequence {
            return false;
        }
        self.sequence = sequence;
        if message {
            self.counters.messages_received = self.counters.messages_received.saturating_add(1);
        }
        true
    }
    fn changed(&mut self, now: Instant) {
        self.counters.pending_replaced = self
            .counters
            .pending_replaced
            .saturating_add(u64::from(self.pending || self.snapshot.is_some()));
        // Drop an old immutable delivery before any replacement copy is made.
        self.snapshot = None;
        self.next_chunk = 0;
        self.attempted = None;
        self.began = now;
        self.pending = self.ingress.assembly().snapshot().is_some();
    }
    pub fn full(
        &mut self,
        token: Token,
        sequence: u64,
        grid: Grid,
        now: Instant,
    ) -> Result<Apply, String> {
        let frame_changed = self
            .ingress
            .assembly()
            .snapshot()
            .is_some_and(|old| old.metadata.frame_id != grid.metadata.frame_id);
        let current = self.current(token, sequence, true);
        let result = self.ingress.full(token, sequence, grid);
        if current && (result.is_err() || matches!(result, Ok(Apply::Updated { .. }))) {
            self.changed(now);
            if frame_changed {
                self.last_delivered = None;
            }
        }
        result
    }
    pub fn patch(
        &mut self,
        token: Token,
        sequence: u64,
        patch: Patch,
        publication: Publication,
        now: Instant,
    ) -> Result<Apply, String> {
        let current = self.current(token, sequence, true);
        let result = self.ingress.patch(token, sequence, patch, publication);
        if current && !matches!(result, Ok(Apply::Stale | Apply::Unchanged)) {
            self.changed(now);
        }
        result
    }
    pub fn fault(&mut self, token: Token, sequence: u64, reason: Invalidation, now: Instant) {
        if self.current(token, sequence, false) && self.ingress.fault(token, sequence, reason) {
            self.changed(now);
        }
    }
    pub fn loss(
        &mut self,
        token: Token,
        sequence: u64,
        reader: Reader,
        total: Option<u64>,
        now: Instant,
    ) {
        if !self.current(token, sequence, false) {
            return;
        }
        let before = self.ingress.assembly().status();
        self.ingress.loss(token, sequence, reader, total);
        if self.ingress.assembly().status() != before {
            self.changed(now);
        }
    }
    fn source_status(&self) -> SourceStatus {
        SourceStatus {
            continuity: self.ingress.continuity(),
            full_loss_events: self.support[0],
            update_loss_events: self.support[1],
            ingress_counters: self.ingress.counters(),
            recovery_floor_ns: self.ingress.recovery_floor(),
        }
    }
    fn next_revision(&mut self) -> Result<u64, String> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("Map delivery revision exhausted; restart required")?;
        Ok(self.revision)
    }
    fn hide(
        &mut self,
        reason: ClearReason,
        metadata: Option<map_grid::Metadata>,
    ) -> Result<Option<Message>, String> {
        self.snapshot = None;
        let hidden = (reason, metadata, self.source_status());
        self.last_delivered = None;
        if self.hidden.as_ref() == Some(&hidden) {
            return Ok(None);
        }
        let clear = Clear {
            contract_version: map_snapshot::CONTRACT_VERSION,
            owner: self.owner.clone(),
            revision: self.next_revision()?,
            reason: hidden.0,
            metadata: hidden.1.clone(),
            source_status: hidden.2.clone(),
        };
        clear.validate()?;
        self.hidden = Some(hidden);
        Ok(Some(Message::Clear(Box::new(clear))))
    }
    pub fn poll(
        &mut self,
        tf: &mut SpatialSession,
        now: Instant,
        shared: &spatial::Counters,
    ) -> Result<Option<Message>, String> {
        if self.token.is_none() {
            return Ok(None);
        }
        if !tf.is_active() {
            return Err("Map producer requires active VerifiedSystem TF".into());
        }
        if let map_grid::Status::WaitingFull(reason) = self.ingress.assembly().status() {
            let reason = match reason {
                None => ClearReason::WaitingFull,
                Some(Invalidation::InvalidInput) => ClearReason::InvalidInput,
                Some(Invalidation::QueueOverflow) => ClearReason::QueueOverflow,
                Some(Invalidation::SourceFailure) => ClearReason::SourceFailure,
                Some(Invalidation::Clear) => ClearReason::Clear,
            };
            return self.hide(reason, None);
        }
        if self.pending
            && self.attempted.is_none_or(|last| {
                now.saturating_duration_since(last)
                    >= Duration::from_millis(spatial::RETRY_INTERVAL_MS)
            })
        {
            self.attempted = Some(now);
            let metadata = self
                .ingress
                .assembly()
                .snapshot()
                .ok_or("Map source cache missing")?
                .metadata
                .clone();
            let result = if metadata.frame_id == self.owner.fixed_frame {
                Ok([0., 0., 0., 0., 0., 0., 1.])
            } else {
                tf.lookup_at(
                    &self.owner.fixed_frame,
                    &metadata.frame_id,
                    metadata.stamp_ns,
                )
                .map(|ready| ready.components)
                .map_err(|error| match error.status {
                    TfStatus::Missing | TfStatus::Stale => ClearReason::MissingTransform,
                    TfStatus::Extrapolated => ClearReason::Extrapolated,
                    _ => ClearReason::InvalidTransform,
                })
            };
            match result {
                Err(reason) => {
                    let limit = Duration::from_millis(spatial::PENDING_TIMEOUT_MS);
                    let delivered_age = self
                        .last_delivered
                        .map(|last| now.saturating_duration_since(last));
                    let timed = now.saturating_duration_since(self.began) >= limit
                        || delivered_age.is_some_and(|age| age >= limit);
                    // A new map can arrive just ahead of its TF sample. Keep the
                    // previous fully transformed snapshot while retrying, never
                    // project new cells with its old transform. Continuous input
                    // cannot extend this bound by resetting `began` on every map.
                    if !timed
                        && delivered_age.is_some()
                        && matches!(
                            reason,
                            ClearReason::MissingTransform | ClearReason::Extrapolated
                        )
                    {
                        return Ok(None);
                    }
                    let reason = if timed {
                        ClearReason::LookupTimeout
                    } else {
                        reason
                    };
                    if timed
                        && !self
                            .hidden
                            .as_ref()
                            .is_some_and(|v| v.0 == ClearReason::LookupTimeout)
                    {
                        self.counters.lookup_timeouts =
                            self.counters.lookup_timeouts.saturating_add(1);
                    }
                    return self.hide(reason, Some(metadata));
                }
                Ok(transform) => {
                    let valid = mapping::map_corners(
                        metadata.width,
                        metadata.height,
                        metadata.resolution,
                        &metadata.origin,
                    )
                    .is_some_and(|corners| {
                        corners
                            .into_iter()
                            .flat_map(|p| mapping::transform_point(&transform, p))
                            .all(|v| v.is_finite() && v.abs() <= mapping::MAX_COORDINATE)
                    });
                    if !valid {
                        return self.hide(ClearReason::InvalidTransform, Some(metadata));
                    }
                    self.snapshot = None;
                    let revision = self.next_revision()?;
                    let source = self
                        .ingress
                        .assembly()
                        .snapshot()
                        .ok_or("Map source cache missing")?;
                    let copy = Grid {
                        metadata: source.metadata.clone(),
                        cells: source.cells.to_vec().into_boxed_slice(),
                    };
                    let counters = mapping::Counters {
                        tf_dynamic_batches: shared.tf_dynamic_batches,
                        tf_static_batches: shared.tf_static_batches,
                        tf_accepted: shared.tf_accepted,
                        tf_rejected: shared.tf_rejected,
                        duplicate_statics: shared.duplicate_statics,
                        native_errors: shared.native_errors,
                        ..self.counters
                    };
                    self.snapshot = Some(Snapshot::new(
                        self.owner.clone(),
                        revision,
                        copy,
                        transform,
                        counters,
                        self.ingress.assembly().counters(),
                        self.source_status(),
                    )?);
                    self.next_chunk = 0;
                    self.pending = false;
                    self.hidden = None;
                }
            }
        }
        let Some(snapshot) = &self.snapshot else {
            return Ok(None);
        };
        let chunk = snapshot
            .chunk(self.next_chunk)
            .ok_or("Map chunk cursor invalid")?;
        self.next_chunk += 1;
        if self.next_chunk == snapshot.chunk_count() {
            self.snapshot = None;
            self.last_delivered = Some(now);
        }
        Ok(Some(Message::Chunk(Box::new(chunk))))
    }
}

#[cfg(test)]
mod tests;
