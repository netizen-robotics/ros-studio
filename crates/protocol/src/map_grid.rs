//! Shared, worker-side full-map/patch assembly. Not a negotiated wire codec.
//! Raw signed cells preserve every byte for map/costmap/raw presentation.
use crate::{mapping, spatial};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_DIMENSION: u32 = 2048;
pub const MAX_CELLS: usize = MAX_DIMENSION as usize * MAX_DIMENSION as usize;
pub const MAX_CELL_BYTES: usize = MAX_CELLS;
pub const UPDATE_MESSAGE_TYPE: &str = "map_msgs/msg/OccupancyGridUpdate";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    pub topic: String,
    pub best_effort: bool,
    pub transient_local: bool,
}
impl UpdateSource {
    pub fn valid(&self) -> bool {
        crate::subscriptions::valid_topic(&self.topic)
    }
}

/// Availability of observed update sequence metadata, never revision proof.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuity {
    #[default]
    Disabled,
    Unknown,
    SequenceAvailable,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngressCounters {
    pub stale_events: u64,
    pub sequence_faults: u64,
    pub publisher_changes: u64,
    pub reported_lost: u64,
    pub loss_counter_resets: u64,
    pub rejected_recovery_full: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub frame_id: String,
    /// Composite acquisition time, advanced by each accepted patch.
    pub stamp_ns: i64,
    pub width: u32,
    pub height: u32,
    pub resolution: f64,
    /// XYZ followed by quaternion XYZW in the source frame.
    pub origin: [f64; 7],
    pub map_load_time_ns: i64,
}
impl Metadata {
    pub fn cell_count(&self) -> Result<usize, &'static str> {
        cell_count(self.width, self.height)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        self.cell_count()?;
        if !spatial::valid_frame(&self.frame_id)
            || !valid_stamp(self.stamp_ns)
            || !(0..=mapping::MAX_STAMP_NS).contains(&self.map_load_time_ns)
            || !mapping::valid_map_bounds(self.width, self.height, self.resolution, &self.origin)
        {
            return Err("Invalid map frame, time, resolution, origin or source bounds");
        }
        Ok(())
    }
}
fn valid_stamp(stamp: i64) -> bool {
    (1..=mapping::MAX_STAMP_NS).contains(&stamp)
}
pub fn cell_count(width: u32, height: u32) -> Result<usize, &'static str> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("Map dimensions must be within 1–2048 cells");
    }
    (width as usize)
        .checked_mul(height as usize)
        .filter(|count| *count <= MAX_CELLS)
        .ok_or("Map cell count exceeds the bounded grid")
}

#[derive(PartialEq)]
pub struct Grid {
    pub metadata: Metadata,
    pub cells: Box<[i8]>,
}
impl std::fmt::Debug for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grid")
            .field("metadata", &self.metadata)
            .field("cell_count", &self.cells.len())
            .finish()
    }
}
impl Grid {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.metadata.validate()?;
        if self.cells.len() != self.metadata.cell_count()? {
            return Err("Map data length does not match its dimensions");
        }
        Ok(())
    }
    /// Requested cell storage only; excludes metadata and allocator overhead.
    pub fn cell_bytes(&self) -> usize {
        self.cells.len()
    }
}

