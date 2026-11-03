use super::*;
use crate::spatial::RigidTransform;
fn instance() -> Instance {
    Instance {
        pose: RigidTransform::identity(),
        scale: [1.; 3],
        rgba: [1., 0., 0., 1.],
    }
}
fn apply(p: &ClipInstance, local: [f32; 3]) -> [f32; 4] {
    std::array::from_fn(|i| {
        p.columns[3][i] + (0..3).map(|j| p.columns[j][i] * local[j]).sum::<f32>()
    })
}
#[test]
fn calibrated_marker_homogeneous_columns_match_scene_projection_and_reject_stale_intrinsics() {
    use crate::spatial::calibrated_camera::RectifiedProjection;
    let make = |cx, zoom| {
        CameraPose::calibrated(
            RigidTransform::new([999_900.123, 0., 0.], [0., 0., 0., 1.]).unwrap(),
            RectifiedProjection::new(
                [640, 480],
                [500., 0., cx, -50., 0., 450., 200., 22.5, 0., 0., 1., 0.],
                0.05,
                1000.,
            )
            .unwrap(),
            zoom,
        )
        .unwrap()
    };
    let camera = make(290., 1.);
    let i = Instance {
        pose: RigidTransform::new([999_900.323, 0.3, 4.], [0., 0., 0., 1.]).unwrap(),
        scale: [0.01, 0.02, 0.03],
        ..instance()
    };
    let packed = project(Kind::Sphere, &i, camera, [1280, 480]).unwrap();
    for local in [[0.; 3], [0.5, -0.5, 0.5], [-0.2, 0.3, 0.4]] {
        let world = i.pose.apply(Point3(std::array::from_fn(|j| {
            local[j] as f64 * i.scale[j]
        })));
        let actual = apply(&packed, local);
        let reference = camera.clip(world, 1280. / 480.);
        for j in 0..4 {
            assert!(
                (actual[j] - reference[j]).abs() < 2e-6,
                "{actual:?} vs {reference:?}"
            );
        }
    }
    let mut frame = Frame::default();
    frame.push(Kind::Sphere, &i, camera, [1280, 480]).unwrap();
    assert!(frame.validate_projection(camera, [1280, 480]).is_ok());
    assert!(frame
        .validate_projection(make(291., 1.), [1280, 480])
        .is_err());
    assert!(frame
        .validate_projection(make(290., 2.), [1280, 480])
        .is_err());
    assert!(frame.validate_projection(camera, [640, 480]).is_err());
    // w=0 directions must not acquire P translation or depth offset.
    let projection = projection::Projection::new(camera, [1280, 480]).unwrap();
    let a = projection.point(Point3([999_900.323, 0.3, 4.])).unwrap();
    let b = projection.point(Point3([999_900.423, 0.5, 4.5])).unwrap();
    let delta = projection.delta([0.1, 0.2, 0.5], 0.).unwrap();
    for j in 0..4 {
        assert!((b[j] - a[j] - delta[j]).abs() < 2e-6);
    }
}
#[test]
fn packed_layout_meshes_and_analytic_projection_match_world_points() {
    assert_eq!(std::mem::size_of::<ClipInstance>(), INSTANCE_BYTES);
    assert_eq!(MAX_NON_TEXT_INSTANCES * INSTANCE_BYTES, 16908288);
    assert_eq!(MAX_INSTANCE_BYTES, 40085504);
    let (vertices, ranges) = meshes();
    assert_eq!(vertices.len(), 333);
    assert_eq!(ranges[0], ranges[1]);
    assert!(vertices
        .iter()
        .all(|v| v.position.iter().all(|v| v.is_finite())));
    let i = Instance {
        pose: RigidTransform::new(
            [1., 2., 3.],
            [
                0.,
                0.,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
        )
        .unwrap(),
        scale: [2., 3., 4.],
        ..instance()
    };
    for camera in [
        CameraPose::orbit(0.5, 0.7, 10., [1., 2., 3.]),
        CameraPose::top_down(10., [1., 2., 3.]),
    ] {
        let p = project(Kind::Cube, &i, camera, [300, 200]).unwrap();
        for local in [[0.; 3], [0.5, -0.5, 0.5], [-0.2, 0.3, 0.4]] {
            let world = i.pose.apply(Point3(std::array::from_fn(|j| {
                local[j] as f64 * i.scale[j]
            })));
            let reference = camera.clip(world, 1.5);
            let actual = apply(&p, local);
            for j in 0..4 {
                assert!(
                    (reference[j] - actual[j]).abs() < 2e-5,
                    "{reference:?} vs {actual:?}"
                );
            }
        }
    }
}
#[test]
fn projection_preserves_small_dimensions_near_large_coordinates() {
    let i = Instance {
        pose: RigidTransform::new([999999.8, 0., 0.], [0., 0., 0., 1.]).unwrap(),
        scale: [0.001; 3],
        ..instance()
    };
    let camera = CameraPose::top_down(1., [999999.8, 0., 0.]);
    let p = project(Kind::Cube, &i, camera, [100, 100]).unwrap();
    let a = apply(&p, [-0.5, 0., 0.]);
    let b = apply(&p, [0.5, 0., 0.]);
    assert!((b[0] - a[0] - 0.002).abs() < 1e-7);
}
#[test]
fn invalid_inputs_are_rejected_before_frame_mutation_and_instance_limit_is_exact() {
    let mut f = Frame::default();
    let camera = CameraPose::top_down(10., [0.; 3]);
    let mut i = instance();
    for scale in [[0., 1., 1.], [f64::NAN, 1., 1.], [1e-300; 3]] {
        i.scale = scale;
        assert!(f.push(Kind::Sphere, &i, camera, [64, 64]).is_err());
        assert!(f.is_empty());
    }
    let mut invalid_camera = camera;
    invalid_camera.forward = [0.; 3];
    assert!(f
        .push(Kind::Sphere, &instance(), invalid_camera, [64, 64])
        .is_err());
    assert!(f.is_empty());
    i = instance();
    i.rgba[3] = 2.;
    assert!(f.push(Kind::Cube, &i, camera, [64, 64]).is_err());
    assert!(f.is_empty());
    let packed = project(Kind::Cube, &instance(), camera, [64, 64]).unwrap();
    f.instances = vec![packed; MAX_NON_TEXT_INSTANCES];
    f.kinds = vec![Kind::Cube; MAX_NON_TEXT_INSTANCES];
    f.key_slots.resize(MAX_NON_TEXT_INSTANCES, UNKEYED);
    assert!(f.push(Kind::Cube, &instance(), camera, [64, 64]).is_err());
    assert_eq!(f.len(), MAX_NON_TEXT_INSTANCES);
}

#[test]
fn sealed_frames_reject_mixed_or_stale_projection_and_preserve_viewport_normalization() {
    let camera = CameraPose::top_down(10., [0.; 3]);
    let other = CameraPose::top_down(11., [0.; 3]);
    let mut f = Frame::default();
    f.push(Kind::Sphere, &instance(), camera, [4096, 2048])
        .unwrap();
    assert!(f
        .push(Kind::Cube, &instance(), other, [4096, 2048])
        .is_err());
    assert_eq!(f.len(), 1);
    let ready = f.seal().unwrap();
    ready.validate_projection(camera, [2048, 1024]).unwrap();
    assert!(ready.validate_projection(other, [2048, 1024]).is_err());
    assert!(ready.validate_projection(camera, [128, 128]).is_err());
    let mut invalid = (*ready).clone();
    invalid.instances[0].rgba[3] = 2.;
    assert!(invalid.seal().is_err());
}

#[test]
fn sealing_reorders_payload_pairs_and_batches_sphere_boxes_with_cubes() {
    let camera = CameraPose::top_down(10., [0.; 3]);
    let mut f = Frame::default();
    for (kind, z, alpha, color) in [
        (Kind::Sphere, 1., 0.5, [1., 0., 0.]),
        (Kind::Cube, 0., 1., [0., 1., 0.]),
        (Kind::Sphere, -1., 0.5, [0., 0., 1.]),
        (Kind::Cylinder, 0., 1., [1., 1., 0.]),
        (Kind::Cube, 2., 0.5, [1., 0., 1.]),
        (Kind::Sphere, 0., 0., [0., 0., 0.]),
    ] {
        let i = Instance {
            pose: RigidTransform::new([0., 0., z], [0., 0., 0., 1.]).unwrap(),
            rgba: [color[0], color[1], color[2], alpha],
            ..instance()
        };
        f.push(kind, &i, camera, [128, 128]).unwrap();
    }
    let f = f.seal().unwrap();
    assert_eq!(
        f.kinds,
        vec![
            Kind::Cube,
            Kind::Cylinder,
            Kind::Sphere,
            Kind::Sphere,
            Kind::Cube,
            Kind::Sphere
        ]
    );
    assert_eq!(&f.instances[2].rgba[..3], &[0., 0., 1.]);
    assert_eq!(&f.instances[3].rgba[..3], &[1., 0., 0.]);
    assert_eq!(
        f.opaque_runs(),
        &[(Kind::Cube, 0..1), (Kind::Cylinder, 1..2)]
    );
    assert_eq!(
        f.transparent_instances()
            .iter()
            .map(|i| (i.mesh, i.index))
            .collect::<Vec<_>>(),
        vec![(Kind::Cube, 2), (Kind::Cube, 3), (Kind::Cube, 4)]
    );
    assert!(f
        .transparent_instances()
        .windows(2)
        .all(|w| w[0].depth >= w[1].depth));
}

#[test]
fn timed_instances_require_a_clock_preserve_integer_bits_and_cache_next_expiry() {
    let camera = CameraPose::top_down(10., [0.; 3]);
    let mut f = Frame::default();
    assert!(f
        .push_timed(Kind::Sphere, &instance(), camera, [128, 128], Some(1))
        .is_err());
    assert!(f.is_empty());
    let clock = MarkerClock::new();
    let far = (1u64 << 48) + u32::MAX as u64;
    let mut f = Frame::with_clock(clock);
    f.push_timed(Kind::Sphere, &instance(), camera, [128, 128], Some(far))
        .unwrap();
    f.push_timed(Kind::Cube, &instance(), camera, [128, 128], Some(0))
        .unwrap();
    f.push(Kind::Cylinder, &instance(), camera, [128, 128])
        .unwrap();
    let f = f.seal().unwrap();
    assert_eq!(f.0.deadlines, vec![0, far]);
    assert!(f.next_expiry_after(0).unwrap() > std::time::Duration::from_secs(1));
    assert!(f
        .instances
        .iter()
        .any(|p| p.shape == [1, u32::MAX, 1 << 16, 1]));
}

#[test]
fn repaint_after_render_catches_a_passed_deadline_once_without_spinning() {
    let mut f = Frame::with_clock(MarkerClock::new());
    let camera = CameraPose::top_down(10., [0.; 3]);
    f.push_timed(Kind::Sphere, &instance(), camera, [128, 128], Some(1))
        .unwrap();
    f.push(Kind::Cube, &instance(), camera, [128, 128]).unwrap();
    let f = f.seal().unwrap();
    assert_eq!(f.next_expiry_after_at(0, 2), Some(Duration::ZERO));
    assert_eq!(f.next_expiry_after_at(1, 2), None);
    assert!(!f.all_expired());
}

#[test]
fn world_point_dimensions_do_not_scale_position_and_lines_keep_endpoint_alpha() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::top_down(10., [0.; 3]);
    let mut f = Frame::default();
    f.push_point(
        ColoredPoint {
            position: Point3([2., 3., 4.]),
            rgba: [1., 0., 0., 1.],
        },
        [2., 4.],
        camera,
        [100, 100],
        None,
    )
    .unwrap();
    assert!((f.instances[0].columns[3][0] - 0.4).abs() < 1e-6);
    assert!((f.instances[0].columns[3][1] - 0.6).abs() < 1e-6);
    assert!((f.instances[0].columns[0][0] - 0.4).abs() < 1e-6);
    assert!((f.instances[0].columns[1][1] - 0.8).abs() < 1e-6);
    let a = ColoredPoint {
        position: Point3([-1., 0., 0.]),
        rgba: [1., 0., 0., 0.],
    };
    let b = ColoredPoint {
        position: Point3([1., 0., 0.]),
        rgba: [0., 0., 1., 1.],
    };
    assert!(f.push_line(a, b, 0.25, camera, [100, 100], None).unwrap());
    assert!(f.push_line(b, a, 0.25, camera, [100, 100], None).unwrap());
    let f = f.seal().unwrap();
    assert_eq!(f.opaque_runs().len(), 1);
    assert_eq!(f.transparent_instances().len(), 2);
    assert_eq!(f.transparent_instances()[0].mesh, Kind::Point);
}
#[test]
fn triangle_corners_use_precise_projection_and_all_alpha_values() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::top_down(4., [0.; 3]);
    let points = [
        ColoredPoint {
            position: Point3([-1., -1., 0.]),
            rgba: [1., 0., 0., 0.],
        },
        ColoredPoint {
            position: Point3([1., -1., 0.]),
            rgba: [0., 1., 0., 1.],
        },
        ColoredPoint {
            position: Point3([0., 1., 0.]),
            rgba: [0., 0., 1., 1.],
        },
    ];
    let mut f = Frame::default();
    f.push_triangle(points, camera, [128, 128], None).unwrap();
    assert_eq!(f.instances[0].shape[0], 3);
    for (i, p) in points.iter().enumerate() {
        let expected = camera.clip(p.position, 1.);
        for (j, value) in expected.iter().enumerate() {
            assert!((f.instances[0].columns[i][j] - value).abs() < 1e-6);
        }
    }
    let f = f.seal().unwrap();
    assert!(f.opaque_runs().is_empty());
    assert_eq!(f.transparent_instances().len(), 1);
    assert_eq!(f.instances[0].eye, [0., 1., 0., 1.]);
    assert_eq!(f.instances[0].ray, [0., 0., 1., 1.]);
}
#[test]
fn zero_length_parallel_and_invalid_flat_geometry_do_not_commit_partial_instances() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::top_down(4., [0.; 3]);
    let p = ColoredPoint {
        position: Point3([0.; 3]),
        rgba: [1.; 4],
    };
    let mut f = Frame::default();
    assert!(!f.push_line(p, p, 1., camera, [128, 128], None).unwrap());
    let b = ColoredPoint {
        position: Point3([0., 0., 1.]),
        ..p
    };
    assert!(!f.push_line(p, b, 1., camera, [128, 128], None).unwrap());
    assert!(f.push_point(p, [0., 1.], camera, [128, 128], None).is_err());
    let bad = ColoredPoint {
        rgba: [f32::NAN; 4],
        ..p
    };
    assert!(f
        .push_triangle([p, p, bad], camera, [128, 128], None)
        .is_err());
    assert!(f.is_empty());
}
#[test]
fn near_clip_crossing_ribbon_and_shared_timed_packets_stay_finite() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::orbit(0., 0., 1., [0.; 3]);
    let a = ColoredPoint {
        position: Point3([0.99, -0.2, 0.]),
        rgba: [1.; 4],
    };
    let b = ColoredPoint {
        position: Point3([0.5, 0.2, 0.]),
        rgba: [1.; 4],
    };
    let mut f = Frame::with_clock(MarkerClock::new());
    assert!(f
        .push_line(a, b, 0.1, camera, [128, 128], Some(1u64 << 40))
        .unwrap());
    assert_eq!(f.instances[0].shape, [2, 0, 256, 1]);
    assert!(f.instances[0]
        .columns
        .iter()
        .flatten()
        .all(|v| v.is_finite()));
    f.seal().unwrap();
}

