#![cfg(studio_windows_qos_compat)]

use studio_backend as _;

#[test]
fn rclrs_best_available_links_and_preserves_ros_header_policy() {
    extern "C" {
        fn studio_windows_best_available_matches_header() -> bool;
    }
    // SAFETY: This takes no arguments and only compares immutable C++ objects.
    assert!(unsafe { studio_windows_best_available_matches_header() });
    let profile = rclrs::QoSProfile::best_available();
    assert_eq!(
        profile.history,
        rclrs::QoSHistoryPolicy::KeepLast { depth: 10 }
    );
    assert_eq!(
        profile.reliability,
        rclrs::QoSReliabilityPolicy::BestAvailable
    );
    assert_eq!(
        profile.durability,
        rclrs::QoSDurabilityPolicy::BestAvailable
    );
    assert_eq!(
        profile.liveliness,
        rclrs::QoSLivelinessPolicy::BestAvailable
    );
    assert!(!profile.avoid_ros_namespace_conventions);
}
