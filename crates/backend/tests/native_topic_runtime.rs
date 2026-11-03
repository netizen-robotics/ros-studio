//! Explicit localhost-only runtime regression for platform library resolution.
use studio_backend::native_topic::NativeTopic;

#[test]
#[ignore = "requires an isolated real ROS runtime on localhost domain 187"]
fn native_topic_resolves_runtime_libraries_and_opens_reader() {
    assert_eq!(std::env::var("ROS_DOMAIN_ID").as_deref(), Ok("187"));
    assert_eq!(std::env::var("ROS_LOCALHOST_ONLY").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("ROS_AUTOMATIC_DISCOVERY_RANGE").as_deref(),
        Ok("LOCALHOST")
    );
    for name in [
        "ROS_STATIC_PEERS",
        "CYCLONEDDS_URI",
        "ROS_DISCOVERY_SERVER",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_DEFAULT_PROFILES_FILE",
    ] {
        assert!(std::env::var(name).unwrap_or_default().is_empty(), "{name}");
    }
    let _reader = NativeTopic::open(
        "/studio_fixture",
        "std_msgs/msg/String",
        false,
        false,
        false,
    )
    .expect("resolve and load the installed platform message libraries");
}
