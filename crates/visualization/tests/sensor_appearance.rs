//! Projection/color/resource contracts, independent of native window/GPU state.
use studio_visualization::spatial::{
    combined::{
        combined_vertices, CombinedSceneFrame, PointLayerFrame, PointStyle,
        MAX_HISTORY_LAYER_POINTS,
    },
    sensor_appearance::{self, ColorMode, ColorRange, SensorAppearance, SizeMode},
    sensor_history::SensorPoint,
    CameraPose, Point3,
};
fn sample(position: [f64; 3]) -> SensorPoint {
    SensorPoint {
        position: Point3(position),
        rgba: None,
        intensity: None,
    }
}
fn layer<'a>(
    id: u64,
    samples: &'a [SensorPoint],
    appearance: &'a SensorAppearance,
) -> PointLayerFrame<'a> {
    PointLayerFrame {
        id,
        points: &[],
        samples: Some(samples),
        appearance: Some(appearance),
        style: PointStyle::for_id(id),
        status: "Receiving",
        error: None,
    }
}
fn frame<'a>(layers: &'a [PointLayerFrame<'a>]) -> CombinedSceneFrame<'a> {
    CombinedSceneFrame {
        fixed_frame: "map",
        layers,
        robot: None,
        map: None,
        pose: None,
        path: None,
        tf: None,
    }
}
#[test]
fn rgb_alpha_absence_and_extreme_intensity_ranges_are_finite_and_explicit() {
    let mut settings = SensorAppearance {
        color: ColorMode::Rgb,
        opacity: 128,
        ..Default::default()
    };
    let point = SensorPoint {
        rgba: Some([255, 0, 0, 64]),
        ..sample([0.; 3])
    };
    let color = sensor_appearance::color(&settings, point, None);
    assert_eq!(color[..3], [1., 0., 0.]);
    assert!((color[3] - (128. * 64. / (255. * 255.))).abs() < 1e-7);
    let absent = sensor_appearance::color(&settings, sample([0.; 3]), None);
    assert!(absent[0] > 0. && absent[0] < 1. && absent[1] > 0.);
    settings.color = ColorMode::Intensity;
    let points = [
        SensorPoint {
            intensity: Some(-f64::MAX),
            ..sample([0.; 3])
        },
        SensorPoint {
            intensity: Some(f64::MAX),
            ..sample([0.; 3])
        },
        sample([0.; 3]),
    ];
    let range = sensor_appearance::range(&settings, points.into_iter()).unwrap();
    assert_eq!(range.normalized(0.), 0.5);
    assert_eq!(range.normalized(-f64::MAX), 0.);
    assert_eq!(range.normalized(f64::MAX), 1.);
    for point in points {
        assert!(sensor_appearance::color(&settings, point, Some(range))
            .iter()
            .all(|n| n.is_finite()));
    }
    assert_eq!(
        ColorRange {
            minimum: 5.,
            maximum: 5.
        }
        .normalized(5.),
        0.5
    );
    settings.automatic_range = false;
    settings.intensity_milli = [-2500, 5000];
    assert_eq!(
        sensor_appearance::range(&settings, points.into_iter()),
        Some(ColorRange {
            minimum: -2.5,
            maximum: 5.
        })
    );
    settings.color = ColorMode::Height;
    settings.automatic_range = true;
    assert_eq!(
        sensor_appearance::range(
            &settings,
            [sample([0., 0., -3.]), sample([0., 0., 7.])].into_iter()
        ),
        Some(ColorRange {
            minimum: -3.,
            maximum: 7.
        })
    );
}
#[test]
fn pixel_diameter_is_depth_invariant_and_world_diameter_obeys_both_projections() {
    let camera = CameraPose::orbit(0., 0., 10., [0., 0., 3.]);
    let size = [800, 600];
    let mut settings = SensorAppearance {
        pixels: 8,
        diameter_mm: 1000,
        ..Default::default()
    };
    let width = |settings: &SensorAppearance, camera: CameraPose, point: Point3| {
        let p = camera.clip(point, 800. / 600.);
        sensor_appearance::half_extents(settings, camera, point, size, p)[0] / p[3] * 800.
    };
    let far = Point3([0., 0., 3.]);
    let near = Point3([5., 0., 3.]);
    assert!((width(&settings, camera, far) - 8.).abs() < 1e-4);
    assert!((width(&settings, camera, near) - 8.).abs() < 1e-4);
    settings.size_mode = SizeMode::World;
    assert!((width(&settings, camera, near) / width(&settings, camera, far) - 2.).abs() < 1e-4);
    let ortho = CameraPose::top_down(10., [0.; 3]);
    assert!((width(&settings, ortho, Point3([0.; 3])) - 60.).abs() < 1e-4);
    assert_eq!(
        width(&settings, ortho, Point3([0.; 3])),
        width(&settings, ortho, Point3([0., 0., 10.]))
    );
}
#[test]
fn explicit_history_limits_and_zero_alpha_preserve_legacy_validation_and_fit() {
    let mut settings = SensorAppearance::default();
    let camera = CameraPose::top_down(10., [0.; 3]);
    let samples = vec![sample([0., 0., 1.]); MAX_HISTORY_LAYER_POINTS];
    let layers = std::array::from_fn::<_, 4, _>(|i| layer(i as u64 + 1, &samples, &settings));
    let scene = frame(&layers);
    scene.validate().unwrap();
    assert_eq!(
        combined_vertices(&scene, camera, [64, 64]).unwrap().len(),
        4 * MAX_HISTORY_LAYER_POINTS * 6
    );
    let oversized = vec![sample([0.; 3]); MAX_HISTORY_LAYER_POINTS + 1];
    assert!(frame(&[layer(1, &oversized, &settings)])
        .validate()
        .is_err());
    let legacy = vec![Point3([0.; 3]); 8193];
    let mut old = layer(1, &[], &settings);
    old.samples = None;
    old.appearance = None;
    old.points = &legacy;
    assert!(frame(&[old]).validate().is_err());
    let mut inconsistent = layer(1, &samples, &settings);
    inconsistent.points = &legacy[..1];
    assert!(frame(&[inconsistent]).validate().is_err());
    settings.opacity = 0;
    let layers = [layer(1, &samples[..1], &settings)];
    let scene = frame(&layers);
    assert!(combined_vertices(&scene, camera, [64, 64])
        .unwrap()
        .is_empty());
    assert!(
        studio_visualization::spatial::camera_fit::visible_bounds(&scene, None, None).is_none()
    );
    let invalid = [SensorPoint {
        intensity: Some(f64::NAN),
        ..sample([0.; 3])
    }];
    assert!(frame(&[layer(1, &invalid, &settings)]).validate().is_err());
}
