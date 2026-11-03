use super::*;
fn scan(ranges: &[f32], stamp: i64) -> Scan<'_> {
    Scan {
        frame: "laser",
        stamp_ns: stamp,
        clock: Clock::System,
        complete: true,
        angle_min: 0.0,
        angle_increment: std::f64::consts::FRAC_PI_2,
        range_min: 0.1,
        range_max: 10.0,
        ranges,
    }
}
fn tf(stamp: i64) -> TfOutcome<'static> {
    TfOutcome::Ready {
        source: "laser",
        target: "map",
        stamp_ns: stamp,
        transform: RigidTransform::identity(),
    }
}
fn scene() -> (Scene, LayerToken) {
    let mut scene = Scene::default();
    scene.set_fixed_frame("map").unwrap();
    scene.add_layer(1).unwrap();
    scene.start().unwrap();
    let g = scene.layer_token(1).unwrap();
    (scene, g)
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}
#[test]
fn rigid_transform_composition_preserves_ros_axes() {
    let q = (0.5_f64).sqrt();
    let rotation = RigidTransform::new([1.0, 2.0, 3.0], [0.0, 0.0, q, q]).unwrap();
    let p = rotation.apply(Point3([1.0, 0.0, 0.0])).0;
    for (a, b) in p.into_iter().zip([1.0, 3.0, 3.0]) {
        close(a, b);
    }
    let translation = RigidTransform::new([2.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]).unwrap();
    assert_eq!(
        rotation.compose(translation).apply(Point3([0.0; 3])),
        rotation.apply(translation.apply(Point3([0.0; 3])))
    );
    assert!(RigidTransform::new([0.0; 3], [0.0; 4]).is_none());
    assert!(RigidTransform::new([f64::NAN, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]).is_none());
}
#[test]
fn scan_geometry_skips_invalid_rays_without_origin_fallback() {
    let (mut s, g) = scene();
    s.update(
        g,
        scan(&[1.0, 2.0, f32::NAN, f32::INFINITY, 0.01, 11.0], 1),
        tf(1),
    )
    .unwrap();
    let p = s.layers()[0].points();
    assert_eq!(p.len(), 2);
    close(p[0].0[0], 1.0);
    close(p[1].0[1], 2.0);
    assert_eq!(s.layers()[0].skipped_rays, 4);
    assert_eq!(
        s.update(g, scan(&[1.0], 2), TfOutcome::Missing),
        Err(SceneError::MissingTf)
    );
    assert!(s.layers()[0].points().is_empty());
}
#[test]
fn tf_outcomes_and_identity_require_explicit_verified_lookup() {
    let (mut s, g) = scene();
    for (status, error) in [
        (TfOutcome::Stale, SceneError::StaleTf),
        (TfOutcome::Extrapolated, SceneError::ExtrapolatedTf),
        (TfOutcome::InvalidGraph, SceneError::InvalidTfGraph),
        (TfOutcome::CyclicGraph, SceneError::CyclicTfGraph),
        (TfOutcome::OutOfOrder, SceneError::OutOfOrder),
    ] {
        assert_eq!(s.update(g, scan(&[1.0], 1), status), Err(error));
        assert!(s.layers()[0].points().is_empty());
    }
    assert_eq!(
        s.update(g, scan(&[1.0], 1), tf(2)),
        Err(SceneError::TfMismatch)
    );
    let wrong = TfOutcome::Ready {
        source: "other",
        target: "map",
        stamp_ns: 1,
        transform: RigidTransform::identity(),
    };
    assert_eq!(
        s.update(g, scan(&[1.0], 1), wrong),
        Err(SceneError::TfMismatch)
    );
}
#[test]
fn clock_reset_stale_generation_and_stop_require_explicit_restart() {
    let (mut s, g) = scene();
    let mut simulated = scan(&[1.0], 1);
    simulated.clock = Clock::Simulated;
    assert_eq!(
        s.update(g, simulated, tf(1)),
        Err(SceneError::UnsupportedClock)
    );
    s.update(g, scan(&[1.0], 2), tf(2)).unwrap();
    assert_eq!(
        s.update(g, scan(&[1.0], 1), tf(1)),
        Err(SceneError::OutOfOrder)
    );
    let mut reset = scan(&[1.0], 3);
    reset.clock = Clock::Reset;
    assert_eq!(s.update(g, reset, tf(3)), Err(SceneError::ClockReset));
    assert_eq!(s.start(), Err(SceneError::ClockReset));
    s.stop();
    s.start().unwrap();
    let new = s.layer_token(1).unwrap();
    assert_eq!(
        s.update(g, scan(&[1.0], 4), tf(4)),
        Err(SceneError::StaleGeneration)
    );
    s.update(new, scan(&[1.0], 4), tf(4)).unwrap();
    s.stop();
    assert!(s.layers()[0].points().is_empty());
}
#[test]
fn caps_malformed_scan_layers_and_frame_changes() {
    let (mut s, g) = scene();
    for id in 2..=MAX_LAYERS as u64 {
        s.add_layer(id).unwrap();
    }
    assert_eq!(s.add_layer(5), Err(SceneError::LayerLimit));
    assert_eq!(s.add_layer(1), Err(SceneError::DuplicateLayer));
    let data = vec![1.0; MAX_POINTS + 1];
    assert_eq!(
        s.update(g, scan(&data, 1), tf(1)),
        Err(SceneError::PointLimit)
    );
    let mut incomplete = scan(&[1.0], 1);
    incomplete.complete = false;
    assert_eq!(
        s.update(g, incomplete, tf(1)),
        Err(SceneError::IncompleteScan)
    );
    for angle in [0.0, -1.0, f64::NAN] {
        let mut bad = scan(&[1.0], 1);
        bad.angle_increment = angle;
        assert_eq!(s.update(g, bad, tf(1)), Err(SceneError::InvalidScan));
    }
    s.update(g, scan(&[1.0], 1), tf(1)).unwrap();
    s.set_fixed_frame("odom").unwrap();
    assert!(!s.is_active());
    assert!(s.layers()[0].points().is_empty());
    assert_eq!(
        s.set_fixed_frame("bad frame"),
        Err(SceneError::InvalidFrame)
    );
    s.remove_layer(1);
    assert_eq!(s.layers().len(), 3);
}
#[test]
fn offline_projection_headless_draw_and_camera_bounds() {
    let (mut s, g) = scene();
    s.update(g, scan(&[1.0, 2.0, 3.0], 1), tf(1)).unwrap();
    let ctx = eframe::egui::Context::default();
    let mut camera = Camera {
        pixels_per_meter: f32::INFINITY,
        ..Camera::default()
    };
    let output = ctx.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| camera.show(ui, &s));
    });
    assert!(!output.shapes.is_empty());
    assert!(camera.pixels_per_meter.is_finite());
    let origin = eframe::egui::pos2(100.0, 100.0);
    let z = camera.project(Point3([0.0, 0.0, 1.0]), origin);
    assert!(z.y < origin.y);
    s.stop();
    let _ = ctx.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| camera.show(ui, &s));
    });
}
#[test]
fn verified_transform_is_applied_and_layer_removal_is_independent() {
    let (mut s, g) = scene();
    s.add_layer(2).unwrap();
    let g2 = s.layer_token(2).unwrap();
    let quarter = (0.5_f64).sqrt();
    let transformed = TfOutcome::Ready {
        source: "laser",
        target: "map",
        stamp_ns: 1,
        transform: RigidTransform::new([1.0, 2.0, 3.0], [0.0, 0.0, quarter, quarter]).unwrap(),
    };
    s.update(g, scan(&[1.0], 1), transformed).unwrap();
    let point = s.layers()[0].points()[0].0;
    for (a, b) in point.into_iter().zip([1.0, 3.0, 3.0]) {
        close(a, b);
    }
    let full = vec![1.0; MAX_POINTS];
    s.update(g2, scan(&full, 2), tf(2)).unwrap();
    s.remove_layer(1);
    assert_eq!(s.layers()[0].points().len(), MAX_POINTS);
    let wrong_target = TfOutcome::Ready {
        source: "laser",
        target: "odom",
        stamp_ns: 3,
        transform: RigidTransform::identity(),
    };
    assert_eq!(
        s.update(g2, scan(&[1.0], 3), wrong_target),
        Err(SceneError::TfMismatch)
    );
    let huge = TfOutcome::Ready {
        source: "laser",
        target: "map",
        stamp_ns: 4,
        transform: RigidTransform::new([1e7, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]).unwrap(),
    };
    assert_eq!(
        s.update(g2, scan(&[1.0], 4), huge),
        Err(SceneError::InvalidPoint)
    );
}
#[test]
fn removed_readded_id_rejects_old_worker_without_disturbing_other_layer() {
    let (mut s, g) = scene();
    s.add_layer(2).unwrap();
    let g2 = s.layer_token(2).unwrap();
    s.update(g, scan(&[1.0], 1), tf(1)).unwrap();
    s.update(g2, scan(&[2.0], 1), tf(1)).unwrap();
    let other = s.layers()[1].points().to_vec();
    s.remove_layer(1);
    s.add_layer(1).unwrap();
    assert!(
        s.update(g, scan(&[9.0], 2), tf(2)) == Err(SceneError::StaleLayer),
        "old worker repopulated re-added layer"
    );
    assert!(s
        .layers()
        .iter()
        .find(|l| l.id == 1)
        .unwrap()
        .points()
        .is_empty());
    assert_eq!(
        s.layers().iter().find(|l| l.id == 2).unwrap().points(),
        other
    );
    let replacement = s.layer_token(1).unwrap();
    assert_ne!(replacement, g);
    s.update(replacement, scan(&[3.0], 3), tf(3)).unwrap();
    let replacement_points = s
        .layers()
        .iter()
        .find(|l| l.id == 1)
        .unwrap()
        .points()
        .to_vec();
    assert_eq!(
        s.update(g, scan(&[9.0], 4), TfOutcome::Missing),
        Err(SceneError::StaleLayer)
    );
    assert_eq!(
        s.layers().iter().find(|l| l.id == 1).unwrap().points(),
        replacement_points
    );
    assert_eq!(
        s.layers().iter().find(|l| l.id == 2).unwrap().points(),
        other
    );
    s.update(g2, scan(&[4.0], 2), tf(2)).unwrap();
    for _ in 0..1000 {
        let old = s.layer_token(1).unwrap();
        s.remove_layer(1);
        s.add_layer(1).unwrap();
        assert_ne!(s.layer_token(1).unwrap(), old);
        assert_eq!(
            s.update(old, scan(&[1.0], 1), tf(1)),
            Err(SceneError::StaleLayer)
        );
        assert_eq!(s.layers().len(), 2);
    }
}
#[test]
fn accepted_near_unit_quaternion_preserves_composition_law() {
    let q = (1.0 + 5e-7_f64).sqrt();
    let t = RigidTransform::new([0.0; 3], [0.0, 0.0, q, 0.0]).unwrap();
    let p = Point3([1.0, 0.0, 0.0]);
    let composed = t.compose(t).apply(p);
    let sequential = t.apply(t.apply(p));
    for (a, b) in composed.0.into_iter().zip(sequential.0) {
        assert!((a - b).abs() < 1e-12, "composition law broke: {a} vs {b}");
    }
}

#[test]
fn layer_tokens_are_not_reused_across_scene_instances() {
    let (_, old) = scene();
    let (mut fresh, _) = scene();
    assert_eq!(
        fresh.update(old, scan(&[1.0], 1), tf(1)),
        Err(SceneError::StaleLayer)
    );
    assert!(fresh.layers()[0].points().is_empty());
    fresh.stop();
    assert_eq!(fresh.layer_token(1), Err(SceneError::Stopped));
}
