// rclrs 0.8.0 binds this header-local constant as an external symbol. MSVC
// requires a definition even when best_available() shares an otherwise-used
// codegen unit. Copy the installed ROS header value, keeping its exact policy.
#define rmw_qos_profile_best_available studio_header_best_available
#include <rmw/qos_profiles.h>
#undef rmw_qos_profile_best_available

extern "C" {
extern const rmw_qos_profile_t rmw_qos_profile_best_available = studio_header_best_available;

bool studio_windows_best_available_matches_header() {
  const auto &actual = rmw_qos_profile_best_available;
  const auto &expected = studio_header_best_available;
  return actual.history == expected.history && actual.depth == expected.depth &&
         actual.reliability == expected.reliability && actual.durability == expected.durability &&
         actual.deadline.sec == expected.deadline.sec && actual.deadline.nsec == expected.deadline.nsec &&
         actual.lifespan.sec == expected.lifespan.sec && actual.lifespan.nsec == expected.lifespan.nsec &&
         actual.liveliness == expected.liveliness &&
         actual.liveliness_lease_duration.sec == expected.liveliness_lease_duration.sec &&
         actual.liveliness_lease_duration.nsec == expected.liveliness_lease_duration.nsec &&
         actual.avoid_ros_namespace_conventions == expected.avoid_ros_namespace_conventions;
}
}
