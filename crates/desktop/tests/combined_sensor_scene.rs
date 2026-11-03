//! Pure combined geometry checks; no UI/GPU/ROS acceptance claims.
use std::{path::Path, sync::atomic::AtomicBool};
use studio_desktop::{
    robot::{load_model, LoadOptions},
    spatial::{
        combined::{
            combined_vertices, CombinedSceneFrame, PointLayerFrame, PointStyle, RobotGeometryFrame,
        },
        robot_geometry::prepare_robot,
        CameraPose, Point3, RigidTransform,
    },
};
fn style(color: [f32; 4]) -> PointStyle {
    PointStyle {
        rgba: color,
        size_px: 3.,
    }
}
#[test]
fn robot_two_scans_and_noncoplanar_cloud_share_analytic_fixed_frame_projection() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/robot_visualizer/articulated.urdf");
    let options = LoadOptions {
        package_roots: [("fixture_robot".into(), path.parent().unwrap().to_owned())].into(),
    };
    let cancel = AtomicBool::new(false);
    let model = load_model(&path, &options, &cancel).unwrap();
    let prepared = prepare_robot(&model, &cancel).unwrap();
    let poses = vec![Some(RigidTransform::identity()); model.links.len()];
    let scan_a = [Point3([1., 3., 3.]), Point3([0., 2., 3.])];
    let scan_b = [Point3([-1., -2., 0.5]), Point3([1., 0., 0.5])];
    let cloud = [
        Point3([1., 3., 3.]),
        Point3([0., 2., 3.]),
        Point3([1., 2., 4.]),
        Point3([0., 3., 4.]),
    ];
    let layers = [
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 1,
            points: &scan_a,
            style: style([1., 0., 0., 1.]),
            status: "Receiving scan A",
            error: None,
        },
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 2,
            points: &scan_b,
            style: style([0., 1., 0., 1.]),
            status: "Receiving scan B",
            error: None,
        },
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 3,
            points: &cloud,
            style: style([0., 0., 1., 1.]),
            status: "Receiving cloud",
            error: None,
        },
    ];
    let camera = CameraPose::orbit(0.5, 0.8, 12., [0.; 3]);
    let frame = CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: Some(RobotGeometryFrame {
            prepared: &prepared,
            poses: &poses,
        }),
        layers: &layers,
    };
    frame.validate().unwrap();
    let vertices = combined_vertices(&frame, camera, [800, 600]).unwrap();
    let color_vertices = |color| {
        vertices
            .iter()
            .filter(|v| v.color == color)
            .collect::<Vec<_>>()
    };
    assert_eq!(color_vertices([1., 0., 0., 1.]).len(), 12);
    assert_eq!(color_vertices([0., 1., 0., 1.]).len(), 12);
    let cloud_vertices = color_vertices([0., 0., 1., 1.]);
    assert_eq!(cloud_vertices.len(), 24);
    // Average the six symmetric billboard corners to recover its clip-space center.
    for (i, point) in cloud.iter().enumerate() {
        let group = &cloud_vertices[i * 6..i * 6 + 6];
        let center: [f32; 4] =
            std::array::from_fn(|j| group.iter().map(|v| v.clip[j]).sum::<f32>() / 6.);
        let expected = camera.clip(*point, 800. / 600.);
        for (a, b) in center.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-4);
        }
    }
}
#[test]
fn missing_layer_is_empty_and_peer_geometry_is_unchanged() {
    let peer = [Point3([1., 2., 3.])];
    let layers = [
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 7,
            points: &[],
            style: PointStyle::default(),
            status: "Waiting for TF",
            error: Some("Missing cloud TF"),
        },
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 8,
            points: &peer,
            style: PointStyle::default(),
            status: "Receiving",
            error: None,
        },
    ];
    let frame = CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &layers,
    };
    let camera = CameraPose::orbit(0.5, 0.8, 12., [0.; 3]);
    let both = combined_vertices(&frame, camera, [800, 600]).unwrap();
    let only = CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &layers[1..],
    };
    let one = combined_vertices(&only, camera, [800, 600]).unwrap();
    assert_eq!(both.len(), 6);
    for (a, b) in both.iter().zip(one) {
        assert_eq!(a.clip, b.clip);
        assert_eq!(a.color, b.color);
    }
    let bad = [PointLayerFrame {
        samples: None,
        appearance: None,
        id: 7,
        points: &peer,
        style: PointStyle::default(),
        status: "Missing",
        error: Some("cannot keep stale points"),
    }];
    assert!(CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &bad
    }
    .validate()
    .is_err());
}
#[test]
fn duplicate_layer_ids_transparency_nonfinite_points_and_fifth_layer_fail() {
    let point = [Point3([f64::NAN, 0., 0.])];
    let invalid = [PointLayerFrame {
        samples: None,
        appearance: None,
        id: 1,
        points: &point,
        style: PointStyle::default(),
        status: "",
        error: None,
    }];
    assert!(CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &invalid
    }
    .validate()
    .is_err());
    let layers = (0..5)
        .map(|i| PointLayerFrame {
            samples: None,
            appearance: None,
            id: i + 1,
            points: &[],
            style: PointStyle::default(),
            status: "Waiting",
            error: None,
        })
        .collect::<Vec<_>>();
    assert!(CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &layers
    }
    .validate()
    .is_err());
    let duplicates = [
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 1,
            points: &[],
            style: PointStyle::default(),
            status: "",
            error: None,
        },
        PointLayerFrame {
            samples: None,
            appearance: None,
            id: 1,
            points: &[],
            style: PointStyle::default(),
            status: "",
            error: None,
        },
    ];
    assert!(CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &duplicates
    }
    .validate()
    .is_err());
    let alpha = [PointLayerFrame {
        samples: None,
        appearance: None,
        id: 1,
        points: &[],
        style: style([1., 0., 0., 0.5]),
        status: "",
        error: None,
    }];
    assert!(CombinedSceneFrame {
        tf: None,
        map: None,
        pose: None,
        path: None,
        fixed_frame: "map",
        robot: None,
        layers: &alpha
    }
    .validate()
    .is_err());
}