#[test]
fn strip_join_shares_exact_corners_and_bounded_miters() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::top_down(8., [0.; 3]);
    let p = |x, y| ColoredPoint {
        position: Point3([x, y, 0.]),
        rgba: [1., 0., 0., 0.5],
    };
    for points in [
        vec![p(-2., 0.), p(0., 0.), p(0., 2.)],
        vec![p(-2., 0.), p(0., 0.), p(-2., 0.001)],
        vec![p(-2., 0.), p(0., 0.), p(-2., 0.)],
    ] {
        let mut f = Frame::default();
        assert_eq!(
            f.push_line_strip(&points, 0.5, camera, [128, 128], None, || false)
                .unwrap(),
            2
        );
        assert_eq!(f.instances[0].columns[1], f.instances[1].columns[0]);
        assert_eq!(f.instances[0].columns[2], f.instances[1].columns[3]);
        for corner in [f.instances[0].columns[1], f.instances[0].columns[2]] {
            assert!((corner[0] * corner[0] + corner[1] * corner[1]).sqrt() <= 0.250001);
        }
        let f = f.seal().unwrap();
        assert_eq!(f.transparent_instances().len(), 2);
        assert!(f
            .transparent_instances()
            .iter()
            .all(|v| v.mesh == Kind::Point));
    }
}
#[test]
fn strip_duplicate_view_parallel_and_transactional_cancellation() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::top_down(8., [0.; 3]);
    let p = |x, y, z| ColoredPoint {
        position: Point3([x, y, z]),
        rgba: [0., 1., 0., 1.],
    };
    let points = [
        p(-2., 0., 0.),
        p(-2., 0., 0.),
        p(0., 0., 0.),
        p(0., 0., 1.),
        p(2., 0., 1.),
    ];
    let mut f = Frame::default();
    assert_eq!(
        f.push_line_strip(&points, 0.5, camera, [128, 128], None, || false)
            .unwrap(),
        2
    );
    let old = f.instances.clone();
    let key = f.projection.clone();
    let points = [p(-2., 0., 0.), p(0., 0., 0.), p(0., 2., 0.)];
    let mut calls = 0;
    assert!(f
        .push_line_strip(&points, 0.5, camera, [128, 128], None, || {
            calls += 1;
            calls == 8
        })
        .is_err());
    assert_eq!(f.len(), old.len());
    assert_eq!(
        bytemuck::cast_slice::<ClipInstance, u8>(&f.instances),
        bytemuck::cast_slice::<ClipInstance, u8>(&old)
    );
    assert_eq!(f.projection, key);
    assert!(f
        .push_line_strip(&points, 0.5, camera, [128, 128], Some(1), || false)
        .is_err());
    assert_eq!(f.len(), old.len());
    let mut bad = points;
    bad[2].rgba[0] = f32::NAN;
    assert!(f
        .push_line_strip(&bad, 0.5, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), old.len());
}
#[test]
fn strip_perspective_timing_and_maximum_admission() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let camera = CameraPose::orbit(0.5, 0.4, 10., [0.; 3]);
    let points = (0..studio_protocol::marker::MAX_RETAINED_POINTS)
        .map(|i| ColoredPoint {
            position: Point3([i as f64 * 0.00001, (i % 2) as f64 * 0.001, 0.]),
            rgba: [1., 0., 0., 0.5],
        })
        .collect::<Vec<_>>();
    let mut f = Frame::with_clock(MarkerClock::new());
    assert_eq!(
        f.push_line_strip(&points, 0.01, camera, [128, 128], Some(1 << 40), || false)
            .unwrap(),
        points.len() - 1
    );
    assert!(f.instances.iter().all(|p| p.shape == [4, 0, 256, 1]));
    f.seal().unwrap();
}

