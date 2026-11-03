//! Bounded retention of already projected, displayed sensor samples.
//! Receipt age is monotonic and local; acquisition-time TF stays upstream.
use super::Point3;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use studio_protocol::cloud::{Channel, Channels};

pub const MAX_WINDOW_MS: u16 = 10_000;
pub const MAX_HISTORY_FRAMES: usize = 32;
pub const MAX_HISTORY_POINTS: usize = 32_768;
pub const MAX_SAMPLE_POINTS: usize = studio_protocol::cloud::MAX_SOURCE_POINTS;
/// Requested frame/cached sample storage, excluding allocator and in-flight work.
pub const MAX_HISTORY_BYTES: usize = 3 * 1024 * 1024;

/// Attributes travel with their already projected point through every eviction.
/// Missing/unsupported channels and nonfinite source intensity have no value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SensorPoint {
    pub position: Point3,
    pub rgba: Option<[u8; 4]>,
    pub intensity: Option<f64>,
}
const _: () =
    assert!(2 * MAX_HISTORY_POINTS * std::mem::size_of::<SensorPoint>() <= MAX_HISTORY_BYTES);

/// Convert off UI frames after acquisition-time projection. Source layout status
/// stays with the live frame; a mixed history uses explicit per-point absence.
pub fn attributed_points(
    points: &[Point3],
    channels: &Channels,
) -> Result<Box<[SensorPoint]>, HistoryError> {
    if points.len() > MAX_SAMPLE_POINTS || points.iter().any(|p| !valid_position(*p)) {
        return Err(HistoryError::InvalidPoints);
    }
    channels
        .validate(points.len())
        .map_err(|_| HistoryError::InvalidAttributes)?;
    Ok(points
        .iter()
        .enumerate()
        .map(|(i, p)| SensorPoint {
            position: *p,
            rgba: match &channels.color {
                Channel::Values(v) => Some(v[i]),
                _ => None,
            },
            intensity: match &channels.intensity {
                Channel::Values(v) => v[i],
                _ => None,
            },
        })
        .collect())
}
fn valid_position(point: Point3) -> bool {
    point
        .0
        .iter()
        .all(|v| v.is_finite() && v.abs() <= studio_protocol::cloud::MAX_COORDINATE)
}
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryToken(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryError {
    InvalidFrame,
    InvalidWindow,
    Inactive,
    StaleToken,
    StaleSequence,
    InvalidStamp,
    StampRegression,
    ClockRegression,
    InvalidPoints,
    InvalidAttributes,
    EpochExhausted,
}
impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidFrame => "Sensor history requires a valid fixed frame",
            Self::InvalidWindow => "Sensor history duration must be0..10000ms",
            Self::Inactive => "Sensor history is stopped",
            Self::StaleToken => "Old sensor history activation rejected",
            Self::StaleSequence => "Old sensor history sequence rejected",
            Self::InvalidStamp => "Sensor history requires a positive bounded acquisition stamp",
            Self::StampRegression => "Acquisition time moved backward; sensor history cleared",
            Self::ClockRegression => "Local receipt age moved backward; sensor history cleared",
            Self::InvalidPoints => "Invalid or oversized projected sensor sample",
            Self::InvalidAttributes => "Invalid sensor attributes",
            Self::EpochExhausted => "Sensor history activation identities exhausted",
        })
    }
}
impl std::error::Error for HistoryError {}

