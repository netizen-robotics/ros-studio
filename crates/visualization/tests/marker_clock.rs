//! Public clock boundary: finite receiver deadlines cannot wrap during rebasing.
use std::time::{Duration, Instant};
use studio_visualization::spatial::marker_clock::MarkerClock;
#[test]
fn checked_public_deadline_conversion_rejects_u64_overflow_and_clips_expired_sources() {
    let source_origin = Instant::now();
    let clock = MarkerClock::new();
    let future_origin = Instant::now().checked_add(Duration::from_secs(1)).unwrap();
    assert!(clock.deadline_from(future_origin, u64::MAX).is_err());
    assert_eq!(clock.deadline_from(source_origin, 0).unwrap(), 0);
}