fn arrow_body(pose: RigidTransform) -> crate::spatial::marker_geometry::Body {
    crate::spatial::marker_geometry::Body::Arrow {
        pose,
        shaft: 2.,
        head: 1.,
        shaft_diameters: [0.4, 0.6],
        head_diameters: [0.8, 1.2],
        rgba: [1., 0., 0., 0.5],
    }
}
#[test]
fn prepared_arrow_projects_ros_x_endpoints_and_elliptical_diameters() {
    let pose = RigidTransform::new(
        [1., 2., 3.],
        [
            0.,
            0.,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    )
    .unwrap();
    let camera = CameraPose::orbit(0.3, 0.8, 12., [0.; 3]);
    let mut f = Frame::with_clock(MarkerClock::new());
    assert_eq!(
        f.push_geometry(&arrow_body(pose), camera, [128, 128], Some(1 << 40), || {
            false
        })
        .unwrap(),
        2
    );
    assert_eq!(f.kinds, vec![Kind::Cylinder, Kind::Cone]);
    for (index, local, world) in [
        (0, [0., 0., -0.5], [0., 0., 0.]),
        (0, [0., 0., 0.5], [2., 0., 0.]),
        (1, [0., 0., 0.5], [3., 0., 0.]),
        (0, [0.5, 0., 0.], [1., 0.2, 0.]),
        (0, [0., 0.5, 0.], [1., 0., 0.3]),
        (1, [0.5, 0., -0.5], [2., 0.4, 0.]),
        (1, [0., 0.5, -0.5], [2., 0., 0.6]),
    ] {
        let expected = camera.clip(pose.apply(Point3(world)), 1.);
        let actual = apply(&f.instances[index], local);
        for i in 0..4 {
            assert!(
                (actual[i] - expected[i]).abs() < 1e-5,
                "{actual:?} != {expected:?}"
            );
        }
    }
    assert!(f.instances.iter().all(|p| p.shape[1..] == [0, 256, 1]));
    let f = f.seal().unwrap();
    assert_eq!(f.transparent_instances().len(), 2);
}
#[test]
fn prepared_arrow_zero_pieces_invalid_values_and_cancellation_are_atomic() {
    use crate::spatial::marker_geometry::Body;
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    for (shaft, head, expected) in [(0., 1., 1), (2., 0., 1), (0., 0., 0)] {
        let Body::Arrow {
            pose,
            shaft_diameters,
            head_diameters,
            rgba,
            ..
        } = arrow_body(RigidTransform::identity())
        else {
            unreachable!()
        };
        let body = Body::Arrow {
            pose,
            shaft,
            head,
            shaft_diameters,
            head_diameters,
            rgba,
        };
        assert_eq!(
            f.push_geometry(&body, camera, [128, 128], None, || false)
                .unwrap(),
            expected
        );
    }
    let original = f.instances.clone();
    let key = f.projection.clone();
    let mut calls = 0;
    assert!(f
        .push_geometry(
            &arrow_body(RigidTransform::identity()),
            camera,
            [128, 128],
            None,
            || {
                calls += 1;
                calls == 4
            }
        )
        .is_err());
    assert_eq!(
        bytemuck::cast_slice::<ClipInstance, u8>(&f.instances),
        bytemuck::cast_slice::<ClipInstance, u8>(&original)
    );
    assert_eq!(f.projection, key);
    let Body::Arrow {
        pose,
        shaft,
        head,
        shaft_diameters,
        head_diameters,
        ..
    } = arrow_body(RigidTransform::identity())
    else {
        unreachable!()
    };
    let body = Body::Arrow {
        pose,
        shaft,
        head,
        shaft_diameters,
        head_diameters,
        rgba: [f32::NAN; 4],
    };
    assert!(f
        .push_geometry(&body, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), original.len());
    let mut empty = Frame::default();
    let mut calls = 0;
    assert!(empty
        .push_geometry(
            &arrow_body(RigidTransform::identity()),
            camera,
            [128, 128],
            None,
            || {
                calls += 1;
                calls == 3
            }
        )
        .is_err());
    assert!(empty.is_empty());
    assert!(empty.projection.is_none());
}
#[test]
fn prepared_geometry_bridge_routes_connected_strips_lists_triangles_and_preserves_peers() {
    use crate::spatial::marker_geometry::{Body, ColoredPoint};
    let camera = CameraPose::top_down(8., [0.; 3]);
    let p = |x, y| ColoredPoint {
        position: Point3([x, y, 0.]),
        rgba: [1.; 4],
    };
    let segments = vec![[p(-1., 0.), p(0., 0.)], [p(0., 0.), p(0., 1.)]];
    let bodies = [
        Body::Instances {
            primitive: Primitive::Sphere,
            instances: vec![instance()].into(),
        },
        Body::Points {
            points: vec![p(0., 0.)].into(),
            dimensions: [0.5; 2],
        },
        Body::Lines {
            strip: true,
            segments: segments.clone().into(),
            width: 0.5,
        },
        Body::Lines {
            strip: false,
            segments: segments.into(),
            width: 0.5,
        },
        Body::Triangles {
            triangles: vec![[p(0., 0.), p(1., 0.), p(0., 1.)]].into(),
        },
    ];
    let mut f = Frame::with_clock(MarkerClock::new());
    for (body, n) in bodies.iter().zip([1, 1, 2, 2, 1]) {
        assert_eq!(
            f.push_geometry(body, camera, [128, 128], Some(10), || false)
                .unwrap(),
            n
        );
    }
    assert_eq!(
        f.kinds,
        vec![
            Kind::Sphere,
            Kind::Point,
            Kind::Ribbon,
            Kind::Ribbon,
            Kind::Line,
            Kind::Line,
            Kind::Triangle
        ]
    );
    assert!(f.instances.iter().all(|p| p.shape[1..] == [10, 0, 1]));
    let n = f.len();
    let bad = Body::Points {
        points: vec![
            p(0., 0.),
            ColoredPoint {
                rgba: [f32::NAN; 4],
                ..p(1., 0.)
            },
        ]
        .into(),
        dimensions: [1.; 2],
    };
    assert!(f
        .push_geometry(&bad, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), n);
    let bad = Body::Lines {
        strip: true,
        segments: vec![[p(0., 0.), p(1., 0.)], [p(2., 0.), p(3., 0.)]].into(),
        width: 1.,
    };
    assert!(f
        .push_geometry(&bad, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), n);
    let text = Body::Text {
        position: Point3([0.; 3]),
        text: "pending".into(),
        height: 1.,
        space_width: None,
        rgba: [1.; 4],
    };
    assert!(f
        .push_geometry(&text, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), n);
    f.seal().unwrap();
}
#[test]
fn prepared_projection_capacity_and_camera_failures_leave_existing_frames_unchanged() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    f.push(Kind::Cube, &instance(), camera, [128, 128]).unwrap();
    assert!(f
        .push_geometry(
            &arrow_body(RigidTransform::identity()),
            CameraPose::top_down(10., [0.; 3]),
            [128, 128],
            None,
            || false
        )
        .is_err());
    assert_eq!(f.len(), 1);
    let sample = f.instances[0];
    f.instances.resize(MAX_NON_TEXT_INSTANCES - 1, sample);
    f.kinds.resize(MAX_NON_TEXT_INSTANCES - 1, Kind::Cube);
    f.key_slots.resize(MAX_NON_TEXT_INSTANCES - 1, UNKEYED);
    assert!(f
        .push_geometry(
            &arrow_body(RigidTransform::identity()),
            camera,
            [128, 128],
            None,
            || false
        )
        .is_err());
    assert_eq!(f.len(), MAX_NON_TEXT_INSTANCES - 1);
}