pub struct Patch {
    pub frame_id: String,
    pub stamp_ns: i64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub cells: Box<[i8]>,
}
impl Patch {
    /// Validate scalar metadata before a native decoder allocates cell storage.
    pub fn cell_count(&self) -> Result<usize, &'static str> {
        if !spatial::valid_frame(&self.frame_id)
            || !valid_stamp(self.stamp_ns)
            || self.x < 0
            || self.y < 0
            || (self.x as u32)
                .checked_add(self.width)
                .is_none_or(|x| x > MAX_DIMENSION)
            || (self.y as u32)
                .checked_add(self.height)
                .is_none_or(|y| y > MAX_DIMENSION)
        {
            return Err("Invalid map update frame, time, region or data length");
        }
        cell_count(self.width, self.height)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.cells.len() != self.cell_count()? {
            return Err("Invalid map update frame, time, region or data length");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(u64);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalidation {
    InvalidInput,
    QueueOverflow,
    SourceFailure,
    Clear,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Stopped,
    WaitingFull(Option<Invalidation>),
    Ready,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Stale,
    WaitingFull,
    Unchanged,
    Updated { revision: u64 },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub full_maps: u64,
    pub patches: u64,
    pub stale_inputs: u64,
    pub updates_without_base: u64,
    pub invalidations: u64,
}

/// One current owner, one mutable cell block, no per-cell geometry or clone cache.
/// Call only from the map worker. Borrow a snapshot or explicitly budget a copy
/// for immutable transport; this core never queues or publishes partial maps.
#[derive(Default)]
pub struct Assembly {
    token: Option<Token>,
    sequence: u64,
    revision: u64,
    grid: Option<Grid>,
    status: Status,
    counters: Counters,
}
impl Assembly {
    pub fn start(&mut self) -> Result<Token, &'static str> {
        self.stop();
        let token = Token(
            NEXT_TOKEN
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| "Map owner exhausted")?,
        );
        self.token = Some(token);
        self.status = Status::WaitingFull(None);
        Ok(token)
    }
    pub fn stop(&mut self) {
        self.token = None;
        self.grid = None;
        self.sequence = 0;
        self.revision = 0;
        self.status = Status::Stopped;
        self.counters = Counters::default();
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn snapshot(&self) -> Option<&Grid> {
        self.grid.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cell_bytes(&self) -> usize {
        self.grid.as_ref().map_or(0, Grid::cell_bytes)
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    fn current_input(&mut self, token: Token, sequence: u64) -> bool {
        if self.token != Some(token) || sequence <= self.sequence {
            self.counters.stale_inputs = self.counters.stale_inputs.saturating_add(1);
            return false;
        }
        self.sequence = sequence;
        true
    }
    pub fn invalidate(&mut self, token: Token, reason: Invalidation) -> bool {
        if self.token != Some(token) {
            return false;
        }
        self.grid = None;
        self.status = Status::WaitingFull(Some(reason));
        self.counters.invalidations = self.counters.invalidations.saturating_add(1);
        true
    }
    fn invalid(&mut self, token: Token, error: &'static str) -> Result<Apply, &'static str> {
        self.invalidate(token, Invalidation::InvalidInput);
        Err(error)
    }
    fn next_revision(&mut self, token: Token) -> Result<u64, &'static str> {
        let Some(next) = self.revision.checked_add(1) else {
            self.invalidate(token, Invalidation::InvalidInput);
            return Err("Map revision exhausted; restart the display");
        };
        self.revision = next;
        Ok(next)
    }
    fn stale(&mut self) -> Apply {
        self.counters.stale_inputs = self.counters.stale_inputs.saturating_add(1);
        Apply::Stale
    }
    pub fn full(&mut self, token: Token, sequence: u64, grid: Grid) -> Result<Apply, &'static str> {
        if !self.current_input(token, sequence) {
            return Ok(Apply::Stale);
        }
        if let Err(error) = grid.validate() {
            return self.invalid(token, error);
        }
        if let Some(previous) = &self.grid {
            if grid.metadata.stamp_ns < previous.metadata.stamp_ns {
                return Ok(self.stale());
            }
            if grid == *previous {
                return Ok(Apply::Unchanged);
            }
            if grid.metadata.stamp_ns == previous.metadata.stamp_ns {
                return self.invalid(token, "Changed full map has an unchanged acquisition stamp");
            }
        }
        let revision = self.next_revision(token)?;
        self.grid = Some(grid);
        self.status = Status::Ready;
        self.counters.full_maps = self.counters.full_maps.saturating_add(1);
        Ok(Apply::Updated { revision })
    }
    pub fn patch(
        &mut self,
        token: Token,
        sequence: u64,
        patch: Patch,
    ) -> Result<Apply, &'static str> {
        if !self.current_input(token, sequence) {
            return Ok(Apply::Stale);
        }
        if let Err(error) = patch.validate() {
            return self.invalid(token, error);
        }
        let Some(grid) = &self.grid else {
            self.counters.updates_without_base =
                self.counters.updates_without_base.saturating_add(1);
            return Ok(Apply::WaitingFull);
        };
        // All dimensions/counts/offsets are validated before the first row write.
        if patch.frame_id != grid.metadata.frame_id
            || patch.x as u32 + patch.width > grid.metadata.width
            || patch.y as u32 + patch.height > grid.metadata.height
        {
            return self.invalid(
                token,
                "Map update frame or region differs from the full map",
            );
        }
        if patch.stamp_ns < grid.metadata.stamp_ns {
            return Ok(self.stale());
        }
        let revision = self.next_revision(token)?;
        let grid = self.grid.as_mut().expect("validated full map");
        for row in 0..patch.height as usize {
            let source = row * patch.width as usize;
            let target = (patch.y as usize + row) * grid.metadata.width as usize + patch.x as usize;
            grid.cells[target..target + patch.width as usize]
                .copy_from_slice(&patch.cells[source..source + patch.width as usize]);
        }
        grid.metadata.stamp_ns = patch.stamp_ns;
        self.counters.patches = self.counters.patches.saturating_add(1);
        Ok(Apply::Updated { revision })
    }
}
