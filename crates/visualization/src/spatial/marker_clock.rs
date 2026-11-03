//! Process-local monotonic presentation time. No ROS/System clock authorization
//! or persistence is introduced by this rendering clock.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub struct MarkerClock(Arc<Instant>);
impl Default for MarkerClock {
    fn default() -> Self {
        Self::new()
    }
}
impl MarkerClock {
    pub fn new() -> Self {
        Self(Arc::new(Instant::now()))
    }
    pub fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos().try_into().unwrap_or(u64::MAX)
    }
    /// Preserve the exact absolute Instant from an existing client stream.
    /// Deadlines older than this presentation clock are already expired.
    pub fn deadline_from(
        &self,
        source_origin: Instant,
        source_deadline_ns: u64,
    ) -> Result<u64, String> {
        let deadline = source_origin
            .checked_add(Duration::from_nanos(source_deadline_ns))
            .ok_or("Marker deadline Instant overflow")?;
        deadline
            .checked_duration_since(*self.0)
            .unwrap_or_default()
            .as_nanos()
            .try_into()
            .map_err(|_| "Marker deadline exceeds rendering clock".into())
    }
}
pub(crate) fn words(time: u64) -> [u32; 4] {
    [time as u32, (time >> 32) as u32, 0, 0]
}
pub(crate) fn deadline(shape: [u32; 4]) -> Option<u64> {
    (shape[3] == 1).then_some(shape[1] as u64 | ((shape[2] as u64) << 32))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_translation_preserves_absolute_deadlines_and_clips_old_values() {
        let base = Instant::now();
        let clock = MarkerClock(Arc::new(base));
        let old = base.checked_sub(Duration::from_secs(2)).unwrap();
        assert_eq!(clock.deadline_from(old, 2_000_000_007).unwrap(), 7);
        assert_eq!(clock.deadline_from(old, 1).unwrap(), 0);
        assert_eq!(
            clock
                .deadline_from(base + Duration::from_secs(3), 5)
                .unwrap(),
            3_000_000_005
        );
        let first = clock.now_ns();
        assert!(clock.now_ns() >= first);
    }
    #[test]
    fn deadline_words_roundtrip_across_low_word_rollover_and_full_u64_range() {
        for value in [
            0,
            1,
            u32::MAX as u64,
            1u64 << 32,
            (1u64 << 32) + 1,
            u64::MAX,
        ] {
            let w = words(value);
            assert_eq!(deadline([1, w[0], w[1], 1]), Some(value));
        }
        assert_eq!(deadline([1, 0, 0, 0]), None);
    }
}
