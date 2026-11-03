use super::*;
fn entry(id: i32, geometry: Geometry) -> LiveEntry {
    LiveEntry {
        marker: Arc::new(marker::Marker {
            key: Key {
                namespace: "objects".into(),
                id,
            },
            frame_id: "source".into(),
            stamp_ns: 0,
            pose: [1., 2., 3., 0., 0., 0., 1.],
            scale: [2., 4., 6.],
            color: [0.2, 0.3, 0.4, 0.5],
            lifetime_ns: 0,
            frame_locked: false,
            geometry,
        }),
        version: 1,
        transform: Transform::Resolved {
            pose: LatestPose::Ready {
                resolved_stamp_ns: 0,
                components: [
                    10.,
                    20.,
                    30.,
                    0.,
                    0.,
                    std::f64::consts::FRAC_1_SQRT_2,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
            },
        },
        expires_at_monotonic_ns: None,
    }
}
fn prepared(e: LiveEntry) -> Body {
    let p = prepare(&[e], 0, &AtomicBool::new(false)).unwrap();
    assert!(p.hidden.is_empty(), "{:?}", p.hidden);
    p.entries.into_iter().next().unwrap().body
}
fn near(a: [f64; 3], b: [f64; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() < 1e-8, "{a:?} != {b:?}");
    }
}
#[test]
fn primitive_and_list_pose_scale_and_colors_do_not_scale_point_positions() {
    let Body::Instances {
        primitive,
        instances,
    } = prepared(entry(1, Geometry::Cube))
    else {
        panic!()
    };
    assert_eq!(primitive, Primitive::Cube);
    assert_eq!(instances.len(), 1);
    near(instances[0].pose.apply(Point3([0.; 3])).0, [8., 21., 33.]);
    assert_eq!(instances[0].scale, [2., 4., 6.]);
    let Body::Instances { instances, .. } = prepared(entry(
        1,
        Geometry::CubeList {
            points: vec![[2., 0., 0.]],
            colors: vec![[1., 0., 0., 0.25]],
        },
    )) else {
        panic!()
    };
    near(instances[0].pose.apply(Point3([0.; 3])).0, [8., 23., 33.]);
    assert_eq!(instances[0].rgba, [1., 0., 0., 0.25]);
}
#[test]
fn points_and_lines_preserve_world_units_and_per_point_alpha() {
    let Body::Points { points, dimensions } = prepared(entry(
        1,
        Geometry::Points {
            points: vec![[2., 0., 0.]],
            colors: vec![[1., 0., 0., 0.25]],
        },
    )) else {
        panic!()
    };
    near(points[0].position.0, [8., 23., 33.]);
    assert_eq!(points[0].rgba[3], 0.25);
    assert_eq!(dimensions, [2., 4.]);
    let Body::Lines {
        segments,
        width,
        strip,
    } = prepared(entry(
        1,
        Geometry::LineStrip {
            points: vec![[0., 0., 0.], [2., 0., 0.], [2., 1., 0.]],
            colors: vec![],
        },
    ))
    else {
        panic!()
    };
    assert_eq!(width, 2.);
    assert!(strip);
    assert_eq!(segments.len(), 2);
    near(segments[1][1].position.0, [7., 23., 33.]);
    assert_eq!(segments[0][1].rgba, [0.2, 0.3, 0.4, 0.5]);
    let Body::Lines { segments, .. } = prepared(entry(
        1,
        Geometry::LineList {
            points: vec![[0.; 3], [1.; 3], [2.; 3], [3.; 3]],
            colors: vec![],
        },
    )) else {
        panic!()
    };
    assert_eq!(segments.len(), 2);
}
#[test]
fn triangles_scale_before_pose_and_multiply_vertex_alpha_only() {
    let Body::Triangles { triangles } = prepared(entry(
        1,
        Geometry::Triangles {
            points: vec![[0.; 3], [1., 0., 0.], [0., 1., 0.]],
            colors: vec![[1., 0., 0., 0.4]; 3],
        },
    )) else {
        panic!()
    };
    near(triangles[0][1].position.0, [8., 23., 33.]);
    near(triangles[0][2].position.0, [4., 21., 33.]);
    assert_eq!(triangles[0][0].rgba, [1., 0., 0., 0.2]);
}
#[test]
fn arrow_modes_follow_ros_x_and_clamp_endpoint_head_with_antiparallel_support() {
    let Body::Arrow {
        pose,
        shaft,
        head,
        shaft_diameters,
        head_diameters,
        ..
    } = prepared(entry(1, Geometry::Arrow { points: vec![] }))
    else {
        panic!()
    };
    assert_eq!(shaft, 1.54);
    assert_eq!(head, 0.46);
    assert_eq!(shaft_diameters, [4., 6.]);
    assert_eq!(head_diameters, [8., 12.]);
    near(pose.apply(Point3([2., 0., 0.])).0, [8., 23., 33.]);
    let mut e = entry(
        2,
        Geometry::Arrow {
            points: vec![[0.; 3], [-10., 0., 0.]],
        },
    );
    Arc::make_mut(&mut e.marker).scale = [1., 2., 0.];
    let Body::Arrow {
        pose, shaft, head, ..
    } = prepared(e)
    else {
        panic!()
    };
    assert!((shaft - 7.7).abs() < 1e-8);
    assert!((head - 2.3).abs() < 1e-8);
    near(pose.apply(Point3([10., 0., 0.])).0, [8., 11., 33.]);
    let Body::Arrow { shaft, head, .. } = prepared(entry(
        3,
        Geometry::Arrow {
            points: vec![[0.; 3], [1., 0., 0.]],
        },
    )) else {
        panic!()
    };
    assert_eq!(shaft, 0.);
    assert_eq!(head, 1.);
}
#[test]
fn lifetime_tf_invalid_and_bound_failures_stay_scoped_and_no_asset_is_opened() {
    let mut expired = entry(1, Geometry::Cube);
    expired.expires_at_monotonic_ns = Some(10);
    let mut missing = entry(2, Geometry::Sphere);
    missing.transform = Transform::Pending;
    let mut bounds = entry(3, Geometry::Cube);
    Arc::make_mut(&mut bounds.marker).pose[0] = 1_000_000.;
    let mut zero = entry(4, Geometry::Cube);
    Arc::make_mut(&mut zero.marker).scale[0] = 0.;
    let mesh = entry(
        5,
        Geometry::Mesh {
            resource: "file:///definitely-not-loaded.obj".into(),
            use_embedded_materials: true,
        },
    );
    let p = prepare(
        &[expired, missing, bounds, zero, mesh],
        10,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(p.entries.len(), 1);
    assert!(matches!(p.entries[0].body, Body::Mesh { .. }));
    assert_eq!(
        p.hidden.iter().map(|h| h.status).collect::<Vec<_>>(),
        vec![
            Status::Expired,
            Status::MissingTransform,
            Status::Bounds,
            Status::ZeroScale
        ]
    );
}
#[test]
fn empty_zero_text_defaults_and_unsupported_kind_are_explicit() {
    let mut text = entry(
        1,
        Geometry::Text {
            text: "two\nlines".into(),
        },
    );
    Arc::make_mut(&mut text.marker).scale[0] = 0.;
    let Body::Text {
        height,
        space_width,
        ..
    } = prepared(text)
    else {
        panic!()
    };
    assert_eq!(height, 6.);
    assert_eq!(space_width, None);
    let p = prepare(
        &[
            entry(
                2,
                Geometry::Points {
                    points: vec![],
                    colors: vec![],
                },
            ),
            entry(
                3,
                Geometry::ArrowStrip {
                    points: vec![],
                    colors: vec![],
                },
            ),
        ],
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        p.hidden.iter().map(|h| h.status).collect::<Vec<_>>(),
        vec![Status::Empty, Status::Unsupported]
    );
}
#[test]
fn maximum_lists_stay_instanced_and_aggregate_identity_and_cancel_guards_precede_work() {
    let entries = (0..16)
        .map(|id| {
            entry(
                id,
                Geometry::SphereList {
                    points: vec![[0.; 3]; 4096],
                    colors: vec![],
                },
            )
        })
        .collect::<Vec<_>>();
    let p = prepare(&entries, 0, &AtomicBool::new(false)).unwrap();
    assert_eq!(p.entries.len(), 16);
    let count = p
        .entries
        .iter()
        .map(|e| {
            if let Body::Instances { instances, .. } = &e.body {
                instances.len()
            } else {
                panic!()
            }
        })
        .sum::<usize>();
    assert_eq!(count, 65536);
    let mut excess = entries.clone();
    excess.push(entry(
        99,
        Geometry::Points {
            points: vec![[0.; 3]],
            colors: vec![],
        },
    ));
    assert!(prepare(&excess, 0, &AtomicBool::new(false))
        .unwrap_err()
        .contains("aggregate"));
    assert!(prepare(
        &[entries[0].clone(), entries[0].clone()],
        0,
        &AtomicBool::new(false)
    )
    .unwrap_err()
    .contains("identity"));
    assert!(prepare(&entries, 0, &AtomicBool::new(true))
        .unwrap_err()
        .contains("canceled"));
}

#[test]
fn nearly_antiparallel_arrow_keeps_the_endpoint_instead_of_snapping_direction() {
    let end = [-1000., 0.00001, 0.];
    let Body::Arrow {
        pose, shaft, head, ..
    } = prepared(entry(
        1,
        Geometry::Arrow {
            points: vec![[0.; 3], end],
        },
    ))
    else {
        panic!()
    };
    near(
        pose.apply(Point3([shaft + head, 0., 0.])).0,
        [7.99999, -979., 33.],
    );
}

#[test]
fn invalid_resolved_time_is_hidden_without_affecting_valid_peer() {
    let mut invalid = entry(1, Geometry::Cube);
    invalid.transform = Transform::Resolved {
        pose: LatestPose::Ready {
            resolved_stamp_ns: -1,
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
    };
    let p = prepare(
        &[invalid, entry(2, Geometry::Sphere)],
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(p.entries.len(), 1);
    assert_eq!(p.entries[0].key.id, 2);
    assert_eq!(p.hidden[0].status, Status::Invalid);
}

#[test]
fn canonical_arrow_modes_reach_projected_tf_endpoints_through_shared_bridge() {
    use crate::spatial::{
        marker_clock::MarkerClock,
        marker_primitives::{Frame, Kind},
        CameraPose,
    };
    let camera = CameraPose::orbit(0.4, 0.8, 50., [8., 21., 33.]);
    for (points, expected, head_only) in [
        (vec![], [8., 23., 33.], false),
        (vec![[0.; 3], [-10., 0., 0.]], [8., 11., 33.], false),
        (vec![[0.; 3], [1., 0., 0.]], [8., 22., 33.], true),
    ] {
        let mut e = entry(1, Geometry::Arrow { points });
        if !matches!(&e.marker.geometry,Geometry::Arrow{points} if points.is_empty()) && !head_only
        {
            Arc::make_mut(&mut e.marker).scale = [1., 2., 0.];
        }
        let body = prepared(e);
        let mut f = Frame::with_clock(MarkerClock::new());
        let n = f
            .push_geometry(&body, camera, [128, 128], Some(123), || false)
            .unwrap();
        assert_eq!(n, if head_only { 1 } else { 2 });
        assert_eq!(f.kind(n - 1), Some(Kind::Cone));
        let p = f.instances.last().unwrap();
        let tip = std::array::from_fn::<_, 4, _>(|i| p.columns[3][i] + p.columns[2][i] * 0.5);
        let expected = camera.clip(Point3(expected), 1.);
        for i in 0..4 {
            assert!(
                (tip[i] - expected[i]).abs() < 1e-4,
                "{tip:?} != {expected:?}"
            );
        }
        assert!(f.instances.iter().all(|p| p.shape[1..] == [123, 0, 1]));
    }
}

#[test]
fn unsupported_text_controls_hide_only_their_key_before_font_preparation() {
    let entries = [
        entry(
            1,
            Geometry::Text {
                text: "A\0B".into(),
            },
        ),
        entry(
            2,
            Geometry::Text {
                text: "A\n中🤖".into(),
            },
        ),
        entry(3, Geometry::Cube),
    ];
    let prepared = prepare(&entries, 0, &AtomicBool::new(false)).unwrap();
    assert_eq!(prepared.hidden.len(), 1);
    assert_eq!(prepared.hidden[0].key.id, 1);
    assert_eq!(prepared.hidden[0].status, Status::Invalid);
    assert_eq!(prepared.entries.len(), 2);
    let text = prepared
        .entries
        .iter()
        .filter_map(|e| matches!(e.body, Body::Text { .. }).then_some(&e.body))
        .collect::<Vec<_>>();
    assert!(crate::spatial::marker_text::prepare(&text, || false).is_ok());
}
