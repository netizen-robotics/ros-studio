//! Desktop borrowing adapter; decoding is shared pure protocol code.
#[cfg(test)]
use super::RigidTransform;
use super::{Clock, Scan};
pub use studio_protocol::spatial_input::{
    decode_tf_message, SpatialClockPolicy, SpatialSampleError, TfStreamKind, TypedTfBatch,
    TypedTransform, MAX_TF_TRANSFORMS, SCAN_MESSAGE_TYPE, TF_MESSAGE_TYPE,
};
#[cfg(test)]
use studio_protocol::structured::{ClockDomain, FloatValue, PathSegment, Sample, Value, MAX_ITEMS};
#[derive(Debug)]
pub struct TypedScan {
    pub frame: String,
    pub stamp_ns: i64,
    pub angle_min: f64,
    pub angle_max: f64,
    pub angle_increment: f64,
    pub time_increment: f64,
    pub scan_time: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub ranges: Vec<f32>,
    pub intensities: Vec<f32>,
}
impl TypedScan {
    pub fn as_scan(&self) -> Scan<'_> {
        Scan {
            frame: &self.frame,
            stamp_ns: self.stamp_ns,
            clock: Clock::System,
            complete: true,
            angle_min: self.angle_min,
            angle_increment: self.angle_increment,
            range_min: self.range_min,
            range_max: self.range_max,
            ranges: &self.ranges,
        }
    }
}
pub fn decode_laser_scan(
    sample: &studio_protocol::structured::Sample,
    policy: SpatialClockPolicy,
) -> Result<TypedScan, SpatialSampleError> {
    studio_protocol::spatial_input::decode_laser_scan(sample, policy).map(|scan| TypedScan {
        frame: scan.frame,
        stamp_ns: scan.stamp_ns,
        angle_min: scan.angle_min,
        angle_max: scan.angle_max,
        angle_increment: scan.angle_increment,
        time_increment: scan.time_increment,
        scan_time: scan.scan_time,
        range_min: scan.range_min,
        range_max: scan.range_max,
        ranges: scan.ranges,
        intensities: scan.intensities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{Scene, TfOutcome};
    use studio_protocol::structured::{Field, SourceTime, CONTRACT_VERSION};

    const SYSTEM: SpatialClockPolicy = SpatialClockPolicy::VerifiedSystem;
    fn f(number: f64) -> Value {
        Value::Float {
            value: if number.is_nan() {
                FloatValue::Nan
            } else if number == f64::INFINITY {
                FloatValue::PositiveInfinity
            } else if number == f64::NEG_INFINITY {
                FloatValue::NegativeInfinity
            } else {
                FloatValue::Finite(number.to_string())
            },
        }
    }
    fn text(value: &str) -> Value {
        Value::Text {
            value: value.into(),
            original_bytes: value.len(),
        }
    }
    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object {
            original_fields: fields.len(),
            fields: fields
                .into_iter()
                .map(|(name, value)| Field {
                    name: name.into(),
                    value,
                })
                .collect(),
        }
    }
    fn array(items: Vec<Value>) -> Value {
        Value::Array {
            original_items: items.len(),
            items,
        }
    }
    fn header(frame: &str, sec: i32, nsec: u32) -> Value {
        object(vec![
            ("frame_id", text(frame)),
            (
                "stamp",
                object(vec![
                    (
                        "sec",
                        Value::Signed {
                            value: sec.to_string(),
                        },
                    ),
                    (
                        "nanosec",
                        Value::Unsigned {
                            value: nsec.to_string(),
                        },
                    ),
                ]),
            ),
        ])
    }
    fn scan() -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            topic: "/scan".into(),
            message_type: SCAN_MESSAGE_TYPE.into(),
            source_time: None,
            value: object(vec![
                ("header", header("laser", 2, 3)),
                ("angle_min", f(-0.5)),
                ("angle_max", f(0.75)),
                ("angle_increment", f(0.25)),
                ("time_increment", f(0.001)),
                ("scan_time", f(0.1)),
                ("range_min", f(0.1)),
                ("range_max", f(10.0)),
                (
                    "ranges",
                    array(vec![
                        f(1.0),
                        f(f64::NAN),
                        f(f64::INFINITY),
                        f(f64::NEG_INFINITY),
                        f(11.0),
                        f(2.0),
                    ]),
                ),
                ("intensities", array(vec![])),
            ]),
        }
    }
    fn transform(parent: &str, child: &str, sec: i32) -> Value {
        object(vec![
            ("header", header(parent, sec, 0)),
            ("child_frame_id", text(child)),
            (
                "transform",
                object(vec![
                    (
                        "translation",
                        object(vec![("x", f(1.0)), ("y", f(-2.0)), ("z", f(0.0))]),
                    ),
                    (
                        "rotation",
                        object(vec![
                            ("x", f(0.0)),
                            ("y", f(0.0)),
                            ("z", f(0.0)),
                            ("w", f(1.0)),
                        ]),
                    ),
                ]),
            ),
        ])
    }
    fn tf(items: Vec<Value>) -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            topic: "/arbitrary/identified_tf".into(),
            message_type: TF_MESSAGE_TYPE.into(),
            source_time: None,
            value: object(vec![("transforms", array(items))]),
        }
    }
    fn field<'a>(value: &'a mut Value, path: &[&str]) -> &'a mut Value {
        let (name, rest) = path.split_first().unwrap();
        let Value::Object { fields, .. } = value else {
            panic!("test object")
        };
        let value = &mut fields
            .iter_mut()
            .find(|field| field.name == *name)
            .unwrap()
            .value;
        if rest.is_empty() {
            value
        } else {
            field(value, rest)
        }
    }
    fn set(sample: &mut Sample, path: &[&str], value: Value) {
        *field(&mut sample.value, path) = value;
    }

    #[test]
    fn typed_scan_preserves_acquisition_and_explicit_nonfinite_rays_for_scene_skips() {
        let scan = decode_laser_scan(&scan(), SYSTEM).unwrap();
        assert_eq!(scan.frame, "laser");
        assert_eq!(scan.stamp_ns, 2_000_000_003);
        assert_eq!(scan.angle_max, 0.75);
        assert!(scan.ranges[1].is_nan());
        assert_eq!(scan.ranges[2], f32::INFINITY);
        assert_eq!(scan.ranges[3], f32::NEG_INFINITY);
        let mut scene = Scene::default();
        scene.set_fixed_frame("map").unwrap();
        scene.add_layer(1).unwrap();
        scene.start().unwrap();
        scene
            .update(
                scene.layer_token(1).unwrap(),
                scan.as_scan(),
                TfOutcome::Ready {
                    source: "laser",
                    target: "map",
                    stamp_ns: scan.stamp_ns,
                    transform: RigidTransform::identity(),
                },
            )
            .unwrap();
        assert_eq!(scene.layers()[0].skipped_rays, 4);
        assert_eq!(scene.layers()[0].points().len(), 2);
    }

    #[test]
    fn scan_requires_complete_metadata_valid_angles_and_nonzero_stamp() {
        let mut sample = scan();
        let Value::Array { original_items, .. } = field(&mut sample.value, &["ranges"]) else {
            unreachable!()
        };
        *original_items += 1;
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::Incomplete
        );
        for (path, value, expected) in [
            (vec!["angle_increment"], f(0.0), "angle_increment"),
            (vec!["angle_max"], f(1.0), "angle_max"),
            (vec!["range_max"], f(0.1), "range_max"),
            (vec!["time_increment"], f(-1.0), "scan_time"),
            (vec!["ranges"], array(vec![]), "ranges"),
            (vec!["intensities"], array(vec![f(1.0)]), "intensities"),
            (vec!["header", "frame_id"], text("bad frame"), "frame_id"),
            (
                vec!["header", "stamp", "sec"],
                Value::Signed { value: "-1".into() },
                "stamp",
            ),
            (
                vec!["header", "stamp", "nanosec"],
                Value::Unsigned {
                    value: "1000000000".into(),
                },
                "stamp",
            ),
            (vec!["range_min"], f(f64::NAN), "range_min"),
            (vec!["scan_time"], f(1e100), "scan_time"),
        ] {
            let mut sample = scan();
            set(&mut sample, &path, value);
            assert_eq!(
                decode_laser_scan(&sample, SYSTEM).unwrap_err(),
                SpatialSampleError::InvalidField(expected)
            );
        }
        let mut sample = scan();
        set(&mut sample, &["ranges"], array(vec![text("1.0")]));
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::InvalidField("ranges")
        );
        let mut sample = scan();
        set(&mut sample, &["header"], header("laser", 0, 0));
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::InvalidField("stamp")
        );
    }

    #[test]
    fn clock_policy_and_source_provenance_are_explicit_and_never_substitute_receipt_time() {
        let mut sample = scan();
        assert_eq!(
            decode_laser_scan(&sample, SpatialClockPolicy::Unknown).unwrap_err(),
            SpatialSampleError::ClockUnverified
        );
        assert_eq!(
            decode_laser_scan(&sample, SpatialClockPolicy::Simulated).unwrap_err(),
            SpatialSampleError::UnsupportedClock
        );
        sample.source_time = Some(SourceTime {
            seconds: 2,
            nanoseconds: 3,
            clock: ClockDomain::System,
            header_path: vec![
                PathSegment::Member("header".into()),
                PathSegment::Member("stamp".into()),
            ],
        });
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap().stamp_ns,
            2_000_000_003
        );
        sample.source_time.as_mut().unwrap().nanoseconds = 4;
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::SourceTimeMismatch
        );
        sample.source_time.as_mut().unwrap().nanoseconds = 3;
        sample.source_time.as_mut().unwrap().clock = ClockDomain::Ros { epoch: 1 };
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::SourceTimeMismatch
        );
        sample.source_time.as_mut().unwrap().clock = ClockDomain::System;
        sample.source_time.as_mut().unwrap().header_path =
            vec![PathSegment::Member("header".into())];
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::SourceTimeMismatch
        );
    }

    #[test]
    fn typed_scan_capacity_is_2048_not_the_scene_4096_ray_limit() {
        let mut sample = scan();
        set(&mut sample, &["angle_min"], f(0.0));
        set(&mut sample, &["angle_increment"], f(0.125));
        set(
            &mut sample,
            &["angle_max"],
            f((MAX_ITEMS - 1) as f64 * 0.125),
        );
        set(&mut sample, &["ranges"], array(vec![f(1.0); MAX_ITEMS]));
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap().ranges.len(),
            MAX_ITEMS
        );
        set(&mut sample, &["ranges"], array(vec![f(1.0); MAX_ITEMS + 1]));
        assert_eq!(
            decode_laser_scan(&sample, SYSTEM).unwrap_err(),
            SpatialSampleError::InvalidContract
        );
    }

    #[test]
    fn tf_batch_preserves_exact_components_static_zero_stamp_and_identified_kind() {
        let sample = tf(vec![
            transform("map", "base", 2),
            transform("base", "laser", 3),
        ]);
        let decoded = decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap();
        assert_eq!(decoded.kind, TfStreamKind::Dynamic);
        assert_eq!(decoded.transforms.len(), 2);
        assert_eq!(
            decoded.transforms[0],
            TypedTransform {
                parent: "map".into(),
                child: "base".into(),
                stamp_ns: 2_000_000_000,
                components: [1.0, -2.0, 0.0, 0.0, 0.0, 0.0, 1.0]
            }
        );
        assert_eq!(decoded.transforms[1].stamp_ns, 3_000_000_000);
        let sample = tf(vec![transform("base", "laser", 0)]);
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap_err(),
            SpatialSampleError::InvalidField("stamp")
        );
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Static, SYSTEM)
                .unwrap()
                .transforms[0]
                .stamp_ns,
            0
        );
        assert!(decode_tf_message(&tf(vec![]), TfStreamKind::Static, SYSTEM)
            .unwrap()
            .transforms
            .is_empty());
    }

    #[test]
    fn tf_timestamp_provenance_names_an_actual_transform_header() {
        let mut sample = tf(vec![transform("map", "base", i32::MAX)]);
        sample.source_time = Some(SourceTime {
            seconds: i64::from(i32::MAX),
            nanoseconds: 0,
            clock: ClockDomain::System,
            header_path: vec![
                PathSegment::Member("transforms".into()),
                PathSegment::Index(0),
                PathSegment::Member("header".into()),
                PathSegment::Member("stamp".into()),
            ],
        });
        let decoded = decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap();
        assert_eq!(
            decoded.transforms[0].stamp_ns,
            i64::from(i32::MAX) * 1_000_000_000
        );
        sample.source_time.as_mut().unwrap().header_path[1] = PathSegment::Index(1);
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap_err(),
            SpatialSampleError::SourceTimeMismatch
        );
        sample.source_time = None;
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Dynamic, SpatialClockPolicy::Unknown)
                .unwrap_err(),
            SpatialSampleError::ClockUnverified
        );
    }

    #[test]
    fn tf_batches_reject_partial_conflicting_or_invalid_rigid_transforms() {
        let mut sample = tf(vec![transform("map", "laser", 2)]);
        let Value::Array { original_items, .. } = field(&mut sample.value, &["transforms"]) else {
            unreachable!()
        };
        *original_items += 1;
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap_err(),
            SpatialSampleError::Incomplete
        );
        assert_eq!(
            decode_tf_message(
                &tf(vec![
                    transform("map", "laser", 2),
                    transform("base", "laser", 2)
                ]),
                TfStreamKind::Dynamic,
                SYSTEM
            )
            .unwrap_err(),
            SpatialSampleError::DuplicateChild
        );
        assert_eq!(
            decode_tf_message(
                &tf(vec![transform("laser", "laser", 2)]),
                TfStreamKind::Dynamic,
                SYSTEM
            )
            .unwrap_err(),
            SpatialSampleError::InvalidField("child_frame_id")
        );
        for (path, value) in [
            (vec!["transform", "rotation", "w"], f(0.0)),
            (vec!["transform", "translation", "x"], f(1_000_001.0)),
            (vec!["transform", "rotation", "x"], f(f64::NAN)),
        ] {
            let mut transform = transform("map", "laser", 2);
            *field(&mut transform, &path) = value;
            assert!(
                decode_tf_message(&tf(vec![transform]), TfStreamKind::Dynamic, SYSTEM).is_err()
            );
        }
        // Smaller malformed items fit the typed tree budget and test the adapter
        // cap before per-transform validation or any native cache insertion.
        let oversized = tf(vec![object(vec![]); MAX_TF_TRANSFORMS + 1]);
        assert_eq!(
            decode_tf_message(&oversized, TfStreamKind::Static, SYSTEM).unwrap_err(),
            SpatialSampleError::TransformLimit
        );
        let mut sample = tf(vec![transform("map", "laser", 2)]);
        sample.message_type = SCAN_MESSAGE_TYPE.into();
        assert_eq!(
            decode_tf_message(&sample, TfStreamKind::Dynamic, SYSTEM).unwrap_err(),
            SpatialSampleError::WrongMessageType(TF_MESSAGE_TYPE)
        );
    }
}