fn text_body(s: &str) -> crate::spatial::marker_geometry::Body {
    crate::spatial::marker_geometry::Body::Text {
        position: Point3([0.; 3]),
        text: s.into(),
        height: 0.5,
        space_width: None,
        rgba: [1., 0., 0., 1.],
    }
}
#[test]
fn glyph_projection_preserves_shared_uv_clock_and_transparent_coverage_class() {
    let body = text_body("A中🤖");
    let layouts = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::with_clock(MarkerClock::new());
    assert_eq!(
        f.push_text(&layouts[0], camera, [128, 128], Some(1 << 40), || false)
            .unwrap(),
        3
    );
    assert!(f.instances.iter().all(|p| p.shape == [5, 0, 256, 1]));
    for (p, g) in f.instances.iter().zip(layouts[0].glyphs.iter()) {
        assert_eq!(p.eye, g.uv);
    }
    let ready = f.seal().unwrap();
    assert!(ready.opaque_runs().is_empty());
    assert_eq!(ready.transparent_instances().len(), 3);
    assert_eq!(
        ready.atlas_dimensions(),
        Some(layouts[0].atlas().dimensions())
    );
}
#[test]
fn glyph_projection_cancel_invalid_bounds_and_different_atlas_preserve_peers() {
    let body = text_body("AB");
    let layouts = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    f.push(Kind::Sphere, &instance(), camera, [128, 128])
        .unwrap();
    let mut calls = 0;
    assert!(f
        .push_text(&layouts[0], camera, [128, 128], None, || {
            calls += 1;
            calls == 2
        })
        .is_err());
    assert_eq!(f.len(), 1);
    assert_eq!(f.glyph_count, 0);
    assert!(f.atlas.is_none());
    f.push_text(&layouts[0], camera, [128, 128], None, || false)
        .unwrap();
    let n = f.len();
    let second = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    assert!(f
        .push_text(&second[0], camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), n);
    let mut bad = layouts[0].clone();
    bad.position = Point3([1_000_001., 0., 0.]);
    assert!(f
        .push_text(&bad, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.len(), n);
    f.seal().unwrap();
}
#[test]
fn text_allowance_does_not_expand_non_text_admission() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    f.push(Kind::Cube, &instance(), camera, [128, 128]).unwrap();
    f.instances.resize(MAX_NON_TEXT_INSTANCES, f.instances[0]);
    f.kinds.resize(MAX_NON_TEXT_INSTANCES, Kind::Cube);
    f.key_slots.resize(MAX_NON_TEXT_INSTANCES, UNKEYED);
    assert!(f.push(Kind::Cube, &instance(), camera, [128, 128]).is_err());
    let body = text_body("A");
    let layouts = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    assert_eq!(
        f.push_text(&layouts[0], camera, [128, 128], None, || false)
            .unwrap(),
        1
    );
    assert!(f.push(Kind::Cube, &instance(), camera, [128, 128]).is_err());
    f.seal().unwrap();
}

