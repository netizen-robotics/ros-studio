use super::*;
use crate::native_schema::test_support::Fixture;
use serde_json::json;
use studio_protocol::pose_snapshot::{ODOMETRY_MESSAGE_TYPE, POSE_MESSAGE_TYPE};
fn pose() -> serde_json::Value {
    json!({"position":{"x":1.,"y":2.,"z":3.},"orientation":{"x":0.,"y":0.,"z":0.,"w":1.}})
}
fn covariance() -> [f64; 36] {
    let v = [1., 2., 3., 0.1, 0.2, 0.3];
    std::array::from_fn(|i| v[i / 6] * v[i % 6])
}
fn message() -> serde_json::Value {
    json!({"header":{"frame_id":"odom","stamp":{"sec":2,"nanosec":3}},"pose":{"pose":pose(),"covariance":covariance().as_slice()}})
}
#[test]
fn standard_pose_covariance_c_storage_copies_all_values_without_preview_truncation() {
    let fixture = Fixture::new("geometry_msgs", "PoseWithCovarianceStamped", message());
    let result = unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }.unwrap();
    assert_eq!(result.kind, Kind::PoseWithCovariance);
    assert_eq!(
        result.source,
        Source {
            frame_id: "odom".into(),
            child_frame_id: None
        }
    );
    assert_eq!(result.acquisition_stamp_ns, 2_000_000_003);
    assert_eq!(result.components, [1., 2., 3., 0., 0., 0., 1.]);
    assert_eq!(result.covariance.row_major(), &covariance());
    assert_eq!(Decoded::decode(&result.encode().unwrap()).unwrap(), result);
    fixture.write(json!({"header":{"stamp":{"sec":0,"nanosec":0}}}));
    assert_eq!(
        unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }
            .unwrap()
            .acquisition_stamp_ns,
        0
    );
    fixture.write(json!({"header":{"stamp":{"sec":i32::MAX,"nanosec":999999999}}}));
    assert_eq!(
        unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }
            .unwrap()
            .acquisition_stamp_ns,
        studio_protocol::latest_pose::MAX_STAMP_NS
    );
    // The original complete copy remains independent of subsequent C writes.
    assert_eq!(result.acquisition_stamp_ns, 2_000_000_003);
}
#[test]
fn odometry_uses_header_pose_and_keeps_child_frame_as_metadata_only() {
    let mut value = message();
    value["child_frame_id"] = json!("base_link");
    value["twist"] = json!({"twist":{"linear":{"x":42.,"y":43.,"z":44.}}});
    let fixture = Fixture::new("nav_msgs", "Odometry", value);
    let result =
        unsafe { native_pose_covariance(fixture.0, "/odom", ODOMETRY_MESSAGE_TYPE) }.unwrap();
    assert_eq!(result.kind, Kind::Odometry);
    assert_eq!(
        result.source,
        Source {
            frame_id: "odom".into(),
            child_frame_id: Some("base_link".into())
        }
    );
    assert_eq!(result.components[..3], [1., 2., 3.]);
    assert_eq!(result.covariance.row_major(), &covariance());
    fixture.write(json!({"child_frame_id":""}));
    assert_eq!(
        unsafe { native_pose_covariance(fixture.0, "/odom", ODOMETRY_MESSAGE_TYPE) }
            .unwrap()
            .source
            .child_frame_id,
        Some(String::new())
    );
    fixture.write(json!({"child_frame_id":"/invalid"}));
    assert!(unsafe { native_pose_covariance(fixture.0, "/odom", ODOMETRY_MESSAGE_TYPE) }.is_err());
}
#[test]
fn invalid_covariance_and_pose_values_reject_actual_native_storage() {
    let fixture = Fixture::new("geometry_msgs", "PoseWithCovarianceStamped", message());
    let invalid = [1e12, 1., 1., -0.01, 1., 1.];
    let values: Vec<_> = (0..36)
        .map(|i| if i / 6 == i % 6 { invalid[i / 6] } else { 0. })
        .collect();
    fixture.write(json!({"pose":{"covariance":values}}));
    assert!(unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }.is_err());
    fixture.write(
        json!({"pose":{"covariance":covariance().as_slice(),"pose":{"orientation":{"w":0.}}}}),
    );
    assert!(unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }.is_err());
    fixture.write(json!({"pose":{"pose":pose()}}));
    unsafe {
        let (pm, p) = fixture.root("pose", 18, 0);
        let (cm, c) = field(sp_nested(pm), p, "covariance", 2, 1).unwrap();
        *sp_element(cm, c, 35).cast::<f64>() = f64::NAN;
        assert!(native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE).is_err());
    }
}
#[test]
fn invalid_header_type_schema_and_null_owner_fail_before_extraction() {
    let fixture = Fixture::new("geometry_msgs", "PoseWithCovarianceStamped", message());
    fixture.write(json!({"header":{"frame_id":"/bad"}}));
    assert!(unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }.is_err());
    fixture.write(json!({"header":{"frame_id":"odom","stamp":{"sec":-1,"nanosec":0}}}));
    assert!(unsafe { native_pose_covariance(fixture.0, "/pose", POSE_MESSAGE_TYPE) }.is_err());
    assert!(unsafe { native_pose_covariance(fixture.0, "bad", POSE_MESSAGE_TYPE) }.is_err());
    assert!(unsafe { native_pose_covariance(fixture.0, "/pose", ODOMETRY_MESSAGE_TYPE) }.is_err());
    assert!(
        unsafe { native_pose_covariance(fixture.0, "/pose", "geometry_msgs/msg/PoseStamped") }
            .is_err()
    );
    assert!(
        unsafe { native_pose_covariance(std::ptr::null_mut(), "/pose", POSE_MESSAGE_TYPE) }
            .is_err()
    );
    let wrong = Fixture::new(
        "geometry_msgs",
        "PoseStamped",
        json!({"header":{"frame_id":"odom"},"pose":pose()}),
    );
    assert!(unsafe { native_pose_covariance(wrong.0, "/pose", POSE_MESSAGE_TYPE) }.is_err());
}
