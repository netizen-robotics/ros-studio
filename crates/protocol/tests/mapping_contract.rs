//! Independent read-only mapping contract cases; no native ROS dependency.
use studio_protocol::{
    mapping::{self, Geometry, PathPose, Sample},
    spatial::TfOutcome,
};
fn identity() -> [f64; 7] {
    [0., 0., 0., 0., 0., 0., 1.]
}
fn grid() -> Geometry {
    let q = 0.5_f64.sqrt();
    Geometry::OccupancyGrid {
        width: 3,
        height: 2,
        resolution: 0.5,
        origin: [1.25, -0.25, 0., 0., 0., q, q],
        map_load_time_ns: 0,
        cells: vec![-1, 0, 25, 50, 75, 100],
    }
}
fn sample(geometry: Geometry) -> Sample {
    Sample {
        contract_version: 1,
        topic: "/mapping/map".into(),
        message_type: geometry.message_type().into(),
        fixed_frame: "map".into(),
        source_frame: "mapping_source".into(),
        acquisition_stamp_ns: 1,
        geometry,
        tf: TfOutcome::Ready {
            components: identity(),
        },
        counters: Default::default(),
    }
}
#[test]
fn rotated_map_pose_path_roundtrip_keep_complete_source_geometry_and_acquisition_policy() {
    for geometry in [
        grid(),
        Geometry::Pose {
            components: identity(),
        },
        Geometry::Path {
            poses: vec![
                PathPose {
                    frame_id: String::new(),
                    stamp_ns: 0,
                    components: identity(),
                },
                PathPose {
                    frame_id: "mapping_source".into(),
                    stamp_ns: 123,
                    components: identity(),
                },
            ],
        },
    ] {
        let value = sample(geometry);
        let bytes = value.encode().unwrap();
        let decoded: Sample = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, value);
        decoded.validate().unwrap();
    }
}
#[test]
fn map_exact_capacity_empty_and_invalid_metadata_are_distinct() {
    let mut value = sample(Geometry::OccupancyGrid {
        width: 256,
        height: 256,
        resolution: 0.5,
        origin: identity(),
        map_load_time_ns: mapping::MAX_STAMP_NS,
        cells: vec![100; 65536],
    });
    assert!(value.encode().unwrap().len() < mapping::MAX_SAMPLE_BYTES);
    let base = value.clone();
    for case in 0..11 {
        value = base.clone();
        if let Geometry::OccupancyGrid {
            width,
            height,
            resolution,
            origin,
            map_load_time_ns,
            cells,
        } = &mut value.geometry
        {
            match case {
                0 => *width = 257,
                1 => *height = 257,
                2 => {
                    cells.pop();
                }
                3 => cells[0] = -2,
                4 => cells[0] = 101,
                5 => *resolution = 0.,
                6 => *resolution = f64::NAN,
                7 => origin[6] = 0.,
                8 => *map_load_time_ns = -1,
                9 => *map_load_time_ns = mapping::MAX_STAMP_NS + 1,
                _ => {
                    *width = 0;
                    cells.clear();
                }
            }
        }
        assert!(value.validate().is_err(), "invalid grid case {case}");
    }
    value = sample(Geometry::OccupancyGrid {
        width: 0,
        height: 0,
        resolution: 1.,
        origin: identity(),
        map_load_time_ns: 0,
        cells: vec![],
    });
    value.validate().unwrap();
    if let Geometry::OccupancyGrid {
        origin,
        width,
        height,
        ..
    } = &mut value.geometry
    {
        *width = 1;
        *height = 1;
        origin[0] = mapping::MAX_COORDINATE;
    }
    assert!(value.validate().is_err());
    let mut corner = sample(Geometry::OccupancyGrid {
        width: 1,
        height: 1,
        resolution: 1.,
        origin: [mapping::MAX_COORDINATE, 0., 0., 0., 0., 0., 1.],
        map_load_time_ns: 0,
        cells: vec![0],
    });
    assert!(
        corner.validate().is_err(),
        "affine extent must fit even when origin fits"
    );
    if let Geometry::OccupancyGrid { origin, .. } = &mut corner.geometry {
        origin[0] -= 1.;
    }
    corner.validate().unwrap();
}
#[test]
fn pose_and_path_validate_orientation_frames_times_and_exact_pose_limit() {
    let pose = PathPose {
        frame_id: "mapping_source".into(),
        stamp_ns: 0,
        components: identity(),
    };
    let mut value = sample(Geometry::Path {
        poses: vec![pose.clone(); 1024],
    });
    value.encode().unwrap();
    if let Geometry::Path { poses } = &mut value.geometry {
        poses.push(pose);
    }
    assert!(value.validate().is_err());
    for case in 0..5 {
        let mut p = PathPose {
            frame_id: String::new(),
            stamp_ns: 0,
            components: identity(),
        };
        match case {
            0 => p.frame_id = "other".into(),
            1 => p.stamp_ns = -1,
            2 => p.stamp_ns = mapping::MAX_STAMP_NS + 1,
            3 => p.components[3] = 0.5,
            _ => p.components[0] = f64::INFINITY,
        }
        assert!(sample(Geometry::Path { poses: vec![p] })
            .validate()
            .is_err());
    }
    sample(Geometry::Path { poses: vec![] }).validate().unwrap();
    for components in [
        [0.; 7],
        [0., 0., 0., 0., 0., 0., 2.],
        [1_000_001., 0., 0., 0., 0., 0., 1.],
    ] {
        assert!(sample(Geometry::Pose { components }).validate().is_err());
    }
}
#[test]
fn source_identity_positive_header_time_tf_and_strict_fields_are_bounded() {
    let base = sample(grid());
    for case in 0..8 {
        let mut value = base.clone();
        match case {
            0 => value.acquisition_stamp_ns = 0,
            1 => value.acquisition_stamp_ns = mapping::MAX_STAMP_NS + 1,
            2 => value.source_frame = "bad frame".into(),
            3 => value.fixed_frame = "/map".into(),
            4 => value.topic = "/bad//topic".into(),
            5 => value.message_type = mapping::POSE_MESSAGE_TYPE.into(),
            6 => value.contract_version = 2,
            _ => {
                value.tf = TfOutcome::Ready {
                    components: [0.; 7],
                }
            }
        }
        assert!(value.encode().is_err(), "envelope case{case}");
    }
    for tf in [
        TfOutcome::Missing,
        TfOutcome::Stale,
        TfOutcome::Extrapolated,
        TfOutcome::LookupTimeout,
    ] {
        let mut value = base.clone();
        value.tf = tf;
        value.validate().unwrap();
    }
    let mut value = serde_json::to_value(base).unwrap();
    value["geometry"]["runtime"] = true.into();
    assert!(serde_json::from_value::<Sample>(value).is_err());
}