#[test]
fn whitespace_has_no_draw_or_atlas_and_cached_bounds_enclose_real_glyphs() {
    let body = text_body(" \n  ");
    let layouts = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    assert_eq!(layouts[0].glyph_count(), 0);
    assert!(layouts[0].local_bounds().is_none());
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    assert_eq!(
        f.push_text(&layouts[0], camera, [128, 128], None, || false)
            .unwrap(),
        0
    );
    assert!(f.atlas.is_none());
    assert!(f.is_empty());
    let body = text_body("A\n🤖");
    let layouts = crate::spatial::marker_text::prepare(&[&body], || false).unwrap();
    let bounds = layouts[0].local_bounds().unwrap();
    for g in layouts[0].glyphs.iter() {
        for (i, _) in g.center.iter().enumerate() {
            assert!(bounds[0][i] <= g.center[i] - g.dimensions[i] * 0.5);
            assert!(bounds[1][i] >= g.center[i] + g.dimensions[i] * 0.5);
        }
    }
}

fn prepared_mesh(
    image: Option<Arc<crate::robot::RobotTexture>>,
    count: usize,
) -> crate::spatial::marker_mesh::Prepared {
    use crate::{
        robot::{MeshAsset, MeshAssetPart, Triangle},
        spatial::marker_geometry::Body,
    };
    let a = MeshAsset {
        path: "fixture".into(),
        parts: vec![MeshAssetPart {
            triangles: vec![Triangle([[-1., -1., 0.], [1., -1., 0.], [0., 1., 0.]]); count].into(),
            uvs: image
                .as_ref()
                .map(|_| vec![[[0., 0.], [1., 0.], [0.5, 1.]]; count].into()),
            texture: image,
            rgba: Some([1.; 4]),
        }]
        .into(),
        triangle_count: count,
        embedded_materials: true,
    };
    let b = Body::Mesh {
        pose: RigidTransform::identity(),
        scale: [1.; 3],
        resource: "fixture".into(),
        embedded: true,
        rgba: [0.; 4],
    };
    crate::spatial::marker_mesh::prepare(&[(&b, &a)], &std::sync::atomic::AtomicBool::new(false))
        .unwrap()
        .remove(0)
        .unwrap()
}
#[test]
fn textured_mesh_slots_deadlines_and_transparency_survive_sealing() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let image = Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 128],
        has_transparency: true,
    });
    let mesh = prepared_mesh(Some(image), 2);
    let mut f = Frame::with_clock(MarkerClock::new());
    f.push_mesh(&mesh, camera, [128, 128], Some(1 << 40), || false)
        .unwrap();
    assert_eq!(f.textures.len(), 1);
    assert_eq!(f.mesh_count, 2);
    assert!(f.kinds.iter().all(|k| *k == Kind::TexturedTriangle(0)));
    assert!(f.instances.iter().all(|p| p.shape == [6, 0, 256, 1]));
    let second = prepared_mesh(
        Some(Arc::new(crate::robot::RobotTexture {
            width: 1,
            height: 1,
            rgba: vec![0, 255, 0, 128],
            has_transparency: true,
        })),
        1,
    );
    f.push_mesh(&second, camera, [128, 128], Some(1 << 40), || false)
        .unwrap();
    let f = f.seal().unwrap();
    assert_eq!(f.transparent_instances().len(), 3);
    assert!(f
        .transparent_instances()
        .iter()
        .any(|t| t.mesh == Kind::TexturedTriangle(1)));
}
#[test]
fn mesh_projection_cancellation_and_capacity_preserve_existing_peers_and_images() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mesh = prepared_mesh(
        Some(Arc::new(crate::robot::RobotTexture {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            has_transparency: false,
        })),
        2,
    );
    let mut f = Frame::default();
    f.push(Kind::Cube, &instance(), camera, [128, 128]).unwrap();
    let mut calls = 0;
    assert!(f
        .push_mesh(&mesh, camera, [128, 128], None, || {
            calls += 1;
            calls == 3
        })
        .is_err());
    assert_eq!(f.len(), 1);
    assert_eq!(f.mesh_count, 0);
    assert!(f.textures.is_empty());
    let maximum = prepared_mesh(None, crate::spatial::marker_mesh::MAX_TRIANGLES);
    f.push_mesh(&maximum, camera, [128, 128], None, || false)
        .unwrap();
    assert!(f
        .push_mesh(&mesh, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.mesh_count, crate::spatial::marker_mesh::MAX_TRIANGLES);
    assert!(f.textures.is_empty());
    f.seal().unwrap();
}

