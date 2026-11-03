//! Independent source-origin-to-fixed geometry checks; no native/GPU acceptance.
use studio_desktop::spatial::{
    combined::{
        combined_vertices, CombinedSceneFrame, MapLayerFrame, PathLayerFrame, PointStyle,
        PoseLayerFrame,
    },
    map_geometry::map_vertices,
    mapping_live::{decode_mapping_sample, WorldGeometry},
    CameraPose, Point3, RigidTransform,
};
use studio_protocol::{
    mapping::{Geometry, PathPose, Sample},
    spatial::TfOutcome,
};
fn sample(geometry: Geometry) -> Sample {
    let q = 0.5_f64.sqrt();
    Sample {
        contract_version: 1,
        topic: "/mapping/data".into(),
        message_type: geometry.message_type().into(),
        fixed_frame: "map".into(),
        source_frame: "mapping_source".into(),
        acquisition_stamp_ns: 1,
        geometry,
        tf: TfOutcome::Ready {
            components: [1., 2., 3., 0., 0., q, q],
        },
        counters: Default::default(),
    }
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
fn near(a: Point3, b: [f64; 3]) {
    for (x, y) in a.0.into_iter().zip(b) {
        assert!((x - y).abs() < 1e-10, "{a:?} != {b:?}");
    }
}
#[test]
fn rotated_map_cell_centers_align_with_sensor_fixture_and_occupancy_is_preserved() {
    let frame = decode_mapping_sample(&sample(grid())).unwrap();
    let Some(WorldGeometry::Map(map)) = frame.geometry else {
        panic!("map")
    };
    assert_eq!((map.width, map.height), (3, 2));
    assert_eq!(&*map.cells, &[-1, 0, 25, 50, 75, 100]);
    // Prepared corners form a shared row-major (width+1)×(height+1) lattice.
    assert_eq!(map.corners.len(), 12);
    near(map.corners[0], [1.25, 3.25, 3.]);
    near(map.corners[1], [0.75, 3.25, 3.]);
    near(map.corners[4], [1.25, 2.75, 3.]);
    let center = Point3(std::array::from_fn(|i| {
        (map.corners[0].0[i] + map.corners[5].0[i]) * 0.5
    }));
    near(center, [1., 3., 3.]);
    let vertices =
        map_vertices(&map, CameraPose::orbit(0.5, 0.8, 12., [0.; 3]), [800, 600]).unwrap();
    assert_eq!(vertices.len(), 36);
    assert_ne!(
        vertices[0].color, vertices[6].color,
        "unknown must differ from free"
    );
    assert_ne!(
        vertices[6].color, vertices[30].color,
        "free must differ from occupied"
    );
}
#[test]
fn pose_orientation_and_path_use_the_header_transform_once_and_missing_clears() {
    let q = 0.5_f64.sqrt();
    let pose = sample(Geometry::Pose {
        components: [1., 0., 0., 0., 0., q, q],
    });
    let frame = decode_mapping_sample(&pose).unwrap();
    let Some(WorldGeometry::Pose(transform)) = frame.geometry else {
        panic!("pose")
    };
    near(transform.apply(Point3([0.; 3])), [1., 3., 3.]);
    near(transform.apply(Point3([0.35, 0., 0.])), [0.65, 3., 3.]);
    let path = sample(Geometry::Path {
        poses: vec![
            PathPose {
                frame_id: String::new(),
                stamp_ns: 0,
                components: [1., 0., 0., 0., 0., 0., 1.],
            },
            PathPose {
                frame_id: "mapping_source".into(),
                stamp_ns: 17,
                components: [1., 0.5, 0., 0., 0., 0., 1.],
            },
            PathPose {
                frame_id: String::new(),
                stamp_ns: 0,
                components: [0.5, 0., 0., 0., 0., 0., 1.],
            },
        ],
    });
    let frame = decode_mapping_sample(&path).unwrap();
    let Some(WorldGeometry::Path(points)) = frame.geometry else {
        panic!("path")
    };
    for (point, want) in points
        .iter()
        .zip([[1., 3., 3.], [0.5, 3., 3.], [1., 2.5, 3.]])
    {
        near(*point, want);
    }
    for mut value in [pose, path, sample(grid())] {
        for tf in [
            TfOutcome::Missing,
            TfOutcome::Extrapolated,
            TfOutcome::LookupTimeout,
        ] {
            value.tf = tf;
            assert!(decode_mapping_sample(&value).unwrap().geometry.is_none());
        }
    }
}
#[test]
fn independent_mapping_layers_combine_without_retaining_missing_geometry() {
    let frame = decode_mapping_sample(&sample(grid())).unwrap();
    let Some(WorldGeometry::Map(map)) = frame.geometry else {
        panic!("map")
    };
    let points = [Point3([1., 3., 3.]), Point3([0.5, 3., 3.])];
    let frame = CombinedSceneFrame {
        tf: None,
        fixed_frame: "map",
        robot: None,
        layers: &[],
        map: Some(MapLayerFrame {
            id: 1,
            local_generation: 1,
            sequence: 1,
            map: Some(&map),
            status: "Receiving",
            error: None,
        }),
        pose: Some(PoseLayerFrame {
            id: 2,
            pose: Some(RigidTransform::identity()),
            status: "Receiving",
            error: None,
        }),
        path: Some(PathLayerFrame {
            id: 3,
            points: &points,
            style: PointStyle::default(),
            status: "Receiving",
            error: None,
        }),
    };
    frame.validate().unwrap();
    let camera = CameraPose::orbit(0.5, 0.8, 12., [0.; 3]);
    let all = combined_vertices(&frame, camera, [800, 600]).unwrap();
    let cleared = CombinedSceneFrame {
        tf: None,
        map: Some(MapLayerFrame {
            id: 1,
            local_generation: 1,
            sequence: 2,
            map: None,
            status: "Missing",
            error: None,
        }),
        ..frame
    };
    assert!(
        combined_vertices(&cleared, camera, [800, 600])
            .unwrap()
            .len()
            < all.len()
    );
    let invalid = CombinedSceneFrame {
        tf: None,
        path: Some(PathLayerFrame {
            id: 3,
            points: &points,
            style: PointStyle::default(),
            status: "Error",
            error: Some("must clear"),
        }),
        ..cleared
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn maximum_map_and_path_are_complete_and_valid_empty_messages_clear_geometry() {
    let identity = [0., 0., 0., 0., 0., 0., 1.];
    let camera = CameraPose::orbit(0.5, 0.8, 12., [0.; 3]);
    let mut value = sample(Geometry::OccupancyGrid {
        width: 256,
        height: 256,
        resolution: 0.5,
        origin: identity,
        map_load_time_ns: 0,
        cells: vec![50; 65536],
    });
    let frame = decode_mapping_sample(&value).unwrap();
    let Some(WorldGeometry::Map(map)) = frame.geometry else {
        panic!("map")
    };
    assert_eq!(map.corners.len(), 257 * 257);
    assert_eq!(
        map_vertices(&map, camera, [800, 600]).unwrap().len(),
        65536 * 6
    );
    value.geometry = Geometry::OccupancyGrid {
        width: 0,
        height: 0,
        resolution: 1.,
        origin: identity,
        map_load_time_ns: 0,
        cells: vec![],
    };
    let frame = decode_mapping_sample(&value).unwrap();
    let Some(WorldGeometry::Map(map)) = frame.geometry else {
        panic!("empty map")
    };
    assert!(map_vertices(&map, camera, [800, 600]).unwrap().is_empty());
    let value = sample(Geometry::Path {
        poses: (0..1024)
            .map(|i| PathPose {
                frame_id: String::new(),
                stamp_ns: 0,
                components: [i as f64 / 1024., 0., 0., 0., 0., 0., 1.],
            })
            .collect(),
    });
    let frame = decode_mapping_sample(&value).unwrap();
    let Some(WorldGeometry::Path(points)) = frame.geometry else {
        panic!("path")
    };
    assert_eq!(points.len(), 1024);
    near(points[1023], [1., 2. + 1023. / 1024., 3.]);
    let frame = decode_mapping_sample(&sample(Geometry::Path { poses: vec![] })).unwrap();
    let Some(WorldGeometry::Path(points)) = frame.geometry else {
        panic!("empty path")
    };
    assert!(points.is_empty());
}