/// Display retention accounting, distinct from DDS and worker/application loss.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryCounters {
    pub expired_frames: u64,
    pub replaced_frames: u64,
    pub budget_evicted_frames: u64,
    pub budget_evicted_points: u64,
    pub cleared_frames: u64,
    pub stale_rejected: u64,
    pub clock_resets: u64,
}
struct Frame {
    received: Duration,
    stamp_ns: i64,
    points: Box<[SensorPoint]>,
}
pub struct SensorHistory {
    token: Option<HistoryToken>,
    fixed_frame: Option<String>,
    window_ms: u16,
    last_sequence: u64,
    last_stamp: Option<i64>,
    last_now: Option<Duration>,
    frames: VecDeque<Frame>,
    points: Vec<SensorPoint>,
    counters: HistoryCounters,
}
impl Default for SensorHistory {
    fn default() -> Self {
        Self {
            token: None,
            fixed_frame: None,
            window_ms: 0,
            last_sequence: 0,
            last_stamp: None,
            last_now: None,
            frames: VecDeque::with_capacity(MAX_HISTORY_FRAMES),
            points: Vec::new(),
            counters: Default::default(),
        }
    }
}
impl SensorHistory {
    /// No ROS activation is implied. Invalid start leaves current history intact.
    pub fn start(&mut self, fixed_frame: &str) -> Result<HistoryToken, HistoryError> {
        if !studio_protocol::spatial::valid_frame(fixed_frame) {
            return Err(HistoryError::InvalidFrame);
        }
        let epoch = NEXT_EPOCH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| HistoryError::EpochExhausted)?;
        let token = HistoryToken(epoch);
        self.clear_frames();
        self.counters = Default::default();
        self.last_sequence = 0;
        self.last_stamp = None;
        self.last_now = None;
        self.token = Some(token);
        self.fixed_frame = Some(fixed_frame.into());
        Ok(token)
    }
    pub fn stop(&mut self) {
        self.clear_frames();
        self.token = None;
        self.fixed_frame = None;
        self.last_stamp = None;
        self.last_now = None;
    }
    pub fn points(&self) -> &[SensorPoint] {
        &self.points
    }
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
    pub fn fixed_frame(&self) -> Option<&str> {
        self.fixed_frame.as_deref()
    }
    pub fn window_ms(&self) -> u16 {
        self.window_ms
    }
    pub fn counters(&self) -> HistoryCounters {
        self.counters
    }
    pub fn oldest_acquisition_stamp(&self) -> Option<i64> {
        self.frames.front().map(|f| f.stamp_ns)
    }
    /// Positions and attributes, not allocator overhead, worker inputs or GPU/driver memory.
    pub fn owned_sample_bytes(&self) -> usize {
        (self.frames.iter().map(|f| f.points.len()).sum::<usize>() + self.points.capacity())
            * std::mem::size_of::<SensorPoint>()
    }
    pub fn next_expiration(&self) -> Option<Duration> {
        if self.window_ms == 0 {
            None
        } else {
            self.frames.front().map(|f| {
                f.received
                    .saturating_add(Duration::from_millis(self.window_ms.into()))
            })
        }
    }
    fn check_token(&mut self, token: HistoryToken) -> Result<(), HistoryError> {
        if self.token == Some(token) {
            return Ok(());
        }
        self.counters.stale_rejected = self.counters.stale_rejected.saturating_add(1);
        Err(if self.token.is_none() {
            HistoryError::Inactive
        } else {
            HistoryError::StaleToken
        })
    }
    fn clock(&mut self, now: Duration) -> Result<(), HistoryError> {
        if self.last_now.is_some_and(|last| now < last) {
            self.clear_frames();
            self.last_stamp = None;
            self.last_now = Some(now);
            self.counters.clock_resets = self.counters.clock_resets.saturating_add(1);
            return Err(HistoryError::ClockRegression);
        }
        self.last_now = Some(now);
        Ok(())
    }
    fn clear_frames(&mut self) -> bool {
        let changed = !self.frames.is_empty();
        self.counters.cleared_frames = self
            .counters
            .cleared_frames
            .saturating_add(self.frames.len() as u64);
        self.frames.clear();
        self.points = Vec::new();
        changed
    }
    /// Missing TF/error/explicit Clear removes only this activation's geometry.
    pub fn clear(&mut self, token: HistoryToken) -> Result<bool, HistoryError> {
        self.check_token(token)?;
        Ok(self.clear_frames())
    }
    fn rebuild(&mut self) {
        let count = self.frames.iter().map(|f| f.points.len()).sum();
        if count == 0 {
            self.points = Vec::new();
            return;
        }
        self.points.clear();
        self.points.reserve_exact(count);
        for frame in &self.frames {
            self.points.extend_from_slice(&frame.points);
        }
        debug_assert!(self.points.len() <= MAX_HISTORY_POINTS);
        debug_assert!(self.owned_sample_bytes() <= MAX_HISTORY_BYTES);
    }
    fn expire_at(&mut self, now: Duration) -> bool {
        if self.window_ms == 0 {
            return false;
        }
        let duration = Duration::from_millis(self.window_ms.into());
        let mut changed = false;
        while self
            .frames
            .front()
            .is_some_and(|f| now.saturating_sub(f.received) >= duration)
        {
            self.frames.pop_front();
            self.counters.expired_frames = self.counters.expired_frames.saturating_add(1);
            changed = true;
        }
        if changed {
            self.rebuild();
        }
        changed
    }
    /// Call on idle frames too; the last retained sample must actually expire.
    pub fn expire(&mut self, now: Duration) -> Result<bool, HistoryError> {
        if self.token.is_none() {
            return Ok(false);
        }
        self.clock(now)?;
        Ok(self.expire_at(now))
    }
    /// Changing duration never resurrects previously expired/evicted samples.
    pub fn set_window_ms(&mut self, window_ms: u16, now: Duration) -> Result<bool, HistoryError> {
        if window_ms > MAX_WINDOW_MS {
            return Err(HistoryError::InvalidWindow);
        }
        let mut changed = self.expire(now)?;
        changed |= self.window_ms != window_ms;
        self.window_ms = window_ms;
        if window_ms == 0 {
            while self.frames.len() > 1 {
                self.frames.pop_front();
                self.counters.replaced_frames = self.counters.replaced_frames.saturating_add(1);
            }
            self.rebuild();
        } else {
            changed |= self.expire_at(now);
        }
        Ok(changed)
    }
    pub fn push(
        &mut self,
        token: HistoryToken,
        sequence: u64,
        stamp_ns: i64,
        now: Duration,
        points: Box<[SensorPoint]>,
    ) -> Result<(), HistoryError> {
        self.check_token(token)?;
        if sequence == 0 || sequence <= self.last_sequence {
            self.counters.stale_rejected = self.counters.stale_rejected.saturating_add(1);
            return Err(HistoryError::StaleSequence);
        }
        self.last_sequence = sequence;
        self.clock(now)?;
        if stamp_ns <= 0 || stamp_ns > studio_protocol::cloud::MAX_STAMP_NS {
            self.clear_frames();
            self.last_stamp = None;
            return Err(HistoryError::InvalidStamp);
        }
        if self.last_stamp.is_some_and(|last| stamp_ns < last) {
            self.clear_frames();
            self.last_stamp = None;
            self.counters.clock_resets = self.counters.clock_resets.saturating_add(1);
            return Err(HistoryError::StampRegression);
        }
        self.last_stamp = Some(stamp_ns);
        if points.len() > MAX_SAMPLE_POINTS || points.iter().any(|p| !valid_position(p.position)) {
            self.clear_frames();
            return Err(HistoryError::InvalidPoints);
        }
        if points
            .iter()
            .any(|p| p.intensity.is_some_and(|v| !v.is_finite()))
        {
            self.clear_frames();
            return Err(HistoryError::InvalidAttributes);
        }
        self.expire_at(now);
        if points.is_empty() {
            self.clear_frames();
            return Ok(());
        }
        if self.window_ms == 0 {
            self.counters.replaced_frames = self
                .counters
                .replaced_frames
                .saturating_add(self.frames.len() as u64);
            self.frames.clear();
        }
        let mut total = self.frames.iter().map(|f| f.points.len()).sum::<usize>();
        while self.frames.len() >= MAX_HISTORY_FRAMES || total + points.len() > MAX_HISTORY_POINTS {
            let old = self
                .frames
                .pop_front()
                .expect("per-sample cap fits empty history");
            total -= old.points.len();
            self.counters.budget_evicted_frames =
                self.counters.budget_evicted_frames.saturating_add(1);
            self.counters.budget_evicted_points = self
                .counters
                .budget_evicted_points
                .saturating_add(old.points.len() as u64);
        }
        self.frames.push_back(Frame {
            received: now,
            stamp_ns,
            points,
        });
        self.rebuild();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }
    fn point(x: f64) -> SensorPoint {
        SensorPoint {
            position: Point3([x, 0., 0.]),
            ..Default::default()
        }
    }
    fn points(count: usize, x: f64) -> Box<[SensorPoint]> {
        vec![point(x); count].into_boxed_slice()
    }
    #[test]
    fn mixed_source_attributes_remain_attached_through_retention_and_expiry() {
        let mut history = SensorHistory::default();
        history.set_window_ms(100, ms(0)).unwrap();
        let token = history.start("map").unwrap();
        let channels = Channels {
            color: Channel::Values(vec![[1, 2, 3, 4], [5, 6, 7, 8]]),
            intensity: Channel::Values(vec![Some(10.), None]),
        };
        let attributed =
            attributed_points(&[Point3([1., 2., 3.]), Point3([4., 5., 6.])], &channels).unwrap();
        let expected = attributed.clone();
        history.push(token, 1, 100, ms(0), attributed).unwrap();
        let absent = attributed_points(
            &[Point3([7., 8., 9.])],
            &Channels {
                color: Channel::Unsupported,
                intensity: Channel::Missing,
            },
        )
        .unwrap();
        history.push(token, 2, 101, ms(50), absent).unwrap();
        assert_eq!(&history.points()[..2], &*expected);
        assert_eq!(
            history.points()[2],
            SensorPoint {
                position: Point3([7., 8., 9.]),
                rgba: None,
                intensity: None
            }
        );
        assert_eq!(history.points()[1].rgba, Some([5, 6, 7, 8]));
        assert_eq!(history.points()[1].intensity, None);
        history.expire(ms(100)).unwrap();
        assert_eq!(history.points().len(), 1);
        assert_eq!(history.points()[0].position, Point3([7., 8., 9.]));
        assert!(history.points()[0].rgba.is_none());
        history.expire(ms(150)).unwrap();
        assert_eq!(history.owned_sample_bytes(), 0);
    }
    #[test]
    fn attributed_conversion_checks_alignment_and_current_invalid_values_clear_only_the_owner() {
        let position = [Point3([1., 2., 3.])];
        assert_eq!(
            attributed_points(
                &position,
                &Channels {
                    color: Channel::Values(vec![]),
                    intensity: Channel::Missing
                }
            ),
            Err(HistoryError::InvalidAttributes)
        );
        assert_eq!(
            attributed_points(
                &position,
                &Channels {
                    color: Channel::Missing,
                    intensity: Channel::Values(vec![Some(f64::NAN)])
                }
            ),
            Err(HistoryError::InvalidAttributes)
        );
        assert_eq!(
            attributed_points(
                &vec![position[0]; MAX_SAMPLE_POINTS + 1],
                &Channels::default()
            ),
            Err(HistoryError::InvalidPoints)
        );
        assert_eq!(
            attributed_points(&[Point3([f64::INFINITY, 0., 0.])], &Channels::default()),
            Err(HistoryError::InvalidPoints)
        );
        let mut history = SensorHistory::default();
        let old = history.start("map").unwrap();
        let token = history.start("map").unwrap();
        history.push(token, 1, 100, ms(0), points(1, 1.)).unwrap();
        let bad = vec![SensorPoint {
            position: position[0],
            rgba: Some([1, 2, 3, 4]),
            intensity: Some(f64::INFINITY),
        }]
        .into_boxed_slice();
        assert_eq!(
            history.push(old, 2, 101, ms(1), bad.clone()),
            Err(HistoryError::StaleToken)
        );
        assert_eq!(
            history.push(token, 1, 101, ms(1), bad.clone()),
            Err(HistoryError::StaleSequence)
        );
        assert_eq!(history.points(), [point(1.)]);
        let mut peer = SensorHistory::default();
        let other = peer.start("map").unwrap();
        peer.push(other, 1, 1, ms(0), points(1, 99.)).unwrap();
        assert_eq!(
            history.push(token, 2, 101, ms(1), bad),
            Err(HistoryError::InvalidAttributes)
        );
        assert_eq!(history.owned_sample_bytes(), 0);
        assert_eq!(peer.points(), [point(99.)]);
    }
    #[test]
    fn full_attributed_history_respects_three_mib_and_evicts_complete_sample_blocks() {
        let mut history = SensorHistory::default();
        let token = history.start("map").unwrap();
        history.set_window_ms(10_000, ms(0)).unwrap();
        for sequence in 1..=4 {
            let sample = SensorPoint {
                position: Point3([sequence as f64, 0., 0.]),
                rgba: Some([sequence as u8, 2, 3, 4]),
                intensity: Some(sequence as f64),
            };
            history
                .push(
                    token,
                    sequence,
                    sequence as i64,
                    ms(sequence),
                    vec![sample; MAX_SAMPLE_POINTS].into_boxed_slice(),
                )
                .unwrap();
        }
        assert_eq!(history.points().len(), MAX_HISTORY_POINTS);
        assert_eq!(
            history.owned_sample_bytes(),
            2 * MAX_HISTORY_POINTS * std::mem::size_of::<SensorPoint>()
        );
        assert!(history.owned_sample_bytes() <= MAX_HISTORY_BYTES);
        history.push(token, 5, 5, ms(5), points(1, 5.)).unwrap();
        assert_eq!(
            history.counters().budget_evicted_points,
            MAX_SAMPLE_POINTS as u64
        );
        assert_eq!(history.points()[0].rgba, Some([2, 2, 3, 4]));
        assert_eq!(history.points()[0].intensity, Some(2.));
        assert_eq!(history.points().last().unwrap(), &point(5.));
        history.clear(token).unwrap();
        assert_eq!(history.owned_sample_bytes(), 0);
        history.push(token, 6, 6, ms(6), points(1, 6.)).unwrap();
        history.stop();
        assert_eq!(history.owned_sample_bytes(), 0);
    }
    #[test]
    fn latest_persists_and_positive_history_expires_even_when_idle() {
        let mut history = SensorHistory::default();
        let token = history.start("map").unwrap();
        history.push(token, 1, 100, ms(0), points(1, 1.)).unwrap();
        history.expire(ms(50_000)).unwrap();
        assert_eq!(history.points(), [point(1.)]);
        history
            .push(token, 2, 101, ms(50_000), points(1, 2.))
            .unwrap();
        assert_eq!(history.frame_count(), 1);
        assert_eq!(history.counters().replaced_frames, 1);
        history.set_window_ms(100, ms(50_000)).unwrap();
        assert_eq!(history.next_expiration(), Some(ms(50_100)));
        assert!(!history.expire(ms(50_099)).unwrap());
        assert!(history.expire(ms(50_100)).unwrap());
        assert!(history.points().is_empty());
        assert_eq!(history.owned_sample_bytes(), 0);
        assert_eq!(history.counters().expired_frames, 1);
    }
    #[test]
    fn generation_and_sequence_rejection_never_clear_current_owner() {
        let mut history = SensorHistory::default();
        let old = history.start("map").unwrap();
        history.stop();
        let token = history.start("odom").unwrap();
        assert_ne!(old, token);
        history.push(token, 4, 4, ms(4), points(1, 4.)).unwrap();
        assert_eq!(
            history.push(old, 100, 100, ms(100), points(1, 100.)),
            Err(HistoryError::StaleToken)
        );
        assert_eq!(
            history.push(token, 4, 5, ms(5), points(1, 5.)),
            Err(HistoryError::StaleSequence)
        );
        assert_eq!(history.clear(old), Err(HistoryError::StaleToken));
        assert_eq!(history.points(), [point(4.)]);
        assert!(history.start("/invalid").is_err());
        assert_eq!(history.fixed_frame(), Some("odom"));
        history.stop();
        assert_eq!(history.owned_sample_bytes(), 0);
        assert!(history.push(token, 5, 5, ms(5), points(1, 5.)).is_err());
    }
    #[test]
    fn whole_frame_eviction_enforces_point_frame_and_requested_sample_allocation_caps() {
        let mut history = SensorHistory::default();
        history.set_window_ms(10_000, ms(0)).unwrap();
        let token = history.start("map").unwrap();
        for seq in 1..=4 {
            history
                .push(
                    token,
                    seq,
                    seq as i64,
                    ms(seq),
                    points(MAX_SAMPLE_POINTS, seq as f64),
                )
                .unwrap();
        }
        assert_eq!(history.points().len(), MAX_HISTORY_POINTS);
        assert_eq!(history.frame_count(), 4);
        assert!(
            history.owned_sample_bytes()
                <= 2 * MAX_HISTORY_POINTS * std::mem::size_of::<SensorPoint>()
        );
        history.push(token, 5, 5, ms(5), points(1, 5.)).unwrap();
        assert_eq!(history.points().len(), 3 * MAX_SAMPLE_POINTS + 1);
        assert_eq!(history.points()[0], point(2.));
        assert_eq!(history.counters().budget_evicted_frames, 1);
        assert_eq!(
            history.counters().budget_evicted_points,
            MAX_SAMPLE_POINTS as u64
        );
        for seq in 6..=80 {
            history
                .push(token, seq, seq as i64, ms(seq), points(1, seq as f64))
                .unwrap();
        }
        assert_eq!(history.frame_count(), MAX_HISTORY_FRAMES);
        assert_eq!(history.points().len(), MAX_HISTORY_FRAMES);
        assert_eq!(history.oldest_acquisition_stamp(), Some(49));
        history.clear(token).unwrap();
        assert_eq!(history.owned_sample_bytes(), 0);
    }
    #[test]
    fn window_changes_expire_first_and_cannot_resurrect_old_samples() {
        let mut history = SensorHistory::default();
        let token = history.start("map").unwrap();
        history.set_window_ms(100, ms(0)).unwrap();
        history.push(token, 1, 1, ms(0), points(1, 1.)).unwrap();
        history.push(token, 2, 2, ms(50), points(1, 2.)).unwrap();
        history.set_window_ms(10_000, ms(100)).unwrap();
        assert_eq!(history.points(), [point(2.)]);
        history.push(token, 3, 3, ms(100), points(1, 3.)).unwrap();
        history.set_window_ms(0, ms(101)).unwrap();
        assert_eq!(history.points(), [point(3.)]);
        history.set_window_ms(10_000, ms(101)).unwrap();
        assert_eq!(history.frame_count(), 1);
        assert_eq!(
            history.set_window_ms(10_001, ms(102)),
            Err(HistoryError::InvalidWindow)
        );
        assert_eq!(history.window_ms(), 10_000);
    }
    #[test]
    fn current_owner_malformed_empty_or_clock_reset_clears_without_touching_a_peer() {
        let mut history = SensorHistory::default();
        let token = history.start("map").unwrap();
        let mut peer = SensorHistory::default();
        let other = peer.start("map").unwrap();
        peer.push(other, 1, 1, ms(0), points(1, 77.)).unwrap();
        history.push(token, 1, 100, ms(10), points(1, 1.)).unwrap();
        assert_eq!(
            history.push(token, 2, 99, ms(11), points(1, 2.)),
            Err(HistoryError::StampRegression)
        );
        assert!(history.points().is_empty());
        history.push(token, 3, 5, ms(12), points(1, 3.)).unwrap();
        assert_eq!(history.expire(ms(11)), Err(HistoryError::ClockRegression));
        assert!(history.points().is_empty());
        history.push(token, 4, 6, ms(12), points(1, 4.)).unwrap();
        history.push(token, 5, 7, ms(13), points(0, 0.)).unwrap();
        assert!(history.points().is_empty());
        history.push(token, 6, 8, ms(14), points(1, 6.)).unwrap();
        assert_eq!(
            history.push(token, 7, 9, ms(15), points(1, f64::NAN)),
            Err(HistoryError::InvalidPoints)
        );
        assert_eq!(history.owned_sample_bytes(), 0);
        assert_eq!(peer.points(), [point(77.)]);
    }
    #[test]
    fn exact_source_bound_and_acquisition_projection_are_preserved_without_retransform() {
        let mut history = SensorHistory::default();
        history.set_window_ms(1000, ms(0)).unwrap();
        let token = history.start("map").unwrap();
        history
            .push(token, 1, 10, ms(0), points(MAX_SAMPLE_POINTS, 10.))
            .unwrap();
        history.push(token, 2, 10, ms(1), points(1, 20.)).unwrap();
        assert_eq!(history.points()[0], point(10.));
        assert_eq!(*history.points().last().unwrap(), point(20.));
        assert_eq!(
            history.push(token, 3, 11, ms(2), points(MAX_SAMPLE_POINTS + 1, 0.)),
            Err(HistoryError::InvalidPoints)
        );
        assert!(history.points().is_empty());
        assert_eq!(
            history.push(token, 4, 0, ms(3), points(1, 0.)),
            Err(HistoryError::InvalidStamp)
        );
    }
}