#[test]
fn mesh_texture_limit_rejects_only_new_key_and_preserves_existing_bindings() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::default();
    for i in 0..crate::spatial::marker_mesh::MAX_TEXTURES {
        let image = Arc::new(crate::robot::RobotTexture {
            width: 1,
            height: 1,
            rgba: vec![i as u8, 0, 0, 255],
            has_transparency: false,
        });
        let mesh = prepared_mesh(Some(image), 1);
        f.push_mesh(&mesh, camera, [128, 128], None, || false)
            .unwrap();
    }
    let image = Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255; 4],
        has_transparency: false,
    });
    let mesh = prepared_mesh(Some(image), 1);
    assert!(f
        .push_mesh(&mesh, camera, [128, 128], None, || false)
        .is_err());
    assert_eq!(f.textures.len(), crate::spatial::marker_mesh::MAX_TEXTURES);
    assert_eq!(f.mesh_count, crate::spatial::marker_mesh::MAX_TEXTURES);
    f.seal().unwrap();
}

fn identity(owner: u64, id: i32) -> KeyIdentity {
    KeyIdentity {
        owner,
        source_epoch: 1,
        scene_epoch: 1,
        asset_epoch: None,
        key: studio_protocol::marker::Key {
            namespace: "objects".into(),
            id,
        },
        version: 1,
    }
}
#[test]
fn owned_frames_require_scopes_and_failed_keys_roll_back_all_payloads() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::owned(MarkerClock::new());
    assert!(f.push(Kind::Cube, &instance(), camera, [128, 128]).is_err());
    f.append_key(identity(1, 1), |f| {
        f.push(Kind::Cube, &instance(), camera, [128, 128])
    })
    .unwrap();
    let image = Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255; 4],
        has_transparency: false,
    });
    let mesh = prepared_mesh(Some(image), 2);
    let mut calls = 0;
    assert!(f
        .append_key(identity(1, 2), |f| f.push_mesh(
            &mesh,
            camera,
            [128, 128],
            None,
            || {
                calls += 1;
                calls == 3
            }
        ))
        .is_err());
    assert_eq!(f.len(), 1);
    assert_eq!(f.keys.len(), 1);
    assert_eq!(f.key_slots, vec![0]);
    assert_eq!(f.mesh_count, 0);
    assert!(f.textures.is_empty());
    let b = text_body("AB");
    let text = crate::spatial::marker_text::prepare(&[&b], || false).unwrap();
    assert!(f
        .append_key(identity(1, 2), |f| {
            f.push_text(&text[0], camera, [128, 128], None, || false)?;
            Err::<(), _>("cancel after text".into())
        })
        .is_err());
    assert!(f.atlas.is_none());
    assert_eq!(f.glyph_count, 0);
    assert_eq!(f.len(), 1);
    f.seal().unwrap();
}
#[test]
fn scopes_reject_duplicates_mixed_epochs_owners_scene_and_nested_builds() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::owned(MarkerClock::new());
    let add = |f: &mut Frame| f.push(Kind::Cube, &instance(), camera, [128, 128]);
    assert!(f.append_key(identity(0, 0), add).is_err());
    let mut invalid = identity(1, 1);
    invalid.source_epoch = 0;
    assert!(f.append_key(invalid, add).is_err());
    f.append_key(identity(1, 1), add).unwrap();
    assert!(f.append_key(identity(1, 1), add).is_err());
    let mut bad = identity(1, 2);
    bad.source_epoch = 2;
    assert!(f.append_key(bad, add).is_err());
    let mut bad = identity(2, 1);
    bad.scene_epoch = 2;
    assert!(f.append_key(bad, add).is_err());
    assert!(f
        .append_key(identity(1, 2), |f| f.append_key(identity(1, 3), add))
        .is_err());
    let mut asset_key = identity(1, 2);
    asset_key.asset_epoch = Some(1);
    f.append_key(asset_key, add).unwrap();
    let mut changed = identity(1, 3);
    changed.asset_epoch = Some(2);
    assert!(f.append_key(changed, add).is_err());
    f.append_key(identity(2, 1), add).unwrap();
    assert!(f.append_key(identity(3, 1), add).is_err());
    assert_eq!(f.len(), 3);
    f.seal().unwrap();
}
#[test]
fn frame_bound_permits_filter_and_coalesce_without_exposing_hidden_key_gaps() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::owned(MarkerClock::new());
    for id in 1..=3 {
        f.append_key(identity(1, id), |f| {
            f.push(Kind::Cube, &instance(), camera, [128, 128])
        })
        .unwrap();
    }
    let f = f.seal().unwrap();
    assert!(!f.has_visible(None));
    let all = f.permit(|_| true);
    assert_eq!(
        f.opaque_visible(Some(&all)).collect::<Vec<_>>(),
        vec![(Kind::Cube, 0..3)]
    );
    let selective = f.permit(|key| key.key.id != 2);
    let ranges = f.opaque_visible(Some(&selective)).collect::<Vec<_>>();
    assert_eq!(ranges, vec![(Kind::Cube, 0..1), (Kind::Cube, 2..3)]);
    let none = f.permit(|_| false);
    assert!(!f.has_visible(Some(&none)));
    let other = Frame::default().seal().unwrap();
    assert!(all.validate_for(&other).is_err());
    assert!(!other.has_visible(Some(&all)));
    assert!(all.validate_for(&f.clone()).is_ok());
    let weak = Arc::downgrade(&f.0);
    assert_eq!(Arc::strong_count(&f.0), 1);
    drop(f);
    assert!(
        weak.upgrade().is_none(),
        "Permits must not retain large CPU frames"
    );
}
#[test]
fn transparent_key_slots_survive_sealing_and_visibility_precedes_draw_coalescing() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::owned(MarkerClock::new());
    let mut i = instance();
    i.rgba[3] = 0.5;
    for (owner, id) in [(1, 1), (1, 2), (2, 1)] {
        f.append_key(identity(owner, id), |f| {
            f.push(Kind::Cube, &i, camera, [128, 128])
        })
        .unwrap();
    }
    let f = f.seal().unwrap();
    assert_eq!(
        f.transparent_instances()
            .iter()
            .map(|t| t.key)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let permit = f.permit(|k| k.owner == 2 || k.key.id == 1);
    assert!(permit.allows(0));
    assert!(!permit.allows(1));
    assert!(permit.allows(2));
}
#[test]
fn full_two_owner_key_capacity_and_empty_keys_do_not_leak_slots() {
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = Frame::owned(MarkerClock::new());
    f.append_key(identity(1, 0), |_| Ok(())).unwrap();
    assert!(f.keys.is_empty());
    for owner in 1..=2 {
        for id in 0..studio_protocol::marker::MAX_MARKERS as i32 {
            f.append_key(identity(owner, id), |f| {
                f.push(Kind::Cube, &instance(), camera, [128, 128])
            })
            .unwrap();
        }
    }
    assert_eq!(f.keys.len(), 512);
    assert!(f.append_key(identity(2, 999), |_| Ok(())).is_err());
    let f = f.seal().unwrap();
    let p = f.permit(|k| k.owner == 1);
    assert!(p.allows(255));
    assert!(!p.allows(256));
    assert!(f.has_visible(Some(&p)));
}
