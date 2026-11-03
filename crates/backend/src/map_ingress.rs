//! Ordered, worker-local source assembly. This module opens no ROS endpoints and
//! emits no wire events. Apply every input here before coalescing snapshots.
use crate::native_topic::Publication;
use studio_protocol::map_grid::{Apply, Assembly, Grid, Invalidation, Patch, Status, Token};

pub use studio_protocol::map_grid::{Continuity, IngressCounters as Counters};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    Full,
    Updates,
}

/// One raw map (at most4MiB), one bounded publisher identity, scalar counters.
/// The caller additionally owns at most one native incoming message/copy.
#[derive(Default)]
pub struct MapIngress {
    assembly: Assembly,
    owner: Option<Token>,
    sequence: u64,
    updates: bool,
    publication: Option<Publication>,
    continuity: Continuity,
    lost_totals: [u64; 2],
    latest_stamp: Option<i64>,
    recovery_floor: Option<i64>,
    counters: Counters,
}
impl MapIngress {
    pub fn start(&mut self, updates: bool) -> Result<Token, String> {
        self.stop();
        let owner = self.assembly.start().map_err(str::to_owned)?;
        self.owner = Some(owner);
        self.updates = updates;
        self.continuity = if updates {
            Continuity::Unknown
        } else {
            Continuity::Disabled
        };
        Ok(owner)
    }
    pub fn stop(&mut self) {
        *self = Self::default();
    }
    pub fn assembly(&self) -> &Assembly {
        &self.assembly
    }
    pub fn continuity(&self) -> Continuity {
        self.continuity
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn recovery_floor(&self) -> Option<i64> {
        self.recovery_floor
    }
    fn accept(&mut self, owner: Token, sequence: u64) -> bool {
        if self.owner != Some(owner) || sequence <= self.sequence {
            self.counters.stale_events = self.counters.stale_events.saturating_add(1);
            return false;
        }
        self.sequence = sequence;
        true
    }
    fn invalidate(&mut self, owner: Token, reason: Invalidation) {
        self.assembly.invalidate(owner, reason);
        self.recovery_floor = self.latest_stamp;
    }
    fn observed_stamp(&mut self, stamp: i64) {
        self.latest_stamp = Some(self.latest_stamp.map_or(stamp, |last| last.max(stamp)));
        if matches!(self.assembly.status(), Status::WaitingFull(Some(_))) {
            self.recovery_floor = self.latest_stamp;
        }
    }
    pub fn fault(&mut self, owner: Token, sequence: u64, reason: Invalidation) -> bool {
        if !self.accept(owner, sequence) {
            return false;
        }
        self.invalidate(owner, reason);
        true
    }
    /// Call for observed cumulative middleware loss. None means unavailable;
    /// zero must never be interpreted as proof of complete delivery.
    pub fn loss(
        &mut self,
        owner: Token,
        sequence: u64,
        reader: Reader,
        total: Option<u64>,
    ) -> bool {
        if !self.accept(owner, sequence) {
            return false;
        }
        let Some(total) = total else {
            return true;
        };
        let index = match reader {
            Reader::Full => 0,
            Reader::Updates => 1,
        };
        let previous = self.lost_totals[index];
        if total < previous {
            self.counters.loss_counter_resets = self.counters.loss_counter_resets.saturating_add(1);
            self.invalidate(owner, Invalidation::SourceFailure);
        } else if total > previous {
            self.counters.reported_lost =
                self.counters.reported_lost.saturating_add(total - previous);
            self.invalidate(owner, Invalidation::QueueOverflow);
        }
        self.lost_totals[index] = total;
        true
    }
    pub fn full(&mut self, owner: Token, sequence: u64, grid: Grid) -> Result<Apply, String> {
        if !self.accept(owner, sequence) {
            return Ok(Apply::Stale);
        }
        if let Err(error) = grid.validate() {
            self.invalidate(owner, Invalidation::InvalidInput);
            return Err(error.into());
        }
        let stamp = grid.metadata.stamp_ns;
        if self.recovery_floor.is_some_and(|floor| stamp < floor) {
            self.counters.rejected_recovery_full =
                self.counters.rejected_recovery_full.saturating_add(1);
            return Ok(Apply::Stale);
        }
        let result = self
            .assembly
            .full(owner, sequence, grid)
            .map_err(str::to_owned);
        match &result {
            Ok(Apply::Updated { .. } | Apply::Unchanged) => {
                self.observed_stamp(stamp);
                self.recovery_floor = None;
            }
            Err(_) => self.recovery_floor = self.latest_stamp,
            _ => {}
        }
        result
    }
    pub fn patch(
        &mut self,
        owner: Token,
        sequence: u64,
        patch: Patch,
        publication: Publication,
    ) -> Result<Apply, String> {
        if !self.accept(owner, sequence) {
            return Ok(Apply::Stale);
        }
        if !self.updates {
            self.invalidate(owner, Invalidation::SourceFailure);
            return Err("Updates are not configured for this owner".into());
        }
        if publication.publisher_id.is_empty()
            || publication.publisher_id.len() > 128
            || !publication.publisher_id.len().is_multiple_of(2)
            || !publication
                .publisher_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || publication.sequence == Some(u64::MAX)
        {
            self.invalidate(owner, Invalidation::SourceFailure);
            return Err("Invalid bounded update publication metadata".into());
        }
        self.continuity = Continuity::Unknown;
        if let Some(previous) = &self.publication {
            if previous.publisher_id != publication.publisher_id {
                self.counters.publisher_changes = self.counters.publisher_changes.saturating_add(1);
                self.invalidate(owner, Invalidation::SourceFailure);
            } else if let (Some(before), Some(after)) = (previous.sequence, publication.sequence) {
                self.continuity = Continuity::SequenceAvailable;
                if before.checked_add(1) != Some(after) {
                    self.counters.sequence_faults = self.counters.sequence_faults.saturating_add(1);
                    self.invalidate(owner, Invalidation::QueueOverflow);
                }
            }
        }
        self.publication = Some(publication);
        if patch.validate().is_ok() {
            self.observed_stamp(patch.stamp_ns);
        }
        let result = self
            .assembly
            .patch(owner, sequence, patch)
            .map_err(str::to_owned);
        if result.is_err() {
            self.recovery_floor = self.latest_stamp;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::map_grid::Metadata;
    fn full(stamp: i64) -> Grid {
        Grid {
            metadata: Metadata {
                frame_id: "map".into(),
                stamp_ns: stamp,
                width: 2,
                height: 2,
                resolution: 0.5,
                origin: [0., 0., 0., 0., 0., 0., 1.],
                map_load_time_ns: 0,
            },
            cells: vec![0; 4].into_boxed_slice(),
        }
    }
    fn patch(x: i32, value: i8, stamp: i64) -> Patch {
        Patch {
            frame_id: "map".into(),
            stamp_ns: stamp,
            x,
            y: 0,
            width: 1,
            height: 2,
            cells: vec![value; 2].into_boxed_slice(),
        }
    }
    fn publication(sequence: Option<u64>) -> Publication {
        Publication {
            publisher_id: "aa".repeat(16),
            sequence,
        }
    }
    #[test]
    fn all_rows_apply_before_snapshot_coalescing_and_known_gap_requires_new_full() {
        let mut state = MapIngress::default();
        let owner = state.start(true).unwrap();
        state.full(owner, 1, full(10)).unwrap();
        state
            .patch(owner, 2, patch(0, 3, 11), publication(Some(1)))
            .unwrap();
        assert_eq!(state.continuity(), Continuity::Unknown);
        state
            .patch(owner, 3, patch(1, 4, 12), publication(Some(2)))
            .unwrap();
        assert_eq!(&*state.assembly().snapshot().unwrap().cells, &[3, 4, 3, 4]);
        assert_eq!(state.continuity(), Continuity::SequenceAvailable);
        assert_eq!(
            state.patch(owner, 4, patch(0, 9, 13), publication(Some(32))),
            Ok(Apply::WaitingFull)
        );
        assert_eq!(state.assembly().cell_bytes(), 0);
        assert_eq!(state.recovery_floor(), Some(13));
        assert_eq!(state.full(owner, 5, full(10)), Ok(Apply::Stale));
        state
            .patch(owner, 6, patch(1, 9, 14), publication(Some(33)))
            .unwrap();
        assert_eq!(state.recovery_floor(), Some(14));
        state.full(owner, 7, full(14)).unwrap();
        assert_eq!(&*state.assembly().snapshot().unwrap().cells, &[0; 4]);
        state
            .patch(owner, 8, patch(1, 5, 15), publication(Some(34)))
            .unwrap();
        assert_eq!(&*state.assembly().snapshot().unwrap().cells, &[0, 5, 0, 5]);
        assert_eq!(state.counters().sequence_faults, 1);
    }
    #[test]
    fn unsupported_sequence_never_becomes_verified_and_writer_switch_invalidates() {
        let mut state = MapIngress::default();
        let owner = state.start(true).unwrap();
        state.full(owner, 1, full(10)).unwrap();
        for seq in 2..5 {
            state
                .patch(owner, seq, patch(0, 1, 10 + seq as i64), publication(None))
                .unwrap();
        }
        assert_eq!(state.continuity(), Continuity::Unknown);
        let mut other = publication(None);
        other.publisher_id = "bb".repeat(16);
        assert_eq!(
            state.patch(owner, 5, patch(0, 2, 20), other),
            Ok(Apply::WaitingFull)
        );
        assert_eq!(state.counters().publisher_changes, 1);
        assert_eq!(state.assembly().cell_bytes(), 0);
    }
    #[test]
    fn stale_owner_ingress_and_loss_events_cannot_clear_or_raise_recovery_floor() {
        let mut state = MapIngress::default();
        let old = state.start(true).unwrap();
        let owner = state.start(true).unwrap();
        state.full(owner, 10, full(10)).unwrap();
        assert_eq!(
            state.patch(old, 100, patch(0, 2, 100), publication(Some(99))),
            Ok(Apply::Stale)
        );
        assert_eq!(
            state.patch(owner, 9, patch(0, 2, 100), publication(Some(99))),
            Ok(Apply::Stale)
        );
        assert!(!state.loss(old, 101, Reader::Updates, Some(100)));
        assert!(!state.fault(owner, 9, Invalidation::Clear));
        assert_eq!(state.assembly().status(), Status::Ready);
        assert_eq!(state.recovery_floor(), None);
        state
            .patch(owner, 11, patch(1, 1, 11), publication(Some(1)))
            .unwrap();
        assert_eq!(state.counters().stale_events, 4);
    }
    #[test]
    fn reported_loss_counter_reset_fault_stop_and_restart_have_explicit_recovery() {
        let mut state = MapIngress::default();
        let owner = state.start(true).unwrap();
        state.full(owner, 1, full(10)).unwrap();
        state.loss(owner, 2, Reader::Updates, Some(3));
        assert_eq!(state.assembly().cell_bytes(), 0);
        assert_eq!(state.counters().reported_lost, 3);
        state.full(owner, 3, full(10)).unwrap();
        state.loss(owner, 4, Reader::Updates, Some(3));
        assert_eq!(state.assembly().status(), Status::Ready);
        state.loss(owner, 5, Reader::Updates, Some(0));
        assert_eq!(state.assembly().cell_bytes(), 0);
        assert_eq!(state.counters().loss_counter_resets, 1);
        state.full(owner, 6, full(11)).unwrap();
        state.fault(owner, 7, Invalidation::InvalidInput);
        assert_eq!(state.full(owner, 8, full(10)), Ok(Apply::Stale));
        state.stop();
        assert_eq!(state.assembly().status(), Status::Stopped);
        assert_eq!(state.recovery_floor(), None);
        let next = state.start(false).unwrap();
        assert_ne!(next, owner);
        assert_eq!(state.continuity(), Continuity::Disabled);
        assert_eq!(state.full(owner, 100, full(100)), Ok(Apply::Stale));
        state.full(next, 1, full(1)).unwrap();
        assert!(state
            .patch(next, 2, patch(0, 1, 2), publication(Some(1)))
            .is_err());
        assert_eq!(state.assembly().cell_bytes(), 0);
    }
    #[test]
    fn sequence_reversal_malformed_data_and_invalid_identity_never_partially_mutate() {
        let mut state = MapIngress::default();
        let owner = state.start(true).unwrap();
        state.full(owner, 1, full(10)).unwrap();
        state
            .patch(owner, 2, patch(0, 1, 11), publication(Some(2)))
            .unwrap();
        state
            .patch(owner, 3, patch(0, 2, 12), publication(Some(2)))
            .unwrap();
        assert_eq!(state.assembly().cell_bytes(), 0);
        state.full(owner, 4, full(12)).unwrap();
        let mut bad = patch(0, 9, 13);
        bad.cells = Box::default();
        assert!(state.patch(owner, 5, bad, publication(Some(3))).is_err());
        assert_eq!(state.assembly().cell_bytes(), 0);
        state.full(owner, 6, full(13)).unwrap();
        let mut bad = publication(Some(4));
        bad.publisher_id = "not hex".into();
        assert!(state.patch(owner, 7, patch(0, 9, 14), bad).is_err());
        assert_eq!(state.assembly().cell_bytes(), 0);
    }
    #[test]
    fn independent_reader_counters_do_not_masquerade_as_loss_or_reset_each_other() {
        let mut state = MapIngress::default();
        let owner = state.start(true).unwrap();
        state.full(owner, 1, full(10)).unwrap();
        state.loss(owner, 2, Reader::Updates, Some(3));
        state.full(owner, 3, full(11)).unwrap();
        state.loss(owner, 4, Reader::Full, Some(0));
        assert_eq!(state.assembly().status(), Status::Ready);
        assert_eq!(state.counters().loss_counter_resets, 0);
        state.loss(owner, 5, Reader::Full, Some(1));
        state.full(owner, 6, full(12)).unwrap();
        state.loss(owner, 7, Reader::Updates, Some(3));
        assert_eq!(state.assembly().status(), Status::Ready);
        assert_eq!(state.counters().reported_lost, 4);
    }
}
